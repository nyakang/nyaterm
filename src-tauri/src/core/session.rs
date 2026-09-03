//! Session manager holding active sessions and command history.
//!
//! Tracks SSH/local sessions, routes commands, coordinates command submission
//! confirmation, and persists history for fuzzy search.

use super::history::{CommandHistoryStore, sanitize_history_command};
use super::{InputOrigin, InputSensitivity, RecordingManager};
use crate::config::{AiExecutionProfile, SshProfile};
use crate::core::capabilities::RecentOutputStore;
use crate::core::capture::CapturedOutput;
use crate::core::zmodem::{ZmodemPreparedUpload, ZmodemUploadConflictMode};
use crate::error::{AppError, AppResult};
use crate::observability::{StructuredLog, StructuredLogLevel};
use crate::utils::fuzzy::{FuzzyResult, fuzzy_search_items};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;
use tauri::Emitter;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

const HISTORY_SAVE_DEBOUNCE: Duration = Duration::from_millis(100);
const HISTORY_EVENT_DEBOUNCE: Duration = Duration::from_millis(500);
const MAX_PENDING_CONFIRMATIONS: usize = 256;
const COMMAND_QUEUE_HIGH_THRESHOLD: usize = 100;
const COMMAND_QUEUE_CRITICAL_THRESHOLD: usize = 1000;
const COMMAND_QUEUE_HIGH_RESET_THRESHOLD: usize = 50;
const COMMAND_QUEUE_CRITICAL_RESET_THRESHOLD: usize = 500;

/// Safe, backend-resolved cwd values used by the dynamic-title UI.
///
/// This is intentionally separate from the legacy cwd projection: existing
/// `cwd-changed-*` consumers keep their released string semantics, while new
/// presentation consumers receive a validated, bounded representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CwdPresentation {
    /// Compact path/URI already formatted for a tab title.
    pub title: String,
    /// Full safe value shown in the tab tooltip.
    pub display_path: String,
    /// Exact shell-oriented path for ordinary locations, otherwise an encoded URI.
    pub copy_value: String,
    /// Host-native filesystem path for operational consumers. `None` means
    /// the location is presentation-only (unsafe bytes or an unmappable shell path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operational_path: Option<String>,
    /// True when `copy_value` is an encoded URI rather than a native path.
    pub copy_as_uri: bool,
}

/// Single in-memory cwd record with compatibility, operational and strict
/// presentation projections. All three are replaced under one lock.
#[derive(Debug, Default, Clone)]
pub struct SessionCwdState {
    pub legacy_path: Option<String>,
    pub operational_path: Option<String>,
    pub presentation: Option<CwdPresentation>,
}

impl SessionCwdState {
    pub fn safe_local_execution_cwd(&self) -> Option<String> {
        self.operational_path.clone()
    }
}

#[derive(Debug)]
pub(crate) struct SessionCwdReplacement {
    pub legacy_path: Option<String>,
    pub operational_path: Option<String>,
    pub presentation: Option<CwdPresentation>,
}

#[derive(Debug)]
pub(crate) struct SessionCwdChanges {
    pub operational_changed: bool,
    pub operational_path: Option<String>,
    pub presentation_changed: bool,
    pub presentation: Option<CwdPresentation>,
}

pub type SharedCwd = Arc<Mutex<SessionCwdState>>;
pub type SessionReadyHook = Arc<dyn Fn(&SessionInfo) + Send + Sync>;

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
    let operational_changed = cached.operational_path.as_deref() != Some(normalized.as_str());
    cached.legacy_path = Some(normalized.clone());
    cached.operational_path = Some(normalized.clone());
    if operational_changed {
        Some(normalized)
    } else {
        None
    }
}

pub(crate) async fn replace_cwd_state(
    cwd: &SharedCwd,
    replacement: SessionCwdReplacement,
) -> SessionCwdChanges {
    let mut cached = cwd.lock().await;
    let operational_changed = cached.operational_path != replacement.operational_path;
    let presentation_changed = cached.presentation != replacement.presentation;
    cached.legacy_path = replacement.legacy_path;
    cached.operational_path = replacement.operational_path.clone();
    cached.presentation = replacement.presentation.clone();
    SessionCwdChanges {
        operational_changed,
        operational_path: replacement.operational_path,
        presentation_changed,
        presentation: replacement.presentation,
    }
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

/// Runtime-only dynamic-title policy and integration capabilities.
///
/// This is flattened into [`SessionInfo`] so the established frontend wire
/// fields remain top-level and backward compatible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DynamicTitleCapabilities {
    /// Whether application-provided dynamic titles may be promoted to the tab.
    /// This is copied from the saved connection at session creation time.
    #[serde(default, rename = "dynamic_title_enabled")]
    pub enabled: bool,
    /// Whether the selected shell received NyaTerm's dynamic-title/cwd hooks.
    /// Passive OSC title handling may still work when this is false.
    #[serde(default, rename = "dynamic_title_integration_active")]
    pub integration_active: bool,
    /// Trusted Windows Local executable identity used for initial ConPTY filtering.
    #[serde(
        default,
        rename = "trusted_initial_title",
        skip_serializing_if = "Option::is_none"
    )]
    pub trusted_initial_title: Option<String>,
}

impl DynamicTitleCapabilities {
    pub fn new(enabled: bool, trusted_initial_title: Option<String>) -> Self {
        Self {
            enabled,
            integration_active: false,
            trusted_initial_title,
        }
    }
}

/// Metadata for a session exposed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub session_type: SessionType,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub connected: bool,
    #[serde(default)]
    pub owner_window_label: Option<String>,
    /// Effective AI command execution profile for this session.
    #[serde(default)]
    pub ai_execution_profile: AiExecutionProfile,
    /// True when the released backend shell integration expects private
    /// command-confirmation events. Dynamic-title cwd hooks are independent.
    #[serde(default)]
    pub injection_active: bool,
    #[serde(flatten)]
    pub dynamic_title_capabilities: DynamicTitleCapabilities,
    /// True when the remote file browser is enabled for this session.
    #[serde(default = "default_remote_file_browser_enabled")]
    pub remote_file_browser_enabled: bool,
    /// True when Linux-style remote system statistics are enabled for this session.
    #[serde(default = "default_remote_stats_enabled")]
    pub remote_stats_enabled: bool,
    /// SSH runtime profile used for capability gating on the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_profile: Option<SshProfile>,
}

fn default_remote_file_browser_enabled() -> bool {
    true
}

fn default_remote_stats_enabled() -> bool {
    true
}

pub(crate) fn now_session_started_at() -> String {
    time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc().to_string())
}

/// Commands sent from the frontend to a session's I/O loop.
pub enum SessionCommand {
    /// Attach and acknowledge only after the backend has flushed its buffered
    /// output into the frontend event queue. Used by hibernation replay so
    /// dynamic title publication resumes after the attach boundary.
    AttachConfirmed { ack: oneshot::Sender<()> },
    /// Frontend renderer has been hibernated; keep the session alive but stop emitting output.
    DetachRenderer,
    /// Input to send to the terminal.
    Write {
        data: Vec<u8>,
        automated: bool,
        origin: InputOrigin,
        sensitivity: InputSensitivity,
    },
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
    ZmodemAcceptUpload {
        files: Vec<std::path::PathBuf>,
        conflict_mode: ZmodemUploadConflictMode,
        preserve_timestamps: bool,
    },
    /// ZMODEM: user cancelled the ZMODEM transfer.
    ZmodemCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCommandQueueSnapshot {
    pub queued_commands: u64,
    pub processed_commands: u64,
    pub current_pending: usize,
    pub max_pending_observed: usize,
}

struct SessionCommandQueueMetrics {
    session_id: String,
    queued_commands: AtomicU64,
    processed_commands: AtomicU64,
    current_pending: AtomicUsize,
    max_pending_observed: AtomicUsize,
    pressure_tier: AtomicU8,
}

impl SessionCommandQueueMetrics {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            queued_commands: AtomicU64::new(0),
            processed_commands: AtomicU64::new(0),
            current_pending: AtomicUsize::new(0),
            max_pending_observed: AtomicUsize::new(0),
            pressure_tier: AtomicU8::new(0),
        }
    }

    fn reserve_enqueue(&self) -> usize {
        self.queued_commands
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .unwrap_or(u64::MAX);
        let previous = self
            .current_pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .unwrap_or(usize::MAX);
        previous.saturating_add(1)
    }

    fn commit_enqueue(&self, pending: usize) {
        let mut observed = self.max_pending_observed.load(Ordering::Relaxed);
        while pending > observed {
            match self.max_pending_observed.compare_exchange_weak(
                observed,
                pending,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
        self.maybe_log_pressure(pending);
    }

    fn rollback_enqueue(&self) {
        saturating_atomic_sub_u64(&self.queued_commands, 1);
        let pending = saturating_atomic_sub_usize(&self.current_pending, 1);
        self.relax_pressure_tier(pending);
    }

    fn mark_processed(&self) {
        self.processed_commands
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .ok();
        let pending = saturating_atomic_sub_usize(&self.current_pending, 1);
        self.relax_pressure_tier(pending);
    }

    fn clear_pending(&self) {
        self.current_pending.store(0, Ordering::Relaxed);
        self.pressure_tier.store(0, Ordering::Relaxed);
    }

    fn snapshot(&self) -> SessionCommandQueueSnapshot {
        SessionCommandQueueSnapshot {
            queued_commands: self.queued_commands.load(Ordering::Relaxed),
            processed_commands: self.processed_commands.load(Ordering::Relaxed),
            current_pending: self.current_pending.load(Ordering::Relaxed),
            max_pending_observed: self.max_pending_observed.load(Ordering::Relaxed),
        }
    }

    fn maybe_log_pressure(&self, pending: usize) {
        let next_tier = if pending >= COMMAND_QUEUE_CRITICAL_THRESHOLD {
            2
        } else if pending >= COMMAND_QUEUE_HIGH_THRESHOLD {
            1
        } else {
            return;
        };
        let previous_tier = self.pressure_tier.fetch_max(next_tier, Ordering::Relaxed);
        if previous_tier >= next_tier {
            return;
        }

        let (event, threshold) = if next_tier == 2 {
            (
                "command_queue_pressure.critical",
                COMMAND_QUEUE_CRITICAL_THRESHOLD,
            )
        } else {
            ("command_queue_pressure.high", COMMAND_QUEUE_HIGH_THRESHOLD)
        };
        let snapshot = self.snapshot();
        crate::observability::log_rate_limited(StructuredLog {
            level: StructuredLogLevel::Warn,
            domain: "session.command".to_string(),
            event: event.to_string(),
            message: "Session command queue pressure exceeded a diagnostic threshold".to_string(),
            ids: Some(json!({ "session_id": self.session_id })),
            data: Some(json!({
                "threshold": threshold,
                "queued_commands": snapshot.queued_commands,
                "processed_commands": snapshot.processed_commands,
                "current_pending": snapshot.current_pending,
                "max_pending_observed": snapshot.max_pending_observed,
            })),
            error: None,
            client_timestamp: None,
        });
    }

    fn relax_pressure_tier(&self, pending: usize) {
        let tier = self.pressure_tier.load(Ordering::Relaxed);
        if pending < COMMAND_QUEUE_HIGH_RESET_THRESHOLD {
            self.pressure_tier.store(0, Ordering::Relaxed);
        } else if tier >= 2 && pending < COMMAND_QUEUE_CRITICAL_RESET_THRESHOLD {
            self.pressure_tier.store(1, Ordering::Relaxed);
        }
    }
}

fn saturating_atomic_sub_usize(value: &AtomicUsize, amount: usize) -> usize {
    value
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(amount))
        })
        .map(|previous| previous.saturating_sub(amount))
        .unwrap_or(0)
}

fn saturating_atomic_sub_u64(value: &AtomicU64, amount: u64) -> u64 {
    value
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(amount))
        })
        .map(|previous| previous.saturating_sub(amount))
        .unwrap_or(0)
}

pub struct SessionCommandSender {
    sender: mpsc::UnboundedSender<SessionCommand>,
    metrics: Arc<SessionCommandQueueMetrics>,
}

impl Clone for SessionCommandSender {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

impl SessionCommandSender {
    pub fn send(
        &self,
        command: SessionCommand,
    ) -> Result<(), mpsc::error::SendError<SessionCommand>> {
        let pending = self.metrics.reserve_enqueue();
        match self.sender.send(command) {
            Ok(()) => {
                self.metrics.commit_enqueue(pending);
                Ok(())
            }
            Err(error) => {
                self.metrics.rollback_enqueue();
                Err(error)
            }
        }
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> SessionCommandQueueSnapshot {
        self.metrics.snapshot()
    }
}

pub struct SessionCommandReceiver {
    receiver: mpsc::UnboundedReceiver<SessionCommand>,
    metrics: Arc<SessionCommandQueueMetrics>,
}

impl SessionCommandReceiver {
    pub async fn recv(&mut self) -> Option<SessionCommand> {
        let command = self.receiver.recv().await;
        if command.is_some() {
            self.metrics.mark_processed();
        }
        command
    }

    pub fn try_recv(&mut self) -> Result<SessionCommand, mpsc::error::TryRecvError> {
        let command = self.receiver.try_recv();
        if command.is_ok() {
            self.metrics.mark_processed();
        }
        command
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    pub fn blocking_recv(&mut self) -> Option<SessionCommand> {
        let command = self.receiver.blocking_recv();
        if command.is_some() {
            self.metrics.mark_processed();
        }
        command
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> SessionCommandQueueSnapshot {
        self.metrics.snapshot()
    }
}

impl Drop for SessionCommandReceiver {
    fn drop(&mut self) {
        self.metrics.clear_pending();
    }
}

pub fn session_command_channel(
    session_id: impl Into<String>,
) -> (SessionCommandSender, SessionCommandReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let metrics = Arc::new(SessionCommandQueueMetrics::new(session_id.into()));
    (
        SessionCommandSender {
            sender,
            metrics: metrics.clone(),
        },
        SessionCommandReceiver { receiver, metrics },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupInputBarrierPhase {
    Pending,
    Injected,
    Cancelled,
}

#[derive(Debug)]
struct StartupInputBarrierState {
    phase: StartupInputBarrierPhase,
    pending_terminal_queries: usize,
    pending_terminal_writes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartupInjectionAttempt<T> {
    Injected(T),
    WaitForTerminalResponse,
    Cancelled,
}

/// Orders Local startup injection against real input before it is enqueued.
/// The mutex is held across the short PTY source-command write, so either the
/// input cancellation wins first or the complete injection write wins first.
pub struct StartupInputBarrier {
    state: StdMutex<StartupInputBarrierState>,
}

impl StartupInputBarrier {
    pub fn new() -> Self {
        Self {
            state: StdMutex::new(StartupInputBarrierState {
                phase: StartupInputBarrierPhase::Pending,
                pending_terminal_queries: 0,
                pending_terminal_writes: 0,
            }),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, StartupInputBarrierState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn cancel_pending(&self) -> bool {
        let mut state = self.lock_state();
        if state.phase != StartupInputBarrierPhase::Pending {
            return false;
        }
        state.phase = StartupInputBarrierPhase::Cancelled;
        true
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock_state().phase == StartupInputBarrierPhase::Cancelled
    }

    pub fn cancel_for_input(&self, origin: InputOrigin) -> bool {
        if origin == InputOrigin::TerminalResponse {
            return false;
        }
        let mut state = self.lock_state();
        if state.phase == StartupInputBarrierPhase::Pending {
            state.phase = StartupInputBarrierPhase::Cancelled;
        }
        state.phase == StartupInputBarrierPhase::Cancelled
    }

    pub fn note_terminal_queries(&self, count: usize) {
        if count == 0 {
            return;
        }
        let mut state = self.lock_state();
        if state.phase == StartupInputBarrierPhase::Pending {
            state.pending_terminal_queries = state.pending_terminal_queries.saturating_add(count);
        }
    }

    pub fn note_terminal_response_enqueued(&self) {
        let mut state = self.lock_state();
        if state.phase == StartupInputBarrierPhase::Pending
            && state.pending_terminal_writes < state.pending_terminal_queries
        {
            state.pending_terminal_writes = state.pending_terminal_writes.saturating_add(1);
        }
    }

    pub fn resolve_terminal_response(&self) -> bool {
        let mut state = self.lock_state();
        let matched_query = state.pending_terminal_queries > 0;
        if matched_query {
            state.pending_terminal_queries -= 1;
            state.pending_terminal_writes = state.pending_terminal_writes.saturating_sub(1);
        }
        matched_query
    }

    pub fn has_pending_terminal_query(&self) -> bool {
        let state = self.lock_state();
        state.phase == StartupInputBarrierPhase::Pending
            && (state.pending_terminal_queries > 0 || state.pending_terminal_writes > 0)
    }

    pub fn try_inject<T, E>(
        &self,
        inject: impl FnOnce() -> Result<T, E>,
    ) -> Result<StartupInjectionAttempt<T>, E> {
        let mut state = self.lock_state();
        match state.phase {
            StartupInputBarrierPhase::Cancelled | StartupInputBarrierPhase::Injected => {
                return Ok(StartupInjectionAttempt::Cancelled);
            }
            StartupInputBarrierPhase::Pending
                if state.pending_terminal_queries > 0 || state.pending_terminal_writes > 0 =>
            {
                return Ok(StartupInjectionAttempt::WaitForTerminalResponse);
            }
            StartupInputBarrierPhase::Pending => {}
        }
        match inject() {
            Ok(value) => {
                state.phase = StartupInputBarrierPhase::Injected;
                Ok(StartupInjectionAttempt::Injected(value))
            }
            Err(error) => {
                state.phase = StartupInputBarrierPhase::Cancelled;
                Err(error)
            }
        }
    }
}

/// Handle to an active session; used to send commands and access SSH config for SFTP.
pub struct SessionHandle {
    pub info: SessionInfo,
    pub cmd_tx: SessionCommandSender,
    /// Local Bash/Zsh only: producer-side input/injection ordering barrier.
    pub startup_input_barrier: Option<Arc<StartupInputBarrier>>,
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
    confirmation_overflow_degraded: bool,
}

/// Central registry of sessions, history, and fuzzy search store.
pub struct SessionManager {
    pub sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    pub history_store: Arc<Mutex<CommandHistoryStore>>,
    command_submissions: Arc<Mutex<HashMap<String, CommandSubmissionState>>>,
    pending_zmodem_uploads: Arc<Mutex<HashMap<String, ZmodemPreparedUpload>>>,
    history_save_notify: Arc<Notify>,
    history_save_worker_started: AtomicBool,
    history_event_notify: Arc<Notify>,
    history_event_worker_started: AtomicBool,
    pending_creations: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    app_handle: OnceLock<tauri::AppHandle>,
    recording_manager: OnceLock<Arc<RecordingManager>>,
    recent_output: Arc<RecentOutputStore>,
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
            recording_manager: OnceLock::new(),
            recent_output: Arc::new(RecentOutputStore::default()),
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

    pub fn set_recording_manager(&self, recording_manager: Arc<RecordingManager>) {
        let _ = self.recording_manager.set(recording_manager);
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
            self.recent_output.remove(id);
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
    ) -> Option<ZmodemPreparedUpload> {
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
            if let SessionCommand::Write {
                origin,
                sensitivity,
                ..
            } = &cmd
            {
                let _ = *sensitivity;
                if let Some(barrier) = handle.startup_input_barrier.as_ref() {
                    if *origin == InputOrigin::TerminalResponse {
                        barrier.note_terminal_response_enqueued();
                    } else {
                        barrier.cancel_for_input(*origin);
                    }
                }
            }
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

    /// Returns the best-effort working directory currently tracked for a session.
    pub async fn session_cwd(&self, id: &str) -> AppResult<Option<String>> {
        let cwd = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(id)
                .map(|handle| handle.cwd.clone())
                .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", id)))?
        };
        Ok(cwd.lock().await.legacy_path.clone())
    }

    pub fn append_recent_output(&self, session_id: &str, text: &str) {
        self.recent_output.append(session_id, text);
    }

    pub fn recent_output(&self, session_id: &str, lines: usize) -> String {
        self.recent_output.read(session_id, lines)
    }

    /// Returns the validated dynamic-title cwd presentation for a session.
    pub async fn session_cwd_presentation(&self, id: &str) -> Option<CwdPresentation> {
        let cwd = self.sessions.lock().await.get(id)?.cwd.clone();
        cwd.lock().await.presentation.clone()
    }

    /// Updates a Local session's runtime integration capability after its
    /// session-bound ready marker is observed.
    pub async fn set_dynamic_title_integration_active(&self, id: &str, active: bool) {
        let changed = {
            let mut sessions = self.sessions.lock().await;
            let Some(session) = sessions.get_mut(id) else {
                return;
            };
            if session.info.dynamic_title_capabilities.integration_active == active {
                false
            } else {
                session.info.dynamic_title_capabilities.integration_active = active;
                true
            }
        };

        if changed {
            if let Some(app) = self.app_handle.get() {
                let _ = app.emit("sessions-changed", ());
                crate::tray::schedule_refresh(app);
            }
        }
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

        let overflow_flush = {
            let mut submissions = self.command_submissions.lock().await;
            if let Some(state) = submissions.get_mut(session_id) {
                if state.awaits_shell_event {
                    state.pending_candidates.push_back(command.clone());
                    if state.pending_candidates.len() > MAX_PENDING_CONFIRMATIONS {
                        state.awaits_shell_event = false;
                        state.confirmation_overflow_degraded = true;
                        state.last_shell_event = None;
                        Some(state.pending_candidates.drain(..).collect::<Vec<_>>())
                    } else {
                        None
                    }
                } else {
                    Some(vec![command.clone()])
                }
            } else {
                Some(vec![command.clone()])
            }
        };

        if let Some(commands) = overflow_flush {
            if commands.len() > 1 {
                log_confirmation_overflow(session_id, commands.len(), 0);
            }
            for command in commands {
                self.record_command_transcript(session_id, &command);
                self.add_history_entry(command).await;
            }
        }
    }

    /// Registers a candidate only when this session awaits a shell marker.
    /// Used for synchronized peer input so non-shell peers retain their
    /// released no-registration behavior.
    pub async fn register_confirmation_candidate(&self, session_id: &str, command: String) -> bool {
        let Some(command) = sanitize_history_command(&command) else {
            return false;
        };
        let overflow_flush = {
            let mut submissions = self.command_submissions.lock().await;
            let Some(state) = submissions.get_mut(session_id) else {
                return false;
            };
            if !state.awaits_shell_event || state.confirmation_overflow_degraded {
                return false;
            }
            state.pending_candidates.push_back(command);
            if state.pending_candidates.len() > MAX_PENDING_CONFIRMATIONS {
                state.awaits_shell_event = false;
                state.confirmation_overflow_degraded = true;
                state.last_shell_event = None;
                Some(state.pending_candidates.drain(..).collect::<Vec<_>>())
            } else {
                None
            }
        };

        if let Some(commands) = overflow_flush {
            log_confirmation_overflow(session_id, commands.len(), 0);
            for command in commands {
                self.record_command_transcript(session_id, &command);
                self.add_history_entry(command).await;
            }
            return false;
        }
        true
    }

    /// Records a shell-confirmed command emitted by the backend integration
    /// channel. A marker is only authoritative when it exactly matches a
    /// pending client-side candidate; unmatched remote output is ignored.
    /// Returns true only when a pending candidate was consumed.
    pub async fn confirm_command_submission(&self, session_id: &str, command: String) -> bool {
        let Some(command) = sanitize_history_command(&command) else {
            return false;
        };

        let should_add = {
            let mut submissions = self.command_submissions.lock().await;
            let state = submissions.entry(session_id.to_string()).or_default();

            if state.confirmation_overflow_degraded {
                return false;
            }
            let Some(index) = state
                .pending_candidates
                .iter()
                .position(|pending| pending == &command)
            else {
                return false;
            };

            state.pending_candidates.remove(index);
            state.last_shell_event = Some(command.clone());
            command
        };

        self.record_command_transcript(session_id, &should_add);
        self.add_history_entry(should_add).await;
        true
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
            self.record_command_transcript(session_id, &command);
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

    fn record_command_transcript(&self, session_id: &str, command: &str) {
        if let Some(recording_manager) = self.recording_manager.get() {
            recording_manager.record_command_submission(
                session_id,
                command.to_string(),
                InputSensitivity::Normal,
            );
        }
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

fn log_confirmation_overflow(session_id: &str, flushed_count: usize, dropped_count: usize) {
    crate::observability::log_rate_limited(StructuredLog {
        level: StructuredLogLevel::Warn,
        domain: "session.lifecycle".to_string(),
        event: "command_confirmation.pending_overflow".to_string(),
        message:
            "Shell command confirmation queue overflowed; downgraded to direct history recording"
                .to_string(),
        ids: Some(json!({ "session_id": session_id })),
        data: Some(json!({
            "pending_limit": MAX_PENDING_CONFIRMATIONS,
            "flushed_count": flushed_count,
            "dropped_count": dropped_count,
        })),
        error: None,
        client_timestamp: None,
    });
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
        COMMAND_QUEUE_CRITICAL_RESET_THRESHOLD, COMMAND_QUEUE_CRITICAL_THRESHOLD,
        COMMAND_QUEUE_HIGH_RESET_THRESHOLD, CwdPresentation, DynamicTitleCapabilities, InputOrigin,
        SessionCommand, SessionCommandSender, SessionCwdReplacement, SessionCwdState,
        SessionHandle, SessionInfo, SessionManager, SessionType, StartupInjectionAttempt,
        StartupInputBarrier, normalize_cwd_path, now_session_started_at, replace_cwd_state,
        session_command_channel,
    };
    use std::fs;
    use std::sync::{Arc, atomic::Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Mutex;

    fn test_handle(id: &str, session_type: SessionType, injection_active: bool) -> SessionHandle {
        let (cmd_tx, _cmd_rx) = session_command_channel(id);
        test_handle_with_sender(id, session_type, injection_active, cmd_tx)
    }

    fn test_handle_with_sender(
        id: &str,
        session_type: SessionType,
        injection_active: bool,
        cmd_tx: SessionCommandSender,
    ) -> SessionHandle {
        SessionHandle {
            info: SessionInfo {
                id: id.to_string(),
                name: id.to_string(),
                session_type,
                started_at: now_session_started_at(),
                connection_id: None,
                connected: true,
                owner_window_label: None,
                ai_execution_profile: AiExecutionProfile::Auto,
                injection_active,
                dynamic_title_capabilities: DynamicTitleCapabilities::default(),
                remote_file_browser_enabled: true,
                remote_stats_enabled: true,
                ssh_profile: None,
            },
            cmd_tx,
            startup_input_barrier: None,
            ssh_config: None,
            ssh_handle: None,
            cwd: Arc::new(Mutex::new(Default::default())),
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
    fn dynamic_title_capabilities_keep_the_flat_session_wire_contract() {
        let mut info = test_handle("wire", SessionType::Local, false).info;
        info.dynamic_title_capabilities =
            DynamicTitleCapabilities::new(true, Some("pwsh.exe".to_string()));
        info.dynamic_title_capabilities.integration_active = true;

        let value = serde_json::to_value(&info).expect("serialize SessionInfo");
        assert_eq!(value["dynamic_title_enabled"], true);
        assert_eq!(value["dynamic_title_integration_active"], true);
        assert_eq!(value["trusted_initial_title"], "pwsh.exe");
        assert!(value.get("dynamic_title_capabilities").is_none());

        let decoded: SessionInfo =
            serde_json::from_value(value).expect("deserialize flattened SessionInfo");
        assert_eq!(
            decoded.dynamic_title_capabilities,
            info.dynamic_title_capabilities
        );
    }

    #[test]
    fn startup_input_barrier_orders_cancellation_and_injection() {
        let cancelled = StartupInputBarrier::new();
        assert!(cancelled.cancel_for_input(InputOrigin::Keyboard));
        assert_eq!(
            cancelled.try_inject(|| Ok::<_, ()>(())).unwrap(),
            StartupInjectionAttempt::Cancelled
        );

        let injected = Arc::new(StartupInputBarrier::new());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let injection_barrier = injected.clone();
        let injection = std::thread::spawn(move || {
            injection_barrier.try_inject(|| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok::<_, ()>(())
            })
        });
        started_rx.recv().unwrap();
        let input_barrier = injected.clone();
        let input =
            std::thread::spawn(move || input_barrier.cancel_for_input(InputOrigin::Keyboard));
        release_tx.send(()).unwrap();
        assert_eq!(
            injection.join().unwrap().unwrap(),
            StartupInjectionAttempt::Injected(())
        );
        assert!(!input.join().unwrap());
        assert!(!injected.cancel_pending());

        let query = StartupInputBarrier::new();
        query.note_terminal_queries(1);
        query.note_terminal_response_enqueued();
        assert_eq!(
            query.try_inject(|| Ok::<_, ()>(())).unwrap(),
            StartupInjectionAttempt::WaitForTerminalResponse
        );
        assert!(query.resolve_terminal_response());
        assert_eq!(
            query.try_inject(|| Ok::<_, ()>(())).unwrap(),
            StartupInjectionAttempt::Injected(())
        );
    }

    #[tokio::test]
    async fn manager_marks_real_input_before_enqueuing_it() {
        let manager = SessionManager::new();
        let barrier = Arc::new(StartupInputBarrier::new());
        let (cmd_tx, mut cmd_rx) = session_command_channel("local-startup");
        let mut handle =
            test_handle_with_sender("local-startup", SessionType::Local, false, cmd_tx);
        handle.startup_input_barrier = Some(barrier.clone());
        manager.add_session(handle).await;

        manager
            .send_command(
                "local-startup",
                SessionCommand::Write {
                    data: b"x".to_vec(),
                    automated: false,
                    origin: InputOrigin::Keyboard,
                    sensitivity: super::InputSensitivity::Normal,
                },
            )
            .await
            .unwrap();

        assert!(barrier.is_cancelled());
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(SessionCommand::Write { .. })
        ));
    }

    #[tokio::test]
    async fn cwd_replacement_updates_operational_and_presentation_atomically() {
        let cwd = Arc::new(Mutex::new(SessionCwdState {
            legacy_path: Some("/legacy-a".to_string()),
            operational_path: Some("C:\\safe-a".to_string()),
            presentation: Some(CwdPresentation {
                title: "safe-a".to_string(),
                display_path: "C:\\safe-a".to_string(),
                copy_value: "C:\\safe-a".to_string(),
                operational_path: Some("C:\\safe-a".to_string()),
                copy_as_uri: false,
            }),
        }));
        let changes = replace_cwd_state(
            &cwd,
            SessionCwdReplacement {
                legacy_path: Some("/legacy-b%2".to_string()),
                operational_path: Some("/legacy-b%2".to_string()),
                presentation: None,
            },
        )
        .await;
        assert!(changes.operational_changed);
        assert!(changes.presentation_changed);
        let state = cwd.lock().await;
        assert_eq!(state.legacy_path.as_deref(), Some("/legacy-b%2"));
        assert_eq!(state.operational_path.as_deref(), Some("/legacy-b%2"));
        assert!(state.presentation.is_none());
        assert_eq!(
            state.safe_local_execution_cwd().as_deref(),
            Some("/legacy-b%2")
        );
    }

    #[test]
    fn local_execution_cwd_uses_only_host_operational_path() {
        let cwd = SessionCwdState {
            legacy_path: Some("/shell/path".to_string()),
            operational_path: None,
            presentation: Some(CwdPresentation {
                title: "shell".to_string(),
                display_path: "/shell/path".to_string(),
                copy_value: "/shell/path".to_string(),
                operational_path: None,
                copy_as_uri: false,
            }),
        };
        assert!(cwd.safe_local_execution_cwd().is_none());
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
    async fn unmatched_shell_marker_cannot_inject_history() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-untrusted", SessionType::SSH, true))
            .await;

        assert!(
            !manager
                .confirm_command_submission("ssh-untrusted", "forged command".to_string())
                .await
        );
        assert!(manager.get_all_history().await.is_empty());
    }

    #[tokio::test]
    async fn synchronized_confirmation_candidate_only_queues_shell_sessions() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-peer", SessionType::SSH, true))
            .await;
        manager
            .add_session(test_handle("local-peer", SessionType::Local, false))
            .await;

        assert!(
            manager
                .register_confirmation_candidate("ssh-peer", "uptime".to_string())
                .await
        );
        assert!(
            !manager
                .register_confirmation_candidate("local-peer", "uptime".to_string())
                .await
        );
        assert!(manager.get_all_history().await.is_empty());

        assert!(
            manager
                .confirm_command_submission("ssh-peer", "uptime".to_string())
                .await
        );
        assert_eq!(manager.get_all_history().await, vec!["uptime".to_string()]);
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
        let (cmd_tx, mut cmd_rx) = session_command_channel("local-flow");
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

    #[test]
    fn command_queue_metrics_track_enqueue_process_and_max_pending() {
        let (cmd_tx, mut cmd_rx) = session_command_channel("queue-pressure");
        for _ in 0..1001 {
            cmd_tx
                .send(SessionCommand::AckOutput { bytes: 1 })
                .expect("enqueue command");
        }

        assert_eq!(
            cmd_tx.snapshot(),
            super::SessionCommandQueueSnapshot {
                queued_commands: 1001,
                processed_commands: 0,
                current_pending: 1001,
                max_pending_observed: 1001,
            }
        );

        for _ in 0..1001 {
            assert!(cmd_rx.try_recv().is_ok());
        }
        assert_eq!(
            cmd_rx.snapshot(),
            super::SessionCommandQueueSnapshot {
                queued_commands: 1001,
                processed_commands: 1001,
                current_pending: 0,
                max_pending_observed: 1001,
            }
        );
    }

    #[test]
    fn command_queue_metrics_roll_back_failed_send_and_clear_on_receiver_drop() {
        let (closed_tx, closed_rx) = session_command_channel("closed-queue");
        drop(closed_rx);
        assert!(closed_tx.send(SessionCommand::Close).is_err());
        assert_eq!(closed_tx.snapshot().queued_commands, 0);
        assert_eq!(closed_tx.snapshot().current_pending, 0);

        let (cmd_tx, cmd_rx) = session_command_channel("abandoned-queue");
        cmd_tx
            .send(SessionCommand::AckOutput { bytes: 1 })
            .expect("enqueue command");
        cmd_tx
            .send(SessionCommand::AckOutput { bytes: 2 })
            .expect("enqueue command");
        drop(cmd_rx);
        assert_eq!(cmd_tx.snapshot().current_pending, 0);
        assert_eq!(cmd_tx.snapshot().max_pending_observed, 2);
    }

    #[test]
    fn command_queue_pressure_tiers_rearm_with_hysteresis() {
        let (cmd_tx, mut cmd_rx) = session_command_channel("queue-hysteresis");
        for _ in 0..COMMAND_QUEUE_CRITICAL_THRESHOLD {
            cmd_tx
                .send(SessionCommand::AckOutput { bytes: 1 })
                .expect("enqueue command");
        }
        assert_eq!(cmd_tx.metrics.pressure_tier.load(Ordering::Relaxed), 2);

        for _ in 0..=(COMMAND_QUEUE_CRITICAL_THRESHOLD - COMMAND_QUEUE_CRITICAL_RESET_THRESHOLD) {
            cmd_rx.try_recv().expect("dequeue command");
        }
        assert_eq!(cmd_tx.metrics.pressure_tier.load(Ordering::Relaxed), 1);

        while cmd_tx.snapshot().current_pending >= COMMAND_QUEUE_HIGH_RESET_THRESHOLD {
            cmd_rx.try_recv().expect("dequeue command");
        }
        assert_eq!(cmd_tx.metrics.pressure_tier.load(Ordering::Relaxed), 0);
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
    async fn pending_confirmation_queue_stays_pending_at_limit() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-limit", SessionType::SSH, true))
            .await;

        for index in 0..super::MAX_PENDING_CONFIRMATIONS {
            manager
                .register_command_submission("ssh-limit", format!("echo {index}"))
                .await;
        }

        assert!(
            manager.get_all_history().await.is_empty(),
            "commands at the pending limit should still await shell confirmation"
        );
        let results = manager.fuzzy_search("echo 255", 8, None, None).await;
        assert!(
            results.iter().any(|result| result.command == "echo 255"),
            "pending commands should remain searchable before overflow"
        );
    }

    #[tokio::test]
    async fn pending_confirmation_overflow_degrades_without_duplicate_confirmation() {
        let manager = SessionManager::new();
        manager
            .add_session(test_handle("ssh-overflow", SessionType::SSH, true))
            .await;

        for index in 0..=super::MAX_PENDING_CONFIRMATIONS {
            manager
                .register_command_submission("ssh-overflow", format!("echo {index}"))
                .await;
        }

        let history = manager.get_all_history().await;
        assert_eq!(history.len(), super::MAX_PENDING_CONFIRMATIONS + 1);
        assert_eq!(history.first().map(String::as_str), Some("echo 256"));

        manager
            .confirm_command_submission("ssh-overflow", "echo 0".to_string())
            .await;
        let history_after_confirmation = manager.get_all_history().await;
        assert_eq!(
            history_after_confirmation, history,
            "late confirmations after degradation should not update history"
        );

        manager
            .register_command_submission("ssh-overflow", "echo after".to_string())
            .await;
        let final_history = manager.get_all_history().await;
        assert_eq!(
            final_history.first().map(String::as_str),
            Some("echo after")
        );
        assert_eq!(final_history.len(), super::MAX_PENDING_CONFIRMATIONS + 2);
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
