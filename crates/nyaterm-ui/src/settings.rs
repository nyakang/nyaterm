use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, FontWeight, IntoElement, RenderOnce, Rgba, ScrollHandle,
    SharedString, Window, canvas, div, prelude::*, px, rgb, rgba, svg,
};

use gpui_kit::component::scroll::ScrollableElement;

use crate::theme::{ThemePalette, theme_palette};
use crate::tooltip::NyaTooltip;

type NyaSettingsSelectHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;
type NyaSettingsGroupToggleHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;
type NyaSettingsContentWidthHandler = Rc<dyn Fn(f32, &mut Window, &mut App)>;

const DEFAULT_COMPACT_BREAKPOINT: f32 = 640.;
const DEFAULT_WIDE_BREAKPOINT: f32 = 1024.;
const COMPACT_SIDEBAR_WIDTH: f32 = 56.;
const MEDIUM_SIDEBAR_WIDTH: f32 = 192.;
const WIDE_SIDEBAR_WIDTH: f32 = 224.;

#[derive(Clone, Debug)]
pub struct NyaSettingsNavItem {
    id: SharedString,
    label: SharedString,
    icon_path: SharedString,
}

impl NyaSettingsNavItem {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon_path: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon_path: icon_path.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NyaSettingsNavGroup {
    id: Option<SharedString>,
    title: Option<SharedString>,
    icon_path: Option<SharedString>,
    accent: u32,
    expanded: bool,
    items: Vec<NyaSettingsNavItem>,
}

impl NyaSettingsNavGroup {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        icon_path: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: Some(id.into()),
            title: Some(title.into()),
            icon_path: Some(icon_path.into()),
            accent: 0,
            expanded: true,
            items: Vec::new(),
        }
    }

    pub fn standalone(items: impl IntoIterator<Item = NyaSettingsNavItem>) -> Self {
        Self {
            id: None,
            title: None,
            icon_path: None,
            accent: 0,
            expanded: true,
            items: items.into_iter().collect(),
        }
    }

    pub fn accent(mut self, accent: u32) -> Self {
        self.accent = accent;
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn item(mut self, item: NyaSettingsNavItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = NyaSettingsNavItem>) -> Self {
        self.items.extend(items);
        self
    }
}

#[derive(IntoElement)]
pub struct NyaSettingsLayout {
    id: SharedString,
    groups: Vec<NyaSettingsNavGroup>,
    active_item_id: SharedString,
    active_title: SharedString,
    sidebar_title: SharedString,
    content: AnyElement,
    palette: ThemePalette,
    content_background: Option<Rgba>,
    viewport_width: Option<f32>,
    on_content_width_changed: Option<NyaSettingsContentWidthHandler>,
    content_scroll_handle: Option<ScrollHandle>,
    compact_breakpoint: f32,
    wide_breakpoint: f32,
    on_select: Option<NyaSettingsSelectHandler>,
    on_toggle_group: Option<NyaSettingsGroupToggleHandler>,
}

impl NyaSettingsLayout {
    pub fn new(
        id: impl Into<SharedString>,
        groups: impl IntoIterator<Item = NyaSettingsNavGroup>,
        active_item_id: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        let groups: Vec<NyaSettingsNavGroup> = groups.into_iter().collect();
        let active_item_id = active_item_id.into();
        let active_title = active_item_title(&groups, &active_item_id).unwrap_or_else(|| {
            groups
                .iter()
                .flat_map(|group| group.items.iter())
                .next()
                .map(|item| item.label.clone())
                .unwrap_or_default()
        });
        Self {
            id: id.into(),
            groups,
            active_item_id,
            active_title,
            sidebar_title: SharedString::from("Settings"),
            content: content.into_any_element(),
            palette: theme_palette("github-dark"),
            content_background: None,
            viewport_width: None,
            on_content_width_changed: None,
            content_scroll_handle: None,
            compact_breakpoint: DEFAULT_COMPACT_BREAKPOINT,
            wide_breakpoint: DEFAULT_WIDE_BREAKPOINT,
            on_select: None,
            on_toggle_group: None,
        }
    }

    pub fn palette(mut self, palette: ThemePalette) -> Self {
        self.palette = palette;
        self
    }

    /// Overrides the opaque `palette.bg` content fill, e.g. so a translucent
    /// host surface shows through instead of being painted over.
    pub fn content_background(mut self, background: impl Into<Rgba>) -> Self {
        self.content_background = Some(background.into());
        self
    }

    pub fn active_title(mut self, title: impl Into<SharedString>) -> Self {
        self.active_title = title.into();
        self
    }

    pub fn sidebar_title(mut self, title: impl Into<SharedString>) -> Self {
        self.sidebar_title = title.into();
        self
    }

    /// Sets the available host width in logical pixels, including the sidebar.
    /// Embedded layouts should pass their pane width rather than the window width.
    pub fn viewport_width(mut self, width: f32) -> Self {
        self.viewport_width = Some(width);
        self
    }

    /// Reports the laid-out body width after padding and the maximum-width cap.
    /// Called after layout; consumers should ignore unchanged measurements.
    pub fn on_content_width_changed(
        mut self,
        handler: impl Fn(f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_content_width_changed = Some(Rc::new(handler));
        self
    }

    pub fn content_scroll_handle(mut self, handle: ScrollHandle) -> Self {
        self.content_scroll_handle = Some(handle);
        self
    }

    pub fn compact_breakpoint(mut self, breakpoint: f32) -> Self {
        self.compact_breakpoint = breakpoint;
        self
    }

    pub fn wide_breakpoint(mut self, breakpoint: f32) -> Self {
        self.wide_breakpoint = breakpoint;
        self
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_toggle_group(
        mut self,
        handler: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_group = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NyaSettingsLayout {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let viewport_width = self
            .viewport_width
            .unwrap_or_else(|| f32::from(window.viewport_size().width));
        let compact = viewport_width < self.compact_breakpoint;
        let wide = viewport_width >= self.wide_breakpoint;
        let sidebar_width = if compact {
            COMPACT_SIDEBAR_WIDTH
        } else if wide {
            WIDE_SIDEBAR_WIDTH
        } else {
            MEDIUM_SIDEBAR_WIDTH
        };
        let palette = self.palette;
        let active_item_id = self.active_item_id.clone();
        let on_select = self.on_select.clone();
        let on_toggle_group = self.on_toggle_group.clone();
        let content = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex_none()
            .child(self.content)
            .when_some(self.on_content_width_changed, |this, handler| {
                this.child(
                    canvas(
                        move |bounds, window, cx| {
                            let width = f32::from(bounds.size.width);
                            let handler = handler.clone();
                            window.defer(cx, move |window, cx| handler(width, window, cx));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
            });

        div()
            .id(self.id)
            .flex()
            .flex_1()
            .min_w_0()
            .min_h(px(0.))
            .size_full()
            .child(settings_sidebar(SettingsSidebarParams {
                palette,
                sidebar_width,
                compact,
                sidebar_title: self.sidebar_title,
                groups: self.groups,
                active_item_id,
                on_select,
                on_toggle_group,
            }))
            .child(settings_content_panel(
                palette,
                self.content_background.unwrap_or_else(|| rgb(palette.bg)),
                compact,
                wide,
                self.active_title,
                content.into_any_element(),
                self.content_scroll_handle,
            ))
    }
}

fn active_item_title(
    groups: &[NyaSettingsNavGroup],
    active_item_id: &SharedString,
) -> Option<SharedString> {
    groups
        .iter()
        .flat_map(|group| group.items.iter())
        .find(|item| &item.id == active_item_id)
        .map(|item| item.label.clone())
}

struct SettingsSidebarParams {
    palette: ThemePalette,
    sidebar_width: f32,
    compact: bool,
    sidebar_title: SharedString,
    groups: Vec<NyaSettingsNavGroup>,
    active_item_id: SharedString,
    on_select: Option<NyaSettingsSelectHandler>,
    on_toggle_group: Option<NyaSettingsGroupToggleHandler>,
}

fn settings_sidebar(params: SettingsSidebarParams) -> impl IntoElement {
    let SettingsSidebarParams {
        palette,
        sidebar_width,
        compact,
        sidebar_title,
        groups,
        active_item_id,
        on_select,
        on_toggle_group,
    } = params;
    div()
        .debug_selector(|| "settings-sidebar".into())
        .w(px(sidebar_width))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgba((palette.border << 8) | 0xb3))
        .bg(rgba((palette.surface_elevated << 8) | 0x33))
        .child(settings_sidebar_header(palette, compact, sidebar_title))
        .child(
            div()
                .id(SharedString::from("settings-sidebar-scroll"))
                .flex_1()
                .min_h_0()
                .when(compact, |this| this.px_2().py_3())
                .when(!compact, |this| this.px_3().py_3())
                .overflow_y_scrollbar()
                .children(groups.into_iter().map(|group| {
                    settings_nav_group(
                        palette,
                        compact,
                        group,
                        active_item_id.clone(),
                        on_select.clone(),
                        on_toggle_group.clone(),
                    )
                    .into_any_element()
                })),
        )
}

fn settings_sidebar_header(
    palette: ThemePalette,
    compact: bool,
    sidebar_title: SharedString,
) -> impl IntoElement {
    div()
        .h(px(64.))
        .flex_none()
        .flex()
        .items_center()
        .gap_3()
        .when(compact, |this| this.justify_center())
        .when(!compact, |this| this.px_3())
        .border_b_1()
        .border_color(rgba((palette.border << 8) | 0xb3))
        .child(
            svg()
                .size(px(if compact { 22. } else { 24. }))
                .flex_none()
                .path("icons/settings.svg")
                .text_color(rgb(palette.primary)),
        )
        .when(!compact, |this| {
            this.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(16.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text))
                    .child(sidebar_title),
            )
        })
}

fn settings_nav_group(
    palette: ThemePalette,
    compact: bool,
    group: NyaSettingsNavGroup,
    active_item_id: SharedString,
    on_select: Option<NyaSettingsSelectHandler>,
    on_toggle_group: Option<NyaSettingsGroupToggleHandler>,
) -> impl IntoElement {
    let nested = group.title.is_some();
    let expanded = group.expanded || !nested;
    let header = group_header(palette, compact, &group, on_toggle_group);
    let items = group.items;

    div()
        .flex()
        .flex_col()
        .when_some(header, |this, header| this.child(header))
        .when(expanded, |this| {
            let children = div()
                .flex()
                .flex_col()
                .when(nested && !compact, |this| {
                    this.ml_4()
                        .pl_3()
                        .border_l_1()
                        .border_color(rgba((palette.border << 8) | 0xb3))
                })
                .children(items.into_iter().map(|item| {
                    settings_nav_item(
                        palette,
                        compact,
                        nested,
                        item,
                        active_item_id.clone(),
                        on_select.clone(),
                    )
                    .into_any_element()
                }));
            this.child(children)
        })
}

fn group_header(
    palette: ThemePalette,
    compact: bool,
    group: &NyaSettingsNavGroup,
    on_toggle_group: Option<NyaSettingsGroupToggleHandler>,
) -> Option<AnyElement> {
    let id = group.id.clone()?;
    let title = group.title.clone()?;
    let icon_path = group.icon_path.clone()?;
    let expanded = group.expanded;
    let toggle_id = id.clone();
    let on_toggle = on_toggle_group.clone();

    Some(
        div()
            .id(SharedString::from(format!("settings-group-{id}")))
            .mt_1()
            .mb_1()
            .h(px(40.))
            .when(!compact, |this| this.px_3())
            .flex()
            .items_center()
            .when(compact, |this| this.justify_center())
            .when(!compact, |this| this.justify_between())
            .rounded_lg()
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(palette.hover)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text_muted))
                    .child(
                        svg()
                            .size(px(18.))
                            .path(icon_path)
                            .text_color(rgb(palette.text_muted)),
                    )
                    .when(!compact, |this| this.child(title.clone())),
            )
            .when(!compact, |this| {
                this.child(
                    svg()
                        .size(px(18.))
                        .flex_none()
                        .path(if expanded {
                            "icons/chevron-down.svg"
                        } else {
                            "icons/fe/forward.svg"
                        })
                        .text_color(rgb(palette.text_dimmed)),
                )
            })
            .when(compact, |this| {
                let title = title.clone();
                this.tooltip(move |window, cx| NyaTooltip::new(title.clone()).build(window, cx))
            })
            .on_click(move |_: &ClickEvent, window, cx| {
                if let Some(on_toggle) = &on_toggle {
                    on_toggle(toggle_id.clone(), window, cx);
                }
            })
            .into_any_element(),
    )
}

fn settings_nav_item(
    palette: ThemePalette,
    compact: bool,
    nested: bool,
    item: NyaSettingsNavItem,
    active_item_id: SharedString,
    on_select: Option<NyaSettingsSelectHandler>,
) -> impl IntoElement {
    let selected = item.id == active_item_id;
    let item_id = item.id.clone();
    let label = item.label.clone();
    let icon_path = item.icon_path.clone();
    let on_select = on_select.clone();

    div()
        .id(item.id)
        .mt(if nested { px(4.) } else { px(8.) })
        .h(px(if nested { 34. } else { 40. }))
        .when(!compact, |this| this.px_3())
        .flex()
        .items_center()
        .when(compact, |this| this.justify_center())
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(if selected {
            rgba((palette.primary << 8) | 0x33)
        } else {
            rgba(0x00000000)
        })
        .bg(if selected {
            rgba((palette.primary << 8) | 0x1f)
        } else {
            rgba(0x00000000)
        })
        .text_color(if selected {
            rgb(palette.text)
        } else {
            rgb(palette.text_muted)
        })
        .text_size(px(if nested { 13. } else { 14. }))
        .font_weight(if selected {
            FontWeight(600.)
        } else {
            FontWeight(500.)
        })
        .cursor_pointer()
        .hover(move |this| {
            this.bg(if selected {
                rgba((palette.primary << 8) | 0x29)
            } else {
                rgb(palette.hover)
            })
            .text_color(rgb(palette.text))
        })
        .child(
            svg()
                .size(px(if nested { 16. } else { 18. }))
                .flex_none()
                .path(icon_path)
                .text_color(if selected {
                    rgb(palette.text)
                } else {
                    rgb(palette.text_muted)
                }),
        )
        .when(!compact, |this| {
            this.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .child(label.clone()),
            )
        })
        .when(compact, |this| {
            let label = label.clone();
            this.tooltip(move |window, cx| NyaTooltip::new(label.clone()).build(window, cx))
        })
        .on_click(move |_: &ClickEvent, window, cx| {
            if let Some(on_select) = &on_select {
                on_select(item_id.clone(), window, cx);
            }
        })
}

fn settings_content_panel(
    palette: ThemePalette,
    background: Rgba,
    compact: bool,
    wide: bool,
    active_title: SharedString,
    content: AnyElement,
    scroll_handle: Option<ScrollHandle>,
) -> impl IntoElement {
    let handle = scroll_handle.unwrap_or_default();
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .bg(background)
        .child(
            div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .when(compact, |this| this.px_4().py_4())
                        .when(!compact, |this| this.px_6().py_5())
                        .border_b_1()
                        .border_color(rgb(palette.border))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(if compact { 18. } else { 24. }))
                                .font_weight(FontWeight(700.))
                                .text_color(rgb(palette.text))
                                .child(active_title),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .vertical_scrollbar(&handle)
                        .child(
                            div()
                                .id(SharedString::from("settings-content-scroll"))
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&handle)
                                .when(compact, |this| this.px_4().py_4())
                                .when(!compact && !wide, |this| this.px_6().py_6())
                                .when(wide, |this| this.px_8().py_8())
                                .child(
                                    div()
                                        .debug_selector(|| "settings-content-body".into())
                                        .w_full()
                                        .min_w_0()
                                        .flex_none()
                                        .max_w(px(1024.))
                                        .mx_auto()
                                        .flex()
                                        .flex_col()
                                        .gap(if compact { px(20.) } else { px(24.) })
                                        .child(content),
                                ),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size};

    use super::{NyaSettingsLayout, NyaSettingsNavGroup, NyaSettingsNavItem};

    struct LayoutFixture {
        width: f32,
        measured_width: Rc<Cell<f32>>,
    }

    impl Render for LayoutFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let measured = self.measured_width.clone();
            div().w(px(self.width)).h(px(400.)).child(
                NyaSettingsLayout::new(
                    "fixture",
                    [NyaSettingsNavGroup::standalone([NyaSettingsNavItem::new(
                        "models",
                        "Models",
                        "icons/settings.svg",
                    )])],
                    "models",
                    div().h(px(1200.)),
                )
                .viewport_width(self.width)
                .on_content_width_changed(move |width, _, _| measured.set(width)),
            )
        }
    }

    #[gpui::test]
    fn embedded_layout_measures_body_after_sidebar_padding_and_width_cap(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let measured_width = Rc::new(Cell::new(0.));
        let (fixture, cx) = cx.add_window_view(|_, _| LayoutFixture {
            width: 395.,
            measured_width: measured_width.clone(),
        });
        cx.simulate_resize(size(px(1800.), px(960.)));
        for (width, sidebar, padding) in [
            (395., 56., 32.),
            (639., 56., 32.),
            (640., 192., 48.),
            (1023., 192., 48.),
            (1024., 224., 64.),
            (1600., 224., 64.),
            (395., 56., 32.),
        ] {
            fixture.update(cx, |fixture, cx| {
                fixture.width = width;
                cx.notify();
            });
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
            cx.run_until_parked();
            let nav = cx.debug_bounds("settings-sidebar").unwrap();
            let body = cx.debug_bounds("settings-content-body").unwrap();
            let expected = (width - sidebar - padding).min(1024.);
            assert_eq!(nav.size.width, px(sidebar), "host width {width}");
            assert_eq!(body.size.width, px(expected), "host width {width}");
            assert_eq!(measured_width.get(), expected);
            assert!(body.left() >= nav.right());
            assert!(body.right() <= px(width));
            assert_eq!(
                body.size.height,
                px(1200.),
                "body must retain its natural height"
            );
        }
    }
}
