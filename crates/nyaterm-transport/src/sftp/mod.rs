//! SFTP browsing and file transfer.
//!
//! Split out of `lib.rs` by domain. The wire protocol, retry and resume
//! behaviour, conflict resolution and progress reporting are unchanged; this
//! only moves the code.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use russh::{ChannelOpenFailure, Disconnect, client};
use russh_sftp::client::{Config as SftpClientConfig, SftpSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::{
    PROCESS_TIMEOUT, SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest,
    SftpDuplicateResolver, SftpPathTransferOptions, SftpTransferDirection, SftpTransferOptions,
    SftpTransferProgress, SftpTransferSummary, SshClientHandler, SshMultiplexHandle,
    SshProcessService, SshSessionConfig, open_authenticated_ssh_handle,
};

mod path_codec;
pub use path_codec::SftpPathCodec;
mod remote_metadata;
use remote_metadata::{
    attrs_to_sftp_file_type, ensure_remote_text_bytes, ensure_safe_remote_delete_target,
    format_sftp_permissions, remote_file_name, remote_file_path_bytes, remote_parent,
    rename_target_path_bytes, resolve_remote_group_name, resolve_remote_group_value,
    resolve_remote_identity_names, resolve_remote_user_name, resolve_remote_user_value,
};
mod transfer_paths;
#[cfg(test)]
use transfer_paths::{remote_conflict_candidate, resolve_duplicate_decision};
use transfer_paths::{
    remote_join, resolve_local_download_target, resolve_remote_upload_target,
    resolve_remote_write_target,
};

pub const SFTP_TRANSFER_CANCELLED: &str = "SFTP transfer cancelled";

const SFTP_MIN_REQUEST_KIB: usize = 64;
const SFTP_MAX_REQUEST_KIB: usize = 256;
const SFTP_WRITE_PIPELINE_TARGET_KIB: usize = 2048;
const SFTP_MIN_CONCURRENT_WRITES: usize = 8;
const SFTP_MAX_CONCURRENT_WRITES: usize = 16;
const SFTP_PACKET_OVERHEAD_RESERVE: usize = 1024;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpFileType {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpFileEntry {
    pub name: String,
    pub path: String,
    pub file_type: SftpFileType,
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub owner: String,
    pub group: String,
    pub modified_at: Option<u32>,
    pub raw_path_token: Option<String>,
    pub symlink_target_is_directory: bool,
}

impl SftpFileEntry {
    pub fn is_directory(&self) -> bool {
        self.file_type == SftpFileType::Directory || self.symlink_target_is_directory
    }

    pub fn is_symlink(&self) -> bool {
        self.file_type == SftpFileType::Symlink
    }

    pub fn remote_path(&self) -> RemoteFilePath {
        RemoteFilePath {
            display_path: self.path.clone(),
            raw_path_token: self.raw_path_token.clone(),
        }
    }

    pub fn identity_key(&self) -> String {
        self.remote_path().identity_key()
    }

    pub fn matches_identity(&self, identity: &str) -> bool {
        self.path == identity || self.identity_key() == identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpFileProperties {
    pub name: String,
    pub path: String,
    pub file_type: SftpFileType,
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub permissions_symbolic: String,
    pub owner: String,
    pub group: String,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub modified_at: Option<u32>,
    pub accessed_at: Option<u32>,
    pub raw_path_token: Option<String>,
    pub symlink_target_is_directory: bool,
}

impl SftpFileProperties {
    pub fn is_directory(&self) -> bool {
        self.file_type == SftpFileType::Directory || self.symlink_target_is_directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteFilePath {
    pub display_path: String,
    pub raw_path_token: Option<String>,
}

impl RemoteFilePath {
    pub fn new(display_path: impl Into<String>) -> Self {
        Self {
            display_path: display_path.into(),
            raw_path_token: None,
        }
    }

    pub fn from_raw(display_path: impl Into<String>, raw_path: &[u8]) -> Self {
        Self {
            display_path: display_path.into(),
            raw_path_token: Some(URL_SAFE_NO_PAD.encode(raw_path)),
        }
    }

    pub fn raw_path(&self) -> anyhow::Result<Option<Vec<u8>>> {
        self.raw_path_token
            .as_deref()
            .map(|token| {
                URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|error| anyhow::anyhow!("invalid remote path token: {error}"))
            })
            .transpose()
    }

    pub fn identity_key(&self) -> String {
        self.raw_path_token
            .as_ref()
            .map(|token| format!("raw-path-token:{token}"))
            .unwrap_or_else(|| self.display_path.clone())
    }

    pub fn parent(&self) -> anyhow::Result<Self> {
        let display_path = remote_parent(&self.display_path).to_string();
        let Some(mut raw_path) = self.raw_path()? else {
            return Ok(Self::new(display_path));
        };
        while raw_path.len() > 1 && raw_path.last() == Some(&b'/') {
            raw_path.pop();
        }
        match raw_path.iter().rposition(|byte| *byte == b'/') {
            Some(0) => raw_path.truncate(1),
            Some(index) => raw_path.truncate(index),
            None => return Ok(Self::new(display_path)),
        }
        Ok(Self::from_raw(display_path, &raw_path))
    }
}

impl From<&str> for RemoteFilePath {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SftpAttributeUpdate {
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpRemoteTextFile {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub modified_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBinaryFile {
    pub path: String,
    pub content_bytes: Vec<u8>,
    pub size: u64,
    pub modified_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpWriteTextResult {
    Saved { modified_at: u64, size: u64 },
    Conflict { modified_at: u64, size: u64 },
}

#[derive(Debug, Clone, Default)]
pub struct SftpTransferControl {
    cancelled: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl SftpTransferControl {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn pause(&self) {
        if !self.is_cancelled() {
            self.paused.store(true, Ordering::Relaxed);
        }
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn check_cancelled(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            anyhow::bail!(SFTP_TRANSFER_CANCELLED);
        }
        Ok(())
    }

    pub(crate) fn wait_if_paused_blocking(&self) -> anyhow::Result<()> {
        self.check_cancelled()?;
        while self.is_paused() {
            std::thread::sleep(Duration::from_millis(100));
            self.check_cancelled()?;
        }
        Ok(())
    }

    pub(crate) async fn wait_if_paused(&self) -> anyhow::Result<()> {
        self.check_cancelled()?;
        while self.is_paused() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.check_cancelled()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SftpService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
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

struct OpenSftpSession {
    sftp: Arc<SftpSession>,
    connection: OpenSftpConnection,
}

enum OpenSftpConnection {
    Dedicated {
        handle: client::Handle<SshClientHandler>,
        jump_handles: Vec<client::Handle<SshClientHandler>>,
    },
    Multiplex,
}

async fn open_sftp_session(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
) -> anyhow::Result<OpenSftpSession> {
    open_sftp_session_with_client_config(config, multiplex, SftpClientConfig::default()).await
}

async fn open_sftp_session_with_client_config(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    client_config: SftpClientConfig,
) -> anyhow::Result<OpenSftpSession> {
    let (channel, connection) = if let Some(multiplex) = multiplex {
        multiplex.ensure_matches_config(config)?;
        let handle = multiplex.target_handle();
        let channel = open_multiplex_sftp_channel(&handle).await?;
        (channel, OpenSftpConnection::Multiplex)
    } else {
        let (handle, jump_handles) = open_authenticated_ssh_handle(config).await?;
        let channel = open_dedicated_sftp_channel(&handle).await?;
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
    Ok(OpenSftpSession {
        sftp: Arc::new(sftp),
        connection,
    })
}

async fn open_multiplex_sftp_channel(
    handle: &super::SharedSshHandle,
) -> anyhow::Result<russh::Channel<russh::client::Msg>> {
    let attempts = SFTP_CHANNEL_OPEN_RETRY_DELAYS
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
) -> anyhow::Result<russh::Channel<russh::client::Msg>> {
    let attempts = SFTP_CHANNEL_OPEN_RETRY_DELAYS
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
    let OpenSftpSession { sftp, connection } = session;
    let _ = sftp.close().await;
    close_sftp_connection(connection).await;
}

async fn close_sftp_connection(connection: OpenSftpConnection) {
    if let OpenSftpConnection::Dedicated {
        handle,
        jump_handles,
    } = connection
    {
        let _ = handle
            .disconnect(Disconnect::ByApplication, "sftp session closed", "en")
            .await;
        for jump_handle in jump_handles {
            let _ = jump_handle
                .disconnect(Disconnect::ByApplication, "sftp session closed", "en")
                .await;
        }
    }
}

fn sftp_pipeline_config(options: &SftpTransferOptions) -> (usize, usize) {
    let request_kib =
        (options.buffer_size_bytes() / 1024).clamp(SFTP_MIN_REQUEST_KIB, SFTP_MAX_REQUEST_KIB);
    let max_concurrent_writes = SFTP_WRITE_PIPELINE_TARGET_KIB
        .div_ceil(request_kib)
        .clamp(SFTP_MIN_CONCURRENT_WRITES, SFTP_MAX_CONCURRENT_WRITES);
    (request_kib, max_concurrent_writes)
}

fn sftp_payload_size(request_kib: usize) -> usize {
    (request_kib * 1024)
        .saturating_sub(SFTP_PACKET_OVERHEAD_RESERVE)
        .max(32 * 1024)
}

fn sftp_upload_buffer_size(options: &SftpTransferOptions) -> usize {
    let (request_kib, _) = sftp_pipeline_config(options);
    sftp_payload_size(request_kib)
}

fn sftp_client_config_for_options(options: &SftpTransferOptions) -> SftpClientConfig {
    let (request_kib, max_concurrent_writes) = sftp_pipeline_config(options);
    SftpClientConfig {
        max_packet_len: (request_kib * 1024) as u32,
        max_concurrent_writes,
        ..SftpClientConfig::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SftpDirectoryConcurrency {
    session_pool_size: usize,
    small_file_concurrency: usize,
    large_file_concurrency: usize,
}

fn sftp_directory_concurrency(
    max_open_handles: Option<u64>,
    options: &SftpTransferOptions,
) -> SftpDirectoryConcurrency {
    let configured_threads = options.directory_upload_threads();
    let requested_small_file_concurrency =
        if configured_threads < super::SFTP_TRANSFER_DEFAULT_DIRECTORY_UPLOAD_THREADS {
            configured_threads
        } else {
            SFTP_DEFAULT_SMALL_FILE_CONCURRENCY
        };
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
    connection: StdMutex<Option<OpenSftpConnection>>,
}

impl PooledSftpSession {
    fn from_open_session(session: OpenSftpSession) -> Self {
        Self {
            sftp: session.sftp,
            connection: StdMutex::new(Some(session.connection)),
        }
    }

    async fn close(&self) {
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
    pub fn new(config: SshSessionConfig) -> Self {
        Self {
            config,
            multiplex: None,
        }
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        Ok(Self {
            config,
            multiplex: Some(multiplex),
        })
    }

    fn run_operation<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        if !self.config.remote_file_browser_enabled() {
            return Err(anyhow::anyhow!("SFTP is disabled for this SSH profile"));
        }
        if let Some(multiplex) = self.multiplex.as_ref() {
            multiplex.block_on_after_interactive_ready(operation)
        } else {
            run_sftp_operation(operation)
        }
    }

    pub(crate) fn copy_remote_path_to(
        &self,
        source_path: &RemoteFilePath,
        destination: &Self,
        destination_path: &RemoteFilePath,
        control: SftpTransferControl,
        options: SftpTransferOptions,
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
        self.run_operation(async move {
            let source_codec = SftpPathCodec::from_ssh_config(&source_config)?;
            let destination_codec = SftpPathCodec::from_ssh_config(&destination_config)?;
            let source_raw = remote_file_path_bytes(&source_codec, &source_path)?;
            let destination_raw = remote_file_path_bytes(&destination_codec, &destination_path)?;
            let source_session =
                open_sftp_session(&source_config, source_multiplex.as_ref()).await?;
            let destination_session = match open_sftp_session(
                &destination_config,
                destination_multiplex.as_ref(),
            )
            .await
            {
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
            )
            .await;
            close_sftp_session(destination_session).await;
            close_sftp_session(source_session).await;
            result
        })
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
        let mut entries = self.run_operation(async move {
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
        self.run_operation(async move {
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
        self.run_operation(async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let raw_path = remote_file_path_bytes(&codec, &remote_path)?;
            let result = delete_remote_path_recursive_bytes(&session.sftp, raw_path).await;
            close_sftp_session(session).await;
            result
        })
    }

    pub fn create_dir_path(
        &self,
        remote_path: impl AsRef<str>,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        let remote_path = remote_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = codec.encode_path(&remote_path)?;
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
        let remote_path = remote_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = codec.encode_path(&remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let _file = session.sftp.create_bytes(remote_path_bytes.clone()).await?;
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
        self.run_operation(async move {
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
        let mut properties = self.run_operation(async move {
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
                        .metadata_bytes(remote_path_bytes)
                        .await
                        .ok()
                        .is_some_and(|target| {
                            target.file_type() == russh_sftp::protocol::FileType::Dir
                        });
                let permissions = attrs.permissions;
                Ok(SftpFileProperties {
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

    pub fn update_remote_path_attributes(
        &self,
        remote_path: &RemoteFilePath,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let uid = resolve_remote_user_value(&config, multiplex.clone(), update.owner.as_deref())?;
        let gid = resolve_remote_group_value(&config, multiplex.clone(), update.group.as_deref())?;
        let mode = update.mode;
        if mode.is_none() && uid.is_none() && gid.is_none() {
            return Ok(());
        }
        self.run_operation(async move {
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
        self.run_operation(async move {
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
                let mut file = session.sftp.open_bytes(remote_path_bytes).await?;
                let mut bytes = Vec::with_capacity(size as usize);
                file.read_to_end(&mut bytes).await?;
                file.shutdown().await?;
                ensure_remote_text_bytes(&bytes, max_bytes)?;
                let content = String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("Only UTF-8 text files are supported"))?;
                Ok(SftpRemoteTextFile {
                    path: remote_path.display_path,
                    content,
                    size,
                    modified_at: u64::from(attrs.mtime.unwrap_or(0)),
                })
            }
            .await;
            close_sftp_session(session).await;
            result
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
        self.run_operation(async move {
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
        self.run_operation(async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let remote_path_bytes = remote_file_path_bytes(&codec, &remote_path)?;
            let session = open_sftp_session(&config, multiplex.as_ref()).await?;
            let result = async {
                let original_attrs = session
                    .sftp
                    .metadata_bytes(remote_path_bytes.clone())
                    .await?;
                if !force {
                    let current_modified_at = u64::from(original_attrs.mtime.unwrap_or(0));
                    let current_size = original_attrs.size.unwrap_or(0);
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
                    if original_attrs.permissions.is_some() {
                        session
                            .sftp
                            .set_metadata_bytes(
                                temporary_path.clone(),
                                russh_sftp::protocol::FileAttributes {
                                    permissions: original_attrs.permissions,
                                    ..russh_sftp::protocol::FileAttributes::empty()
                                },
                            )
                            .await?;
                    }
                    session
                        .sftp
                        .rename_bytes(temporary_path.clone(), remote_path_bytes.clone())
                        .await?;
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
        let remote_path = remote_path.clone();
        let local_path = local_path.into();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let mut last_error = None;
            for _attempt in 0..=options.max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let codec = SftpPathCodec::from_ssh_config(&config)?;
                    let session = open_sftp_session(&config, multiplex.as_ref()).await?;
                    let bytes = download_remote_file_bytes(
                        &session.sftp,
                        remote_file_path_bytes(&codec, &remote_path)?,
                        &remote_path.display_path,
                        &local_path,
                        &control,
                        &options,
                        &mut progress,
                    )
                    .await?;
                    close_sftp_session(session).await;
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
        let remote_path = remote_path.clone();
        let local_path = local_path.into();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let mut last_error = None;
            for _attempt in 0..=path_options.transfer_options().max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let codec = SftpPathCodec::from_ssh_config(&config)?;
                    let session = open_sftp_session(&config, multiplex.as_ref()).await?;
                    let sftp = &session.sftp;
                    control.wait_if_paused().await?;
                    let raw_path = remote_file_path_bytes(&codec, &remote_path)?;
                    let metadata = sftp.metadata_bytes(raw_path.clone()).await?;
                    let is_directory = metadata.file_type() == russh_sftp::protocol::FileType::Dir;
                    let Some(local_target) = resolve_local_download_target(
                        &remote_path.display_path,
                        &local_path,
                        is_directory,
                        path_options.duplicate_policy(),
                        path_options.duplicate_resolver(),
                    )?
                    else {
                        close_sftp_session(session).await;
                        return Ok(SftpTransferSummary {
                            remote_path: remote_path.display_path.clone(),
                            local_path: local_path.clone(),
                            bytes: 0,
                            skipped: true,
                        });
                    };
                    let bytes = if is_directory {
                        download_remote_directory_bytes(
                            sftp,
                            &codec,
                            raw_path,
                            &remote_path.display_path,
                            &local_target,
                            &control,
                            &path_options,
                            &mut progress,
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
                        )
                        .await?
                    };
                    close_sftp_session(session).await;
                    Ok(SftpTransferSummary {
                        remote_path: remote_path.display_path.clone(),
                        local_path: local_target,
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
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let remote_path = resolve_remote_upload_target(&local_path, &remote_path)?;
            let mut last_error = None;
            for _attempt in 0..=options.max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let codec = SftpPathCodec::from_ssh_config(&config)?;
                    let session = open_sftp_session_with_client_config(
                        &config,
                        multiplex.as_ref(),
                        sftp_client_config_for_options(&options),
                    )
                    .await?;
                    let bytes = upload_local_file(
                        &session.sftp,
                        &codec,
                        &local_path,
                        &remote_path,
                        &control,
                        &options,
                        &mut progress,
                    )
                    .await?;
                    close_sftp_session(session).await;
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
        let local_path = local_path.into();
        let remote_path = remote_path.clone();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let mut last_error = None;
            for _attempt in 0..=options.max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let codec = SftpPathCodec::from_ssh_config(&config)?;
                    let session = open_sftp_session_with_client_config(
                        &config,
                        multiplex.as_ref(),
                        sftp_client_config_for_options(&options),
                    )
                    .await?;
                    let bytes = upload_local_file_bytes(
                        &session.sftp,
                        remote_file_path_bytes(&codec, &remote_path)?,
                        &local_path,
                        &remote_path.display_path,
                        &control,
                        &options,
                        &mut progress,
                    )
                    .await?;
                    close_sftp_session(session).await;
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
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        self.run_operation(async move {
            let metadata = tokio::fs::metadata(&local_path).await?;
            let remote_path = resolve_remote_upload_target(&local_path, &remote_path)?;
            let mut last_error = None;
            for _attempt in 0..=path_options.transfer_options().max_retries() {
                control.check_cancelled()?;
                let result = async {
                    let codec = SftpPathCodec::from_ssh_config(&config)?;
                    let session = open_sftp_session_with_client_config(
                        &config,
                        multiplex.as_ref(),
                        sftp_client_config_for_options(path_options.transfer_options()),
                    )
                    .await?;
                    let sftp = Arc::clone(&session.sftp);
                    control.wait_if_paused().await?;
                    let Some(remote_target) = resolve_remote_write_target(
                        &sftp,
                        &codec,
                        &local_path.display().to_string(),
                        &remote_path,
                        metadata.is_dir(),
                        path_options.duplicate_policy(),
                        path_options.duplicate_resolver(),
                    )
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
                        upload_local_directory(
                            Arc::clone(&sftp),
                            &codec,
                            &config,
                            multiplex.as_ref(),
                            &local_path,
                            &remote_target,
                            &control,
                            &path_options,
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

async fn delete_remote_path_recursive_bytes(
    sftp: &SftpSession,
    remote_path: Vec<u8>,
) -> anyhow::Result<()> {
    let metadata = match sftp.symlink_metadata_bytes(remote_path.clone()).await {
        Ok(metadata) => metadata,
        Err(error) => {
            let message = error.to_string().to_ascii_lowercase();
            if message.contains("no such")
                || message.contains("not found")
                || message.contains("does not exist")
            {
                return Ok(());
            }
            return Err(error.into());
        }
    };
    if metadata.file_type() == russh_sftp::protocol::FileType::Dir {
        let children = sftp
            .read_dir_bytes(remote_path.clone())
            .await?
            .filter(|entry| !matches!(entry.file_name_bytes(), b"." | b".."))
            .map(|entry| entry.path_bytes())
            .collect::<Vec<_>>();
        for child in children {
            Box::pin(delete_remote_path_recursive_bytes(sftp, child)).await?;
        }
        sftp.remove_dir_bytes(remote_path).await?;
    } else {
        sftp.remove_file_bytes(remote_path).await?;
    }
    Ok(())
}

async fn copy_remote_path_between_sftp(
    source: &SftpSession,
    source_path: Vec<u8>,
    destination: &SftpSession,
    destination_path: Vec<u8>,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
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

fn last_sftp_retry_error(last_error: Option<anyhow::Error>) -> anyhow::Error {
    last_error.unwrap_or_else(|| anyhow::anyhow!("SFTP transfer failed before starting"))
}

async fn apply_remote_default_file_mode_bytes(
    sftp: &SftpSession,
    remote_path: Vec<u8>,
    mode: Option<u32>,
) {
    let Some(mode) = mode else {
        return;
    };
    let attrs = russh_sftp::protocol::FileAttributes {
        permissions: Some(mode),
        ..russh_sftp::protocol::FileAttributes::empty()
    };
    let _ = sftp.set_metadata_bytes(remote_path, attrs).await;
}

fn preserve_local_modified_time(local_path: &Path, remote_mtime: Option<u32>) {
    let Some(remote_mtime) = remote_mtime.filter(|mtime| *mtime > 0) else {
        return;
    };
    let modified = UNIX_EPOCH + Duration::from_secs(u64::from(remote_mtime));
    if let Ok(file) = std::fs::File::open(local_path) {
        let _ = file.set_modified(modified);
    }
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

async fn download_remote_file_bytes<F>(
    sftp: &SftpSession,
    raw_path: Vec<u8>,
    remote_path: &str,
    local_path: &Path,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    control.wait_if_paused().await?;
    if let Some(parent) = local_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut remote = sftp.open_bytes(raw_path).await?;
    let remote_metadata = remote.metadata().await?;
    let total_bytes = remote_metadata.size;
    let resume_offset = transfer_resume_offset(local_path, total_bytes, options);
    let mut local = if resume_offset > 0 {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(local_path)
            .await?
    } else {
        tokio::fs::File::create(local_path).await?
    };
    let mut buffer = vec![0_u8; options.buffer_size_bytes()];
    let mut bytes = resume_offset;
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
        let read = if resume_offset > 0 {
            let data = remote.read_at(bytes, buffer.len()).await?;
            let read = data.len();
            buffer[..read].copy_from_slice(&data);
            read
        } else {
            remote.read(&mut buffer).await?
        };
        if read == 0 {
            break;
        }
        local.write_all(&buffer[..read]).await?;
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
    local.flush().await?;
    if options.preserve_timestamps {
        preserve_local_modified_time(local_path, remote_metadata.mtime);
    }
    remote.shutdown().await?;
    Ok(bytes)
}

#[expect(clippy::too_many_arguments)]
async fn download_remote_directory_bytes<F>(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    raw_path: Vec<u8>,
    remote_path: &str,
    local_path: &Path,
    control: &SftpTransferControl,
    path_options: &SftpPathTransferOptions,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    control.wait_if_paused().await?;
    tokio::fs::create_dir_all(local_path).await?;
    let (expected_bytes, item_count_total) =
        remote_directory_transfer_totals_bytes(sftp, raw_path.clone(), control).await?;
    let mut total_bytes = 0_u64;
    let mut item_count_completed = 0_u64;
    let mut pending = vec![(raw_path, remote_path.to_string(), local_path.to_path_buf())];
    while let Some((remote_dir_raw, remote_dir, local_dir)) = pending.pop() {
        control.wait_if_paused().await?;
        tokio::fs::create_dir_all(&local_dir).await?;
        for entry in sftp.read_dir_bytes(remote_dir_raw).await? {
            control.wait_if_paused().await?;
            let name = codec.decode_path_lossy(entry.file_name_bytes());
            if name == "." || name == ".." {
                continue;
            }
            let remote_child = remote_join(&remote_dir, &name);
            let remote_child_raw = entry.path_bytes();
            let local_child = local_dir.join(&name);
            match entry.file_type() {
                russh_sftp::protocol::FileType::Dir => {
                    if let Some(local_child) = resolve_local_download_target(
                        &remote_child,
                        &local_child,
                        true,
                        path_options.duplicate_policy(),
                        path_options.duplicate_resolver(),
                    )? {
                        pending.push((remote_child_raw, remote_child, local_child));
                    }
                }
                russh_sftp::protocol::FileType::File | russh_sftp::protocol::FileType::Symlink => {
                    if let Some(local_child) = resolve_local_download_target(
                        &remote_child,
                        &local_child,
                        false,
                        path_options.duplicate_policy(),
                        path_options.duplicate_resolver(),
                    )? {
                        let completed_bytes = total_bytes;
                        let mut aggregate_progress = |current| {
                            progress(directory_transfer_progress(
                                current,
                                completed_bytes,
                                expected_bytes,
                                item_count_completed,
                                item_count_total,
                            ));
                        };
                        total_bytes += download_remote_file_bytes(
                            sftp,
                            remote_child_raw,
                            &remote_child,
                            &local_child,
                            control,
                            path_options.transfer_options(),
                            &mut aggregate_progress,
                        )
                        .await?;
                    }
                    item_count_completed = item_count_completed.saturating_add(1);
                    progress(SftpTransferProgress {
                        remote_path: remote_child,
                        local_path: local_child,
                        bytes_transferred: total_bytes,
                        total_bytes: (expected_bytes > 0).then_some(expected_bytes),
                        item_count_completed: Some(item_count_completed.min(item_count_total)),
                        item_count_total: Some(item_count_total),
                    });
                }
                russh_sftp::protocol::FileType::Other => {}
            }
        }
    }
    Ok(total_bytes)
}

async fn upload_local_file<F>(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
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
        progress,
    )
    .await
}

async fn upload_local_file_bytes<F>(
    sftp: &SftpSession,
    raw_path: Vec<u8>,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    control.wait_if_paused().await?;
    let mut local = tokio::fs::File::open(local_path).await?;
    let local_metadata = local.metadata().await.ok();
    let total_bytes = local_metadata.as_ref().map(|metadata| metadata.len());
    let mut remote = sftp.create_bytes(raw_path.clone()).await?;
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
        remote.write_all(&buffer[..read]).await?;
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
    remote.flush().await?;
    remote.shutdown().await?;
    apply_remote_default_file_mode_bytes(sftp, raw_path.clone(), options.default_file_mode).await;
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

async fn upload_local_directory<F>(
    sftp: Arc<SftpSession>,
    codec: &SftpPathCodec,
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    path_options: &SftpPathTransferOptions,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    control.wait_if_paused().await?;
    let max_open_handles = sftp.max_open_handles();
    let inventory = collect_local_directory_upload_inventory(local_path, control).await?;
    let entries = plan_local_directory_upload_entries(
        &sftp,
        codec,
        remote_path,
        inventory,
        control,
        path_options,
    )
    .await?;
    upload_local_directory_entries(
        config,
        multiplex,
        *codec,
        entries,
        max_open_handles,
        control,
        path_options,
        progress,
    )
    .await
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
        let Some(remote_child) = resolve_remote_upload_write_target(
            sftp,
            codec,
            &directory.local_path.display().to_string(),
            &remote_child,
            true,
            path_options.duplicate_policy(),
            path_options.duplicate_resolver(),
        )
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
        let Some(remote_child) = resolve_remote_upload_write_target(
            sftp,
            codec,
            &file.local_path.display().to_string(),
            &remote_child,
            false,
            path_options.duplicate_policy(),
            path_options.duplicate_resolver(),
        )
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
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    codec: SftpPathCodec,
    entries: Vec<LocalDirectoryUploadEntry>,
    max_open_handles: Option<u64>,
    control: &SftpTransferControl,
    path_options: &SftpPathTransferOptions,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    let item_count_total = entries.len() as u64;
    let expected_bytes = entries
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.size));
    let concurrency = sftp_directory_concurrency(max_open_handles, path_options.transfer_options());
    let worker_count = directory_upload_worker_count(entries.len(), concurrency);
    if worker_count == 0 {
        return Ok(0);
    }
    let pool = SftpSessionPool::open(
        config,
        multiplex,
        concurrency.session_pool_size,
        sftp_client_config_for_options(path_options.transfer_options()),
    )
    .await?;

    let queue = Arc::new(StdMutex::new(VecDeque::from(entries)));
    let bytes_transferred = Arc::new(AtomicU64::new(0));
    let item_count_completed = Arc::new(AtomicU64::new(0));
    let large_lane = Arc::new(Semaphore::new(concurrency.large_file_concurrency));
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let mut workers = JoinSet::new();

    for worker_index in 0..worker_count {
        let worker_sftp = pool.session_for(worker_index);
        let worker_queue = Arc::clone(&queue);
        let worker_control = control.clone();
        let worker_options = path_options.transfer_options().clone();
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
                if control.is_paused() {
                    last_progress_snapshot = current_snapshot;
                    last_progress_at = Instant::now();
                } else if control.is_cancelled() {
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

async fn resolve_remote_upload_write_target(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    local_path: &str,
    remote_path: &str,
    is_directory: bool,
    duplicate_policy: SftpDuplicatePolicy,
    duplicate_resolver: Option<&dyn SftpDuplicateResolver>,
) -> anyhow::Result<Option<String>> {
    if !remote_upload_write_target_requires_probe(duplicate_policy) {
        return Ok(Some(remote_path.to_string()));
    }
    resolve_remote_write_target(
        sftp,
        codec,
        local_path,
        remote_path,
        is_directory,
        duplicate_policy,
        duplicate_resolver,
    )
    .await
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

async fn remote_directory_transfer_totals_bytes(
    sftp: &SftpSession,
    raw_path: Vec<u8>,
    control: &SftpTransferControl,
) -> anyhow::Result<(u64, u64)> {
    let mut total_bytes = 0_u64;
    let mut total_items = 0_u64;
    let mut pending = vec![raw_path];
    while let Some(remote_dir) = pending.pop() {
        control.wait_if_paused().await?;
        for entry in sftp.read_dir_bytes(remote_dir).await? {
            if entry.file_name_bytes() == b"." || entry.file_name_bytes() == b".." {
                continue;
            }
            match entry.file_type() {
                russh_sftp::protocol::FileType::Dir => {
                    pending.push(entry.path_bytes());
                }
                russh_sftp::protocol::FileType::File | russh_sftp::protocol::FileType::Symlink => {
                    total_items = total_items.saturating_add(1);
                    total_bytes = total_bytes.saturating_add(entry.metadata().size.unwrap_or(0));
                }
                russh_sftp::protocol::FileType::Other => {}
            }
        }
    }
    Ok((total_bytes, total_items))
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
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use crate::{SftpSettings, SshSessionConfig};

    use super::{
        DirectoryUploadProgressSnapshot, RemoteFilePath, SFTP_DIRECTORY_STALL_TIMEOUT,
        SFTP_SMALL_FILE_THRESHOLD, SFTP_TRANSFER_CANCELLED, SftpDirectoryConcurrency,
        SftpDuplicatePolicy, SftpFileEntry, SftpFileType, SftpPathCodec, SftpTransferControl,
        SftpTransferDirection, SftpTransferOptions, SftpTransferProgress,
        collect_local_directory_upload_inventory, directory_transfer_progress,
        directory_upload_aggregate_progress, directory_upload_stalled,
        directory_upload_worker_count, is_sftp_large_file, is_sftp_transfer_cancelled,
        remote_conflict_candidate, remote_join, remote_join_bytes,
        remote_upload_write_target_requires_probe, resolve_duplicate_decision,
        resolve_local_download_target, resolve_remote_upload_target,
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
        assert!(
            error
                .to_string()
                .contains("Unsupported SFTP filename encoding")
        );
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
    fn directory_upload_worker_count_uses_file_count_and_configured_limit() {
        let default_options = SftpTransferOptions::default();
        let default_concurrency = sftp_directory_concurrency(None, &default_options);
        assert_eq!(directory_upload_worker_count(0, default_concurrency), 0);
        assert_eq!(directory_upload_worker_count(1, default_concurrency), 1);
        assert_eq!(directory_upload_worker_count(10, default_concurrency), 10);
        assert_eq!(directory_upload_worker_count(20, default_concurrency), 16);

        let two_workers = SftpTransferOptions::default().with_directory_upload_threads(2);
        let two_worker_concurrency = sftp_directory_concurrency(None, &two_workers);
        assert_eq!(directory_upload_worker_count(10, two_worker_concurrency), 2);

        let capped = SftpTransferOptions::default().with_directory_upload_threads(99);
        let capped_concurrency = sftp_directory_concurrency(None, &capped);
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
            sftp_directory_concurrency(None, &default_options),
            SftpDirectoryConcurrency {
                session_pool_size: 2,
                small_file_concurrency: 16,
                large_file_concurrency: 2,
            }
        );
        assert_eq!(
            sftp_directory_concurrency(Some(10), &default_options),
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
            )
            .small_file_concurrency,
            1
        );
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
        let dir =
            std::env::temp_dir().join(format!("nyaterm-upload-inventory-{}", uuid::Uuid::new_v4()));
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

        std::fs::remove_dir_all(&dir).expect("cleanup");
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
    fn transfer_resume_offset_requires_partial_local_file() {
        let dir =
            std::env::temp_dir().join(format!("nyaterm-resume-test-{}", uuid::Uuid::new_v4()));
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

        let _ = std::fs::remove_dir_all(dir);
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
        let dir = std::env::temp_dir().join(format!("nyaterm-sftp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("archive.tar.gz");
        std::fs::write(&target, b"existing").expect("target");

        assert_eq!(
            resolve_local_download_target(
                "/remote/archive.tar.gz",
                &target,
                false,
                SftpDuplicatePolicy::Skip,
                None,
            )
            .expect("skip"),
            None
        );
        assert_eq!(
            resolve_local_download_target(
                "/remote/archive.tar.gz",
                &target,
                false,
                SftpDuplicatePolicy::Rename,
                None,
            )
            .expect("rename"),
            Some(dir.join("archive.tar(1).gz"))
        );

        std::fs::remove_dir_all(&dir).expect("cleanup");
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
