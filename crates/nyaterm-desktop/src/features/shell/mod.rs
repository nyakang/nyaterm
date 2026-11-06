//! Shell chrome, navigation, layout runtime and the GPUI event pump.

mod activity_bar_runtime;
mod appearance;
mod cursor_blink;
mod drop_hover;
mod event_pump;
mod global_shortcut_runtime;
mod idle_lock;
mod keybinding_runtime;
mod main_window;
mod navigation_runtime;
mod panel_resize_runtime;
mod panel_stack_runtime;
mod pending_focus;
mod persistence_debounce;
mod post_start_work;
mod quick_commands_runtime;
mod quick_switch_runtime;
mod runtime_state;
pub(in crate::features) mod session_tabs;
mod shortcut_action_runtime;
mod state;
mod status_clocks;
mod tab_drag;
pub(in crate::features) mod tab_mouse;
pub(in crate::features) mod tab_reorder;
mod tab_windows_runtime;
mod terminal_recovery;
pub(crate) mod tray;
mod window_decorations;
pub(in crate::features) mod window_material;
mod workspace_runtime;

pub(in crate::features) use activity_bar_runtime::{
    ActivityBarDragPayload, ActivityBarDragPreview,
};
pub(in crate::features) use appearance::{
    appearance_font_stack, configured_appearance_font_stack, gpui_code_font_family,
    gpui_terminal_font_fallback, gpui_ui_font_fallback,
};
#[cfg(test)]
pub(in crate::features) use state::ResizeHandleHoverState;
pub(in crate::features) use state::{NewSessionMenuAnchor, ShellFeatureInit, ShellFeatureState};
pub(in crate::features) use tab_mouse::{
    SessionTabDragPreview, SessionTabTooltip, TAB_MOUSE_ACTIONS, TabMouseActionTarget,
    WorkspaceTabDragPayload,
};

mod workspace_groups;

mod window_state;
pub(in crate::features) use window_state::TerminalWindowDockResult;

mod editor_tabs;
