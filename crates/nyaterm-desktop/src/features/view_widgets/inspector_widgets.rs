use gpui::{
    App, ClickEvent, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px, rgb, svg,
};

use crate::theme::ThemePalette;

/// Tauri EmptyWorkspaceState row: action label (primary) + shortcut key chips.
pub(in crate::features) fn empty_workspace_action(
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

pub(in crate::features) fn tab_menu_item(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    tab_menu_item_enabled(palette, id, label, true, on_click)
}

pub(in crate::features) fn tab_menu_item_enabled(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id = id.into();
    let icon_path = match id.as_str() {
        "tab-ctx-rename" => Some("icons/session/rename.svg"),
        "tab-ctx-lock" => Some("icons/lock.svg"),
        "tab-ctx-unlock" => Some("icons/unlock.svg"),
        "tab-ctx-color-reset" => Some("icons/window/close.svg"),
        "tab-ctx-copy-name" | "tab-ctx-copy-ip" | "tab-ctx-copy-ssh" => Some("icons/copy.svg"),
        "tab-ctx-duplicate" => Some("icons/transfer/play.svg"),
        "tab-ctx-duplicate-run" | "tab-ctx-multiplex-run" => Some("icons/menu/input.svg"),
        "tab-ctx-multiplex" | "tab-ctx-smart-split" => Some("icons/menu/call-split.svg"),
        "tab-ctx-reconnect" => Some("icons/session/reconnect.svg"),
        "tab-ctx-disconnect" => Some("icons/session/disconnect.svg"),
        "tab-ctx-rdp-secure-attention" => Some("icons/security.svg"),
        "tab-ctx-split-h" | "tab-ctx-window-below" | "tab-ctx-tile-h" => {
            Some("icons/menu/horizontal.svg")
        }
        "tab-ctx-split-v" | "tab-ctx-window-right" | "tab-ctx-tile-v" => {
            Some("icons/menu/vertical.svg")
        }
        "tab-ctx-unsplit" => Some("icons/menu/merge.svg"),
        "tab-ctx-window-flat" => Some("icons/menu/fit.svg"),
        "tab-ctx-close" => Some("icons/window/close.svg"),
        "tab-ctx-close-all" => Some("icons/menu/close-fullscreen.svg"),
        "tab-ctx-close-others" => Some("icons/menu/circle-dot-filled.svg"),
        "tab-ctx-close-right" => Some("icons/menu/arrow-bar-to-right.svg"),
        "tab-ctx-info" => Some("icons/menu/info.svg"),
        _ => None,
    };
    tab_menu_item_enabled_with_icon(palette, id, label, icon_path, enabled, on_click)
}

pub(in crate::features) fn tab_menu_item_with_icon(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    icon_path: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    tab_menu_item_enabled_with_icon(palette, id, label, Some(icon_path), true, on_click)
}

fn tab_menu_item_enabled_with_icon(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    icon_path: Option<&'static str>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id = id.into();
    let label = label.into();
    let text_color = rgb(palette.text);
    let icon_color = rgb(palette.text_muted);
    div()
        .id(SharedString::from(id))
        .h(px(28.))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_sm()
        .text_size(px(12.))
        .text_color(text_color)
        .opacity(if enabled { 1. } else { 0.5 })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_click(on_click)
        })
        .when_some(icon_path, |this, icon_path| {
            this.child(
                div()
                    .w(px(24.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(svg().size(px(16.)).path(icon_path).text_color(icon_color)),
            )
        })
        .child(div().min_w_0().flex_1().child(label))
}

pub(in crate::features) fn tab_menu_separator(palette: ThemePalette) -> impl IntoElement {
    div().h(px(1.)).flex_none().my_1().bg(rgb(palette.border))
}

pub(in crate::features) fn tab_action_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let detail: SharedString = detail.into();
    tab_action_button_enabled(palette, id, label, detail, true, on_click)
}

pub(in crate::features) fn tab_action_button_enabled(
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
