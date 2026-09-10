use gpui::{
    AnyElement, App, ClickEvent, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    Window, prelude::FluentBuilder as _,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Disableable, Icon, Selectable, Sizable};

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
    content: Option<AnyElement>,
    variant: NyaButtonVariant,
    small: bool,
    compact: bool,
    selected: bool,
    disabled: bool,
    loading: bool,
    tooltip: Option<SharedString>,
    on_click: Option<NyaButtonClickHandler>,
}

impl NyaButton {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            content: None,
            variant: NyaButtonVariant::Secondary,
            small: false,
            compact: false,
            selected: false,
            disabled: false,
            loading: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    pub fn variant(mut self, variant: NyaButtonVariant) -> Self {
        self.variant = variant;
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
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut button = Button::new(self.id).label(self.label).loading(self.loading);
        if let Some(content) = self.content {
            button = button.child(content);
        }
        if self.small {
            button = button.small();
        }
        if self.compact {
            button = button.compact();
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
        button.disabled(self.disabled)
    }
}

#[derive(IntoElement)]
pub struct NyaIconButton {
    id: SharedString,
    icon_path: SharedString,
    icon_size: Option<Pixels>,
    disabled: bool,
    tooltip: Option<SharedString>,
    on_click: Option<NyaButtonClickHandler>,
}

impl NyaIconButton {
    pub fn new(id: impl Into<SharedString>, icon_path: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon_path: icon_path.into(),
            icon_size: None,
            disabled: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn icon_size(mut self, size: Pixels) -> Self {
        self.icon_size = Some(size);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
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

impl RenderOnce for NyaIconButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon = Icon::default()
            .path(self.icon_path)
            .when_some(self.icon_size, |icon, size| icon.with_size(size));
        let mut button = Button::new(self.id).icon(icon).ghost().small();
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(tooltip);
        }
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        button.disabled(self.disabled)
    }
}
