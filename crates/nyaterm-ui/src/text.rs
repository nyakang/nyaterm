//! Stable text and window-selection adapters. Renderer types stay inside this boundary.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, HighlightStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, StyleRefinement, Styled as _, Subscription, Window, div, px, rems, rgb,
};
use gpui_base::text::{CodeBlock, SelectionFormat, TextView, TextViewState, TextViewStyle};
use gpui_base::{ElementExt as _, TextSelection};

use crate::theme::ThemePalette;

mod code;
mod retention;
mod selection;
pub use code::NyaSelectableCode;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NyaTextFormat {
    Markdown,
    Plain,
    Code,
}

/// An explicitly owned renderer survives virtual-list unmounts.
#[derive(Clone)]
pub struct NyaTextState {
    inner: Entity<TextViewState>,
    format: NyaTextFormat,
    requested_document: Rc<RefCell<String>>,
    presentation: Rc<RefCell<Option<retention::TextPresentation>>>,
    painted: Rc<Cell<bool>>,
    copy_valid: Rc<Cell<bool>>,
    code_highlighter: Rc<RefCell<CodeHighlightCache>>,
    retained_selection: Rc<RefCell<Option<selection::RetainedRange>>>,
    applied_selection: Rc<RefCell<Option<selection::AppliedRange>>>,
}

impl NyaTextState {
    pub fn new(format: NyaTextFormat, source: &str, cx: &mut App) -> Self {
        let document = text_document(format, source);
        let inner = cx.new(|cx| {
            let mut state =
                TextViewState::markdown("", cx).selection_format(SelectionFormat::Plain);
            state.push_str(&document, cx);
            state
        });
        Self {
            inner,
            format,
            requested_document: Rc::new(RefCell::new(document)),
            presentation: Rc::default(),
            painted: Rc::default(),
            copy_valid: Rc::new(Cell::new(true)),
            code_highlighter: Rc::new(RefCell::new(CodeHighlightCache::default())),
            retained_selection: Rc::default(),
            applied_selection: Rc::default(),
        }
    }

    pub fn set_text(&self, source: &str, cx: &mut App) {
        let document = text_document(self.format, source);
        let previous = self.requested_document.replace(document.clone());
        let appending = source.starts_with(document_content(self.format, &previous));
        if !appending {
            self.copy_valid.set(false);
            self.clear_retained_selection(cx);
        }
        self.retain_current_selection(&previous, &document, cx);
        self.inner.update(cx, |state, cx| {
            if document.starts_with(&previous) {
                if document.len() > previous.len() && state.selected_source_range().is_some() {
                    // Keep the existing byte endpoints when a line grows or a
                    // streamed newline changes the geometry under its cursor.
                    state.invalidate_inline_layout(cx);
                }
                state.push_str(&document[previous.len()..], cx);
            } else {
                // The renderer parses short set_text replacements synchronously.
                // An empty baseline plus append keeps every real parse off-thread.
                if !appending {
                    state.clear_selection(cx);
                }
                state.set_text("", cx);
                state.push_str(&document, cx);
            }
        });
    }

    /// Parsing may finish asynchronously. Selection and color changes must not
    /// invalidate measured list rows, so observe only committed documents.
    pub fn observe_layout<V: 'static>(
        &self,
        cx: &mut Context<V>,
        mut callback: impl FnMut(&mut V, &mut Context<V>) + 'static,
    ) -> Subscription {
        let mut document = self.inner.read(cx).rendered_text();
        let requested = self.requested_document.clone();
        cx.observe(&self.inner, move |owner, state, cx| {
            let next = state.read(cx).rendered_text();
            if document != next
                && (next.source() == *requested.borrow() || !next.source().is_empty())
            {
                document = next;
                callback(owner, cx);
            }
        })
    }

    pub fn clear_selection(&self, cx: &mut App) {
        self.clear_retained_selection(cx);
        self.inner.update(cx, |state, cx| state.clear_selection(cx));
    }

    pub fn select_all(&self, cx: &mut App) {
        self.clear_retained_selection(cx);
        self.copy_valid.set(true);
        self.inner.update(cx, |state, cx| state.select_all(cx));
    }

    /// Mark a replacement invalid before a feature's asynchronous preprocessing
    /// finishes. Updating the prepared source makes the state copyable again.
    pub fn invalidate_selection(&self, cx: &mut App) {
        self.copy_valid.set(false);
        self.clear_selection(cx);
    }

    /// Never copy a retired document while its replacement is being parsed.
    pub fn selected_text(&self, cx: &App) -> String {
        if !self.copy_valid.get() {
            return String::new();
        }
        if let Some(text) = self.retained_selected_text(cx) {
            return text;
        }
        let state = self.inner.read(cx);
        let rendered = state.rendered_text();
        let requested = self.requested_document.borrow();
        let current = rendered.source();
        if *requested == current || (!current.is_empty() && requested.starts_with(current)) {
            let selected = state.selected_text();
            if self.format != NyaTextFormat::Markdown {
                let start = requested.find('\n').map_or(0, |index| index + 1);
                if let Some(range) = state.selected_source_range() {
                    if range == (0..current.len()) {
                        return current[start..].to_string();
                    }
                    if let Some(raw) = current.get(range) {
                        return raw.to_string();
                    }
                }
            }
            // TextView emits a structural LF after the final selected block.
            // Preserve every LF inside the selection and remove only that one.
            selected.strip_suffix('\n').unwrap_or(&selected).to_string()
        } else {
            String::new()
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let focus = self.inner.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
    }

    pub(crate) fn present(
        &self,
        view: TextView,
        style: NyaTextStyle,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement + use<> {
        self.prepare_retained_selection(style.0.selection(), cx);
        *self.presentation.borrow_mut() = Some(retention::TextPresentation::new(style, window));
        let painted = self.painted.clone();
        let copy_valid = self.copy_valid.clone();
        let gesture_state = self.clone();
        let outside_state = self.clone();
        div()
            .min_w_0()
            .w_full()
            .on_prepaint(move |_, _, _| painted.set(true))
            .capture_any_mouse_down(move |event, _, cx| {
                if event.button == gpui::MouseButton::Left {
                    gesture_state.clear_retained_selection(cx);
                    copy_valid.set(true);
                }
            })
            .on_mouse_down_out(move |event, window, cx| {
                if event.button == gpui::MouseButton::Left
                    && !crate::menu::nya_popup_is_active(window, cx)
                {
                    outside_state.clear_retained_selection(cx);
                }
            })
            .child(view)
    }

    fn full_text(&self, cx: &App) -> String {
        if !self.copy_valid.get() {
            return String::new();
        }
        let state = self.inner.read(cx);
        let rendered = state.rendered_text();
        if !self
            .requested_document
            .borrow()
            .starts_with(rendered.source())
        {
            return String::new();
        }
        if self.format == NyaTextFormat::Markdown {
            let text = rendered.as_str();
            text.strip_suffix('\n').unwrap_or(text).to_string()
        } else {
            let start = rendered.source().find('\n').map_or(0, |index| index + 1);
            rendered.source()[start..].to_string()
        }
    }

    pub(crate) fn set_code_highlights(&self, highlights: NyaCodeHighlights) {
        let mut cache = self.code_highlighter.borrow_mut();
        if Arc::ptr_eq(&cache.highlights, &highlights) || cache.highlights == highlights {
            return;
        }
        let styles = highlights.clone();
        cache.highlighter = Arc::new(move |_| styles.iter().cloned().collect());
        cache.highlights = highlights;
    }

    pub(crate) fn view(&self) -> TextView {
        TextView::new(&self.inner)
            .selectable(true)
            .selection_format(SelectionFormat::Plain)
            .shared_code_block_highlighter(self.code_highlighter.borrow().highlighter.clone())
    }
}

fn text_document(format: NyaTextFormat, source: &str) -> String {
    match format {
        NyaTextFormat::Markdown => source.to_string(),
        NyaTextFormat::Plain | NyaTextFormat::Code => {
            let longest = source
                .split(|ch| ch != '`')
                .map(str::len)
                .max()
                .unwrap_or(0);
            let fence = "`".repeat(longest.max(2) + 1);
            // An open fence is a valid Markdown code block. Appending output
            // then appends to the document without replacing its selected runs.
            format!("{fence}\n{source}")
        }
    }
}

fn document_content(format: NyaTextFormat, document: &str) -> &str {
    if format == NyaTextFormat::Markdown {
        document
    } else {
        document.split_once('\n').map_or("", |(_, content)| content)
    }
}

#[derive(Clone)]
pub struct NyaTextStyle(TextViewStyle, bool);

impl Default for NyaTextStyle {
    fn default() -> Self {
        Self(TextViewStyle::default(), true)
    }
}

impl NyaTextStyle {
    pub(crate) fn themed(cx: &App) -> Self {
        Self(
            TextViewStyle::from_theme(&gpui_base::Theme::global(cx)),
            true,
        )
    }

    pub fn transcript(palette: ThemePalette, answer: bool) -> Self {
        let text_size = if answer { 14. } else { 12. };
        Self(
            TextViewStyle::default()
                .with_foreground(rgb(palette.text).into())
                .with_muted_foreground(rgb(palette.text_muted).into())
                .with_link(rgb(palette.link).into())
                .with_selection(rgb(palette.terminal_selection).into())
                .with_border(rgb(palette.border).into())
                .with_code_background(rgb(palette.bg).into())
                .with_paragraph_gap(rems(if answer { 0.5 } else { 0.25 }))
                .with_heading(move |level| {
                    let size = text_size - 12.
                        + match level {
                            1 => 16.,
                            2 => 14.,
                            _ => 13.,
                        };
                    StyleRefinement::default()
                        .text_size(px(size))
                        .font_weight(gpui::FontWeight(800.))
                })
                .with_code_block({
                    let mut code = StyleRefinement::default()
                        .p_2()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .max_h(px(256.));
                    code.overflow.y = Some(gpui::Overflow::Scroll);
                    code
                }),
            true,
        )
    }

    pub fn foreground(mut self, color: u32) -> Self {
        self.1 = false;
        self.0 = self.0.with_foreground(rgb(color).into());
        self
    }

    pub(crate) fn foreground_color(mut self, color: gpui::Hsla) -> Self {
        if self.1 {
            self.0 = self.0.with_foreground(color);
        }
        self.1 = false;
        self
    }

    pub(crate) fn raw_code(mut self, window: &Window) -> Self {
        let inherited = window.text_style();
        self.0 = self.0.with_code_block(
            StyleRefinement::default()
                .p_0()
                .border_0()
                .bg(gpui::transparent_black())
                .font_family(inherited.font_family.clone())
                .text_size(inherited.font_size.to_pixels(window.rem_size()))
                .line_height(px(18.)),
        );
        self
    }

    pub(crate) fn apply(&self, view: TextView) -> TextView {
        view.style(self.0.clone())
    }
}

/// One scope per semantic conversation, shared by all its text surfaces.
#[derive(Clone, Default)]
pub struct NyaTextScope(
    Rc<RefCell<Vec<NyaTextState>>>,
    Rc<Cell<Option<(gpui::EntityId, gpui::EntityId)>>>,
);

impl NyaTextScope {
    /// Keep keyboard copy consistent with the conversation's context menu.
    /// Root continues to own the window gesture and modal selection scopes.
    pub fn wrap(&self, element: impl IntoElement) -> impl IntoElement {
        let scope = self.clone();
        let endpoints = self.1.clone();
        let gesture_members = self.0.clone();
        let select_all_members = self.0.clone();
        let moving_members = self.0.clone();
        div()
            .min_w_0()
            .w_full()
            .child(element)
            .capture_any_mouse_down(move |event, _, cx| {
                if event.button == gpui::MouseButton::Left {
                    endpoints.set(None);
                    for state in gesture_members.borrow().iter() {
                        state.clear_retained_selection(cx);
                        state.copy_valid.set(true);
                    }
                }
            })
            .on_mouse_move(move |event, _, cx| {
                if event.pressed_button == Some(gpui::MouseButton::Left) {
                    for state in moving_members.borrow().iter() {
                        state.clear_retained_selection(cx);
                    }
                }
            })
            .capture_action(move |_: &gpui_base::input::SelectAll, _, cx| {
                for state in select_all_members.borrow().iter() {
                    state.clear_retained_selection(cx);
                    state.copy_valid.set(true);
                }
                cx.propagate();
            })
            .capture_action(move |_: &gpui_base::input::Copy, window, cx| {
                if !scope
                    .0
                    .borrow()
                    .iter()
                    .any(|state| state.inner.read(cx).focus_handle().is_focused(window))
                {
                    cx.propagate();
                    return;
                }
                let text = scope.selected_text(window, cx);
                if !text.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                cx.stop_propagation();
            })
    }

    pub fn replace_members(&self, members: Vec<NyaTextState>) {
        if let Some((first, last)) = self.1.get()
            && (!members.iter().any(|state| state.inner.entity_id() == first)
                || !members.iter().any(|state| state.inner.entity_id() == last))
        {
            self.1.set(None);
        }
        *self.0.borrow_mut() = members;
    }

    pub fn selected_text(&self, window: &mut Window, cx: &mut App) -> String {
        // Refresh native byte projections even when Copy follows pointer input
        // before another paint. The window serializer itself is not scoped.
        let _ = TextSelection::selected_text(window, cx);
        let members = self.0.borrow();
        let selected: Vec<_> = members
            .iter()
            .map(|state| state.selected_text(cx))
            .collect();
        let Some(first) = selected.iter().position(|text| !text.is_empty()) else {
            return String::new();
        };
        let last = selected.iter().rposition(|text| !text.is_empty()).unwrap();
        if members[first..=last]
            .iter()
            .any(|state| !state.copy_valid.get())
        {
            return String::new();
        }
        (first..=last)
            .map(|index| {
                if index == first || index == last {
                    selected[index].clone()
                } else {
                    members[index].full_text(cx)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Keep only the selection endpoints mounted when a virtual list retires
    /// their rows. Intermediate content is copied from the ordered members.
    pub fn retain_selection(&self, element: impl IntoElement) -> impl IntoElement {
        let scope = self.clone();
        let outside_scope = self.clone();
        div()
            .size_full()
            .child(retention::SelectionRetention::new(self.clone(), element))
            .on_mouse_down_out(move |event, window, cx| {
                if event.button == gpui::MouseButton::Left
                    && !crate::menu::nya_popup_is_active(window, cx)
                {
                    for state in outside_scope.0.borrow().iter() {
                        state.clear_retained_selection(cx);
                    }
                }
            })
            .capture_action(move |_: &gpui_base::input::Copy, window, cx| {
                if !scope
                    .0
                    .borrow()
                    .iter()
                    .any(|state| state.inner.read(cx).focus_handle().is_focused(window))
                {
                    cx.propagate();
                    return;
                }
                let text = scope.selected_text(window, cx);
                if !text.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                cx.stop_propagation();
            })
    }

    pub fn clear(&self, window: &mut Window, cx: &mut App) {
        for state in self.0.borrow().iter() {
            state.clear_selection(cx);
        }
        TextSelection::clear(window, cx);
    }
}

pub type NyaCodeHighlights = Arc<[(Range<usize>, HighlightStyle)]>;

type CodeHighlighter = Arc<dyn Fn(&CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync>;

struct CodeHighlightCache {
    highlights: NyaCodeHighlights,
    highlighter: CodeHighlighter,
}

impl Default for CodeHighlightCache {
    fn default() -> Self {
        Self {
            highlights: Arc::from([]),
            highlighter: Arc::new(|_| Vec::new()),
        }
    }
}
