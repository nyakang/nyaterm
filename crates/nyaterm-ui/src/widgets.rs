use crate::button::{NyaButton, NyaButtonVariant, NyaIconButton};
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, App, ClickEvent, FontWeight, Hsla, IntoElement, Pixels, RenderOnce, ScrollHandle,
    SharedString, UniformListScrollHandle, Window, div, prelude::*, px, rgb,
};
use gpui_kit::component::scroll::Scrollbar;

fn platform_code_font_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "JetBrains Mono"
    }
}

#[derive(IntoElement)]
pub struct NyaScrollArea {
    id: SharedString,
    max_height: Option<Pixels>,
    children: Vec<AnyElement>,
}

impl NyaScrollArea {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            max_height: None,
            children: Vec::new(),
        }
    }

    pub fn max_h(mut self, height: Pixels) -> Self {
        self.max_height = Some(height);
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl RenderOnce for NyaScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id;
        let scroll_id = SharedString::from(format!("{}/scroll", id.as_ref()));
        let viewport_id = SharedString::from(format!("{}/viewport", id.as_ref()));
        let scrollbar_layer_id = SharedString::from(format!("{}/scrollbar-layer", id.as_ref()));
        let scrollbar_id = SharedString::from(format!("{}/scrollbar", id.as_ref()));
        let scrollbar_layer_selector = scrollbar_layer_id.to_string();
        let scroll_handle = window
            .use_keyed_state(scroll_id, cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        let mut viewport = div()
            .id(viewport_id)
            .w_full()
            .flex()
            .flex_col()
            .children(self.children)
            .overflow_y_scroll()
            .track_scroll(&scroll_handle);
        if let Some(max_height) = self.max_height {
            viewport = viewport.max_h(max_height);
        }

        div()
            .id(id.clone())
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .child(viewport)
            .child(
                div()
                    .id(scrollbar_layer_id)
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .debug_selector(move || scrollbar_layer_selector.clone())
                    .child(
                        Scrollbar::vertical(&scroll_handle)
                            .id(scrollbar_id)
                            .viewport_from_layout(),
                    ),
            )
    }
}

/// A component-themed vertical scrollbar for GPUI's virtualized uniform list.
#[derive(IntoElement)]
pub struct NyaUniformListScrollbar {
    id: SharedString,
    handle: UniformListScrollHandle,
}

impl NyaUniformListScrollbar {
    pub fn new(id: impl Into<SharedString>, handle: &UniformListScrollHandle) -> Self {
        Self {
            id: id.into(),
            handle: handle.clone(),
        }
    }
}

/// A component-themed horizontal scrollbar for a GPUI scroll container.
#[derive(IntoElement)]
pub struct NyaHorizontalScrollbar {
    id: SharedString,
    handle: ScrollHandle,
}

impl NyaHorizontalScrollbar {
    pub fn new(id: impl Into<SharedString>, handle: &ScrollHandle) -> Self {
        Self {
            id: id.into(),
            handle: handle.clone(),
        }
    }
}

impl RenderOnce for NyaHorizontalScrollbar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        Scrollbar::horizontal(&self.handle)
            .id(self.id)
            .viewport_from_layout()
    }
}

impl RenderOnce for NyaUniformListScrollbar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        Scrollbar::vertical(&self.handle)
            .id(self.id)
            .viewport_from_layout()
    }
}

pub fn status_pill(
    label: impl Into<SharedString>,
    fg: impl Into<Hsla>,
    bg: impl Into<Hsla>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .rounded_sm()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(fg.into())
        .bg(bg.into())
        .child(label)
}

pub fn empty_panel(text: impl Into<SharedString>, palette: ThemePalette) -> impl IntoElement {
    empty_panel_with_icon(text, palette, "icons/eye-off.svg")
}

pub fn empty_panel_with_icon(
    text: impl Into<SharedString>,
    palette: ThemePalette,
    icon_path: &'static str,
) -> impl IntoElement {
    let text: SharedString = text.into();
    div()
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .text_center()
        .child(
            gpui::svg()
                .size(px(24.))
                .path(icon_path)
                .text_color(rgb(palette.text_dimmed)),
        )
        .child(
            div()
                .text_sm()
                .line_height(px(20.))
                .text_color(rgb(palette.text_muted))
                .child(text),
        )
}

pub fn section_header(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    palette: ThemePalette,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let detail: SharedString = detail.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_2xl().font_weight(FontWeight(800.)).child(title))
        .child(
            div()
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .child(detail),
        )
}

pub fn capability_line(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .mt_2()
        .flex()
        .items_center()
        .justify_between()
        .text_sm()
        .child(div().text_color(rgb(palette.text)).child(label))
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(value.into()),
        )
}

pub fn session_info_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: String,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .px_3()
        .py_2()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(104.))
                .flex_none()
                .text_xs()
                .font_weight(FontWeight(700.))
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .font_family(platform_code_font_family())
                .text_xs()
                .text_color(rgb(palette.text))
                .child(value),
        )
}

pub fn small_button(
    _palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    NyaButton::new(id.into(), label.into())
        .variant(NyaButtonVariant::Secondary)
        .small()
        .compact()
        .on_click(on_click)
}

pub fn mode_button(
    id: impl Into<String>,
    label: impl Into<SharedString>,
    active: bool,
    _palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    NyaButton::new(id.into(), label.into())
        .variant(NyaButtonVariant::Ghost)
        .selected(active)
        .small()
        .compact()
        .on_click(on_click)
}

pub fn svg_icon_button(
    id: impl Into<String>,
    icon_path: &'static str,
    icon_size: f32,
    _palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    NyaIconButton::new(id.into(), icon_path)
        .icon_size(px(icon_size))
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::{NyaHorizontalScrollbar, NyaScrollArea};
    use gpui::{
        Context, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, TestAppContext,
        VisualTestContext, Window, div, point, prelude::*, px,
    };

    struct MaxHeightScrollAreaFixture;

    struct HorizontalScrollbarFixture {
        scroll: ScrollHandle,
    }

    impl Render for HorizontalScrollbarFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(100.))
                .h(px(40.))
                .overflow_hidden()
                .child(
                    div()
                        .id("horizontal-scroll-area")
                        .size_full()
                        .overflow_x_scroll()
                        .overflow_y_hidden()
                        .restrict_scroll_to_axis()
                        .track_scroll(&self.scroll)
                        .child(div().w(px(300.)).h_full()),
                )
                // Mirrors the file browser: the overlay spans the viewport so the
                // bar's hitbox is the panel, inset on the right by one track width
                // to leave room for a vertical bar.
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .right(px(16.))
                        .debug_selector(|| "horizontal-scrollbar-layer".to_string())
                        .child(NyaHorizontalScrollbar::new(
                            "horizontal-scrollbar",
                            &self.scroll,
                        )),
                )
        }
    }

    impl Render for MaxHeightScrollAreaFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(120.))
                .child(
                    NyaScrollArea::new("max-height-scroll-area")
                        .max_h(px(60.))
                        .child(row("first-scroll-row"))
                        .child(row("middle-scroll-row"))
                        .child(row("last-scroll-row")),
                )
                .child(
                    div()
                        .h(px(10.))
                        .flex_shrink_0()
                        .debug_selector(|| "scroll-footer".to_string()),
                )
        }
    }

    fn row(selector: &'static str) -> impl IntoElement {
        div()
            .h(px(30.))
            .flex_shrink_0()
            .debug_selector(move || selector.to_string())
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    fn scroll(cx: &mut VisualTestContext, dy: f32) {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            ..Default::default()
        });
        draw(cx);
    }

    fn scroll_xy(cx: &mut VisualTestContext, dx: f32, dy: f32) {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            ..Default::default()
        });
        draw(cx);
    }

    #[gpui::test]
    fn max_height_scroll_area_handles_wheel_events(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| MaxHeightScrollAreaFixture);
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        let footer = cx.debug_bounds("scroll-footer").unwrap();
        assert_eq!(footer.top(), px(60.));
        assert!(
            cx.debug_bounds("max-height-scroll-area/scrollbar-layer")
                .is_some()
        );

        let initial_last_y = cx.debug_bounds("last-scroll-row").unwrap().origin.y;
        scroll(cx, -30.);

        let scrolled_last_y = cx.debug_bounds("last-scroll-row").unwrap().origin.y;
        assert!(scrolled_last_y < initial_last_y);
    }

    #[gpui::test]
    fn horizontal_scrollbar_tracks_only_horizontal_wheel_input(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let scroll = ScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll = scroll.clone();
            move |_, _| HorizontalScrollbarFixture {
                scroll: scroll.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        // The overlay spans the scroll viewport rather than a thin bottom strip:
        // hover-to-reveal watches the bar's own hitbox, so a strip-sized overlay
        // would only react within a track width of the edge.
        let layer = cx.debug_bounds("horizontal-scrollbar-layer").unwrap();
        assert_eq!(layer.top(), px(0.));
        assert_eq!(layer.bottom(), px(40.));
        assert_eq!(layer.left(), px(0.));
        assert_eq!(layer.right(), px(84.));

        scroll_xy(cx, 0., -30.);
        assert_eq!(scroll.offset().x, px(0.));

        scroll_xy(cx, -30., 0.);
        assert!(scroll.offset().x < px(0.));
    }
}
