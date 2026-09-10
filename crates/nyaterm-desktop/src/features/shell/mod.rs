//! Shell chrome, navigation, layout runtime and the GPUI event pump.

mod activity_bar_runtime;
mod appearance;
mod cursor_blink;
mod drop_hover;
mod event_pump;
mod global_shortcut_runtime;
mod idle_lock;
mod keybinding_runtime;
mod navigation_runtime;
mod panel_resize_runtime;
mod panel_stack_runtime;
mod pending_focus;
mod persistence_debounce;
mod post_start_work;
mod quick_switch_runtime;
mod runtime_state;
mod shortcut_action_runtime;
mod state;
mod status_clocks;
mod tab_mouse;
mod tab_windows_runtime;
mod terminal_recovery;
mod tray;
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
    SessionTabDragPayload, SessionTabDragPreview, SessionTabTooltip, TAB_MOUSE_ACTIONS,
    TabMouseActionTarget,
};
