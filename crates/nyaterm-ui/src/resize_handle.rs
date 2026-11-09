//! Shared visual affordances for pane resizing.

use crate::theme::ThemePalette;
use gpui::{div, prelude::*, px, rgb, rgba};

pub fn vertical_resize_handle_visual(
    palette: ThemePalette,
    dragging: bool,
    highlighted: bool,
) -> gpui::Div {
    div()
        .relative()
        .w(px(5.))
        .ml(px(-2.))
        .mr(px(-2.))
        .h_full()
        .flex_none()
        .child(
            div()
                .absolute()
                .left(px(2.))
                .top_0()
                .bottom_0()
                .w(px(1.))
                .bg(rgb(palette.border)),
        )
        .child(
            div()
                .absolute()
                .left(px(1.))
                .top_0()
                .bottom_0()
                .w(px(3.))
                .bg(if dragging || highlighted {
                    rgb(palette.primary)
                } else {
                    rgba(0x00000000)
                }),
        )
}

pub fn horizontal_resize_handle_visual(
    palette: ThemePalette,
    dragging: bool,
    highlighted: bool,
) -> gpui::Div {
    horizontal_resize_handle_visual_impl(palette, dragging, highlighted, true)
}

/// Same hit strip without the resting divider line. Use it where the dragged
/// edge already borders a framed control — an idle seam there reads as noise,
/// so only the hover/drag highlight shows.
pub fn horizontal_resize_handle_borderless(
    palette: ThemePalette,
    dragging: bool,
    highlighted: bool,
) -> gpui::Div {
    horizontal_resize_handle_visual_impl(palette, dragging, highlighted, false)
}

fn horizontal_resize_handle_visual_impl(
    palette: ThemePalette,
    dragging: bool,
    highlighted: bool,
    idle_line: bool,
) -> gpui::Div {
    div()
        .relative()
        .h(px(5.))
        .mt(px(-2.))
        .mb(px(-2.))
        .w_full()
        .flex_none()
        .when(idle_line, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(2.))
                    .h(px(1.))
                    .bg(rgb(palette.border)),
            )
        })
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(1.))
                .h(px(3.))
                .bg(if dragging || highlighted {
                    rgb(palette.primary)
                } else {
                    rgba(0x00000000)
                }),
        )
}
