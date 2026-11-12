mod agents;
mod general;
mod mcp_tasks;
mod models;
mod rules;
mod section;

use crate::theme::ThemePalette;
use gpui::{FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};

fn ai_card(
    palette: ThemePalette,
    title: impl IntoElement,
    action: impl IntoElement,
    content: impl IntoElement,
) -> gpui::Div {
    div()
        .min_w_0()
        .rounded_xl()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .overflow_hidden()
        .child(
            div()
                .px_5()
                .py_4()
                .border_b_1()
                .border_color(rgb(palette.border))
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(action),
        )
        .child(div().px_5().py_5().min_w_0().child(content))
}

fn ai_provider_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    control: impl IntoElement,
) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_3()
        .py_2()
        .border_b_1()
        .border_color(rgb(palette.border))
        .child(
            div()
                .w(px(145.))
                .text_size(px(12.))
                .text_color(rgb(palette.text))
                .child(label.into()),
        )
        .child(div().min_w(px(170.)).flex_1().child(control))
}

fn ai_field(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    desc: Option<SharedString>,
    control: impl IntoElement,
) -> gpui::Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.into()),
                )
                .when_some(desc, |body, desc| {
                    body.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text_muted))
                            .child(desc),
                    )
                }),
        )
        .child(div().w_full().min_w_0().child(control))
}
