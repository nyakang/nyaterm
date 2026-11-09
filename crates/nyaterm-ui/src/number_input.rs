use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement, Render, RenderOnce, SharedString,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::input::{
    InputEvent, InputState, MaskPattern, NumberInput, NumberInputEvent, NumberStep, StepAction,
};

use crate::input_focus::{preserve_nya_input_focus_on_pointer_down, register_nya_input_focus};
use crate::sizing::{form_control_height, form_control_size};
use crate::theme_bridge::input_surface_color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyaNumberStep {
    Decrement,
    Increment,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NyaNumberInputEvent {
    Changed(String),
    Submitted(String),
    Stepped(NyaNumberStep),
}

#[derive(Clone, Debug)]
pub struct NyaNumberInputOptions {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub decimal_places: Option<usize>,
    pub allow_infinity: bool,
    pub disabled: bool,
    pub prefix: Option<SharedString>,
    pub suffix: Option<SharedString>,
}

impl Default for NyaNumberInputOptions {
    fn default() -> Self {
        Self {
            min: f64::MIN,
            max: f64::MAX,
            step: 1.0,
            decimal_places: None,
            allow_infinity: false,
            disabled: false,
            prefix: None,
            suffix: None,
        }
    }
}

impl NyaNumberInputOptions {
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn decimal_places(mut self, decimal_places: usize) -> Self {
        self.decimal_places = Some(decimal_places);
        self
    }

    pub fn allow_infinity(mut self, allow_infinity: bool) -> Self {
        self.allow_infinity = allow_infinity;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    pub fn suffix(mut self, suffix: impl Into<SharedString>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    fn accepts_draft(&self, text: &str) -> bool {
        if self.allow_infinity && (text == "∞" || "inf".starts_with(&text.to_ascii_lowercase())) {
            return true;
        }

        let negative = text.starts_with('-');
        if negative && self.min >= 0. {
            return false;
        }
        let unsigned = text
            .strip_prefix('-')
            .or_else(|| text.strip_prefix('+'))
            .unwrap_or(text);
        let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        if !integer.bytes().all(|ch| ch.is_ascii_digit())
            || !fraction.bytes().all(|ch| ch.is_ascii_digit())
            || self.decimal_places.is_some_and(|places| {
                fraction.len() > places || (places == 0 && unsigned.contains('.'))
            })
        {
            return false;
        }

        // Bound the integer text as well as the value so leading zeroes cannot
        // grow the buffer indefinitely. Use the bound for the entered sign.
        let bound = if negative { self.min } else { self.max };
        let integer_digits = format!("{:.0}", bound.abs().floor()).len();
        if integer.len() > integer_digits {
            return false;
        }
        if unsigned.is_empty() || unsigned == "." {
            return true;
        }

        // Keep zero and incomplete prefixes editable: typing 100 must be able
        // to pass through 1 and 10, and typing -10 through -1.
        text.parse::<f64>().is_ok_and(|value| {
            value.is_finite() && value >= self.min.min(0.) && value <= self.max.max(0.)
        })
    }
}

pub struct NyaNumberInputState {
    state: Option<Entity<InputState>>,
    seed: SharedString,
    pending_value: Option<SharedString>,
    silent_value: Option<SharedString>,
    placeholder: SharedString,
    options: NyaNumberInputOptions,
    focus: FocusHandle,
    focused: bool,
    subscriptions: Vec<Subscription>,
}

impl NyaNumberInputState {
    pub fn new(
        cx: &mut Context<Self>,
        seed: impl Into<SharedString>,
        options: NyaNumberInputOptions,
    ) -> Self {
        Self {
            state: None,
            seed: seed.into(),
            pending_value: None,
            silent_value: None,
            placeholder: SharedString::default(),
            options,
            focus: cx.focus_handle(),
            focused: false,
            subscriptions: Vec::new(),
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn value(&self, cx: &App) -> String {
        if let Some(state) = &self.state {
            state.read(cx).value().to_string()
        } else if let Some(value) = &self.pending_value {
            value.to_string()
        } else {
            self.seed.to_string()
        }
    }

    pub fn set_content(&mut self, text: &str, cx: &mut Context<Self>) {
        self.pending_value = Some(SharedString::from(text.to_string()));
        cx.notify();
    }

    /// Replace a draft buffer without reporting the rollback as a user edit.
    pub fn set_content_silent(&mut self, text: &str, cx: &mut Context<Self>) {
        let value = SharedString::from(text.to_string());
        self.silent_value = Some(value.clone());
        self.pending_value = Some(value);
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.options.disabled != disabled {
            self.options.disabled = disabled;
            cx.notify();
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn component_focus_handle(&self, cx: &App) -> FocusHandle {
        self.state
            .as_ref()
            .map(|state| state.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
    }

    pub fn has_focus(&self) -> bool {
        self.focused
    }

    pub fn component_state(&self) -> Option<Entity<InputState>> {
        self.state.clone()
    }

    fn ensure_component(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(state) = self.state.clone() {
            if let Some(value) = self.pending_value.take() {
                state.update(cx, |state, cx| state.set_value(value, window, cx));
            }
            return state;
        }

        let value = self
            .pending_value
            .take()
            .unwrap_or_else(|| self.seed.clone());
        let placeholder = self.placeholder.clone();
        // The component installs a numeric mask unless one is set explicitly,
        // and that mask would reject the `∞` an infinity-capable field steps to.
        let mask_pattern = if self.options.allow_infinity {
            MaskPattern::None
        } else {
            MaskPattern::Number {
                separator: None,
                fraction: self.options.decimal_places,
            }
        };
        let draft_options = self.options.clone();
        let state = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .default_value(value)
                .placeholder(placeholder)
                .mask_pattern(mask_pattern)
                .min(self.options.min)
                .max(self.options.max)
                .step(self.options.step)
                .validate(move |text, _| draft_options.accepts_draft(text));
            // Only the infinity cycle needs caller-controlled stepping.
            if self.options.allow_infinity {
                state.set_step(None::<NumberStep>, window, cx);
            }
            state
        });
        register_nya_input_focus(&state.read(cx).focus_handle(cx), cx);
        self.subscriptions = vec![
            cx.subscribe_in(
                &state,
                window,
                |this, input, event: &NumberInputEvent, window, cx| {
                    if !this.options.allow_infinity {
                        return;
                    }
                    let NumberInputEvent::Step(action) = event;
                    let step = match action {
                        StepAction::Decrement => NyaNumberStep::Decrement,
                        StepAction::Increment => NyaNumberStep::Increment,
                    };
                    let next = this.stepped_value(input.read(cx).value().as_ref(), step);
                    input.update(cx, |input, cx| {
                        input.set_value(SharedString::from(next.clone()), window, cx);
                    });
                    cx.emit(NyaNumberInputEvent::Stepped(step));
                    cx.emit(NyaNumberInputEvent::Changed(next));
                },
            ),
            cx.subscribe_in(
                &state,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let value = input.read(cx).value().to_string();
                        if this
                            .silent_value
                            .take()
                            .is_some_and(|expected| expected.as_ref() == value)
                        {
                            return;
                        }
                        cx.emit(NyaNumberInputEvent::Changed(value));
                    }
                    // Same contract as `forward_input_event` in input.rs:
                    // `Shift+Enter` is the newline gesture, not a submit.
                    InputEvent::PressEnter { shift, .. } if !shift => {
                        let committed = this.committed_value(input.read(cx).value().as_ref());
                        input.update(cx, |input, cx| {
                            input.set_value(SharedString::from(committed.clone()), window, cx);
                        });
                        cx.emit(NyaNumberInputEvent::Submitted(committed));
                    }
                    InputEvent::PressEnter { .. } => {}
                    InputEvent::Blur if this.options.allow_infinity => {
                        // The native blur clamp only applies to numeric masks.
                        let value = input.read(cx).value().to_string();
                        let committed = this.committed_value(&value);
                        if committed != value {
                            input.update(cx, |input, cx| {
                                input.set_value(SharedString::from(committed.clone()), window, cx);
                            });
                            cx.emit(NyaNumberInputEvent::Changed(committed));
                        }
                    }
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ),
        ];
        self.state = Some(state.clone());
        state
    }

    fn stepped_value(&self, text: &str, direction: NyaNumberStep) -> String {
        stepped_number_text(text, direction, &self.options)
    }

    fn committed_value(&self, text: &str) -> String {
        committed_number_text(text, &self.options)
    }
}

impl EventEmitter<NyaNumberInputEvent> for NyaNumberInputState {}

impl Focusable for NyaNumberInputState {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.component_focus_handle(cx)
    }
}

impl Render for NyaNumberInputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.ensure_component(window, cx);
        let component_focus = state.read(cx).focus_handle(cx);
        if self.focus.is_focused(window) && !component_focus.is_focused(window) {
            state.update(cx, |state, cx| state.focus(window, cx));
        }
        self.focused = self.focus.is_focused(window) || component_focus.is_focused(window);
        NumberInput::new(&state)
            .with_size(form_control_size())
            .h(form_control_height())
            .bg(input_surface_color(cx))
            .disabled(self.options.disabled)
            .when_some(self.options.prefix.clone(), |this, prefix| {
                this.prefix(text_affix(prefix).into_any_element())
            })
            .when_some(self.options.suffix.clone(), |this, suffix| {
                this.suffix(text_affix(suffix).into_any_element())
            })
    }
}

#[derive(IntoElement)]
pub struct NyaNumberInput {
    state: Entity<NyaNumberInputState>,
    appearance: bool,
}

impl NyaNumberInput {
    pub fn new(state: &Entity<NyaNumberInputState>) -> Self {
        Self {
            state: state.clone(),
            appearance: true,
        }
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }
}

impl RenderOnce for NyaNumberInput {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (state, options, focused) = self.state.update(cx, |state, cx| {
            (
                state.ensure_component(window, cx),
                state.options.clone(),
                state.focus.is_focused(window),
            )
        });
        if focused {
            state.update(cx, |state, cx| state.focus(window, cx));
        }
        self.state.update(cx, |input_state, cx| {
            let component_focus = state.read(cx).focus_handle(cx);
            input_state.focused =
                input_state.focus.is_focused(window) || component_focus.is_focused(window);
        });
        div()
            .size_full()
            .capture_any_mouse_down(|_, _, cx| {
                preserve_nya_input_focus_on_pointer_down(cx);
            })
            .child(
                NumberInput::new(&state)
                    .w_full()
                    .with_size(form_control_size())
                    .h(form_control_height())
                    .appearance(self.appearance)
                    .when(self.appearance, |this| this.bg(input_surface_color(cx)))
                    .disabled(options.disabled)
                    .when_some(options.prefix, |this, prefix| {
                        this.prefix(text_affix(prefix).into_any_element())
                    })
                    .when_some(options.suffix, |this, suffix| {
                        this.suffix(text_affix(suffix).into_any_element())
                    }),
            )
    }
}

fn text_affix(text: SharedString) -> impl IntoElement {
    div().child(text)
}

fn stepped_number_text(
    text: &str,
    direction: NyaNumberStep,
    options: &NyaNumberInputOptions,
) -> String {
    if options.allow_infinity && is_infinity_text(text) {
        return match direction {
            NyaNumberStep::Decrement => "∞".to_string(),
            NyaNumberStep::Increment => format_number(options.min.max(1.0), options),
        };
    }

    let delta = match direction {
        NyaNumberStep::Decrement => -options.step.abs(),
        NyaNumberStep::Increment => options.step.abs(),
    };
    let current = text.trim().parse::<f64>().unwrap_or({
        if delta > 0.0 {
            options.min - delta
        } else {
            options.max - delta
        }
    });
    let next = current + delta;
    if options.allow_infinity && next < options.min {
        return "∞".to_string();
    }
    format_number(next.clamp(options.min, options.max), options)
}

fn committed_number_text(text: &str, options: &NyaNumberInputOptions) -> String {
    if options.allow_infinity && is_infinity_text(text) {
        return "∞".to_string();
    }
    match text.trim().parse::<f64>() {
        Ok(value) if value.is_finite() => {
            format_number(value.clamp(options.min, options.max), options)
        }
        _ => text.to_string(),
    }
}

fn is_infinity_text(text: &str) -> bool {
    let text = text.trim();
    text == "∞" || text.eq_ignore_ascii_case("inf")
}

fn format_number(value: f64, options: &NyaNumberInputOptions) -> String {
    if let Some(decimal_places) = options.decimal_places {
        return format!("{value:.decimal_places$}");
    }
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{
        AppContext as _, ClipboardItem, IntoElement, Modifiers, MouseButton, MouseDownEvent,
        MouseUpEvent, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
        div, point, px,
    };
    use gpui_kit::component::input::{Paste, SelectAll};

    use super::{
        NyaNumberInput, NyaNumberInputEvent, NyaNumberInputOptions, NyaNumberInputState,
        NyaNumberStep, committed_number_text, stepped_number_text,
    };
    use crate::sizing::{NYA_FORM_CONTROL_HEIGHT_PX, form_control_size};

    struct NumberBehaviorFixture {
        field: gpui::Entity<NyaNumberInputState>,
    }

    impl Render for NumberBehaviorFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(220.))
                .h(px(32.))
                .child(NyaNumberInput::new(&self.field))
        }
    }

    fn number_fixture<'a>(
        cx: &'a mut TestAppContext,
        value: &str,
        options: NyaNumberInputOptions,
    ) -> (gpui::Entity<NyaNumberInputState>, &'a mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| NumberBehaviorFixture {
            field: cx.new(|cx| NyaNumberInputState::new(cx, value.to_string(), options)),
        });
        let field = fixture.read_with(cx, |fixture, _| fixture.field.clone());
        cx.update(|window, cx| {
            window.activate_window();
            _ = window.draw(cx);
            window.focus(&field.read(cx).component_focus_handle(cx), cx);
            _ = window.draw(cx);
        });
        (field, cx)
    }

    #[gpui::test]
    fn scrollback_number_selection_keeps_editor_focus_with_accessibility_active(
        cx: &mut TestAppContext,
    ) {
        let (field, cx) = number_fixture(
            cx,
            "10000",
            NyaNumberInputOptions::default()
                .range(100., 100_000.)
                .step(100.)
                .decimal_places(0),
        );
        assert!(cx.activate_accessibility());
        cx.update(|window, cx| window.blur(cx));
        cx.simulate_click(point(px(100.), px(16.)), Modifiers::default());
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(100.), px(16.)),
            modifiers: Modifiers::default(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(100.), px(16.)),
            modifiers: Modifiers::default(),
            click_count: 2,
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.is_a11y_active());
            assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
            let input = field.read(cx).component_state().unwrap();
            assert_eq!(input.read(cx).selected_value(), "10000");
            assert_eq!(field.read(cx).value(cx), "10000");
        });
    }

    #[gpui::test]
    fn native_keyboard_and_button_steps_report_one_change_and_respect_bounds(
        cx: &mut TestAppContext,
    ) {
        let (field, cx) = number_fixture(
            cx,
            "500",
            NyaNumberInputOptions::default()
                .range(100., 100_000.)
                .step(100.),
        );
        let events = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&field, move |_, event, _| {
                events.borrow_mut().push(event.clone())
            })
        });
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [NyaNumberInputEvent::Changed("600".into())]
        );
        events.borrow_mut().clear();
        cx.simulate_click(point(px(204.), px(16.)), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [NyaNumberInputEvent::Changed("700".into())]
        );
        for (value, key, expected) in [
            ("99999", "up", "100000"),
            ("100000", "up", "100000"),
            ("101", "down", "100"),
            ("100", "down", "100"),
        ] {
            cx.update(|window, cx| {
                field
                    .read(cx)
                    .component_state()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value(value, window, cx);
                    });
            });
            cx.simulate_keystrokes(key);
            cx.run_until_parked();
            assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), expected);
        }
    }

    #[gpui::test]
    fn number_input_rejects_excess_digits_and_values_while_typing_and_pasting(
        cx: &mut TestAppContext,
    ) {
        let (field, cx) = number_fixture(
            cx,
            "",
            NyaNumberInputOptions::default()
                .range(100., 100_000.)
                .step(100.)
                .decimal_places(0),
        );
        for (text, expected) in [
            ("1", "1"),
            ("0", "10"),
            ("0", "100"),
            ("0", "1000"),
            ("0", "10000"),
            ("0", "100000"),
        ] {
            cx.simulate_input(text);
            assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), expected);
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&field, move |_, event, _| {
                events.borrow_mut().push(event.clone())
            })
        });
        cx.simulate_input("9");
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "100000");
        cx.simulate_keystrokes("home");
        cx.simulate_input("9");
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "100000");
        for text in ["100001", "999999", "5678945656465645", "0000000", "1.5"] {
            cx.update(|window, cx| {
                window.dispatch_action(Box::new(SelectAll), cx);
                cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
                window.dispatch_action(Box::new(Paste), cx);
            });
            cx.run_until_parked();
            assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "100000");
        }
        assert!(
            events.borrow().is_empty(),
            "rejected edits must not reach the business state"
        );
        cx.update(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));
        cx.simulate_input("９９９９９");
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "99999");
        cx.update(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));
        cx.simulate_keystrokes("backspace");
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "");
        cx.simulate_input("1");
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "1");
    }

    #[test]
    fn number_drafts_allow_signed_and_decimal_prefixes_within_the_text_budget() {
        let signed = NyaNumberInputOptions::default()
            .range(-20., 19.)
            .decimal_places(0);
        for text in ["", "-", "-1", "-20", "0", "19"] {
            assert!(signed.accepts_draft(text), "{text}");
        }
        for text in ["-21", "20", "000", "1.2", "1e1"] {
            assert!(!signed.accepts_draft(text), "{text}");
        }
        let decimal = NyaNumberInputOptions::default()
            .range(0., 60.)
            .decimal_places(2);
        for text in ["", ".", ".2", "0.25", "60.00"] {
            assert!(decimal.accepts_draft(text), "{text}");
        }
        for text in ["-1", "60.01", "0.250", "000.1"] {
            assert!(!decimal.accepts_draft(text), "{text}");
        }
    }

    #[gpui::test]
    fn number_commit_and_native_blur_write_back_out_of_range_drafts(cx: &mut TestAppContext) {
        let (field, cx) = number_fixture(
            cx,
            "999999",
            NyaNumberInputOptions::default()
                .range(100., 100_000.)
                .step(100.),
        );
        let events = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&field, move |_, event, _| {
                events.borrow_mut().push(event.clone())
            })
        });
        for (value, enter, expected) in [("999999", true, "100000"), ("1", false, "100")] {
            cx.update(|window, cx| {
                window.focus(&field.read(cx).component_focus_handle(cx), cx);
                _ = window.draw(cx);
                field
                    .read(cx)
                    .component_state()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.replace_all(value, window, cx);
                    });
            });
            cx.run_until_parked();
            assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), value);
            events.borrow_mut().clear();
            if enter {
                cx.simulate_keystrokes("enter");
            } else {
                cx.update(|window, cx| {
                    window.blur(cx);
                    _ = window.draw(cx);
                });
            }
            cx.run_until_parked();
            assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), expected);
            let expected_event = if enter {
                NyaNumberInputEvent::Submitted(expected.into())
            } else {
                NyaNumberInputEvent::Changed(expected.into())
            };
            assert_eq!(events.borrow()[0], expected_event);
            assert!(events.borrow().iter().all(|event| match event {
                NyaNumberInputEvent::Changed(value) | NyaNumberInputEvent::Submitted(value) =>
                    value == expected,
                NyaNumberInputEvent::Stepped(_) => false,
            }));
        }
    }

    #[gpui::test]
    fn shift_enter_does_not_submit(cx: &mut TestAppContext) {
        let (field, cx) = number_fixture(cx, "100", NyaNumberInputOptions::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&field, move |_, event, _| {
                events.borrow_mut().push(event.clone())
            })
        });

        cx.simulate_keystrokes("shift-enter");
        cx.run_until_parked();
        assert!(
            events
                .borrow()
                .iter()
                .all(|event| !matches!(event, NyaNumberInputEvent::Submitted(_))),
            "shift-enter must not submit"
        );

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, NyaNumberInputEvent::Submitted(_)))
        );
    }

    #[gpui::test]
    fn infinity_cycle_survives_keyboard_buttons_and_blur(cx: &mut TestAppContext) {
        let (field, cx) = number_fixture(
            cx,
            "1",
            NyaNumberInputOptions::default()
                .range(1., 9999.)
                .allow_infinity(true),
        );
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "∞");
        cx.update(|window, cx| {
            window.blur(cx);
            _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "∞");
        cx.simulate_click(point(px(204.), px(16.)), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "1");
        cx.update(|window, cx| {
            window.focus(&field.read(cx).component_focus_handle(cx), cx);
            _ = window.draw(cx);
            field
                .read(cx)
                .component_state()
                .unwrap()
                .update(cx, |input, cx| {
                    input.replace_all("inf", window, cx);
                });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.blur(cx);
            _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "∞");
    }

    #[gpui::test]
    fn native_decimal_steps_preserve_precision(cx: &mut TestAppContext) {
        let (field, cx) = number_fixture(
            cx,
            "1.00",
            NyaNumberInputOptions::default()
                .range(0., 60.)
                .step(0.25)
                .decimal_places(2),
        );
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "1.25");
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, cx| field.value(cx)), "1.00");
    }

    struct NumberLayoutFixture {
        count: gpui::Entity<NyaNumberInputState>,
        interval: gpui::Entity<NyaNumberInputState>,
    }

    impl Render for NumberLayoutFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().flex().children([
                div()
                    .w(px(128.))
                    .h(px(32.))
                    .flex_none()
                    .child(NyaNumberInput::new(&self.count).appearance(false)),
                div()
                    .w(px(160.))
                    .h(px(32.))
                    .flex_none()
                    .child(NyaNumberInput::new(&self.interval).appearance(false)),
            ])
        }
    }

    #[gpui::test]
    fn borderless_number_inputs_leave_room_for_values_and_both_step_buttons(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| NumberLayoutFixture {
            count: cx.new(|cx| {
                NyaNumberInputState::new(
                    cx,
                    "9999",
                    NyaNumberInputOptions::default()
                        .range(1., 9999.)
                        .allow_infinity(true),
                )
            }),
            interval: cx.new(|cx| {
                NyaNumberInputState::new(
                    cx,
                    "60.00",
                    NyaNumberInputOptions::default()
                        .range(0., 60.)
                        .decimal_places(2)
                        .suffix("s"),
                )
            }),
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        fixture.read_with(cx, |fixture, cx| {
            for (field, left, width, expected) in [
                (&fixture.count, 0., 128., "9999"),
                (&fixture.interval, 128., 160., "60.00"),
            ] {
                let input = field.read(cx).component_state().unwrap();
                let bounds = input.read(cx).input_bounds();
                assert!(bounds.size.width >= px(32.));
                assert!(bounds.size.height > px(0.));
                assert!(bounds.origin.x >= px(left + 32.));
                assert!(bounds.right() <= px(left + width - 32.));
                assert_eq!(field.read(cx).value(cx), expected);
            }
        });
    }

    #[test]
    fn stepped_number_increments_decrements_and_clamps() {
        let options = NyaNumberInputOptions::default().range(1.0, 10.0).step(2.0);

        assert_eq!(
            stepped_number_text("3", NyaNumberStep::Increment, &options),
            "5"
        );
        assert_eq!(
            stepped_number_text("10", NyaNumberStep::Increment, &options),
            "10"
        );
        assert_eq!(
            stepped_number_text("1", NyaNumberStep::Decrement, &options),
            "1"
        );
    }

    #[test]
    fn stepped_number_starts_from_bounds_for_invalid_text() {
        let options = NyaNumberInputOptions::default().range(1.0, 10.0).step(2.0);

        assert_eq!(
            stepped_number_text("", NyaNumberStep::Increment, &options),
            "1"
        );
        assert_eq!(
            stepped_number_text("nope", NyaNumberStep::Decrement, &options),
            "10"
        );
    }

    #[test]
    fn committed_number_allows_empty_and_invalid_draft_text() {
        let options = NyaNumberInputOptions::default().range(1.0, 10.0).step(2.0);

        assert_eq!(committed_number_text("", &options), "");
        assert_eq!(committed_number_text("nope", &options), "nope");
        assert_eq!(committed_number_text("99", &options), "10");
    }

    #[test]
    fn stepped_number_preserves_decimal_places() {
        let options = NyaNumberInputOptions::default()
            .range(0.0, 60.0)
            .step(0.25)
            .decimal_places(2);

        assert_eq!(
            stepped_number_text("1.00", NyaNumberStep::Increment, &options),
            "1.25"
        );
        assert_eq!(committed_number_text("999", &options), "60.00");
    }

    #[test]
    fn stepped_number_supports_infinity_cycle() {
        let options = NyaNumberInputOptions::default()
            .range(1.0, 9999.0)
            .allow_infinity(true);

        assert_eq!(
            stepped_number_text("1", NyaNumberStep::Decrement, &options),
            "∞"
        );
        assert_eq!(
            stepped_number_text("∞", NyaNumberStep::Increment, &options),
            "1"
        );
        assert_eq!(committed_number_text("inf", &options), "∞");
    }

    #[test]
    fn number_input_uses_standard_form_control_size() {
        assert_eq!(NYA_FORM_CONTROL_HEIGHT_PX, 32.);
        assert_eq!(form_control_size(), gpui_kit::component::Size::Medium);
    }

    #[test]
    fn number_input_state_exposes_seed_value_before_render() {
        let mut cx = TestAppContext::single();
        let input =
            cx.new(|cx| NyaNumberInputState::new(cx, "64", NyaNumberInputOptions::default()));

        assert_eq!(cx.read_entity(&input, |input, cx| input.value(cx)), "64");
    }
}
