//! Session manager holding active sessions and command history.
//!
//! Tracks SSH/local sessions, routes commands, coordinates command submission
//! confirmation, and persists history for fuzzy search.

use super::history::{CommandHistoryStore, sanitize_history_command};
use crate::config::AiExecutionProfile;
use crate::core::capture::CapturedOutput;
use crate::error::{AppError, AppResult};
use crate::utils::fuzzy::{FuzzyResult, fuzzy_search_items};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

const HISTORY_SAVE_DEBOUNCE: Duration = Duration::from_millis(100);
const HISTORY_EVENT_DEBOUNCE: Duration = Duration::from_millis(500);

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
    #[serde(default)]
    pub owner_window_label: Option<String>,
    /// Effective AI command execution profile for this session.
    #[serde(default)]
    pub ai_execution_profile: AiExecutionProfile,
    /// True when backend terminal-path tracking is available for this session.
    /// Currently this is enabled for sessions that can report directory changes to the backend.
    #[serde(default)]
    pub injection_active: bool,
    /// True when the remote file browser is enabled for this session.
    #[serde(default = "default_remote_file_browser_enabled")]
    pub remote_file_browser_enabled: bool,
}

fn default_remote_file_browser_enabled() -> bool {
    true
}

/// Commands sent from the frontend to a session's I/O loop.
pub enum SessionCommand {
    /// Frontend listener is ready — flush buffered output and start emitting.
    Attach,
    /// User input to send to the terminal.
    Write(Vec<u8>),
    /// Temporarily stop reading output from the underlying terminal source.
    PauseOutput,
    /// Resume reading output from the underlying terminal source.
    ResumeOutput,
    /// Renderer has finished consuming this many emitted output bytes.
    AckOutput { bytes: usize },
    /// Terminal size change (cols × rows).
    Resize { cols: u32, rows: u32 },
    /// Close the session and clean up.
    Close,
    /// AI capture: inject a marker-wrapped command into the PTY and capture output.
    CaptureExec {
        marker_id: String,
        wrapped_command: Vec<u8>,
        result_tx: oneshot::Sender<CapturedOutput>,
    },
    /// AI capture: cancel a marker-wrapped command capture that no longer has a caller.
    CancelCapture { marker_id: String },
    /// ZMODEM: user accepted a download — save to this directory.
    ZmodemAcceptDownload { save_dir: std::path::PathBuf },
    /// ZMODEM: user accepted an upload — send these files.
    ZmodemAcceptUpload { files: Vec<std::path::PathBuf> },
    /// ZMODEM: user cancelled the ZMODEM transfer.
    ZmodemCancel,
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
    /// Lazily-initialised remote file system (auto-fallback across SFTP / SCP backends).
    pub remote_fs: Option<Arc<crate::core::sftp::AutoRemoteFs>>,
}

pub struct SessionCreationGuard {
    request_id: String,
    pending_creations: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
}

impl Drop for SessionCreationGuard {
    fn drop(&mut self) {
        let request_id = self.request_id.clone();
        let pending_creations = self.pending_creations.clone();
        tokio::spawn(async move {
            pending_creations.lock().await.remove(&request_id);
        });
    }
}

#[derive(Debug, Default)]
struct CommandSubmissionState {
    pending_candidates: VecDeque<String>,
    last_shell_event: Option<String>,
    awaits_shell_event: bool,
}

/// Central registry of sessions, history, and fuzzy search store.
pub struct SessionManager {
    pub sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    pub history_store: Arc<Mutex<CommandHistoryStore>>,
    command_submissions: Arc<Mutex<HashMap<String, CommandSubmissionState>>>,
    pending_zmodem_uploads: Arc<Mutex<HashMap<String, Vec<std::path::PathBuf>>>>,
    history_save_notify: Arc<Notify>,
    history_save_worker_started: AtomicBool,
    history_event_notify: Arc<Notify>,
    history_event_worker_started: AtomicBool,
    pending_creations: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    app_handle: OnceLock<tauri::AppHandle>,
}

impl SessionManager {
    /// Creates an empty manager; history store is initialized in setup.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            history_store: Arc::new(Mutex::new(CommandHistoryStore::new())),
            command_submissions: Arc::new(Mutex::new(HashMap::new())),
            pending_zmodem_uploads: Arc::new(Mutex::new(HashMap::new())),
            history_save_notify: Arc::new(Notify::new()),
            history_save_worker_started: AtomicBool::new(false),
            history_event_notify: Arc::new(Notify::new()),
            history_event_worker_started: AtomicBool::new(false),
            pending_creations: Arc::new(Mutex::new(HashMap::new())),
            app_handle: OnceLock::new(),
        }
    }

    pub async fn begin_session_creation(
        &self,
        request_id: Option<String>,
    ) -> Option<(SessionCreationGuard, oneshot::Receiver<()>)> {
        let request_id = request_id?;
        let (tx, rx) = oneshot::channel();
        self.pending_creations
            .lock()
            .await
            .insert(request_id.clone(), tx);
        Some((
            SessionCreationGuard {
                request_id,
                pending_creations: self.pending_creations.clone(),
            },
            rx,
        ))
    }

    pub async fn cancel_session_creation(&self, request_id: &str) -> bool {
        self.pending_creations
            .lock()
            .await
            .remove(request_id)
            .is_some_and(|tx| tx.send(()).is_ok())
    }

    /// Store the app handle so the manager can emit events to the frontend.
    pub fn set_app_handle(&self, app: tauri::AppHandle) {
        let _ = self.app_handle.set(app);
    }

    /// Returns the stored app handle, if set.
    pub fn app(&self) -> Option<tauri::AppHandle> {
        self.app_handle.get().cloned()
    }

    /// Loads command history from redb for fuzzy search.
    pub async fn init_history_store(&self) {
        let needs_save = {
            let mut store = self.history_store.lock().await;
            if let Err(e) = store.load() {
                tracing::warn!("Failed to load command history: {}", e);
            }
            store.is_dirty()
        };

        self.ensure_history_save_worker();
        if needs_save {
            self.request_history_save();
        }
    }

    /// Reload command history from redb and notify listeners.
    pub async fn reload_history_from_storage(&self) -> AppResult<()> {
        {
            let mut store = self.history_store.lock().await;
            store.load()?;
        }
        if let Some(app) = self.app_handle.get() {
            let _ = app.emit("command-history-changed", ());
        }
        Ok(())
    }

    /// Registers a new active session.
    pub async fn add_session(&self, handle: SessionHandle) {
        let id = handle.info.id.clone();
        let awaits_shell_event = session_awaits_shell_event(&handle.info);
        self.sessions.lock().await.insert(id.clone(), handle);
        self.command_submissions.lock().await.insert(
            id.clone(),
            CommandSubmissionState {
                awaits_shell_event,
                ..CommandSubmissionState::default()
            },
        );
        if let Some(app) = self.app_handle.get() {
            let _ = app.emit("sessions-changed", ());
            crate::tray::schedule_refresh(app);
        }
    }

    /// Removes a session; returns true if the session existed.
    pub async fn remove_session(&self, id: &str) -> bool {
        let removed = self.sessions.lock().await.remove(id).is_some();
        if removed {
            self.flush_pending_submission(id).await;
            self.command_submissions.lock().await.remove(id);
            self.pending_zmodem_uploads.lock().await.remove(id);
        }
        if removed {
            if let Some(app) = self.app_handle.get() {
                let _ = app.emit("sessions-changed", ());
                crate::tray::schedule_refresh(app);
            }
        }
        removed
    }

    /// Takes and clears any prepared ZMODEM upload paths for a session.
    pub async fn take_pending_zmodem_upload(
        &self,
        session_id: &str,
    ) -> Option<Vec<std::path::PathBuf>> {
        self.pending_zmodem_uploads.lock().await.remove(session_id)
    }

    /// Clears prepared ZMODEM upload paths without starting a transfer.
    pub async fn clear_pending_zmodem_upload(&self, session_id: &str) {
        self.pending_zmodem_uploads.lock().await.remove(session_id);
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

    /// Returns metadata for a single active session.
    pub async fn session_info(&self, id: &str) -> AppResult<SessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(id)
            .map(|handle| handle.info.clone())
            .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", id)))
    }

    /// Appends a command to persistent history and schedules a coalesced save.
    pub async fn add_command(&self, _session_id: &str, command: String) {
        self.add_history_entry(command).await;
    }

    /// Registers a client-side command candidate. Sessions with backend shell
    /// events keep it pending until confirmation; other sessions add directly.
    pub async fn register_command_submission(&self, session_id: &str, command: String) {
        let Some(command) = sanitize_history_command(&command) else {
            return;
        };

        let should_add_immediately = {
            let mut submissions = self.command_submissions.lock().await;
            if let Some(state) = submissions.get_mut(session_id) {
                if state.awaits_shell_event {
                    state.pending_candidates.push_back(command.clone());
                    false
                } else {
                    true
                }
            } else {
                true
            }
        };

        if should_add_immediately {
            self.add_history_entry(command).await;
        }
    }

    /// Records a shell-confirmed command emitted by the backend integration
    /// channel. Duplicate confirmations are ignored once the matching pending
    /// candidate has already been consumed.
    pub async fn confirm_command_submission(&self, session_id: &str, command: String) {
        let Some(command) = sanitize_history_command(&command) else {
            return;
        };

        let should_add = {
            let mut submissions = self.command_submissions.lock().await;
            let state = submissions.entry(session_id.to_string()).or_default();

            if let Some(index) = state
                .pending_candidates
                .iter()
                .position(|pending| pending == &command)
            {
                state.pending_candidates.remove(index);
                state.last_shell_event = Some(command.clone());
                Some(command)
            } else if state.last_shell_event.as_deref() == Some(command.as_str()) {
                None
            } else {
                state.last_shell_event = Some(command.clone());
                Some(command)
            }
        };

        if let Some(command) = should_add {
            self.add_history_entry(command).await;
        }
    }

    /// Flushes any pending client candidate when a shell-capable session ends
    /// without sending a matching backend confirmation.
    pub async fn flush_pending_submission(&self, session_id: &str) {
        let pending_commands = {
            let mut submissions = self.command_submissions.lock().await;
            submissions
                .get_mut(session_id)
                .map(|state| state.pending_candidates.drain(..).collect::<Vec<_>>())
                .unwrap_or_default()
        };

        for command in pending_commands {
            self.add_history_entry(command).await;
        }
    }

    async fn add_history_entry(&self, command: String) {
        let changed = {
            let mut store = self.history_store.lock().await;
            store.add(command)
        };

        if !changed {
            return;
        }

        self.request_history_save();
        self.request_history_event();
    }

    /// Removes a command from persistent history and notifies suggestion listeners.
    pub async fn delete_history_command(&self, command: String) {
        let Some(command) = sanitize_history_command(&command) else {
            return;
        };

        {
            let mut submissions = self.command_submissions.lock().await;
            for state in submissions.values_mut() {
                state
                    .pending_candidates
                    .retain(|pending| pending != &command);
                if state.last_shell_event.as_deref() == Some(command.as_str()) {
                    state.last_shell_event = None;
                }
            }
        }

        let changed = {
            let mut store = self.history_store.lock().await;
            store.delete_command(&command)
        };

        if !changed {
            return;
        }

        self.request_history_save();
        self.request_history_event();
    }

    fn request_history_event(&self) {
        self.ensure_history_event_worker();
        self.history_event_notify.notify_one();
    }

    fn ensure_history_event_worker(&self) {
        if self
            .history_event_worker_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        let notify = self.history_event_notify.clone();
        let app_handle = self.app_handle.clone();

        tauri::async_runtime::spawn(async move {
            loop {
                notify.notified().await;
                tokio::time::sleep(HISTORY_EVENT_DEBOUNCE).await;
                while tokio::time::timeout(HISTORY_EVENT_DEBOUNCE, notify.notified())
                    .await
                    .is_ok()
                {}
                if let Some(app) = app_handle.get() {
                    let _ = app.emit("command-history-changed", ());
                }
            }
        });
    }

    fn ensure_history_save_worker(&self) {
        if self
            .history_save_worker_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        let history_store = self.history_store.clone();
        let history_save_notify = self.history_save_notify.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                history_save_notify.notified().await;

                loop {
                    tokio::time::sleep(HISTORY_SAVE_DEBOUNCE).await;

                    while tokio::time::timeout(
                        HISTORY_SAVE_DEBOUNCE,
                        history_save_notify.notified(),
                    )
                    .await
                    .is_ok()
                    {}

                    save_history_snapshot(&history_store).await;

                    let needs_resave = {
                        let store = history_store.lock().await;
                        store.is_dirty()
                    };
                    if !needs_resave {
                        break;
                    }
                }
            }
        });
    }

    fn request_history_save(&self) {
        self.ensure_history_save_worker();
        self.history_save_notify.notify_one();
    }

    /// Flushes pending shell-submission fallbacks and history state
    /// synchronously during application shutdown.
    pub fn flush_history_before_shutdown(&self) {
        let pending_commands: Vec<String> = {
            let mut submissions = self.command_submissions.blocking_lock();
            submissions
                .values_mut()
                .flat_map(|state| state.pending_candidates.drain(..).collect::<Vec<_>>())
                .collect()
        };

        if !pending_commands.is_empty() {
            let mut store = self.history_store.blocking_lock();
            for command in pending_commands {
                let _ = store.add(command);
            }
        }

        loop {
            let pending = {
                let mut store = self.history_store.blocking_lock();
                store.prepare_save()
            };

            let Some(pending) = pending else {
                break;
            };

            if let Err(err) = super::history::flush_prepared_save(pending) {
                tracing::warn!("Failed to flush command history during shutdown: {}", err);
                break;
            }
        }
    }

    /// Returns persistent history in stable most-recent-first order.
    pub async fn get_all_history(&self) -> Vec<String> {
        let store = self.history_store.lock().await;
        store.list()
    }

    /// Fuzzy searches command history; returns top `limit` matches by score.
    pub async fn fuzzy_search(
        &self,
        pattern: &str,
        limit: usize,
        min_command_length: Option<usize>,
        max_command_length: Option<usize>,
    ) -> Vec<FuzzyResult> {
        let mut results = {
            let store = self.history_store.lock().await;
            store.search(pattern, limit, min_command_length, max_command_length)
        };

        let pending_commands = self.pending_history_candidates().await;
        if pending_commands.is_empty() {
            return results;
        }

        let pending_refs: Vec<(&str, &str)> = pending_commands
            .iter()
            .map(|command| (command.as_str(), command.as_str()))
            .collect();
        let pending_results = fuzzy_search_items(
            &pending_refs,
            pattern,
            "history",
            limit,
            min_command_length,
            max_command_length,
        );
        let mut existing = results
            .iter()
            .map(|result| result.command.clone())
            .collect::<HashSet<_>>();

        for result in pending_results {
            if existing.insert(result.command.clone()) {
                results.push(result);
            }
        }

        results.sort_by(|a, b| b.score.cmp(&a.score).then(a.command.cmp(&b.command)));
        results.truncate(limit);
        results
    }
}

fn session_awaits_shell_event(info: &SessionInfo) -> bool {
    matches!(info.session_type, SessionType::SSH | SessionType::Local) && info.injection_active
}

impl SessionManager {
    async fn pending_history_candidates(&self) -> Vec<String> {
        let submissions = self.command_submissions.lock().await;
        let mut seen = HashSet::new();
        let mut commands = Vec::new();

        for state in submissions.values() {
            for command in &state.pending_candidates {
                if seen.insert(command.clone()) {
                    commands.push(command.clone());
                }
            }
        }

        commands
    }
}

async fn save_history_snapshot(history_store: &Arc<Mutex<CommandHistoryStore>>) {
    let pending = {
        let mut store = history_store.lock().await;
        store.prepare_save()
    };

    let Some(pending) = pending else {
        return;
    };

    let write_result =
        tokio::task::spawn_blocking(move || super::history::flush_prepared_save(pending)).await;
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

#[cfg(test)]
mod tests {
    use crate::config::AiExecutionProfile;

    use super::{
        SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, normalize_cwd_path,
    };
    use std::fs;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::{Mutex, mpsc};

    fn test_handle(id: &str, session_type: SessionType, injection_active: bool) -> SessionHandle {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
        test_handle_with_sender(id, session_type, injection_active, cmd_tx)
    }

    fn test_handle_with_sender(
        id: &str,
        session_type: SessionType,
        injection_active: bool,
        cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    ) -> SessionHandle {
        SessionHandle {
            info: SessionInfo {
                id: id.to_string(),
                name: id.to_string(),
                session_type,
                connected: true,
                owner_window_label: None,
                ai_execution_profile: AiExecutionProfile::Auto,
                injection_active,
                remote_file_browser_enabled: true,
            },
            cmd_tx,
            ssh_config: None,
            ssh_handle: None,
            cwd: Arc::new(Mutex::new(None)),
            remote_fs: None,
        }
    }

    fn unique_history_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("nyaterm-session-history-{name}-{nanos}.json"))
    }

    #[test]
    fn normalizes_trailing_slashes_without_breaking_roots() {
        assert_eq!(normalize_cwd_path("/var/log/"), "/var/log");
        assert_eq!(normalize_cwd_path("/"), "/");
        assert_eq!(normalize_cwd_path("C:/"), "C:/");
        assert_eq!(normalize_cwd_path("/C:/"), "/C:/");
    }

    #[tokio::test]
    async fn shell_event_session_waits_for_confirmation() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-1", SessionType::SSH, true))
            .await;

        manager
            .register_command_submission("ssh-1", "echo hello".to_string())
            .await;
        assert!(manager.get_all_history().await.is_empty());

        manager
            .confirm_command_submission("ssh-1", "echo hello".to_string())
            .await;
        assert_eq!(
            manager.get_all_history().await,
            vec!["echo hello".to_string()]
        );

        manager
            .confirm_command_submission("ssh-1", "echo hello".to_string())
            .await;
        assert_eq!(
            manager.get_all_history().await,
            vec!["echo hello".to_string()]
        );
    }

    #[tokio::test]
    async fn direct_session_adds_submission_immediately() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("telnet-1", SessionType::Telnet, false))
            .await;

        manager
            .register_command_submission("telnet-1", "show version".to_string())
            .await;

        assert_eq!(
            manager.get_all_history().await,
            vec!["show version".to_string()]
        );
    }

    #[tokio::test]
    async fn session_creation_cancel_signal_is_one_shot() {
        let manager = SessionManager::new();
        let (_guard, mut cancel_rx) = manager
            .begin_session_creation(Some("create-1".to_string()))
            .await
            .expect("creation guard");

        assert!(manager.cancel_session_creation("create-1").await);
        assert!(cancel_rx.try_recv().is_ok());
        assert!(!manager.cancel_session_creation("create-1").await);
    }

    #[tokio::test]
    async fn sends_output_pause_and_resume_commands() {
        let manager = SessionManager::new();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
        manager
            .add_session(test_handle_with_sender(
                "local-flow",
                SessionType::Local,
                false,
                cmd_tx,
            ))
            .await;

        manager
            .send_command("local-flow", SessionCommand::PauseOutput)
            .await
            .expect("pause command");
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::PauseOutput)
        ));

        manager
            .send_command("local-flow", SessionCommand::ResumeOutput)
            .await
            .expect("resume command");
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::ResumeOutput)
        ));

        manager
            .send_command("local-flow", SessionCommand::AckOutput { bytes: 4096 })
            .await
            .expect("ack command");
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::AckOutput { bytes: 4096 })
        ));
    }

    #[tokio::test]
    async fn close_flushes_unconfirmed_pending_submission_once() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("local-1", SessionType::Local, true))
            .await;

        manager
            .register_command_submission("local-1", "exit".to_string())
            .await;
        assert!(manager.remove_session("local-1").await);

        assert_eq!(manager.get_all_history().await, vec!["exit".to_string()]);
    }

    #[tokio::test]
    async fn pending_submission_is_searchable_before_shell_confirmation() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-2", SessionType::SSH, true))
            .await;

        manager
            .register_command_submission("ssh-2", "docker images".to_string())
            .await;

        let results = manager.fuzzy_search("docker im", 8, None, None).await;
        assert!(
            results
                .iter()
                .any(|result| result.command == "docker images"),
            "pending submission should be searchable before shell confirmation"
        );
        assert!(
            manager.get_all_history().await.is_empty(),
            "pending submissions should not be committed to history until confirmation"
        );
    }

    #[tokio::test]
    async fn newer_pending_submission_survives_out_of_order_confirmation() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-3", SessionType::SSH, true))
            .await;

        manager
            .register_command_submission("ssh-3", "docker ps".to_string())
            .await;
        manager
            .register_command_submission("ssh-3", "docker images".to_string())
            .await;

        manager
            .confirm_command_submission("ssh-3", "docker ps".to_string())
            .await;

        let results = manager.fuzzy_search("docker im", 8, None, None).await;
        assert!(
            results
                .iter()
                .any(|result| result.command == "docker images"),
            "later pending submission should remain searchable after an earlier confirmation"
        );
        assert_eq!(
            manager.get_all_history().await,
            vec!["docker ps".to_string()]
        );
    }

    #[tokio::test]
    async fn deletes_persistent_history_command() {
        let manager = SessionManager::new();
        manager.add_command("test", "docker ps".to_string()).await;
        manager.add_command("test", "ls".to_string()).await;

        manager
            .delete_history_command("root@ubuntu:~# docker ps".to_string())
            .await;

        assert_eq!(manager.get_all_history().await, vec!["ls".to_string()]);
        assert!(
            !manager
                .fuzzy_search("docker ps", 8, None, None)
                .await
                .iter()
                .any(|result| result.command == "docker ps")
        );
    }

    #[test]
    fn shutdown_flush_persists_pending_history_without_waiting_for_debounce() {
        let history_path = unique_history_path("shutdown-flush");
        let manager = SessionManager::new();

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            {
                let mut store = manager.history_store.lock().await;
                store.set_history_path(history_path.clone());
            }

            manager
                .add_session(test_handle("local-2", SessionType::Local, true))
                .await;
            manager
                .register_command_submission("local-2", "exit".to_string())
                .await;
        });
        drop(runtime);

        manager.flush_history_before_shutdown();

        let content = fs::read_to_string(&history_path).expect("history file");
        assert!(content.contains("\"command\":\"exit\""));

        let _ = fs::remove_file(history_path);
    }
}
