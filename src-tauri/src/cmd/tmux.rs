//! Tauri commands for tmux control-mode (`-CC`) integration.
//!
//! These commands address the *control session* — the SSH session whose
//! channel switched to tmux control mode after `tmux -CC` was detected.
//! Pane-level input/resize/close continue to use the regular
//! `write_to_session` / `resize_session` / `close_session` commands through
//! the virtual session ids reported in `tmux-session-state` events.

use crate::core::{SessionCommand, SessionManager};
use crate::error::AppResult;
use std::sync::Arc;

/// Send a raw tmux command line on the control channel, e.g.
/// `split-window -h`, `new-window`, `select-window -t @2`.
#[tauri::command]
pub async fn tmux_send_command(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::TmuxCommand { line: command })
        .await
}

/// Detach the tmux control client. The remote tmux session keeps running
/// and the channel returns to normal shell I/O.
#[tauri::command]
pub async fn tmux_detach(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::TmuxDetach)
        .await
}
