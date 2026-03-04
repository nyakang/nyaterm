//! Session manager holding active sessions and command history.
//!
//! Tracks SSH/local sessions, routes commands, and persists history for fuzzy search.

use crate::error::{AppError, AppResult};
use crate::fuzzy::{CommandHistoryStore, FuzzyResult};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Distinguishes SSH vs local PTY sessions for UI and routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionType {
    SSH,
    Local,
}

/// Metadata for a session exposed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub session_type: SessionType,
    pub connected: bool,
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
}

/// Central registry of sessions, history, and fuzzy search store.
pub struct SessionManager {
    pub sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    pub command_history: Arc<Mutex<HashMap<String, Vec<String>>>>,
    pub history_store: Arc<Mutex<CommandHistoryStore>>,
}

impl SessionManager {
    /// Creates an empty manager; history store is initialized in setup.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            command_history: Arc::new(Mutex::new(HashMap::new())),
            history_store: Arc::new(Mutex::new(CommandHistoryStore::new())),
        }
    }

    /// Loads history from config_dir/history.json for fuzzy search.
    pub async fn init_history_store(&self, config_dir: PathBuf) {
        let mut store = self.history_store.lock().await;
        store.set_history_path(config_dir.join("history.json"));
        if let Err(e) = store.load() {
            tracing::warn!("Failed to load command history: {}", e);
        }
    }

    /// Registers a new session and allocates empty command history.
    pub async fn add_session(&self, handle: SessionHandle) {
        let id = handle.info.id.clone();
        self.sessions.lock().await.insert(id.clone(), handle);
        self.command_history.lock().await.insert(id, Vec::new());
    }

    /// Removes session and its history; returns true if the session existed.
    pub async fn remove_session(&self, id: &str) -> bool {
        self.command_history.lock().await.remove(id);
        self.sessions.lock().await.remove(id).is_some()
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

    /// Appends a command to per-session history and persists to the fuzzy store.
    pub async fn add_command(&self, session_id: &str, command: String) {
        {
            let mut history = self.command_history.lock().await;
            if let Some(cmds) = history.get_mut(session_id) {
                cmds.push(command.clone());
            }
        }
        {
            let mut store = self.history_store.lock().await;
            store.add(command);
            if let Err(e) = store.save() {
                tracing::warn!("Failed to save command history: {}", e);
            }
        }
    }

    /// Returns all commands from all sessions (for history UI).
    pub async fn get_all_history(&self) -> Vec<String> {
        let history = self.command_history.lock().await;
        let mut all: Vec<String> = Vec::new();
        for cmds in history.values() {
            all.extend(cmds.iter().cloned());
        }
        all
    }

    /// Fuzzy searches command history; returns top `limit` matches by score.
    pub async fn fuzzy_search(&self, pattern: &str, limit: usize) -> Vec<FuzzyResult> {
        let store = self.history_store.lock().await;
        store.search(pattern, limit)
    }
}
