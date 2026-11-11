use gpui::{
    Action as _, AnyElement, App, AppContext, ClickEvent, Context, Entity, EntityInputHandler as _,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, ParentElement as _, Render, RenderOnce, SharedString, Styled as _, Subscription,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_base::input::InputContextMenuCapabilities;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Copy, Cut, Editor, EditorState, Enter, Escape, IndentInline, Input, InputContentType,
    InputEvent, InputState, MoveDown, MoveUp, OutdentInline, Paste, SelectAll, Textarea,
    TextareaState,
};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable, Size};

use crate::input_focus::{preserve_nya_input_focus_on_pointer_down, register_nya_input_focus};
use crate::menu::{NyaContextMenu, NyaMenuItem};
use crate::theme_bridge::input_surface_color;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NyaInputEvent {
    Changed(String),
    Submitted(String),
    Blurred(String),
}

/// Semantic purpose stays stable while the user reveals or hides the value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NyaInputKind {
    #[default]
    Text,
    Password,
    NewPassword,
    Secret,
    OneTimeCode,
}

impl NyaInputKind {
    fn initially_masked(self) -> bool {
        matches!(self, Self::Password | Self::NewPassword | Self::Secret)
    }

    fn content_type(self) -> Option<InputContentType> {
        match self {
            Self::Password => Some(InputContentType::Password),
            Self::NewPassword => Some(InputContentType::NewPassword),
            Self::OneTimeCode => Some(InputContentType::OneTimeCode),
            Self::Text | Self::Secret => None,
        }
    }
}

type MaskToggleHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Application-owned text shared by all input presentations.
pub struct NyaInputLabels {
    show_secret: SharedString,
    hide_secret: SharedString,
}

impl NyaInputLabels {
    pub fn new(show_secret: impl Into<SharedString>, hide_secret: impl Into<SharedString>) -> Self {
        Self {
            show_secret: show_secret.into(),
            hide_secret: hide_secret.into(),
        }
    }

    fn mask_toggle_label(self, masked: bool) -> SharedString {
        if masked {
            self.show_secret
        } else {
            self.hide_secret
        }
    }
}

impl Default for NyaInputLabels {
    fn default() -> Self {
        Self::new("Show value", "Hide value")
    }
}

struct InputLabelsProvider(Box<dyn Fn() -> NyaInputLabels>);
impl gpui::Global for InputLabelsProvider {}

/// Vertical chrome a framed medium multi-line input adds around its editor
/// element: `input_py` padding above and below plus the 1px border on each
/// side (8 + 8 + 1 + 1).
const MULTI_LINE_FRAME_HEIGHT: f32 = 18.;

/// Resolve labels at render time so existing inputs follow language changes.
pub fn set_nya_input_label_provider(provider: impl Fn() -> NyaInputLabels + 'static, cx: &mut App) {
    cx.set_global(InputLabelsProvider(Box::new(provider)));
}

fn input_labels(cx: &App) -> NyaInputLabels {
    cx.try_global::<InputLabelsProvider>()
        .map(|provider| (provider.0)())
        .unwrap_or_default()
}

struct SecretInputPresentation {
    kind: NyaInputKind,
    masked: bool,
    disabled: bool,
}

gpui::actions!(nya_input, [ToggleMask]);

struct MaskToggleBindings;
impl gpui::Global for MaskToggleBindings {}

/// Keep all input presentations on the wrapper's authoritative mask state.
fn configure_secret_input(
    input: Input,
    owner: Entity<NyaInputState>,
    presentation: SecretInputPresentation,
    on_toggle: Option<MaskToggleHandler>,
    mut trailing: Vec<AnyElement>,
    cx: &App,
) -> Input {
    let SecretInputPresentation {
        kind,
        masked,
        disabled,
    } = presentation;
    let input = input.when_some(kind.content_type(), |input, kind| input.content_type(kind));
    if kind.initially_masked() || kind == NyaInputKind::OneTimeCode {
        let toggle = Rc::new(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                if owner.read(cx).disabled {
                    return;
                }
                if let Some(handler) = &on_toggle {
                    handler(event, window, cx);
                } else {
                    owner.update(cx, |state, cx| state.set_masked(!state.masked, cx));
                }
                window.refresh();
            },
        );
        let keyboard_toggle = toggle.clone();
        let label = input_labels(cx).mask_toggle_label(masked);
        trailing.insert(
            0,
            Button::new("nya-input-mask-toggle")
                .icon(if masked {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                })
                .xsmall()
                .text()
                .tab_stop(true)
                .debug_selector(|| "nya-input-mask-toggle".to_string())
                .accessibility_label(label.clone())
                .tooltip(label)
                .disabled(disabled)
                .key_context("NyaSecretToggle")
                .on_action(move |_: &ToggleMask, window, cx| {
                    // This binding takes precedence over a dialog's Enter-to-submit.
                    cx.stop_propagation();
                    if !disabled {
                        keyboard_toggle(&ClickEvent::default(), window, cx);
                    }
                })
                .on_click(move |event, window, cx| {
                    toggle(event, window, cx);
                })
                .into_any_element(),
        );
    }
    if trailing.is_empty() {
        input
    } else {
        input.suffix(div().flex().items_center().gap_1().children(trailing))
    }
}

#[derive(Clone)]
enum ComponentState {
    Input(Entity<InputState>),
    Textarea(Entity<TextareaState>),
    /// A code surface: the same editing engine as `Textarea`, plus the line-number
    /// gutter and soft-wrap control that a script box needs.
    Editor(Entity<EditorState>),
}

impl ComponentState {
    fn set_context_menu_enabled(&self, enabled: bool, cx: &mut App) {
        match self {
            Self::Input(state) => {
                state.update(cx, |state, _| state.set_context_menu_enabled(enabled))
            }
            Self::Textarea(state) => {
                state.update(cx, |state, _| state.set_context_menu_enabled(enabled))
            }
            Self::Editor(state) => {
                state.update(cx, |state, _| state.set_context_menu_enabled(enabled))
            }
        }
    }

    fn context_menu_capabilities(&self, cx: &App) -> InputContextMenuCapabilities {
        match self {
            Self::Input(state) => state.read(cx).context_menu_capabilities(),
            Self::Textarea(state) => state.read(cx).context_menu_capabilities(),
            Self::Editor(state) => state.read(cx).context_menu_capabilities(),
        }
    }

    fn value(&self, cx: &App) -> String {
        match self {
            Self::Input(state) => state.read(cx).value().to_string(),
            Self::Textarea(state) => state.read(cx).value().to_string(),
            Self::Editor(state) => state.read(cx).value().to_string(),
        }
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Input(state) => state.read(cx).focus_handle(cx),
            Self::Textarea(state) => state.read(cx).focus_handle(cx),
            Self::Editor(state) => state.read(cx).focus_handle(cx),
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Input(state) => state.update(cx, |state, cx| state.focus(window, cx)),
            Self::Textarea(state) => state.update(cx, |state, cx| state.focus(window, cx)),
            Self::Editor(state) => state.update(cx, |state, cx| state.focus(window, cx)),
        }
    }

    fn is_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        match self {
            Self::Input(state) => state.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }),
            Self::Textarea(state) => state.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }),
            Self::Editor(state) => state.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }),
        }
    }

    fn finish_composition(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Input(state) => state.update(cx, |state, cx| state.unmark_text(window, cx)),
            Self::Textarea(state) => state.update(cx, |state, cx| state.unmark_text(window, cx)),
            Self::Editor(state) => state.update(cx, |state, cx| state.unmark_text(window, cx)),
        }
    }

    fn set_value(&self, value: SharedString, window: &mut Window, cx: &mut App) {
        match self {
            Self::Input(state) => {
                state.update(cx, |state, cx| state.set_value(value.clone(), window, cx))
            }
            Self::Textarea(state) => {
                state.update(cx, |state, cx| state.set_value(value.clone(), window, cx))
            }
            Self::Editor(state) => {
                state.update(cx, |state, cx| state.set_value(value.clone(), window, cx))
            }
        }
    }

    fn select_all_with_cursor_at_end(&self, cx: &mut App) {
        let end = self.value(cx).len();
        match self {
            Self::Input(state) => {
                state.update(cx, |state, cx| state.set_selected_range(0..end, cx))
            }
            Self::Textarea(state) => {
                state.update(cx, |state, cx| state.set_selected_range(0..end, cx))
            }
            Self::Editor(state) => {
                state.update(cx, |state, cx| state.set_selected_range(0..end, cx))
            }
        }
    }

    fn move_cursor_to_end(&self, cx: &mut App) {
        let end = self.value(cx).len();
        match self {
            Self::Input(state) => {
                state.update(cx, |state, cx| state.set_selected_range(end..end, cx))
            }
            Self::Textarea(state) => {
                state.update(cx, |state, cx| state.set_selected_range(end..end, cx))
            }
            Self::Editor(state) => {
                state.update(cx, |state, cx| state.set_selected_range(end..end, cx))
            }
        }
    }
}

pub struct NyaInputState {
    state: Option<ComponentState>,
    seed: SharedString,
    pending_value: Option<SharedString>,
    silent_value: Option<SharedString>,
    placeholder: SharedString,
    placeholder_pending: bool,
    masked: bool,
    kind: NyaInputKind,
    applied_masked: bool,
    multi_line: bool,
    submit_on_enter: bool,
    code: bool,
    language: Option<SharedString>,
    rows: Option<usize>,
    auto_grow: Option<(usize, usize)>,
    bounded_auto_grow: bool,
    disabled: bool,
    readonly: bool,
    error: bool,
    max_chars: Option<usize>,
    focus: FocusHandle,
    focused: bool,
    subscription: Option<Subscription>,
}

impl NyaInputState {
    pub fn new(cx: &mut Context<Self>, seed: impl Into<SharedString>) -> Self {
        if !cx.has_global::<MaskToggleBindings>() {
            cx.set_global(MaskToggleBindings);
            cx.bind_keys([
                gpui::KeyBinding::new("enter", ToggleMask, Some("NyaSecretToggle")),
                gpui::KeyBinding::new("space", ToggleMask, Some("NyaSecretToggle")),
            ]);
        }
        Self {
            state: None,
            seed: seed.into(),
            pending_value: None,
            silent_value: None,
            placeholder: SharedString::default(),
            placeholder_pending: false,
            masked: false,
            kind: NyaInputKind::Text,
            applied_masked: false,
            multi_line: false,
            submit_on_enter: false,
            code: false,
            language: None,
            rows: None,
            auto_grow: None,
            bounded_auto_grow: false,
            disabled: false,
            readonly: false,
            error: false,
            max_chars: None,
            focus: cx.focus_handle(),
            focused: false,
            subscription: None,
        }
    }

    pub fn single_line(cx: &mut Context<Self>, seed: impl Into<SharedString>) -> Self {
        Self::new(cx, seed)
    }

    pub fn multi_line(mut self, rows: Option<usize>) -> Self {
        self.multi_line = true;
        self.rows = rows;
        self
    }

    /// A multi-line box whose height follows its content between `min_rows`
    /// and `max_rows`, scrolling beyond that. Its shell must not fix a height.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        self.multi_line = true;
        self.auto_grow = Some((min_rows.max(1), max_rows.max(min_rows.max(1))));
        self
    }

    /// [`Self::auto_grow`] for a field that already exists, applied to its
    /// mounted textarea as well.
    pub fn set_auto_grow(&mut self, min_rows: usize, max_rows: usize, cx: &mut Context<Self>) {
        let rows = (min_rows.max(1), max_rows.max(min_rows.max(1)));
        if !self.multi_line || self.code || self.auto_grow == Some(rows) {
            return;
        }
        self.auto_grow = Some(rows);
        if let Some(ComponentState::Textarea(state)) = &self.state
            && !self.bounded_auto_grow
        {
            state.update(cx, |state, cx| state.set_auto_grow(rows.0, rows.1, cx));
        }
        cx.notify();
    }

    /// Send plain Enter through the multi-line input; Shift+Enter inserts a newline.
    pub fn submit_on_enter(mut self, submit: bool) -> Self {
        self.submit_on_enter = submit;
        self
    }

    /// A multi-line box that is source code: line-number gutter, no soft wrap.
    pub fn code(mut self, rows: Option<usize>) -> Self {
        self.multi_line = true;
        self.code = true;
        self.rows = rows;
        self
    }

    /// Sets the syntax-highlighting language for a source-code editor.
    pub fn language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.placeholder_pending = true;
            cx.notify();
        }
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn kind(mut self, kind: NyaInputKind) -> Self {
        self.kind = kind;
        self.masked = kind.initially_masked();
        self
    }

    pub fn input_kind(&self) -> NyaInputKind {
        self.kind
    }

    pub fn hide_secret(&mut self, cx: &mut Context<Self>) {
        if self.kind != NyaInputKind::Text {
            self.set_masked(true, cx);
        }
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        if self.masked != masked {
            self.masked = masked;
            cx.notify();
        }
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            cx.notify();
        }
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        if self.readonly != readonly {
            self.readonly = readonly;
            cx.notify();
        }
    }

    pub fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }

    pub fn max_chars(mut self, max_chars: Option<usize>) -> Self {
        self.max_chars = max_chars;
        self
    }

    pub fn value(&self, cx: &App) -> String {
        if let Some(state) = &self.state {
            state.value(cx)
        } else if let Some(value) = &self.pending_value {
            value.to_string()
        } else {
            self.seed.to_string()
        }
    }

    /// Let adapters defer special key handling while an IME owns marked text.
    pub fn is_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.is_composing(window, cx))
    }

    pub fn set_content(&mut self, text: &str, cx: &mut Context<Self>) {
        self.pending_value = Some(SharedString::from(text.to_string()));
        cx.notify();
    }

    /// Queue a programmatic replacement without reporting it as a user edit.
    pub fn set_content_silent(&mut self, text: &str, cx: &mut Context<Self>) {
        let value = SharedString::from(text.to_string());
        self.silent_value = Some(value.clone());
        self.pending_value = Some(value);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_content("", cx);
    }

    /// Release a closing prompt's value and editing history even if it never
    /// renders again. `clear` queues a replacement for the next render instead.
    pub fn discard_content(&mut self, cx: &mut Context<Self>) {
        self.subscription = None;
        self.state = None;
        self.seed = SharedString::default();
        self.pending_value = None;
        self.silent_value = None;
        self.focused = false;
        self.hide_secret(cx);
        cx.notify();
    }

    pub fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.ensure_component(window, cx);
        state.focus(window, cx);
        window.dispatch_action(SelectAll.boxed_clone(), cx);
    }

    pub fn select_all_with_cursor_at_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.ensure_component(window, cx);
        state.focus(window, cx);
        state.select_all_with_cursor_at_end(cx);
    }

    pub fn focus_with_cursor_at_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.ensure_component(window, cx);
        state.focus(window, cx);
        state.move_cursor_to_end(cx);
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn component_focus_handle(&self, cx: &App) -> FocusHandle {
        self.state
            .as_ref()
            .map(|state| state.focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
    }

    pub fn has_focus(&self) -> bool {
        self.focused
    }

    pub fn component_state(&self) -> Option<Entity<InputState>> {
        match self.state.as_ref() {
            Some(ComponentState::Input(state)) => Some(state.clone()),
            _ => None,
        }
    }

    /// Height the auto-grow layout gives this textarea for its current
    /// content, frame included. Independent of any outer height constraint,
    /// so callers can clamp manual sizing against the "natural" box without
    /// reading back what their own constraint produced.
    pub fn auto_grow_height(&self, cx: &App) -> Option<gpui::Pixels> {
        let (min_rows, max_rows) = self.auto_grow?;
        let ComponentState::Textarea(state) = self.state.as_ref()? else {
            return None;
        };
        let state = state.read(cx);
        let line_height = state.line_height()?;
        let rows = state
            .display_map()
            .wrap_row_count()
            .clamp(min_rows, max_rows) as f32;
        (line_height > px(0.)).then(|| px(rows * f32::from(line_height) + MULTI_LINE_FRAME_HEIGHT))
    }

    /// One visible editing row, including the medium textarea's frame.
    pub fn textarea_min_height(&self, cx: &App) -> Option<gpui::Pixels> {
        let ComponentState::Textarea(state) = self.state.as_ref()? else {
            return None;
        };
        let line_height = state.read(cx).line_height()?;
        (line_height > px(0.)).then(|| line_height + px(MULTI_LINE_FRAME_HEIGHT))
    }

    fn set_bounded_auto_grow(&mut self, bounded: bool, cx: &mut Context<Self>) {
        let Some(rows) = self.auto_grow else {
            return;
        };
        if self.bounded_auto_grow == bounded {
            return;
        }
        self.bounded_auto_grow = bounded;
        if let Some(ComponentState::Textarea(state)) = &self.state {
            // The shell owns the constrained height. Leave the editor a one-row
            // minimum so its scroll viewport cannot overflow a shorter frame.
            // Natural sizing still reads the display map and the original rows.
            let (min, max) = if bounded { (1, 1) } else { rows };
            state.update(cx, |state, cx| state.set_auto_grow(min, max, cx));
        }
    }

    fn ensure_component(&mut self, window: &mut Window, cx: &mut Context<Self>) -> ComponentState {
        if let Some(state) = self.state.clone() {
            if std::mem::take(&mut self.placeholder_pending) {
                let placeholder =
                    component_placeholder(self.placeholder.clone(), self.multi_line, self.code);
                match &state {
                    ComponentState::Input(input) => {
                        input.update(cx, |input, cx| {
                            input.set_placeholder(placeholder, window, cx)
                        });
                    }
                    ComponentState::Textarea(input) => {
                        input.update(cx, |input, cx| {
                            input.set_placeholder(placeholder, window, cx)
                        });
                    }
                    ComponentState::Editor(input) => {
                        input.update(cx, |input, cx| {
                            input.set_placeholder(placeholder, window, cx)
                        });
                    }
                }
            }
            if let Some(value) = self.pending_value.take() {
                state.set_value(value, window, cx);
            }
            return state;
        }

        let value = self
            .pending_value
            .take()
            .unwrap_or_else(|| self.seed.clone());
        let masked = self.masked;
        let multi_line = self.multi_line;
        self.placeholder_pending = false;
        let placeholder = component_placeholder(self.placeholder.clone(), multi_line, self.code);
        let language = self.language.clone();
        // The shell supplies a definite height, including a resizable footer.
        // Kit 0.7.1 treats rows as a minimum and would force that viewport taller.
        let rows = self.rows.unwrap_or(1).min(1);
        let auto_grow = self.auto_grow;
        let submit_on_enter = self.submit_on_enter;
        let (state, subscription) = if multi_line && self.code {
            let state = cx.new(|cx| {
                let mut editor = EditorState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder);
                if let Some(language) = language {
                    editor = editor.language(language);
                }
                editor
                    // Source newlines define the script structure; wrapping would
                    // make gutter numbers stop matching the visible lines.
                    .soft_wrap(false)
            });
            let subscription = cx.subscribe(&state, |this, input, event: &InputEvent, cx| {
                forward_input_event(this, input.read(cx).value().to_string(), event, cx)
            });
            (ComponentState::Editor(state), subscription)
        } else if multi_line {
            let state = cx.new(|cx| {
                let state = TextareaState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
                    .submit_on_enter(submit_on_enter);
                match auto_grow {
                    Some((min_rows, max_rows)) => state.auto_grow(min_rows, max_rows),
                    None => state.rows(rows),
                }
            });
            let subscription = cx.subscribe(&state, |this, input, event: &InputEvent, cx| {
                forward_input_event(this, input.read(cx).value().to_string(), event, cx)
            });
            (ComponentState::Textarea(state), subscription)
        } else {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
                    .masked(masked)
            });
            let subscription = cx.subscribe(&state, |this, input, event: &InputEvent, cx| {
                forward_input_event(this, input.read(cx).value().to_string(), event, cx)
            });
            (ComponentState::Input(state), subscription)
        };
        register_nya_input_focus(&state.focus_handle(cx), cx);
        self.subscription = Some(subscription);
        self.state = Some(state.clone());
        self.applied_masked = masked;
        state
    }
}

fn forward_input_event(
    state: &mut NyaInputState,
    value: String,
    event: &InputEvent,
    cx: &mut Context<NyaInputState>,
) {
    match event {
        InputEvent::Change => {
            if state
                .silent_value
                .take()
                .is_some_and(|expected| expected.as_ref() == value)
            {
                return;
            }
            cx.emit(NyaInputEvent::Changed(value));
        }
        // Multi-line boxes also emit PressEnter for `Shift+Enter`, where the
        // component itself inserted the newline; only an unmodified Enter
        // means submit.
        InputEvent::PressEnter { shift, .. } if !shift => {
            cx.emit(NyaInputEvent::Submitted(value));
        }
        InputEvent::PressEnter { .. } => {}
        InputEvent::Focus => {
            state.focused = true;
            cx.notify();
        }
        InputEvent::Blur => {
            state.focused = false;
            cx.emit(NyaInputEvent::Blurred(value));
            cx.notify();
        }
    }
}

fn component_placeholder(placeholder: SharedString, multi_line: bool, code: bool) -> SharedString {
    // Code editors support separate placeholder lines alongside their line gutter.
    if code || !multi_line || (!placeholder.contains('\r') && !placeholder.contains('\n')) {
        return placeholder;
    }

    let mut normalized = String::with_capacity(placeholder.len());
    let mut in_line_break = false;
    for ch in placeholder.chars() {
        if matches!(ch, '\r' | '\n') {
            if !in_line_break {
                normalized.push(' ');
                in_line_break = true;
            }
        } else {
            normalized.push(ch);
            in_line_break = false;
        }
    }
    SharedString::from(normalized)
}

impl EventEmitter<NyaInputEvent> for NyaInputState {}

impl Focusable for NyaInputState {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.component_focus_handle(cx)
    }
}

impl Render for NyaInputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.ensure_component(window, cx);
        if let ComponentState::Input(input) = &state
            && self.applied_masked != self.masked
        {
            let masked = self.masked;
            input.update(cx, |state, cx| state.set_masked(masked, window, cx));
            self.applied_masked = masked;
        }
        let component_focus = state.focus_handle(cx);
        if self.focus.is_focused(window) && !component_focus.is_focused(window) {
            state.focus(window, cx);
        }
        self.focused = self.focus.is_focused(window) || component_focus.is_focused(window);
        match state {
            ComponentState::Input(state) => configure_secret_input(
                Input::new(&state)
                    .bg(input_surface_color(cx))
                    .disabled(self.disabled)
                    .readonly(self.readonly),
                cx.entity(),
                SecretInputPresentation {
                    kind: self.kind,
                    masked: self.masked,
                    disabled: self.disabled,
                },
                None,
                Vec::new(),
                cx,
            )
            .into_any_element(),
            ComponentState::Textarea(state) => Textarea::new(&state)
                .bg(input_surface_color(cx))
                .disabled(self.disabled)
                .readonly(self.readonly)
                .h_full()
                .into_any_element(),
            ComponentState::Editor(state) => Editor::new(&state)
                .disabled(self.disabled)
                .readonly(self.readonly)
                .h_full()
                .into_any_element(),
        }
    }
}

#[derive(IntoElement)]
pub struct NyaInput {
    state: Entity<NyaInputState>,
    font_size: Option<gpui::Pixels>,
    on_mask_toggle: Option<MaskToggleHandler>,
}

impl NyaInput {
    pub fn new(state: &Entity<NyaInputState>) -> Self {
        Self {
            state: state.clone(),
            font_size: None,
            on_mask_toggle: None,
        }
    }

    /// Override the compact component size for fields that follow the UI font.
    pub fn text_size(mut self, font_size: gpui::Pixels) -> Self {
        self.font_size = Some(font_size);
        self
    }

    pub fn on_mask_toggle(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_mask_toggle = Some(Box::new(handler));
        self
    }
}

// Both bare inputs and framed registry fields must guard actions before the
// component emits Submitted; a raw key callback runs after bound actions.
fn protect_input_composition<E: gpui::InteractiveElement>(
    element: E,
    state: ComponentState,
    owner: Entity<NyaInputState>,
) -> E {
    let enter_state = state.clone();
    let escape_state = state.clone();
    let enter_owner = owner.clone();
    element
        .capture_action(move |_: &Enter, window, cx| {
            if enter_state.is_composing(window, cx) {
                enter_state.finish_composition(window, cx);
                // Marked-text updates need not emit Change. Publish the confirmed
                // value so a surrounding search list can refresh before another key.
                let value = enter_state.value(cx);
                enter_owner.update(cx, |_, cx| cx.emit(NyaInputEvent::Changed(value)));
                cx.stop_propagation();
            }
        })
        .capture_action(move |_: &Escape, window, cx| {
            if escape_state.is_composing(window, cx) {
                escape_state.finish_composition(window, cx);
                let value = escape_state.value(cx);
                owner.update(cx, |_, cx| cx.emit(NyaInputEvent::Changed(value)));
                cx.stop_propagation();
            }
        })
        .capture_key_down(move |event, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "escape" | "tab")
                && state.is_composing(window, cx)
            {
                cx.stop_propagation();
            }
        })
}

impl RenderOnce for NyaInput {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (state, disabled, readonly, multi_line) =
            prepare_input_component(&self.state, window, cx);
        let kind = self.state.read(cx).kind;
        let masked = self.state.read(cx).masked;
        let focus_state = state.clone();
        let composition_state = state.clone();
        let input = match state {
            ComponentState::Input(state) => configure_secret_input(
                Input::new(&state)
                    .xsmall()
                    .when_some(self.font_size, |this, font_size| {
                        this.text_size(font_size)
                            .line_height(font_size * 1.5)
                            .h_full()
                    })
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .disabled(disabled)
                    .readonly(readonly),
                self.state.clone(),
                SecretInputPresentation {
                    kind,
                    masked,
                    disabled,
                },
                self.on_mask_toggle,
                Vec::new(),
                cx,
            )
            .into_any_element(),
            ComponentState::Textarea(state) => Textarea::new(&state)
                .appearance(false)
                .bordered(false)
                .disabled(disabled)
                .readonly(readonly)
                .h_full()
                .text_xs()
                .when_some(self.font_size, |this, font_size| this.text_size(font_size))
                .into_any_element(),
            ComponentState::Editor(state) => Editor::new(&state)
                .appearance(false)
                .bordered(false)
                .disabled(disabled)
                .readonly(readonly)
                .h_full()
                .text_xs()
                .when_some(self.font_size, |this, font_size| this.text_size(font_size))
                .into_any_element(),
        };
        protect_input_composition(
            div()
                .size_full()
                .bg(input_surface_color(cx))
                .when(!multi_line, |this| this.flex().items_center())
                .capture_any_mouse_down(|_, _, cx| {
                    preserve_nya_input_focus_on_pointer_down(cx);
                })
                .on_any_mouse_down(move |_, window, cx| {
                    if !disabled {
                        focus_state.focus(window, cx);
                    }
                })
                .child(input),
            composition_state,
            self.state,
        )
    }
}

type KeyDownHandler = Box<dyn Fn(&KeyDownEvent, &mut Window, &mut App) + 'static>;
type VerticalNavigationHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
type SecondaryEnterHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

fn prepare_input_component(
    input_state: &Entity<NyaInputState>,
    window: &mut Window,
    cx: &mut App,
) -> (ComponentState, bool, bool, bool) {
    let (state, disabled, readonly, multi_line, focused) = input_state.update(cx, |input, cx| {
        let state = input.ensure_component(window, cx);
        if let ComponentState::Input(component) = &state
            && input.applied_masked != input.masked
        {
            let masked = input.masked;
            component.update(cx, |component, cx| component.set_masked(masked, window, cx));
            input.applied_masked = masked;
        }
        (
            state,
            input.disabled,
            input.readonly,
            input.multi_line,
            input.focus.is_focused(window),
        )
    });
    if focused {
        state.focus(window, cx);
    }
    input_state.update(cx, |input_state, cx| {
        let component_focus = state.focus_handle(cx);
        input_state.focused =
            input_state.focus.is_focused(window) || component_focus.is_focused(window);
    });
    (state, disabled, readonly, multi_line)
}

#[derive(IntoElement)]
pub struct NyaInputShell {
    id: SharedString,
    state: Entity<NyaInputState>,
    size: Size,
    multi_line: bool,
    search: bool,
    framed: bool,
    height: Option<gpui::Pixels>,
    height_limits: Option<(gpui::Pixels, gpui::Pixels)>,
    fill_height: bool,
    trailing: Vec<AnyElement>,
    footer: Option<AnyElement>,
    on_key_down: Option<KeyDownHandler>,
    on_vertical_navigation: Option<VerticalNavigationHandler>,
    on_plain_enter: Option<SecondaryEnterHandler>,
    on_secondary_enter: Option<SecondaryEnterHandler>,
    tab_navigation: bool,
    context_menu_labels: Option<[SharedString; 4]>,
    on_mask_toggle: Option<MaskToggleHandler>,
}

impl NyaInputShell {
    pub fn new(id: impl Into<SharedString>, state: &Entity<NyaInputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            size: Size::Medium,
            multi_line: false,
            search: false,
            framed: true,
            height: None,
            height_limits: None,
            fill_height: false,
            trailing: Vec::new(),
            footer: None,
            on_key_down: None,
            on_vertical_navigation: None,
            on_plain_enter: None,
            on_secondary_enter: None,
            tab_navigation: false,
            context_menu_labels: None,
            on_mask_toggle: None,
        }
    }

    /// Use the 24px input size for controls embedded in compact rows.
    pub fn compact(mut self) -> Self {
        self.size = Size::Small;
        self
    }

    pub fn multi_line(mut self) -> Self {
        self.multi_line = true;
        self
    }

    /// Opt into the NyaTerm popup menu. Labels are Cut, Copy, Paste, Select All,
    /// localized by the caller. Other inputs keep the platform's default menu.
    pub fn gpui_context_menu(mut self, labels: [SharedString; 4]) -> Self {
        self.context_menu_labels = Some(labels);
        self
    }

    pub fn height(mut self, height: gpui::Pixels) -> Self {
        self.height = Some(height);
        self.height_limits = None;
        self.fill_height = false;
        self
    }

    /// Constrain an auto-growing textarea while preserving its natural row
    /// range. The ceiling wins over the floor, with at least one editing row.
    /// Like `height` and `fill_height`, the last sizing setter wins.
    pub fn height_limits(mut self, lower: gpui::Pixels, upper: gpui::Pixels) -> Self {
        self.height_limits = Some((lower.max(px(0.)), upper.max(px(0.))));
        self.height = None;
        self.fill_height = false;
        self
    }

    /// Fill a definite-height parent instead of using the default textarea height.
    pub fn fill_height(mut self) -> Self {
        self.height = None;
        self.height_limits = None;
        self.fill_height = true;
        self
    }

    fn search(mut self) -> Self {
        self.search = true;
        self
    }

    /// Remove the component chrome when a parent surface owns the frame.
    pub fn bare(mut self) -> Self {
        self.framed = false;
        self
    }

    pub fn trailing(mut self, child: impl IntoElement) -> Self {
        self.trailing.push(child.into_any_element());
        self
    }

    /// Override revealing a draft when the feature must first load a saved secret.
    pub fn on_mask_toggle(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_mask_toggle = Some(Box::new(handler));
        self
    }

    /// Add a fixed action row inside a textarea's frame, outside its scroll viewport.
    pub fn footer(mut self, child: impl IntoElement) -> Self {
        self.footer = Some(child.into_any_element());
        self
    }

    /// Route Up/Down actions to a choice list while retaining IME ownership.
    /// The callback receives true for Down and false for Up.
    pub fn on_vertical_navigation(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_vertical_navigation = Some(Rc::new(handler));
        self
    }

    pub fn on_key_down(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_key_down = Some(Box::new(handler));
        self
    }

    /// Consume Enter actions and call the handler only for an unmodified Enter.
    /// IME confirmation is handled first, without emitting Submitted to subscribers.
    pub fn on_plain_enter(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_plain_enter = Some(Box::new(handler));
        self
    }

    /// Take Ctrl/Cmd+Enter before the field sees it. A multi-line field binds
    /// that keystroke to a newline and stops propagation, so an `on_key_down`
    /// on an ancestor never receives it.
    pub fn on_secondary_enter(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_secondary_enter = Some(Box::new(handler));
        self
    }

    /// Keep Tab/Shift+Tab as form navigation instead of code indentation.
    pub fn tab_navigation(mut self) -> Self {
        self.tab_navigation = true;
        self
    }
}

impl RenderOnce for NyaInputShell {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let NyaInputShell {
            id,
            state,
            size,
            multi_line,
            search,
            framed,
            mut height,
            height_limits,
            fill_height,
            trailing,
            footer,
            on_key_down,
            on_vertical_navigation,
            on_plain_enter,
            on_secondary_enter,
            tab_navigation,
            context_menu_labels,
            on_mask_toggle,
        } = self;
        let owner = state.clone();
        let kind = owner.read(cx).kind;
        let masked = owner.read(cx).masked;
        let (state, disabled, readonly, state_multi_line) =
            prepare_input_component(&state, window, cx);
        let height_limits = height_limits.filter(|_| {
            owner.read(cx).auto_grow.is_some() && matches!(&state, ComponentState::Textarea(_))
        });
        owner.update(cx, |input, cx| {
            input.set_bounded_auto_grow(height_limits.is_some(), cx);
        });
        if let Some((lower, upper)) = height_limits {
            let input = owner.read(cx);
            let minimum = input.textarea_min_height(cx).unwrap_or(px(38.));
            let natural = input.auto_grow_height(cx).unwrap_or_else(|| {
                // Before the first text layout, preserve the configured row
                // minimum rather than briefly painting a one-row prompt.
                let rows = input.auto_grow.map_or(1, |(minimum, _)| minimum);
                (minimum - px(MULTI_LINE_FRAME_HEIGHT)) * rows as f32 + px(MULTI_LINE_FRAME_HEIGHT)
            });
            height = Some(natural.max(lower).min(upper.max(minimum)));
        }
        let content_height = owner.read(cx).auto_grow.is_some() && height.is_none() && !fill_height;
        state.set_context_menu_enabled(context_menu_labels.is_none(), cx);
        let composition_state = state.clone();
        let menu_state = state.clone();
        let capture_focus_state = state.clone();
        let gpui_context_menu = context_menu_labels.is_some();
        let focus_state = state.clone();
        let enter_state = state.clone();
        let tab_state = state.clone();
        let shift_tab_state = state.clone();
        let debug_selector = id.to_string();
        let prefix_debug_selector = format!("{}-prefix", id);
        let has_footer = footer.is_some();
        let input_surface = input_surface_color(cx);
        let input = match state {
            ComponentState::Input(state) => {
                let mut input = Input::new(&state)
                    .with_size(size)
                    .appearance(framed)
                    .bordered(framed)
                    .focus_bordered(framed)
                    .disabled(disabled)
                    .readonly(readonly)
                    .when(framed, |this| this.bg(input_surface));
                if multi_line || state_multi_line {
                    input = input.h(px(88.));
                }
                if search {
                    input = input.prefix(
                        div()
                            .debug_selector(move || prefix_debug_selector.clone())
                            .flex()
                            .items_center()
                            .child(Icon::new(IconName::Search).small()),
                    );
                }
                input = configure_secret_input(
                    input,
                    owner.clone(),
                    SecretInputPresentation {
                        kind,
                        masked,
                        disabled,
                    },
                    on_mask_toggle,
                    trailing,
                    cx,
                );
                input.into_any_element()
            }
            ComponentState::Textarea(state) if has_footer => {
                let theme = cx.theme();
                let focused = state.read(cx).focus_handle(cx).is_focused(window);
                div()
                    .w_full()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .h(if fill_height {
                        gpui::relative(1.)
                    } else {
                        height.unwrap_or(px(88.)).into()
                    })
                    .when(framed, |this| {
                        this.rounded(theme.radius)
                            .border_1()
                            .border_color(if focused { theme.ring } else { theme.input })
                            .bg(input_surface)
                    })
                    .child(
                        div().flex_1().min_h_0().min_w_0().overflow_hidden().child(
                            Textarea::new(&state)
                                .appearance(false)
                                .bordered(false)
                                .disabled(disabled)
                                .readonly(readonly)
                                .h_full(),
                        ),
                    )
                    .child(footer.unwrap())
                    .into_any_element()
            }
            ComponentState::Textarea(state) if content_height => Textarea::new(&state)
                .appearance(framed)
                .bordered(framed)
                .disabled(disabled)
                .readonly(readonly)
                .when(framed, |this| this.bg(input_surface))
                .into_any_element(),
            ComponentState::Textarea(state) => Textarea::new(&state)
                .appearance(framed)
                .bordered(framed)
                .disabled(disabled)
                .readonly(readonly)
                .when(framed, |this| this.bg(input_surface))
                .h(if fill_height {
                    gpui::relative(1.)
                } else {
                    height.unwrap_or(px(88.)).into()
                })
                .into_any_element(),
            ComponentState::Editor(state) => Editor::new(&state)
                .disabled(disabled)
                .readonly(readonly)
                // A script box needs more than the two-line note height a textarea
                // gets; the gutter makes short boxes read as cramped.
                .h(if fill_height {
                    gpui::relative(1.)
                } else {
                    height.unwrap_or(px(168.)).into()
                })
                .text_size(px(14.))
                .into_any_element(),
        };

        let mut container = div()
            .id(id)
            .debug_selector(move || debug_selector.clone())
            .w_full()
            .min_w_0()
            .relative()
            .when(fill_height, |this| this.h_full().min_h_0())
            .capture_any_mouse_down(move |event, window, cx| {
                preserve_nya_input_focus_on_pointer_down(cx);
                // The input menu opens during bubble dispatch, after this capture
                // handler establishes the input as its dismissal focus target.
                if gpui_context_menu && !disabled && event.button == MouseButton::Right {
                    capture_focus_state.focus(window, cx);
                }
            })
            .when(!has_footer, |this| {
                this.on_any_mouse_down(move |_, window, cx| {
                    if !disabled {
                        focus_state.focus(window, cx);
                    }
                })
            });
        container = protect_input_composition(container, composition_state, owner.clone());
        if let Some(handler) = on_plain_enter {
            container = container.capture_action(move |action: &Enter, window, cx| {
                cx.stop_propagation();
                let modifiers = window.modifiers();
                if !action.shift
                    && !action.secondary
                    && !modifiers.shift
                    && !modifiers.platform
                    && !modifiers.control
                    && !modifiers.alt
                    && !modifiers.function
                {
                    handler(window, cx);
                }
            });
        }
        if let Some(handler) = on_vertical_navigation {
            let up_state = owner.clone();
            let down_state = owner.clone();
            let up_handler = handler.clone();
            container = container
                .capture_action(move |_: &MoveUp, window, cx| {
                    if !up_state.update(cx, |state, cx| state.is_composing(window, cx)) {
                        cx.stop_propagation();
                        up_handler(&false, window, cx);
                    }
                })
                .capture_action(move |_: &MoveDown, window, cx| {
                    if !down_state.update(cx, |state, cx| state.is_composing(window, cx)) {
                        cx.stop_propagation();
                        handler(&true, window, cx);
                    }
                });
        }
        if let Some(handler) = on_key_down {
            container = container.on_key_down(handler);
        }
        if let Some(handler) = on_secondary_enter {
            container = container.capture_action(move |action: &Enter, window, cx| {
                if action.secondary {
                    cx.stop_propagation();
                    if enter_state.is_composing(window, cx) {
                        enter_state.finish_composition(window, cx);
                    } else {
                        handler(window, cx);
                    }
                }
            });
        }
        if tab_navigation {
            container = container
                .capture_action(move |_: &IndentInline, window, cx| {
                    cx.stop_propagation();
                    if !tab_state.is_composing(window, cx) {
                        window.focus_next(cx);
                    }
                })
                .capture_action(move |_: &OutdentInline, window, cx| {
                    cx.stop_propagation();
                    if !shift_tab_state.is_composing(window, cx) {
                        window.focus_prev(cx);
                    }
                });
        }
        let container = container
            .child(input)
            .when_some(height_limits, |this, (lower, upper)| {
                let measured_owner = owner.clone();
                this.child(
                    gpui::canvas(
                        move |bounds, _, cx| {
                            let input = measured_owner.read(cx);
                            let Some(natural) = input.auto_grow_height(cx) else {
                                return;
                            };
                            let minimum = input.textarea_min_height(cx).unwrap_or(px(38.));
                            let desired = natural.max(lower).min(upper.max(minimum));
                            if (desired - bounds.size.height).abs() <= px(0.5) {
                                return;
                            }
                            // Wrapping/font metrics become available in prepaint.
                            // Notify the owner after measurement, never during it.
                            cx.defer(move |cx| {
                                measured_owner.update(cx, |_, cx| cx.notify());
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
            });
        if let Some(labels) = context_menu_labels {
            NyaContextMenu::new_dynamic(container, move |_, cx| {
                let capabilities = menu_state.context_menu_capabilities(cx);
                let focus = menu_state.focus_handle(cx);
                let editable = capabilities.is_editable();
                let copyable = capabilities.is_copyable();
                let actions: [Box<dyn gpui::Action>; 4] = [
                    Box::new(Cut),
                    Box::new(Copy),
                    Box::new(Paste),
                    Box::new(SelectAll),
                ];
                let enabled = [
                    editable && copyable,
                    copyable,
                    editable && cx.read_from_clipboard().is_some(),
                    true,
                ];
                let mut items = Vec::with_capacity(5);
                for (index, ((label, action), enabled)) in
                    labels.iter().zip(actions).zip(enabled).enumerate()
                {
                    if index == 3 {
                        items.push(NyaMenuItem::separator());
                    }
                    let focus = focus.clone();
                    items.push(
                        NyaMenuItem::action(label.clone())
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                // PopupMenu can initially select a disabled first row.
                                // Keep keyboard confirmation under the same policy as clicks.
                                if enabled {
                                    focus.dispatch_action(action.as_ref(), window, cx);
                                }
                            }),
                    );
                }
                items
            })
            .bubble_right_click()
            .min_width(px(144.))
            .enabled(!disabled)
            .into_any_element()
        } else {
            container.into_any_element()
        }
    }
}

#[derive(IntoElement)]
pub struct NyaSearchInput {
    shell: NyaInputShell,
}

impl NyaSearchInput {
    pub fn new(id: impl Into<SharedString>, state: &Entity<NyaInputState>) -> Self {
        Self {
            shell: NyaInputShell::new(id, state).search(),
        }
    }

    pub fn trailing(mut self, child: impl IntoElement) -> Self {
        self.shell = self.shell.trailing(child);
        self
    }

    /// Use the 24px input size inside dense toolbars and overlays.
    pub fn compact(mut self) -> Self {
        self.shell = self.shell.compact();
        self
    }

    /// Remove the input frame when the containing overlay draws it.
    pub fn bare(mut self) -> Self {
        self.shell = self.shell.bare();
        self
    }

    pub fn on_key_down(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.shell = self.shell.on_key_down(handler);
        self
    }
}

impl RenderOnce for NyaSearchInput {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.shell
    }
}

pub type NyaTextArea = NyaInput;

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AppContext as _, ClipboardItem, Focusable as _, InteractiveElement as _, IntoElement,
        MouseButton, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
        div, point, prelude::FluentBuilder as _, px,
    };

    use gpui_kit::component::highlighter::LanguageRegistry;

    use super::{ComponentState, NyaInputShell, NyaInputState, component_placeholder};

    struct SecretInputFixture {
        field: gpui::Entity<NyaInputState>,
        presentation: u8,
        changes: Rc<Cell<usize>>,
        custom_toggles: Rc<Cell<usize>>,
        custom: bool,
        _subscription: gpui::Subscription,
    }

    impl SecretInputFixture {
        fn new(presentation: u8, cx: &mut gpui::Context<Self>) -> Self {
            let field = cx.new(|cx| {
                NyaInputState::new(cx, "fixture secret").kind(super::NyaInputKind::Password)
            });
            let changes = Rc::new(Cell::new(0));
            let change_count = changes.clone();
            let subscription = cx.subscribe(&field, move |_, _, event, _| {
                if matches!(event, super::NyaInputEvent::Changed(_)) {
                    change_count.set(change_count.get() + 1);
                }
            });
            Self {
                field,
                presentation,
                changes,
                custom_toggles: Rc::new(Cell::new(0)),
                custom: false,
                _subscription: subscription,
            }
        }
    }

    impl Render for SecretInputFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let input = match self.presentation {
                0 => {
                    let shell = NyaInputShell::new("secret-input", &self.field).trailing(
                        div()
                            .debug_selector(|| "secret-trailing".into())
                            .child("suffix"),
                    );
                    if self.custom {
                        let count = self.custom_toggles.clone();
                        shell
                            .on_mask_toggle(move |_, _, _| count.set(count.get() + 1))
                            .into_any_element()
                    } else {
                        shell.into_any_element()
                    }
                }
                1 => super::NyaInput::new(&self.field).into_any_element(),
                _ => self.field.clone().into_any_element(),
            };
            div().w(px(320.)).h(px(36.)).child(input)
        }
    }

    #[gpui::test]
    fn secret_toggle_preserves_value_selection_and_mask_state_in_every_presentation(
        cx: &mut TestAppContext,
    ) {
        use gpui::EntityInputHandler as _;
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| SecretInputFixture::new(0, cx));
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        for presentation in 0..3 {
            fixture.update(cx, |fixture, cx| {
                fixture.presentation = presentation;
                cx.notify();
            });
            cx.update(|window, cx| {
                field.update(cx, |field, cx| field.set_masked(true, cx));
                _ = window.draw(cx);
                field
                    .read(cx)
                    .component_state()
                    .unwrap()
                    .update(cx, |state, cx| {
                        state.set_selected_range(2..7, cx);
                        state.focus(window, cx);
                    });
            });
            cx.run_until_parked();
            let button = cx
                .debug_bounds("nya-input-mask-toggle")
                .expect("eye button");
            if presentation == 0 {
                assert!(cx.debug_bounds("secret-trailing").is_some());
            }
            cx.simulate_click(button.center(), Default::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                _ = window.draw(cx);
                assert!(!field.read(cx).is_masked());
                assert_eq!(field.read(cx).value(cx), "fixture secret");
                let selection = field
                    .read(cx)
                    .component_state()
                    .unwrap()
                    .update(cx, |state, cx| {
                        state.selected_text_range(false, window, cx).unwrap().range
                    });
                assert_eq!(selection, 2..7);
                assert_eq!(fixture.read(cx).changes.get(), 0);
            });
            // The button remains present after revealing; a second click hides again.
            let button = cx.debug_bounds("nya-input-mask-toggle").unwrap();
            cx.simulate_click(button.center(), Default::default());
            cx.run_until_parked();
            assert!(field.read_with(cx, |state, _| state.is_masked()));
        }
    }

    #[gpui::test]
    fn secret_toggle_is_keyboard_accessible_and_disabled_with_its_input(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| SecretInputFixture::new(0, cx));
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            _ = window.draw(cx);
            window.focus(&field.read(cx).component_focus_handle(cx), cx);
            window.focus_next(cx);
            _ = window.draw(cx);
            assert_ne!(
                window.focused(cx),
                Some(field.read(cx).component_focus_handle(cx)),
                "Tab should reach the reveal button"
            );
        });
        let key = gpui::Keystroke::parse("enter").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: key.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent { keystroke: key });
        cx.run_until_parked();
        assert!(!field.read_with(cx, |field, _| field.is_masked()));
        cx.update(|window, cx| {
            field.update(cx, |field, cx| {
                field.set_disabled(true, cx);
                field.hide_secret(cx);
            });
            window.refresh();
            _ = window.draw(cx);
        });
        let button = cx.debug_bounds("nya-input-mask-toggle").unwrap();
        cx.simulate_click(button.center(), Default::default());
        cx.run_until_parked();
        assert!(field.read_with(cx, |field, _| field.is_masked()));
    }

    #[gpui::test]
    fn custom_secret_toggle_does_not_reveal_placeholders_or_change_the_draft(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| SecretInputFixture::new(0, cx));
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        fixture.update(cx, |fixture, cx| {
            fixture.custom = true;
            cx.notify();
        });
        cx.update(|window, cx| {
            field.update(cx, |field, cx| {
                field.clear(cx);
                field.set_placeholder("••••••••", cx);
            });
            _ = window.draw(cx);
        });
        let button = cx.debug_bounds("nya-input-mask-toggle").unwrap();
        cx.simulate_click(button.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            fixture.read_with(cx, |fixture, _| fixture.custom_toggles.get()),
            1
        );
        assert!(field.read_with(cx, |field, _| field.is_masked()));
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "");
    }

    #[test]
    fn secret_semantics_are_independent_of_masking() {
        use super::NyaInputKind;
        assert!(NyaInputKind::Password.content_type().is_some());
        assert!(NyaInputKind::NewPassword.content_type().is_some());
        assert!(NyaInputKind::Secret.content_type().is_none());
    }

    #[gpui::test]
    fn application_labels_update_without_recreating_or_remasking_inputs(cx: &mut TestAppContext) {
        let translated = Rc::new(Cell::new(false));
        let current_language = translated.clone();
        cx.update(|cx| {
            super::set_nya_input_label_provider(
                move || {
                    if current_language.get() {
                        super::NyaInputLabels::new("显示内容", "隐藏内容")
                    } else {
                        super::NyaInputLabels::default()
                    }
                },
                cx,
            );
        });
        let field = cx
            .new(|cx| NyaInputState::new(cx, "fixture secret").kind(super::NyaInputKind::Password));
        field.update(cx, |field, cx| field.set_masked(false, cx));
        for (language, show, hide) in [
            (false, "Show value", "Hide value"),
            (true, "显示内容", "隐藏内容"),
        ] {
            translated.set(language);
            cx.update(|cx| {
                assert_eq!(super::input_labels(cx).mask_toggle_label(true), show);
                assert_eq!(super::input_labels(cx).mask_toggle_label(false), hide);
                assert!(!field.read(cx).is_masked());
                assert_eq!(field.read(cx).value(cx), "fixture secret");
            });
        }
    }

    struct InputHeightFixture {
        height: f32,
        footer: bool,
        flexible: gpui::Entity<NyaInputState>,
        fixed: gpui::Entity<NyaInputState>,
        ordinary: gpui::Entity<NyaInputState>,
    }

    impl Render for InputHeightFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(320.))
                .flex()
                .flex_col()
                .child(
                    div().h(px(self.height)).flex_none().child(
                        NyaInputShell::new("flexible-textarea", &self.flexible)
                            .multi_line()
                            .fill_height()
                            .when(self.footer, |this| {
                                this.footer(
                                    div()
                                        .id("textarea-footer")
                                        .debug_selector(|| "textarea-footer".into())
                                        .h(px(34.))
                                        .flex_none()
                                        .child("Send"),
                                )
                            }),
                    ),
                )
                .child(NyaInputShell::new("fixed-textarea", &self.fixed).multi_line())
                .child(NyaInputShell::new("ordinary-input", &self.ordinary))
        }
    }

    #[gpui::test]
    fn textarea_fills_resized_parent_without_changing_default_input_heights(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| InputHeightFixture {
            height: 68.,
            footer: false,
            flexible: cx.new(|cx| NyaInputState::new(cx, "draft\nsecond line").multi_line(Some(4))),
            fixed: cx.new(|cx| NyaInputState::new(cx, "fixed").multi_line(Some(4))),
            ordinary: cx.new(|cx| NyaInputState::new(cx, "ordinary")),
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.flexible.clone());
        for (height, footer) in [
            (68., false),
            (128., false),
            (468., false),
            (68., false),
            (68., true),
            (128., true),
            (468., true),
            (68., true),
        ] {
            fixture.update(cx, |fixture, cx| {
                fixture.height = height;
                fixture.footer = footer;
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
            assert_eq!(
                cx.debug_bounds("flexible-textarea").unwrap().size.height,
                px(height),
            );
            assert_eq!(
                cx.debug_bounds("fixed-textarea").unwrap().size.height,
                px(88.)
            );
            assert_eq!(
                cx.debug_bounds("ordinary-input").unwrap().size.height,
                px(32.)
            );
            let footer_bounds = footer.then(|| cx.debug_bounds("textarea-footer").unwrap());
            field.read_with(cx, |field, cx| {
                let Some(ComponentState::Textarea(component)) = field.state.as_ref() else {
                    panic!("expected a rendered textarea");
                };
                // Check the editing engine, not just the shell hit target.
                let bounds = component.read(cx).input_bounds();
                let editing_height = height - if footer { 34. } else { 0. };
                assert!(bounds.size.height > px((editing_height - 24.).max(0.)));
                assert!(bounds.size.height <= px(height));
                if let Some(footer_bounds) = footer_bounds {
                    assert_eq!(footer_bounds.size.height, px(34.));
                    assert!(
                        bounds.bottom() <= footer_bounds.origin.y,
                        "height {height}: editing bounds {bounds:?}, footer {footer_bounds:?}"
                    );
                    assert!(footer_bounds.bottom() <= px(height));
                }
                assert_eq!(field.value(cx), "draft\nsecond line");
            });
        }
    }

    struct AutoGrowFixture {
        field: gpui::Entity<NyaInputState>,
    }

    struct BoundedAutoGrowFixture {
        field: gpui::Entity<NyaInputState>,
        limits: Option<(gpui::Pixels, gpui::Pixels)>,
        width: gpui::Pixels,
        _observation: gpui::Subscription,
    }

    impl Render for BoundedAutoGrowFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(self.width).child(
                NyaInputShell::new("bounded-auto-grow", &self.field)
                    .multi_line()
                    .when_some(self.limits, |this, (lower, upper)| {
                        this.height_limits(lower, upper)
                    }),
            )
        }
    }

    #[gpui::test]
    fn bounded_auto_grow_preserves_natural_growth_and_the_real_editing_viewport(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            let field = cx.new(|cx| NyaInputState::new(cx, "one").auto_grow(3, 6));
            let observation = cx.observe(&field, |_, _, cx| cx.notify());
            BoundedAutoGrowFixture {
                field,
                limits: Some((px(110.), px(480.))),
                width: px(320.),
                _observation: observation,
            }
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        let draw = |cx: &mut VisualTestContext| {
            for _ in 0..4 {
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.refresh();
                    _ = window.draw(cx);
                });
            }
            cx.debug_bounds("bounded-auto-grow").unwrap()
        };
        assert_eq!(draw(cx).size.height, px(110.));
        field.update(cx, |field, cx| field.set_content("1\n2\n3\n4\n5\n6", cx));
        let six = draw(cx).size.height;
        assert!(six > px(110.));
        assert_eq!(
            field.read_with(cx, |field, cx| field.auto_grow_height(cx)),
            Some(six)
        );
        field.update(cx, |field, cx| field.set_content("one", cx));
        assert_eq!(draw(cx).size.height, px(110.));

        let text = vec!["long line"; 30].join("\n");
        field.update(cx, |field, cx| field.set_content(&text, cx));
        draw(cx);
        let component = field.read_with(cx, |field, _| {
            let Some(ComponentState::Textarea(component)) = &field.state else {
                panic!("textarea must be mounted");
            };
            component.clone()
        });
        cx.update(|window, cx| {
            component.update(cx, |state, cx| {
                state.set_selected_range(0..4, cx);
                state.set_scroll_offset(point(px(0.), px(-100.)), cx);
            });
            let focus = component.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
        for upper in [px(60.), px(38.), px(180.)] {
            fixture.update(cx, |fixture, cx| {
                fixture.limits = Some((px(110.), upper));
                cx.notify();
            });
            let shell = draw(cx);
            let expected = six.max(px(110.)).min(upper);
            assert_eq!(shell.size.height, expected);
            component.read_with(cx, |state, _| {
                let editing = state.input_bounds();
                assert!(editing.top() >= shell.top());
                assert!(
                    editing.bottom() <= shell.bottom(),
                    "editor {editing:?} must fit shell {shell:?}"
                );
                assert!(editing.size.height > px(0.));
                assert_eq!(state.selected_range(), 0..4);
                assert_eq!(state.value().as_str(), text);
                assert!(
                    state.scroll_offset().y < px(0.),
                    "overflowing text must have a scrollable viewport"
                );
            });
            cx.update(|window, cx| assert!(component.read(cx).focus_handle(cx).is_focused(window)));
        }

        fixture.update(cx, |fixture, cx| {
            fixture.limits = None;
            cx.notify();
        });
        assert_eq!(
            draw(cx).size.height,
            six,
            "removing limits restores the six-row natural height"
        );
    }

    #[gpui::test]
    fn bounded_auto_grow_recomputes_wrapping_after_width_changes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            let field = cx.new(|cx| NyaInputState::new(cx, "words ".repeat(25)).auto_grow(3, 6));
            let observation = cx.observe(&field, |_, _, cx| cx.notify());
            BoundedAutoGrowFixture {
                field,
                limits: Some((px(0.), px(480.))),
                width: px(480.),
                _observation: observation,
            }
        });
        let height = |cx: &mut VisualTestContext| {
            for _ in 0..4 {
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.refresh();
                    _ = window.draw(cx);
                });
            }
            cx.debug_bounds("bounded-auto-grow").unwrap().size.height
        };
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let initial = cx.debug_bounds("bounded-auto-grow").unwrap().size.height;
        let wide = height(cx);
        assert_eq!(
            initial, wide,
            "the first paint must preserve the configured three-row minimum"
        );
        fixture.update(cx, |fixture, cx| {
            fixture.width = px(160.);
            cx.notify();
        });
        let narrow = height(cx);
        assert!(
            narrow > wide,
            "wrapped content grows the bounded shell: {wide:?} {narrow:?}"
        );
        fixture.update(cx, |fixture, cx| {
            fixture.width = px(480.);
            cx.notify();
        });
        assert_eq!(height(cx), wide);
        cx.update(|window, _| window.set_rem_size(px(24.)));
        assert!(
            height(cx) > wide,
            "font metrics must update constrained natural height"
        );
    }

    impl Render for AutoGrowFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(320.))
                .flex()
                .flex_col()
                .child(NyaInputShell::new("auto-grow-textarea", &self.field).multi_line())
        }
    }

    #[gpui::test]
    fn auto_grow_textarea_follows_its_lines_up_to_the_row_limit(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| AutoGrowFixture {
            field: cx.new(|cx| NyaInputState::new(cx, "one").auto_grow(1, 4)),
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        let height_for = |text: &str, cx: &mut gpui::VisualTestContext| {
            field.update(cx, |field, cx| field.set_content(text, cx));
            for _ in 0..3 {
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.refresh();
                    _ = window.draw(cx);
                });
            }
            cx.debug_bounds("auto-grow-textarea").unwrap().size.height
        };
        let one = height_for("one", cx);
        let three = height_for("one\ntwo\nthree", cx);
        let many = height_for("1\n2\n3\n4\n5\n6\n7\n8", cx);
        let four = height_for("1\n2\n3\n4", cx);
        assert!(one < three, "one={one:?} three={three:?}");
        assert!(three < four, "three={three:?} four={four:?}");
        assert_eq!(
            many, four,
            "rows beyond the limit scroll instead of growing"
        );
        assert_eq!(height_for("one", cx), one);
    }

    struct InputContextMenuFixture {
        field: gpui::Entity<NyaInputState>,
    }

    impl Render for InputContextMenuFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(px(320.)).child(
                NyaInputShell::new("popup-input", &self.field).gpui_context_menu([
                    "Cut".into(),
                    "Copy".into(),
                    "Paste".into(),
                    "Select All".into(),
                ]),
            )
        }
    }

    fn open_input_context_menu(cx: &mut VisualTestContext) {
        cx.simulate_mouse_down(
            point(px(12.), px(12.)),
            MouseButton::Right,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(12.), px(12.)),
            MouseButton::Right,
            Default::default(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn popup_input_menu_reuses_edit_actions_and_restores_input_focus(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| InputContextMenuFixture {
            field: cx.new(|cx| NyaInputState::new(cx, "hello").multi_line(Some(3))),
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        // A right click from outside the field must restore focus to the field.
        open_input_context_menu(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
            field.update(cx, |field, cx| field.select_all(window, cx));
        });
        cx.run_until_parked();

        open_input_context_menu(cx);
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("hello".into())
            );
            assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
        });
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "hello");

        open_input_context_menu(cx);
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "");
        // Undo verifies Cut went through the editor's history.
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "hello");

        cx.update(|window, cx| {
            field.update(cx, |field, cx| field.select_all(window, cx));
            cx.write_to_clipboard(ClipboardItem::new_string("world".into()));
        });
        cx.run_until_parked();
        open_input_context_menu(cx);
        cx.simulate_keystrokes("down down down enter");
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "world");

        open_input_context_menu(cx);
        cx.simulate_keystrokes("up enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let state = field.read(cx).state.as_ref().expect("rendered input");
            assert!(state.context_menu_capabilities(cx).has_selection());
        });
    }

    #[gpui::test]
    fn readonly_popup_input_menu_allows_copy_without_editing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| InputContextMenuFixture {
            field: cx.new(|cx| NyaInputState::new(cx, "read only").readonly(true)),
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            _ = window.draw(cx);
            field.update(cx, |field, cx| field.select_all(window, cx));
            cx.write_to_clipboard(ClipboardItem::new_string("replacement".into()));
        });
        cx.run_until_parked();
        open_input_context_menu(cx);
        // Initial confirmation must not invoke the disabled Cut row.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("replacement".into())
            );
        });
        open_input_context_menu(cx);
        // Keyboard navigation skips disabled Cut and moves straight to Copy.
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert_eq!(
            field.read_with(cx, |field, cx| field.value(cx)),
            "read only"
        );
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("read only".into())
            );
        });
        open_input_context_menu(cx);
        cx.simulate_keystrokes("up enter");
        cx.run_until_parked();
        assert_eq!(
            field.read_with(cx, |field, cx| field.value(cx)),
            "read only"
        );
    }

    struct AncestorKeyListenerFixture {
        plain: gpui::Entity<NyaInputState>,
        masked: gpui::Entity<NyaInputState>,
        handled: Rc<Cell<usize>>,
    }

    struct SubmitOnEnterFixture {
        field: gpui::Entity<NyaInputState>,
        submitted: Rc<Cell<usize>>,
    }

    impl Render for SubmitOnEnterFixture {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let submitted = Rc::clone(&self.submitted);
            div()
                .on_key_down(move |event, _, cx| {
                    if event.keystroke.key == "enter" && !event.keystroke.modifiers.shift {
                        submitted.set(submitted.get() + 1);
                        cx.stop_propagation();
                    }
                })
                .child(self.field.clone())
        }
    }

    impl Render for AncestorKeyListenerFixture {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let handled = Rc::clone(&self.handled);
            div()
                .on_key_down(move |event, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "escape") {
                        handled.set(handled.get() + 1);
                        cx.stop_propagation();
                    }
                })
                .child(self.plain.clone())
                .child(self.masked.clone())
        }
    }

    struct CompositionFormFixture {
        field: gpui::Entity<NyaInputState>,
        submitted: Rc<Cell<usize>>,
    }

    impl Render for CompositionFormFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let submitted = self.submitted.clone();
            div()
                .w(px(300.))
                .h(px(36.))
                .on_key_down(move |event, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "escape") {
                        submitted.set(submitted.get() + 1);
                        cx.stop_propagation();
                    }
                })
                .child(super::NyaInput::new(&self.field))
        }
    }

    #[gpui::test]
    fn composing_text_does_not_submit_or_dismiss_the_form(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler as _;
        cx.update(gpui_kit::init);
        let submitted = Rc::new(Cell::new(0));
        let (fixture, cx) = cx.add_window_view({
            let submitted = submitted.clone();
            move |_, cx| CompositionFormFixture {
                field: cx.new(|cx| NyaInputState::new(cx, "")),
                submitted,
            }
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            _ = window.draw(cx);
            window.focus(&field.read(cx).component_focus_handle(cx), cx);
            field
                .read(cx)
                .component_state()
                .unwrap()
                .update(cx, |state, cx| {
                    state.replace_and_mark_text_in_range(None, "拼", Some(0..1), window, cx);
                });
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(submitted.get(), 0, "Enter must confirm composition first");
        cx.update(|window, cx| {
            field
                .read(cx)
                .component_state()
                .unwrap()
                .update(cx, |state, cx| {
                    state.replace_and_mark_text_in_range(None, "音", Some(0..1), window, cx);
                });
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(submitted.get(), 0, "Escape must cancel composition first");
        cx.update(|window, cx| {
            field
                .read(cx)
                .component_state()
                .unwrap()
                .update(cx, |state, cx| state.unmark_text(window, cx))
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(submitted.get(), 1);
    }

    #[gpui::test]
    fn composition_query_tracks_password_ime_without_confirming_marked_text(
        cx: &mut TestAppContext,
    ) {
        use gpui::EntityInputHandler as _;
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| SecretInputFixture::new(0, cx));
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_content_silent("", cx));
            _ = window.draw(cx);
            window.focus(&field.read(cx).component_focus_handle(cx), cx);
            assert!(!field.update(cx, |state, cx| state.is_composing(window, cx)));
            let component = field.read(cx).component_state().unwrap();
            component.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "拼", Some(0..1), window, cx);
            });
            assert!(field.update(cx, |state, cx| state.is_composing(window, cx)));
            assert!(
                field.update(cx, |state, cx| state.is_composing(window, cx)),
                "queries must not confirm IME text"
            );
            component.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
            });
            assert!(!field.update(cx, |state, cx| state.is_composing(window, cx)));
            component.update(cx, |state, cx| state.unmark_text(window, cx));
            assert!(!field.update(cx, |state, cx| state.is_composing(window, cx)));
        });
    }

    #[test]
    fn code_input_keeps_registered_bash_language() {
        assert!(LanguageRegistry::singleton().language("bash").is_some());

        let mut cx = TestAppContext::single();
        let field = cx.new(|cx| {
            NyaInputState::new(cx, "echo hello")
                .code(Some(4))
                .language("bash")
        });

        assert_eq!(
            cx.read_entity(&field, |field, _| field.language.clone()),
            Some("bash".into())
        );
    }

    struct ScriptFormFixture {
        previous: gpui::Entity<NyaInputState>,
        script: gpui::Entity<NyaInputState>,
        next: gpui::Entity<NyaInputState>,
        saved: Rc<Cell<usize>>,
    }

    impl Render for ScriptFormFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let saved = self.saved.clone();
            div()
                .w(px(500.))
                .flex()
                .flex_col()
                .on_key_down(|event, window, cx| {
                    if event.keystroke.key == "tab" {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                        cx.stop_propagation();
                    }
                })
                .child(NyaInputShell::new("script-form-previous", &self.previous))
                .child(
                    NyaInputShell::new("script-form-editor", &self.script)
                        .height(px(168.))
                        .tab_navigation()
                        .on_secondary_enter(move |_, _| saved.set(saved.get() + 1)),
                )
                .child(NyaInputShell::new("script-form-next", &self.next))
        }
    }

    #[gpui::test]
    fn script_form_keeps_top_alignment_pointer_selection_save_and_tab_navigation(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let saved = Rc::new(Cell::new(0));
        let (fixture, cx) = cx.add_window_view(|_, cx| ScriptFormFixture {
            previous: cx.new(|cx| NyaInputState::new(cx, "previous")),
            script: cx.new(|cx| {
                NyaInputState::new(cx, "echo first\necho second")
                    .code(Some(4))
                    .language("bash")
            }),
            next: cx.new(|cx| NyaInputState::new(cx, "next")),
            saved: saved.clone(),
        });
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        let (previous, script, next) = fixture.read_with(cx, |fixture, _| {
            (
                fixture.previous.clone(),
                fixture.script.clone(),
                fixture.next.clone(),
            )
        });
        let component = cx.update(|_, cx| {
            let Some(ComponentState::Editor(editor)) = script.read(cx).state.clone() else {
                panic!("script fields must use the code editor");
            };
            editor
        });
        let [first, second] = cx.update(|_, cx| {
            let editor = component.read(cx);
            [2, 13].map(|offset| editor.range_to_bounds(&(offset..offset)).unwrap())
        });
        let shell = cx.debug_bounds("script-form-editor").unwrap();
        assert_eq!(shell.size.height, px(168.));
        assert!(first.top() - shell.top() < px(24.));
        assert!(second.top() > first.top());
        for (bounds, offset) in [(first, 2), (second, 13)] {
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|_, cx| assert_eq!(component.read(cx).selected_range(), offset..offset));
        }
        cx.update(|window, cx| {
            script.update(cx, |script, cx| script.focus_with_cursor_at_end(window, cx));
        });
        cx.simulate_keystrokes("enter shift-enter");
        cx.run_until_parked();
        let edited = script.read_with(cx, |script, cx| script.value(cx));
        assert_eq!(edited, "echo first\necho second\n\n");
        assert_eq!(saved.get(), 0);
        let save_shortcut = if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        };
        cx.simulate_keystrokes(save_shortcut);
        cx.run_until_parked();
        assert_eq!(saved.get(), 1);
        assert_eq!(script.read_with(cx, |script, cx| script.value(cx)), edited);
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(next.read(cx).component_focus_handle(cx).is_focused(window))
        });
        cx.simulate_keystrokes("shift-tab");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(
                script
                    .read(cx)
                    .component_focus_handle(cx)
                    .is_focused(window)
            )
        });
        cx.simulate_keystrokes("shift-tab");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(
                previous
                    .read(cx)
                    .component_focus_handle(cx)
                    .is_focused(window)
            );
        });
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(script.read_with(cx, |script, cx| script.value(cx)), edited);
        cx.update(|window, cx| {
            use gpui::EntityInputHandler as _;
            component.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "拼", Some(0..1), window, cx)
            });
        });
        cx.simulate_keystrokes(save_shortcut);
        cx.run_until_parked();
        assert_eq!(saved.get(), 1, "IME confirmation must not save the form");
        assert!(
            script
                .read_with(cx, |script, cx| script.value(cx))
                .ends_with('拼')
        );
    }

    #[test]
    fn value_tracks_seed_reset_and_clear_before_component_renders() {
        let mut cx = TestAppContext::single();
        let field = cx.new(|cx| NyaInputState::new(cx, "seed").placeholder("Name"));

        assert_eq!(cx.read_entity(&field, |field, cx| field.value(cx)), "seed");

        field.update(&mut cx, |field, cx| field.set_content("reset", cx));
        assert_eq!(cx.read_entity(&field, |field, cx| field.value(cx)), "reset");

        field.update(&mut cx, |field, cx| field.clear(cx));
        assert_eq!(cx.read_entity(&field, |field, cx| field.value(cx)), "");
    }

    #[gpui::test]
    fn ancestor_key_listener_does_not_block_plain_or_masked_ascii_input(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let handled = Rc::new(Cell::new(0));
        let (fixture, cx) = cx.add_window_view({
            let handled = Rc::clone(&handled);
            move |_, cx| AncestorKeyListenerFixture {
                plain: cx.new(|cx| NyaInputState::new(cx, "")),
                masked: cx.new(|cx| NyaInputState::new(cx, "").masked(true)),
                handled,
            }
        });
        let (plain, masked) = fixture.read_with(cx, |fixture, _| {
            (fixture.plain.clone(), fixture.masked.clone())
        });

        cx.update(|window, cx| {
            _ = window.draw(cx);
            window.focus(&plain.read(cx).focus_handle(), cx);
        });
        cx.simulate_keystrokes("a");
        cx.run_until_parked();
        assert_eq!(plain.read_with(cx, |state, cx| state.value(cx)), "a");

        cx.update(|window, cx| window.focus(&masked.read(cx).focus_handle(), cx));
        cx.simulate_keystrokes("b enter escape");
        cx.run_until_parked();
        assert_eq!(masked.read_with(cx, |state, cx| state.value(cx)), "b");
        assert_eq!(handled.get(), 2);
    }

    #[gpui::test]
    fn textarea_submit_on_enter_reaches_ancestor_and_shift_enter_inserts_newline(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let submitted = Rc::new(Cell::new(0));
        let (fixture, cx) = cx.add_window_view({
            let submitted = Rc::clone(&submitted);
            move |_, cx| SubmitOnEnterFixture {
                field: cx.new(|cx| {
                    NyaInputState::new(cx, "hello")
                        .multi_line(Some(4))
                        .submit_on_enter(true)
                }),
                submitted,
            }
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            _ = window.draw(cx);
            window.focus(&field.read(cx).focus_handle(), cx);
        });
        let submissions = Rc::new(Cell::new(0));
        let _subscription = cx.update(|_, cx| {
            let submissions = submissions.clone();
            cx.subscribe(&field, move |_, event: &super::NyaInputEvent, _| {
                if matches!(event, super::NyaInputEvent::Submitted(_)) {
                    submissions.set(submissions.get() + 1);
                }
            })
        });

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(submitted.get(), 1);
        assert_eq!(submissions.get(), 1);
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "hello");

        cx.simulate_keystrokes("shift-enter");
        cx.run_until_parked();
        assert_eq!(submitted.get(), 1);
        assert_eq!(submissions.get(), 1);
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "\nhello");
    }

    #[test]
    fn code_placeholder_preserves_instruction_lines_and_literal_escapes() {
        let text =
            "例如：ls -la\n使用 {{变量名}} 注入动态参数。\n\\r 回车、\\n 换行、\\p 暂停 1 秒";
        assert_eq!(
            component_placeholder(text.into(), true, true).as_ref(),
            text
        );
    }

    #[test]
    fn multiline_placeholder_collapses_line_breaks() {
        let placeholder = component_placeholder("first\nsecond".into(), true, false);

        assert_eq!(placeholder.as_ref(), "first second");
    }

    #[test]
    fn multiline_placeholder_collapses_crlf_and_blank_lines() {
        let placeholder = component_placeholder("first\r\n\r\nsecond\n\nthird".into(), true, false);

        assert_eq!(placeholder.as_ref(), "first second third");
        assert!(!placeholder.contains('\r'));
        assert!(!placeholder.contains('\n'));
    }

    #[test]
    fn single_line_placeholder_is_preserved() {
        let placeholder = component_placeholder("first\nsecond".into(), false, false);

        assert_eq!(placeholder.as_ref(), "first\nsecond");
    }

    #[test]
    fn multiline_placeholder_preserves_multibyte_text() {
        let placeholder = component_placeholder(
            "例如：ls -la\n使用 {{变量名}} 注入动态参数。".into(),
            true,
            false,
        );

        assert_eq!(
            placeholder.as_ref(),
            "例如：ls -la 使用 {{变量名}} 注入动态参数。"
        );
    }
}
