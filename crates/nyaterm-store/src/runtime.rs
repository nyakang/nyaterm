use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::time::Instant;

mod maintenance;
use self::maintenance::MaintenanceSchedule;

use futures::channel::mpsc as futures_mpsc;
use futures::channel::oneshot;
use nyaterm_core::{
    AiSettings, AppSettingsSummary, CloudSyncSettings, CloudSyncState, CommandHistoryEntry,
    DeviceWindowManifest, Group, KeywordHighlightConfig, MainWindowState, OtpEntry, ProxyConfig,
    ProxyGroup, QuickCommand, QuickCommandCategory, SavedConnection, SavedCredential,
    SavedPassword, SshKey, TranslationSettings, TunnelConfig, TunnelGroup, WorkspaceId,
    WorkspaceRestoreManifest,
};

use crate::maintenance::LocalStorageMaintenanceResult;
use crate::storage::{ConnectionStore, StorageError, WorkspaceStartupRestore};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreMutationEvent {
    pub request_id: RequestId,
    pub domain: StoreDomain,
    pub generation: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct StoreOperationError {
    category: &'static str,
    message: String,
    local_replica_payload_limit: Option<u64>,
}

impl StoreOperationError {
    pub fn category(&self) -> &'static str {
        self.category
    }

    pub fn user_message(&self) -> &str {
        &self.message
    }

    /// Typed, non-secret detail retained across the worker boundary.
    pub fn local_replica_payload_limit(&self) -> Option<u64> {
        self.local_replica_payload_limit
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            category: "unavailable",
            message: message.into(),
            local_replica_payload_limit: None,
        }
    }
}

impl From<StorageError> for StoreOperationError {
    fn from(error: StorageError) -> Self {
        let local_replica_payload_limit = match &error {
            StorageError::PortableSnapshot(
                nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { limit_bytes },
            ) => Some(*limit_bytes),
            _ => None,
        };
        let category = match &error {
            StorageError::PortableSnapshot(
                nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { .. },
            ) => "replica_payload_too_large",
            StorageError::CloudSyncLocalChanged => "sync_local_changed",
            StorageError::CreateDir { .. } => "create_dir",
            StorageError::Open { .. } => "open",
            StorageError::Crypto(_)
            | StorageError::MissingMasterKey
            | StorageError::MissingPortableKey { .. } => "crypto",
            StorageError::InvalidData(_) | StorageError::PortableSnapshotEntity { .. } => {
                "invalid_data"
            }
            _ => "storage",
        };
        Self {
            category,
            message: error.to_string(),
            local_replica_payload_limit,
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

    fn changes_sync_payload(&self) -> bool {
        false
    }

    /// Whether this request updates flush-barrier failures. Opting out means
    /// neither recording a failure nor resolving any earlier domain failure.
    fn records_failures(&self) -> bool {
        true
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError>;
}

pub struct StoreFnRequest<F, T> {
    domain: StoreDomain,
    changes_sync_payload: bool,
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
        changes_sync_payload: false,
        operation,
        response: PhantomData,
    }
}

pub fn store_mutation<F, T>(domain: StoreDomain, operation: F) -> StoreFnRequest<F, T>
where
    F: FnOnce(&ConnectionStore) -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    let mut request = store_request(domain, operation);
    request.changes_sync_payload = true;
    request
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

    fn changes_sync_payload(&self) -> bool {
        self.changes_sync_payload
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        (self.operation)(&*store)
    }
}

struct StoreExclusiveFnRequest<F, T> {
    domain: StoreDomain,
    operation: F,
    response: PhantomData<fn() -> T>,
}

impl<F, T> StoreRequest for StoreExclusiveFnRequest<F, T>
where
    F: FnOnce(&mut ConnectionStore) -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    type Response = T;

    fn domain(&self) -> StoreDomain {
        self.domain
    }

    fn records_failures(&self) -> bool {
        false
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
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

enum BarrierEffect {
    Ignore,
    Success,
    Failure(StoreOperationError),
}

impl BarrierEffect {
    fn from_outcome<T>(records_failures: bool, outcome: &Result<T, StoreOperationError>) -> Self {
        if !records_failures {
            Self::Ignore
        } else if let Err(error) = outcome {
            Self::Failure(error.clone())
        } else {
            Self::Success
        }
    }
}

type WorkerJob =
    Box<dyn FnOnce(Result<&mut ConnectionStore, &StoreOperationError>) -> BarrierEffect + Send>;

struct MaintenanceWaiter {
    request_id: RequestId,
    sender: oneshot::Sender<StoreEvent<LocalStorageMaintenanceResult>>,
}

enum WorkerMessage {
    Execute {
        domain: StoreDomain,
        job: WorkerJob,
    },
    ScheduleMaintenance {
        requested_at: Instant,
        waiter: MaintenanceWaiter,
    },
}

#[derive(Clone)]
pub struct StoreUiClient {
    sender: mpsc::SyncSender<WorkerMessage>,
    next_request_id: Arc<AtomicU64>,
    accepting: Arc<AtomicBool>,
    submission_gate: Arc<Mutex<()>>,
    sync_mutation_generation: Arc<AtomicU64>,
    mutation_sender: futures_mpsc::UnboundedSender<StoreMutationEvent>,
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
        let changes_sync_payload = request.changes_sync_payload();
        let records_failures = request.records_failures();
        let sync_mutation_generation = self.sync_mutation_generation.clone();
        let mutation_sender = self.mutation_sender.clone();
        let (sender, receiver) = oneshot::channel();
        let job = Box::new(
            move |store: Result<&mut ConnectionStore, &StoreOperationError>| {
                let outcome = match store {
                    Ok(store) => request.execute(store).map_err(StoreOperationError::from),
                    Err(error) => Err(error.clone()),
                };
                let failure = BarrierEffect::from_outcome(records_failures, &outcome);
                if changes_sync_payload && outcome.is_ok() {
                    let generation = sync_mutation_generation.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = mutation_sender.unbounded_send(StoreMutationEvent {
                        request_id,
                        domain,
                        generation,
                    });
                }
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
    sync_mutation_generation: Arc<AtomicU64>,
    mutation_sender: futures_mpsc::UnboundedSender<StoreMutationEvent>,
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

    pub fn request_mutation_fn<F, T>(
        &self,
        domain: StoreDomain,
        operation: F,
    ) -> Result<T, StoreClientError>
    where
        F: FnOnce(&ConnectionStore) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        self.request(0, store_mutation(domain, operation))
            .map_err(StoreClientError::Submit)?
            .outcome
            .map_err(StoreClientError::Operation)
    }

    /// Runs a maintenance operation that needs exclusive access to the open
    /// database handle, such as compaction. Unlike the ordinary blocking
    /// requests this hands out `&mut ConnectionStore`, so it must stay off the
    /// UI thread and is never used for ordinary reads or writes.
    pub fn request_exclusive_fn<F, T>(
        &self,
        domain: StoreDomain,
        operation: F,
    ) -> Result<T, StoreClientError>
    where
        F: FnOnce(&mut ConnectionStore) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        self.request(
            0,
            StoreExclusiveFnRequest {
                domain,
                operation,
                response: PhantomData,
            },
        )
        .map_err(StoreClientError::Submit)?
        .outcome
        .map_err(StoreClientError::Operation)
    }

    pub fn sync_mutation_generation(&self) -> u64 {
        self.sync_mutation_generation.load(Ordering::Acquire)
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
        let changes_sync_payload = request.changes_sync_payload();
        let records_failures = request.records_failures();
        let sync_mutation_generation = self.sync_mutation_generation.clone();
        let mutation_sender = self.mutation_sender.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let job = Box::new(
            move |store: Result<&mut ConnectionStore, &StoreOperationError>| {
                let outcome = match store {
                    Ok(store) => request.execute(store).map_err(StoreOperationError::from),
                    Err(error) => Err(error.clone()),
                };
                let failure = BarrierEffect::from_outcome(records_failures, &outcome);
                if changes_sync_payload && outcome.is_ok() {
                    let generation = sync_mutation_generation.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = mutation_sender.unbounded_send(StoreMutationEvent {
                        request_id,
                        domain,
                        generation,
                    });
                }
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

#[derive(Clone)]
pub struct StoreRuntime {
    ui_client: StoreUiClient,
    blocking_client: StoreBlockingClient,
    mutation_receiver: Arc<Mutex<Option<futures_mpsc::UnboundedReceiver<StoreMutationEvent>>>>,
}

impl StoreRuntime {
    pub fn spawn(config: StoreConfig) -> Result<Self, std::io::Error> {
        let (sender, receiver) = mpsc::sync_channel(STORE_QUEUE_CAPACITY);
        let next_request_id = Arc::new(AtomicU64::new(1));
        let accepting = Arc::new(AtomicBool::new(true));
        let submission_gate = Arc::new(Mutex::new(()));
        let sync_mutation_generation = Arc::new(AtomicU64::new(0));
        let (mutation_sender, mutation_receiver) = futures_mpsc::unbounded();
        let worker_accepting = accepting.clone();
        std::thread::Builder::new()
            .name("nyaterm-store".to_string())
            .spawn(move || store_worker(config, receiver, worker_accepting))?;
        Ok(Self {
            ui_client: StoreUiClient {
                sender: sender.clone(),
                next_request_id: next_request_id.clone(),
                accepting: accepting.clone(),
                submission_gate: submission_gate.clone(),
                sync_mutation_generation: sync_mutation_generation.clone(),
                mutation_sender: mutation_sender.clone(),
            },
            blocking_client: StoreBlockingClient {
                sender: sender.clone(),
                next_request_id,
                accepting,
                submission_gate,
                sync_mutation_generation,
                mutation_sender,
            },
            mutation_receiver: Arc::new(Mutex::new(Some(mutation_receiver))),
        })
    }

    pub fn ui_client(&self) -> StoreUiClient {
        self.ui_client.clone()
    }

    pub fn blocking_client(&self) -> StoreBlockingClient {
        self.blocking_client.clone()
    }

    /// Schedule after the first successful bootstrap. Duplicate callers share
    /// the same deferred attempt and result; this never changes flush failures.
    pub fn schedule_local_storage_maintenance(
        &self,
    ) -> Result<StoreTask<LocalStorageMaintenanceResult>, StoreSubmitError> {
        let client = &self.ui_client;
        let _gate = client
            .submission_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !client.accepting.load(Ordering::Acquire) {
            return Err(StoreSubmitError::ShuttingDown);
        }
        let request_id = client.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        match client.sender.try_send(WorkerMessage::ScheduleMaintenance {
            requested_at: Instant::now(),
            waiter: MaintenanceWaiter { request_id, sender },
        }) {
            Ok(()) => Ok(StoreTask {
                request_id,
                receiver,
            }),
            Err(mpsc::TrySendError::Full(_)) => Err(StoreSubmitError::QueueFull),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(StoreSubmitError::Disconnected),
        }
    }

    pub fn sync_mutation_generation(&self) -> u64 {
        self.blocking_client.sync_mutation_generation()
    }

    pub fn take_sync_mutation_events(
        &self,
    ) -> Option<futures_mpsc::UnboundedReceiver<StoreMutationEvent>> {
        self.mutation_receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
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
        #[cfg(test)]
        let decoded = crate::decode_encrypted_raw_portable_snapshot(bytes, master_password);
        #[cfg(not(test))]
        let decoded = if std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .is_some_and(|name| name.starts_with("nyaterm_desktop-"))
        {
            // Desktop unit tests run in Cargo's test harness, which cannot dispatch
            // the application helper argument. The packaged app always uses the helper.
            crate::decode_encrypted_raw_portable_snapshot(bytes, master_password)
        } else {
            crate::portable_decode_helper::decode_remote_snapshot_in_helper(bytes, master_password)
        };
        decoded.map_err(nyaterm_core::CloudSyncError::PortableSnapshot)
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
                store.build_fixed_sync_snapshot(device_id, app_version)
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
        let snapshot_password = options.master_password.clone();
        let database_path = config_dir.join("nyaterm.redb");
        let safety_backup_path = cloud_store_response(self.request(
            0,
            store_mutation(StoreDomain::CloudSync, move |store| {
                let backup = store.apply_cloud_sync_snapshot(
                    &config_dir,
                    &snapshot,
                    snapshot_password.expose_secret(),
                )?;
                LoadBootstrap.load(store)?;
                Ok(backup)
            }),
        ))?;
        Ok(nyaterm_core::CloudSyncBackupInfo {
            database_path,
            safety_backup_path,
        })
    }

    fn apply_sync_snapshot_checked(
        &self,
        options: &nyaterm_core::LocalCloudSyncOptions,
        snapshot: &nyaterm_core::RawPortableSnapshot,
        expected_content_hash: &str,
        mut state: CloudSyncState,
    ) -> Result<(nyaterm_core::CloudSyncBackupInfo, CloudSyncState), nyaterm_core::CloudSyncError>
    {
        let options = options.clone();
        let snapshot = snapshot.clone();
        let expected_content_hash = expected_content_hash.to_string();
        // One store job prevents a save from slipping between the comparison,
        // replacement and normalized baseline. On a persistence failure the old
        // baseline remains conservative and the next sync reports a conflict.
        cloud_store_response(self.request(
            0,
            store_mutation(StoreDomain::CloudSync, move |store| {
                let current =
                    store.build_fixed_sync_snapshot(&options.device_id, &options.app_version)?;
                if current.sync_content_hash()? != expected_content_hash {
                    return Err(StorageError::CloudSyncLocalChanged);
                }
                let backup = store.apply_cloud_sync_snapshot(
                    &options.config_dir,
                    &snapshot,
                    options.master_password.expose_secret(),
                )?;
                let mut applied =
                    store.build_fixed_sync_snapshot(&options.device_id, &options.app_version)?;
                applied.recalculate_hash()?;
                state.last_synced_content_hash = Some(applied.sync_content_hash()?);
                state.last_synced_payload_hash = Some(applied.meta.payload_hash);
                store.save_cloud_sync_state(&state)?;
                LoadBootstrap.load(store)?;
                Ok((
                    nyaterm_core::CloudSyncBackupInfo {
                        database_path: store.db_path().to_path_buf(),
                        safety_backup_path: backup,
                    },
                    state,
                ))
            }),
        ))
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

    fn verify_master_password(
        &self,
        master_password: &str,
    ) -> Result<Option<bool>, nyaterm_core::CloudSyncError> {
        let master_password = master_password.to_string();
        // Decrypting the stored master password does not use the master password
        // itself, so this is a real check of the candidate. A store with no
        // stored password cannot tell, and reports `None`.
        let matches = self
            .request_fn(StoreDomain::Security, move |store| {
                store.master_password_matches(&master_password)
            })
            .map_err(|error| nyaterm_core::CloudSyncError::LocalStore(error.to_string()))?;
        Ok(matches)
    }
}

fn cloud_store_response<T>(
    response: Result<StoreEvent<T>, StoreSubmitError>,
) -> Result<T, nyaterm_core::CloudSyncError> {
    let event =
        response.map_err(|error| nyaterm_core::CloudSyncError::LocalStore(error.to_string()))?;
    event.outcome.map_err(|error| {
        if error.category() == "sync_local_changed" {
            return nyaterm_core::CloudSyncError::LocalChangedDuringSync;
        }
        nyaterm_core::CloudSyncError::LocalStore(format!(
            "{}: {}",
            error.category(),
            error.user_message()
        ))
    })
}

fn store_worker(
    config: StoreConfig,
    receiver: mpsc::Receiver<WorkerMessage>,
    accepting: Arc<AtomicBool>,
) {
    let mut store =
        ConnectionStore::open_with_portable_key_path(config.config_dir, config.portable_key_path)
            .map_err(StoreOperationError::from);
    let mut barrier_failures = Vec::new();
    let mut maintenance = MaintenanceSchedule::new(Instant::now());
    let mut waiters = Vec::new();
    let mut maintenance_result = None;
    loop {
        if !accepting.load(Ordering::Acquire) && maintenance.deadline().is_some() {
            maintenance.finish();
            let outcome = Err(StoreOperationError::unavailable(
                "storage maintenance cancelled during shutdown",
            ));
            complete_maintenance_waiters(&mut waiters, &outcome);
            maintenance_result = Some(outcome);
        }
        let message = if let Some(deadline) = maintenance.deadline() {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(message) => message,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Check admission and queued work again before entering the
                    // non-preemptible compaction. Never acquire the submission
                    // gate here: a full queue may hold it while waiting for us.
                    if !accepting.load(Ordering::Acquire) {
                        continue;
                    }
                    match receiver.try_recv() {
                        Ok(message) => message,
                        Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {
                            maintenance.finish();
                            let outcome = match store.as_mut() {
                                Ok(database) => database
                                    .maintain_local_storage()
                                    .map_err(StoreOperationError::from),
                                Err(error) => Err(error.clone()),
                            };
                            complete_maintenance_waiters(&mut waiters, &outcome);
                            maintenance_result = Some(outcome);
                            continue;
                        }
                    }
                }
            }
        } else {
            let Ok(message) = receiver.recv() else { break };
            message
        };
        let ordinary_request = matches!(message, WorkerMessage::Execute { .. });
        match message {
            WorkerMessage::ScheduleMaintenance {
                requested_at,
                waiter,
            } => {
                if let Some(outcome) = &maintenance_result {
                    send_maintenance_result(waiter, outcome);
                } else if !accepting.load(Ordering::Acquire) {
                    let outcome = Err(StoreOperationError::unavailable(
                        "storage maintenance cancelled during shutdown",
                    ));
                    send_maintenance_result(waiter, &outcome);
                    complete_maintenance_waiters(&mut waiters, &outcome);
                    maintenance.finish();
                    maintenance_result = Some(outcome);
                } else {
                    maintenance.request(requested_at);
                    waiters.push(waiter);
                }
            }
            WorkerMessage::Execute {
                domain: StoreDomain::Barrier,
                job,
            } => {
                let aggregate = aggregate_barrier_failures(&barrier_failures);
                if let Some(error) = aggregate.as_ref() {
                    job(Err(error));
                } else {
                    match store.as_mut() {
                        Ok(database) => job(Ok(database)),
                        Err(error) => job(Err(&*error)),
                    };
                }
            }
            WorkerMessage::Execute { domain, job } => {
                let failure = match store.as_mut() {
                    Ok(database) => job(Ok(database)),
                    Err(error) => job(Err(&*error)),
                };
                match failure {
                    BarrierEffect::Ignore => {}
                    BarrierEffect::Success => {
                        clear_resolved_barrier_failures(&mut barrier_failures, domain)
                    }
                    BarrierEffect::Failure(error) => barrier_failures.push((domain, error)),
                }
            }
        }
        // Scheduling messages do not represent storage activity.
        // A duplicate cannot change the original startup deadline.
        // Barrier requests do count: shutdown's final writes must run first.
        if ordinary_request {
            maintenance.request_completed(Instant::now());
        }
    }
}

fn send_maintenance_result(
    waiter: MaintenanceWaiter,
    outcome: &Result<LocalStorageMaintenanceResult, StoreOperationError>,
) {
    let _ = waiter.sender.send(StoreEvent {
        request_id: waiter.request_id,
        domain: StoreDomain::CloudSync,
        generation: 0,
        outcome: outcome.clone(),
    });
}

fn complete_maintenance_waiters(
    waiters: &mut Vec<MaintenanceWaiter>,
    outcome: &Result<LocalStorageMaintenanceResult, StoreOperationError>,
) {
    for waiter in waiters.drain(..) {
        send_maintenance_result(waiter, outcome);
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
        local_replica_payload_limit: None,
        message: format!(
            "{} storage request(s) failed before the flush barrier ({})",
            failures.len(),
            categories.join(", ")
        ),
    })
}

#[derive(Clone)]
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
    pub workspace_restore: WorkspaceRestoreManifest,
    pub device_windows: DeviceWindowManifest,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadMainWindowState;

impl StoreRequest for LoadMainWindowState {
    type Response = Option<MainWindowState>;

    fn domain(&self) -> StoreDomain {
        StoreDomain::WindowState
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
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

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        store.save_main_window_state(&self.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoadWorkspaceRestoreManifest;

#[derive(Debug, Clone, Copy)]
pub struct LoadWorkspaceStartupRestore;

impl StoreRequest for LoadWorkspaceStartupRestore {
    type Response = WorkspaceStartupRestore;

    fn domain(&self) -> StoreDomain {
        StoreDomain::Sessions
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        store.load_workspace_startup_restore()
    }
}

impl StoreRequest for LoadWorkspaceRestoreManifest {
    type Response = WorkspaceRestoreManifest;

    fn domain(&self) -> StoreDomain {
        StoreDomain::Sessions
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        store.load_workspace_restore_manifest()
    }
}

#[derive(Debug, Clone)]
pub struct SaveWorkspaceRestoreManifest(pub WorkspaceRestoreManifest);

impl StoreRequest for SaveWorkspaceRestoreManifest {
    type Response = ();

    fn domain(&self) -> StoreDomain {
        StoreDomain::Sessions
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        store.save_workspace_restore_manifest(&self.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoadDeviceWindowManifest(pub WorkspaceId);

impl StoreRequest for LoadDeviceWindowManifest {
    type Response = DeviceWindowManifest;

    fn domain(&self) -> StoreDomain {
        StoreDomain::WindowState
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        store.load_device_window_manifest(self.0)
    }
}

#[derive(Debug, Clone)]
pub struct SaveDeviceWindowManifest(pub DeviceWindowManifest);

impl StoreRequest for SaveDeviceWindowManifest {
    type Response = ();

    fn domain(&self) -> StoreDomain {
        StoreDomain::WindowState
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        store.save_device_window_manifest(&self.0)
    }
}

pub struct LoadBootstrap;

impl LoadBootstrap {
    /// Read-only bootstrap load, also usable from jobs that only hold a shared
    /// store reference.
    pub fn load(self, store: &ConnectionStore) -> Result<BootstrapSnapshot, StorageError> {
        let sessions = store.load_sessions()?;
        let quick_commands = store.load_quick_commands()?;
        let ai_history = store.load_ai_history()?;
        let workspace_restore = store.load_workspace_restore_manifest()?;
        let legacy_workspace_id = workspace_restore
            .most_recent()
            .map(|workspace| workspace.id)
            .unwrap_or_default();
        let device_windows = store.load_device_window_manifest(legacy_workspace_id)?;
        let open_tabs = workspace_restore
            .most_recent()
            .map(|workspace| workspace.sessions.open_tabs.clone())
            .unwrap_or_default();
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
            open_tabs,
            workspace_restore,
            device_windows,
        })
    }
}

impl StoreRequest for LoadBootstrap {
    type Response = BootstrapSnapshot;

    fn domain(&self) -> StoreDomain {
        StoreDomain::Bootstrap
    }

    fn execute(self, store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        self.load(&*store)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlushBarrier;

impl StoreRequest for FlushBarrier {
    type Response = ();

    fn domain(&self) -> StoreDomain {
        StoreDomain::Barrier
    }

    fn execute(self, _store: &mut ConnectionStore) -> Result<Self::Response, StorageError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FlushBarrier, LoadBootstrap, LoadMainWindowState, SaveMainWindowState, StoreClientError,
        StoreConfig, StoreDomain, StoreExclusiveFnRequest, StoreOperationError, StoreRuntime,
        store_request,
    };
    use futures::{FutureExt as _, StreamExt as _};
    use nyaterm_core::{MainWindowBounds, MainWindowState};
    use std::time::Instant;

    fn temp_dir(label: &str) -> nyaterm_core::test_support::TestTempDir {
        nyaterm_core::test_support::TestTempDir::new(&format!("nyaterm-store-runtime-{label}"))
    }

    fn fixture_outcome(success: bool) -> Result<(), crate::StorageError> {
        if success {
            Ok(())
        } else {
            Err(crate::StorageError::InvalidData(
                "synthetic private fixture".into(),
            ))
        }
    }

    #[test]
    fn replica_payload_limit_remains_typed_across_the_worker_boundary() {
        let dir = temp_dir("replica-limit-error");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: dir.path().into(),
            portable_key_path: None,
        })
        .unwrap();
        let error = runtime
            .blocking_client()
            .request_fn(
                StoreDomain::CloudSync,
                |_| -> Result<(), crate::StorageError> {
                    Err(crate::StorageError::PortableSnapshot(
                        nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge {
                            limit_bytes: 42,
                        },
                    ))
                },
            )
            .unwrap_err();
        let StoreClientError::Operation(error) = error else {
            panic!("expected operation error")
        };
        assert_eq!(error.category(), "replica_payload_too_large");
        assert_eq!(error.local_replica_payload_limit(), Some(42));
        assert_eq!(
            error.user_message(),
            "portable snapshot error: local replica payload exceeds the 42-byte limit"
        );
    }

    fn fixture_request(
        runtime: &StoreRuntime,
        asynchronous: bool,
        silent: bool,
        domain: StoreDomain,
        success: bool,
    ) -> bool {
        if silent {
            let request = StoreExclusiveFnRequest {
                domain,
                operation: move |_: &mut crate::ConnectionStore| fixture_outcome(success),
                response: std::marker::PhantomData,
            };
            if asynchronous {
                futures::executor::block_on(runtime.ui_client().try_submit(0, request).unwrap())
                    .outcome
                    .is_ok()
            } else {
                runtime
                    .blocking_client()
                    .request(0, request)
                    .unwrap()
                    .outcome
                    .is_ok()
            }
        } else {
            let request = store_request(domain, move |_| fixture_outcome(success));
            if asynchronous {
                futures::executor::block_on(runtime.ui_client().try_submit(0, request).unwrap())
                    .outcome
                    .is_ok()
            } else {
                runtime
                    .blocking_client()
                    .request(0, request)
                    .unwrap()
                    .outcome
                    .is_ok()
            }
        }
    }

    #[test]
    fn silent_maintenance_neither_adds_nor_resolves_barrier_failures_in_both_clients() {
        for asynchronous in [false, true] {
            for maintenance_success in [false, true] {
                for prior_failure in [false, true] {
                    let dir = temp_dir("silent-maintenance");
                    let runtime = StoreRuntime::spawn(StoreConfig {
                        config_dir: dir.path().into(),
                        portable_key_path: None,
                    })
                    .unwrap();
                    let client = runtime.blocking_client();
                    if prior_failure {
                        assert!(!fixture_request(
                            &runtime,
                            asynchronous,
                            false,
                            StoreDomain::CloudSync,
                            false
                        ));
                    }
                    assert_eq!(
                        fixture_request(
                            &runtime,
                            asynchronous,
                            true,
                            StoreDomain::CloudSync,
                            maintenance_success
                        ),
                        maintenance_success
                    );
                    assert_eq!(
                        client.request(0, FlushBarrier).unwrap().outcome.is_err(),
                        prior_failure
                    );
                    assert!(fixture_request(
                        &runtime,
                        asynchronous,
                        true,
                        StoreDomain::Settings,
                        true
                    ));
                    assert_eq!(
                        client.request(0, FlushBarrier).unwrap().outcome.is_err(),
                        prior_failure
                    );
                    assert!(fixture_request(
                        &runtime,
                        asynchronous,
                        false,
                        StoreDomain::Settings,
                        true
                    ));
                    assert_eq!(
                        client.request(0, FlushBarrier).unwrap().outcome.is_err(),
                        prior_failure
                    );
                    assert!(fixture_request(
                        &runtime,
                        asynchronous,
                        false,
                        StoreDomain::CloudSync,
                        true
                    ));
                    assert!(client.request(0, FlushBarrier).unwrap().outcome.is_ok());
                }
            }
        }
    }

    #[test]
    fn pending_maintenance_allows_bootstrap_and_requests_and_is_cancelled_before_shutdown_barrier()
    {
        let dir = temp_dir("maintenance-shutdown");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: dir.path().into(),
            portable_key_path: None,
        })
        .unwrap();
        let client = runtime.blocking_client();
        client.request(0, LoadBootstrap).unwrap().outcome.unwrap();
        let mut first = runtime.schedule_local_storage_maintenance().unwrap();
        let mut duplicate = runtime.schedule_local_storage_maintenance().unwrap();
        client
            .request_fn(StoreDomain::Connections, |_| Ok(()))
            .unwrap();
        assert!((&mut first).now_or_never().is_none());
        assert!((&mut duplicate).now_or_never().is_none());
        runtime.begin_shutdown();
        client
            .request_shutdown(0, FlushBarrier)
            .unwrap()
            .outcome
            .unwrap();
        let first_error = futures::executor::block_on(first).outcome.unwrap_err();
        let duplicate_error = futures::executor::block_on(duplicate).outcome.unwrap_err();
        assert!(first_error == duplicate_error);
        assert!(runtime.schedule_local_storage_maintenance().is_err());
        runtime.resume_after_failed_shutdown();
        let cached =
            futures::executor::block_on(runtime.schedule_local_storage_maintenance().unwrap())
                .outcome
                .unwrap_err();
        assert!(cached == first_error);
        client.request(0, FlushBarrier).unwrap().outcome.unwrap();
    }

    #[test]
    #[ignore = "manual integration check of the real thirty-second startup delay"]
    fn deferred_maintenance_executes_once_and_duplicate_windows_receive_the_cached_result() {
        let dir = temp_dir("maintenance-completion");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: dir.path().into(),
            portable_key_path: None,
        })
        .unwrap();
        runtime
            .blocking_client()
            .request(0, LoadBootstrap)
            .unwrap()
            .outcome
            .unwrap();
        let started = Instant::now();
        let first = runtime.schedule_local_storage_maintenance().unwrap();
        let second = runtime.schedule_local_storage_maintenance().unwrap();
        let result = futures::executor::block_on(first).outcome.unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_secs(30));
        assert_eq!(futures::executor::block_on(second).outcome.unwrap(), result);
        assert_eq!(
            futures::executor::block_on(runtime.schedule_local_storage_maintenance().unwrap())
                .outcome
                .unwrap(),
            result
        );
        assert_eq!(runtime.sync_mutation_generation(), 0);
    }

    #[test]
    fn bootstrap_and_barrier_run_on_the_store_worker() {
        let config_dir = temp_dir("bootstrap");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.path().to_path_buf(),
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
    fn cloud_apply_checks_local_content_and_persists_the_applied_baseline_in_one_job() {
        use nyaterm_core::cloud_sync::{
            CloudLocalStore, CloudSyncError, CloudSyncState, LocalCloudSyncOptions,
        };

        let dir = temp_dir("cloud-apply-guard");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: dir.path().to_path_buf(),
            portable_key_path: None,
        })
        .unwrap();
        let client = runtime.blocking_client();
        let options = LocalCloudSyncOptions {
            device_name: None,
            config_dir: dir.path().to_path_buf(),
            portable_key_path: None,
            remote_dir: dir.join("remote"),
            remote_root: "test".into(),
            device_id: "device".into(),
            app_version: "test".into(),
            master_password: "test-password".into(),
            enabled: true,
        };
        let mut incoming = client.build_sync_snapshot(&options).unwrap();
        incoming.recalculate_hash().unwrap();
        let initial_hash = incoming.sync_content_hash().unwrap();
        client
            .request_mutation_fn(StoreDomain::Settings, |store| {
                store.create_note(None, Some("Local edit".into()), Some("Text".into()))
            })
            .unwrap();
        let result = client.apply_sync_snapshot_checked(
            &options,
            &incoming,
            &initial_hash,
            CloudSyncState::default(),
        );
        assert!(matches!(
            result,
            Err(CloudSyncError::LocalChangedDuringSync)
        ));
        assert!(
            client
                .request_fn(StoreDomain::CloudSync, |store| store
                    .load_cloud_sync_state())
                .unwrap()
                .last_synced_content_hash
                .is_none()
        );
        let current_hash = client
            .build_sync_snapshot(&options)
            .unwrap()
            .sync_content_hash()
            .unwrap();
        assert_ne!(initial_hash, current_hash);

        let (backup, state) = client
            .apply_sync_snapshot_checked(
                &options,
                &incoming,
                &current_hash,
                CloudSyncState::default(),
            )
            .unwrap();
        assert!(backup.safety_backup_path.unwrap().is_file());
        let applied_hash = client
            .build_sync_snapshot(&options)
            .unwrap()
            .sync_content_hash()
            .unwrap();
        assert_eq!(
            state.last_synced_content_hash.as_deref(),
            Some(applied_hash.as_str())
        );
        assert_eq!(
            client
                .request_fn(StoreDomain::CloudSync, |store| store
                    .load_cloud_sync_state())
                .unwrap(),
            state
        );
        client
            .request_mutation_fn(StoreDomain::Settings, |store| {
                let mut settings = store.load_translation_settings()?;
                settings.target_language = "ja".into();
                store.save_translation_settings(settings)
            })
            .unwrap();
        assert_eq!(
            client
                .build_sync_snapshot(&options)
                .unwrap()
                .sync_content_hash()
                .unwrap(),
            applied_hash
        );
        assert_eq!(
            client
                .request_fn(StoreDomain::CloudSync, |store| store
                    .load_cloud_sync_state())
                .unwrap(),
            state
        );
    }

    #[test]
    fn request_ids_are_monotonic_across_clients() {
        let config_dir = temp_dir("request-ids");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.path().to_path_buf(),
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
    fn sync_generation_tracks_only_successful_marked_writes() {
        let config_dir = temp_dir("sync-generation");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.path().to_path_buf(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let client = runtime.blocking_client();
        assert_eq!(runtime.sync_mutation_generation(), 0);
        client
            .request_fn(StoreDomain::Connections, |store| store.load_sessions())
            .expect("read sessions");
        assert_eq!(runtime.sync_mutation_generation(), 0);
        client
            .request_mutation_fn(StoreDomain::Connections, |store| {
                store.replace_sessions(&nyaterm_core::SessionsConfig::default())
            })
            .expect("replace sessions");
        assert_eq!(runtime.sync_mutation_generation(), 1);
        let mut events = runtime.take_sync_mutation_events().expect("event receiver");
        let event = futures::executor::block_on(events.next()).expect("mutation event");
        assert_eq!(event.domain, StoreDomain::Connections);
        assert_eq!(event.generation, 1);
        assert!(runtime.take_sync_mutation_events().is_none());
        assert!(
            client
                .request_mutation_fn(
                    StoreDomain::Connections,
                    |_| -> Result<(), crate::StorageError> {
                        Err(crate::StorageError::InvalidData("failed write".to_string()))
                    },
                )
                .is_err()
        );
        assert_eq!(runtime.sync_mutation_generation(), 1);
    }

    #[test]
    fn blocking_client_error_debug_redacts_operation_details() {
        let error = StoreClientError::Operation(StoreOperationError {
            category: "storage",
            message: "secret-token-value".to_string(),
            local_replica_payload_limit: None,
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
            config_dir: config_dir.path().to_path_buf(),
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
    fn concurrent_ai_history_appends_preserve_every_session_and_message() {
        let config_dir = temp_dir("ai-concurrent-history");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.path().to_path_buf(),
            portable_key_path: None,
        })
        .expect("spawn runtime");
        let client = runtime.blocking_client();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        std::thread::scope(|scope| {
            let tasks = (0..8)
                .map(|index| {
                    let client = client.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        let session_id = format!("parallel-{index}");
                        barrier.wait();
                        client
                            .request_fn(StoreDomain::Ai, move |store| {
                                store.append_ai_user_message(
                                    &session_id,
                                    None,
                                    format!("prompt {index}"),
                                )
                            })
                            .expect("append AI message");
                    })
                })
                .collect::<Vec<_>>();
            for task in tasks {
                task.join().expect("AI writer thread");
            }
        });

        let history = client
            .request_fn(StoreDomain::Ai, |store| store.load_ai_history())
            .expect("load AI history");
        assert_eq!(history.sessions.len(), 8);
        assert_eq!(history.messages.len(), 8);
    }

    #[test]
    fn flush_barrier_retains_failures_until_the_domain_succeeds() {
        let config_dir = temp_dir("barrier-failure");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.path().to_path_buf(),
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
            config_dir: config_dir.path().to_path_buf(),
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
            config_dir: config_dir.path().to_path_buf(),
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
