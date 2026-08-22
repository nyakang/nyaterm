//! Shell chrome, navigation, layout runtime and the GPUI event pump.

mod activity_bar_runtime;
mod appearance;
mod cursor_blink;
mod event_pump;
mod global_shortcut_runtime;
mod keybinding_runtime;
mod navigation_runtime;
mod panel_resize_runtime;
mod panel_stack_runtime;
mod persistence_debounce;
mod quick_switch_runtime;
mod runtime_state;
mod state;
mod tab_mouse;
mod tab_windows_runtime;
mod workspace_runtime;

pub(in crate::features) use activity_bar_runtime::{
    ActivityBarDragPayload, ActivityBarDragPreview,
};
pub(in crate::features) use appearance::{
    ResolvedAppearanceFont, appearance_font_stack, gpui_code_font_family,
};
#[cfg(test)]
pub(in crate::features) use state::ResizeHandleHoverState;
pub(in crate::features) use state::{ShellFeatureInit, ShellFeatureState};
pub(in crate::features) use tab_mouse::{
    SessionTabDragPayload, SessionTabDragPreview, SessionTabTooltip, TAB_MOUSE_ACTIONS,
    TabMouseActionTarget,
};
