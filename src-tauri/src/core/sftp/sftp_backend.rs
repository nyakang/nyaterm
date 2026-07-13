//! SFTP backend: standard SSH SFTP subsystem via russh-sftp.
//!
//! This is the preferred backend when the server supports `Subsystem sftp`.

use super::traits::RemoteFs;
use super::transfer::*;
use super::util::*;
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use crate::observability::{StructuredLog, StructuredLogLevel, log_event};
use russh::{ChannelMsg, ChannelOpenFailure};
use russh_sftp::client::{Config as SftpClientConfig, SftpSession, error::Error as SftpError};
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

const SFTP_MIN_REQUEST_KIB: usize = 64;
const SFTP_MAX_REQUEST_KIB: usize = 256;
const SFTP_PIPELINE_TARGET_KIB: usize = 1024;
const SFTP_WRITE_PIPELINE_TARGET_KIB: usize = 2048;
const SFTP_MIN_PIPELINE_DEPTH: usize = 4;
const SFTP_MAX_PIPELINE_DEPTH: usize = 16;
const SFTP_MIN_CONCURRENT_WRITES: usize = 8;
const SFTP_MAX_CONCURRENT_WRITES: usize = 16;
const SFTP_PACKET_OVERHEAD_RESERVE: usize = 1024;
const TRANSFER_PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const SFTP_SMALL_FILE_THRESHOLD: u64 = 512 * 1024;
const SFTP_DEFAULT_SMALL_FILE_CONCURRENCY: usize = 64;
const SFTP_MAX_SMALL_FILE_CONCURRENCY: usize = 256;
const SFTP_DEFAULT_SESSION_POOL_SIZE: usize = 2;
const SFTP_MAX_SESSION_POOL_SIZE: usize = 4;
const SFTP_LARGE_FILE_CONCURRENCY: usize = 2;
const SFTP_HANDLE_RESERVE: usize = 8;
const SFTP_CHANNEL_OPEN_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(150),
    Duration::from_millis(300),
];

fn sftp_pipeline_config(ts: &crate::config::TransferSettings) -> (usize, usize, usize) {
    let request_kib =
        (ts.transfer_buffer_size as usize).clamp(SFTP_MIN_REQUEST_KIB, SFTP_MAX_REQUEST_KIB);
    let pipeline_depth = SFTP_PIPELINE_TARGET_KIB
        .div_ceil(request_kib)
        .clamp(SFTP_MIN_PIPELINE_DEPTH, SFTP_MAX_PIPELINE_DEPTH);
    let max_concurrent_writes = SFTP_WRITE_PIPELINE_TARGET_KIB
        .div_ceil(request_kib)
        .clamp(SFTP_MIN_CONCURRENT_WRITES, SFTP_MAX_CONCURRENT_WRITES);
    (request_kib, pipeline_depth, max_concurrent_writes)
}

fn sftp_client_config(request_kib: usize, max_concurrent_writes: usize) -> SftpClientConfig {
    SftpClientConfig {
        max_packet_len: (request_kib * 1024) as u32,
        max_concurrent_writes,
        ..SftpClientConfig::default()
    }
}

fn is_sftp_not_found(error: &SftpError) -> bool {
    matches!(
        error,
        SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile
    )
}

fn ignore_sftp_not_found(result: Result<(), SftpError>) -> AppResult<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_sftp_not_found(&error) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn is_retryable_sftp_channel_open_error(error: &russh::Error) -> bool {
    matches!(
        error,
        russh::Error::ChannelOpenFailure(
            ChannelOpenFailure::ConnectFailed | ChannelOpenFailure::ResourceShortage
        )
    )
}

fn sftp_remove_error(path: &str, kind: &str, error: SftpError) -> Option<String> {
    if is_sftp_not_found(&error) {
        None
    } else {
        Some(format!("Failed to remove {kind} '{}': {error}", path))
    }
}

fn sftp_payload_size(request_kib: usize) -> usize {
    (request_kib * 1024)
        .saturating_sub(SFTP_PACKET_OVERHEAD_RESERVE)
        .max(32 * 1024)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SftpDirectoryConcurrency {
    session_pool_size: usize,
    small_file_concurrency: usize,
    large_file_concurrency: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadReadProgress {
    Continue(u64),
    Complete,
}

fn unexpected_download_eof_error(
    remote_path: &str,
    local_path: &str,
    remote_size: u64,
    bytes_written: u64,
    request_kib: usize,
    payload_bytes: usize,
) -> AppError {
    AppError::Channel(format!(
        "Unexpected EOF while downloading {remote_path}: expected {remote_size} bytes, got {bytes_written} bytes (local_path={local_path}, request_kib={request_kib}, payload_bytes={payload_bytes})"
    ))
}

fn classify_download_read_progress(
    remote_path: &str,
    local_path: &str,
    remote_size: u64,
    offset: u64,
    bytes_written: u64,
    bytes_read: usize,
    request_kib: usize,
    payload_bytes: usize,
) -> AppResult<DownloadReadProgress> {
    if offset >= remote_size {
        return Ok(DownloadReadProgress::Complete);
    }

    if bytes_read == 0 {
        return Err(unexpected_download_eof_error(
            remote_path,
            local_path,
            remote_size,
            bytes_written,
            request_kib,
            payload_bytes,
        ));
    }

    let next_offset = offset.saturating_add(bytes_read as u64);
    if next_offset >= remote_size {
        Ok(DownloadReadProgress::Complete)
    } else {
        Ok(DownloadReadProgress::Continue(next_offset))
    }
}

fn ensure_download_complete(
    remote_path: &str,
    local_path: &str,
    remote_size: u64,
    bytes_written: u64,
    request_kib: usize,
    payload_bytes: usize,
) -> AppResult<()> {
    if bytes_written == remote_size {
        Ok(())
    } else {
        Err(unexpected_download_eof_error(
            remote_path,
            local_path,
            remote_size,
            bytes_written,
            request_kib,
            payload_bytes,
        ))
    }
}

fn sftp_directory_concurrency(max_open_handles: Option<u64>) -> SftpDirectoryConcurrency {
    let small_file_concurrency = match max_open_handles {
        Some(handles) => (handles.saturating_sub(SFTP_HANDLE_RESERVE as u64) as usize)
            .clamp(1, SFTP_MAX_SMALL_FILE_CONCURRENCY),
        None => SFTP_DEFAULT_SMALL_FILE_CONCURRENCY,
    };
    let large_file_concurrency = SFTP_LARGE_FILE_CONCURRENCY
        .min(small_file_concurrency)
        .max(1);
    let session_pool_size = SFTP_DEFAULT_SESSION_POOL_SIZE
        .min(SFTP_MAX_SESSION_POOL_SIZE)
        .min(small_file_concurrency)
        .max(1);

    SftpDirectoryConcurrency {
        session_pool_size,
        small_file_concurrency,
        large_file_concurrency,
    }
}

#[derive(Clone)]
struct SftpSessionPool {
    sessions: Arc<Vec<Arc<ManagedSftpSession>>>,
}

impl SftpSessionPool {
    async fn new(backend: &SftpBackend, size: usize, config: SftpClientConfig) -> AppResult<Self> {
        let mut sessions = Vec::with_capacity(size);
        for _ in 0..size {
            sessions.push(Arc::new(
                backend.open_sftp_with_client_config(config.clone()).await?,
            ));
        }
        Ok(Self {
            sessions: Arc::new(sessions),
        })
    }

    fn session_for(&self, index: usize) -> Arc<ManagedSftpSession> {
        self.sessions[index % self.sessions.len()].clone()
    }

    async fn close_all(self) {
        for session in self.sessions.iter() {
            let _ = session.close().await;
        }
    }
}

struct ManagedSftpSession {
    inner: SftpSession,
    _permit: OwnedSemaphorePermit,
}

impl ManagedSftpSession {
    fn new(inner: SftpSession, permit: OwnedSemaphorePermit) -> Self {
        Self {
            inner,
            _permit: permit,
        }
    }
}

impl Deref for ManagedSftpSession {
    type Target = SftpSession;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

fn log_transfer_performance(
    direction: &str,
    kind: &str,
    bytes: u64,
    elapsed: Duration,
    request_kib: usize,
    pipeline_depth: usize,
    max_concurrent_writes: usize,
    concurrent_tasks: usize,
) {
    let elapsed_secs = elapsed.as_secs_f64().max(0.001);
    let mbps = bytes as f64 / 1024.0 / 1024.0 / elapsed_secs;
    log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "transfer.performance".to_string(),
        event: "sftp.transfer.completed".to_string(),
        message: "SFTP transfer performance summary".to_string(),
        ids: None,
        data: Some(serde_json::json!({
            "backend": "sftp",
            "direction": direction,
            "kind": kind,
            "bytes": bytes,
            "elapsed_ms": elapsed.as_millis(),
            "average_mbps": mbps,
            "request_kib": request_kib,
            "payload_bytes": sftp_payload_size(request_kib),
            "pipeline_depth": pipeline_depth,
            "max_concurrent_writes": max_concurrent_writes,
            "concurrent_tasks": concurrent_tasks,
        })),
        error: None,
        client_timestamp: None,
    });
}

#[derive(Clone)]
pub(crate) struct SftpBackend {
    ssh_handle: Arc<SshConnectionHandles>,
    identity_cache: Arc<RwLock<RemoteIdentityCache>>,
}

#[derive(Default)]
struct RemoteIdentityCache {
    users_by_uid: HashMap<u32, String>,
    groups_by_gid: HashMap<u32, String>,
    uids_by_user: HashMap<String, u32>,
    gids_by_group: HashMap<String, u32>,
}

struct ExecResult {
    exit_code: u32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl SftpBackend {
    pub(crate) fn new(ssh_handle: Arc<SshConnectionHandles>) -> Self {
        Self {
            ssh_handle,
            identity_cache: Arc::new(RwLock::new(RemoteIdentityCache::default())),
        }
    }

    /// Attempt to open a throwaway SFTP session to verify subsystem availability.
    pub(crate) async fn probe(ssh_handle: &Arc<SshConnectionHandles>) -> AppResult<()> {
        let sftp = Self::open_sftp_raw(ssh_handle.clone(), SftpClientConfig::default()).await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn open_sftp_raw(
        ssh_handle: Arc<SshConnectionHandles>,
        config: SftpClientConfig,
    ) -> AppResult<ManagedSftpSession> {
        for attempt in 0..=SFTP_CHANNEL_OPEN_RETRY_DELAYS.len() {
            let permit = ssh_handle.acquire_sftp_channel_permit().await?;
            let channel_result = {
                let handle_mtx = ssh_handle.target_handle();
                let handle = handle_mtx.lock().await;
                // Use a short timeout to avoid hanging when the server
                // doesn't support multiple channels.  If the server drops
                // the connection on channel open, this will fail quickly
                // and we can return an error without affecting the shell
                // session (though the transport may already be dead).
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    handle.channel_open_session(),
                ).await {
                    Ok(result) => result,
                    Err(_) => {
                        drop(permit);
                        return Err(AppError::Channel(
                            "Timed out waiting for SFTP channel open".to_string(),
                        ));
                    }
                }
            };

            let channel = match channel_result {
                Ok(channel) => channel,
                Err(error)
                    if attempt < SFTP_CHANNEL_OPEN_RETRY_DELAYS.len()
                        && is_retryable_sftp_channel_open_error(&error) =>
                {
                    drop(permit);
                    tokio::time::sleep(SFTP_CHANNEL_OPEN_RETRY_DELAYS[attempt]).await;
                    continue;
                }
                Err(error) => {
                    drop(permit);
                    return Err(AppError::Channel(format!(
                        "Failed to open SFTP channel: {}",
                        error
                    )));
                }
            };

            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|e| AppError::Channel(format!("Failed to start SFTP subsystem: {}", e)))?;

            let sftp = SftpSession::new_with_config(channel.into_stream(), config).await?;
            return Ok(ManagedSftpSession::new(sftp, permit));
        }

        unreachable!("SFTP channel open retry loop always returns or continues");
    }

    async fn open_sftp(&self) -> AppResult<ManagedSftpSession> {
        Self::open_sftp_raw(self.ssh_handle.clone(), SftpClientConfig::default()).await
    }

    async fn open_sftp_with_client_config(
        &self,
        config: SftpClientConfig,
    ) -> AppResult<ManagedSftpSession> {
        Self::open_sftp_raw(self.ssh_handle.clone(), config).await
    }

    async fn exec(&self, command: &str) -> AppResult<ExecResult> {
        let handle_mtx = self.ssh_handle.target_handle();
        let mut channel = {
            let handle = handle_mtx.lock().await;
            handle
                .channel_open_session()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open exec channel: {}", e)))?
        };

        channel.exec(true, command.as_bytes()).await?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<u32> = None;

        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    stdout.extend_from_slice(&data);
                }
                Some(ChannelMsg::ExtendedData { data, ext }) => {
                    if ext == 1 {
                        stderr.extend_from_slice(&data);
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = Some(exit_status);
                }
                Some(ChannelMsg::Eof) | None => {
                    if exit_code.is_none() {
                        if let Some(ChannelMsg::ExitStatus { exit_status }) = channel.wait().await {
                            exit_code = Some(exit_status);
                        }
                    }
                    break;
                }
                _ => {}
            }
        }

        Ok(ExecResult {
            exit_code: exit_code.unwrap_or(255),
            stdout,
            stderr,
        })
    }

    async fn exec_ok(&self, command: &str) -> AppResult<Vec<u8>> {
        let result = self.exec(command).await?;
        if result.exit_code != 0 {
            let msg = String::from_utf8_lossy(&result.stderr);
            return Err(AppError::Channel(format!(
                "Remote command failed (exit {}): {}",
                result.exit_code,
                msg.trim()
            )));
        }
        Ok(result.stdout)
    }

    async fn resolve_uid_names(&self, uids: HashSet<u32>) -> HashMap<u32, String> {
        let missing: Vec<u32> = {
            let cache = self.identity_cache.read().await;
            uids.iter()
                .copied()
                .filter(|uid| !cache.users_by_uid.contains_key(uid))
                .collect()
        };
        if missing.is_empty() {
            let cache = self.identity_cache.read().await;
            return uids
                .into_iter()
                .filter_map(|uid| cache.users_by_uid.get(&uid).map(|name| (uid, name.clone())))
                .collect();
        }

        let id_list = missing
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "ids={}; for id in $ids; do name=$(getent passwd \"$id\" 2>/dev/null | cut -d: -f1); if [ -z \"$name\" ] && [ -r /etc/passwd ]; then name=$(awk -F: -v id=\"$id\" '$3==id {{print $1; exit}}' /etc/passwd 2>/dev/null); fi; if [ -n \"$name\" ]; then printf '%s:%s\\n' \"$id\" \"$name\"; fi; done",
            sh_quote(&id_list)
        );

        let mut resolved = HashMap::new();
        if let Ok(output) = self.exec_ok(&command).await {
            for line in String::from_utf8_lossy(&output).lines() {
                if let Some((id, name)) = line.split_once(':') {
                    if let Ok(uid) = id.parse::<u32>() {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            resolved.insert(uid, trimmed.to_string());
                        }
                    }
                }
            }
        }

        let mut cache = self.identity_cache.write().await;
        for (uid, name) in &resolved {
            cache.users_by_uid.insert(*uid, name.clone());
            cache.uids_by_user.insert(name.clone(), *uid);
        }
        uids.into_iter()
            .filter_map(|uid| cache.users_by_uid.get(&uid).map(|name| (uid, name.clone())))
            .collect()
    }

    async fn resolve_gid_names(&self, gids: HashSet<u32>) -> HashMap<u32, String> {
        let missing: Vec<u32> = {
            let cache = self.identity_cache.read().await;
            gids.iter()
                .copied()
                .filter(|gid| !cache.groups_by_gid.contains_key(gid))
                .collect()
        };
        if missing.is_empty() {
            let cache = self.identity_cache.read().await;
            return gids
                .into_iter()
                .filter_map(|gid| {
                    cache
                        .groups_by_gid
                        .get(&gid)
                        .map(|name| (gid, name.clone()))
                })
                .collect();
        }

        let id_list = missing
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "ids={}; for id in $ids; do name=$(getent group \"$id\" 2>/dev/null | cut -d: -f1); if [ -z \"$name\" ] && [ -r /etc/group ]; then name=$(awk -F: -v id=\"$id\" '$3==id {{print $1; exit}}' /etc/group 2>/dev/null); fi; if [ -n \"$name\" ]; then printf '%s:%s\\n' \"$id\" \"$name\"; fi; done",
            sh_quote(&id_list)
        );

        let mut resolved = HashMap::new();
        if let Ok(output) = self.exec_ok(&command).await {
            for line in String::from_utf8_lossy(&output).lines() {
                if let Some((id, name)) = line.split_once(':') {
                    if let Ok(gid) = id.parse::<u32>() {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            resolved.insert(gid, trimmed.to_string());
                        }
                    }
                }
            }
        }

        let mut cache = self.identity_cache.write().await;
        for (gid, name) in &resolved {
            cache.groups_by_gid.insert(*gid, name.clone());
            cache.gids_by_group.insert(name.clone(), *gid);
        }
        gids.into_iter()
            .filter_map(|gid| {
                cache
                    .groups_by_gid
                    .get(&gid)
                    .map(|name| (gid, name.clone()))
            })
            .collect()
    }

    async fn resolve_user_to_uid(&self, owner: &str) -> AppResult<u32> {
        if let Ok(uid) = owner.parse::<u32>() {
            return Ok(uid);
        }
        if let Some(uid) = self
            .identity_cache
            .read()
            .await
            .uids_by_user
            .get(owner)
            .copied()
        {
            return Ok(uid);
        }
        let command = format!(
            "name={}; id=$(getent passwd \"$name\" 2>/dev/null | cut -d: -f3); if [ -z \"$id\" ] && [ -r /etc/passwd ]; then id=$(awk -F: -v name=\"$name\" '$1==name {{print $3; exit}}' /etc/passwd 2>/dev/null); fi; [ -n \"$id\" ] && printf '%s\\n' \"$id\"",
            sh_quote(owner)
        );
        let output = self.exec_ok(&command).await?;
        let text = String::from_utf8_lossy(&output);
        let uid = text
            .trim()
            .parse::<u32>()
            .map_err(|_| AppError::Channel(format!("Failed to resolve remote user '{}'", owner)))?;
        let mut cache = self.identity_cache.write().await;
        cache.uids_by_user.insert(owner.to_string(), uid);
        cache.users_by_uid.insert(uid, owner.to_string());
        Ok(uid)
    }

    async fn resolve_group_to_gid(&self, group: &str) -> AppResult<u32> {
        if let Ok(gid) = group.parse::<u32>() {
            return Ok(gid);
        }
        if let Some(gid) = self
            .identity_cache
            .read()
            .await
            .gids_by_group
            .get(group)
            .copied()
        {
            return Ok(gid);
        }
        let command = format!(
            "name={}; id=$(getent group \"$name\" 2>/dev/null | cut -d: -f3); if [ -z \"$id\" ] && [ -r /etc/group ]; then id=$(awk -F: -v name=\"$name\" '$1==name {{print $3; exit}}' /etc/group 2>/dev/null); fi; [ -n \"$id\" ] && printf '%s\\n' \"$id\"",
            sh_quote(group)
        );
        let output = self.exec_ok(&command).await?;
        let text = String::from_utf8_lossy(&output);
        let gid = text.trim().parse::<u32>().map_err(|_| {
            AppError::Channel(format!("Failed to resolve remote group '{}'", group))
        })?;
        let mut cache = self.identity_cache.write().await;
        cache.gids_by_group.insert(group.to_string(), gid);
        cache.groups_by_gid.insert(gid, group.to_string());
        Ok(gid)
    }
}

fn sftp_attrs_is_dir(attrs: &FileAttributes) -> bool {
    attrs.permissions.map_or(false, |permissions| {
        (permissions & SFTP_FILE_TYPE_MASK) == 0o040000
    })
}

fn sftp_attrs_is_symlink(attrs: &FileAttributes) -> bool {
    attrs.permissions.map_or(false, |permissions| {
        (permissions & SFTP_FILE_TYPE_MASK) == 0o120000
    })
}

fn normalize_remote_dir_path(path: &str) -> &str {
    if path == "/" {
        "/"
    } else {
        path.trim_end_matches('/')
    }
}

fn join_remote_child(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn is_safe_recursive_remove_target(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() || matches!(trimmed, "/" | "." | "..") {
        return false;
    }

    let normalized = normalize_remote_dir_path(trimmed);
    !normalized.is_empty()
        && !matches!(normalized, "/" | "." | "..")
        && !normalized.split('/').any(|part| part == "..")
}

async fn resolve_remote_path(
    app: &tauri::AppHandle,
    session_manager: &crate::core::SessionManager,
    sftp: &SftpSession,
    session_id: &str,
    remote_path: &str,
    strategy: &str,
) -> Option<String> {
    let exists = sftp.metadata(remote_path).await.is_ok();
    if !exists {
        return Some(remote_path.to_string());
    }
    let file_name = remote_path.split('/').last().unwrap_or(remote_path);
    let is_directory = sftp
        .metadata(remote_path)
        .await
        .map(|attrs| sftp_attrs_is_dir(&attrs))
        .unwrap_or(false);
    match strategy {
        "skip" => None,
        "rename" => {
            let path = std::path::Path::new(remote_path);
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let ext = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let parent = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            for i in 1..=999 {
                let candidate = format!("{}/{}({}){}", parent.trim_end_matches('/'), stem, i, ext);
                if sftp.metadata(&candidate).await.is_err() {
                    return Some(candidate);
                }
            }
            Some(remote_path.to_string())
        }
        "ask" => {
            match super::duplicate::prompt_duplicate_choice(
                app,
                session_manager,
                session_id,
                remote_path,
                file_name,
                is_directory,
            )
            .await
            {
                Ok(super::duplicate::DuplicateChoice::Skip) => None,
                Ok(super::duplicate::DuplicateChoice::Overwrite) => Some(remote_path.to_string()),
                Err(_) => None,
            }
        }
        _ => Some(remote_path.to_string()),
    }
}

async fn ensure_remote_upload_target_allowed(
    app: &tauri::AppHandle,
    session_manager: &crate::core::SessionManager,
    sftp: &SftpSession,
    session_id: &str,
    remote_path: &str,
    strategy: &str,
) -> bool {
    let exists = sftp.metadata(remote_path).await.is_ok();
    if !exists {
        return true;
    }

    let file_name = file_name_from_path(remote_path);
    let is_directory = sftp
        .metadata(remote_path)
        .await
        .map(|attrs| sftp_attrs_is_dir(&attrs))
        .unwrap_or(false);

    match strategy {
        "skip" => false,
        "rename" => true,
        "ask" => {
            matches!(
                super::duplicate::prompt_duplicate_choice(
                    app,
                    session_manager,
                    session_id,
                    remote_path,
                    &file_name,
                    is_directory,
                )
                .await,
                Ok(super::duplicate::DuplicateChoice::Overwrite)
            )
        }
        _ => true,
    }
}

async fn apply_remote_mode(sftp: &SftpSession, path: &str, requested_mode: u32) -> AppResult<()> {
    let original_attrs = sftp.metadata(path).await?;
    let original_permissions = original_attrs.permissions;
    let requested_permissions = requested_mode & POSIX_MODE_MASK;

    let mut attrs = FileAttributes::empty();
    attrs.permissions = Some(requested_permissions);
    sftp.set_metadata(path, attrs).await.map_err(|error| {
        tracing::warn!(
            remote_path = path,
            original_permissions = %describe_permissions(original_permissions),
            requested_permissions = format!("{requested_permissions:#06o}"),
            error = %error,
            "Failed to update remote permissions with a permissions-only SETSTAT payload"
        );
        AppError::from(error)
    })?;

    let actual_permissions = sftp
        .metadata(path)
        .await
        .ok()
        .and_then(|attrs| attrs.permissions);
    tracing::debug!(
        target: "user_action",
        action = "chmod",
        remote_path = path,
        original_permissions = %describe_permissions(original_permissions),
        requested_permissions = format!("{requested_permissions:#06o}"),
        actual_permissions = %describe_permissions(actual_permissions),
        "Applied remote permissions"
    );

    Ok(())
}

async fn apply_remote_attrs(
    sftp: &SftpSession,
    path: &str,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
) -> AppResult<()> {
    let original_attrs = sftp.symlink_metadata(path).await?;
    let mut attrs = FileAttributes::empty();
    if let Some(mode) = mode {
        let type_bits = original_attrs.permissions.unwrap_or(0) & SFTP_FILE_TYPE_MASK;
        attrs.permissions = Some(type_bits | (mode & POSIX_MODE_MASK));
    }
    if uid.is_some() || gid.is_some() {
        let effective_uid = uid.or(original_attrs.uid);
        let effective_gid = gid.or(original_attrs.gid);
        match (effective_uid, effective_gid) {
            (Some(effective_uid), Some(effective_gid)) => {
                attrs.uid = Some(effective_uid);
                attrs.gid = Some(effective_gid);
            }
            _ => {
                return Err(AppError::Channel(
                    "Cannot update SFTP ownership because the server did not provide the current UID/GID; set both owner and group."
                        .to_string(),
                ));
            }
        }
    }
    if attrs.permissions.is_none() && attrs.uid.is_none() && attrs.gid.is_none() {
        return Ok(());
    }

    sftp.set_metadata(path, attrs).await.map_err(|error| {
        tracing::warn!(
            remote_path = path,
            requested_mode = ?mode,
            requested_uid = ?uid,
            requested_gid = ?gid,
            error = %error,
            "Failed to update remote file attributes"
        );
        AppError::from(error)
    })?;

    Ok(())
}

async fn apply_remote_attrs_recursive(
    sftp: &SftpSession,
    path: &str,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
) -> AppResult<()> {
    let path = normalize_remote_dir_path(path);
    let meta = sftp.symlink_metadata(path).await?;
    let is_dir = sftp_attrs_is_dir(&meta);
    let is_symlink = sftp_attrs_is_symlink(&meta);

    apply_remote_attrs(sftp, path, mode, uid, gid).await?;

    if !is_dir || is_symlink {
        return Ok(());
    }

    let dir = sftp.read_dir(path).await?;
    let mut errors: Vec<String> = Vec::new();
    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child = join_remote_child(path, &name);
        let attrs = entry.metadata();
        if sftp_attrs_is_dir(&attrs) && !sftp_attrs_is_symlink(&attrs) {
            if let Err(error) =
                Box::pin(apply_remote_attrs_recursive(sftp, &child, mode, uid, gid)).await
            {
                errors.push(error.to_string());
            }
        } else if let Err(error) = apply_remote_attrs(sftp, &child, mode, uid, gid).await {
            errors.push(format!("'{}': {}", child, error));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Channel(format!(
            "{} item(s) could not be updated:\n{}",
            errors.len(),
            errors.join("\n")
        )))
    }
}

async fn apply_remote_mode_after_create(
    sftp: &SftpSession,
    path: &str,
    mode: &str,
    item_kind: &str,
) -> AppResult<()> {
    let requested_mode = parse_octal_mode(mode)?;

    match apply_remote_mode(sftp, path, requested_mode).await {
        Ok(()) => Ok(()),
        Err(error) => {
            if sftp.metadata(path).await.is_ok() {
                tracing::warn!(
                    remote_path = path,
                    requested_mode = mode,
                    item_kind = %item_kind,
                    error = %error,
                    "Remote item created, but failed to apply requested permissions"
                );
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

async fn cleanup_cancelled_upload(backend: &SftpBackend, remote_path: &str) -> AppResult<()> {
    let sftp = backend.open_sftp().await?;
    if sftp.metadata(remote_path).await.is_ok() {
        let _ = sftp.remove_file(remote_path).await;
    }
    let _ = sftp.close().await;
    Ok(())
}

async fn read_sftp_chunk(
    remote_file: russh_sftp::client::fs::File,
    offset: u64,
    len: usize,
    remote_path: String,
    remote_size: u64,
    payload_bytes: usize,
) -> AppResult<(u64, Vec<u8>, bool, russh_sftp::client::fs::File)> {
    let mut data = Vec::with_capacity(len);
    let mut read_offset = offset;
    let end_offset = offset.saturating_add(len as u64).min(remote_size);
    let mut completed_range = true;

    while read_offset < end_offset {
        let remaining = (end_offset - read_offset) as usize;
        let chunk = remote_file
            .read_at(read_offset, remaining.min(payload_bytes))
            .await
            .map_err(|e| {
                AppError::Channel(format!(
                    "SFTP read failed for {remote_path} at offset {read_offset}: {e}"
                ))
            })?;
        if chunk.is_empty() {
            completed_range = false;
            break;
        }
        read_offset = match classify_download_read_progress(
            &remote_path,
            "",
            end_offset,
            read_offset,
            data.len() as u64,
            chunk.len(),
            0,
            payload_bytes,
        )? {
            DownloadReadProgress::Continue(next_offset) => next_offset,
            DownloadReadProgress::Complete => end_offset,
        };
        data.extend_from_slice(&chunk);
    }

    Ok((offset, data, completed_range, remote_file))
}

async fn download_known_size_to_local_file<F, G>(
    sftp: &SftpSession,
    remote_path: &str,
    local_path: &str,
    local_file: &mut tokio::fs::File,
    total_size: u64,
    request_kib: usize,
    pipeline_depth: usize,
    max_pipeline_depth: usize,
    controller: &Arc<TransferController>,
    parent_controller: Option<&Arc<TransferController>>,
    mut on_bytes: F,
    mut on_progress_interval: G,
) -> AppResult<u64>
where
    F: FnMut(u64, u64),
    G: FnMut(u64),
{
    use std::io::SeekFrom;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    use tokio::task::JoinSet;

    if total_size == 0 {
        ensure_download_complete(
            remote_path,
            local_path,
            total_size,
            0,
            request_kib,
            sftp_payload_size(request_kib),
        )?;
        return Ok(0);
    }

    let chunk_size = sftp_payload_size(request_kib) as u64;
    let num_chunks = total_size.div_ceil(chunk_size) as usize;
    let concurrency = pipeline_depth
        .min(max_pipeline_depth.max(1))
        .min(num_chunks);

    let mut handle_pool: Vec<russh_sftp::client::fs::File> = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        handle_pool.push(
            sftp.open(remote_path)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?,
        );
    }

    type Task = AppResult<(u64, Vec<u8>, bool, russh_sftp::client::fs::File)>;
    let mut join_set: JoinSet<Task> = JoinSet::new();
    let mut next_offset: u64 = 0;
    let mut last_progress = Instant::now();
    let mut bytes_transferred: u64 = 0;

    while let Some(fh) = handle_pool.pop() {
        if next_offset >= total_size {
            break;
        }
        wait_for_transfer_chain(controller, parent_controller).await?;
        let len = chunk_size.min(total_size - next_offset) as usize;
        let offset = next_offset;
        next_offset += len as u64;
        join_set.spawn(read_sftp_chunk(
            fh,
            offset,
            len,
            remote_path.to_string(),
            total_size,
            chunk_size as usize,
        ));
    }

    while let Some(res) = join_set.join_next().await {
        wait_for_transfer_chain(controller, parent_controller).await?;
        let (chunk_offset, data, completed_range, fh) =
            res.map_err(|e| AppError::Channel(format!("Task panicked: {}", e)))??;

        if !data.is_empty() {
            local_file
                .seek(SeekFrom::Start(chunk_offset))
                .await
                .map_err(|e| AppError::Channel(format!("Local seek failed: {}", e)))?;
            local_file
                .write_all(&data)
                .await
                .map_err(|e| AppError::Channel(format!("Local write failed: {}", e)))?;

            let delta = data.len() as u64;
            bytes_transferred = bytes_transferred.saturating_add(delta);
            on_bytes(bytes_transferred, delta);
        }

        if !completed_range {
            return Err(unexpected_download_eof_error(
                remote_path,
                local_path,
                total_size,
                bytes_transferred,
                request_kib,
                chunk_size as usize,
            ));
        }

        if next_offset < total_size {
            wait_for_transfer_chain(controller, parent_controller).await?;
            let len = chunk_size.min(total_size - next_offset) as usize;
            let offset = next_offset;
            next_offset += len as u64;
            join_set.spawn(read_sftp_chunk(
                fh,
                offset,
                len,
                remote_path.to_string(),
                total_size,
                chunk_size as usize,
            ));
        }

        if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
            last_progress = Instant::now();
            on_progress_interval(bytes_transferred);
        }
    }

    ensure_download_complete(
        remote_path,
        local_path,
        total_size,
        bytes_transferred,
        request_kib,
        chunk_size as usize,
    )?;

    Ok(bytes_transferred)
}

async fn download_remote_file_inner_with_controller(
    backend: &SftpBackend,
    app: &tauri::AppHandle,
    _session_id: &str,
    remote_path: &str,
    actual_path: &str,
    ts: &crate::config::TransferSettings,
    controller: Arc<TransferController>,
    parent_controller: Option<Arc<TransferController>>,
) -> AppResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let (request_kib, pipeline_depth, max_concurrent_writes) = sftp_pipeline_config(ts);
    let chunk_size = sftp_payload_size(request_kib) as u64;
    let transfer_started = Instant::now();

    let result: AppResult<u64> = async {
        if let Some(parent) = std::path::Path::new(&actual_path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;
            }
        }

        let sftp = backend
            .open_sftp_with_client_config(sftp_client_config(request_kib, max_concurrent_writes))
            .await?;

        let remote_attrs = sftp.metadata(remote_path).await.ok();
        let remote_size = remote_attrs.as_ref().and_then(|m| m.size);
        let total_size = remote_size.unwrap_or(0);
        controller.update_progress(0, total_size);

        let mut local_file = tokio::fs::File::create(&actual_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local file: {}", e)))?;

        if total_size > 0 {
            let _ = local_file.set_len(total_size).await;
        }

        let mut bytes_transferred: u64 = 0;

        if let Some(total_size) = remote_size {
            bytes_transferred = download_known_size_to_local_file(
                &sftp,
                remote_path,
                actual_path,
                &mut local_file,
                total_size,
                request_kib,
                pipeline_depth,
                pipeline_depth,
                &controller,
                parent_controller.as_ref(),
                |current, _delta| {
                    controller.update_progress(current, total_size);
                },
                |current| {
                    controller.update_progress(current, total_size);
                    emit_parent_progress(app, parent_controller.as_ref());
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                },
            )
            .await?;
        } else {
            let mut last_progress = Instant::now();
            let mut remote_file = sftp
                .open(remote_path)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?;

            let seq_chunk = (chunk_size as usize).max(64 * 1024);
            let mut buf = vec![0u8; seq_chunk];
            loop {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let n = remote_file
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Channel(format!("SFTP read failed: {}", e)))?;
                if n == 0 {
                    break;
                }
                local_file
                    .write_all(&buf[..n])
                    .await
                    .map_err(|e| AppError::Channel(format!("Write failed: {}", e)))?;
                bytes_transferred += n as u64;
                controller.update_progress(bytes_transferred, 0);

                if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    emit_parent_progress(app, parent_controller.as_ref());
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", 0, None),
                    );
                }
            }
        }

        local_file
            .flush()
            .await
            .map_err(|e| AppError::Channel(format!("Flush failed: {}", e)))?;

        if let Some(remote_size) = remote_size {
            ensure_download_complete(
                remote_path,
                actual_path,
                remote_size,
                bytes_transferred,
                request_kib,
                chunk_size as usize,
            )?;
        }

        if ts.preserve_timestamps {
            if let Some(ref attrs) = remote_attrs {
                let mtime = attrs.mtime.unwrap_or(0);
                if mtime > 0 {
                    use std::time::UNIX_EPOCH;
                    let set_mtime = UNIX_EPOCH + std::time::Duration::from_secs(u64::from(mtime));
                    let local_file_for_ts = std::fs::File::open(actual_path);
                    if let Ok(f) = local_file_for_ts {
                        let _ = f.set_modified(set_mtime);
                    }
                }
            }
        }

        let _ = sftp.close().await;

        Ok(bytes_transferred)
    }
    .await;

    match result {
        Ok(size) => {
            log_transfer_performance(
                "download",
                "file",
                size,
                transfer_started.elapsed(),
                request_kib,
                pipeline_depth,
                max_concurrent_writes,
                1,
            );
            controller.update_progress(size, size);
            let _ = app.emit(
                "transfer-event",
                &controller.build_event("completed", size, None),
            );
            unregister_transfer(&controller.id());
            Ok(())
        }
        Err(e) => {
            if matches!(e, AppError::Cancelled(_)) {
                cleanup_cancelled_download(actual_path).await;
            } else {
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("error", 0, Some(e.to_string())),
                );
            }
            unregister_transfer(&controller.id());
            Err(e)
        }
    }
}

async fn upload_local_file_inner_with_controller(
    backend: &SftpBackend,
    app: &tauri::AppHandle,
    _session_id: &str,
    local_path: &str,
    remote_path: &str,
    ts: &crate::config::TransferSettings,
    controller: Arc<TransferController>,
    parent_controller: Option<Arc<TransferController>>,
) -> AppResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let (request_kib, pipeline_depth, max_concurrent_writes) = sftp_pipeline_config(ts);
    let chunk_size = sftp_payload_size(request_kib);
    let transfer_started = Instant::now();

    let result: AppResult<u64> = async {
        let local_meta = tokio::fs::metadata(local_path).await;
        let total_size = local_meta.as_ref().map(|m| m.len()).unwrap_or(0);
        controller.update_progress(0, total_size);

        let sftp = backend
            .open_sftp_with_client_config(sftp_client_config(request_kib, max_concurrent_writes))
            .await?;
        let mut bytes_transferred: u64 = 0;

        let mut last_progress = Instant::now();
        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open local file: {}", e)))?;
        let mut remote_file = sftp
            .create(remote_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create remote file: {}", e)))?;

        if total_size > 0 {
            let mut buf = vec![0u8; chunk_size];
            loop {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let read = local_file
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to read local file: {}", e)))?;
                if read == 0 {
                    break;
                }

                // russh-sftp >= 2.3 pipelines write ACKs internally according to
                // client::Config::max_concurrent_writes; shutdown below drains them.
                remote_file
                    .write_all(&buf[..read])
                    .await
                    .map_err(|e| AppError::Channel(format!("SFTP write failed: {}", e)))?;

                bytes_transferred += read as u64;
                controller.update_progress(bytes_transferred, total_size);

                if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    emit_parent_progress(app, parent_controller.as_ref());
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                }
            }
        }

        remote_file
            .shutdown()
            .await
            .map_err(|e| AppError::Channel(format!("SFTP flush failed: {}", e)))?;

        if ts.preserve_timestamps {
            if let Ok(ref meta) = local_meta {
                if let Ok(mtime) = meta.modified() {
                    if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                        let atime_secs = meta
                            .accessed()
                            .ok()
                            .and_then(|a| a.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as u32)
                            .unwrap_or(dur.as_secs() as u32);
                        if let Ok(mut attrs) = sftp.metadata(remote_path).await {
                            attrs.mtime = Some(dur.as_secs() as u32);
                            attrs.atime = Some(atime_secs);
                            let _ = sftp.set_metadata(remote_path, attrs).await;
                        }
                    }
                }
            }
        }

        let _ = sftp.close().await;

        Ok(bytes_transferred)
    }
    .await;

    match result {
        Ok(size) => {
            log_transfer_performance(
                "upload",
                "file",
                size,
                transfer_started.elapsed(),
                request_kib,
                pipeline_depth,
                max_concurrent_writes,
                1,
            );
            controller.update_progress(size, size);
            let _ = app.emit(
                "transfer-event",
                &controller.build_event("completed", size, None),
            );
            unregister_transfer(&controller.id());
            Ok(())
        }
        Err(e) => {
            if matches!(e, AppError::Cancelled(_)) {
                let _ = cleanup_cancelled_upload(backend, remote_path).await;
            } else {
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("error", 0, Some(e.to_string())),
                );
            }
            unregister_transfer(&controller.id());
            Err(e)
        }
    }
}

#[async_trait::async_trait]
impl RemoteFs for SftpBackend {
    fn backend_name(&self) -> &'static str {
        "sftp"
    }

    async fn home_dir(&self) -> AppResult<String> {
        let sftp = self.open_sftp().await?;
        let home = sftp.canonicalize(".").await?;
        let _ = sftp.close().await;

        if home.is_empty() {
            Err(AppError::Config(
                "Failed to determine home directory".to_string(),
            ))
        } else {
            Ok(home)
        }
    }

    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let sftp = self.open_sftp().await?;
        let dir = sftp.read_dir(path).await?;

        let mut pending = Vec::new();
        let mut uid_set = HashSet::new();
        let mut gid_set = HashSet::new();
        for entry in dir {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let file_type = entry.file_type();
            let is_symlink = file_type == FileType::Symlink;
            let full_path = join_remote_child(normalize_remote_dir_path(path), &name);
            let is_symlink_to_dir = is_symlink
                && sftp
                    .metadata(&full_path)
                    .await
                    .ok()
                    .as_ref()
                    .map_or(false, sftp_attrs_is_dir);
            let is_dir = file_type == FileType::Dir || is_symlink_to_dir;
            let type_char = if is_dir {
                if is_symlink { 'l' } else { 'd' }
            } else if is_symlink {
                'l'
            } else {
                '-'
            };

            let attrs = entry.metadata();
            let size = attrs.size.unwrap_or(0);
            let perms = attrs.permissions.unwrap_or(0);
            let permissions = permissions_to_string(perms, type_char);
            let mtime = u64::from(attrs.mtime.unwrap_or(0));

            if attrs
                .user
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                if let Some(uid) = attrs.uid {
                    uid_set.insert(uid);
                }
            }
            if attrs
                .group
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                if let Some(gid) = attrs.gid {
                    gid_set.insert(gid);
                }
            }

            pending.push((name, is_dir, is_symlink, size, permissions, attrs, mtime));
        }

        let _ = sftp.close().await;
        let user_names = self.resolve_uid_names(uid_set).await;
        let group_names = self.resolve_gid_names(gid_set).await;
        let entries = pending
            .into_iter()
            .map(
                |(name, is_dir, is_symlink, size, permissions, attrs, mtime)| FileEntry {
                    name,
                    is_dir,
                    is_symlink,
                    size,
                    permissions,
                    owner: attrs
                        .uid
                        .and_then(|uid| user_names.get(&uid).cloned())
                        .unwrap_or_else(|| owner_or_id(&attrs.user, attrs.uid)),
                    group: attrs
                        .gid
                        .and_then(|gid| group_names.get(&gid).cloned())
                        .unwrap_or_else(|| group_or_id(&attrs.group, attrs.gid)),
                    mtime,
                },
            )
            .collect();
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<FileProperties> {
        let sftp = self.open_sftp().await?;
        let attrs = sftp.symlink_metadata(path).await?;
        let is_symlink = sftp_attrs_is_symlink(&attrs);
        let target_attrs = if is_symlink {
            sftp.metadata(path).await.ok()
        } else {
            None
        };
        let _ = sftp.close().await;

        let perms = attrs.permissions.unwrap_or(0);
        let is_dir =
            sftp_attrs_is_dir(&attrs) || target_attrs.as_ref().map_or(false, sftp_attrs_is_dir);
        let type_char = if is_dir {
            if is_symlink { 'l' } else { 'd' }
        } else if is_symlink {
            'l'
        } else {
            '-'
        };
        let permissions = permissions_to_string(perms, type_char);
        let name = path.split('/').last().unwrap_or(path).to_string();
        let owner = if let Some(uid) = attrs.uid {
            self.resolve_uid_names(HashSet::from([uid]))
                .await
                .get(&uid)
                .cloned()
                .unwrap_or_else(|| owner_or_id(&attrs.user, attrs.uid))
        } else {
            owner_or_id(&attrs.user, attrs.uid)
        };
        let group = if let Some(gid) = attrs.gid {
            self.resolve_gid_names(HashSet::from([gid]))
                .await
                .get(&gid)
                .cloned()
                .unwrap_or_else(|| group_or_id(&attrs.group, attrs.gid))
        } else {
            group_or_id(&attrs.group, attrs.gid)
        };

        Ok(FileProperties {
            name,
            is_dir,
            is_symlink,
            size: attrs.size.unwrap_or(0),
            permissions,
            owner,
            group,
            uid: attrs.uid.map_or_else(String::new, |v| v.to_string()),
            gid: attrs.gid.map_or_else(String::new, |v| v.to_string()),
            mtime: u64::from(attrs.mtime.unwrap_or(0)),
            atime: u64::from(attrs.atime.unwrap_or(0)),
        })
    }

    async fn mkdir(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        sftp.create_dir(path).await?;
        if let Some(ref m) = mode {
            apply_remote_mode_after_create(&sftp, path, m, "directory").await?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        let meta = match sftp.symlink_metadata(path).await {
            Ok(meta) => meta,
            Err(error) if is_sftp_not_found(&error) => {
                let _ = sftp.close().await;
                return Ok(());
            }
            Err(error) => {
                let _ = sftp.close().await;
                return Err(error.into());
            }
        };

        if sftp_attrs_is_symlink(&meta) {
            ignore_sftp_not_found(sftp.remove_file(path).await)?;
        } else if sftp_attrs_is_dir(&meta) {
            let _ = sftp.close().await;
            self.remove_dir_fast(path).await?;
            return Ok(());
        } else {
            ignore_sftp_not_found(sftp.remove_file(path).await)?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn rename(&self, old_path: &str, new_path: &str) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        sftp.rename(old_path, new_path).await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn create_file(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        let file = sftp.create(path).await?;
        drop(file);
        if let Some(ref m) = mode {
            apply_remote_mode_after_create(&sftp, path, m, "file").await?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn create_symlink(&self, link_path: &str, target_path: &str) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        sftp.symlink_openssh(target_path, link_path).await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn update_attrs(&self, path: &str, update: &RemoteFileAttributeUpdate) -> AppResult<()> {
        let mode = update
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(parse_octal_mode)
            .transpose()?
            .map(|value| value & POSIX_MODE_MASK);
        let uid = match update
            .owner
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(owner) => Some(self.resolve_user_to_uid(owner).await?),
            None => None,
        };
        let gid = match update
            .group
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(group) => Some(self.resolve_group_to_gid(group).await?),
            None => None,
        };

        if mode.is_none() && uid.is_none() && gid.is_none() {
            return Ok(());
        }

        let sftp = self.open_sftp().await?;
        if update.recursive {
            apply_remote_attrs_recursive(&sftp, path, mode, uid, gid).await?;
        } else {
            apply_remote_attrs(&sftp, path, mode, uid, gid).await?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
        use tokio::io::AsyncReadExt;

        let sftp = self.open_sftp().await?;
        let attrs = sftp.metadata(path).await?;
        let size = attrs.size.unwrap_or(0);
        let mtime = u64::from(attrs.mtime.unwrap_or(0));
        let type_bits = attrs.permissions.unwrap_or(0) & SFTP_FILE_TYPE_MASK;
        if type_bits == 0o040000 {
            let _ = sftp.close().await;
            return Err(AppError::Config(
                "Directories cannot be opened as text".to_string(),
            ));
        }
        if size > max_bytes {
            let _ = sftp.close().await;
            return Err(AppError::Config(format!(
                "File is too large to open as text ({} bytes > {} bytes)",
                size, max_bytes
            )));
        }

        let mut file = sftp
            .open(path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open remote file: {error}")))?;
        let mut bytes = Vec::with_capacity(size as usize);
        file.read_to_end(&mut bytes)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to read remote file: {error}")))?;
        let _ = sftp.close().await;

        ensure_text_bytes(&bytes, max_bytes)?;
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Config("Only UTF-8 text files are supported".to_string()))?;

        Ok(RemoteTextFile {
            path: path.to_string(),
            content,
            size,
            mtime,
        })
    }

    async fn write_file_text(
        &self,
        path: &str,
        content: &str,
        expected_mtime: Option<u64>,
        expected_size: Option<u64>,
        force: bool,
    ) -> AppResult<WriteRemoteTextResult> {
        use tokio::io::AsyncWriteExt;

        let sftp = self.open_sftp().await?;
        if !force {
            let attrs = sftp.metadata(path).await?;
            let current_mtime = u64::from(attrs.mtime.unwrap_or(0));
            let current_size = attrs.size.unwrap_or(0);
            if expected_mtime.is_some_and(|mtime| mtime != current_mtime)
                || expected_size.is_some_and(|size| size != current_size)
            {
                let _ = sftp.close().await;
                return Ok(WriteRemoteTextResult::conflict(current_mtime, current_size));
            }
        }

        let mut file = sftp
            .create(path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open remote file: {error}")))?;
        file.write_all(content.as_bytes())
            .await
            .map_err(|error| AppError::Channel(format!("Failed to write remote file: {error}")))?;
        file.flush()
            .await
            .map_err(|error| AppError::Channel(format!("Failed to flush remote file: {error}")))?;

        let attrs = sftp.metadata(path).await?;
        let _ = sftp.close().await;
        Ok(WriteRemoteTextResult::saved(
            u64::from(attrs.mtime.unwrap_or(0)),
            attrs.size.unwrap_or(content.len() as u64),
        ))
    }

    async fn download_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let safe_local_path = sanitize_local_download_target(local_path, remote_path);
        let actual_local_path =
            match resolve_local_path(&safe_local_path, &transfer_settings.duplicate_strategy) {
                Some(path) => path,
                None => {
                    let file_name = remote_path.split('/').last().unwrap_or(remote_path);
                    let transfer_id = transfer_id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    remember_transfer_target_external(
                        transfer_id.clone(),
                        safe_local_path.clone(),
                        "download".to_string(),
                        "file".to_string(),
                    );
                    let _ = app.emit(
                        "transfer-event",
                        &TransferEvent {
                            id: transfer_id,
                            session_id: session_id.to_string(),
                            file_name: file_name.to_string(),
                            remote_path: remote_path.to_string(),
                            local_path: safe_local_path,
                            direction: "download".to_string(),
                            kind: "file".to_string(),
                            status: "completed".to_string(),
                            size: 0,
                            bytes_transferred: 0,
                            total_size: 0,
                            parent_id: None,
                            item_count_total: None,
                            item_count_completed: None,
                            error_msg: None,
                        },
                    );
                    return Ok(());
                }
            };

        let mut last_err = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                log_event(StructuredLog {
                    level: StructuredLogLevel::Info,
                    domain: "transfer.lifecycle".to_string(),
                    event: "transfer.retry".to_string(),
                    message: "Retrying download".to_string(),
                    ids: Some(serde_json::json!({ "session_id": session_id })),
                    data: Some(serde_json::json!({
                        "direction": "download",
                        "attempt": attempt,
                        "remote_path": remote_path,
                    })),
                    error: None,
                    client_timestamp: None,
                });
            }
            match download_remote_file_inner_with_controller(
                self,
                app,
                session_id,
                remote_path,
                &actual_local_path,
                transfer_settings,
                create_child_file_transfer_controller(
                    transfer_id.clone(),
                    session_id,
                    file_name_from_path(remote_path),
                    remote_path,
                    &actual_local_path,
                    "download",
                    None,
                ),
                None,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if matches!(e, AppError::Cancelled(_)) {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn upload_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let session_manager = app.state::<Arc<crate::core::SessionManager>>();
        let sftp_for_resolve = self.open_sftp().await?;
        let actual_remote_path = match resolve_remote_path(
            app,
            session_manager.inner(),
            &sftp_for_resolve,
            session_id,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            Some(path) => path,
            None => {
                let _ = sftp_for_resolve.close().await;
                return Ok(());
            }
        };
        let _ = sftp_for_resolve.close().await;

        let mut last_err = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                log_event(StructuredLog {
                    level: StructuredLogLevel::Info,
                    domain: "transfer.lifecycle".to_string(),
                    event: "transfer.retry".to_string(),
                    message: "Retrying upload".to_string(),
                    ids: Some(serde_json::json!({ "session_id": session_id })),
                    data: Some(serde_json::json!({
                        "direction": "upload",
                        "attempt": attempt,
                        "local_path": local_path,
                    })),
                    error: None,
                    client_timestamp: None,
                });
            }
            match upload_local_file_inner_with_controller(
                self,
                app,
                session_id,
                local_path,
                &actual_remote_path,
                transfer_settings,
                create_child_file_transfer_controller(
                    transfer_id.clone(),
                    session_id,
                    file_name_from_path(&actual_remote_path),
                    &actual_remote_path,
                    local_path,
                    "upload",
                    None,
                ),
                None,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if matches!(e, AppError::Cancelled(_)) {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn download_directory(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let transfer_settings = crate::config::load_app_settings(app)
            .map(|s| s.transfer)
            .unwrap_or_default();
        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(&transfer_settings);
        let transfer_started = Instant::now();
        let directory_controller = create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name_from_path(remote_path),
            remote_path,
            local_path,
            "download",
            0,
            0,
        );
        register_transfer(directory_controller.clone());
        let _ = app.emit(
            "transfer-event",
            &directory_controller.build_event("started", 0, None),
        );

        let result = async {
            let inventory = self
                .collect_remote_directory_inventory(remote_path, local_path, &directory_controller)
                .await?;
            self.download_remote_directory_files(
                app,
                inventory,
                directory_controller.clone(),
                &transfer_settings,
            )
            .await
        }
        .await;

        match result {
            Ok(summary) => {
                log_transfer_performance(
                    "download",
                    "directory",
                    summary.bytes,
                    transfer_started.elapsed(),
                    request_kib,
                    pipeline_depth,
                    max_concurrent_writes,
                    summary.small_file_concurrency,
                );
                directory_controller.update_progress(summary.bytes, summary.bytes);
                directory_controller.update_item_progress(summary.completed, summary.total_files);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("completed", 0, None),
                );
                unregister_transfer(&directory_controller.id());
                Ok(())
            }
            Err(e) => {
                if matches!(e, AppError::Cancelled(_)) {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("cancelled", 0, None),
                    );
                    cleanup_cancelled_download(local_path).await;
                } else {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("error", 0, Some(e.to_string())),
                    );
                }
                unregister_transfer(&directory_controller.id());
                Err(e)
            }
        }
    }

    async fn upload_directory(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let session_manager = app.state::<Arc<crate::core::SessionManager>>();
        let sftp_for_check = self.open_sftp().await?;
        if !ensure_remote_upload_target_allowed(
            app,
            session_manager.inner(),
            &sftp_for_check,
            session_id,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            let _ = sftp_for_check.close().await;
            return Ok(());
        }
        let _ = sftp_for_check.close().await;

        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(transfer_settings);
        let transfer_started = Instant::now();
        let directory_controller = create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name_from_path(local_path),
            remote_path,
            local_path,
            "upload",
            0,
            0,
        );
        register_transfer(directory_controller.clone());
        let _ = app.emit(
            "transfer-event",
            &directory_controller.build_event("started", 0, None),
        );

        let result = async {
            let inventory = self
                .collect_local_directory_inventory(
                    local_path,
                    remote_path,
                    &directory_controller,
                    transfer_settings,
                )
                .await?;
            self.upload_local_directory_files(
                app,
                inventory,
                &directory_controller,
                transfer_settings,
            )
            .await
        }
        .await;

        match result {
            Ok(summary) => {
                log_transfer_performance(
                    "upload",
                    "directory",
                    summary.bytes,
                    transfer_started.elapsed(),
                    request_kib,
                    pipeline_depth,
                    max_concurrent_writes,
                    summary.small_file_concurrency,
                );
                directory_controller.update_progress(summary.bytes, summary.bytes);
                directory_controller.update_item_progress(summary.completed, summary.total_files);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("completed", 0, None),
                );
                unregister_transfer(&directory_controller.id());
                Ok(())
            }
            Err(e) => {
                if matches!(e, AppError::Cancelled(_)) {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("cancelled", 0, None),
                    );
                    let _ = cleanup_cancelled_upload(self, remote_path).await;
                } else {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("error", 0, Some(e.to_string())),
                    );
                }
                unregister_transfer(&directory_controller.id());
                Err(e)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct RemoteDirectoryFile {
    remote_path: String,
    local_path: String,
    size: u64,
    mtime: Option<u32>,
}

#[derive(Clone, Debug)]
struct LocalDirectoryFile {
    local_path: String,
    remote_path: String,
    size: u64,
    mtime: Option<std::time::SystemTime>,
    atime: Option<std::time::SystemTime>,
}

struct RemoteDirectoryInventory {
    files: Vec<RemoteDirectoryFile>,
    total_files: u64,
    total_size: u64,
    max_open_handles: Option<u64>,
}

struct LocalDirectoryInventory {
    files: Vec<LocalDirectoryFile>,
    total_files: u64,
    total_size: u64,
    max_open_handles: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct DirectoryTransferSummary {
    completed: u64,
    total_files: u64,
    bytes: u64,
    small_file_concurrency: usize,
}

struct RemoveInventory {
    files: Vec<String>,
    dirs: Vec<String>,
}

async fn collect_remove_inventory(sftp: &SftpSession, path: &str) -> AppResult<RemoveInventory> {
    let path = normalize_remote_dir_path(path).to_string();
    let dir = match sftp.read_dir(&path).await {
        Ok(dir) => dir,
        Err(error) if is_sftp_not_found(&error) => {
            return Ok(RemoveInventory {
                files: Vec::new(),
                dirs: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    let mut dirs = vec![path.clone()];

    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child = join_remote_child(&path, &name);
        if entry.file_type() == FileType::Dir {
            let child_inventory = Box::pin(collect_remove_inventory(sftp, &child)).await?;
            files.extend(child_inventory.files);
            dirs.extend(child_inventory.dirs);
        } else {
            files.push(child);
        }
    }

    Ok(RemoveInventory { files, dirs })
}

async fn remove_inventory_concurrent(
    pool: SftpSessionPool,
    mut inventory: RemoveInventory,
    concurrency: SftpDirectoryConcurrency,
) -> AppResult<()> {
    let worker_count = sftp_directory_file_concurrency(inventory.files.len(), concurrency);
    let queue = Arc::new(StdMutex::new(VecDeque::from(std::mem::take(
        &mut inventory.files,
    ))));
    let mut join_set: tokio::task::JoinSet<AppResult<()>> = tokio::task::JoinSet::new();

    for worker_index in 0..worker_count {
        let pool = pool.clone();
        let queue = queue.clone();
        join_set.spawn(async move {
            loop {
                let file = {
                    let mut queue = queue.lock().unwrap();
                    queue.pop_front()
                };
                let Some(file) = file else {
                    return Ok(());
                };
                let session = pool.session_for(worker_index);
                if let Err(error) = session.remove_file(&file).await {
                    if let Some(message) = sftp_remove_error(&file, "file", error) {
                        return Err(AppError::Channel(message));
                    }
                }
            }
        });
    }

    let mut errors = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(error.to_string()),
            Err(error) => errors.push(format!("Directory delete worker panicked: {error}")),
        }
    }

    inventory
        .dirs
        .sort_by_key(|dir| std::cmp::Reverse(dir.matches('/').count()));
    for dir in inventory.dirs {
        let session = pool.session_for(0);
        if let Err(error) = session.remove_dir(&dir).await {
            if let Some(message) = sftp_remove_error(&dir, "directory", error) {
                errors.push(message);
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Channel(format!(
            "{} item(s) could not be deleted:\n{}",
            errors.len(),
            errors.join("\n")
        )))
    }
}

impl SftpBackend {
    async fn remove_dir_fast(&self, path: &str) -> AppResult<()> {
        if is_safe_recursive_remove_target(path) {
            let command = format!("rm -rf -- {}", sh_quote(normalize_remote_dir_path(path)));
            match self.exec_ok(&command).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    tracing::warn!(
                        remote_path = path,
                        error = %error,
                        "Remote rm -rf fast path failed, falling back to SFTP recursive delete"
                    );
                }
            }
        }

        self.remove_dir_concurrent_sftp(path).await
    }

    async fn remove_dir_concurrent_sftp(&self, path: &str) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        let max_open_handles = sftp.max_open_handles();
        let result = collect_remove_inventory(&sftp, path).await;
        let _ = sftp.close().await;
        let inventory = result?;

        if inventory.files.is_empty() && inventory.dirs.is_empty() {
            return Ok(());
        }

        if inventory.files.is_empty() && inventory.dirs.len() <= 1 {
            let sftp = self.open_sftp().await?;
            let result = sftp.remove_dir(normalize_remote_dir_path(path)).await;
            let _ = sftp.close().await;
            return match result {
                Ok(()) => Ok(()),
                Err(error) if is_sftp_not_found(&error) => Ok(()),
                Err(error) => Err(AppError::Channel(format!(
                    "Failed to remove directory '{}': {}",
                    normalize_remote_dir_path(path),
                    error
                ))),
            };
        }

        let concurrency = sftp_directory_concurrency(max_open_handles);
        let pool = SftpSessionPool::new(
            self,
            concurrency.session_pool_size,
            SftpClientConfig::default(),
        )
        .await?;
        let result = remove_inventory_concurrent(pool.clone(), inventory, concurrency).await;
        pool.close_all().await;
        result
    }

    async fn collect_remote_directory_inventory(
        &self,
        remote_path: &str,
        local_path: &str,
        directory_controller: &Arc<TransferController>,
    ) -> AppResult<RemoteDirectoryInventory> {
        let sftp = self.open_sftp().await?;
        let max_open_handles = sftp.max_open_handles();
        let result = self
            .collect_remote_directory_inventory_inner(
                &sftp,
                remote_path,
                local_path,
                directory_controller,
            )
            .await;
        let _ = sftp.close().await;
        result.map(|(files, total_size)| {
            let total_files = files.len() as u64;
            directory_controller.update_totals(total_size, total_files);
            RemoteDirectoryInventory {
                files,
                total_files,
                total_size,
                max_open_handles,
            }
        })
    }

    async fn collect_remote_directory_inventory_inner(
        &self,
        sftp: &SftpSession,
        remote_path: &str,
        local_path: &str,
        directory_controller: &Arc<TransferController>,
    ) -> AppResult<(Vec<RemoteDirectoryFile>, u64)> {
        wait_for_transfer_ready(directory_controller).await?;

        tokio::fs::create_dir_all(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;

        let dir = sftp.read_dir(remote_path).await?;
        let mut files = Vec::new();
        let mut total_size = 0u64;

        for entry in dir {
            wait_for_transfer_ready(directory_controller).await?;

            let name = entry.file_name();
            let child_remote = join_remote_child(normalize_remote_dir_path(remote_path), &name);
            let child_local = append_safe_local_child_path(local_path, &name);
            let attrs = entry.metadata();
            let file_type = entry.file_type();
            let is_symlink = file_type == FileType::Symlink;
            let is_symlink_to_dir = is_symlink
                && sftp
                    .metadata(&child_remote)
                    .await
                    .ok()
                    .as_ref()
                    .map_or(false, sftp_attrs_is_dir);

            if file_type == FileType::Dir || is_symlink_to_dir {
                let (child_files, child_size) =
                    Box::pin(self.collect_remote_directory_inventory_inner(
                        sftp,
                        &child_remote,
                        &child_local,
                        directory_controller,
                    ))
                    .await?;
                total_size = total_size.saturating_add(child_size);
                files.extend(child_files);
            } else if !is_symlink {
                let size = attrs.size.unwrap_or(0);
                total_size = total_size.saturating_add(size);
                files.push(RemoteDirectoryFile {
                    remote_path: child_remote,
                    local_path: child_local,
                    size,
                    mtime: attrs.mtime,
                });
                directory_controller.update_totals(total_size, files.len() as u64);
            }
        }

        Ok((files, total_size))
    }

    async fn collect_local_directory_inventory(
        &self,
        local_path: &str,
        remote_path: &str,
        directory_controller: &Arc<TransferController>,
        transfer_settings: &crate::config::TransferSettings,
    ) -> AppResult<LocalDirectoryInventory> {
        let (request_kib, _, max_concurrent_writes) = sftp_pipeline_config(transfer_settings);
        let sftp = self
            .open_sftp_with_client_config(sftp_client_config(request_kib, max_concurrent_writes))
            .await?;
        let max_open_handles = sftp.max_open_handles();
        let result = self
            .collect_local_directory_inventory_inner(
                &sftp,
                local_path,
                remote_path,
                directory_controller,
            )
            .await;
        let _ = sftp.close().await;
        result.map(|(files, total_size)| {
            let total_files = files.len() as u64;
            directory_controller.update_totals(total_size, total_files);
            LocalDirectoryInventory {
                files,
                total_files,
                total_size,
                max_open_handles,
            }
        })
    }

    async fn collect_local_directory_inventory_inner(
        &self,
        sftp: &SftpSession,
        local_path: &str,
        remote_path: &str,
        directory_controller: &Arc<TransferController>,
    ) -> AppResult<(Vec<LocalDirectoryFile>, u64)> {
        wait_for_transfer_ready(directory_controller).await?;

        let _ = sftp.create_dir(remote_path).await;
        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut read_dir = tokio::fs::read_dir(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to read local dir: {}", e)))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| AppError::Channel(format!("Failed to read dir entry: {}", e)))?
        {
            wait_for_transfer_ready(directory_controller).await?;

            let file_type = entry
                .file_type()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to get file type: {}", e)))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let child_local = std::path::Path::new(local_path)
                .join(&name)
                .to_string_lossy()
                .to_string();
            let child_remote = join_remote_child(normalize_remote_dir_path(remote_path), &name);

            if file_type.is_dir() {
                let (child_files, child_size) =
                    Box::pin(self.collect_local_directory_inventory_inner(
                        sftp,
                        &child_local,
                        &child_remote,
                        directory_controller,
                    ))
                    .await?;
                total_size = total_size.saturating_add(child_size);
                files.extend(child_files);
            } else if file_type.is_file() {
                let metadata = entry.metadata().await.map_err(|e| {
                    AppError::Channel(format!("Failed to read file metadata: {}", e))
                })?;
                let size = metadata.len();
                total_size = total_size.saturating_add(size);
                files.push(LocalDirectoryFile {
                    local_path: child_local,
                    remote_path: child_remote,
                    size,
                    mtime: metadata.modified().ok(),
                    atime: metadata.accessed().ok(),
                });
                directory_controller.update_totals(total_size, files.len() as u64);
            }
        }

        Ok((files, total_size))
    }

    async fn download_remote_directory_files(
        &self,
        app: &tauri::AppHandle,
        inventory: RemoteDirectoryInventory,
        directory_controller: Arc<TransferController>,
        transfer_settings: &crate::config::TransferSettings,
    ) -> AppResult<DirectoryTransferSummary> {
        let concurrency = sftp_directory_concurrency(inventory.max_open_handles);
        if inventory.files.is_empty() {
            return Ok(DirectoryTransferSummary {
                completed: 0,
                total_files: 0,
                bytes: 0,
                small_file_concurrency: concurrency.small_file_concurrency,
            });
        }

        let (request_kib, _, max_concurrent_writes) = sftp_pipeline_config(transfer_settings);
        let pool = SftpSessionPool::new(
            self,
            concurrency.session_pool_size,
            sftp_client_config(request_kib, max_concurrent_writes),
        )
        .await?;
        let result = run_download_directory_workers(
            app,
            pool.clone(),
            inventory,
            directory_controller,
            transfer_settings,
            concurrency,
        )
        .await;
        pool.close_all().await;
        result
    }

    async fn upload_local_directory_files(
        &self,
        app: &tauri::AppHandle,
        inventory: LocalDirectoryInventory,
        directory_controller: &Arc<TransferController>,
        transfer_settings: &crate::config::TransferSettings,
    ) -> AppResult<DirectoryTransferSummary> {
        let concurrency = sftp_directory_concurrency(inventory.max_open_handles);
        if inventory.files.is_empty() {
            return Ok(DirectoryTransferSummary {
                completed: 0,
                total_files: 0,
                bytes: 0,
                small_file_concurrency: concurrency.small_file_concurrency,
            });
        }

        let (request_kib, _, max_concurrent_writes) = sftp_pipeline_config(transfer_settings);
        let pool = SftpSessionPool::new(
            self,
            concurrency.session_pool_size,
            sftp_client_config(request_kib, max_concurrent_writes),
        )
        .await?;
        let result = run_upload_directory_workers(
            app,
            pool.clone(),
            inventory,
            directory_controller.clone(),
            transfer_settings,
            concurrency,
        )
        .await;
        pool.close_all().await;
        result
    }
}

fn sftp_directory_file_concurrency(
    files_len: usize,
    concurrency: SftpDirectoryConcurrency,
) -> usize {
    files_len.min(concurrency.small_file_concurrency).max(1)
}

fn add_directory_transferred_bytes(
    directory_controller: &Arc<TransferController>,
    completed_bytes: &AtomicU64,
    delta: u64,
    total_size: u64,
) -> u64 {
    let bytes_done = completed_bytes.fetch_add(delta, Ordering::SeqCst) + delta;
    directory_controller.update_progress(bytes_done, total_size);
    bytes_done
}

async fn run_download_directory_workers(
    app: &tauri::AppHandle,
    pool: SftpSessionPool,
    inventory: RemoteDirectoryInventory,
    directory_controller: Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    concurrency: SftpDirectoryConcurrency,
) -> AppResult<DirectoryTransferSummary> {
    let worker_count = sftp_directory_file_concurrency(inventory.files.len(), concurrency);
    let total_files = inventory.total_files;
    let total_size = inventory.total_size;
    let queue = Arc::new(StdMutex::new(VecDeque::from(inventory.files)));
    let completed_count = Arc::new(AtomicU64::new(0));
    let completed_bytes = Arc::new(AtomicU64::new(0));
    let large_lane = Arc::new(Semaphore::new(concurrency.large_file_concurrency));
    let mut join_set = tokio::task::JoinSet::new();

    for worker_index in 0..worker_count {
        let app = app.clone();
        let pool = pool.clone();
        let queue = queue.clone();
        let directory_controller = directory_controller.clone();
        let completed_count = completed_count.clone();
        let completed_bytes = completed_bytes.clone();
        let large_lane = large_lane.clone();
        let transfer_settings = transfer_settings.clone();
        join_set.spawn(async move {
            loop {
                wait_for_transfer_ready(&directory_controller).await?;
                let file = {
                    let mut queue = queue.lock().unwrap();
                    queue.pop_front()
                };
                let Some(file) = file else {
                    return Ok(());
                };
                let _large_permit = if file.size > SFTP_SMALL_FILE_THRESHOLD {
                    Some(large_lane.acquire().await.map_err(|e| {
                        AppError::Channel(format!("SFTP large-file lane closed: {}", e))
                    })?)
                } else {
                    None
                };
                let session = pool.session_for(worker_index);
                let _bytes = download_directory_file_with_session(
                    &app,
                    session,
                    file,
                    &directory_controller,
                    &transfer_settings,
                    &completed_bytes,
                    total_size,
                    concurrency.small_file_concurrency,
                )
                .await?;
                let completed = completed_count.fetch_add(1, Ordering::SeqCst) + 1;
                directory_controller.update_item_progress(completed, total_files);
                let bytes_done = completed_bytes.load(Ordering::SeqCst);
                directory_controller.update_progress(bytes_done, total_size);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("progress", 0, None),
                );
            }
        });
    }

    let mut first_err = None;
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if first_err.is_none() {
                    first_err = Some(error);
                }
            }
            Err(error) => {
                if first_err.is_none() {
                    first_err = Some(AppError::Channel(format!(
                        "Directory download worker panicked: {}",
                        error
                    )));
                }
            }
        }
    }

    if let Some(error) = first_err {
        Err(error)
    } else {
        Ok(DirectoryTransferSummary {
            completed: completed_count.load(Ordering::SeqCst),
            total_files,
            bytes: completed_bytes.load(Ordering::SeqCst),
            small_file_concurrency: concurrency.small_file_concurrency,
        })
    }
}

async fn run_upload_directory_workers(
    app: &tauri::AppHandle,
    pool: SftpSessionPool,
    inventory: LocalDirectoryInventory,
    directory_controller: Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    concurrency: SftpDirectoryConcurrency,
) -> AppResult<DirectoryTransferSummary> {
    let worker_count = sftp_directory_file_concurrency(inventory.files.len(), concurrency);
    let total_files = inventory.total_files;
    let total_size = inventory.total_size;
    let queue = Arc::new(StdMutex::new(VecDeque::from(inventory.files)));
    let completed_count = Arc::new(AtomicU64::new(0));
    let completed_bytes = Arc::new(AtomicU64::new(0));
    let large_lane = Arc::new(Semaphore::new(concurrency.large_file_concurrency));
    let mut join_set = tokio::task::JoinSet::new();

    for worker_index in 0..worker_count {
        let app = app.clone();
        let pool = pool.clone();
        let queue = queue.clone();
        let directory_controller = directory_controller.clone();
        let completed_count = completed_count.clone();
        let completed_bytes = completed_bytes.clone();
        let large_lane = large_lane.clone();
        let transfer_settings = transfer_settings.clone();
        join_set.spawn(async move {
            loop {
                wait_for_transfer_ready(&directory_controller).await?;
                let file = {
                    let mut queue = queue.lock().unwrap();
                    queue.pop_front()
                };
                let Some(file) = file else {
                    return Ok(());
                };
                let _large_permit = if file.size > SFTP_SMALL_FILE_THRESHOLD {
                    Some(large_lane.acquire().await.map_err(|e| {
                        AppError::Channel(format!("SFTP large-file lane closed: {}", e))
                    })?)
                } else {
                    None
                };
                let session = pool.session_for(worker_index);
                let _bytes = upload_directory_file_with_session(
                    &app,
                    session,
                    file,
                    &directory_controller,
                    &transfer_settings,
                    &completed_bytes,
                    total_size,
                )
                .await?;
                let completed = completed_count.fetch_add(1, Ordering::SeqCst) + 1;
                directory_controller.update_item_progress(completed, total_files);
                let bytes_done = completed_bytes.load(Ordering::SeqCst);
                directory_controller.update_progress(bytes_done, total_size);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("progress", 0, None),
                );
            }
        });
    }

    let mut first_err = None;
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if first_err.is_none() {
                    first_err = Some(error);
                }
            }
            Err(error) => {
                if first_err.is_none() {
                    first_err = Some(AppError::Channel(format!(
                        "Directory upload worker panicked: {}",
                        error
                    )));
                }
            }
        }
    }

    if let Some(error) = first_err {
        Err(error)
    } else {
        Ok(DirectoryTransferSummary {
            completed: completed_count.load(Ordering::SeqCst),
            total_files,
            bytes: completed_bytes.load(Ordering::SeqCst),
            small_file_concurrency: concurrency.small_file_concurrency,
        })
    }
}

async fn download_directory_file_with_session(
    app: &tauri::AppHandle,
    sftp: Arc<ManagedSftpSession>,
    file: RemoteDirectoryFile,
    directory_controller: &Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    completed_bytes: &Arc<AtomicU64>,
    total_size: u64,
    max_pipeline_depth: usize,
) -> AppResult<u64> {
    use tokio::io::AsyncWriteExt;

    wait_for_transfer_ready(directory_controller).await?;
    if let Some(parent) = std::path::Path::new(&file.local_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;
    }

    let mut local_file = tokio::fs::File::create(&file.local_path)
        .await
        .map_err(|e| {
            AppError::Channel(format!(
                "Failed to create local file {}: {}",
                file.local_path, e
            ))
        })?;
    if file.size > 0 {
        let _ = local_file.set_len(file.size).await;
    }

    let mut bytes_transferred = 0u64;
    let (request_kib, pipeline_depth, _) = sftp_pipeline_config(transfer_settings);
    let payload_bytes = sftp_payload_size(request_kib);
    if file.size > 0 {
        let app_for_progress = app.clone();
        bytes_transferred = download_known_size_to_local_file(
            &sftp,
            &file.remote_path,
            &file.local_path,
            &mut local_file,
            file.size,
            request_kib,
            pipeline_depth,
            max_pipeline_depth,
            directory_controller,
            None,
            |_current, delta| {
                add_directory_transferred_bytes(
                    directory_controller,
                    completed_bytes,
                    delta,
                    total_size,
                );
            },
            |_current| {
                let _ = app_for_progress.emit(
                    "transfer-event",
                    &directory_controller.build_event("progress", 0, None),
                );
            },
        )
        .await?;
    }

    local_file
        .flush()
        .await
        .map_err(|e| AppError::Channel(format!("Flush failed for {}: {}", file.local_path, e)))?;

    ensure_download_complete(
        &file.remote_path,
        &file.local_path,
        file.size,
        bytes_transferred,
        request_kib,
        payload_bytes,
    )?;

    if transfer_settings.preserve_timestamps {
        if let Some(mtime) = file.mtime.filter(|mtime| *mtime > 0) {
            let set_mtime =
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(mtime));
            if let Ok(f) = std::fs::File::open(&file.local_path) {
                let _ = f.set_modified(set_mtime);
            }
        }
    }

    emit_parent_progress(app, Some(directory_controller));
    Ok(bytes_transferred)
}

async fn upload_directory_file_with_session(
    app: &tauri::AppHandle,
    sftp: Arc<ManagedSftpSession>,
    file: LocalDirectoryFile,
    directory_controller: &Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    completed_bytes: &Arc<AtomicU64>,
    total_size: u64,
) -> AppResult<u64> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    wait_for_transfer_ready(directory_controller).await?;
    let mut local_file = tokio::fs::File::open(&file.local_path).await.map_err(|e| {
        AppError::Channel(format!(
            "Failed to open local file {}: {}",
            file.local_path, e
        ))
    })?;
    let mut remote_file = sftp.create(&file.remote_path).await.map_err(|e| {
        AppError::Channel(format!(
            "Failed to create remote file {}: {}",
            file.remote_path, e
        ))
    })?;

    let (request_kib, _, _) = sftp_pipeline_config(transfer_settings);
    let mut buf = vec![0u8; sftp_payload_size(request_kib)];
    let mut bytes_transferred = 0u64;
    let mut last_progress = Instant::now();
    loop {
        wait_for_transfer_ready(directory_controller).await?;
        let read = local_file.read(&mut buf).await.map_err(|e| {
            AppError::Channel(format!(
                "Failed to read local file {}: {}",
                file.local_path, e
            ))
        })?;
        if read == 0 {
            break;
        }
        remote_file.write_all(&buf[..read]).await.map_err(|e| {
            AppError::Channel(format!("SFTP write failed for {}: {}", file.remote_path, e))
        })?;
        bytes_transferred = bytes_transferred.saturating_add(read as u64);
        add_directory_transferred_bytes(
            directory_controller,
            completed_bytes,
            read as u64,
            total_size,
        );

        if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
            last_progress = Instant::now();
            let _ = app.emit(
                "transfer-event",
                &directory_controller.build_event("progress", 0, None),
            );
        }
    }
    remote_file.shutdown().await.map_err(|e| {
        AppError::Channel(format!("SFTP flush failed for {}: {}", file.remote_path, e))
    })?;

    if transfer_settings.preserve_timestamps {
        if let Some(mtime) = file.mtime {
            if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                let atime_secs = file
                    .atime
                    .and_then(|a| a.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as u32)
                    .unwrap_or(dur.as_secs() as u32);
                if let Ok(mut attrs) = sftp.metadata(&file.remote_path).await {
                    attrs.mtime = Some(dur.as_secs() as u32);
                    attrs.atime = Some(atime_secs);
                    let _ = sftp.set_metadata(&file.remote_path, attrs).await;
                }
            }
        }
    }

    emit_parent_progress(app, Some(directory_controller));
    Ok(bytes_transferred)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_progress_allows_short_non_empty_reads_until_complete() {
        let remote_size = 256 * 1024;
        let request_kib = 256;
        let payload_bytes = 256 * 1024;
        let read_bytes = 64 * 1024;
        let mut offset = 0;
        let mut bytes_written = 0;

        for _ in 0..3 {
            let progress = classify_download_read_progress(
                "/remote/file.bin",
                "C:/local/file.bin",
                remote_size,
                offset,
                bytes_written,
                read_bytes as usize,
                request_kib,
                payload_bytes,
            )
            .expect("short non-empty read should continue");

            offset = match progress {
                DownloadReadProgress::Continue(next_offset) => next_offset,
                DownloadReadProgress::Complete => panic!("download completed too early"),
            };
            bytes_written += read_bytes;
        }

        let progress = classify_download_read_progress(
            "/remote/file.bin",
            "C:/local/file.bin",
            remote_size,
            offset,
            bytes_written,
            read_bytes as usize,
            request_kib,
            payload_bytes,
        )
        .expect("final short read should complete");
        assert_eq!(progress, DownloadReadProgress::Complete);
        bytes_written += read_bytes;
        assert!(
            ensure_download_complete(
                "/remote/file.bin",
                "C:/local/file.bin",
                remote_size,
                bytes_written,
                request_kib,
                payload_bytes,
            )
            .is_ok()
        );
    }

    #[test]
    fn download_progress_rejects_empty_read_before_remote_size() {
        let error = classify_download_read_progress(
            "/remote/file.bin",
            "C:/local/file.bin",
            256 * 1024,
            64 * 1024,
            64 * 1024,
            0,
            256,
            256 * 1024,
        )
        .expect_err("empty read before remote size should fail");

        assert!(error.to_string().contains("Unexpected EOF"));
        assert!(
            error
                .to_string()
                .contains("expected 262144 bytes, got 65536 bytes")
        );
    }

    #[test]
    fn download_completion_rejects_short_written_count() {
        let error = ensure_download_complete(
            "/remote/file.bin",
            "C:/local/file.bin",
            256 * 1024,
            192 * 1024,
            256,
            256 * 1024,
        )
        .expect_err("short written count should fail");

        assert!(error.to_string().contains("Unexpected EOF"));
        assert!(
            error
                .to_string()
                .contains("expected 262144 bytes, got 196608 bytes")
        );
    }

    #[test]
    fn download_completion_accepts_empty_remote_file() {
        assert!(
            ensure_download_complete(
                "/remote/empty.txt",
                "C:/local/empty.txt",
                0,
                0,
                256,
                256 * 1024,
            )
            .is_ok()
        );
    }

    #[test]
    fn directory_concurrency_uses_fast_default_without_server_limits() {
        let concurrency = sftp_directory_concurrency(None);

        assert_eq!(concurrency.session_pool_size, 2);
        assert_eq!(concurrency.small_file_concurrency, 64);
        assert_eq!(concurrency.large_file_concurrency, 2);
    }

    #[test]
    fn directory_concurrency_respects_low_server_handle_limits() {
        let concurrency = sftp_directory_concurrency(Some(12));

        assert_eq!(concurrency.session_pool_size, 2);
        assert_eq!(concurrency.small_file_concurrency, 4);
        assert_eq!(concurrency.large_file_concurrency, 2);
    }

    #[test]
    fn sftp_channel_open_retry_classifies_temporary_capacity_failures() {
        assert!(is_retryable_sftp_channel_open_error(
            &russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)
        ));
        assert!(is_retryable_sftp_channel_open_error(
            &russh::Error::ChannelOpenFailure(ChannelOpenFailure::ResourceShortage)
        ));
    }

    #[test]
    fn sftp_channel_open_retry_rejects_policy_and_type_failures() {
        assert!(!is_retryable_sftp_channel_open_error(
            &russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited)
        ));
        assert!(!is_retryable_sftp_channel_open_error(
            &russh::Error::ChannelOpenFailure(ChannelOpenFailure::UnknownChannelType)
        ));
    }

    #[test]
    fn directory_concurrency_keeps_at_least_one_worker() {
        let concurrency = sftp_directory_concurrency(Some(2));

        assert_eq!(concurrency.session_pool_size, 1);
        assert_eq!(concurrency.small_file_concurrency, 1);
        assert_eq!(concurrency.large_file_concurrency, 1);
    }

    #[test]
    fn directory_progress_accumulates_chunk_deltas_without_completion_double_count() {
        let controller = create_directory_transfer_controller(
            Some("directory-progress-test".to_string()),
            "session-1",
            "folder".to_string(),
            "/remote/folder",
            "C:/local/folder",
            "download",
            2,
            1_000,
        );
        let completed_bytes = AtomicU64::new(0);

        assert_eq!(
            add_directory_transferred_bytes(&controller, &completed_bytes, 128, 1_000),
            128
        );
        assert_eq!(
            add_directory_transferred_bytes(&controller, &completed_bytes, 256, 1_000),
            384
        );

        controller.update_item_progress(1, 2);
        controller.update_progress(completed_bytes.load(Ordering::SeqCst), 1_000);

        let event = controller.build_event("progress", 0, None);
        assert_eq!(event.bytes_transferred, 384);
        assert_eq!(event.item_count_completed, Some(1));
        assert_eq!(event.item_count_total, Some(2));
    }

    #[test]
    fn directory_worker_count_is_bounded_by_file_count() {
        let concurrency = sftp_directory_concurrency(None);

        assert_eq!(sftp_directory_file_concurrency(0, concurrency), 1);
        assert_eq!(sftp_directory_file_concurrency(3, concurrency), 3);
        assert_eq!(
            sftp_directory_file_concurrency(10_000, concurrency),
            concurrency.small_file_concurrency
        );
    }

    #[test]
    fn recursive_remove_rejects_dangerous_targets() {
        assert!(!is_safe_recursive_remove_target(""));
        assert!(!is_safe_recursive_remove_target("/"));
        assert!(!is_safe_recursive_remove_target("."));
        assert!(!is_safe_recursive_remove_target(".."));
        assert!(!is_safe_recursive_remove_target("/tmp/../home"));
    }

    #[test]
    fn recursive_remove_accepts_normal_remote_targets() {
        assert!(is_safe_recursive_remove_target("/tmp/uploads"));
        assert!(is_safe_recursive_remove_target("relative/uploads"));
        assert!(is_safe_recursive_remove_target("/home/user/data/"));
    }
}
