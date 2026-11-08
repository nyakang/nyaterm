use gpui::{
    AnyElement, App, ElementId, IntoElement, Pixels, RenderOnce, ScrollHandle, SharedString,
    Window, div, prelude::*, px, rems,
};
use gpui_kit::component::{
    Icon, Sizable,
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
};

use crate::sizing::form_control_size;

type NyaTabSelectHandler = Box<dyn Fn(&usize, &mut Window, &mut App)>;

// Geometry of a medium segmented `gpui-kit` tab bar, mirrored so a fitted bar
// can decide its presentation before layout. Keep these in step with
// `TabVariant::inner_paddings` and the segmented `TabBar` paddings and gap;
// `fitted_label_tab_matches_mirrored_padding` guards the label padding.
const SEGMENTED_BAR_PADDING_X: f32 = 4.;
const SEGMENTED_TAB_GAP: f32 = 2.;
const SEGMENTED_LABEL_PADDING_X: f32 = 12.;
/// Minimum width NyaTerm enforces on an icon-only tab so it stays a usable
/// click target; `gpui-kit` would otherwise collapse it to the 16px glyph.
const SEGMENTED_ICON_TAB_WIDTH: f32 = 28.;
/// Medium tabs render labels with `text_sm`.
const SEGMENTED_LABEL_FONT_REMS: f32 = 0.875;
/// Slack per label for font fallback and subpixel rounding, so a label that
/// measures as just fitting is never clipped into an ellipsis.
const LABEL_FIT_SLACK: f32 = 4.;

/// How a width-fitted tab bar presents its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FittedTabsLayout {
    /// Every full label fits.
    Labels,
    /// Labels would truncate; every item shows its icon, label in a tooltip.
    Icons,
    /// Not even icons fit; items keep their natural width and scroll.
    Scroll,
}

/// Chooses the richest presentation that fits `available` without truncating.
///
/// Labels take their natural width plus padding, so the sum decides rather than
/// the widest label. Icons are only an option when every item has one.
fn choose_fitted_layout(
    available: f32,
    label_widths: &[f32],
    all_items_have_icons: bool,
) -> FittedTabsLayout {
    let count = label_widths.len();
    if count == 0 {
        return FittedTabsLayout::Labels;
    }
    let chrome = SEGMENTED_BAR_PADDING_X * 2. + SEGMENTED_TAB_GAP * (count - 1) as f32;
    let labels: f32 = label_widths
        .iter()
        .map(|width| width + SEGMENTED_LABEL_PADDING_X * 2. + LABEL_FIT_SLACK)
        .sum();
    if chrome + labels <= available {
        return FittedTabsLayout::Labels;
    }
    if all_items_have_icons && chrome + SEGMENTED_ICON_TAB_WIDTH * count as f32 <= available {
        return FittedTabsLayout::Icons;
    }
    FittedTabsLayout::Scroll
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyaTabsVariant {
    Segmented,
    Pill,
    Underline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NyaTabItem {
    label: SharedString,
    accessible_label: Option<SharedString>,
    icon_path: Option<SharedString>,
    disabled: bool,
}

impl NyaTabItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            accessible_label: None,
            icon_path: None,
            disabled: false,
        }
    }

    /// Tintable icon shown in place of the label when a fitted bar is too
    /// narrow for full labels. See [`NyaTabs::fit_width`].
    pub fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.icon_path = Some(path.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
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
    max_tab_width: Option<Pixels>,
    scroll_handle: Option<ScrollHandle>,
    fit_width: Option<Pixels>,
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
            max_tab_width: None,
            scroll_handle: None,
            fit_width: None,
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

    pub fn max_tab_width(mut self, width: Pixels) -> Self {
        self.max_tab_width = Some(width);
        self
    }

    /// Keep labels at their natural width and expose a scrollbar and overflow menu.
    pub fn scrollable(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// Never truncate labels: measure them against `available` (the bar's own
    /// width) and fall back to icon-only tabs, then to scrolling icons, when they
    /// do not fit. Segmented bars only; overrides `max_tab_width` and
    /// `scrollable`.
    pub fn fit_width(mut self, available: Pixels) -> Self {
        self.fit_width = Some(available);
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }

    fn measure_label_widths(&self, window: &Window) -> Vec<f32> {
        let font_size = rems(SEGMENTED_LABEL_FONT_REMS).to_pixels(window.rem_size());
        let style = window.text_style();
        self.items
            .iter()
            .map(|item| {
                let run = style.to_run(item.label.len());
                let line =
                    window
                        .text_system()
                        .shape_line(item.label.clone(), font_size, &[run], None);
                f32::from(line.width)
            })
            .collect()
    }

    fn render_fitted(self, available: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
        let label_widths = self.measure_label_widths(window);
        let all_items_have_icons = self.items.iter().all(|item| item.icon_path.is_some());
        let layout =
            choose_fitted_layout(f32::from(available), &label_widths, all_items_have_icons);
        let scroll_handle = (layout == FittedTabsLayout::Scroll).then(|| {
            window
                .use_keyed_state(
                    ElementId::Name(format!("{}-fit-scroll", self.id).into()),
                    cx,
                    |_, _| ScrollHandle::new(),
                )
                .read(cx)
                .clone()
        });
        let show_icons = layout != FittedTabsLayout::Labels && all_items_have_icons;

        let mut tabs = TabBar::new(self.id)
            .with_size(form_control_size())
            .segmented()
            .w_full();
        if let Some(selected_index) = self.selected_index {
            tabs = tabs.selected_index(selected_index);
        }
        if let Some(handle) = &scroll_handle {
            tabs = tabs.track_scroll(handle).menu(true);
        }
        if let Some(on_select) = self.on_select {
            tabs = tabs.on_click(move |index, window, cx| on_select(index, window, cx));
        }
        let tabs = tabs
            .children(self.items.into_iter().map(|item| {
                // Icon-only tabs still carry the label for the overflow menu,
                // the tooltip, and assistive technology.
                let tooltip = item
                    .accessible_label
                    .clone()
                    .unwrap_or_else(|| item.label.clone());
                let show_tooltip = show_icons || item.accessible_label.is_some();
                Tab::new()
                    .label(item.label)
                    .aria_label(tooltip.clone())
                    .when_some(item.icon_path.filter(|_| show_icons), |tab, path| {
                        tab.icon(Icon::default().path(path))
                    })
                    .when(show_tooltip, |tab| {
                        tab.tooltip(move |window, cx| {
                            Tooltip::new(tooltip.clone()).build(window, cx)
                        })
                    })
                    .disabled(item.disabled)
                    .map(|tab| match layout {
                        // Labels were measured to fit, so tabs grow from their
                        // natural width and never need to shrink.
                        FittedTabsLayout::Labels => tab.flex_auto().min_w_0(),
                        FittedTabsLayout::Icons => tab.flex_1().min_w(px(SEGMENTED_ICON_TAB_WIDTH)),
                        FittedTabsLayout::Scroll => tab
                            .flex_none()
                            .min_w(px(SEGMENTED_ICON_TAB_WIDTH))
                            .whitespace_nowrap(),
                    })
            }))
            .last_empty_space(div());
        match scroll_handle {
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

impl RenderOnce for NyaTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if let (Some(available), NyaTabsVariant::Segmented) = (self.fit_width, self.variant) {
            return self.render_fitted(available, window, cx);
        }
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
        if let Some(width) = self.max_tab_width {
            tabs = tabs.max_width(width);
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
                    .when_some(item.accessible_label, |tab, label| {
                        tab.aria_label(label.clone()).tooltip(move |window, cx| {
                            Tooltip::new(label.clone()).build(window, cx)
                        })
                    })
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
    use super::{
        FittedTabsLayout, NyaTabItem, NyaTabs, NyaTabsVariant, SEGMENTED_BAR_PADDING_X,
        SEGMENTED_ICON_TAB_WIDTH, SEGMENTED_LABEL_PADDING_X, SEGMENTED_TAB_GAP,
        choose_fitted_layout,
    };
    use crate::sizing::form_control_size;
    use gpui::{Context, Render, ScrollHandle, TestAppContext, Window, div, point, prelude::*, px};
    use gpui_kit::component::{
        Sizable,
        tab::{Tab, TabBar},
    };

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

    const FIVE_LABELS: [f32; 5] = [30., 66., 26., 74., 80.];

    #[test]
    fn fitted_tabs_keep_full_labels_when_their_natural_widths_fit() {
        assert_eq!(
            choose_fitted_layout(480., &FIVE_LABELS, true),
            FittedTabsLayout::Labels
        );
    }

    #[test]
    fn fitted_tabs_switch_to_icons_instead_of_truncating_labels() {
        // The reported case: a ~240px side panel minus its 24px padding.
        assert_eq!(
            choose_fitted_layout(216., &FIVE_LABELS, true),
            FittedTabsLayout::Icons
        );
    }

    #[test]
    fn fitted_tabs_scroll_when_even_icons_do_not_fit() {
        assert_eq!(
            choose_fitted_layout(136., &FIVE_LABELS, true),
            FittedTabsLayout::Scroll
        );
    }

    #[test]
    fn fitted_tabs_without_icons_scroll_rather_than_truncate() {
        assert_eq!(
            choose_fitted_layout(216., &FIVE_LABELS, false),
            FittedTabsLayout::Scroll
        );
    }

    #[test]
    fn fitted_layout_thresholds_are_inclusive() {
        let chrome = SEGMENTED_BAR_PADDING_X * 2. + SEGMENTED_TAB_GAP * 4.;
        let icons_exact = chrome + SEGMENTED_ICON_TAB_WIDTH * 5.;
        assert_eq!(
            choose_fitted_layout(icons_exact, &FIVE_LABELS, true),
            FittedTabsLayout::Icons
        );
        assert_eq!(
            choose_fitted_layout(icons_exact - 1., &FIVE_LABELS, true),
            FittedTabsLayout::Scroll
        );
    }

    const PROBE_CONTENT_WIDTH: f32 = 40.;

    struct LabelTabFixture;

    impl Render for LabelTabFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // A fixed-width child stands in for label text, which the test
            // platform cannot measure deterministically.
            TabBar::new("label-tab")
                .with_size(form_control_size())
                .segmented()
                .selected_index(0)
                .child(
                    Tab::new()
                        .child(div().w(px(PROBE_CONTENT_WIDTH)).flex_none())
                        .debug_selector(|| "label-tab".to_string()),
                )
        }
    }

    /// The fit decision is made before layout from mirrored `gpui-kit`
    /// geometry; fail loudly if a real label tab stops matching it.
    #[gpui::test]
    fn fitted_label_tab_matches_mirrored_padding(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| LabelTabFixture);
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("label-tab")
            .expect("label tab not rendered");
        assert_eq!(
            bounds.size.width,
            px(PROBE_CONTENT_WIDTH + SEGMENTED_LABEL_PADDING_X * 2.)
        );
    }
}
