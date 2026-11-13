//! Command rail geometry shared by terminal surfaces.
use gpui::{Div, div, prelude::*, px, rgb};

pub const COMMAND_RAIL_WIDTH: f32 = 19.0;

pub fn command_rail_row(
    height: f32,
    head: bool,
    tail: bool,
    collapsed: bool,
    color: u32,
    background: u32,
) -> Div {
    let middle = height * 0.5;
    let mut row = div()
        .relative()
        .w(px(COMMAND_RAIL_WIDTH))
        .h(px(height))
        .flex_none()
        .child(
            div()
                .absolute()
                .left(px(8.0))
                .top(px(if head { middle } else { 0.0 }))
                .w(px(1.0))
                .h(px(if head && tail {
                    0.0
                } else if head || tail {
                    middle
                } else {
                    height
                }))
                .bg(rgb(color)),
        );
    if tail {
        row = row.child(
            div()
                .absolute()
                .left(px(8.0))
                .top(px(middle))
                .w(px(6.0))
                .h(px(1.0))
                .bg(rgb(color)),
        );
    }
    if head {
        let mut button = div()
            .absolute()
            .left(px(2.0))
            .top(px((height - 12.0) * 0.5))
            .w(px(13.0))
            .h(px(12.0))
            .border_1()
            .border_color(rgb(color))
            .bg(rgb(background))
            .child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .top(px(4.0))
                    .w(px(7.0))
                    .h(px(1.0))
                    .bg(rgb(color)),
            );
        if collapsed {
            button = button.child(
                div()
                    .absolute()
                    .left(px(5.0))
                    .top(px(1.0))
                    .w(px(1.0))
                    .h(px(7.0))
                    .bg(rgb(color)),
            );
        }
        row = row.child(button);
    }
    row
}
