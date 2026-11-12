//! Stateless presentation helpers. Applications supply labels and actions.
use gpui::{
    App, ClickEvent, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px, rgb,
};

use crate::theme::ThemePalette;

/// Tauri EmptyWorkspaceState row: action label (primary) + shortcut key chips.
pub fn shortcut_action(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    shortcut: impl Into<String>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    // Tauri EmptyWorkspaceState: primary label left, Kbd chips right with "+" separators.
    let label = label.into();
    let shortcut = shortcut.into();
    let parts: Vec<String> = shortcut
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let mut keys = div().flex().items_center().gap_1();
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            keys = keys.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child("+"),
            );
        }
        keys = keys.child(
            div()
                .h(px(24.))
                .min_w(px(28.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(part),
        );
    }

    // Tauri: grid-cols-[max-content_auto] gap-x-4 gap-y-3; label primary, kbd chips right.
    div()
        .id(SharedString::from(format!("empty-action-{label}")))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .w_full()
        .max_w(px(480.))
        .cursor_pointer()
        .child(
            div()
                .min_w(px(160.))
                .text_sm()
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.primary))
                .hover(|this| this.text_color(rgb(palette.text)))
                .child(label),
        )
        .child(keys)
        .on_click(on_click)
}

pub fn action_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let detail: SharedString = detail.into();
    action_button_enabled(palette, id, label, detail, true, on_click)
}

pub fn action_button_enabled(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let detail: SharedString = detail.into();
    div()
        .id(SharedString::from(id.into()))
        .min_h(px(46.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .opacity(if enabled { 1.0 } else { 0.45 })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)).border_color(rgb(0x3b82f6)))
        })
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight(800.))
                .text_color(if enabled {
                    rgb(palette.text)
                } else {
                    rgb(palette.text_dimmed)
                })
                .child(label),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(palette.text_muted))
                .child(detail),
        )
        .when(enabled, |this| this.on_click(on_click))
}

pub fn progress_bar(palette: ThemePalette, ratio: f64) -> impl IntoElement {
    let ratio = ratio.clamp(0., 1.) as f32;
    div()
        .h(px(5.))
        .w_full()
        .overflow_hidden()
        .rounded_sm()
        .bg(rgb(palette.border))
        .child(
            div()
                .h(px(5.))
                .w(gpui::relative(ratio))
                .rounded_sm()
                .bg(if ratio >= 0.9_f32 {
                    rgb(0xfb7185)
                } else if ratio >= 0.7_f32 {
                    rgb(palette.warning)
                } else {
                    rgb(palette.primary)
                }),
        )
}
