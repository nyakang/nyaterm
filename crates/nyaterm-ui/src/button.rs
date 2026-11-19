use gpui::{
    AnyElement, App, ClickEvent, FontWeight, Hsla, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Icon, Selectable, Sizable};

use crate::sizing::{form_control_height, form_control_size};

type NyaButtonClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyaButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(IntoElement)]
pub struct NyaButton {
    id: SharedString,
    label: SharedString,
    icon_path: Option<SharedString>,
    content: Option<AnyElement>,
    variant: NyaButtonVariant,
    height: Option<Pixels>,
    text_color: Option<Hsla>,
    text_size: Option<Pixels>,
    font_weight: Option<FontWeight>,
    small: bool,
    compact: bool,
    form_control: bool,
    full_width: bool,
    label_left: bool,
    selected: bool,
    disabled: bool,
    loading: bool,
    autofocus: bool,
    tooltip: Option<SharedString>,
    on_click: Option<NyaButtonClickHandler>,
}

impl NyaButton {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon_path: None,
            content: None,
            variant: NyaButtonVariant::Secondary,
            height: None,
            text_color: None,
            text_size: None,
            font_weight: None,
            small: false,
            compact: false,
            form_control: false,
            full_width: false,
            label_left: false,
            selected: false,
            disabled: false,
            loading: false,
            autofocus: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    pub fn icon(mut self, icon_path: impl Into<SharedString>) -> Self {
        self.icon_path = Some(icon_path.into());
        self
    }

    pub fn variant(mut self, variant: NyaButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    pub fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = Some(size);
        self
    }

    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = Some(weight);
        self
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Matches ordinary single-line inputs, with room for a readable action label.
    pub fn form_control(mut self) -> Self {
        self.form_control = true;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Align the single button label to the start of the available content width.
    pub fn label_left(mut self) -> Self {
        self.label_left = true;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Focus this button when it first appears, without stealing focus on redraw.
    pub fn autofocus(mut self) -> Self {
        self.autofocus = true;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for NyaButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let icon_only = self.label.is_empty() && self.icon_path.is_some() && self.content.is_none();
        if self.autofocus && !self.disabled && !self.loading {
            // Initialize the same keyed focus state used by gpui-kit's Button.
            // Render it in this scope below so its focus ring and keyboard clicks
            // use this handle too. Keyed state expires when the button disappears.
            window.use_keyed_state(self.id.clone(), cx, |window, cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                focus
            });
        }
        let mut button = Button::new(self.id).loading(self.loading);
        if icon_only {
            // An empty label would create a second content slot and offset the icon.
            if let Some(tooltip) = &self.tooltip {
                button = button.accessibility_label(tooltip.clone());
            }
        } else if self.text_color.is_some()
            || self.text_size.is_some()
            || self.font_weight.is_some()
            || self.label_left
        {
            // Kit's label container sets its own size. Style the label itself,
            // retaining one visible label and its accessible name.
            button = button.accessibility_label(self.label.clone()).child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(self.label_left, |this| this.flex_1().text_left())
                    .when_some(self.text_color, |this, color| this.text_color(color))
                    .when_some(self.text_size, |this, size| this.text_size(size))
                    .when_some(self.font_weight, |this, weight| this.font_weight(weight))
                    .child(self.label),
            );
        } else {
            button = button.label(self.label);
        }
        if let Some(icon_path) = self.icon_path {
            button = button.icon(Icon::default().path(icon_path));
        }
        if let Some(content) = self.content {
            button = button.child(content);
        }
        if self.small {
            button = button.small();
        }
        if self.compact {
            button = button.compact();
        }
        if self.form_control {
            button = button
                .with_size(form_control_size())
                .h(form_control_height())
                .min_w(form_control_height())
                .px_3()
                .flex_none()
                .when(icon_only, |this| this.w(form_control_height()).px_0());
        }
        if self.full_width {
            button = button.w_full();
        }
        if self.selected {
            button = button.selected(true);
        }
        button = match self.variant {
            NyaButtonVariant::Primary => button.primary(),
            NyaButtonVariant::Secondary => button,
            NyaButtonVariant::Ghost => button.ghost(),
            NyaButtonVariant::Danger => button.danger(),
        };
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(tooltip);
        }
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        if let Some(height) = self.height {
            button = button.h(height);
        }
        if let Some(color) = self.text_color {
            button = button.text_color(color);
        }
        button.disabled(self.disabled).render(window, cx)
    }
}

#[derive(IntoElement)]
pub struct NyaIconButton {
    id: SharedString,
    icon_path: SharedString,
    icon_size: Option<Pixels>,
    size: Option<Pixels>,
    disabled: bool,
    tooltip: Option<SharedString>,
    tab_stop: bool,
    on_click: Option<NyaButtonClickHandler>,
}

impl NyaIconButton {
    pub fn new(id: impl Into<SharedString>, icon_path: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon_path: icon_path.into(),
            icon_size: None,
            size: None,
            disabled: false,
            tooltip: None,
            tab_stop: true,
            on_click: None,
        }
    }

    pub fn icon_size(mut self, size: Pixels) -> Self {
        self.icon_size = Some(size);
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }

    /// Gives a trailing input action the same height and a square hit target.
    pub fn form_control(self) -> Self {
        self.size(form_control_height())
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Removes the button from Tab traversal while keeping it clickable.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for NyaIconButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon = Icon::default()
            .path(self.icon_path)
            .when_some(self.icon_size, |icon, size| icon.with_size(size));
        let mut button = Button::new(self.id).icon(icon).ghost().small();
        if let Some(size) = self.size {
            button = button.size(size).flex_none();
        }
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(tooltip);
        }
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        button.disabled(self.disabled).tab_stop(self.tab_stop)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent,
        Keystroke, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
        Window, div,
    };

    use super::NyaButton;

    struct AutofocusHost {
        show_button: bool,
        other_focus: FocusHandle,
        clicks: usize,
    }

    fn press_enter(cx: &mut VisualTestContext) {
        let keystroke = Keystroke::parse("enter").unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
    }

    impl Render for AutofocusHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div()
                .size_full()
                .child(div().track_focus(&self.other_focus));
            if self.show_button {
                root = root.child(NyaButton::new("paste", "Paste").autofocus().on_click(
                    cx.listener(|this, _, _, cx| {
                        this.clicks += 1;
                        cx.notify();
                    }),
                ));
            }
            root
        }
    }

    #[gpui::test]
    fn autofocus_supports_enter_without_stealing_focus_on_redraw(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|_, cx| AutofocusHost {
            show_button: true,
            other_focus: cx.focus_handle(),
            clicks: 0,
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(window.focused(cx).is_some());
        });
        press_enter(cx);
        assert_eq!(host.read_with(cx, |host, _| host.clicks), 1);

        cx.update(|window, cx| {
            let other_focus = host.read(cx).other_focus.clone();
            window.focus(&other_focus, cx);
            host.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
            assert!(host.read(cx).other_focus.is_focused(window));
        });
        press_enter(cx);
        assert_eq!(host.read_with(cx, |host, _| host.clicks), 1);

        cx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.show_button = false;
                cx.notify();
            });
            let _ = window.draw(cx);
        });
        cx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.show_button = true;
                cx.notify();
            });
            let _ = window.draw(cx);
        });
        press_enter(cx);
        assert_eq!(host.read_with(cx, |host, _| host.clicks), 2);
    }

    #[test]
    fn ordinary_buttons_keep_content_width_and_no_icon_by_default() {
        let button = NyaButton::new("action", "Action");

        assert!(!button.full_width);
        assert!(button.icon_path.is_none());
    }

    #[test]
    fn full_width_is_opt_in_for_equal_width_dialog_actions() {
        let button = NyaButton::new("website", "Website").small().full_width();

        assert!(button.full_width);
        assert!(button.small);
        assert_eq!(button.label.as_ref(), "Website");
    }

    #[test]
    fn adding_an_icon_preserves_the_accessible_button_label() {
        let button = NyaButton::new("copy", "Copy all").icon("icons/copy.svg");

        assert_eq!(button.icon_path.as_deref(), Some("icons/copy.svg"));
        assert_eq!(button.label.as_ref(), "Copy all");
    }
}
