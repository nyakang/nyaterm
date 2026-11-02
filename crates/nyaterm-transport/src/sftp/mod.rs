//! SFTP browsing and file transfer.
//!
//! Domain-specific SFTP implementation. Protocol, retry, resume, and progress contracts
//! remain unchanged; downloads additionally validate remote names, local path boundaries,
//! and symbolic links to prevent server-controlled names from redirecting local writes.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex as StdMutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use russh::{ChannelOpenFailure, Disconnect, client};
use russh_sftp::client::{Config as SftpClientConfig, SftpSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::Instrument as _;

use super::{
    PROCESS_TIMEOUT, SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest,
    SftpDuplicateResolver, SftpPathTransferOptions, SftpTransferDirection, SftpTransferOptions,
    SftpTransferProgress, SftpTransferSummary, SshClientHandler, SshMultiplexHandle,
    SshProcessService, SshSessionConfig, open_authenticated_ssh_handle,
};
use crate::remote_file::copy_progress::CopyProgress;
use crate::remote_file::{
    RemoteTextDocument, RemoteTextMetadata, RemoteTextRevision, RemoteTextWriteResult,
    metadata_is_stable,
};

mod delete;
mod drag_inventory;
mod export_reader;
pub(crate) use export_reader::spawn_export_reader;
mod path_codec;
pub use path_codec::SftpPathCodec;
mod contracts;
pub use contracts::{
    RemoteBinaryFile, RemoteFilePath, SftpAttributeUpdate, SftpFileEntry, SftpFileProperties,
    SftpFileType, SftpRemoteTextFile, SftpTransferControl, SftpWriteTextResult,
};
mod remote_metadata;
use remote_metadata::{
    attrs_to_sftp_file_type, ensure_remote_text_bytes, ensure_safe_remote_delete_target,
    format_sftp_permissions, remote_file_name, remote_file_path_bytes, rename_target_path_bytes,
    resolve_remote_group_name, resolve_remote_group_value, resolve_remote_identity_names,
    resolve_remote_user_name, resolve_remote_user_value,
};
mod transfer_paths;
mod tuning;
mod upload_metadata;
use transfer_paths::{
    SftpLocalDownloadTargetContext, SftpRemoteWriteTargetContext, remote_join,
    resolve_local_download_target, resolve_remote_upload_target, resolve_remote_write_target,
};
#[cfg(test)]
use transfer_paths::{remote_conflict_candidate, resolve_duplicate_decision};
use tuning::TransferParameters;
#[cfg(test)]
use upload_metadata::upload_file_mode as remote_upload_file_mode;
use upload_metadata::{UploadMetadata, finish_upload_metadata};

pub const SFTP_TRANSFER_CANCELLED: &str = "SFTP transfer cancelled";

const SFTP_SMALL_FILE_THRESHOLD: u64 = 512 * 1024;
const SFTP_DEFAULT_SMALL_FILE_CONCURRENCY: usize = 16;
const SFTP_MAX_SMALL_FILE_CONCURRENCY: usize = 16;
const SFTP_SMALL_FILE_WORKERS_PER_SESSION: usize = 8;
const SFTP_DEFAULT_SESSION_POOL_SIZE: usize = 2;
const SFTP_MAX_SESSION_POOL_SIZE: usize = 4;
const SFTP_LARGE_FILE_CONCURRENCY: usize = 2;
const SFTP_HANDLE_RESERVE: usize = 8;
const SFTP_DIRECTORY_STALL_TIMEOUT: Duration = Duration::from_secs(60);
const SFTP_PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const SFTP_CHANNEL_OPEN_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(150),
    Duration::from_millis(300),
];

#[derive(Debug, Clone)]
pub struct SftpService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    compatibility: Option<Arc<SftpCompatibilityState>>,
    session_id: u64,
    export_budget: Arc<tokio::sync::Semaphore>,
}

tokio::task_local! {
    static SFTP_COMPATIBILITY_CACHE: Option<Arc<StdMutex<Option<OpenSftpSession>>>>;
}

static NEXT_SFTP_COMPATIBILITY_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static SFTP_COMPATIBILITY_RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> =
    OnceLock::new();

fn compatibility_runtime() -> anyhow::Result<&'static tokio::runtime::Runtime> {
    SFTP_COMPATIBILITY_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("nyaterm-sftp-compatible")
                .build()
                .map_err(|error| format!("failed to start SFTP runtime: {error}"))
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

#[derive(Debug)]
enum SftpCompatibilityRunner {
    Dedicated,
    Multiplex(SshMultiplexHandle),
}

struct SftpCompatibilityState {
    id: u64,
    gate: tokio::sync::Mutex<()>,
    runner: SftpCompatibilityRunner,
    cache: Arc<StdMutex<Option<OpenSftpSession>>>,
}

impl std::fmt::Debug for SftpCompatibilityState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SftpCompatibilityState")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl SftpCompatibilityState {
    fn dedicated() -> Self {
        Self {
            id: NEXT_SFTP_COMPATIBILITY_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            gate: tokio::sync::Mutex::new(()),
            runner: SftpCompatibilityRunner::Dedicated,
            cache: Arc::new(StdMutex::new(None)),
        }
    }

    fn multiplex(handle: SshMultiplexHandle) -> Self {
        Self {
            id: NEXT_SFTP_COMPATIBILITY_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            gate: tokio::sync::Mutex::new(()),
            runner: SftpCompatibilityRunner::Multiplex(handle),
            cache: Arc::new(StdMutex::new(None)),
        }
    }

    fn block_on<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let _guard = self.gate.blocking_lock();
        let result = self.block_on_unguarded(operation);
        if result
            .as_ref()
            .err()
            .is_some_and(sftp_error_invalidates_compatibility_session)
        {
            self.evict_closed_session();
        }
        result
    }

    // Call only while holding this state's gate; never retry the failed operation.
    fn evict_closed_session(&self) {
        let session = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(session) = session {
            let _ = self.block_on_unguarded(async move {
                force_close_sftp_session(session).await;
                Ok(())
            });
        }
    }

    // Async controlled operations retain the gate until bounded cleanup completes.
    async fn evict_failed_session(&self) {
        let session = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(session) = session {
            let _ = tokio::time::timeout(Duration::from_secs(5), force_close_sftp_session(session))
                .await;
        }
    }

    fn block_on_unguarded<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let operation = SFTP_COMPATIBILITY_CACHE.scope(Some(self.cache.clone()), operation);
        match &self.runner {
            SftpCompatibilityRunner::Dedicated => compatibility_runtime()?.block_on(operation),
            SftpCompatibilityRunner::Multiplex(handle) => handle.block_on(operation),
        }
    }
}

impl Drop for SftpCompatibilityState {
    fn drop(&mut self) {
        let session = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(session) = session else {
            return;
        };
        match &self.runner {
            SftpCompatibilityRunner::Dedicated => {
                if let Ok(runtime) = compatibility_runtime() {
                    runtime.spawn(force_close_sftp_session(session));
                }
            }
            SftpCompatibilityRunner::Multiplex(handle) => {
                handle
                    .inner
                    .runtime
                    .spawn(force_close_sftp_session(session));
            }
        }
    }
}

fn run_sftp_operation<T, F>(operation: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nyaterm-sftp")
        .build()
        .map_err(|error| anyhow::anyhow!("failed to start SFTP runtime: {error}"))?;
    runtime.block_on(operation)
}

#[derive(Clone)]
struct OpenSftpSession {
    sftp: Arc<SftpSession>,
    connection: Arc<StdMutex<Option<OpenSftpConnection>>>,
    persistent: bool,
}

enum OpenSftpConnection {
    Dedicated {
        handle: client::Handle<SshClientHandler>,
        jump_handles: Vec<client::Handle<SshClientHandler>>,
    },
    Multiplex,
}

/// Verify a file-only runtime before reporting the session ready.
pub(crate) async fn probe_sftp_session(
    config: &SshSessionConfig,
    multiplex: &SshMultiplexHandle,
) -> anyhow::Result<()> {
    let session = config
        .attempt
        .until_cancelled(open_sftp_session(config, Some(multiplex)))
        .await
        .map_err(anyhow::Error::msg)??;
    close_sftp_session(session).await;
    Ok(())
}

async fn open_sftp_session(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
) -> anyhow::Result<OpenSftpSession> {
    let mut client_config = SftpClientConfig::default();
    if let Some(depth) = config.sftp.pipeline_depth {
        client_config.max_concurrent_reads = depth.clamp(4, 64) as usize;
        client_config.max_concurrent_writes = depth.clamp(4, 64) as usize;
    }
    open_sftp_session_with_client_config(config, multiplex, client_config).await
}

async fn open_sftp_session_with_client_config(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    client_config: SftpClientConfig,
) -> anyhow::Result<OpenSftpSession> {
    let cache = SFTP_COMPATIBILITY_CACHE
        .try_with(Clone::clone)
        .ok()
        .flatten();
    if let Some(cache) = cache.as_ref()
        && let Some(session) = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
    {
        return Ok(session);
    }
    let mut client_config = client_config;
    client_config.max_write_packet_len = client_config.max_packet_len;
    if config.sftp.compatibility_mode {
        client_config.max_concurrent_writes = 1;
        client_config.max_concurrent_reads = 1;
    }
    let (channel, connection) = if let Some(multiplex) = multiplex {
        multiplex.ensure_matches_config(config)?;
        let handle = multiplex.exec_target_handle().await;
        let channel = open_multiplex_sftp_channel(&handle, config.sftp.compatibility_mode).await?;
        (channel, OpenSftpConnection::Multiplex)
    } else {
        let (handle, jump_handles) = open_authenticated_ssh_handle(config).await?;
        let channel = open_dedicated_sftp_channel(&handle, config.sftp.compatibility_mode).await?;
        (
            channel,
            OpenSftpConnection::Dedicated {
                handle,
                jump_handles,
            },
        )
    };
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| anyhow::anyhow!("failed to start SFTP subsystem: {error}"))?;
    let sftp = tokio::time::timeout(
        Duration::from_secs(30),
        SftpSession::new_with_config(channel.into_stream(), client_config),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SFTP initialization timed out"))??;
    let session = OpenSftpSession {
        sftp: Arc::new(sftp),
        connection: Arc::new(StdMutex::new(Some(connection))),
        persistent: cache.is_some(),
    };
    if let Some(cache) = cache {
        *cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session.clone());
    }
    Ok(session)
}

async fn open_multiplex_sftp_channel(
    handle: &super::SharedSshHandle,
    compatibility_mode: bool,
) -> anyhow::Result<russh::Channel<russh::client::Msg>> {
    let retry_delays: &[Duration] = if compatibility_mode {
        &[]
    } else {
        &SFTP_CHANNEL_OPEN_RETRY_DELAYS
    };
    let attempts = retry_delays
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None));
    for retry_delay in attempts {
        let result = {
            let handle = handle.lock().await;
            tokio::time::timeout(Duration::from_secs(30), handle.channel_open_session()).await
        };
        match result {
            Ok(Ok(channel)) => return Ok(channel),
            Ok(Err(error))
                if retry_delay.is_some() && is_retryable_sftp_channel_open_error(&error) =>
            {
                tokio::time::sleep(retry_delay.expect("retry delay checked above")).await;
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => anyhow::bail!("SFTP channel open timed out"),
        }
    }
    unreachable!("SFTP channel retry loop returns or retries")
}

async fn open_dedicated_sftp_channel(
    handle: &client::Handle<SshClientHandler>,
    compatibility_mode: bool,
) -> anyhow::Result<russh::Channel<russh::client::Msg>> {
    let retry_delays: &[Duration] = if compatibility_mode {
        &[]
    } else {
        &SFTP_CHANNEL_OPEN_RETRY_DELAYS
    };
    let attempts = retry_delays
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None));
    for retry_delay in attempts {
        match tokio::time::timeout(Duration::from_secs(30), handle.channel_open_session()).await {
            Ok(Ok(channel)) => return Ok(channel),
            Ok(Err(error))
                if retry_delay.is_some() && is_retryable_sftp_channel_open_error(&error) =>
            {
                tokio::time::sleep(retry_delay.expect("retry delay checked above")).await;
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => anyhow::bail!("SFTP channel open timed out"),
        }
    }
    unreachable!("SFTP channel retry loop returns or retries")
}

fn is_retryable_sftp_channel_open_error(error: &russh::Error) -> bool {
    matches!(
        error,
        russh::Error::ChannelOpenFailure(
            ChannelOpenFailure::ConnectFailed | ChannelOpenFailure::ResourceShortage
        )
    )
}

async fn close_sftp_session(session: OpenSftpSession) {
    if session.persistent {
        return;
    }
    force_close_sftp_session(session).await;
}

async fn force_close_sftp_session(session: OpenSftpSession) {
    let OpenSftpSession {
        sftp, connection, ..
    } = session;
    let connection = connection
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(connection) = connection else {
        return;
    };
    if sftp.close().await.is_err() {
        tracing::warn!(
            stage = "session_close",
            category = "close_failed",
            "SFTP close failed"
        );
    }
    if tokio::time::timeout(Duration::from_secs(5), close_sftp_connection(connection))
        .await
        .is_err()
    {
        tracing::warn!(
            stage = "connection_close",
            category = "timeout",
            "SFTP close timed out"
        );
    }
}

async fn close_sftp_connection(connection: OpenSftpConnection) {
    if let OpenSftpConnection::Dedicated {
        handle,
        jump_handles,
    } = connection
    {
        if handle
            .disconnect(Disconnect::ByApplication, "sftp session closed", "en")
            .await
            .is_err()
        {
            tracing::warn!(
                stage = "connection_close",
                category = "close_failed",
                "SFTP disconnect failed"
            );
        }
        for jump_handle in jump_handles {
            if jump_handle
                .disconnect(Disconnect::ByApplication, "sftp session closed", "en")
                .await
                .is_err()
            {
                tracing::warn!(
                    stage = "jump_close",
                    category = "close_failed",
                    "SFTP jump disconnect failed"
                );
            }
        }
    }
}

#[cfg(test)]
fn sftp_pipeline_config(options: &SftpTransferOptions) -> (usize, usize) {
    let parameters = TransferParameters::new(options, &Default::default());
    ((parameters.payload + 1024) / 1024, parameters.writes)
}

fn sftp_upload_buffer_size(options: &SftpTransferOptions) -> usize {
    TransferParameters::new(options, &Default::default()).payload
}

#[cfg(test)]
fn sftp_client_config_for_options(options: &SftpTransferOptions) -> SftpClientConfig {
    TransferParameters::new(options, &Default::default()).client_config()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SftpDirectoryConcurrency {
    session_pool_size: usize,
    small_file_concurrency: usize,
    large_file_concurrency: usize,
}

fn sftp_directory_concurrency(
    max_open_handles: Option<u64>,
    _options: &SftpTransferOptions,
    compatibility_mode: bool,
) -> SftpDirectoryConcurrency {
    if compatibility_mode {
        return SftpDirectoryConcurrency {
            session_pool_size: 1,
            small_file_concurrency: 1,
            large_file_concurrency: 1,
        };
    }
    let requested_small_file_concurrency = SFTP_DEFAULT_SMALL_FILE_CONCURRENCY;
    let server_limit = max_open_handles
        .map(|handles| handles.saturating_sub(SFTP_HANDLE_RESERVE as u64) as usize)
        .unwrap_or(requested_small_file_concurrency)
        .max(1);
    let session_pool_size = SFTP_DEFAULT_SESSION_POOL_SIZE
        .min(SFTP_MAX_SESSION_POOL_SIZE)
        .min(server_limit)
        .max(1);
    let small_file_concurrency = server_limit
        .min(session_pool_size * SFTP_SMALL_FILE_WORKERS_PER_SESSION)
        .min(SFTP_MAX_SMALL_FILE_CONCURRENCY)
        .min(requested_small_file_concurrency)
        .max(1);
    let large_file_concurrency = SFTP_LARGE_FILE_CONCURRENCY
        .min(small_file_concurrency)
        .max(1);

    SftpDirectoryConcurrency {
        session_pool_size,
        small_file_concurrency,
        large_file_concurrency,
    }
}

fn directory_upload_worker_count(
    file_count: usize,
    concurrency: SftpDirectoryConcurrency,
) -> usize {
    if file_count == 0 {
        0
    } else {
        file_count.min(concurrency.small_file_concurrency)
    }
}

fn sftp_session_pool_index(worker_index: usize, session_count: usize) -> usize {
    worker_index % session_count.max(1)
}

fn is_sftp_large_file(size: u64) -> bool {
    size > SFTP_SMALL_FILE_THRESHOLD
}

#[derive(Clone)]
struct SftpSessionPool {
    sessions: Arc<Vec<Arc<PooledSftpSession>>>,
}

struct PooledSftpSession {
    sftp: Arc<SftpSession>,
    connection: Arc<StdMutex<Option<OpenSftpConnection>>>,
    persistent: bool,
}

impl PooledSftpSession {
    fn from_open_session(session: OpenSftpSession) -> Self {
        Self {
            sftp: session.sftp,
            connection: session.connection,
            persistent: session.persistent,
        }
    }

    async fn close(&self) {
        if self.persistent {
            return;
        }
        let connection = self
            .connection
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let _ = self.sftp.close().await;
        if let Some(connection) = connection {
            close_sftp_connection(connection).await;
        }
    }
}

impl SftpSessionPool {
    async fn open(
        config: &SshSessionConfig,
        multiplex: Option<&SshMultiplexHandle>,
        size: usize,
        client_config: SftpClientConfig,
    ) -> anyhow::Result<Self> {
        let mut sessions = Vec::with_capacity(size);
        for _ in 0..size {
            match open_sftp_session_with_client_config(config, multiplex, client_config.clone())
                .await
            {
                Ok(session) => {
                    sessions.push(Arc::new(PooledSftpSession::from_open_session(session)))
                }
                Err(error) => {
                    for session in sessions {
                        session.close().await;
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            sessions: Arc::new(sessions),
        })
    }

    fn session_for(&self, worker_index: usize) -> Arc<PooledSftpSession> {
        let index = sftp_session_pool_index(worker_index, self.sessions.len());
        Arc::clone(&self.sessions[index])
    }

    async fn close_all(self) {
        for session in self.sessions.iter() {
            session.close().await;
        }
    }
}

impl SftpService {
    fn effective_transfer_options(&self, options: SftpTransferOptions) -> SftpTransferOptions {
        if self.config.sftp.compatibility_mode {
            options
                .with_download_threads(1)
                .with_directory_upload_threads(1)
        } else {
            options
        }
    }

    fn effective_path_options(&self, options: SftpPathTransferOptions) -> SftpPathTransferOptions {
        let transfer = self.effective_transfer_options(options.transfer_options().clone());
        options.with_transfer_options(transfer)
    }
    pub fn new(config: SshSessionConfig) -> Self {
        let export_budget = Arc::new(tokio::sync::Semaphore::new(
            if config.sftp.compatibility_mode { 1 } else { 3 },
        ));
        let compatibility = config
            .sftp
            .compatibility_mode
            .then(|| Arc::new(SftpCompatibilityState::dedicated()));
        Self {
            config,
            multiplex: None,
            compatibility,
            session_id: NEXT_SFTP_COMPATIBILITY_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            export_budget,
        }
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        let export_budget = Arc::new(tokio::sync::Semaphore::new(
            if config.sftp.compatibility_mode { 1 } else { 3 },
        ));
        let compatibility = config
            .sftp
            .compatibility_mode
            .then(|| Arc::new(SftpCompatibilityState::multiplex(multiplex.clone())));
        Ok(Self {
            config,
            multiplex: Some(multiplex),
            compatibility,
            session_id: NEXT_SFTP_COMPATIBILITY_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            export_budget,
        })
    }

    pub(crate) fn export_runtime(&self) -> anyhow::Result<tokio::runtime::Handle> {
        Ok(if let Some(multiplex) = &self.multiplex {
            multiplex.inner.runtime.handle().clone()
        } else {
            compatibility_runtime()?.handle().clone()
        })
    }

    fn run_controlled_operation<T, F>(
        &self,
        control: SftpTransferControl,
        export: bool,
        operation: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        anyhow::ensure!(
            self.config.remote_file_browser_enabled(),
            "SFTP is disabled for this SSH profile"
        );
        let compatibility = self.compatibility.clone();
        let budget = self.export_budget.clone();
        let operation = async move {
            let _permit = if export {
                Some(control.until_cancelled(budget.acquire_owned()).await??)
            } else {
                None
            };
            let _guard = if let Some(state) = &compatibility {
                Some(control.until_cancelled(state.gate.lock()).await?)
            } else {
                None
            };
            let cache = compatibility.as_ref().map(|state| state.cache.clone());
            let result = SFTP_COMPATIBILITY_CACHE.scope(cache, operation).await;
            if result
                .as_ref()
                .err()
                .is_some_and(sftp_error_invalidates_compatibility_session)
                && let Some(state) = &compatibility
            {
                state.evict_failed_session().await;
            }
            result
        };
        if let Some(multiplex) = &self.multiplex {
            multiplex.block_on(operation)
        } else {
            compatibility_runtime()?.block_on(operation)
        }
    }

    fn run_operation<T, F>(&self, operation_name: &'static str, operation: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        if !self.config.remote_file_browser_enabled() {
            return Err(anyhow::anyhow!("SFTP is disabled for this SSH profile"));
        }
        let operation = operation.instrument(tracing::info_span!(
            "sftp_operation",
            session_id = self.session_id,
            operation = operation_name
        ));
        let result = if let Some(compatibility) = self.compatibility.as_ref() {
            compatibility.block_on(operation)
        } else if let Some(multiplex) = self.multiplex.as_ref() {
            multiplex.block_on(operation)
        } else {
            run_sftp_operation(operation)
        };
        if let Err(error) = result.as_ref() {
            tracing::warn!(
                session_id = self.session_id,
                operation = operation_name,
                stage = "completion",
                category = sftp_error_category(error),
                "SFTP operation failed"
            );
        }
        result
    }

    /// Runs an upload and records its final result; ordinary failures are logged as errors,
    /// while user cancellations are logged as warnings.
    fn run_upload_operation<T, F>(
        &self,
        operation: &'static str,
        attempts: u32,
        operation_future: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let result = self.run_operation(operation, operation_future);
        if let Err(error) = &result {
            if is_sftp_transfer_cancelled(error) {
                tracing::warn!(operation, "SFTP upload cancelled");
            } else {
                log_sftp_upload_failure(operation, attempts, error);
            }
        }
        result
    }

    pub(crate) fn copy_remote_path_to(
        &self,
        source_path: &RemoteFilePath,
        destination: &Self,
        destination_path: &RemoteFilePath,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: CopyProgress,
    ) -> anyhow::Result<u64> {
        if !destination.config.remote_file_browser_enabled() {
            anyhow::bail!("SFTP is disabled for the destination SSH profile");
        }
        let source_path = source_path.clone();
        let destination_path = destination_path.clone();
        let source_config = self.config.clone();
        let source_multiplex = self.multiplex.clone();
        let destination_config = destination.config.clone();
        let destination_multiplex = destination.multiplex.clone();
        let destination_cache = destination
            .compatibility
            .as_ref()
            .map(|state| state.cache.clone());
        let destination_runtime = destination
            .compatibility
            .as_ref()
            .map(|state| match &state.runner {
                SftpCompatibilityRunner::Dedicated => {
                    compatibility_runtime().map(|runtime| runtime.handle().clone())
                }
                SftpCompatibilityRunner::Multiplex(handle) => {
                    Ok(handle.inner.runtime.handle().clone())
                }
            })
            .transpose()?;
        let source_cache = self.compatibility.as_ref().map(|state| state.cache.clone());
        // Lock both endpoints in stable order, including self-copy, before starting any wire work.
        let mut states = [
            self.compatibility.as_ref(),
            destination.compatibility.as_ref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        states.sort_by_key(|state| state.id);
        states.dedup_by_key(|state| state.id);
        let _guards = states
            .iter()
            .map(|state| state.gate.blocking_lock())
            .collect::<Vec<_>>();
        let operation = SFTP_COMPATIBILITY_CACHE.scope(source_cache, async move {
            let source_codec = SftpPathCodec::from_ssh_config(&source_config)?;
            let destination_codec = SftpPathCodec::from_ssh_config(&destination_config)?;
            let source_raw = remote_file_path_bytes(&source_codec, &source_path)?;
            let destination_raw = remote_file_path_bytes(&destination_codec, &destination_path)?;
            let source_session =
                open_sftp_session(&source_config, source_multiplex.as_ref()).await?;
            // Persistent destination tasks must run on its own executor, not the source's.
            let destination_open = SFTP_COMPATIBILITY_CACHE.scope(destination_cache, async move {
                open_sftp_session(&destination_config, destination_multiplex.as_ref()).await
            });
            let destination_result = if let Some(runtime) = destination_runtime {
                match runtime.spawn(destination_open).await {
                    Ok(result) => result,
                    Err(_) => Err(anyhow::anyhow!("destination SFTP task failed")),
                }
            } else {
                destination_open.await
            };
            let destination_session = match destination_result {
                Ok(session) => session,
                Err(error) => {
                    close_sftp_session(source_session).await;
                    return Err(error);
                }
            };
            let result = copy_remote_path_between_sftp(
                &source_session.sftp,
                source_raw,
                &destination_session.sftp,
                destination_raw,
                &control,
                &options,
                &progress,
            )
            .await;
            close_sftp_session(destination_session).await;
            close_sftp_session(source_session).await;
            result
        });
        let result = if let Some(state) = self
            .compatibility
            .as_ref()
            .or(destination.compatibility.as_ref())
        {
            state.block_on_unguarded(operation)
        } else {
            self.run_operation("copy_remote_path", operation)
        };
        if result
            .as_ref()
            .err()
            .is_some_and(sftp_error_is_stream_closed)
        {
            for state in states {
                state.evict_closed_session();
            }
        }
        result
    }

    pub fn list_dir(&self, remote_path: impl AsRef<str>) -> anyhow::Result<Vec<SftpFileEntry>> {
        self.list_dir_path(&RemoteFilePath::new(remote_path.as_ref()))
    }

    pub fn list_dir_path(
        &self,
        remote_path: &RemoteFilePath,
    ) -> anyhow::Result<Vec<SftpFileEntry>> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let identity_config = config.clone();
        let multiplex = self.multiplex.clone();
        let identity_multiplex = multiplex.clone();
        let mut entries = self.run_operation("list_directory", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let sftp = &session.sftp;
            let mut entries = Vec::new();
            for entry in sftp.read_dir_bytes(remote_path_bytes).await? {
                let name_bytes = entry.file_name_bytes();
                let name = codec.decode_path_lossy(name_bytes);
                if name == "." || name == ".." {
                    continue;
                }
                let display_path = remote_join(&remote_path.display_path, &name);
                let raw_path = entry.path_bytes();
                let metadata = entry.metadata();
                let file_type = match entry.file_type() {
                    russh_sftp::protocol::FileType::File => SftpFileType::File,
                    russh_sftp::protocol::FileType::Dir => SftpFileType::Directory,
                    russh_sftp::protocol::FileType::Symlink => SftpFileType::Symlink,
                    russh_sftp::protocol::FileType::Other => SftpFileType::Other,
                };
                let symlink_target_is_directory = file_type == SftpFileType::Symlink
                    && sftp
                        .metadata_bytes(raw_path.clone())
                        .await
                        .ok()
                        .is_some_and(|attrs| {
                            attrs.file_type() == russh_sftp::protocol::FileType::Dir
                        });
                entries.push(SftpFileEntry {
                    name,
                    path: display_path,
                    file_type,
                    size: metadata.size,
                    permissions: metadata.permissions,
                    owner: metadata.uid.map(|uid| uid.to_string()).unwrap_or_default(),
                    group: metadata.gid.map(|gid| gid.to_string()).unwrap_or_default(),
                    modified_at: metadata.mtime,
                    raw_path_token: Some(URL_SAFE_NO_PAD.encode(raw_path)),
                    symlink_target_is_directory,
                });
            }
            entries.sort_by(|left, right| {
                (!left.is_directory())
                    .cmp(&(!right.is_directory()))
                    .then(left.name.cmp(&right.name))
            });
            close_sftp_session(session).await;
            Ok(entries)
        })?;
        let uids = entries
            .iter()
            .filter_map(|entry| entry.owner.parse::<u32>().ok())
            .collect::<Vec<_>>();
        let gids = entries
            .iter()
            .filter_map(|entry| entry.group.parse::<u32>().ok())
            .collect::<Vec<_>>();
        let owners = resolve_remote_identity_names(
            &identity_config,
            identity_multiplex.clone(),
            "passwd",
            &uids,
        );
        let groups =
            resolve_remote_identity_names(&identity_config, identity_multiplex, "group", &gids);
        for entry in &mut entries {
            if let Ok(uid) = entry.owner.parse::<u32>()
                && let Some(owner) = owners.get(&uid)
            {
                entry.owner.clone_from(owner);
            }
            if let Ok(gid) = entry.group.parse::<u32>()
                && let Some(group) = groups.get(&gid)
            {
                entry.group.clone_from(group);
            }
        }
        Ok(entries)
    }

    pub fn rename_path(
        &self,
        old_path: impl AsRef<str>,
        new_path: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        self.rename_remote_paths(
            &RemoteFilePath::new(old_path.as_ref()),
            &RemoteFilePath::new(new_path.as_ref()),
        )
    }

    pub fn rename_remote_paths(
        &self,
        old_path: &RemoteFilePath,
        new_path: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        let old_path = old_path.clone();
        let new_path = new_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("rename", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let old_path_bytes = remote_file_path_bytes(&codec, &old_path)?;
            let new_path_bytes = rename_target_path_bytes(&codec, &old_path, &new_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = session
                .sftp
                .rename_bytes(old_path_bytes, new_path_bytes)
                .await;
            close_sftp_session(session).await;
            result?;
            Ok(())
        })
    }

    pub fn delete_path(&self, remote_path: impl AsRef<str>) -> anyhow::Result<()> {
        self.delete_remote_path(&RemoteFilePath::new(remote_path.as_ref()))
    }

    pub fn delete_remote_path(&self, remote_path: &RemoteFilePath) -> anyhow::Result<()> {
        ensure_safe_remote_delete_target(&remote_path.display_path)?;
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("delete", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let raw_path = remote_file_path_bytes(&codec, &remote_path)?;
            delete::delete_remote_path(&config, multiplex.as_ref(), &codec, &remote_path, raw_path)
                .await
        })
    }

    pub fn create_dir_path(
        &self,
        remote_path: impl AsRef<str>,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        self.create_dir_remote_path(&RemoteFilePath::new(remote_path.as_ref()), mode)
    }

    pub fn create_dir_remote_path(
        &self,
        remote_path: &RemoteFilePath,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("create_directory", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                session
                    .sftp
                    .create_dir_bytes(remote_path_bytes.clone())
                    .await?;
                if let Some(mode) = mode {
                    session
                        .sftp
                        .set_metadata_bytes(
                            remote_path_bytes,
                            russh_sftp::protocol::FileAttributes {
                                permissions: Some(mode),
                                ..russh_sftp::protocol::FileAttributes::empty()
                            },
                        )
                        .await?;
                }
                Ok(())
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn create_file_path(
        &self,
        remote_path: impl AsRef<str>,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        self.create_file_path_impl(&RemoteFilePath::new(remote_path.as_ref()), mode, false)
    }

    pub fn create_file_remote_path(
        &self,
        remote_path: &RemoteFilePath,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        self.create_file_path_impl(remote_path, mode, true)
    }

    fn create_file_path_impl(
        &self,
        remote_path: &RemoteFilePath,
        mode: Option<u32>,
        exclusive: bool,
    ) -> anyhow::Result<()> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("create_file", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let flags = russh_sftp::protocol::OpenFlags::CREATE
                    | russh_sftp::protocol::OpenFlags::WRITE
                    | if exclusive {
                        russh_sftp::protocol::OpenFlags::EXCLUDE
                    } else {
                        russh_sftp::protocol::OpenFlags::TRUNCATE
                    };
                let _file = session
                    .sftp
                    .open_with_flags_bytes(remote_path_bytes.clone(), flags)
                    .await?;
                if let Some(mode) = mode {
                    session
                        .sftp
                        .set_metadata_bytes(
                            remote_path_bytes,
                            russh_sftp::protocol::FileAttributes {
                                permissions: Some(mode),
                                ..russh_sftp::protocol::FileAttributes::empty()
                            },
                        )
                        .await?;
                }
                Ok(())
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn create_symlink_path(
        &self,
        link_path: impl AsRef<str>,
        target_path: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        let link_path = link_path.as_ref().to_string();
        let target_path = target_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("create_symlink", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let link_path = codec.encode_path(&link_path)?;
            let target_path = codec.encode_path(&target_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = session
                .sftp
                .symlink_openssh_bytes(target_path, link_path)
                .await
                .map_err(Into::into);
            close_sftp_session(session).await;
            result
        })
    }

    pub fn file_properties(
        &self,
        remote_path: impl AsRef<str>,
    ) -> anyhow::Result<SftpFileProperties> {
        self.remote_file_properties(&RemoteFilePath::new(remote_path.as_ref()))
    }

    pub fn remote_file_properties(
        &self,
        remote_path: &RemoteFilePath,
    ) -> anyhow::Result<SftpFileProperties> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let identity_config = config.clone();
        let multiplex = self.multiplex.clone();
        let identity_multiplex = multiplex.clone();
        let mut properties = self.run_operation("file_properties", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let attrs = session
                    .sftp
                    .symlink_metadata_bytes(remote_path_bytes.clone())
                    .await?;
                let file_type = attrs_to_sftp_file_type(&attrs);
                let symlink_target_is_directory = file_type == SftpFileType::Symlink
                    && session
                        .sftp
                        .metadata_bytes(remote_path_bytes.clone())
                        .await
                        .ok()
                        .is_some_and(|target| {
                            target.file_type() == russh_sftp::protocol::FileType::Dir
                        });
                let permissions = attrs.permissions;
                Ok(SftpFileProperties {
                    symlink_target: if file_type == SftpFileType::Symlink {
                        Some(codec.decode_path_lossy(
                            &session.sftp.read_link_bytes(remote_path_bytes).await?,
                        ))
                    } else {
                        None
                    },
                    name: remote_file_name(&remote_path.display_path),
                    path: remote_path.display_path.clone(),
                    file_type,
                    size: attrs.size,
                    permissions,
                    permissions_symbolic: permissions
                        .map(|mode| format_sftp_permissions(file_type, mode))
                        .unwrap_or_else(|| "-".to_string()),
                    owner: attrs.uid.map(|value| value.to_string()).unwrap_or_default(),
                    group: attrs.gid.map(|value| value.to_string()).unwrap_or_default(),
                    uid: attrs.uid,
                    gid: attrs.gid,
                    modified_at: attrs.mtime,
                    accessed_at: attrs.atime,
                    raw_path_token: remote_path.raw_path_token.clone(),
                    symlink_target_is_directory,
                })
            }
            .await;
            close_sftp_session(session).await;
            result
        })?;

        // Identity lookup opens an SSH exec operation. Keep it outside the
        // SFTP runtime so the synchronous service does not nest Tokio runtimes.
        if let Some(owner) =
            resolve_remote_user_name(&identity_config, identity_multiplex.clone(), properties.uid)
        {
            properties.owner = owner;
        }
        if let Some(group) =
            resolve_remote_group_name(&identity_config, identity_multiplex, properties.gid)
        {
            properties.group = group;
        }
        Ok(properties)
    }

    pub fn update_path_attributes(
        &self,
        remote_path: impl AsRef<str>,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        self.update_remote_path_attributes(&RemoteFilePath::new(remote_path.as_ref()), update)
    }

    pub fn replace_symlink_target(
        &self,
        path: &RemoteFilePath,
        target: &str,
    ) -> anyhow::Result<()> {
        if target.is_empty() || target.contains('\0') {
            anyhow::bail!("invalid symbolic link target");
        }
        let path = path.clone();
        let target = target.to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("replace_symlink", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let raw = remote_file_path_bytes(&codec, &path)?;
            let temporary = codec.encode_path(&format!(
                "{}.nyaterm-{}",
                path.display_path,
                uuid::Uuid::new_v4()
            ))?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let metadata = session.sftp.symlink_metadata_bytes(raw.clone()).await?;
                if metadata.file_type() != russh_sftp::protocol::FileType::Symlink {
                    anyhow::bail!("path is no longer a symbolic link");
                }
                let previous = session.sftp.read_link_bytes(raw.clone()).await?;
                session
                    .sftp
                    .symlink_openssh_bytes(codec.encode_path(&target)?, temporary.clone())
                    .await?;
                let result = async {
                    if session
                        .sftp
                        .symlink_metadata_bytes(raw.clone())
                        .await?
                        .file_type()
                        != russh_sftp::protocol::FileType::Symlink
                        || session.sftp.read_link_bytes(raw.clone()).await? != previous
                    {
                        anyhow::bail!("symbolic link changed before replacement");
                    }
                    // SFTP offers no conditional rename. Never unlink the original on error.
                    session
                        .sftp
                        .posix_rename_bytes(temporary.clone(), raw)
                        .await?;
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if result.is_err() {
                    let _ = session.sftp.remove_file_bytes(temporary).await;
                }
                result
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn update_remote_path_attributes(
        &self,
        remote_path: &RemoteFilePath,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        if let Some(target) = update.symlink_target.as_deref() {
            if update.mode.is_some()
                || update.owner.is_some()
                || update.group.is_some()
                || update.recursive
            {
                anyhow::bail!("save the link target separately from target attributes");
            }
            return self.replace_symlink_target(remote_path, target);
        }
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let uid = resolve_remote_user_value(&config, multiplex.clone(), update.owner.as_deref())?;
        let gid = resolve_remote_group_value(&config, multiplex.clone(), update.group.as_deref())?;
        let mode = update.mode;
        if mode.is_none() && uid.is_none() && gid.is_none() {
            return Ok(());
        }
        self.run_operation("update_attributes", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let root = remote_file_path_bytes(&codec, &remote_path)?;
                let mut paths = vec![root.clone()];
                if update.recursive {
                    paths = collect_sftp_recursive_paths_bytes(&session.sftp, root).await?;
                }
                for path in paths {
                    session
                        .sftp
                        .set_metadata_bytes(
                            path,
                            russh_sftp::protocol::FileAttributes {
                                permissions: mode,
                                uid,
                                gid,
                                ..russh_sftp::protocol::FileAttributes::empty()
                            },
                        )
                        .await?;
                }
                Ok(())
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn read_text_file(
        &self,
        remote_path: impl AsRef<str>,
        max_bytes: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        self.read_text_file_path(&RemoteFilePath::new(remote_path.as_ref()), max_bytes)
    }

    pub fn read_text_file_path(
        &self,
        remote_path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("read_text", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let attrs = session
                    .sftp
                    .metadata_bytes(remote_path_bytes.clone())
                    .await?;
                if attrs.file_type() == russh_sftp::protocol::FileType::Dir {
                    anyhow::bail!("Directories cannot be opened as text");
                }
                let size = attrs.size.unwrap_or(0);
                if size > max_bytes {
                    anyhow::bail!(
                        "File is too large to open as text ({size} bytes > {max_bytes} bytes)"
                    );
                }
                let mut file = session.sftp.open_bytes(remote_path_bytes.clone()).await?;
                let mut bytes = Vec::with_capacity(size as usize);
                file.read_to_end(&mut bytes).await?;
                file.shutdown().await?;
                ensure_remote_text_bytes(&bytes, max_bytes)?;
                let after_attrs = session.sftp.metadata_bytes(remote_path_bytes).await?;
                let before_metadata = RemoteTextMetadata {
                    size,
                    modified_at: attrs.mtime.map(u64::from),
                };
                let after_metadata = RemoteTextMetadata {
                    size: after_attrs.size.unwrap_or(0),
                    modified_at: after_attrs.mtime.map(u64::from),
                };
                if !metadata_is_stable(&before_metadata, &after_metadata)
                    || after_metadata.size != bytes.len() as u64
                {
                    anyhow::bail!("Remote file changed while it was being read");
                }
                let content = String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("Only UTF-8 text files are supported"))?;
                Ok(SftpRemoteTextFile {
                    path: remote_path.display_path,
                    content,
                    size: after_metadata.size,
                    modified_at: after_metadata.modified_at.unwrap_or(0),
                })
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn read_text_document_path(
        &self,
        remote_path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteTextDocument> {
        let file = self.read_text_file_path(remote_path, max_bytes)?;
        let revision = RemoteTextRevision::from_bytes(
            file.content.as_bytes(),
            RemoteTextMetadata {
                size: file.size,
                modified_at: Some(file.modified_at),
            },
        );
        Ok(RemoteTextDocument {
            path: file.path,
            content: file.content,
            revision,
        })
    }

    pub fn read_file_bytes(
        &self,
        remote_path: impl AsRef<str>,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        self.read_file_bytes_path(&RemoteFilePath::new(remote_path.as_ref()), max_bytes)
    }

    pub fn read_file_bytes_path(
        &self,
        remote_path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("read_binary", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let attrs = session
                    .sftp
                    .metadata_bytes(remote_path_bytes.clone())
                    .await?;
                if attrs.file_type() == russh_sftp::protocol::FileType::Dir {
                    anyhow::bail!("Directories cannot be previewed");
                }
                let size = attrs.size.unwrap_or(0);
                if size > max_bytes {
                    anyhow::bail!(
                        "File is too large to preview ({size} bytes > {max_bytes} bytes)"
                    );
                }
                let mut file = session.sftp.open_bytes(remote_path_bytes).await?;
                let mut content_bytes = Vec::with_capacity(size as usize);
                file.read_to_end(&mut content_bytes).await?;
                file.shutdown().await?;
                Ok(RemoteBinaryFile {
                    path: remote_path.display_path,
                    size: content_bytes.len() as u64,
                    content_bytes,
                    modified_at: u64::from(attrs.mtime.unwrap_or(0)),
                })
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn write_text_file(
        &self,
        remote_path: impl AsRef<str>,
        content: impl AsRef<str>,
        expected_modified_at: Option<u64>,
        expected_size: Option<u64>,
        force: bool,
    ) -> anyhow::Result<SftpWriteTextResult> {
        self.write_text_file_path(
            &RemoteFilePath::new(remote_path.as_ref()),
            content,
            expected_modified_at,
            expected_size,
            force,
        )
    }

    pub fn write_text_file_path(
        &self,
        remote_path: &RemoteFilePath,
        content: impl AsRef<str>,
        expected_modified_at: Option<u64>,
        expected_size: Option<u64>,
        force: bool,
    ) -> anyhow::Result<SftpWriteTextResult> {
        let remote_path = remote_path.clone();
        let content = content.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("write_text", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let original_attrs =
                    match session.sftp.metadata_bytes(remote_path_bytes.clone()).await {
                        Ok(attrs) => Some(attrs),
                        Err(error) if delete::is_not_found(&error) => None,
                        Err(error) => return Err(error.into()),
                    };
                if !force {
                    let current_modified_at = original_attrs
                        .as_ref()
                        .map_or(0, |attrs| u64::from(attrs.mtime.unwrap_or(0)));
                    let current_size = original_attrs
                        .as_ref()
                        .map_or(0, |attrs| attrs.size.unwrap_or(0));
                    if expected_modified_at.is_some_and(|value| value != current_modified_at)
                        || expected_size.is_some_and(|value| value != current_size)
                    {
                        return Ok(SftpWriteTextResult::Conflict {
                            modified_at: current_modified_at,
                            size: current_size,
                        });
                    }
                }
                let mut temporary_path = remote_path_bytes.clone();
                temporary_path.extend_from_slice(
                    format!(".nyaterm-edit-{}", uuid::Uuid::new_v4().simple()).as_bytes(),
                );
                let write_result = async {
                    let mut file = session.sftp.create_bytes(temporary_path.clone()).await?;
                    file.write_all(content.as_bytes()).await?;
                    file.flush().await?;
                    file.shutdown().await?;
                    let temporary_attrs =
                        session.sftp.metadata_bytes(temporary_path.clone()).await?;
                    if temporary_attrs.size != Some(content.len() as u64) {
                        anyhow::bail!("Remote temporary file size verification failed");
                    }
                    if let Some(original_attrs) = original_attrs.as_ref() {
                        preserve_editor_metadata(&session.sftp, &temporary_path, original_attrs)
                            .await?;
                    }
                    if original_attrs.is_none() {
                        session
                            .sftp
                            .rename_bytes(temporary_path.clone(), remote_path_bytes.clone())
                            .await?;
                    } else {
                        replace_with_temporary(&session.sftp, &temporary_path, &remote_path_bytes)
                            .await?;
                    }
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if let Err(error) = write_result {
                    let _ = session.sftp.remove_file_bytes(temporary_path).await;
                    return Err(error);
                }
                let attrs = session.sftp.metadata_bytes(remote_path_bytes).await?;
                Ok(SftpWriteTextResult::Saved {
                    modified_at: u64::from(attrs.mtime.unwrap_or(0)),
                    size: attrs.size.unwrap_or(content.len() as u64),
                })
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn write_text_document_path(
        &self,
        remote_path: &RemoteFilePath,
        content: impl AsRef<str>,
        expected_revision: Option<RemoteTextRevision>,
        force: bool,
    ) -> anyhow::Result<RemoteTextWriteResult> {
        if !force && expected_revision.is_none() {
            anyhow::bail!("Remote text revision is required for a safe save");
        }
        let expected_revision = if force { None } else { expected_revision };
        let remote_path = remote_path.clone();
        let content = content.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("write_document", async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let mut temporary_path = remote_path_bytes.clone();
                temporary_path.extend_from_slice(
                    format!(".nyaterm-edit-{}", uuid::Uuid::new_v4().simple()).as_bytes(),
                );
                let operation = async {
                    let mut temporary = session
                        .sftp
                        .create_bytes(temporary_path.clone())
                        .await
                        .map_err(|error| editor_save_error("create temporary file", error))?;
                    temporary
                        .write_all(content.as_bytes())
                        .await
                        .map_err(|error| editor_save_io_error("write temporary file", error))?;
                    temporary
                        .flush()
                        .await
                        .map_err(|error| editor_save_io_error("flush temporary file", error))?;
                    temporary
                        .shutdown()
                        .await
                        .map_err(|error| editor_save_io_error("close temporary file", error))?;
                    let temporary_attrs = session
                        .sftp
                        .metadata_bytes(temporary_path.clone())
                        .await
                        .map_err(|error| editor_save_error("verify temporary file", error))?;
                    if temporary_attrs.size != Some(content.len() as u64) {
                        tracing::error!(stage = "verify temporary file", "SFTP editor size mismatch");
                        anyhow::bail!("Remote save failed during temporary file verification (size mismatch)");
                    }

                    if let Some(expected) = expected_revision.as_ref() {
                        let before_attrs = session
                            .sftp
                            .metadata_bytes(remote_path_bytes.clone())
                            .await
                            .map_err(|error| editor_save_error("check original revision", error))?;
                        let before_metadata = RemoteTextMetadata {
                            size: before_attrs.size.unwrap_or(0),
                            modified_at: Some(u64::from(before_attrs.mtime.unwrap_or(0))),
                        };
                        if before_metadata != expected.metadata {
                            return Ok(RemoteTextWriteResult::Conflict);
                        }
                        let mut current_file = session
                            .sftp
                            .open_bytes(remote_path_bytes.clone())
                            .await
                            .map_err(|error| editor_save_error("read original file", error))?;
                        let mut current = Vec::with_capacity(before_metadata.size as usize);
                        current_file
                            .read_to_end(&mut current)
                            .await
                            .map_err(|error| editor_save_io_error("read original file", error))?;
                        current_file
                            .shutdown()
                            .await
                            .map_err(|error| editor_save_io_error("close original file", error))?;
                        let after_attrs = session
                            .sftp
                            .metadata_bytes(remote_path_bytes.clone())
                            .await
                            .map_err(|error| editor_save_error("recheck original revision", error))?;
                        let after_metadata = RemoteTextMetadata {
                            size: after_attrs.size.unwrap_or(0),
                            modified_at: Some(u64::from(after_attrs.mtime.unwrap_or(0))),
                        };
                        if !metadata_is_stable(&before_metadata, &after_metadata)
                            || after_metadata.size != current.len() as u64
                        {
                            anyhow::bail!("Remote file changed while it was being verified");
                        }
                        if RemoteTextRevision::from_bytes(&current, after_metadata) != *expected {
                            return Ok(RemoteTextWriteResult::Conflict);
                        }
                    }

                    let replacement_attrs = session
                        .sftp
                        .metadata_bytes(remote_path_bytes.clone())
                        .await
                        .map_err(|error| editor_save_error("check replacement target", error))?;
                    let replacement_metadata = RemoteTextMetadata {
                        size: replacement_attrs.size.unwrap_or(0),
                        modified_at: Some(u64::from(replacement_attrs.mtime.unwrap_or(0))),
                    };
                    if expected_revision
                        .as_ref()
                        .is_some_and(|expected| replacement_metadata != expected.metadata)
                    {
                        return Ok(RemoteTextWriteResult::Conflict);
                    }
                    preserve_editor_metadata(&session.sftp, &temporary_path, &replacement_attrs)
                        .await?;
                    // SFTP has no portable conditional rename. The full revision and final
                    // metadata check above narrow the race to this replacement operation.
                    let backup_suffix =
                        format!(".nyaterm-backup-{}", uuid::Uuid::new_v4().simple());
                    let backup_path = format!("{}{backup_suffix}", remote_path.display_path);
                    let mut backup_path_bytes = remote_path_bytes.clone();
                    backup_path_bytes.extend_from_slice(backup_suffix.as_bytes());
                    let used_backup = match session
                        .sftp
                        .posix_rename_bytes(temporary_path.clone(), remote_path_bytes.clone())
                        .await
                    {
                        Ok(()) => false,
                        Err(error) if posix_rename_unsupported(&error) =>
                        {
                            session
                                .sftp
                                .rename_bytes(remote_path_bytes.clone(), backup_path_bytes.clone())
                                .await
                                .map_err(|error| editor_save_error("backup original", error))?;
                            if let Err(error) = session
                                .sftp
                                .rename_bytes(temporary_path.clone(), remote_path_bytes.clone())
                                .await
                            {
                                let restore = session
                                    .sftp
                                    .rename_bytes(
                                        backup_path_bytes.clone(),
                                        remote_path_bytes.clone(),
                                    )
                                    .await;
                                if let Err(restore_error) = restore {
                                    let restore_error = anyhow::Error::from(restore_error);
                                    tracing::error!(
                                        stage = "restore original",
                                        category = sftp_error_category(&restore_error),
                                        "SFTP editor recovery failed"
                                    );
                                    anyhow::bail!(
                                        "Remote save failed; original file is preserved at {backup_path} (automatic restore failed)"
                                    );
                                }
                                return Err(editor_save_error("replace original", error));
                            }
                            true
                        }
                        Err(error) => return Err(editor_save_error("atomic replacement", error)),
                    };
                    let verification = session
                        .sftp
                        .metadata_bytes(remote_path_bytes.clone())
                        .await
                        .map_err(|error| editor_save_error("verify replacement", error))
                        .and_then(|attrs| {
                            let metadata = RemoteTextMetadata {
                                size: attrs.size.unwrap_or(0),
                                modified_at: Some(u64::from(attrs.mtime.unwrap_or(0))),
                            };
                            if metadata.size != content.len() as u64 {
                                tracing::error!(stage = "verify replacement", "SFTP editor size mismatch");
                                anyhow::bail!("Remote save failed during replacement verification (size mismatch)");
                            }
                            Ok(metadata)
                        });
                    let metadata = match verification {
                        Ok(metadata) => metadata,
                        Err(error) if used_backup => {
                            if let Err(remove_error) = session
                                .sftp
                                .remove_file_bytes(remote_path_bytes.clone())
                                .await
                            {
                                let _ = editor_save_error("remove unverified replacement", remove_error);
                                anyhow::bail!(
                                    "Remote save verification failed; original file is preserved at {backup_path} (automatic restore failed)"
                                );
                            }
                            if let Err(restore_error) = session
                                .sftp
                                .rename_bytes(backup_path_bytes.clone(), remote_path_bytes.clone())
                                .await
                            {
                                let _ = editor_save_error("restore original", restore_error);
                                anyhow::bail!(
                                    "Remote save verification failed; original file is preserved at {backup_path} (automatic restore failed)"
                                );
                            }
                            return Err(error);
                        }
                        Err(error) => return Err(error),
                    };
                    let revision = RemoteTextRevision::from_bytes(content.as_bytes(), metadata);
                    if used_backup
                        && let Err(error) = session.sftp.remove_file_bytes(backup_path_bytes).await
                    {
                        let error = anyhow::Error::from(error);
                        tracing::warn!(
                            stage = "cleanup backup",
                            category = sftp_error_category(&error),
                            "SFTP editor backup cleanup failed"
                        );
                        return Ok(RemoteTextWriteResult::SavedWithBackup {
                            revision,
                            backup_path,
                        });
                    }
                    Ok(RemoteTextWriteResult::Saved { revision })
                }
                .await;
                if !matches!(
                    &operation,
                    Ok(RemoteTextWriteResult::Saved { .. } | RemoteTextWriteResult::SavedWithBackup { .. })
                ) {
                    let _ = session.sftp.remove_file_bytes(temporary_path).await;
                }
                operation
            }
            .await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn download_file(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
    ) -> anyhow::Result<SftpTransferSummary> {
        self.download_file_with_progress(remote_path, local_path, |_| {})
    }

    pub fn download_file_with_progress<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_file_with_progress_and_control(
            remote_path,
            local_path,
            SftpTransferControl::default(),
            progress,
        )
    }

    pub fn download_file_with_progress_and_control<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_file_with_progress_and_control_options(
            remote_path,
            local_path,
            control,
            SftpTransferOptions::default(),
            progress,
        )
    }

    pub fn download_file_with_progress_and_control_options<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_remote_file_with_progress_and_control_options(
            &RemoteFilePath::new(remote_path.as_ref()),
            local_path,
            control,
            options,
            progress,
        )
    }

    pub fn download_remote_file_with_progress_and_control_options<F>(
        &self,
        remote_path: &RemoteFilePath,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let options = self.effective_transfer_options(options);
        let remote_path = remote_path.clone();
        let local_path = local_path.into();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation("download_file", async move {
            let mut last_error = None;
            let download_root = crate::download_path::root_for_target(&local_path);
            for _attempt in 0..=options.max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let codec = SftpPathCodec::from_ssh_config(&config)?;
                    let parameters = TransferParameters::new(&options, &config.sftp);
                    let session = await_sftp_request(
                        &control,
                        open_sftp_session_with_client_config(
                            &config,
                            multiplex.as_ref(),
                            parameters.client_config(),
                        ),
                    )
                    .await?;
                    let bytes = download_remote_file_bytes(
                        &session.sftp,
                        remote_file_path_bytes(&codec, &remote_path)?,
                        &remote_path.display_path,
                        &local_path,
                        &control,
                        &options,
                        &mut progress,
                        download_root,
                        None,
                        parameters,
                    )
                    .await;
                    close_sftp_session(session).await;
                    let bytes = bytes?;
                    Ok(SftpTransferSummary {
                        remote_path: remote_path.display_path.clone(),
                        local_path: local_path.clone(),
                        bytes,
                        skipped: false,
                    })
                }
                .await;
                match result {
                    Ok(summary) => return Ok(summary),
                    Err(error) if is_sftp_transfer_cancelled(&error) => return Err(error),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_sftp_retry_error(last_error))
        })
    }

    pub fn download_path_with_progress<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_path_with_progress_and_control(
            remote_path,
            local_path,
            SftpTransferControl::default(),
            progress,
        )
    }

    pub fn download_path_with_progress_and_control<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_path_with_progress_options(
            remote_path,
            local_path,
            control,
            SftpDuplicatePolicy::Overwrite,
            progress,
        )
    }

    pub fn download_path_with_progress_options<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        duplicate_policy: SftpDuplicatePolicy,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_path_with_progress_options_and_resolver(
            remote_path,
            local_path,
            control,
            duplicate_policy,
            None,
            progress,
        )
    }

    pub fn download_path_with_progress_options_and_resolver<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        duplicate_policy: SftpDuplicatePolicy,
        duplicate_resolver: Option<Arc<dyn SftpDuplicateResolver>>,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_path_with_progress_and_path_options(
            remote_path,
            local_path,
            control,
            SftpPathTransferOptions::new(
                duplicate_policy,
                duplicate_resolver,
                SftpTransferOptions::default(),
            ),
            progress,
        )
    }

    pub fn download_path_with_progress_and_path_options<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        path_options: SftpPathTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new(remote_path.as_ref()),
            local_path,
            control,
            path_options,
            progress,
        )
    }

    pub fn download_remote_path_with_progress_and_path_options<F>(
        &self,
        remote_path: &RemoteFilePath,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        path_options: SftpPathTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let path_options = self.effective_path_options(path_options);
        let remote_path = remote_path.clone();
        let local_path = local_path.into();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_controlled_operation(
            control.clone(),
            path_options.is_promised_download(),
            async move {
                let mut last_error = None;
                let download_root = crate::download_path::root_for_target(&local_path);
                for _attempt in 0..=path_options.transfer_options().max_retries() {
                    control.check_cancelled()?;
                    let result = async {
                        let codec = SftpPathCodec::from_ssh_config(&config)?;
                        let session = await_sftp_request(
                            &control,
                            open_sftp_session_with_client_config(
                                &config,
                                multiplex.as_ref(),
                                TransferParameters::new(
                                    path_options.transfer_options(),
                                    &config.sftp,
                                )
                                .client_config(),
                            ),
                        )
                        .await?;
                        let result = async {
                            let sftp = &session.sftp;
                            control.wait_if_paused().await?;
                            let raw_path = remote_file_path_bytes(&codec, &remote_path)?;
                            let source_metadata = await_sftp_request(
                                &control,
                                sftp.symlink_metadata_bytes(raw_path.clone()),
                            )
                            .await?;
                            ensure_remote_download_source(source_metadata.file_type())?;
                            if let Some(expected) = path_options.expected_source_type() {
                                anyhow::ensure!(
                                    attrs_to_sftp_file_type(&source_metadata) == expected,
                                    "remote drag source changed type"
                                );
                            }
                            let is_directory =
                                source_metadata.file_type() == russh_sftp::protocol::FileType::Dir;
                            let Some(local_target) =
                                resolve_local_download_target(SftpLocalDownloadTargetContext {
                                    remote_path: &remote_path.display_path,
                                    remote_path_raw: &raw_path,
                                    local_path: &local_path,
                                    is_directory,
                                    path_options: &path_options,
                                })?
                            else {
                                return Ok(SftpTransferSummary {
                                    remote_path: remote_path.display_path.clone(),
                                    local_path: local_path.clone(),
                                    bytes: 0,
                                    skipped: true,
                                });
                            };
                            let bytes = if is_directory {
                                let contents_options = path_options.for_directory_contents();
                                download_remote_directory_bytes(
                                    &session,
                                    &codec,
                                    raw_path,
                                    &remote_path.display_path,
                                    &local_target,
                                    &control,
                                    &contents_options,
                                    &mut progress,
                                    download_root,
                                    &config,
                                    multiplex.as_ref(),
                                )
                                .await?
                            } else {
                                download_remote_file_bytes(
                                    sftp,
                                    raw_path,
                                    &remote_path.display_path,
                                    &local_target,
                                    &control,
                                    path_options.transfer_options(),
                                    &mut progress,
                                    download_root,
                                    Some(&path_options),
                                    TransferParameters::new(
                                        path_options.transfer_options(),
                                        &config.sftp,
                                    ),
                                )
                                .await?
                            };
                            Ok(SftpTransferSummary {
                                remote_path: remote_path.display_path.clone(),
                                local_path: local_target,
                                bytes,
                                skipped: false,
                            })
                        }
                        .await;
                        let _ = tokio::time::timeout(
                            Duration::from_secs(5),
                            close_sftp_session(session),
                        )
                        .await;
                        result
                    }
                    .await;
                    match result {
                        Ok(summary) => return Ok(summary),
                        Err(error) if is_sftp_transfer_cancelled(&error) => return Err(error),
                        Err(error) => last_error = Some(error),
                    }
                }
                Err(last_sftp_retry_error(last_error))
            },
        )
    }

    pub fn upload_file(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
    ) -> anyhow::Result<SftpTransferSummary> {
        self.upload_file_with_progress(local_path, remote_path, |_| {})
    }

    pub fn upload_file_with_progress<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.upload_file_with_progress_and_control(
            local_path,
            remote_path,
            SftpTransferControl::default(),
            progress,
        )
    }

    pub fn upload_file_with_progress_and_control<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.upload_file_with_progress_and_control_options(
            local_path,
            remote_path,
            control,
            SftpTransferOptions::default(),
            progress,
        )
    }

    pub fn upload_file_with_progress_and_control_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let options = self.effective_transfer_options(options);
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let attempts = options.max_retries().saturating_add(1);
        self.run_upload_operation("upload_file", attempts, async move {
            let upload_metadata = UploadMetadata::default();
            let remote_path = resolve_remote_upload_target(&local_path, &remote_path)?;
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let raw_path = codec.encode_path(&remote_path)?;
            let mut last_error = None;
            for _attempt in 0..=options.max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let session = open_sftp_session_with_client_config(
                        &config,
                        multiplex.as_ref(),
                        TransferParameters::new(&options, &config.sftp).client_config(),
                    )
                    .await?;
                    let bytes = upload_local_file_bytes(
                        &session.sftp,
                        raw_path.clone(),
                        &local_path,
                        &remote_path,
                        &control,
                        &options,
                        &upload_metadata,
                        &mut progress,
                    )
                    .await;
                    close_sftp_session(session).await;
                    let bytes = bytes?;
                    Ok(SftpTransferSummary {
                        remote_path: remote_path.clone(),
                        local_path: local_path.clone(),
                        bytes,
                        skipped: false,
                    })
                }
                .await;
                match result {
                    Ok(summary) => return Ok(summary),
                    Err(error) if is_sftp_transfer_cancelled(&error) => return Err(error),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_sftp_retry_error(last_error))
        })
    }

    pub fn upload_remote_file_with_progress_and_control_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: &RemoteFilePath,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let options = self.effective_transfer_options(options);
        let local_path = local_path.into();
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let attempts = options.max_retries().saturating_add(1);
        self.run_upload_operation("upload_remote_file", attempts, async move {
            let upload_metadata = UploadMetadata::default();
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let raw_path = remote_file_path_bytes(&codec, &remote_path)?;
            let mut last_error = None;
            for _attempt in 0..=options.max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let session = open_sftp_session_with_client_config(
                        &config,
                        multiplex.as_ref(),
                        TransferParameters::new(&options, &config.sftp).client_config(),
                    )
                    .await?;
                    let bytes = upload_local_file_bytes(
                        &session.sftp,
                        raw_path.clone(),
                        &local_path,
                        &remote_path.display_path,
                        &control,
                        &options,
                        &upload_metadata,
                        &mut progress,
                    )
                    .await;
                    close_sftp_session(session).await;
                    let bytes = bytes?;
                    Ok(SftpTransferSummary {
                        remote_path: remote_path.display_path.clone(),
                        local_path: local_path.clone(),
                        bytes,
                        skipped: false,
                    })
                }
                .await;
                match result {
                    Ok(summary) => return Ok(summary),
                    Err(error) if is_sftp_transfer_cancelled(&error) => return Err(error),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_sftp_retry_error(last_error))
        })
    }

    pub fn upload_path_with_progress<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.upload_path_with_progress_and_control(
            local_path,
            remote_path,
            SftpTransferControl::default(),
            progress,
        )
    }

    pub fn upload_path_with_progress_and_control<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.upload_path_with_progress_options(
            local_path,
            remote_path,
            control,
            SftpDuplicatePolicy::Overwrite,
            progress,
        )
    }

    pub fn upload_path_with_progress_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        duplicate_policy: SftpDuplicatePolicy,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.upload_path_with_progress_options_and_resolver(
            local_path,
            remote_path,
            control,
            duplicate_policy,
            None,
            progress,
        )
    }

    pub fn upload_path_with_progress_options_and_resolver<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        duplicate_policy: SftpDuplicatePolicy,
        duplicate_resolver: Option<Arc<dyn SftpDuplicateResolver>>,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.upload_path_with_progress_and_path_options(
            local_path,
            remote_path,
            control,
            SftpPathTransferOptions::new(
                duplicate_policy,
                duplicate_resolver,
                SftpTransferOptions::default(),
            ),
            progress,
        )
    }

    pub fn upload_path_with_progress_and_path_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        path_options: SftpPathTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let path_options = self.effective_path_options(path_options);
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let attempts = path_options
            .transfer_options()
            .max_retries()
            .saturating_add(1);
        self.run_upload_operation("upload_path", attempts, async move {
            let upload_metadata = UploadMetadata::default();
            let metadata = tokio::fs::metadata(&local_path).await?;
            let remote_path = resolve_remote_upload_target(&local_path, &remote_path)?;
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            codec.encode_path(&remote_path)?;
            let mut last_error = None;
            for _attempt in 0..=path_options.transfer_options().max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let session = open_sftp_session_with_client_config(
                        &config,
                        multiplex.as_ref(),
                        TransferParameters::new(path_options.transfer_options(), &config.sftp)
                            .client_config(),
                    )
                    .await?;
                    let sftp = Arc::clone(&session.sftp);
                    control.wait_if_paused().await?;
                    let Some(remote_target) =
                        resolve_remote_write_target(SftpRemoteWriteTargetContext {
                            sftp: &sftp,
                            codec: &codec,
                            local_path: &local_path,
                            remote_path: &remote_path,
                            is_directory: metadata.is_dir(),
                            path_options: &path_options,
                        })
                        .await?
                    else {
                        close_sftp_session(session).await;
                        return Ok(SftpTransferSummary {
                            remote_path: remote_path.clone(),
                            local_path: local_path.clone(),
                            bytes: 0,
                            skipped: true,
                        });
                    };
                    let bytes = if metadata.is_dir() {
                        let contents_options = path_options.for_directory_contents();
                        upload_local_directory(
                            Arc::clone(&sftp),
                            &local_path,
                            &remote_target,
                            LocalDirectoryUploadContext {
                                codec: &codec,
                                config: &config,
                                multiplex: multiplex.as_ref(),
                                control: &control,
                                path_options: &contents_options,
                                upload_metadata: &upload_metadata,
                            },
                            &mut progress,
                        )
                        .await?
                    } else {
                        upload_local_file(
                            &sftp,
                            &codec,
                            &local_path,
                            &remote_target,
                            &control,
                            path_options.transfer_options(),
                            &upload_metadata,
                            &mut progress,
                        )
                        .await?
                    };
                    close_sftp_session(session).await;
                    Ok(SftpTransferSummary {
                        remote_path: remote_target,
                        local_path: local_path.clone(),
                        bytes,
                        skipped: false,
                    })
                }
                .await;
                match result {
                    Ok(summary) => return Ok(summary),
                    Err(error) if is_sftp_transfer_cancelled(&error) => return Err(error),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_sftp_retry_error(last_error))
        })
    }
}

async fn collect_sftp_recursive_paths_bytes(
    sftp: &SftpSession,
    remote_path: Vec<u8>,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut paths = Vec::new();
    let mut stack = vec![remote_path];
    while let Some(path) = stack.pop() {
        let metadata = sftp.symlink_metadata_bytes(path.clone()).await?;
        let is_directory = metadata.file_type() == russh_sftp::protocol::FileType::Dir;
        paths.push(path.clone());
        if is_directory {
            for entry in sftp.read_dir_bytes(path).await? {
                if matches!(entry.file_name_bytes(), b"." | b"..") {
                    continue;
                }
                stack.push(entry.path_bytes());
            }
        }
    }
    Ok(paths)
}

async fn copy_remote_path_between_sftp(
    source: &SftpSession,
    source_path: Vec<u8>,
    destination: &SftpSession,
    destination_path: Vec<u8>,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &CopyProgress,
) -> anyhow::Result<u64> {
    control.wait_if_paused().await?;
    let source_attrs = source.metadata_bytes(source_path.clone()).await?;
    if source_attrs.file_type() != russh_sftp::protocol::FileType::Dir {
        return copy_remote_file_between_sftp(
            source,
            source_path,
            destination,
            destination_path,
            control,
            options,
            progress,
        )
        .await;
    }

    ensure_sftp_directory_bytes(destination, destination_path.clone()).await?;
    let mut pending = vec![(source_path, destination_path)];
    let mut completed_directories = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some((source_dir, destination_dir)) = pending.pop() {
        control.wait_if_paused().await?;
        let source_dir_attrs = source.metadata_bytes(source_dir.clone()).await?;
        completed_directories.push((destination_dir.clone(), source_dir_attrs));
        for entry in source.read_dir_bytes(source_dir).await? {
            control.wait_if_paused().await?;
            if matches!(entry.file_name_bytes(), b"." | b"..") {
                continue;
            }
            let source_child = entry.path_bytes();
            let destination_child = remote_join_bytes(&destination_dir, entry.file_name_bytes());
            if entry.file_type() == russh_sftp::protocol::FileType::Dir {
                ensure_sftp_directory_bytes(destination, destination_child.clone()).await?;
                pending.push((source_child, destination_child));
            } else {
                total_bytes = total_bytes.saturating_add(
                    copy_remote_file_between_sftp(
                        source,
                        source_child,
                        destination,
                        destination_child,
                        control,
                        options,
                        progress,
                    )
                    .await?,
                );
            }
        }
    }

    for (path, source_attrs) in completed_directories.into_iter().rev() {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: source_attrs.permissions,
            mtime: if options.preserve_timestamps {
                source_attrs.mtime
            } else {
                None
            },
            ..russh_sftp::protocol::FileAttributes::empty()
        };
        destination.set_metadata_bytes(path, attrs).await?;
        progress.finish_item();
    }
    Ok(total_bytes)
}

async fn ensure_sftp_directory_bytes(sftp: &SftpSession, path: Vec<u8>) -> anyhow::Result<()> {
    if !sftp.try_exists_bytes(path.clone()).await? {
        sftp.create_dir_bytes(path).await?;
        return Ok(());
    }
    if sftp.metadata_bytes(path).await?.file_type() != russh_sftp::protocol::FileType::Dir {
        anyhow::bail!("remote copy destination exists and is not a directory");
    }
    Ok(())
}

async fn copy_remote_file_between_sftp(
    source: &SftpSession,
    source_path: Vec<u8>,
    destination: &SftpSession,
    destination_path: Vec<u8>,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &CopyProgress,
) -> anyhow::Result<u64> {
    control.wait_if_paused().await?;
    let source_attrs = source.metadata_bytes(source_path.clone()).await?;
    let mut source_file = source.open_bytes(source_path).await?;
    let mut temporary_path = destination_path.clone();
    temporary_path
        .extend_from_slice(format!(".nyaterm-copy-{}", uuid::Uuid::new_v4().simple()).as_bytes());
    let copy_result = async {
        let mut destination_file = destination.create_bytes(temporary_path.clone()).await?;
        let mut buffer = vec![0_u8; options.buffer_size_bytes()];
        let mut bytes = 0_u64;
        loop {
            control.wait_if_paused().await?;
            let read = source_file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            destination_file.write_all(&buffer[..read]).await?;
            bytes = bytes.saturating_add(read as u64);
            progress.add_bytes(read as u64);
        }
        destination_file.flush().await?;
        destination_file.shutdown().await?;
        source_file.shutdown().await?;
        let temporary_attrs = destination.metadata_bytes(temporary_path.clone()).await?;
        if temporary_attrs.size != Some(bytes) {
            anyhow::bail!("remote copy temporary file size verification failed");
        }
        destination
            .set_metadata_bytes(
                temporary_path.clone(),
                russh_sftp::protocol::FileAttributes {
                    permissions: options.default_file_mode.or(source_attrs.permissions),
                    mtime: if options.preserve_timestamps {
                        source_attrs.mtime
                    } else {
                        None
                    },
                    ..russh_sftp::protocol::FileAttributes::empty()
                },
            )
            .await?;
        destination
            .rename_bytes(temporary_path.clone(), destination_path)
            .await?;
        progress.finish_item();
        Ok::<u64, anyhow::Error>(bytes)
    }
    .await;
    if copy_result.is_err() {
        let _ = destination.remove_file_bytes(temporary_path).await;
    }
    copy_result
}

fn remote_join_bytes(parent: &[u8], child: &[u8]) -> Vec<u8> {
    let mut joined = parent.to_vec();
    if !joined.ends_with(b"/") {
        joined.push(b'/');
    }
    joined.extend_from_slice(child);
    joined
}

fn is_sftp_transfer_cancelled(error: &anyhow::Error) -> bool {
    error.to_string().contains(SFTP_TRANSFER_CANCELLED)
}

/// Server status strings can contain user data; diagnostics record categories, never error text.
fn log_sftp_upload_failure(operation: &'static str, attempts: u32, error: &anyhow::Error) {
    tracing::error!(
        operation,
        attempts,
        stage = "completion",
        category = sftp_error_category(error),
        "SFTP upload failed"
    );
}

/// Replaces an existing file with a fully written temporary file. Plain SFTP v3 rename refuses
/// to overwrite (OpenSSH answers `Failure`), so this prefers `posix-rename@openssh.com` and
/// otherwise moves the original aside, restoring it if the replacement fails.
async fn replace_with_temporary(
    session: &SftpSession,
    temporary_path: &[u8],
    target_path: &[u8],
) -> anyhow::Result<()> {
    match session
        .posix_rename_bytes(temporary_path.to_vec(), target_path.to_vec())
        .await
    {
        Ok(()) => return Ok(()),
        Err(error) if posix_rename_unsupported(&error) => {}
        Err(error) => return Err(error.into()),
    }
    let mut backup_path = target_path.to_vec();
    backup_path
        .extend_from_slice(format!(".nyaterm-backup-{}", uuid::Uuid::new_v4().simple()).as_bytes());
    session
        .rename_bytes(target_path.to_vec(), backup_path.clone())
        .await?;
    if let Err(error) = session
        .rename_bytes(temporary_path.to_vec(), target_path.to_vec())
        .await
    {
        session
            .rename_bytes(backup_path, target_path.to_vec())
            .await
            .map_err(|_| {
                anyhow::anyhow!("Remote write failed and the original file could not be restored")
            })?;
        return Err(error.into());
    }
    let _ = session.remove_file_bytes(backup_path).await;
    Ok(())
}

async fn preserve_editor_metadata(
    session: &SftpSession,
    temporary_path: &[u8],
    original: &russh_sftp::protocol::FileAttributes,
) -> anyhow::Result<()> {
    use russh_sftp::protocol::FileAttributes;

    let (Some(uid), Some(gid), Some(permissions)) =
        (original.uid, original.gid, original.permissions)
    else {
        anyhow::bail!("Remote save refused: original ownership or permissions are unavailable");
    };
    let temporary = session
        .metadata_bytes(temporary_path.to_vec())
        .await
        .map_err(|error| editor_save_error("check temporary ownership", error))?;
    // Avoid chown for an ordinary user's own file. Some servers reject even
    // a no-op ownership change; a real change must succeed before replacement.
    if temporary.uid != Some(uid) || temporary.gid != Some(gid) {
        session
            .set_metadata_bytes(
                temporary_path.to_vec(),
                FileAttributes {
                    uid: Some(uid),
                    gid: Some(gid),
                    ..FileAttributes::empty()
                },
            )
            .await
            .map_err(|error| editor_save_error("preserve original ownership", error))?;
    }
    // chown can clear setuid/setgid, so restore the mode afterwards.
    session
        .set_metadata_bytes(
            temporary_path.to_vec(),
            FileAttributes {
                permissions: Some(permissions & 0o7777),
                ..FileAttributes::empty()
            },
        )
        .await
        .map_err(|error| editor_save_error("copy original permissions", error))?;
    let verified = session
        .metadata_bytes(temporary_path.to_vec())
        .await
        .map_err(|error| editor_save_error("verify temporary attributes", error))?;
    if verified.uid != Some(uid)
        || verified.gid != Some(gid)
        || verified.permissions.map(|mode| mode & 0o7777) != Some(permissions & 0o7777)
    {
        anyhow::bail!(
            "Remote save refused: temporary ownership or permissions verification failed"
        );
    }
    Ok(())
}

fn editor_save_error(
    stage: &'static str,
    error: russh_sftp::client::error::Error,
) -> anyhow::Error {
    let error = anyhow::Error::from(error);
    let category = sftp_error_category(&error);
    tracing::error!(stage, category, "SFTP editor save failed");
    error.context(format!("Remote save failed during {stage} ({category})"))
}

fn posix_rename_unsupported(error: &russh_sftp::client::error::Error) -> bool {
    match error {
        russh_sftp::client::error::Error::UnexpectedBehavior(message) => {
            message == "server does not support atomic POSIX rename"
        }
        russh_sftp::client::error::Error::Status(status) => {
            status.status_code == russh_sftp::protocol::StatusCode::OpUnsupported
        }
        _ => false,
    }
}

fn editor_save_io_error(stage: &'static str, error: std::io::Error) -> anyhow::Error {
    let kind = error.kind();
    tracing::error!(stage, ?kind, "SFTP editor I/O failed");
    anyhow::Error::from(error).context(format!("Remote save failed during {stage} ({kind:?})"))
}

fn sftp_error_category(error: &anyhow::Error) -> &'static str {
    if is_sftp_transfer_cancelled(error) {
        "cancelled"
    } else if sftp_error_is_stream_closed(error) {
        "stream_closed"
    } else if sftp_error_is_timeout(error) {
        "timeout"
    } else {
        "operation_failed"
    }
}

fn sftp_error_is_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<russh_sftp::client::error::Error>(),
            Some(russh_sftp::client::error::Error::Timeout)
        ) || cause
            .downcast_ref::<tokio::time::error::Elapsed>()
            .is_some()
    })
}

fn sftp_error_invalidates_compatibility_session(error: &anyhow::Error) -> bool {
    sftp_error_is_stream_closed(error) || sftp_error_is_timeout(error)
}

fn sftp_error_is_stream_closed(error: &anyhow::Error) -> bool {
    error.chain().any(
        |cause| match cause.downcast_ref::<russh_sftp::client::error::Error>() {
            Some(russh_sftp::client::error::Error::UnexpectedBehavior(message)) => {
                message == "Stream closed"
                    || message == "SFTP stream closed"
                    || message == "session closed"
                    || message == "sender dropped"
                    || message.starts_with("SendError:")
                    || message.starts_with("RecvError:")
            }
            Some(russh_sftp::client::error::Error::Status(status)) => matches!(
                status.status_code,
                russh_sftp::protocol::StatusCode::NoConnection
                    | russh_sftp::protocol::StatusCode::ConnectionLost
            ),
            _ => matches!(
                cause
                    .downcast_ref::<std::io::Error>()
                    .map(std::io::Error::kind),
                Some(
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                )
            ),
        },
    )
}

#[cfg(test)]
mod compatibility_tests;
#[cfg(test)]
mod copy_tests;
#[cfg(test)]
mod editor_save_tests;
#[cfg(test)]
mod filename_tests;
#[cfg(test)]
mod overwrite_tests;
#[cfg(test)]
mod upload_metadata_tests;

fn last_sftp_retry_error(last_error: Option<anyhow::Error>) -> anyhow::Error {
    last_error.unwrap_or_else(|| anyhow::anyhow!("SFTP transfer failed before starting"))
}

async fn preserve_remote_modified_time_bytes(
    sftp: &SftpSession,
    remote_path: Vec<u8>,
    local_metadata: Option<std::fs::Metadata>,
) {
    let Some(local_metadata) = local_metadata else {
        return;
    };
    let Some(mtime) = local_metadata.modified().ok().and_then(sftp_timestamp_secs) else {
        return;
    };
    let atime = local_metadata
        .accessed()
        .ok()
        .and_then(sftp_timestamp_secs)
        .unwrap_or(mtime);
    let attrs = russh_sftp::protocol::FileAttributes {
        atime: Some(atime),
        mtime: Some(mtime),
        ..russh_sftp::protocol::FileAttributes::empty()
    };
    let _ = sftp.set_metadata_bytes(remote_path, attrs).await;
}

fn sftp_timestamp_secs(time: SystemTime) -> Option<u32> {
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(seconds.min(u64::from(u32::MAX)) as u32)
}

fn transfer_resume_offset(
    local_path: &Path,
    total_bytes: Option<u64>,
    options: &SftpTransferOptions,
) -> u64 {
    if !options.resume_broken_transfer {
        return 0;
    }
    let Some(total_bytes) = total_bytes.filter(|total| *total > 0) else {
        return 0;
    };
    let Ok(metadata) = std::fs::metadata(local_path) else {
        return 0;
    };
    if !metadata.is_file() {
        return 0;
    }
    let local_size = metadata.len();
    if local_size > 0 && local_size < total_bytes {
        local_size
    } else {
        0
    }
}

fn ensure_remote_download_source(file_type: russh_sftp::protocol::FileType) -> anyhow::Result<()> {
    match file_type {
        russh_sftp::protocol::FileType::File | russh_sftp::protocol::FileType::Dir => Ok(()),
        russh_sftp::protocol::FileType::Symlink => {
            anyhow::bail!("symbolic links are not supported in downloads")
        }
        russh_sftp::protocol::FileType::Other => {
            anyhow::bail!("remote source is not a regular file or directory")
        }
    }
}

/// Pause and cancellation apply to metadata as well as content requests.
async fn await_sftp_request<T, E: Into<anyhow::Error>>(
    control: &SftpTransferControl,
    operation: impl Future<Output = Result<T, E>>,
) -> anyhow::Result<T> {
    control.wait_if_paused().await?;
    control
        .until_cancelled(tokio::time::timeout(Duration::from_secs(60), operation))
        .await?
        .map_err(|error| anyhow::Error::new(error).context("SFTP request stalled"))?
        .map_err(Into::into)
}

/// Shared bounded range read for destination downloads and native consumers.
async fn read_transfer_range(
    remote: &russh_sftp::client::fs::File,
    control: &SftpTransferControl,
    offset: u64,
    length: usize,
) -> anyhow::Result<Vec<u8>> {
    control.wait_if_paused().await?;
    control
        .until_cancelled(tokio::time::timeout(
            Duration::from_secs(60),
            remote.read_at(offset, length),
        ))
        .await?
        .map_err(|error| anyhow::Error::new(error).context("SFTP download stalled"))?
        .map_err(anyhow::Error::from)
}

#[expect(clippy::too_many_arguments)]
async fn download_remote_file_bytes<F>(
    sftp: &SftpSession,
    raw_path: Vec<u8>,
    remote_path: &str,
    local_path: &Path,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &mut F,
    download_root: &Path,
    path_options: Option<&SftpPathTransferOptions>,
    parameters: TransferParameters,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    let total_started = Instant::now();
    control.wait_if_paused().await?;
    let promised = path_options.is_some_and(SftpPathTransferOptions::is_promised_download);
    let access = if let Some(path_options) = path_options {
        path_options
            .download_runtime()?
            .access(local_path, promised)?
    } else {
        crate::download_path::DownloadTargetAccess::capture(download_root, local_path)?
    };
    let source_metadata =
        await_sftp_request(control, sftp.symlink_metadata_bytes(raw_path.clone())).await?;
    ensure_remote_download_source(source_metadata.file_type())?;
    anyhow::ensure!(
        source_metadata.file_type() == russh_sftp::protocol::FileType::File,
        "download source is not a regular file"
    );
    if promised
        && let Some(path_options) = path_options
        && path_options.download_runtime()?.reusable(
            local_path,
            &raw_path,
            source_metadata.size,
            source_metadata.mtime,
        )?
    {
        let bytes = source_metadata.size.unwrap_or(0);
        progress(SftpTransferProgress {
            remote_path: remote_path.to_string(),
            local_path: local_path.to_path_buf(),
            bytes_transferred: bytes,
            total_bytes: Some(bytes),
            item_count_completed: None,
            item_count_total: None,
        });
        return Ok(bytes);
    }
    if let Some(parent) = local_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    crate::download_path::writable_file_permissions(download_root, local_path)?;
    let mut remote = await_sftp_request(control, sftp.open_bytes(raw_path.clone())).await?;
    let outcome = async {
        let remote_metadata = await_sftp_request(control, remote.metadata()).await?;
        anyhow::ensure!(
            remote_metadata.file_type() == russh_sftp::protocol::FileType::File,
            "opened download source is not a regular file"
        );
        let total_bytes = remote_metadata.size;
        let resume_offset = if promised {
            0
        } else {
            transfer_resume_offset(local_path, total_bytes, options)
        };
        let (temporary, mut local) = crate::download_path::TransferDownload::prepare_with_access(
            download_root,
            local_path,
            resume_offset,
            access,
        )
        .await?;
        let transfer_started = Instant::now();
        let result = async {
            let mut bytes = resume_offset;
            progress(SftpTransferProgress {
                remote_path: remote_path.to_string(),
                local_path: local_path.to_path_buf(),
                bytes_transferred: bytes,
                total_bytes,
                item_count_completed: None,
                item_count_total: None,
            });
            if let Some(total) = total_bytes {
                use futures::{StreamExt as _, TryStreamExt as _};
                let chunk_size = parameters.payload as u64;
                let remote_ref = &remote;
                let mut chunks =
                    futures::stream::iter((resume_offset..total).step_by(chunk_size as usize))
                        .map(|offset| async move {
                            let length = (total - offset).min(chunk_size) as usize;
                            let mut data = Vec::with_capacity(length);
                            while data.len() < length {
                                control.wait_if_paused().await?;
                                let read = read_transfer_range(
                                    remote_ref,
                                    control,
                                    offset + data.len() as u64,
                                    length - data.len(),
                                )
                                .await?;
                                if read.is_empty() {
                                    anyhow::bail!("remote file ended before its advertised size");
                                }
                                data.extend_from_slice(&read);
                            }
                            Ok::<_, anyhow::Error>(data)
                        })
                        .buffered(parameters.reads);
                // Commit in offset order: a cancelled transfer leaves a valid contiguous prefix
                // that the existing resume contract can safely use on the next attempt.
                while let Some(data) = chunks.try_next().await? {
                    control.wait_if_paused().await?;
                    local.write_all(&data).await?;
                    bytes += data.len() as u64;
                    progress(SftpTransferProgress {
                        remote_path: remote_path.to_string(),
                        local_path: local_path.to_path_buf(),
                        bytes_transferred: bytes,
                        total_bytes,
                        item_count_completed: None,
                        item_count_total: None,
                    });
                }
            } else {
                let mut buffer = vec![0_u8; parameters.payload];
                loop {
                    control.wait_if_paused().await?;
                    let read = await_sftp_request(control, remote.read(&mut buffer)).await?;
                    if read == 0 {
                        break;
                    }
                    local.write_all(&buffer[..read]).await?;
                    bytes += read as u64;
                    progress(SftpTransferProgress {
                        remote_path: remote_path.to_string(),
                        local_path: local_path.to_path_buf(),
                        bytes_transferred: bytes,
                        total_bytes,
                        item_count_completed: None,
                        item_count_total: None,
                    });
                }
            }
            local.flush().await?;
            await_sftp_request(control, remote.shutdown()).await?;
            Ok::<_, anyhow::Error>(bytes)
        }
        .await;
        // Finish local writes even on error before committing a valid resume prefix or cleaning up.
        let flushed = local.flush().await;
        let local = local.into_std().await;
        if result.is_ok()
            && options.preserve_timestamps
            && let Some(mtime) = remote_metadata.mtime.filter(|mtime| *mtime > 0)
        {
            let _ = local.set_modified(UNIX_EPOCH + Duration::from_secs(u64::from(mtime)));
        }
        drop(local);
        let transfer_us = transfer_started.elapsed().as_micros() as u64;
        let commit_started = Instant::now();
        let (bytes, identity) = match result {
            Ok(bytes) => {
                flushed?;
                control.check_cancelled()?;
                if promised && let Some(path_options) = path_options {
                    path_options
                        .download_runtime()?
                        .validate_ancestors(local_path)?;
                }
                let identity = temporary.commit_async(download_root, local_path).await?;
                (bytes, identity)
            }
            Err(error) => {
                if !promised && options.resume_broken_transfer && flushed.is_ok() {
                    let identity = temporary.commit_async(download_root, local_path).await?;
                    if let Some(path_options) = path_options {
                        path_options.download_runtime()?.completed(
                            local_path,
                            identity,
                            raw_path.clone(),
                            None,
                            None,
                        )?;
                    }
                }
                return Err(error);
            }
        };
        tracing::debug!(
            bytes,
            request_bytes = parameters.payload,
            reads = parameters.reads,
            transfer_us,
            commit_us = commit_started.elapsed().as_micros() as u64,
            total_us = total_started.elapsed().as_micros() as u64,
            "SFTP file download timings"
        );
        if let Some(path_options) = path_options {
            path_options.download_runtime()?.completed(
                local_path,
                identity,
                raw_path,
                remote_metadata.size,
                remote_metadata.mtime,
            )?;
        }
        Ok(bytes)
    }
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), remote.shutdown()).await;
    outcome
}

#[expect(clippy::too_many_arguments)]
async fn download_remote_directory_bytes<F>(
    session: &OpenSftpSession,
    codec: &SftpPathCodec,
    raw_path: Vec<u8>,
    remote_path: &str,
    local_path: &Path,
    control: &SftpTransferControl,
    path_options: &SftpPathTransferOptions,
    progress: &mut F,
    download_root: &Path,
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    let total_started = Instant::now();
    let sftp = &session.sftp;
    control.wait_if_paused().await?;
    crate::download_path::ensure_no_symlink(download_root, local_path)?;
    path_options.download_runtime()?.ensure_directory(
        download_root,
        local_path,
        path_options.is_promised_download(),
    )?;
    crate::download_path::ensure_no_symlink(download_root, local_path)?;
    let mut expected_bytes = 0_u64;
    let mut item_count_total = 0_u64;
    let mut files = Vec::new();
    let mut item_count_completed = 0_u64;
    let mut pending = vec![(
        raw_path,
        remote_path.to_string(),
        Some(local_path.to_path_buf()),
    )];
    while let Some((remote_dir_raw, remote_dir, local_dir)) = pending.pop() {
        control.wait_if_paused().await?;
        if let Some(local_dir) = &local_dir {
            path_options.download_runtime()?.ensure_directory(
                download_root,
                local_dir,
                path_options.is_promised_download(),
            )?;
            crate::download_path::ensure_no_symlink(download_root, local_dir)?;
        }
        for entry in await_sftp_request(control, sftp.read_dir_bytes(remote_dir_raw)).await? {
            control.wait_if_paused().await?;
            if entry.file_name_bytes() == b"." || entry.file_name_bytes() == b".." {
                continue;
            }
            let file_type = entry.file_type();
            // Keep totals for all regular files, including conflicts skipped below.
            // A skipped directory has no local target but is still scanned for totals.
            if file_type == russh_sftp::protocol::FileType::File {
                item_count_total = item_count_total.saturating_add(1);
                expected_bytes = expected_bytes.saturating_add(entry.metadata().size.unwrap_or(0));
            }
            let Some(local_dir) = &local_dir else {
                if file_type == russh_sftp::protocol::FileType::Dir {
                    pending.push((entry.path_bytes(), String::new(), None));
                }
                continue;
            };
            let name = codec.decode_path_lossy(entry.file_name_bytes());
            if !crate::download_path::validate_directory_entry(
                &name,
                attrs_to_sftp_file_type(&entry.metadata()),
            )? {
                continue;
            }
            let remote_child = remote_join(&remote_dir, &name);
            let remote_child_raw = entry.path_bytes();
            let local_child = local_dir.join(&name);
            match file_type {
                russh_sftp::protocol::FileType::Dir => {
                    let local_child =
                        resolve_local_download_target(SftpLocalDownloadTargetContext {
                            remote_path: &remote_child,
                            remote_path_raw: &remote_child_raw,
                            local_path: &local_child,
                            is_directory: true,
                            path_options,
                        })?;
                    pending.push((remote_child_raw, remote_child, local_child));
                }
                russh_sftp::protocol::FileType::File => {
                    if let Some(local_child) =
                        resolve_local_download_target(SftpLocalDownloadTargetContext {
                            remote_path: &remote_child,
                            remote_path_raw: &remote_child_raw,
                            local_path: &local_child,
                            is_directory: false,
                            path_options,
                        })?
                    {
                        files.push((
                            remote_child_raw,
                            remote_child,
                            local_child,
                            entry.metadata().size.unwrap_or(0),
                        ));
                    } else {
                        item_count_completed = item_count_completed.saturating_add(1);
                    }
                }
                russh_sftp::protocol::FileType::Other => {}
                russh_sftp::protocol::FileType::Symlink => {}
            }
        }
    }
    let scan_us = total_started.elapsed().as_micros() as u64;
    let transfer_started = Instant::now();
    let total_bytes = AtomicU64::new(0);
    let completed = AtomicU64::new(item_count_completed);
    let progress = StdMutex::new(progress);
    let options = path_options.transfer_options();
    let concurrency = sftp_directory_concurrency(
        sftp.max_open_handles(),
        options,
        config.sftp.compatibility_mode,
    );
    let worker_count = files.len().min(concurrency.small_file_concurrency).max(1);
    let parameters = TransferParameters::new(options, &config.sftp).for_directory(worker_count);
    if files.is_empty() {
        return Ok(0);
    }
    // Enumeration holds no live file handles. Release its channel before the
    // transfer pool is opened so an ordinary directory uses at most two sessions.
    if !session.persistent {
        close_sftp_session(session.clone()).await;
    }
    let pool = SftpSessionPool::open(
        config,
        multiplex,
        concurrency.session_pool_size,
        parameters.client_config(),
    )
    .await?;
    let large_lane = Semaphore::new(concurrency.large_file_concurrency);
    let jobs =
        files
            .into_iter()
            .enumerate()
            .map(|(index, (raw_path, remote_path, local_path, size))| {
                let total_bytes = &total_bytes;
                let completed = &completed;
                let progress = &progress;
                let session = pool.session_for(index);
                let large_lane = &large_lane;
                async move {
                    let _large_permit = if is_sftp_large_file(size) {
                        Some(control.until_cancelled(large_lane.acquire()).await??)
                    } else {
                        None
                    };
                    let mut previous_bytes = 0;
                    let mut report = |mut current: SftpTransferProgress| {
                        let delta = current.bytes_transferred.saturating_sub(previous_bytes);
                        previous_bytes = current.bytes_transferred;
                        current.bytes_transferred =
                            total_bytes.fetch_add(delta, Ordering::Relaxed) + delta;
                        current.total_bytes = (expected_bytes > 0).then_some(expected_bytes);
                        current.item_count_completed = Some(completed.load(Ordering::Relaxed));
                        current.item_count_total = Some(item_count_total);
                        if let Ok(mut callback) = progress.lock() {
                            callback(current);
                        }
                    };
                    download_remote_file_bytes(
                        &session.sftp,
                        raw_path,
                        &remote_path,
                        &local_path,
                        control,
                        options,
                        &mut report,
                        download_root,
                        Some(path_options),
                        parameters,
                    )
                    .await?;
                    let finished = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Ok(mut callback) = progress.lock() {
                        callback(SftpTransferProgress {
                            remote_path,
                            local_path,
                            bytes_transferred: total_bytes.load(Ordering::Relaxed),
                            total_bytes: (expected_bytes > 0).then_some(expected_bytes),
                            item_count_completed: Some(finished),
                            item_count_total: Some(item_count_total),
                        });
                    }
                    Ok::<(), anyhow::Error>(())
                }
            });
    let result = finish_download_jobs(jobs, worker_count).await;
    pool.close_all().await;
    result?;
    let bytes = total_bytes.load(Ordering::Relaxed);
    tracing::debug!(
        bytes,
        workers = worker_count,
        reads = parameters.reads,
        request_bytes = parameters.payload,
        scan_us,
        transfer_us = transfer_started.elapsed().as_micros() as u64,
        total_us = total_started.elapsed().as_micros() as u64,
        "SFTP directory download timings"
    );
    Ok(bytes)
}

/// Stop admitting jobs after the first failure, but drain admitted jobs so their
/// file commits and cleanup are not dropped.
async fn finish_download_jobs<I, F>(jobs: I, concurrency: usize) -> anyhow::Result<()>
where
    I: IntoIterator<Item = F>,
    F: Future<Output = anyhow::Result<()>>,
{
    use futures::{StreamExt as _, stream::FuturesUnordered};
    let mut jobs = jobs.into_iter();
    let mut running = FuturesUnordered::new();
    for job in jobs.by_ref().take(concurrency.max(1)) {
        running.push(job);
    }
    let mut first_error = None;
    while let Some(result) = running.next().await {
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
        if first_error.is_none()
            && let Some(job) = jobs.next()
        {
            running.push(job);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[expect(clippy::too_many_arguments)]
async fn upload_local_file<F>(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    upload_metadata: &UploadMetadata,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    upload_local_file_bytes(
        sftp,
        codec.encode_path(remote_path)?,
        local_path,
        remote_path,
        control,
        options,
        upload_metadata,
        progress,
    )
    .await
}

#[expect(clippy::too_many_arguments)]
async fn upload_local_file_bytes<F>(
    sftp: &SftpSession,
    raw_path: Vec<u8>,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    upload_metadata: &UploadMetadata,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    control.wait_if_paused().await?;
    let mut local = tokio::fs::File::open(local_path).await?;
    let local_metadata = local.metadata().await.ok();
    let total_bytes = local_metadata.as_ref().map(|metadata| metadata.len());
    let original = upload_metadata.capture(sftp, &raw_path, control).await?;
    let mut remote = await_sftp_request(control, sftp.create_bytes(raw_path.clone())).await?;
    let mut buffer = vec![0_u8; sftp_upload_buffer_size(options)];
    let mut bytes = 0_u64;
    progress(SftpTransferProgress {
        remote_path: remote_path.to_string(),
        local_path: local_path.to_path_buf(),
        bytes_transferred: bytes,
        total_bytes,
        item_count_completed: None,
        item_count_total: None,
    });
    loop {
        control.wait_if_paused().await?;
        let read = local.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        await_sftp_request(control, remote.write_all(&buffer[..read])).await?;
        control.wait_if_paused().await?;
        bytes += read as u64;
        progress(SftpTransferProgress {
            remote_path: remote_path.to_string(),
            local_path: local_path.to_path_buf(),
            bytes_transferred: bytes,
            total_bytes,
            item_count_completed: None,
            item_count_total: None,
        });
    }
    await_sftp_request(control, remote.shutdown()).await?;
    finish_upload_metadata(
        sftp,
        raw_path.clone(),
        original.as_ref(),
        options.default_file_mode,
        control,
    )
    .await?;
    if options.preserve_timestamps {
        preserve_remote_modified_time_bytes(sftp, raw_path, local_metadata).await;
    }
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalDirectoryUploadFileInventory {
    local_path: PathBuf,
    relative_path: PathBuf,
    size: u64,
    modified_at: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalDirectoryUploadDirectoryInventory {
    local_path: PathBuf,
    relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalDirectoryUploadInventory {
    directories: Vec<LocalDirectoryUploadDirectoryInventory>,
    files: Vec<LocalDirectoryUploadFileInventory>,
    total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalDirectoryUploadEntry {
    local_path: PathBuf,
    remote_path: String,
    size: u64,
}

struct LocalDirectoryUploadContext<'a> {
    codec: &'a SftpPathCodec,
    config: &'a SshSessionConfig,
    multiplex: Option<&'a SshMultiplexHandle>,
    control: &'a SftpTransferControl,
    path_options: &'a SftpPathTransferOptions,
    upload_metadata: &'a UploadMetadata,
}

async fn upload_local_directory<F>(
    sftp: Arc<SftpSession>,
    local_path: &Path,
    remote_path: &str,
    context: LocalDirectoryUploadContext<'_>,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    context.control.wait_if_paused().await?;
    let max_open_handles = sftp.max_open_handles();
    let inventory = collect_local_directory_upload_inventory(local_path, context.control).await?;
    let entries = plan_local_directory_upload_entries(
        &sftp,
        context.codec,
        remote_path,
        inventory,
        context.control,
        context.path_options,
    )
    .await?;
    if context.config.sftp.compatibility_mode {
        let expected_bytes = entries
            .iter()
            .fold(0_u64, |sum, entry| sum.saturating_add(entry.size));
        let item_count = entries.len() as u64;
        let mut transferred = 0_u64;
        for (index, entry) in entries.into_iter().enumerate() {
            let mut report = |current| {
                progress(directory_transfer_progress(
                    current,
                    transferred,
                    expected_bytes,
                    index as u64,
                    item_count,
                ));
            };
            let bytes = upload_local_file(
                &sftp,
                context.codec,
                &entry.local_path,
                &entry.remote_path,
                context.control,
                context.path_options.transfer_options(),
                context.upload_metadata,
                &mut report,
            )
            .await?;
            transferred = transferred.saturating_add(bytes);
            progress(SftpTransferProgress {
                remote_path: entry.remote_path,
                local_path: entry.local_path,
                bytes_transferred: transferred,
                total_bytes: (expected_bytes > 0).then_some(expected_bytes),
                item_count_completed: Some(index as u64 + 1),
                item_count_total: Some(item_count),
            });
        }
        return Ok(transferred);
    }
    upload_local_directory_entries(context, entries, max_open_handles, progress).await
}

async fn collect_local_directory_upload_inventory(
    local_path: &Path,
    control: &SftpTransferControl,
) -> anyhow::Result<LocalDirectoryUploadInventory> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    let mut pending = vec![(local_path.to_path_buf(), PathBuf::new())];
    while let Some((local_dir, remote_dir)) = pending.pop() {
        control.wait_if_paused().await?;
        let mut entries = tokio::fs::read_dir(&local_dir).await?;
        let mut children = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            children.push(entry);
        }
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            control.wait_if_paused().await?;
            let local_child = entry.path();
            let file_type = entry.file_type().await?;
            let relative_child = if remote_dir.as_os_str().is_empty() {
                PathBuf::from(entry.file_name())
            } else {
                remote_dir.join(entry.file_name())
            };
            if file_type.is_dir() {
                directories.push(LocalDirectoryUploadDirectoryInventory {
                    local_path: local_child.clone(),
                    relative_path: relative_child.clone(),
                });
                pending.push((local_child, relative_child));
            } else if file_type.is_file() {
                let metadata = entry.metadata().await?;
                let size = metadata.len();
                let modified_at = metadata.modified().ok().and_then(sftp_timestamp_secs);
                files.push(LocalDirectoryUploadFileInventory {
                    local_path: local_child,
                    relative_path: relative_child,
                    size,
                    modified_at,
                });
                total_bytes = total_bytes.saturating_add(size);
            }
        }
    }
    Ok(LocalDirectoryUploadInventory {
        directories,
        files,
        total_bytes,
    })
}

async fn plan_local_directory_upload_entries(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    remote_path: &str,
    inventory: LocalDirectoryUploadInventory,
    control: &SftpTransferControl,
    path_options: &SftpPathTransferOptions,
) -> anyhow::Result<Vec<LocalDirectoryUploadEntry>> {
    control.wait_if_paused().await?;
    ensure_remote_dir(sftp, codec, remote_path, control).await?;
    let mut directory_targets = HashMap::new();
    directory_targets.insert(PathBuf::new(), remote_path.to_string());

    for directory in inventory.directories {
        control.wait_if_paused().await?;
        let Some((parent_relative, name)) =
            local_upload_relative_parent_and_name(&directory.relative_path)?
        else {
            continue;
        };
        let Some(parent_remote) = directory_targets.get(parent_relative) else {
            continue;
        };
        let remote_child = remote_join(parent_remote, &name);
        let Some(remote_child) = resolve_remote_upload_write_target(SftpRemoteWriteTargetContext {
            sftp,
            codec,
            local_path: &directory.local_path,
            remote_path: &remote_child,
            is_directory: true,
            path_options,
        })
        .await?
        else {
            continue;
        };
        ensure_remote_dir(sftp, codec, &remote_child, control).await?;
        directory_targets.insert(directory.relative_path, remote_child);
    }

    let mut entries = Vec::with_capacity(inventory.files.len());
    for file in inventory.files {
        control.wait_if_paused().await?;
        let Some((parent_relative, name)) =
            local_upload_relative_parent_and_name(&file.relative_path)?
        else {
            continue;
        };
        let Some(parent_remote) = directory_targets.get(parent_relative) else {
            continue;
        };
        let remote_child = remote_join(parent_remote, &name);
        let Some(remote_child) = resolve_remote_upload_write_target(SftpRemoteWriteTargetContext {
            sftp,
            codec,
            local_path: &file.local_path,
            remote_path: &remote_child,
            is_directory: false,
            path_options,
        })
        .await?
        else {
            continue;
        };
        entries.push(LocalDirectoryUploadEntry {
            local_path: file.local_path,
            remote_path: remote_child,
            size: file.size,
        });
    }
    Ok(entries)
}

async fn upload_local_directory_entries<F>(
    context: LocalDirectoryUploadContext<'_>,
    entries: Vec<LocalDirectoryUploadEntry>,
    max_open_handles: Option<u64>,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    let item_count_total = entries.len() as u64;
    let expected_bytes = entries
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.size));
    let concurrency = sftp_directory_concurrency(
        max_open_handles,
        context.path_options.transfer_options(),
        context.config.sftp.compatibility_mode,
    );
    let worker_count = directory_upload_worker_count(entries.len(), concurrency);
    if worker_count == 0 {
        return Ok(0);
    }
    let pool = SftpSessionPool::open(
        context.config,
        context.multiplex,
        concurrency.session_pool_size,
        TransferParameters::new(
            context.path_options.transfer_options(),
            &context.config.sftp,
        )
        .client_config(),
    )
    .await?;

    let queue = Arc::new(StdMutex::new(VecDeque::from(entries)));
    let codec = *context.codec;
    let bytes_transferred = Arc::new(AtomicU64::new(0));
    let item_count_completed = Arc::new(AtomicU64::new(0));
    let large_lane = Arc::new(Semaphore::new(concurrency.large_file_concurrency));
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let mut workers = JoinSet::new();

    for worker_index in 0..worker_count {
        let worker_sftp = pool.session_for(worker_index);
        let worker_queue = Arc::clone(&queue);
        let worker_control = context.control.clone();
        let worker_options = context.path_options.transfer_options().clone();
        let worker_metadata = context.upload_metadata.clone();
        let worker_bytes_transferred = Arc::clone(&bytes_transferred);
        let worker_item_count_completed = Arc::clone(&item_count_completed);
        let worker_large_lane = Arc::clone(&large_lane);
        let worker_progress_tx = progress_tx.clone();
        workers.spawn(async move {
            loop {
                worker_control.wait_if_paused().await?;
                let entry = next_local_directory_upload_entry(&worker_queue)?;
                let Some(entry) = entry else {
                    break;
                };
                let _large_permit = if is_sftp_large_file(entry.size) {
                    Some(worker_large_lane.acquire().await?)
                } else {
                    None
                };
                upload_local_directory_entry(
                    Arc::clone(&worker_sftp.sftp),
                    codec,
                    entry,
                    worker_control.clone(),
                    worker_options.clone(),
                    worker_metadata.clone(),
                    worker_bytes_transferred.clone(),
                    worker_item_count_completed.clone(),
                    expected_bytes,
                    item_count_total,
                    worker_progress_tx.clone(),
                )
                .await?;
            }
            Ok::<(), anyhow::Error>(())
        });
    }
    drop(progress_tx);

    let mut pending_progress = None;
    let mut last_progress_emit = Instant::now();
    let mut last_progress_snapshot =
        directory_upload_progress_snapshot(&bytes_transferred, &item_count_completed);
    let mut last_progress_at = Instant::now();
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut remaining_workers = worker_count;
    let mut progress_closed = false;
    let result = async {
    while remaining_workers > 0 {
        tokio::select! {
            progress_update = progress_rx.recv(), if !progress_closed => {
                if let Some(progress_update) = progress_update {
                    let should_emit = last_progress_emit.elapsed() >= SFTP_PROGRESS_INTERVAL
                        || progress_update.item_count_completed == progress_update.item_count_total;
                    if should_emit {
                        progress(progress_update);
                        last_progress_emit = Instant::now();
                        pending_progress = None;
                    } else {
                        pending_progress = Some(progress_update);
                    }
                } else {
                    progress_closed = true;
                }
            }
            joined = workers.join_next() => {
                match joined {
                    Some(Ok(Ok(()))) => {
                        remaining_workers -= 1;
                    }
                    Some(Ok(Err(error))) => {
                        workers.abort_all();
                        return Err(error);
                    }
                    Some(Err(error)) => {
                        workers.abort_all();
                        return Err(anyhow::anyhow!("SFTP directory upload worker failed: {error}"));
                    }
                    None => break,
                }
            }
            _ = watchdog.tick() => {
                let current_snapshot =
                    directory_upload_progress_snapshot(&bytes_transferred, &item_count_completed);
                if context.control.is_paused() {
                    last_progress_snapshot = current_snapshot;
                    last_progress_at = Instant::now();
                } else if context.control.is_cancelled() {
                    workers.abort_all();
                    return Err(anyhow::anyhow!(SFTP_TRANSFER_CANCELLED));
                } else if current_snapshot != last_progress_snapshot {
                    last_progress_snapshot = current_snapshot;
                    last_progress_at = Instant::now();
                } else if directory_upload_stalled(
                    false,
                    false,
                    last_progress_snapshot,
                    current_snapshot,
                    last_progress_at.elapsed(),
                    item_count_total,
                ) {
                    workers.abort_all();
                    return Err(anyhow::anyhow!("SFTP transfer stalled"));
                }
            }
        }
    }
    if let Some(progress_update) = pending_progress.take() {
        progress(progress_update);
    }
    while let Ok(progress_update) = progress_rx.try_recv() {
        progress(progress_update);
    }
    Ok(bytes_transferred.load(Ordering::Relaxed))
    }
    .await;
    pool.close_all().await;
    result
}

fn next_local_directory_upload_entry(
    queue: &StdMutex<VecDeque<LocalDirectoryUploadEntry>>,
) -> anyhow::Result<Option<LocalDirectoryUploadEntry>> {
    queue
        .lock()
        .map_err(|_| anyhow::anyhow!("SFTP directory upload queue lock poisoned"))
        .map(|mut queue| queue.pop_front())
}

#[expect(clippy::too_many_arguments)]
async fn upload_local_directory_entry(
    sftp: Arc<SftpSession>,
    codec: SftpPathCodec,
    entry: LocalDirectoryUploadEntry,
    control: SftpTransferControl,
    options: SftpTransferOptions,
    upload_metadata: UploadMetadata,
    bytes_transferred: Arc<AtomicU64>,
    item_count_completed: Arc<AtomicU64>,
    expected_bytes: u64,
    item_count_total: u64,
    progress_tx: mpsc::UnboundedSender<SftpTransferProgress>,
) -> anyhow::Result<()> {
    let mut last_file_bytes = 0_u64;
    let progress_remote_path = entry.remote_path.clone();
    let progress_local_path = entry.local_path.clone();
    let mut aggregate_progress = |current: SftpTransferProgress| {
        let delta = current.bytes_transferred.saturating_sub(last_file_bytes);
        if delta == 0 {
            return;
        }
        last_file_bytes = current.bytes_transferred;
        let aggregate_bytes = bytes_transferred
            .fetch_add(delta, Ordering::Relaxed)
            .saturating_add(delta);
        let completed_items = item_count_completed.load(Ordering::Relaxed);
        let _ = progress_tx.send(directory_upload_aggregate_progress(
            current,
            aggregate_bytes,
            expected_bytes,
            completed_items,
            item_count_total,
        ));
    };
    upload_local_file(
        &sftp,
        &codec,
        &entry.local_path,
        &entry.remote_path,
        &control,
        &options,
        &upload_metadata,
        &mut aggregate_progress,
    )
    .await?;
    let completed_items = item_count_completed
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1)
        .min(item_count_total);
    let aggregate_bytes = bytes_transferred.load(Ordering::Relaxed);
    let _ = progress_tx.send(SftpTransferProgress {
        remote_path: progress_remote_path,
        local_path: progress_local_path,
        bytes_transferred: aggregate_bytes,
        total_bytes: (expected_bytes > 0).then_some(expected_bytes),
        item_count_completed: Some(completed_items),
        item_count_total: Some(item_count_total),
    });
    Ok(())
}

fn directory_upload_aggregate_progress(
    current: SftpTransferProgress,
    aggregate_bytes: u64,
    expected_bytes: u64,
    item_count_completed: u64,
    item_count_total: u64,
) -> SftpTransferProgress {
    SftpTransferProgress {
        remote_path: current.remote_path,
        local_path: current.local_path,
        bytes_transferred: aggregate_bytes,
        total_bytes: (expected_bytes > 0).then_some(expected_bytes),
        item_count_completed: Some(item_count_completed.min(item_count_total)),
        item_count_total: Some(item_count_total),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryUploadProgressSnapshot {
    bytes: u64,
    completed_items: u64,
}

fn directory_upload_progress_snapshot(
    bytes_transferred: &AtomicU64,
    item_count_completed: &AtomicU64,
) -> DirectoryUploadProgressSnapshot {
    DirectoryUploadProgressSnapshot {
        bytes: bytes_transferred.load(Ordering::Relaxed),
        completed_items: item_count_completed.load(Ordering::Relaxed),
    }
}

fn directory_upload_stalled(
    paused: bool,
    cancelled: bool,
    last_progress: DirectoryUploadProgressSnapshot,
    current_progress: DirectoryUploadProgressSnapshot,
    idle_for: Duration,
    item_count_total: u64,
) -> bool {
    !paused
        && !cancelled
        && current_progress == last_progress
        && current_progress.completed_items < item_count_total
        && idle_for >= SFTP_DIRECTORY_STALL_TIMEOUT
}

// 目录规划在默认覆盖策略下无需额外探测，其他策略复用统一的目标解析上下文。
async fn resolve_remote_upload_write_target(
    context: SftpRemoteWriteTargetContext<'_>,
) -> anyhow::Result<Option<String>> {
    if !remote_upload_write_target_requires_probe(context.path_options.duplicate_policy()) {
        return Ok(Some(context.remote_path.to_string()));
    }
    resolve_remote_write_target(context).await
}

fn remote_upload_write_target_requires_probe(duplicate_policy: SftpDuplicatePolicy) -> bool {
    matches!(
        duplicate_policy,
        SftpDuplicatePolicy::Ask | SftpDuplicatePolicy::Rename | SftpDuplicatePolicy::Skip
    )
}

fn local_upload_relative_parent_and_name(
    relative_path: &Path,
) -> anyhow::Result<Option<(&Path, String)>> {
    let Some(name) = relative_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
    else {
        return Ok(None);
    };
    let parent = relative_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new(""));
    Ok(Some((parent, name)))
}

fn directory_transfer_progress(
    current: SftpTransferProgress,
    completed_bytes: u64,
    expected_bytes: u64,
    item_count_completed: u64,
    item_count_total: u64,
) -> SftpTransferProgress {
    SftpTransferProgress {
        remote_path: current.remote_path,
        local_path: current.local_path,
        bytes_transferred: completed_bytes.saturating_add(current.bytes_transferred),
        total_bytes: (expected_bytes > 0).then_some(expected_bytes),
        item_count_completed: Some(item_count_completed.min(item_count_total)),
        item_count_total: Some(item_count_total),
    }
}

async fn ensure_remote_dir(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    remote_path: &str,
    control: &SftpTransferControl,
) -> anyhow::Result<()> {
    control.wait_if_paused().await?;
    if sftp
        .try_exists_bytes(codec.encode_path(remote_path)?)
        .await?
    {
        return Ok(());
    }
    control.wait_if_paused().await?;
    sftp.create_dir_bytes(codec.encode_path(remote_path)?)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn directory_failure_drains_started_files_without_starting_queued_files() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let started = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let jobs = (0..5).map(|index| {
            let started = Arc::clone(&started);
            let finished = Arc::clone(&finished);
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    anyhow::bail!("first failure");
                }
                tokio::task::yield_now().await;
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let error = super::finish_download_jobs(jobs, 2).await.unwrap_err();
        assert_eq!(error.to_string(), "first failure");
        assert_eq!(started.load(Ordering::SeqCst), 2);
        assert_eq!(finished.load(Ordering::SeqCst), 1);

        // Successful results, including duplicate skips, allow more jobs to be admitted.
        let completed = AtomicUsize::new(0);
        let jobs = (0..5).map(|_| async {
            completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        super::finish_download_jobs(jobs, 2).await.unwrap();
        assert_eq!(completed.load(Ordering::SeqCst), 5);
    }

    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{Duration, UNIX_EPOCH};

    use crate::{SftpSettings, SshSessionConfig};

    use super::{
        DirectoryUploadProgressSnapshot, RemoteFilePath, SFTP_DIRECTORY_STALL_TIMEOUT,
        SFTP_SMALL_FILE_THRESHOLD, SFTP_TRANSFER_CANCELLED, SftpCompatibilityState,
        SftpDirectoryConcurrency, SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest,
        SftpDuplicateResolver, SftpFileEntry, SftpFileType, SftpLocalDownloadTargetContext,
        SftpPathCodec, SftpPathTransferOptions, SftpTransferControl, SftpTransferDirection,
        SftpTransferOptions, SftpTransferProgress, collect_local_directory_upload_inventory,
        directory_transfer_progress, directory_upload_aggregate_progress, directory_upload_stalled,
        directory_upload_worker_count, ensure_remote_download_source, is_sftp_large_file,
        is_sftp_transfer_cancelled, remote_conflict_candidate, remote_join, remote_join_bytes,
        remote_upload_file_mode, remote_upload_write_target_requires_probe,
        resolve_duplicate_decision, resolve_local_download_target, resolve_remote_upload_target,
        sftp_client_config_for_options, sftp_directory_concurrency, sftp_pipeline_config,
        sftp_session_pool_index, sftp_timestamp_secs, sftp_upload_buffer_size,
        transfer_resume_offset,
    };

    #[test]
    fn raw_remote_paths_round_trip_and_derive_the_raw_parent() {
        let raw = b"/srv/non-utf8-\xff/file.txt";
        let path = RemoteFilePath::from_raw("/srv/non-utf8-?/file.txt", raw);

        assert_eq!(path.raw_path().unwrap().as_deref(), Some(raw.as_slice()));
        let parent = path.parent().unwrap();
        assert_eq!(parent.display_path, "/srv/non-utf8-?");
        assert_eq!(
            parent.raw_path().unwrap().as_deref(),
            Some(b"/srv/non-utf8-\xff".as_slice())
        );
    }

    #[test]
    fn raw_tokens_distinguish_lossy_display_name_collisions() {
        let entry = |raw_name: &[u8]| SftpFileEntry {
            name: "invalid-?".to_string(),
            path: "/srv/invalid-?".to_string(),
            file_type: SftpFileType::File,
            size: Some(1),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: RemoteFilePath::from_raw("/srv/invalid-?", raw_name).raw_path_token,
            symlink_target_is_directory: false,
        };
        let first = entry(b"/srv/invalid-\xfe");
        let second = entry(b"/srv/invalid-\xff");

        assert_eq!(first.path, second.path);
        assert_ne!(first.identity_key(), second.identity_key());
        assert!(first.matches_identity(&first.identity_key()));
        assert!(!first.matches_identity(&second.identity_key()));
    }

    #[test]
    fn symlink_directory_semantics_are_independent_from_file_type() {
        let entry = SftpFileEntry {
            name: "link".to_string(),
            path: "/srv/link".to_string(),
            file_type: SftpFileType::Symlink,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: true,
        };

        assert!(entry.is_symlink());
        assert!(entry.is_directory());
    }

    #[test]
    fn remote_byte_path_join_preserves_non_utf8_names() {
        assert_eq!(
            remote_join_bytes(b"/srv/raw", b"name-\xff"),
            b"/srv/raw/name-\xff"
        );
        assert_eq!(remote_join_bytes(b"/", b"child"), b"/child");
    }

    #[test]
    fn sftp_path_codec_encodes_supported_filename_encodings() {
        let utf8 = SftpPathCodec::from_encoding_name("UTF-8").expect("utf8 codec");
        assert_eq!(utf8.encoding_name(), "UTF-8");
        assert_eq!(
            utf8.encode_path("/tmp/猫.txt").expect("encode"),
            "/tmp/猫.txt".as_bytes()
        );
        assert_eq!(
            utf8.decode_path("/tmp/猫.txt".as_bytes()).expect("decode"),
            "/tmp/猫.txt"
        );

        let gbk = SftpPathCodec::from_encoding_name("GBK").expect("gbk codec");
        assert_eq!(
            gbk.encode_path("中文").expect("encode"),
            vec![0xd6, 0xd0, 0xce, 0xc4]
        );
        assert_eq!(
            gbk.decode_path(&[0xd6, 0xd0, 0xce, 0xc4]).expect("decode"),
            "中文"
        );

        let gb2312 = SftpPathCodec::from_encoding_name("GB2312").expect("gb2312 codec");
        assert_eq!(
            gb2312.encode_path("中文").expect("encode"),
            vec![0xd6, 0xd0, 0xce, 0xc4]
        );

        let gb18030 = SftpPathCodec::from_encoding_name("GB18030").expect("gb18030 codec");
        assert_eq!(
            gb18030
                .decode_path(&gb18030.encode_path("中文").expect("encode"))
                .expect("decode"),
            "中文"
        );
    }

    #[test]
    fn sftp_path_codec_uses_terminal_encoding_and_rejects_unknown_values() {
        let config = SshSessionConfig {
            encoding: "GBK".to_string(),
            sftp: SftpSettings {
                filename_encoding: "terminal".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let codec = SftpPathCodec::from_ssh_config(&config).expect("terminal codec");
        assert_eq!(codec.encoding_name(), "GBK");

        let error = SftpPathCodec::from_encoding_name("KOI8-R").expect_err("unknown encoding");
        assert!(error.to_string().contains("Unsupported character encoding"));
    }

    #[test]
    fn directory_progress_accumulates_file_bytes_and_preserves_item_counts() {
        let current = SftpTransferProgress {
            remote_path: "/remote/two.txt".to_string(),
            local_path: PathBuf::from("/local/two.txt"),
            bytes_transferred: 25,
            total_bytes: Some(100),
            item_count_completed: None,
            item_count_total: None,
        };

        let aggregate = directory_transfer_progress(current, 100, 400, 1, 4);

        assert_eq!(aggregate.bytes_transferred, 125);
        assert_eq!(aggregate.total_bytes, Some(400));
        assert_eq!(aggregate.item_count_completed, Some(1));
        assert_eq!(aggregate.item_count_total, Some(4));
        assert_eq!(aggregate.remote_path, "/remote/two.txt");
    }

    #[test]
    fn directory_upload_worker_count_is_independent_of_outer_queue_limit() {
        let default_options = SftpTransferOptions::default();
        let default_concurrency = sftp_directory_concurrency(None, &default_options, false);
        assert_eq!(directory_upload_worker_count(0, default_concurrency), 0);
        assert_eq!(directory_upload_worker_count(1, default_concurrency), 1);
        assert_eq!(directory_upload_worker_count(10, default_concurrency), 10);
        assert_eq!(directory_upload_worker_count(20, default_concurrency), 16);

        let two_workers = SftpTransferOptions::default().with_directory_upload_threads(2);
        let two_worker_concurrency = sftp_directory_concurrency(None, &two_workers, false);
        assert_eq!(
            directory_upload_worker_count(10, two_worker_concurrency),
            10
        );

        let capped = SftpTransferOptions::default().with_directory_upload_threads(99);
        let capped_concurrency = sftp_directory_concurrency(None, &capped, false);
        assert_eq!(directory_upload_worker_count(20, capped_concurrency), 16);
    }

    #[test]
    fn sftp_pipeline_config_clamps_request_size_and_write_pipeline() {
        let small = SftpTransferOptions::default().with_buffer_size_bytes(8 * 1024);
        assert_eq!(sftp_pipeline_config(&small), (64, 16));
        assert_eq!(sftp_upload_buffer_size(&small), 64 * 1024 - 1024);

        let large = SftpTransferOptions::default().with_buffer_size_bytes(512 * 1024);
        assert_eq!(sftp_pipeline_config(&large), (256, 8));
        let config = sftp_client_config_for_options(&large);
        assert_eq!(config.max_packet_len, 256 * 1024);
        assert_eq!(config.max_concurrent_writes, 8);
    }

    #[test]
    fn sftp_directory_concurrency_respects_server_handle_budget() {
        let default_options = SftpTransferOptions::default();
        assert_eq!(
            sftp_directory_concurrency(None, &default_options, false),
            SftpDirectoryConcurrency {
                session_pool_size: 2,
                small_file_concurrency: 16,
                large_file_concurrency: 2,
            }
        );
        assert_eq!(
            sftp_directory_concurrency(Some(10), &default_options, false),
            SftpDirectoryConcurrency {
                session_pool_size: 2,
                small_file_concurrency: 2,
                large_file_concurrency: 2,
            }
        );
        assert_eq!(
            sftp_directory_concurrency(
                Some(128),
                &SftpTransferOptions::default().with_directory_upload_threads(1),
                false,
            )
            .small_file_concurrency,
            16
        );
    }

    #[test]
    fn compatibility_mode_forces_one_session_and_one_worker() {
        assert_eq!(
            sftp_directory_concurrency(None, &SftpTransferOptions::default(), true),
            SftpDirectoryConcurrency {
                session_pool_size: 1,
                small_file_concurrency: 1,
                large_file_concurrency: 1,
            }
        );
    }

    #[test]
    fn compatibility_state_serializes_operations_on_one_runtime() {
        let state = Arc::new(SftpCompatibilityState::dedicated());
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::new();
        for _ in 0..3 {
            let state = state.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            workers.push(std::thread::spawn(move || {
                state
                    .block_on(async move {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_active.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .expect("compatibility operation");
            }));
        }
        for worker in workers {
            worker.join().expect("worker");
        }
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sftp_session_pool_index_round_robins_workers() {
        let assignments = (0..6)
            .map(|worker| sftp_session_pool_index(worker, 2))
            .collect::<Vec<_>>();
        assert_eq!(assignments, vec![0, 1, 0, 1, 0, 1]);
    }

    #[test]
    fn sftp_large_file_lane_uses_small_file_threshold() {
        assert!(!is_sftp_large_file(SFTP_SMALL_FILE_THRESHOLD));
        assert!(is_sftp_large_file(SFTP_SMALL_FILE_THRESHOLD + 1));
    }

    #[test]
    fn directory_upload_stall_detection_ignores_pause_and_cancel() {
        let snapshot = DirectoryUploadProgressSnapshot {
            bytes: 128,
            completed_items: 1,
        };
        assert!(directory_upload_stalled(
            false,
            false,
            snapshot,
            snapshot,
            SFTP_DIRECTORY_STALL_TIMEOUT,
            2,
        ));
        assert!(!directory_upload_stalled(
            true,
            false,
            snapshot,
            snapshot,
            SFTP_DIRECTORY_STALL_TIMEOUT,
            2,
        ));
        assert!(!directory_upload_stalled(
            false,
            true,
            snapshot,
            snapshot,
            SFTP_DIRECTORY_STALL_TIMEOUT,
            2,
        ));
        assert!(!directory_upload_stalled(
            false,
            false,
            snapshot,
            DirectoryUploadProgressSnapshot {
                bytes: 256,
                completed_items: 1,
            },
            SFTP_DIRECTORY_STALL_TIMEOUT,
            2,
        ));
    }

    #[test]
    fn upload_overwrite_policy_skips_remote_conflict_probe() {
        assert!(!remote_upload_write_target_requires_probe(
            SftpDuplicatePolicy::Overwrite
        ));
        assert!(remote_upload_write_target_requires_probe(
            SftpDuplicatePolicy::Ask
        ));
        assert!(remote_upload_write_target_requires_probe(
            SftpDuplicatePolicy::Skip
        ));
        assert!(remote_upload_write_target_requires_probe(
            SftpDuplicatePolicy::Rename
        ));
    }

    #[test]
    fn upload_file_mode_preserves_existing_permissions_including_special_bits() {
        assert_eq!(
            remote_upload_file_mode(Some(0o106755), Some(0o600)),
            Some(0o6755)
        );
        assert_eq!(remote_upload_file_mode(None, Some(0o600)), Some(0o600));
        assert_eq!(remote_upload_file_mode(Some(0o755), None), Some(0o755));
        assert_eq!(remote_upload_file_mode(None, None), None);
    }

    #[test]
    fn directory_upload_progress_uses_global_bytes_and_completed_items() {
        let current = SftpTransferProgress {
            remote_path: "/remote/current.txt".to_string(),
            local_path: PathBuf::from("/local/current.txt"),
            bytes_transferred: 15,
            total_bytes: Some(20),
            item_count_completed: None,
            item_count_total: None,
        };

        let aggregate = directory_upload_aggregate_progress(current, 250, 400, 2, 5);

        assert_eq!(aggregate.bytes_transferred, 250);
        assert_eq!(aggregate.total_bytes, Some(400));
        assert_eq!(aggregate.item_count_completed, Some(2));
        assert_eq!(aggregate.item_count_total, Some(5));
        assert_eq!(aggregate.remote_path, "/remote/current.txt");
    }

    #[tokio::test]
    async fn local_directory_upload_inventory_preserves_nested_paths_sizes_and_mtime() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-upload-inventory");
        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).expect("nested dir");
        std::fs::write(dir.join("root.txt"), b"root").expect("root file");
        std::fs::write(nested.join("child.txt"), b"child").expect("child file");

        let inventory = collect_local_directory_upload_inventory(&dir, &SftpTransferControl::new())
            .await
            .expect("inventory");
        let mut directories = inventory
            .directories
            .iter()
            .map(|directory| directory.relative_path.clone())
            .collect::<Vec<_>>();
        directories.sort();
        let mut files = inventory.files.clone();
        files.sort_by_key(|file| file.relative_path.clone());

        assert_eq!(directories, vec![PathBuf::from("nested")]);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, PathBuf::from("nested/child.txt"));
        assert_eq!(files[0].size, 5);
        assert!(files[0].modified_at.is_some());
        assert_eq!(files[1].relative_path, PathBuf::from("root.txt"));
        assert_eq!(files[1].size, 4);
        assert!(files[1].modified_at.is_some());
        assert_eq!(inventory.total_bytes, 9);
    }

    #[test]
    fn sftp_transfer_retry_helpers_detect_cancelled_errors() {
        assert!(is_sftp_transfer_cancelled(&anyhow::anyhow!(
            SFTP_TRANSFER_CANCELLED
        )));
        assert!(!is_sftp_transfer_cancelled(&anyhow::anyhow!(
            "permission denied"
        )));
    }

    #[test]
    fn sftp_timestamp_secs_handles_unix_bounds() {
        assert_eq!(sftp_timestamp_secs(UNIX_EPOCH), Some(0));
        assert_eq!(
            sftp_timestamp_secs(UNIX_EPOCH + Duration::from_secs(42)),
            Some(42)
        );
        assert_eq!(
            sftp_timestamp_secs(UNIX_EPOCH - Duration::from_secs(1)),
            None
        );
        assert_eq!(
            sftp_timestamp_secs(UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX) + 100)),
            Some(u32::MAX)
        );
    }

    #[test]
    fn direct_sftp_download_rejects_symbolic_link_sources() {
        assert!(ensure_remote_download_source(russh_sftp::protocol::FileType::File).is_ok());
        assert!(ensure_remote_download_source(russh_sftp::protocol::FileType::Dir).is_ok());
        assert!(ensure_remote_download_source(russh_sftp::protocol::FileType::Symlink).is_err());
        assert!(ensure_remote_download_source(russh_sftp::protocol::FileType::Other).is_err());
    }

    #[test]
    fn transfer_resume_offset_requires_partial_local_file() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-resume-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("partial.bin");
        let enabled = SftpTransferOptions::default().with_resume_broken_transfer(true);
        let disabled = SftpTransferOptions::default();

        assert_eq!(transfer_resume_offset(&file, Some(10), &enabled), 0);
        std::fs::write(&file, [1_u8, 2, 3, 4]).expect("partial");
        assert_eq!(transfer_resume_offset(&file, Some(10), &disabled), 0);
        assert_eq!(transfer_resume_offset(&file, None, &enabled), 0);
        assert_eq!(transfer_resume_offset(&file, Some(10), &enabled), 4);
        assert_eq!(transfer_resume_offset(&file, Some(4), &enabled), 0);
        assert_eq!(transfer_resume_offset(&file, Some(3), &enabled), 0);
    }

    #[test]
    fn remote_join_handles_common_sftp_paths() {
        assert_eq!(remote_join(".", "file.txt"), "file.txt");
        assert_eq!(remote_join("", "file.txt"), "file.txt");
        assert_eq!(remote_join("/", "file.txt"), "/file.txt");
        assert_eq!(remote_join("/opt", "file.txt"), "/opt/file.txt");
        assert_eq!(remote_join("/opt/", "file.txt"), "/opt/file.txt");
    }

    #[test]
    fn upload_target_uses_local_name_for_remote_directories() {
        let local = PathBuf::from("/tmp/archive.tar");
        assert_eq!(
            resolve_remote_upload_target(&local, ".").expect("target"),
            "archive.tar"
        );
        assert_eq!(
            resolve_remote_upload_target(&local, "/srv/").expect("target"),
            "/srv/archive.tar"
        );
        assert_eq!(
            resolve_remote_upload_target(&local, "/srv/custom.tar").expect("target"),
            "/srv/custom.tar"
        );
    }

    #[test]
    fn local_download_target_applies_skip_and_rename_policy() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-sftp");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("archive.tar.gz");
        std::fs::write(&target, b"existing").expect("target");
        let skip_options = SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Skip,
            None,
            SftpTransferOptions::default(),
        );

        assert_eq!(
            resolve_local_download_target(SftpLocalDownloadTargetContext {
                remote_path: "/remote/archive.tar.gz",
                remote_path_raw: b"/remote/archive.tar.gz",
                local_path: &target,
                is_directory: false,
                path_options: &skip_options,
            })
            .expect("skip"),
            None
        );
        let rename_options = SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Rename,
            None,
            SftpTransferOptions::default(),
        );
        let renamed = resolve_local_download_target(SftpLocalDownloadTargetContext {
            remote_path: "/remote/archive.tar.gz",
            remote_path_raw: b"/remote/archive.tar.gz",
            local_path: &target,
            is_directory: false,
            path_options: &rename_options,
        })
        .expect("rename")
        .expect("renamed target");
        std::fs::write(&renamed, b"partial transfer").expect("partial renamed target");
        let retried = resolve_local_download_target(SftpLocalDownloadTargetContext {
            remote_path: "/remote/archive.tar.gz",
            remote_path_raw: b"/remote/archive.tar.gz",
            local_path: &target,
            is_directory: false,
            path_options: &rename_options,
        })
        .expect("retry rename")
        .expect("retry target");
        assert_ne!(retried, renamed, "重试不能覆盖已经占用的 Rename 目标");
    }

    #[test]
    fn local_download_target_rechecks_a_new_conflict_after_an_implicit_target() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-sftp");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("archive.txt");
        let options = SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Skip,
            None,
            SftpTransferOptions::default(),
        );

        assert_eq!(
            resolve_local_download_target(SftpLocalDownloadTargetContext {
                remote_path: "/remote/archive.txt",
                remote_path_raw: b"/remote/archive.txt",
                local_path: &target,
                is_directory: false,
                path_options: &options,
            })
            .expect("implicit target"),
            Some(target.clone())
        );
        std::fs::write(&target, b"created after the first probe").expect("new target");

        assert_eq!(
            resolve_local_download_target(SftpLocalDownloadTargetContext {
                remote_path: "/remote/archive.txt",
                remote_path_raw: b"/remote/archive.txt",
                local_path: &target,
                is_directory: false,
                path_options: &options,
            })
            .expect("rechecked conflict"),
            None
        );
    }

    #[test]
    fn duplicate_decisions_distinguish_lossy_remote_names_and_reuse_the_same_raw_path() {
        struct CountingResolver(Arc<AtomicUsize>);

        impl SftpDuplicateResolver for CountingResolver {
            fn resolve_duplicate(
                &self,
                _request: &SftpDuplicateRequest,
            ) -> Result<SftpDuplicateDecision, String> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(SftpDuplicateDecision::Overwrite)
            }
        }

        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-sftp");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("archive.txt");
        std::fs::write(&target, b"existing").expect("target");
        let calls = Arc::new(AtomicUsize::new(0));
        let options = SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Ask,
            Some(Arc::new(CountingResolver(Arc::clone(&calls)))),
            SftpTransferOptions::default(),
        );

        for (index, raw_path) in [
            b"/remote/\xff.txt".as_slice(),
            b"/remote/\xfe.txt".as_slice(),
        ]
        .into_iter()
        .enumerate()
        {
            let result = resolve_local_download_target(SftpLocalDownloadTargetContext {
                remote_path: "/remote/�.txt",
                remote_path_raw: raw_path,
                local_path: &target,
                is_directory: false,
                path_options: &options,
            });
            if index == 0 {
                assert_eq!(result.expect("first source"), Some(target.clone()));
            } else {
                // Distinct raw names still ask independently, but cannot overwrite
                // a target already reserved for another source in the same batch.
                assert!(result.unwrap_err().to_string().contains("reserved"));
            }
        }
        let retry_options = options.with_transfer_options(SftpTransferOptions::default());
        resolve_local_download_target(SftpLocalDownloadTargetContext {
            remote_path: "/remote/�.txt",
            remote_path_raw: b"/remote/\xff.txt",
            local_path: &target,
            is_directory: false,
            path_options: &retry_options,
        })
        .expect("resolve retry path")
        .expect("reuse overwrite decision");
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn ask_duplicate_policy_requires_resolver() {
        let error = resolve_duplicate_decision(
            SftpTransferDirection::Download,
            "/remote/file",
            "/tmp/file",
            false,
            SftpDuplicatePolicy::Ask,
            None,
        )
        .expect_err("missing resolver");

        assert!(error.to_string().contains("no resolver"));
    }

    #[test]
    fn remote_conflict_candidates_preserve_parent_and_extension() {
        assert_eq!(
            remote_conflict_candidate("/srv/archive.tar.gz", 2),
            "/srv/archive.tar(2).gz"
        );
        assert_eq!(remote_conflict_candidate("file", 1), "file(1)");
        assert_eq!(remote_conflict_candidate("/file", 1), "/file(1)");
    }

    #[test]
    fn sftp_transfer_control_reports_standard_cancel_error() {
        let control = SftpTransferControl::new();
        assert!(!control.is_cancelled());
        assert!(!control.is_paused());
        control.check_cancelled().expect("not cancelled");

        control.pause();
        assert!(control.is_paused());
        control.resume();
        assert!(!control.is_paused());

        control.pause();
        control.cancel();
        assert!(control.is_cancelled());
        assert!(!control.is_paused());
        let error = control.check_cancelled().expect_err("cancelled");
        assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
    }
}
