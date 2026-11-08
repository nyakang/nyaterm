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
    div()
        .relative()
        .h(px(5.))
        .mt(px(-2.))
        .mb(px(-2.))
        .w_full()
        .flex_none()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(2.))
                .h(px(1.))
                .bg(rgb(palette.border)),
        )
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
