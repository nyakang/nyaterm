use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};

use futures::channel::oneshot;
use nyaterm_core::{
    AiSettings, AppSettingsSummary, CloudSyncSettings, CloudSyncState, CommandHistoryEntry, Group,
    KeywordHighlightConfig, MainWindowState, OtpEntry, ProxyConfig, ProxyGroup, QuickCommand,
    QuickCommandCategory, SavedConnection, SavedCredential, SavedPassword, SshKey,
    TranslationSettings, TunnelConfig, TunnelGroup,
};

use crate::storage::{ConnectionStore, StorageError};

const STORE_QUEUE_CAPACITY: usize = 256;

pub type RequestId = u64;

#[derive(Clone)]
pub struct StoreConfig {
    pub config_dir: PathBuf,
    pub portable_key_path: Option<PathBuf>,
}

impl fmt::Debug for StoreConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreConfig")
            .field("config_dir", &self.config_dir)
            .field(
                "portable_key_path",
                &self.portable_key_path.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreDomain {
    Bootstrap,
    Settings,
    WindowState,
    Connections,
    Commands,
    Notes,
    Security,
    Tunnels,
    CloudSync,
    Sessions,
    Ai,
    Terminal,
    Transfers,
    Shutdown,
    Barrier,
}

#[derive(Clone, PartialEq, Eq)]
pub struct StoreOperationError {
    category: &'static str,
    message: String,
}

impl StoreOperationError {
    pub fn category(&self) -> &'static str {
        self.category
    }

    pub fn user_message(&self) -> &str {
        &self.message
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            category: "unavailable",
            message: message.into(),
        }
    }
}

impl From<StorageError> for StoreOperationError {
    fn from(error: StorageError) -> Self {
        let category = match &error {
            StorageError::CreateDir { .. } => "create_dir",
            StorageError::Open { .. } => "open",
            StorageError::Crypto(_) | StorageError::MissingMasterKey => "crypto",
            StorageError::InvalidData(_) | StorageError::PortableSnapshotEntity { .. } => {
                "invalid_data"
            }
            _ => "storage",
        };
        Self {
            category,
            message: error.to_string(),
        }
    }
}

impl fmt::Debug for StoreOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreOperationError")
            .field("category", &self.category)
            .field("message", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for StoreOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StoreOperationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreSubmitError {
    QueueFull,
    Disconnected,
    ShuttingDown,
}

impl fmt::Display for StoreSubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => formatter.write_str("the storage request queue is full"),
            Self::Disconnected => formatter.write_str("the storage worker is unavailable"),
            Self::ShuttingDown => formatter.write_str("the storage runtime is shutting down"),
        }
    }
}

impl std::error::Error for StoreSubmitError {}

pub trait StoreRequest: Send + 'static {
    type Response: Send + 'static;

    fn domain(&self) -> StoreDomain;

    fn execute(self, store: &ConnectionStore) -> Result<Self::Response, StorageError>;
}

pub struct StoreFnRequest<F, T> {
    domain: StoreDomain,
    operation: F,
    response: PhantomData<fn() -> T>,
}

pub fn store_request<F, T>(domain: StoreDomain, operation: F) -> StoreFnRequest<F, T>
where
    F: FnOnce(&ConnectionStore) -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    StoreFnRequest {
        domain,
        operation,
        response: PhantomData,
    }
}

impl<F, T> StoreRequest for StoreFnRequest<F, T>
where
    F: FnOnce(&ConnectionStore) -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    type Response = T;

    fn domain(&self) -> StoreDomain {
        self.domain
    }

    fn execute(self, store: &ConnectionStore) -> Result<Self::Response, StorageError> {
        (self.operation)(store)
    }
}

pub struct StoreEvent<T> {
    pub request_id: RequestId,
    pub domain: StoreDomain,
    pub generation: u64,
    pub outcome: Result<T, StoreOperationError>,
}

impl<T> fmt::Debug for StoreEvent<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreEvent")
            .field("request_id", &self.request_id)
            .field("domain", &self.domain)
            .field("generation", &self.generation)
            .field("outcome", &self.outcome.as_ref().map(|_| "<value>"))
            .finish()
    }
}

pub struct StoreTask<T> {
    request_id: RequestId,
    receiver: oneshot::Receiver<StoreEvent<T>>,
}

impl<T> StoreTask<T> {
    pub fn request_id(&self) -> RequestId {
        self.request_id
    }
}

impl<T> Future for StoreTask<T> {
    type Output = StoreEvent<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Ready(Ok(event)) => Poll::Ready(event),
            Poll::Ready(Err(_)) => Poll::Ready(StoreEvent {
                request_id: self.request_id,
                domain: StoreDomain::Barrier,
                generation: 0,
                outcome: Err(StoreOperationError::unavailable(
                    "the storage worker stopped before returning a result",
                )),
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

type WorkerJob = Box<
    dyn FnOnce(Result<&ConnectionStore, &StoreOperationError>) -> Option<StoreOperationError>
        + Send,
>;

enum WorkerMessage {
    Execute { domain: StoreDomain, job: WorkerJob },
}

#[derive(Clone)]
pub struct StoreUiClient {
    sender: mpsc::SyncSender<WorkerMessage>,
    next_request_id: Arc<AtomicU64>,
    accepting: Arc<AtomicBool>,
    submission_gate: Arc<Mutex<()>>,
}

impl StoreUiClient {
    pub fn try_submit<R: StoreRequest>(
        &self,
        generation: u64,
        request: R,
    ) -> Result<StoreTask<R::Response>, StoreSubmitError> {
        self.try_submit_inner(generation, request, false)
    }

    pub fn try_submit_shutdown<R: StoreRequest>(
        &self,
        generation: u64,
        request: R,
    ) -> Result<StoreTask<R::Response>, StoreSubmitError> {
        self.try_submit_inner(generation, request, true)
    }

    fn try_submit_inner<R: StoreRequest>(
        &self,
        generation: u64,
        request: R,
        allow_shutdown: bool,
    ) -> Result<StoreTask<R::Response>, StoreSubmitError> {
        let _gate = self
            .submission_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !allow_shutdown && !self.accepting.load(Ordering::Acquire) {
            return Err(StoreSubmitError::ShuttingDown);
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let domain = request.domain();
        let (sender, receiver) = oneshot::channel();
        let job = Box::new(
            move |store: Result<&ConnectionStore, &StoreOperationError>| {
                let outcome = match store {
                    Ok(store) => request.execute(store).map_err(StoreOperationError::from),
                    Err(error) => Err(error.clone()),
                };
                let failure = outcome.as_ref().err().cloned();
                let _ = sender.send(StoreEvent {
                    request_id,
                    domain,
                    generation,
                    outcome,
                });
                failure
            },
        );
        match self.sender.try_send(WorkerMessage::Execute { domain, job }) {
            Ok(()) => Ok(StoreTask {
                request_id,
                receiver,
            }),
            Err(mpsc::TrySendError::Full(_)) => Err(StoreSubmitError::QueueFull),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(StoreSubmitError::Disconnected),
        }
    }
}

#[derive(Clone)]
pub struct StoreBlockingClient {
    sender: mpsc::SyncSender<WorkerMessage>,
    next_request_id: Arc<AtomicU64>,
    accepting: Arc<AtomicBool>,
    submission_gate: Arc<Mutex<()>>,
}

impl StoreBlockingClient {
    pub fn request_fn<F, T>(&self, domain: StoreDomain, operation: F) -> Result<T, StoreClientError>
    where
        F: FnOnce(&ConnectionStore) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        self.request(0, store_request(domain, operation))
            .map_err(StoreClientError::Submit)?
            .outcome
            .map_err(StoreClientError::Operation)
    }

    pub fn request<R: StoreRequest>(
        &self,
        generation: u64,
        request: R,
    ) -> Result<StoreEvent<R::Response>, StoreSubmitError> {
        self.request_inner(generation, request, false)
    }

    pub fn request_shutdown<R: StoreRequest>(
        &self,
        generation: u64,
        request: R,
    ) -> Result<StoreEvent<R::Response>, StoreSubmitError> {
        self.request_inner(generation, request, true)
    }

    fn request_inner<R: StoreRequest>(
        &self,
        generation: u64,
        request: R,
        allow_shutdown: bool,
    ) -> Result<StoreEvent<R::Response>, StoreSubmitError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let domain = request.domain();
        let (sender, receiver) = mpsc::sync_channel(1);
        let job = Box::new(
            move |store: Result<&ConnectionStore, &StoreOperationError>| {
                let outcome = match store {
                    Ok(store) => request.execute(store).map_err(StoreOperationError::from),
                    Err(error) => Err(error.clone()),
                };
                let failure = outcome.as_ref().err().cloned();
                let _ = sender.send(StoreEvent {
                    request_id,
                    domain,
                    generation,
                    outcome,
                });
                failure
            },
        );
        {
            let _gate = self
                .submission_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !allow_shutdown && !self.accepting.load(Ordering::Acquire) {
                return Err(StoreSubmitError::ShuttingDown);
            }
            self.sender
                .send(WorkerMessage::Execute { domain, job })
                .map_err(|_| StoreSubmitError::Disconnected)?;
        }
        receiver.recv().map_err(|_| StoreSubmitError::Disconnected)
    }
}

pub enum StoreClientError {
    Submit(StoreSubmitError),
    Operation(StoreOperationError),
}

impl fmt::Debug for StoreClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreClientError")
            .field("category", &self.category())
            .field("message", &"<redacted>")
            .finish()
    }
}

impl StoreClientError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Submit(StoreSubmitError::QueueFull) => "queue_full",
            Self::Submit(StoreSubmitError::Disconnected) => "unavailable",
            Self::Submit(StoreSubmitError::ShuttingDown) => "shutting_down",
            Self::Operation(error) => error.category(),
        }
    }
}

impl fmt::Display for StoreClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Submit(error) => error.fmt(formatter),
            Self::Operation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StoreClientError {}

pub struct StoreRuntime {
    ui_client: StoreUiClient,
    blocking_client: StoreBlockingClient,
}

impl StoreRuntime {
    pub fn spawn(config: StoreConfig) -> Result<Self, std::io::Error> {
        let (sender, receiver) = mpsc::sync_channel(STORE_QUEUE_CAPACITY);
        let next_request_id = Arc::new(AtomicU64::new(1));
        let accepting = Arc::new(AtomicBool::new(true));
        let submission_gate = Arc::new(Mutex::new(()));
        std::thread::Builder::new()
            .name("nyaterm-store".to_string())
            .spawn(move || store_worker(config, receiver))?;
        Ok(Self {
            ui_client: StoreUiClient {
                sender: sender.clone(),
                next_request_id: next_request_id.clone(),
                accepting: accepting.clone(),
                submission_gate: submission_gate.clone(),
            },
            blocking_client: StoreBlockingClient {
                sender: sender.clone(),
                next_request_id,
                accepting,
                submission_gate,
            },
        })
    }

    pub fn ui_client(&self) -> StoreUiClient {
        self.ui_client.clone()
    }

    pub fn blocking_client(&self) -> StoreBlockingClient {
        self.blocking_client.clone()
    }

    pub fn begin_shutdown(&self) {
        let _gate = self
            .ui_client
            .submission_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ui_client.accepting.store(false, Ordering::Release);
    }

    pub fn resume_after_failed_shutdown(&self) {
        let _gate = self
            .ui_client
            .submission_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ui_client.accepting.store(true, Ordering::Release);
    }
}

impl nyaterm_core::CloudLocalStore for StoreBlockingClient {
    fn encode_sync_snapshot(
        &self,
        snapshot: &nyaterm_core::RawPortableSnapshot,
        master_password: &str,
    ) -> Result<Vec<u8>, nyaterm_core::CloudSyncError> {
        crate::encode_encrypted_raw_portable_snapshot(snapshot, master_password)
            .map_err(nyaterm_core::CloudSyncError::PortableSnapshot)
    }

    fn decode_sync_snapshot(
        &self,
        bytes: &[u8],
        master_password: &str,
    ) -> Result<nyaterm_core::RawPortableSnapshot, nyaterm_core::CloudSyncError> {
        crate::decode_encrypted_raw_portable_snapshot(bytes, master_password)
            .map_err(nyaterm_core::CloudSyncError::PortableSnapshot)
    }

    fn encode_sync_pointer(
        &self,
        pointer: &nyaterm_core::RemoteSyncPointer,
    ) -> Result<Vec<u8>, nyaterm_core::CloudSyncError> {
        crate::portable_codec::encode_sync_pointer(pointer)
            .map_err(nyaterm_core::CloudSyncError::PortableSnapshot)
    }

    fn decode_sync_pointer(
        &self,
        bytes: &[u8],
    ) -> Result<nyaterm_core::RemoteSyncPointer, nyaterm_core::CloudSyncError> {
        crate::portable_codec::decode_sync_pointer(bytes)
            .map_err(nyaterm_core::CloudSyncError::PortableSnapshot)
    }

    fn build_sync_snapshot(
        &self,
        options: &nyaterm_core::LocalCloudSyncOptions,
    ) -> Result<nyaterm_core::RawPortableSnapshot, nyaterm_core::CloudSyncError> {
        let device_id = options.device_id.clone();
        let app_version = options.app_version.clone();
        cloud_store_response(self.request(
            0,
            store_request(StoreDomain::CloudSync, move |store| {
                store.build_raw_portable_snapshot(
                    nyaterm_core::PortableSnapshotKind::Sync,
                    device_id,
                    app_version,
                )
            }),
        ))
    }

    fn apply_sync_snapshot(
        &self,
        options: &nyaterm_core::LocalCloudSyncOptions,
        snapshot: &nyaterm_core::RawPortableSnapshot,
    ) -> Result<nyaterm_core::CloudSyncBackupInfo, nyaterm_core::CloudSyncError> {
        let config_dir = options.config_dir.clone();
        let snapshot = snapshot.clone();
        let database_path = config_dir.join("nyaterm.redb");
        let safety_backup_path = cloud_store_response(self.request(
            0,
            store_request(StoreDomain::CloudSync, move |store| {
                store.apply_cloud_sync_snapshot(&config_dir, &snapshot)
            }),
        ))?;
        Ok(nyaterm_core::CloudSyncBackupInfo {
            database_path,
            safety_backup_path,
        })
    }

    fn persist_cloud_sync_state(
        &self,
        state: &nyaterm_core::CloudSyncState,
    ) -> Result<(), nyaterm_core::CloudSyncError> {
        let state = state.clone();
        cloud_store_response(self.request(
            0,
            store_request(StoreDomain::CloudSync, move |store| {
                store.save_cloud_sync_state(&state)
            }),
        ))
    }
}

fn cloud_store_response<T>(
    response: Result<StoreEvent<T>, StoreSubmitError>,
) -> Result<T, nyaterm_core::CloudSyncError> {
    let event =
        response.map_err(|error| nyaterm_core::CloudSyncError::LocalStore(error.to_string()))?;
    event.outcome.map_err(|error| {
        nyaterm_core::CloudSyncError::LocalStore(format!(
            "{}: {}",
            error.category(),
            error.user_message()
        ))
    })
}

fn store_worker(config: StoreConfig, receiver: mpsc::Receiver<WorkerMessage>) {
    let store =
        ConnectionStore::open_with_portable_key_path(config.config_dir, config.portable_key_path)
            .map_err(StoreOperationError::from);
    let mut barrier_failures = Vec::new();
    while let Ok(message) = receiver.recv() {
        match message {
            WorkerMessage::Execute {
                domain: StoreDomain::Barrier,
                job,
            } => {
                let aggregate = aggregate_barrier_failures(&barrier_failures);
                if let Some(error) = aggregate.as_ref() {
                    job(Err(error));
                } else {
                    job(store.as_ref());
                }
            }
            WorkerMessage::Execute { domain, job } => {
                if let Some(error) = job(store.as_ref()) {
                    barrier_failures.push((domain, error));
                } else {
                    clear_resolved_barrier_failures(&mut barrier_failures, domain);
                }
            }
        }
    }
}

fn clear_resolved_barrier_failures(
    failures: &mut Vec<(StoreDomain, StoreOperationError)>,
    successful_domain: StoreDomain,
) {
    failures.retain(|(failed_domain, _)| {
        if *failed_domain == successful_domain {
            return false;
        }
        if successful_domain == StoreDomain::Shutdown {
            return !matches!(
                failed_domain,
                StoreDomain::Settings
                    | StoreDomain::Ai
                    | StoreDomain::Sessions
                    | StoreDomain::Terminal
            );
        }
        true
    });
}

fn aggregate_barrier_failures(
    failures: &[(StoreDomain, StoreOperationError)],
) -> Option<StoreOperationError> {
    if failures.is_empty() {
        return None;
    }
    let mut categories = failures
        .iter()
        .map(|(domain, error)| format!("{domain:?}:{}", error.category()))
        .collect::<Vec<_>>();
    categories.sort();
    categories.dedup();
    Some(StoreOperationError {
        category: "barrier",
        message: format!(
            "{} storage request(s) failed before the flush barrier ({})",
            failures.len(),
            categories.join(", ")
        ),
    })
}

pub struct BootstrapSnapshot {
    pub database_path: PathBuf,
    pub connections: Vec<SavedConnection>,
    pub custom_icons: Vec<nyaterm_core::models::sessions::ConnectionCustomIcon>,
    pub connection_groups: Vec<Group>,
    pub ssh_keys: Vec<SshKey>,
    pub otp_entries: Vec<OtpEntry>,
    pub saved_passwords: Vec<SavedPassword>,
    pub saved_credentials: Vec<SavedCredential>,
    pub tunnels: Vec<TunnelConfig>,
    pub tunnel_groups: Vec<TunnelGroup>,
    pub proxies: Vec<ProxyConfig>,
    pub proxy_groups: Vec<ProxyGroup>,
    pub quick_commands: Vec<QuickCommand>,
    pub quick_command_categories: Vec<QuickCommandCategory>,
    pub command_history: Vec<CommandHistoryEntry>,
    pub keyword_highlights: KeywordHighlightConfig,
    pub settings: AppSettingsSummary,
    pub cloud_sync_settings: CloudSyncSettings,
    pub cloud_sync_state: CloudSyncState,
    pub translation_settings: TranslationSettings,
    pub ai_settings: AiSettings,
    pub ai_session_count: usize,
    pub ai_message_count: usize,
    pub ai_audit_count: usize,
    pub open_tabs: Vec<nyaterm_core::RestorableOpenTab>,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadMainWindowState;

impl StoreRequest for LoadMainWindowState {
    type Response = Option<MainWindowState>;

    fn domain(&self) -> StoreDomain {
        StoreDomain::WindowState
    }

    fn execute(self, store: &ConnectionStore) -> Result<Self::Response, StorageError> {
        store.load_main_window_state()
    }
}

#[derive(Debug, Clone)]
pub struct SaveMainWindowState(pub MainWindowState);

impl StoreRequest for SaveMainWindowState {
    type Response = ();

    fn domain(&self) -> StoreDomain {
        StoreDomain::WindowState
    }

    fn execute(self, store: &ConnectionStore) -> Result<Self::Response, StorageError> {
        store.save_main_window_state(&self.0)
    }
}

pub struct LoadBootstrap;

impl StoreRequest for LoadBootstrap {
    type Response = BootstrapSnapshot;

    fn domain(&self) -> StoreDomain {
        StoreDomain::Bootstrap
    }

    fn execute(self, store: &ConnectionStore) -> Result<Self::Response, StorageError> {
        let sessions = store.load_sessions()?;
        let quick_commands = store.load_quick_commands()?;
        let ai_history = store.load_ai_history()?;
        Ok(BootstrapSnapshot {
            database_path: store.db_path().to_path_buf(),
            connections: sessions.connections,
            custom_icons: sessions.custom_icons,
            connection_groups: sessions.groups,
            ssh_keys: store.list_ssh_keys()?,
            otp_entries: store.list_otp_entries()?,
            saved_passwords: store.list_passwords()?,
            saved_credentials: store.list_credentials()?,
            tunnels: store.list_tunnels()?,
            tunnel_groups: store.list_tunnel_groups()?,
            proxies: store.list_proxies()?,
            proxy_groups: store.list_proxy_groups()?,
            quick_commands: quick_commands.commands,
            quick_command_categories: quick_commands.categories,
            command_history: store.list_command_history(64)?,
            keyword_highlights: store.load_keyword_highlights()?,
            settings: store.load_app_settings_summary()?,
            cloud_sync_settings: store.load_cloud_sync_settings()?,
            cloud_sync_state: store.load_cloud_sync_state()?,
            translation_settings: store.load_translation_settings()?,
            ai_settings: store.load_ai_settings()?,
            ai_session_count: ai_history.sessions.len(),
            ai_message_count: ai_history.messages.len(),
            ai_audit_count: store.list_ai_audit_logs(None)?.len(),
            open_tabs: store.load_open_tabs()?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlushBarrier;

impl StoreRequest for FlushBarrier {
    type Response = ();

    fn domain(&self) -> StoreDomain {
        StoreDomain::Barrier
    }

    fn execute(self, _store: &ConnectionStore) -> Result<Self::Response, StorageError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        FlushBarrier, LoadBootstrap, LoadMainWindowState, SaveMainWindowState, StoreClientError,
        StoreConfig, StoreDomain, StoreOperationError, StoreRuntime,
    };
    use nyaterm_core::{MainWindowBounds, MainWindowState};

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nyaterm-store-runtime-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    use std::path::PathBuf;

    #[test]
    fn bootstrap_and_barrier_run_on_the_store_worker() {
        let config_dir = temp_dir("bootstrap");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let client = runtime.blocking_client();
        let bootstrap = client
            .request(0, LoadBootstrap)
            .expect("receive bootstrap")
            .outcome
            .expect("load bootstrap");
        assert!(bootstrap.connections.is_empty());
        client
            .request(0, FlushBarrier)
            .expect("receive barrier")
            .outcome
            .expect("flush barrier");
        drop(runtime);
        std::fs::remove_dir_all(config_dir).ok();
    }

    #[test]
    fn request_ids_are_monotonic_across_clients() {
        let config_dir = temp_dir("request-ids");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let first = runtime
            .blocking_client()
            .request(0, FlushBarrier)
            .expect("first request");
        let second = runtime
            .blocking_client()
            .request(0, FlushBarrier)
            .expect("second request");
        assert!(second.request_id > first.request_id);
        drop(runtime);
        std::fs::remove_dir_all(config_dir).ok();
    }

    #[test]
    fn blocking_client_error_debug_redacts_operation_details() {
        let error = StoreClientError::Operation(StoreOperationError {
            category: "storage",
            message: "secret-token-value".to_string(),
        });

        let debug = format!("{error:?}");
        assert!(debug.contains("storage"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-token-value"));
        assert_eq!(error.category(), "storage");
    }

    #[test]
    fn request_fn_returns_typed_operation_failures() {
        let config_dir = temp_dir("request-fn");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn runtime");

        let error = runtime
            .blocking_client()
            .request_fn(
                StoreDomain::Settings,
                |_| -> Result<(), crate::StorageError> {
                    Err(crate::StorageError::InvalidData(
                        "invalid settings".to_string(),
                    ))
                },
            )
            .expect_err("operation should fail");
        assert_eq!(error.category(), "invalid_data");

        drop(runtime);
        std::fs::remove_dir_all(config_dir).ok();
    }

    #[test]
    fn flush_barrier_retains_failures_until_the_domain_succeeds() {
        let config_dir = temp_dir("barrier-failure");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let client = runtime.blocking_client();

        let failed = client
            .request_fn(
                StoreDomain::Settings,
                |_| -> Result<(), crate::StorageError> {
                    Err(crate::StorageError::InvalidData(
                        "secret-bearing invalid payload".to_string(),
                    ))
                },
            )
            .expect_err("operation should fail");
        assert_eq!(failed.category(), "invalid_data");

        let barrier_error = client
            .request(0, FlushBarrier)
            .expect("receive failed barrier")
            .outcome
            .expect_err("barrier should aggregate the prior failure");
        assert_eq!(barrier_error.category(), "barrier");
        assert!(
            barrier_error
                .user_message()
                .contains("Settings:invalid_data")
        );
        assert!(
            !barrier_error
                .user_message()
                .contains("secret-bearing invalid payload")
        );

        let retry_error = client
            .request(0, FlushBarrier)
            .expect("receive retry barrier")
            .outcome
            .expect_err("an empty retry must not discard the prior failure");
        assert_eq!(retry_error.category(), "barrier");

        client
            .request_fn(StoreDomain::Settings, |_| Ok(()))
            .expect("retry the failed settings domain");
        client
            .request(0, FlushBarrier)
            .expect("receive resolved barrier")
            .outcome
            .expect("a successful domain retry should resolve its failure");

        drop(runtime);
        std::fs::remove_dir_all(config_dir).ok();
    }

    #[test]
    fn shutdown_rejects_normal_clients_but_accepts_the_final_barrier() {
        let config_dir = temp_dir("shutdown-rejection");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let ui = runtime.ui_client();
        let blocking = runtime.blocking_client();

        runtime.begin_shutdown();

        assert!(matches!(
            ui.try_submit(0, FlushBarrier),
            Err(super::StoreSubmitError::ShuttingDown)
        ));
        assert!(matches!(
            blocking.request(0, FlushBarrier),
            Err(super::StoreSubmitError::ShuttingDown)
        ));
        futures::executor::block_on(
            ui.try_submit_shutdown(0, FlushBarrier)
                .expect("final barrier"),
        )
        .outcome
        .expect("final barrier should run");

        runtime.resume_after_failed_shutdown();
        futures::executor::block_on(ui.try_submit(0, FlushBarrier).expect("resumed barrier"))
            .outcome
            .expect("normal submissions should resume after a failed shutdown");

        drop(runtime);
        std::fs::remove_dir_all(config_dir).ok();
    }

    #[test]
    fn shutdown_window_state_write_completes_before_the_barrier() {
        let config_dir = temp_dir("window-state-shutdown");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let ui = runtime.ui_client();
        let state = MainWindowState::new(
            None,
            MainWindowBounds {
                x: 40,
                y: 60,
                width: 1280,
                height: 800,
            },
            true,
        );

        runtime.begin_shutdown();
        drop(
            ui.try_submit_shutdown(1, SaveMainWindowState(state.clone()))
                .expect("submit state"),
        );
        futures::executor::block_on(
            ui.try_submit_shutdown(2, FlushBarrier)
                .expect("submit barrier"),
        )
        .outcome
        .expect("barrier after state");
        runtime.resume_after_failed_shutdown();
        let loaded = futures::executor::block_on(
            ui.try_submit(3, LoadMainWindowState)
                .expect("submit state load"),
        )
        .outcome
        .expect("load state");

        assert_eq!(loaded, Some(state));
        drop(runtime);
        std::fs::remove_dir_all(config_dir).ok();
    }
}
