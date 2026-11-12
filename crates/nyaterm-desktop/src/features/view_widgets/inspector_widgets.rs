use gpui::{App, ClickEvent, IntoElement, Window};

use crate::theme::ThemePalette;

pub(in crate::features) use nyaterm_ui::presentation::{
    action_button as tab_action_button, menu_item as tab_menu_item_enabled_with_icon,
    menu_separator as tab_menu_separator, shortcut_action as empty_workspace_action,
};

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
