pub(in crate::features) const TERMINAL_INPUT_LATENCY_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(80);

mod buffer;
mod clipboard_files;
mod paste;
mod scroll;
pub(in crate::features) use scroll::{
    TERMINAL_USER_SCROLL_ACTIVE_WINDOW, TerminalScrollVisualState, TerminalScrollWheelStateResult,
    terminal_display_offset_from_state, terminal_local_scroll_delta_lines_from_state,
    terminal_scroll_needs_text_first_repaint, terminal_visual_scroll_active_for_state,
};
mod sessions;
mod view_io;
pub(in crate::features) use view_io::TerminalMouseReportRequest;
