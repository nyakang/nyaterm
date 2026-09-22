use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement, Render, RenderOnce, SharedString,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::input::{
    InputEvent, InputState, MaskPattern, NumberInput, NumberInputEvent, StepAction,
};

use crate::input_focus::{preserve_nya_input_focus_on_pointer_down, register_nya_input_focus};
use crate::sizing::{form_control_height, form_control_size};

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
}

pub struct NyaNumberInputState {
    state: Option<Entity<InputState>>,
    seed: SharedString,
    pending_value: Option<SharedString>,
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
        let mask_pattern = (!self.options.allow_infinity).then_some(MaskPattern::Number {
            separator: None,
            fraction: self.options.decimal_places,
        });
        let state = cx.new(|cx| {
            let state = InputState::new(window, cx)
                .default_value(value)
                .placeholder(placeholder);
            if let Some(mask_pattern) = mask_pattern {
                state.mask_pattern(mask_pattern)
            } else {
                state
            }
        });
        register_nya_input_focus(&state.read(cx).focus_handle(cx), cx);
        self.subscriptions = vec![
            cx.subscribe_in(
                &state,
                window,
                |this, input, event: &NumberInputEvent, window, cx| {
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
                        cx.emit(NyaNumberInputEvent::Changed(
                            input.read(cx).value().to_string(),
                        ));
                    }
                    InputEvent::PressEnter { .. } => {
                        let committed = this.committed_value(input.read(cx).value().as_ref());
                        input.update(cx, |input, cx| {
                            input.set_value(SharedString::from(committed.clone()), window, cx);
                        });
                        cx.emit(NyaNumberInputEvent::Submitted(committed));
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
                    .with_size(form_control_size())
                    .h(form_control_height())
                    .appearance(self.appearance)
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
    use gpui::{AppContext as _, TestAppContext};

    use super::{
        NyaNumberInputOptions, NyaNumberInputState, NyaNumberStep, committed_number_text,
        stepped_number_text,
    };
    use crate::sizing::{NYA_FORM_CONTROL_HEIGHT_PX, form_control_size};

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
