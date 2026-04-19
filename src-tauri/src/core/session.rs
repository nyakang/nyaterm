//! Session manager holding active sessions and command history.
//!
//! Tracks SSH/local sessions, routes commands, and persists history for fuzzy search.

use super::history::CommandHistoryStore;
use crate::error::{AppError, AppResult};
use crate::utils::fuzzy::FuzzyResult;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex};

pub type SharedCwd = Arc<Mutex<Option<String>>>;

pub(crate) fn normalize_cwd_path(path: &str) -> String {
    if path.is_empty() || path == "/" || is_windows_drive_root(path) {
        return path.to_string();
    }

    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() {
        "/".to_string()
    } else {
        normalized.to_string()
    }
}

pub(crate) async fn update_cwd_if_changed(cwd: &SharedCwd, next_path: &str) -> Option<String> {
    let normalized = normalize_cwd_path(next_path);
    if normalized.is_empty() {
        return None;
    }

    let mut cached = cwd.lock().await;
    let unchanged = cached
        .as_deref()
        .is_some_and(|current| normalize_cwd_path(current) == normalized);

    if unchanged {
        return None;
    }

    *cached = Some(normalized.clone());
    Some(normalized)
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    matches!(bytes, [drive, b':', b'/'] if drive.is_ascii_alphabetic())
        || matches!(bytes, [b'/', drive, b':', b'/'] if drive.is_ascii_alphabetic())
}

/// Distinguishes session types for UI and routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionType {
    SSH,
    Local,
    Telnet,
    Serial,
}

/// Metadata for a session exposed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub session_type: SessionType,
    pub connected: bool,
    /// True when backend terminal-path tracking is available for this session.
    /// Currently this is enabled for sessions that can report directory changes to the backend.
    #[serde(default)]
    pub injection_active: bool,
}

/// Commands sent from the frontend to a session's I/O loop.
pub enum SessionCommand {
    /// Frontend listener is ready — flush buffered output and start emitting.
    Attach,
    /// User input to send to the terminal.
    Write(Vec<u8>),
    /// Terminal size change (cols × rows).
    Resize { cols: u32, rows: u32 },
    /// Close the session and clean up.
    Close,
}

/// Handle to an active session; used to send commands and access SSH config for SFTP.
pub struct SessionHandle {
    pub info: SessionInfo,
    pub cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    /// SSH-specific: stores config for potential reconnection.
    #[allow(dead_code)]
    pub ssh_config: Option<Arc<dyn Any + Send + Sync>>,
    /// SSH-specific: authenticated `client::Handle` for channel multiplexing (SFTP, exec).
    pub ssh_handle: Option<Arc<dyn Any + Send + Sync>>,
    /// Current working directory cached from directory updates emitted by the session.
    pub cwd: SharedCwd,
}

/// Central registry of sessions, history, and fuzzy search store.
pub struct SessionManager {
    pub sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    pub history_store: Arc<Mutex<CommandHistoryStore>>,
    history_save_scheduled: Arc<AtomicBool>,
    app_handle: OnceLock<tauri::AppHandle>,
}

impl SessionManager {
    /// Creates an empty manager; history store is initialized in setup.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            history_store: Arc::new(Mutex::new(CommandHistoryStore::new())),
            history_save_scheduled: Arc::new(AtomicBool::new(false)),
            app_handle: OnceLock::new(),
        }
    }

    /// Store the app handle so the manager can emit events to the frontend.
    pub fn set_app_handle(&self, app: tauri::AppHandle) {
        let _ = self.app_handle.set(app);
    }

    /// Loads history from config_dir/history.json for fuzzy search.
    pub async fn init_history_store(&self, config_dir: PathBuf) {
        let mut store = self.history_store.lock().await;
        store.set_history_path(config_dir.join("history.json"));
        if let Err(e) = store.load() {
            tracing::warn!("Failed to load command history: {}", e);
        }
    }

    /// Registers a new active session.
    pub async fn add_session(&self, handle: SessionHandle) {
        let id = handle.info.id.clone();
        self.sessions.lock().await.insert(id, handle);
        if let Some(app) = self.app_handle.get() {
            let _ = app.emit("sessions-changed", ());
        }
    }

    /// Removes a session; returns true if the session existed.
    pub async fn remove_session(&self, id: &str) -> bool {
        let removed = self.sessions.lock().await.remove(id).is_some();
        if removed {
            if let Some(app) = self.app_handle.get() {
                let _ = app.emit("sessions-changed", ());
            }
        }
        removed
    }

    /// Sends a command to a session's I/O loop; errors if session not found.
    pub async fn send_command(&self, id: &str, cmd: SessionCommand) -> AppResult<()> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(id) {
            handle
                .cmd_tx
                .send(cmd)
                .map_err(|e| AppError::Channel(e.to_string()))
        } else {
            Err(AppError::SessionNotFound(format!(
                "Session '{}' not found",
                id
            )))
        }
    }

    /// Returns metadata for all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions.values().map(|h| h.info.clone()).collect()
    }

    /// Appends a command to persistent history and schedules a coalesced save.
    pub async fn add_command(&self, _session_id: &str, command: String) {
        let changed = {
            let mut store = self.history_store.lock().await;
            store.add(command)
        };

        if !changed {
            return;
        }

        self.schedule_history_save();
        if let Some(app) = self.app_handle.get() {
            let _ = app.emit("command-history-changed", ());
        }
    }

    fn schedule_history_save(&self) {
        if self.history_save_scheduled.swap(true, Ordering::SeqCst) {
            return;
        }

        let history_store = self.history_store.clone();
        let history_save_scheduled = self.history_save_scheduled.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                let pending = {
                    let mut store = history_store.lock().await;
                    store.prepare_save()
                };

                if let Some((path, bytes)) = pending {
                    let write_result = tokio::task::spawn_blocking(move || {
                        super::history::flush_to_disk(&path, &bytes)
                    })
                    .await;
                    match write_result {
                        Ok(Err(err)) => {
                            tracing::warn!("Failed to save command history: {}", err);
                        }
                        Err(err) => {
                            tracing::warn!("History save task panicked: {}", err);
                        }
                        _ => {}
                    }
                }

                history_save_scheduled.store(false, Ordering::SeqCst);

                let needs_reschedule = {
                    let store = history_store.lock().await;
                    store.is_dirty()
                };
                if needs_reschedule && !history_save_scheduled.swap(true, Ordering::SeqCst) {
                    continue;
                }

                break;
            }
        });
    }

    /// Returns persistent history in stable most-recent-first order.
    pub async fn get_all_history(&self) -> Vec<String> {
        let store = self.history_store.lock().await;
        store.list()
    }

    /// Fuzzy searches command history; returns top `limit` matches by score.
    pub async fn fuzzy_search(&self, pattern: &str, limit: usize) -> Vec<FuzzyResult> {
        let store = self.history_store.lock().await;
        store.search(pattern, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_cwd_path;

    #[test]
    fn normalizes_trailing_slashes_without_breaking_roots() {
        assert_eq!(normalize_cwd_path("/var/log/"), "/var/log");
        assert_eq!(normalize_cwd_path("/"), "/");
        assert_eq!(normalize_cwd_path("C:/"), "C:/");
        assert_eq!(normalize_cwd_path("/C:/"), "/C:/");
    }
}
