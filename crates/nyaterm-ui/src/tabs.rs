use gpui::{App, IntoElement, RenderOnce, ScrollHandle, SharedString, Window, div, prelude::*};
use gpui_kit::component::{
    Sizable,
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
};

use crate::sizing::form_control_size;

type NyaTabSelectHandler = Box<dyn Fn(&usize, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyaTabsVariant {
    Segmented,
    Pill,
    Underline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NyaTabItem {
    label: SharedString,
    disabled: bool,
}

impl NyaTabItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(IntoElement)]
pub struct NyaTabs {
    id: SharedString,
    items: Vec<NyaTabItem>,
    selected_index: Option<usize>,
    variant: NyaTabsVariant,
    full_width: bool,
    scroll_handle: Option<ScrollHandle>,
    on_select: Option<NyaTabSelectHandler>,
}

impl NyaTabs {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected_index: Some(0),
            variant: NyaTabsVariant::Segmented,
            full_width: true,
            scroll_handle: None,
            on_select: None,
        }
    }

    pub fn item(mut self, item: NyaTabItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = NyaTabItem>) -> Self {
        self.items.extend(items);
        self
    }

    pub fn selected_index(mut self, selected_index: usize) -> Self {
        self.selected_index = Some(selected_index);
        self
    }

    pub fn selected_index_if_visible(mut self, selected_index: Option<usize>) -> Self {
        self.selected_index = selected_index;
        self
    }

    pub fn variant(mut self, variant: NyaTabsVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// Keep labels at their natural width and expose a scrollbar and overflow menu.
    pub fn scrollable(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for NyaTabs {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut tabs = TabBar::new(self.id).with_size(form_control_size());
        if let Some(selected_index) = self.selected_index {
            tabs = tabs.selected_index(selected_index);
        }
        tabs = match self.variant {
            NyaTabsVariant::Segmented => tabs.segmented(),
            NyaTabsVariant::Pill => tabs.pill(),
            NyaTabsVariant::Underline => tabs.underline(),
        };
        if self.full_width {
            tabs = tabs.w_full();
        }
        let scrolling = self.scroll_handle.is_some();
        if let Some(handle) = &self.scroll_handle {
            tabs = tabs.track_scroll(handle).menu(true);
        }
        if let Some(on_select) = self.on_select {
            tabs = tabs.on_click(move |index, window, cx| on_select(index, window, cx));
        }
        let tabs = tabs
            .children(self.items.into_iter().map(|item| {
                Tab::new()
                    .label(item.label)
                    .disabled(item.disabled)
                    .when(!scrolling, |tab| tab.flex_1().min_w_0())
                    .when(scrolling, |tab| tab.flex_none().whitespace_nowrap())
            }))
            .last_empty_space(div());
        match self.scroll_handle {
            Some(handle) => div()
                .relative()
                .w_full()
                .child(tabs)
                .horizontal_scrollbar(&handle)
                .into_any_element(),
            None => tabs.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NyaTabItem, NyaTabs, NyaTabsVariant};
    use gpui::{Context, Render, ScrollHandle, TestAppContext, Window, div, point, prelude::*, px};

    struct ScrollTabsFixture {
        scroll: ScrollHandle,
    }

    impl Render for ScrollTabsFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(160.))
                .child(NyaTabs::new("scroll-tabs").scrollable(&self.scroll).items([
                    NyaTabItem::new("Keys"),
                    NyaTabItem::new("Passwords"),
                    NyaTabItem::new("OTP"),
                    NyaTabItem::new("Credentials"),
                    NyaTabItem::new("Known Hosts"),
                ]))
        }
    }

    #[gpui::test]
    fn natural_width_tabs_remain_scrollable_in_a_narrow_panel(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, _| ScrollTabsFixture {
            scroll: ScrollHandle::new(),
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let scroll = fixture.read_with(cx, |fixture, _| fixture.scroll.clone());
        assert!(
            scroll.max_offset().x > px(0.),
            "labels must not shrink to fit"
        );
        fixture.update(cx, |_, cx| {
            scroll.set_offset(point(px(-60.), px(0.)));
            cx.notify();
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(scroll.offset().x < px(0.));
    }

    #[test]
    fn segmented_tabs_default_to_full_width_equal_segments() {
        let tabs = NyaTabs::new("settings-tabs")
            .items([NyaTabItem::new("General"), NyaTabItem::new("Advanced")]);

        assert_eq!(tabs.variant, NyaTabsVariant::Segmented);
        assert!(tabs.full_width);
        assert_eq!(tabs.items.len(), 2);
    }
}
