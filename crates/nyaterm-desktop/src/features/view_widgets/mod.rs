mod chrome;
pub(in crate::features) use chrome::{
    APP_OVERLAY_PRIORITY, bounded_dialog_width, child_window_header, dialog_action_button,
    full_window_input_layer, full_window_overlay_layer, horizontal_resize_handle_visual, logo_mark,
    modal_dialog_shell, panel_header_with_actions, passive_overlay_layer,
    vertical_resize_handle_visual, window_control_button,
};

mod dialogs;

mod child_window;
pub(in crate::features) use child_window::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_options,
    child_window_root, focus_child_window_shell_if_idle,
    init_key_bindings as init_child_window_key_bindings, modal_scrim_is_drawn,
};

mod inspector_widgets;
pub(in crate::features) use inspector_widgets::{
    empty_workspace_action, tab_action_button, tab_menu_item, tab_menu_item_enabled,
    tab_menu_separator,
};

mod stats;
pub(in crate::features) use stats::stats_progress_bar;
mod rows;
pub(in crate::features) use rows::{CloudSyncHistoryRowLabels, cloud_sync_history_row};

mod icons;
pub(in crate::features) use icons::{
    activity_icon, color_icon, connection_spinner, connection_type_icon, mono_icon,
    nyaterm_app_icon, nyaterm_logo_mark, themed_icon, transfer_entry_icon,
};

mod markdown;
pub(in crate::features) use markdown::markdown_content_view;
