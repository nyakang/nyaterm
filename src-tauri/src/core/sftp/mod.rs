//! Auto-fallback remote file system.
//!
//! Transparently picks the best available backend for each SSH session:
//! SFTP subsystem → SCP Enhanced (find/stat/tar) → SCP Normal (ls/cat).
//! The upper layers and the frontend never need to know which protocol is in use.

mod cache;
pub(crate) mod duplicate;
mod scp_enhanced;
mod scp_normal;
mod sftp_backend;
pub(crate) mod traits;
pub(crate) mod transfer;
pub(crate) mod util;

use cache::{cache_key, load_cached_backend, save_cached_backend};
use scp_enhanced::ScpEnhancedBackend;
use scp_normal::ScpNormalBackend;
use sftp_backend::SftpBackend;
use traits::RemoteFs;

use crate::core::remote_exec::{ensure_success, exec_ssh_session_command};
use crate::core::ssh::SshConnectionHandles;
use crate::core::{SessionManager, SessionType};
use crate::error::{AppError, AppResult};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub(crate) struct CopyResolvedTarget {
    pub(crate) path: String,
    pub(crate) existed: bool,
}

pub(crate) use duplicate::TransferDuplicateManager;
pub(crate) use transfer::{active_transfer_count, transfer_target_directory};
pub use transfer::{cancel_transfer, pause_transfer, resume_transfer};
pub(crate) use util::RemotePathRef;
pub(crate) use util::sanitize_download_file_name;
pub use util::{
    DirectoryChild, FileEntry, FileProperties, RemoteBinaryFile, RemoteFileAttributeUpdate,
    RemoteTextFile, WriteRemoteTextResult,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CopyEndpointKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyEndpoint {
    pub session_id: String,
    pub kind: CopyEndpointKind,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyFileEntryRequest {
    pub source: CopyEndpoint,
    pub target: CopyEndpoint,
    pub file_name: String,
    pub is_directory: bool,
    pub transfer_id: Option<String>,
    pub duplicate_strategy_override: Option<String>,
}

fn is_remote_delete_not_found(error: &AppError) -> bool {
    match error {
        AppError::Sftp(SftpError::Status(status)) => status.status_code == StatusCode::NoSuchFile,
        AppError::Channel(message) => {
            let lower = message.to_ascii_lowercase();
            lower.contains("no such file")
                || lower.contains("not found")
                || lower.contains("no such file or directory")
        }
        _ => false,
    }
}

/// Orchestrator that lazily initialises the best available remote file system
/// backend and delegates all operations through it.
pub(crate) struct AutoRemoteFs {
    inner: RwLock<Option<Box<dyn RemoteFs>>>,
    ssh_handle: Arc<SshConnectionHandles>,
    cache_key: String,
    sftp_encoding: String,
}

impl AutoRemoteFs {
    pub(crate) fn new(
        ssh_handle: Arc<SshConnectionHandles>,
        host: &str,
        port: u16,
        username: &str,
        sftp_encoding: &str,
    ) -> Self {
        Self {
            inner: RwLock::new(None),
            ssh_handle,
            cache_key: cache_key(host, port, username),
            sftp_encoding: sftp_encoding.to_string(),
        }
    }

    async fn ensure_backend(&self) -> AppResult<()> {
        {
            let guard = self.inner.read().await;
            if guard.is_some() {
                return Ok(());
            }
        }

        let mut guard = self.inner.write().await;
        if guard.is_some() {
            return Ok(());
        }

        let backend = self.probe_backends().await?;
        tracing::info!(
            backend = backend.backend_name(),
            cache_key = %self.cache_key,
            "Active remote file backend selected"
        );
        *guard = Some(backend);
        Ok(())
    }

    async fn probe_backends(&self) -> AppResult<Box<dyn RemoteFs>> {
        if let Some(cached) = load_cached_backend(&self.cache_key) {
            tracing::debug!(cached_backend = %cached, "Trying cached backend first");
            if let Some(backend) = self.try_cached_backend(&cached).await {
                return Ok(backend);
            }
            tracing::debug!(cached_backend = %cached, "Cached backend failed, probing all");
        }

        let sftp_failure;

        tracing::debug!("Probing SFTP backend");
        match SftpBackend::probe(&self.ssh_handle).await {
            Ok(()) => {
                save_cached_backend(&self.cache_key, "sftp", false, None);
                return Ok(Box::new(SftpBackend::new(
                    self.ssh_handle.clone(),
                    &self.sftp_encoding,
                )));
            }
            Err(e) => {
                let reason = e.to_string();
                tracing::debug!(error = %reason, "SFTP backend unavailable, trying SCP Enhanced");
                sftp_failure = Some(reason);
            }
        }

        tracing::debug!("Probing SCP Enhanced backend");
        match ScpEnhancedBackend::probe(&self.ssh_handle).await {
            Ok(()) => {
                save_cached_backend(&self.cache_key, "scp_enhanced", true, sftp_failure);
                return Ok(Box::new(ScpEnhancedBackend::new(self.ssh_handle.clone())));
            }
            Err(e) => {
                tracing::debug!(error = %e, "SCP Enhanced backend unavailable, trying SCP Normal");
            }
        }

        tracing::debug!("Probing SCP Normal backend");
        match ScpNormalBackend::probe(&self.ssh_handle).await {
            Ok(()) => {
                save_cached_backend(&self.cache_key, "scp_normal", true, sftp_failure);
                return Ok(Box::new(ScpNormalBackend::new(self.ssh_handle.clone())));
            }
            Err(e) => {
                tracing::debug!(error = %e, "SCP Normal backend unavailable");
            }
        }

        Err(AppError::Channel(
            "Terminal connection is working, but the remote file manager could not be initialized"
                .to_string(),
        ))
    }

    async fn try_cached_backend(&self, name: &str) -> Option<Box<dyn RemoteFs>> {
        match name {
            "sftp" => {
                SftpBackend::probe(&self.ssh_handle)
                    .await
                    .ok()
                    .map(|()| -> Box<dyn RemoteFs> {
                        Box::new(SftpBackend::new(
                            self.ssh_handle.clone(),
                            &self.sftp_encoding,
                        ))
                    })
            }
            "scp_enhanced" => ScpEnhancedBackend::probe(&self.ssh_handle).await.ok().map(
                |()| -> Box<dyn RemoteFs> {
                    Box::new(ScpEnhancedBackend::new(self.ssh_handle.clone()))
                },
            ),
            "scp_normal" => ScpNormalBackend::probe(&self.ssh_handle).await.ok().map(
                |()| -> Box<dyn RemoteFs> {
                    Box::new(ScpNormalBackend::new(self.ssh_handle.clone()))
                },
            ),
            _ => None,
        }
    }

    async fn backend(
        &self,
    ) -> AppResult<tokio::sync::RwLockReadGuard<'_, Option<Box<dyn RemoteFs>>>> {
        self.ensure_backend().await?;
        Ok(self.inner.read().await)
    }
}

// ---------------------------------------------------------------------------
// Public API functions called by cmd/sftp.rs
// ---------------------------------------------------------------------------

async fn get_ssh_info(
    manager: &SessionManager,
    session_id: &str,
) -> AppResult<(
    Arc<SshConnectionHandles>,
    String,
    u16,
    String,
    String,
    String,
)> {
    let sessions = manager.sessions.lock().await;
    let session = sessions
        .get(session_id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", session_id)))?;

    let ssh_handle = session
        .ssh_handle
        .as_ref()
        .ok_or_else(|| AppError::Config("Not an SSH session".to_string()))?
        .clone()
        .downcast::<SshConnectionHandles>()
        .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?;

    let (host, port, username, encoding, sftp_encoding) =
        if let Some(ref cfg_any) = session.ssh_config {
            if let Some(cfg) = cfg_any.downcast_ref::<crate::core::ssh::SshConfig>() {
                let sftp_encoding = if cfg.sftp.filename_encoding.trim().is_empty() {
                    cfg.encoding.clone()
                } else {
                    cfg.sftp.filename_encoding.clone()
                };
                (
                    cfg.host.clone(),
                    cfg.port,
                    cfg.username.clone(),
                    cfg.encoding.clone(),
                    sftp_encoding,
                )
            } else {
                (
                    "unknown".to_string(),
                    22,
                    "unknown".to_string(),
                    "UTF-8".to_string(),
                    "UTF-8".to_string(),
                )
            }
        } else {
            (
                "unknown".to_string(),
                22,
                "unknown".to_string(),
                "UTF-8".to_string(),
                "UTF-8".to_string(),
            )
        };

    Ok((ssh_handle, host, port, username, encoding, sftp_encoding))
}

async fn get_or_create_auto_fs(
    manager: &SessionManager,
    session_id: &str,
) -> AppResult<Arc<AutoRemoteFs>> {
    {
        let sessions = manager.sessions.lock().await;
        let session = sessions.get(session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", session_id))
        })?;
        if !session.info.remote_file_browser_enabled {
            return Err(AppError::Config(
                "Remote file browser is disabled for this SSH connection".to_string(),
            ));
        }
        if let Some(ref fs) = session.remote_fs {
            return Ok(fs.clone());
        }
    }

    let (ssh_handle, host, port, username, _encoding, sftp_encoding) =
        get_ssh_info(manager, session_id).await?;
    let auto_fs = Arc::new(AutoRemoteFs::new(
        ssh_handle,
        &host,
        port,
        &username,
        &sftp_encoding,
    ));

    {
        let mut sessions = manager.sessions.lock().await;
        if let Some(session) = sessions.get_mut(session_id) {
            if session.remote_fs.is_none() {
                session.remote_fs = Some(auto_fs.clone());
            } else {
                return Ok(session.remote_fs.as_ref().unwrap().clone());
            }
        }
    }

    Ok(auto_fs)
}

fn join_local_child(parent: &str, name: &str) -> String {
    Path::new(parent).join(name).to_string_lossy().to_string()
}

fn join_remote_child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn file_name_from_local_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            path.split(['/', '\\'])
                .filter(|segment| !segment.is_empty())
                .next_back()
                .unwrap_or(path)
                .to_string()
        })
}

async fn resolve_local_copy_target(
    app: &tauri::AppHandle,
    manager: &SessionManager,
    session_id: &str,
    path: &str,
    file_name: &str,
    strategy: &str,
    is_directory: bool,
) -> AppResult<Option<CopyResolvedTarget>> {
    let target = PathBuf::from(path);
    if !target.exists() {
        return Ok(Some(CopyResolvedTarget {
            path: path.to_string(),
            existed: false,
        }));
    }
    let resolved = match strategy {
        "skip" => None,
        "ask" => match duplicate::prompt_duplicate_choice(
            app,
            manager,
            session_id,
            path,
            file_name,
            is_directory,
        )
        .await?
        {
            duplicate::DuplicateChoice::Skip => None,
            duplicate::DuplicateChoice::Overwrite => Some(CopyResolvedTarget {
                path: path.to_string(),
                existed: true,
            }),
        },
        "rename" => {
            let parent = target.parent().unwrap_or_else(|| Path::new("."));
            let stem = target
                .file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(if is_directory { "folder" } else { "file" });
            let ext = target
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}"))
                .unwrap_or_default();
            for index in 1..=999 {
                let candidate = parent.join(format!("{stem}({index}){ext}"));
                if !candidate.exists() {
                    return Ok(Some(CopyResolvedTarget {
                        path: candidate.to_string_lossy().to_string(),
                        existed: false,
                    }));
                }
            }
            Some(CopyResolvedTarget {
                path: path.to_string(),
                existed: true,
            })
        }
        _ => Some(CopyResolvedTarget {
            path: path.to_string(),
            existed: true,
        }),
    };
    Ok(resolved)
}

fn local_copy_sidecar_path(target: &Path, suffix: &str) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("nyaterm-copy");
    let sidecar = format!(".{name}.nyaterm-{suffix}-{}", uuid::Uuid::new_v4());
    target
        .parent()
        .map(|parent| parent.join(&sidecar))
        .unwrap_or_else(|| PathBuf::from(sidecar))
}

async fn cleanup_local_copy_temp(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

async fn commit_local_copy_temp(temp_path: &Path, target_path: &Path) -> AppResult<()> {
    let target_meta = tokio::fs::metadata(target_path).await.ok();
    if target_meta.as_ref().is_some_and(|meta| meta.is_dir()) {
        cleanup_local_copy_temp(temp_path).await;
        return Err(AppError::Channel(format!(
            "Cannot overwrite existing directory '{}' with a file",
            target_path.display()
        )));
    }

    let backup_path = target_meta
        .as_ref()
        .map(|_| local_copy_sidecar_path(target_path, "backup"));
    if let Some(backup) = backup_path.as_ref() {
        tokio::fs::rename(target_path, backup)
            .await
            .map_err(|error| {
                AppError::Channel(format!("Failed to protect existing target file: {error}"))
            })?;
    }

    let commit_result = tokio::fs::rename(temp_path, target_path)
        .await
        .map_err(|error| AppError::Channel(format!("Failed to commit copied file: {error}")));

    match commit_result {
        Ok(()) => {
            if let Some(backup) = backup_path {
                let _ = tokio::fs::remove_file(backup).await;
            }
            Ok(())
        }
        Err(error) => {
            cleanup_local_copy_temp(temp_path).await;
            if let Some(backup) = backup_path {
                let _ = tokio::fs::rename(&backup, target_path).await;
                let _ = tokio::fs::remove_file(backup).await;
            }
            Err(error)
        }
    }
}

async fn copy_local_file_staged_with_controller(
    app: &tauri::AppHandle,
    source_session_id: &str,
    source_path: &str,
    target_path: &str,
    controller: Arc<transfer::TransferController>,
) -> AppResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use transfer::{register_transfer, unregister_transfer, wait_for_transfer_ready};

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let target = PathBuf::from(target_path);
    let temp = local_copy_sidecar_path(&target, "tmp");
    let result: AppResult<u64> = async {
        let metadata = tokio::fs::metadata(source_path).await.map_err(|error| {
            AppError::Channel(format!("Failed to read source file metadata: {error}"))
        })?;
        let total_size = metadata.len();
        controller.update_progress(0, total_size);

        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                AppError::Channel(format!("Failed to create target directory: {error}"))
            })?;
        }

        let mut source = tokio::fs::File::open(source_path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open source file: {error}")))?;
        let mut temp_file = tokio::fs::File::create(&temp).await.map_err(|error| {
            AppError::Channel(format!("Failed to create temporary target file: {error}"))
        })?;
        let mut buffer = vec![0_u8; 512 * 1024];
        let mut bytes_written = 0_u64;
        let mut last_progress = std::time::Instant::now();

        loop {
            wait_for_transfer_ready(&controller).await?;
            let read = source.read(&mut buffer).await.map_err(|error| {
                AppError::Channel(format!("Failed to read source file: {error}"))
            })?;
            if read == 0 {
                break;
            }
            temp_file
                .write_all(&buffer[..read])
                .await
                .map_err(|error| {
                    AppError::Channel(format!("Failed to write temporary target file: {error}"))
                })?;
            bytes_written = bytes_written.saturating_add(read as u64);
            controller.update_progress(bytes_written, total_size);
            if last_progress.elapsed() >= std::time::Duration::from_millis(50) {
                last_progress = std::time::Instant::now();
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("progress", total_size, None),
                );
            }
        }
        temp_file.flush().await.map_err(|error| {
            AppError::Channel(format!("Failed to flush temporary target file: {error}"))
        })?;
        drop(temp_file);
        commit_local_copy_temp(&temp, &target).await?;
        Ok(bytes_written)
    }
    .await;

    match result {
        Ok(size) => {
            controller.update_progress(size, size);
            let _ = app.emit(
                "transfer-event",
                &controller.build_event("completed", size, None),
            );
            unregister_transfer(&controller.id());
            Ok(())
        }
        Err(error) => {
            cleanup_local_copy_temp(&temp).await;
            let status = if matches!(error, AppError::Cancelled(_)) {
                "cancelled"
            } else {
                "error"
            };
            let message = (status == "error").then(|| error.to_string());
            let _ = app.emit(
                "transfer-event",
                &controller.build_event(status, 0, message),
            );
            tracing::debug!(
                target: "user_action",
                action = "copy",
                entity = "local_file",
                session_id = %source_session_id,
                local_path = source_path,
                target_path = target_path,
                error = %error,
                "Local copy failed"
            );
            unregister_transfer(&controller.id());
            Err(error)
        }
    }
}

async fn ensure_local_session_kind(
    manager: &SessionManager,
    session_id: &str,
    kind: &CopyEndpointKind,
) -> AppResult<()> {
    let sessions = manager.sessions.lock().await;
    let session = sessions
        .get(session_id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Session '{session_id}' not found")))?;
    let matches_kind = match kind {
        CopyEndpointKind::Local => session.info.session_type == crate::core::SessionType::Local,
        CopyEndpointKind::Remote => session.info.session_type == crate::core::SessionType::SSH,
    };
    if matches_kind {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "Session '{session_id}' does not match requested copy endpoint kind"
        )))
    }
}

#[allow(dead_code)]
fn ssh_endpoint_fingerprint_from_config(config: &crate::core::ssh::SshConfig) -> String {
    let proxy = config
        .proxy
        .as_ref()
        .filter(|proxy| proxy.enabled)
        .map(|proxy| {
            format!(
                "{}:{}:{}:{:?}:{:?}",
                proxy.protocol, proxy.host, proxy.port, proxy.command, proxy.username
            )
        })
        .unwrap_or_default();
    let jump = config
        .proxy_jump
        .as_deref()
        .map(ssh_endpoint_fingerprint_from_config)
        .unwrap_or_default();
    let sftp_encoding = if config.sftp.filename_encoding.trim().is_empty() {
        config.encoding.clone()
    } else {
        config.sftp.filename_encoding.clone()
    };
    format!(
        "host={};port={};user={};encoding={};sftp_encoding={};proxy={};jump={}",
        config.host, config.port, config.username, config.encoding, sftp_encoding, proxy, jump
    )
}

#[allow(dead_code)]
async fn ssh_endpoint_fingerprint(
    manager: &SessionManager,
    session_id: &str,
) -> AppResult<Option<String>> {
    let sessions = manager.sessions.lock().await;
    let Some(session) = sessions.get(session_id) else {
        return Err(AppError::SessionNotFound(format!(
            "Session '{session_id}' not found"
        )));
    };
    Ok(session
        .ssh_config
        .as_ref()
        .and_then(|cfg_any| cfg_any.downcast_ref::<crate::core::ssh::SshConfig>())
        .map(ssh_endpoint_fingerprint_from_config))
}

async fn clone_sftp_backend_pair(
    manager: Arc<SessionManager>,
    source_session_id: &str,
    target_session_id: &str,
) -> AppResult<(SftpBackend, SftpBackend)> {
    let source_auto = get_or_create_auto_fs(&manager, source_session_id).await?;
    let target_auto = get_or_create_auto_fs(&manager, target_session_id).await?;
    let source_guard = source_auto.backend().await?;
    let target_guard = target_auto.backend().await?;
    let source = source_guard
        .as_ref()
        .and_then(|fs| fs.as_any().downcast_ref::<SftpBackend>())
        .ok_or_else(|| {
            AppError::Config(
                "Cross-pane remote copy requires the SFTP backend for the source session"
                    .to_string(),
            )
        })?;
    let target = target_guard
        .as_ref()
        .and_then(|fs| fs.as_any().downcast_ref::<SftpBackend>())
        .ok_or_else(|| {
            AppError::Config(
                "Cross-pane remote copy requires the SFTP backend for the target session"
                    .to_string(),
            )
        })?;
    Ok((source.clone(), target.clone()))
}

async fn clone_sftp_backend(
    manager: Arc<SessionManager>,
    session_id: &str,
) -> AppResult<SftpBackend> {
    let auto = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto.backend().await?;
    let backend = guard
        .as_ref()
        .and_then(|fs| fs.as_any().downcast_ref::<SftpBackend>())
        .ok_or_else(|| {
            AppError::Config(
                "Cross-pane copy requires the SFTP backend for SSH sessions".to_string(),
            )
        })?;
    Ok(backend.clone())
}

async fn copy_local_directory_with_controller(
    app: tauri::AppHandle,
    session_id: &str,
    source_path: &str,
    target_path: &str,
    _target_existed: bool,
    transfer_id: Option<String>,
) -> AppResult<()> {
    use transfer::{
        create_directory_transfer_controller, register_transfer, unregister_transfer,
        wait_for_transfer_ready,
    };

    let mut stack = vec![PathBuf::from(source_path)];
    let mut files = Vec::<(PathBuf, PathBuf, u64)>::new();
    let mut dirs = Vec::<(PathBuf, PathBuf)>::new();
    let mut total_size = 0_u64;
    while let Some(path) = stack.pop() {
        let relative = path.strip_prefix(source_path).unwrap_or(&path);
        dirs.push((path.clone(), Path::new(target_path).join(relative)));
        let mut read_dir = tokio::fs::read_dir(&path).await.map_err(|error| {
            AppError::Channel(format!("Failed to read local source directory: {error}"))
        })?;
        while let Some(entry) = read_dir.next_entry().await.map_err(|error| {
            AppError::Channel(format!(
                "Failed to read local source directory entry: {error}"
            ))
        })? {
            let source_child = entry.path();
            let relative = source_child
                .strip_prefix(source_path)
                .unwrap_or(&source_child);
            let target_child = Path::new(target_path).join(relative);
            let metadata = entry.metadata().await.map_err(|error| {
                AppError::Channel(format!("Failed to read local source metadata: {error}"))
            })?;
            if metadata.is_dir() {
                stack.push(source_child);
            } else if metadata.is_file() {
                total_size = total_size.saturating_add(metadata.len());
                files.push((source_child, target_child, metadata.len()));
            }
        }
    }

    let total_files = files.len() as u64;
    let controller = create_directory_transfer_controller(
        transfer_id,
        session_id,
        file_name_from_local_path(source_path),
        source_path,
        target_path,
        "copy",
        total_files,
        total_size,
    );
    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let result: AppResult<(u64, u64)> = async {
        let mut bytes_done = 0_u64;
        let mut items_done = 0_u64;
        let mut buffer = vec![0_u8; 512 * 1024];
        let mut created_dirs = Vec::<PathBuf>::new();
        for (_, target_dir) in dirs {
            if let Err(error) = wait_for_transfer_ready(&controller).await {
                for dir in created_dirs.iter().rev() {
                    let _ = tokio::fs::remove_dir(dir).await;
                }
                return Err(error);
            }
            if !target_dir.exists() {
                if let Err(error) = tokio::fs::create_dir_all(&target_dir).await {
                    for dir in created_dirs.iter().rev() {
                        let _ = tokio::fs::remove_dir(dir).await;
                    }
                    return Err(AppError::Channel(format!(
                        "Failed to create target directory: {error}"
                    )));
                }
                created_dirs.push(target_dir);
            }
        }
        for (source_file, target_file, _size) in files {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            wait_for_transfer_ready(&controller).await?;
            let temp_file_path = local_copy_sidecar_path(&target_file, "tmp");
            let mut source = tokio::fs::File::open(&source_file).await.map_err(|error| {
                AppError::Channel(format!("Failed to open local source file: {error}"))
            })?;
            let mut target = tokio::fs::File::create(&temp_file_path)
                .await
                .map_err(|error| {
                    AppError::Channel(format!("Failed to create temporary target file: {error}"))
                })?;
            let write_result: AppResult<()> = async {
                loop {
                    wait_for_transfer_ready(&controller).await?;
                    let read = source.read(&mut buffer).await.map_err(|error| {
                        AppError::Channel(format!("Failed to read local source file: {error}"))
                    })?;
                    if read == 0 {
                        break;
                    }
                    target.write_all(&buffer[..read]).await.map_err(|error| {
                        AppError::Channel(format!("Failed to write temporary target file: {error}"))
                    })?;
                    bytes_done = bytes_done.saturating_add(read as u64);
                    controller.update_progress(bytes_done, total_size);
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", 0, None),
                    );
                }
                target.flush().await.map_err(|error| {
                    AppError::Channel(format!("Failed to flush temporary target file: {error}"))
                })?;
                Ok(())
            }
            .await;
            drop(target);
            if let Err(error) = write_result {
                cleanup_local_copy_temp(&temp_file_path).await;
                for dir in created_dirs.iter().rev() {
                    let _ = tokio::fs::remove_dir(dir).await;
                }
                return Err(error);
            }
            if let Err(error) = commit_local_copy_temp(&temp_file_path, &target_file).await {
                for dir in created_dirs.iter().rev() {
                    let _ = tokio::fs::remove_dir(dir).await;
                }
                return Err(error);
            }
            items_done = items_done.saturating_add(1);
            controller.update_item_progress(items_done, total_files);
        }
        Ok((bytes_done, items_done))
    }
    .await;

    match result {
        Ok((bytes, items)) => {
            controller.update_progress(bytes, total_size);
            controller.update_item_progress(items, total_files);
            let _ = app.emit(
                "transfer-event",
                &controller.build_event("completed", 0, None),
            );
            unregister_transfer(&controller.id());
            Ok(())
        }
        Err(error) => {
            if matches!(error, AppError::Cancelled(_)) {
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("cancelled", 0, None),
                );
            } else {
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("error", 0, Some(error.to_string())),
                );
            }
            unregister_transfer(&controller.id());
            Err(error)
        }
    }
}

fn emit_copy_cancelled(
    app: &tauri::AppHandle,
    session_id: &str,
    file_name: String,
    source_path: &str,
    target_path: &str,
    is_directory: bool,
    transfer_id: Option<String>,
) {
    use transfer::{create_child_file_transfer_controller, create_directory_transfer_controller};

    let controller = if is_directory {
        create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name,
            source_path,
            target_path,
            "copy",
            0,
            0,
        )
    } else {
        create_child_file_transfer_controller(
            transfer_id,
            session_id,
            file_name,
            source_path,
            target_path,
            "copy",
            None,
        )
    };
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("cancelled", 0, Some("Copy skipped".to_string())),
    );
}

pub async fn copy_file_entry(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    request: CopyFileEntryRequest,
) -> AppResult<()> {
    let source_session_id = request.source.session_id;
    let source_kind = request.source.kind;
    let source_path = request.source.path;
    let target_session_id = request.target.session_id;
    let target_kind = request.target.kind;
    let target_dir = request.target.path;
    let file_name = request.file_name;
    let is_directory = request.is_directory;
    let transfer_id = request.transfer_id;
    let duplicate_strategy_override = request.duplicate_strategy_override;

    ensure_local_session_kind(&manager, &source_session_id, &source_kind).await?;
    ensure_local_session_kind(&manager, &target_session_id, &target_kind).await?;

    let settings = crate::config::load_app_settings(&app)
        .map(|settings| settings.transfer)
        .unwrap_or_default();
    let duplicate_strategy = duplicate_strategy_override
        .as_deref()
        .unwrap_or(&settings.duplicate_strategy);
    let target = match &target_kind {
        CopyEndpointKind::Local => {
            let target = join_local_child(&target_dir, &file_name);
            match resolve_local_copy_target(
                &app,
                manager.as_ref(),
                &target_session_id,
                &target,
                &file_name,
                duplicate_strategy,
                is_directory,
            )
            .await?
            {
                Some(target) => target,
                None => {
                    emit_copy_cancelled(
                        &app,
                        &source_session_id,
                        file_name,
                        &source_path,
                        &target,
                        is_directory,
                        transfer_id,
                    );
                    return Ok(());
                }
            }
        }
        CopyEndpointKind::Remote => {
            let target = join_remote_child_path(&target_dir, &file_name);
            let backend = clone_sftp_backend(manager.clone(), &target_session_id).await?;
            match backend
                .resolve_remote_copy_target_info(
                    &app,
                    manager.as_ref(),
                    &target_session_id,
                    &target,
                    duplicate_strategy,
                )
                .await
            {
                Some(target) => target,
                None => {
                    emit_copy_cancelled(
                        &app,
                        &source_session_id,
                        file_name,
                        &source_path,
                        &target,
                        is_directory,
                        transfer_id,
                    );
                    return Ok(());
                }
            }
        }
    };
    let target_path = target.path;
    let target_existed = target.existed;

    match (&source_kind, &target_kind, is_directory) {
        (CopyEndpointKind::Local, CopyEndpointKind::Local, false) => {
            let controller = transfer::create_child_file_transfer_controller(
                transfer_id,
                &source_session_id,
                file_name,
                &source_path,
                &target_path,
                "copy",
                None,
            );
            copy_local_file_staged_with_controller(
                &app,
                &source_session_id,
                &source_path,
                &target_path,
                controller,
            )
            .await
        }
        (CopyEndpointKind::Local, CopyEndpointKind::Local, true) => {
            copy_local_directory_with_controller(
                app,
                &source_session_id,
                &source_path,
                &target_path,
                target_existed,
                transfer_id,
            )
            .await
        }
        (CopyEndpointKind::Local, CopyEndpointKind::Remote, false) => {
            let backend = clone_sftp_backend(manager, &target_session_id).await?;
            backend
                .copy_local_file_to_remote(
                    &app,
                    &target_session_id,
                    &source_path,
                    &target_path,
                    target_existed,
                    transfer_id,
                )
                .await
        }
        (CopyEndpointKind::Local, CopyEndpointKind::Remote, true) => {
            let backend = clone_sftp_backend(manager, &target_session_id).await?;
            backend
                .copy_local_directory_to_remote(
                    &app,
                    &target_session_id,
                    &source_path,
                    &target_path,
                    target_existed,
                    transfer_id,
                )
                .await
        }
        (CopyEndpointKind::Remote, CopyEndpointKind::Local, false) => {
            let backend = clone_sftp_backend(manager, &source_session_id).await?;
            backend
                .copy_remote_file_to_local(
                    &app,
                    &source_session_id,
                    &source_path,
                    &target_path,
                    target_existed,
                    transfer_id,
                )
                .await
        }
        (CopyEndpointKind::Remote, CopyEndpointKind::Local, true) => {
            let backend = clone_sftp_backend(manager, &source_session_id).await?;
            backend
                .copy_remote_directory_to_local(
                    &app,
                    &source_session_id,
                    &source_path,
                    &target_path,
                    target_existed,
                    transfer_id,
                )
                .await
        }
        (CopyEndpointKind::Remote, CopyEndpointKind::Remote, false) => {
            let (source, target) =
                clone_sftp_backend_pair(manager, &source_session_id, &target_session_id).await?;
            source
                .copy_remote_file_to_remote_streaming(
                    &target,
                    &app,
                    &source_session_id,
                    &source_path,
                    &target_path,
                    target_existed,
                    transfer_id,
                )
                .await
        }
        (CopyEndpointKind::Remote, CopyEndpointKind::Remote, true) => {
            let (source, target) =
                clone_sftp_backend_pair(manager, &source_session_id, &target_session_id).await?;
            source
                .copy_remote_directory_to_remote_streaming(
                    &target,
                    &app,
                    &source_session_id,
                    &source_path,
                    &target_path,
                    target_existed,
                    transfer_id,
                )
                .await
        }
    }
}

pub async fn get_home_dir(manager: Arc<SessionManager>, session_id: &str) -> AppResult<String> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let result = fs.home_dir().await?;

    if result.is_empty() {
        Err(AppError::Config(
            "Failed to determine home directory".to_string(),
        ))
    } else {
        Ok(result)
    }
}

pub async fn list_remote_dir(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    raw_path_token: Option<&str>,
) -> AppResult<Vec<FileEntry>> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let path_ref = RemotePathRef::new(path, raw_path_token)?;
    let entries = fs.list_dir_ref(&path_ref).await?;

    tracing::debug!(
        target: "user_action",
        action = "list",
        entity = "remote_directory",
        session_id = %session_id,
        remote_path = path,
        item_count = entries.len(),
        "User listed remote directory"
    );

    Ok(entries)
}

fn normalize_remote_child_parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

pub async fn list_remote_child_directories(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    raw_path_token: Option<&str>,
    show_hidden_files: bool,
) -> AppResult<Vec<DirectoryChild>> {
    let entries = list_remote_dir(manager, session_id, path, raw_path_token).await?;
    let normalized_path = normalize_remote_child_parent(path);
    let mut directories: Vec<DirectoryChild> = entries
        .into_iter()
        .filter(|entry| {
            entry.is_dir
                && entry.name != "."
                && entry.name != ".."
                && (show_hidden_files || !entry.name.starts_with('.'))
        })
        .map(|entry| DirectoryChild {
            path: if normalized_path == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", normalized_path, entry.name)
            },
            name: entry.name,
            is_symlink: entry.is_symlink,
            raw_path_token: entry.raw_path_token,
        })
        .collect();
    directories.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(directories)
}

pub async fn delete_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    raw_path_token: Option<&str>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let path_ref = RemotePathRef::new(path, raw_path_token)?;
    match fs.remove_file_ref(&path_ref).await {
        Ok(()) => {}
        Err(error) if is_remote_delete_not_found(&error) => {
            tracing::debug!(
                target: "user_action",
                action = "delete",
                entity = "remote_entry",
                session_id = %session_id,
                remote_path = path,
                "Remote entry was already absent during delete"
            );
        }
        Err(error) => return Err(error),
    }

    tracing::debug!(
        target: "user_action",
        action = "delete",
        entity = "remote_entry",
        session_id = %session_id,
        remote_path = path,
        "User deleted remote entry"
    );

    Ok(())
}

const RM_DELETE_TIMEOUT: Duration = Duration::from_secs(300);

/// Delete a remote file-system entry through the user's POSIX shell.
///
/// This is intentionally separate from the normal remote-file-system delete
/// path. The file browser exposes it as an explicit alternative for cases
/// where the user needs the remote `rm` command rather than SFTP semantics.
pub async fn delete_remote_file_with_rm(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    raw_path_token: Option<&str>,
) -> AppResult<()> {
    ensure_rm_delete_session(&manager, session_id).await?;

    let path_ref = RemotePathRef::new(path, raw_path_token)?;
    let target = path_ref
        .raw_path()
        .unwrap_or_else(|| path_ref.display_path().as_bytes());
    let command = build_rm_delete_command(target)?;
    let output =
        exec_ssh_session_command(&manager, session_id, &command, RM_DELETE_TIMEOUT).await?;
    ensure_success(output, "Failed to delete remote entry with rm")?;

    tracing::debug!(
        target: "user_action",
        action = "delete_with_rm",
        entity = "remote_entry",
        session_id = %session_id,
        remote_path = path,
        "User deleted remote entry with rm"
    );

    Ok(())
}

async fn ensure_rm_delete_session(manager: &SessionManager, session_id: &str) -> AppResult<()> {
    let sessions = manager.sessions.lock().await;
    let session = sessions
        .get(session_id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Session '{session_id}' not found")))?;

    if session.info.session_type != SessionType::SSH {
        return Err(AppError::Config(
            "rm deletion requires an SSH session".to_string(),
        ));
    }
    if !session.info.remote_file_browser_enabled {
        return Err(AppError::Config(
            "Remote file browser is disabled for this SSH connection".to_string(),
        ));
    }

    Ok(())
}

fn build_rm_delete_command(target: &[u8]) -> AppResult<Vec<u8>> {
    validate_rm_delete_target(target)?;

    let quoted_target = sh_quote_bytes(target);
    let mut command = b"rm -rf -- ".to_vec();
    command.extend_from_slice(&quoted_target);
    Ok(command)
}

fn validate_rm_delete_target(target: &[u8]) -> AppResult<()> {
    if target.is_empty() || target.contains(&0) {
        return Err(AppError::Config(
            "Refusing to delete an empty or invalid remote path".to_string(),
        ));
    }

    let normalized = target
        .iter()
        .rposition(|byte| *byte != b'/')
        .map(|index| &target[..=index])
        .unwrap_or_default();

    if normalized.is_empty() || normalized == b"/" {
        return Err(AppError::Config(
            "Refusing to delete the remote root directory".to_string(),
        ));
    }
    if !normalized.starts_with(b"/") {
        return Err(AppError::Config(
            "rm deletion requires an absolute remote path".to_string(),
        ));
    }
    if normalized
        .split(|byte| *byte == b'/')
        .any(|part| part == b"." || part == b"..")
    {
        return Err(AppError::Config(
            "Refusing to delete a remote path containing '.' or '..'".to_string(),
        ));
    }

    Ok(())
}

fn sh_quote_bytes(value: &[u8]) -> Vec<u8> {
    let mut quoted = Vec::with_capacity(value.len() + 2);
    quoted.push(b'\'');
    for byte in value {
        if *byte == b'\'' {
            quoted.extend_from_slice(b"'\\''");
        } else {
            quoted.push(*byte);
        }
    }
    quoted.push(b'\'');
    quoted
}

pub async fn rename_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    old_path: &str,
    new_path: &str,
    old_raw_path_token: Option<&str>,
    new_raw_path_token: Option<&str>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let old_path_ref = RemotePathRef::new(old_path, old_raw_path_token)?;
    let new_path_ref = RemotePathRef::new(new_path, new_raw_path_token)?;
    fs.rename_ref(&old_path_ref, &new_path_ref).await?;

    tracing::debug!(
        target: "user_action",
        action = "update",
        entity = "remote_entry",
        session_id = %session_id,
        old_path = old_path,
        new_path = new_path,
        "User renamed or moved remote entry"
    );

    Ok(())
}

pub async fn download_remote_file(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    transfer_id: Option<String>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let transfer_settings = crate::config::load_app_settings(&app)
        .map(|s| s.transfer)
        .unwrap_or_default();
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.download_file(
        &app,
        session_id,
        remote_path,
        local_path,
        &transfer_settings,
        transfer_id,
    )
    .await
}

pub async fn upload_local_file(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
    transfer_id: Option<String>,
    duplicate_strategy_override: Option<String>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let mut transfer_settings = crate::config::load_app_settings(&app)
        .map(|s| s.transfer)
        .unwrap_or_default();
    if let Some(strategy) = duplicate_strategy_override {
        transfer_settings.duplicate_strategy = strategy;
    }
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.upload_file(
        &app,
        session_id,
        local_path,
        remote_path,
        &transfer_settings,
        transfer_id,
    )
    .await
}

pub async fn get_file_properties(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    raw_path_token: Option<&str>,
) -> AppResult<FileProperties> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let path_ref = RemotePathRef::new(path, raw_path_token)?;
    let props = fs.stat_ref(&path_ref).await?;

    tracing::debug!(
        target: "user_action",
        action = "read",
        entity = "remote_properties",
        session_id = %session_id,
        remote_path = path,
        "User read remote entry properties"
    );

    Ok(props)
}

pub async fn read_remote_file_text(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    max_bytes: u64,
) -> AppResult<RemoteTextFile> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.read_file_text(path, max_bytes).await
}

pub async fn read_remote_file_bytes(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    max_bytes: u64,
) -> AppResult<RemoteBinaryFile> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.read_file_bytes(path, max_bytes).await
}

pub async fn write_remote_file_text(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    content: &str,
    expected_mtime: Option<u64>,
    expected_size: Option<u64>,
    force: bool,
) -> AppResult<WriteRemoteTextResult> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.write_file_text(path, content, expected_mtime, expected_size, force)
        .await
}

pub async fn create_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    mode: Option<String>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.create_file(path, mode.clone()).await?;

    tracing::debug!(
        target: "user_action",
        action = "create",
        entity = "remote_file",
        session_id = %session_id,
        remote_path = path,
        requested_mode = ?mode,
        "User created remote file"
    );

    Ok(())
}

pub async fn create_remote_dir(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    mode: Option<String>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.mkdir(path, mode.clone()).await?;

    tracing::debug!(
        target: "user_action",
        action = "create",
        entity = "remote_directory",
        session_id = %session_id,
        remote_path = path,
        requested_mode = ?mode,
        "User created remote directory"
    );

    Ok(())
}

pub async fn create_remote_symlink(
    manager: Arc<SessionManager>,
    session_id: &str,
    link_path: &str,
    target_path: &str,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.create_symlink(link_path, target_path).await?;

    tracing::debug!(
        target: "user_action",
        action = "create",
        entity = "remote_symlink",
        session_id = %session_id,
        remote_path = link_path,
        target_path = target_path,
        "User created remote symlink"
    );

    Ok(())
}

pub async fn chmod_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    mode: &str,
) -> AppResult<()> {
    update_remote_file_attributes(
        manager,
        session_id,
        path,
        None,
        RemoteFileAttributeUpdate {
            mode: Some(mode.to_string()),
            owner: None,
            group: None,
            recursive: false,
        },
    )
    .await
}

pub async fn update_remote_file_attributes(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    raw_path_token: Option<&str>,
    update: RemoteFileAttributeUpdate,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let path_ref = RemotePathRef::new(path, raw_path_token)?;
    fs.update_attrs_ref(&path_ref, &update).await?;

    tracing::debug!(
        target: "user_action",
        action = "update",
        entity = "remote_attributes",
        session_id = %session_id,
        remote_path = path,
        requested_mode = ?update.mode,
        requested_owner = ?update.owner,
        requested_group = ?update.group,
        recursive = update.recursive,
        "User changed remote file attributes"
    );

    Ok(())
}

pub async fn download_remote_directory(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    transfer_id: Option<String>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.download_directory(&app, session_id, remote_path, local_path, transfer_id)
        .await
}

pub async fn upload_local_directory(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
    transfer_id: Option<String>,
    duplicate_strategy_override: Option<String>,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let mut transfer_settings = crate::config::load_app_settings(&app)
        .map(|s| s.transfer)
        .unwrap_or_default();
    if let Some(strategy) = duplicate_strategy_override {
        transfer_settings.duplicate_strategy = strategy;
    }
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.upload_directory(
        &app,
        session_id,
        local_path,
        remote_path,
        &transfer_settings,
        transfer_id,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        build_rm_delete_command, cleanup_local_copy_temp, commit_local_copy_temp, get_home_dir,
        ssh_endpoint_fingerprint_from_config, validate_rm_delete_target,
    };
    use crate::config::{AiExecutionProfile, ProxySettings, SftpSettings};
    use crate::core::ssh::{SshAuth, SshConfig};
    use crate::core::{SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::{Mutex, mpsc};

    fn temp_test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("nyaterm-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("create temp test dir");
        path
    }

    #[test]
    fn rm_delete_command_quotes_shell_metacharacters() {
        let command = build_rm_delete_command(b"/tmp/a b'c;$()").expect("build rm delete command");

        assert_eq!(command, b"rm -rf -- '/tmp/a b'\\''c;$()'");
    }

    #[test]
    fn rm_delete_command_preserves_non_utf8_path_bytes() {
        let command = build_rm_delete_command(b"/tmp/file-\xff").expect("build rm delete command");
        let mut expected = b"rm -rf -- '/tmp/file-".to_vec();
        expected.push(0xff);
        expected.push(b'\'');

        assert_eq!(command, expected);
    }

    #[test]
    fn rm_delete_rejects_unsafe_targets() {
        let unsafe_targets: &[&[u8]] = &[
            b"",
            b"/",
            b"////",
            b".",
            b"..",
            b"relative/path",
            b"/tmp/../etc",
            b"/tmp/./file",
            b"/tmp/bad\0path",
        ];

        for target in unsafe_targets {
            assert!(
                validate_rm_delete_target(target).is_err(),
                "target should be rejected: {target:?}"
            );
        }
    }

    fn test_ssh_config(host: &str, port: u16, username: &str) -> SshConfig {
        SshConfig {
            connection_id: None,
            owner_window_label: None,
            name: format!("{username}@{host}:{port}"),
            host: host.to_string(),
            port,
            username: username.to_string(),
            auth: SshAuth::None,
            backspace_mode: "auto".to_string(),
            x11_forwarding: false,
            x11_display: String::new(),
            proxy: None,
            proxy_jump: None,
            post_login: None,
            ssh_algorithms: None,
            sftp: SftpSettings::default(),
            encoding: "UTF-8".to_string(),
        }
    }

    #[test]
    fn ssh_endpoint_fingerprint_matches_same_endpoint() {
        let left = test_ssh_config("example.com", 22, "alice");
        let right = test_ssh_config("example.com", 22, "alice");

        assert_eq!(
            ssh_endpoint_fingerprint_from_config(&left),
            ssh_endpoint_fingerprint_from_config(&right)
        );
    }

    #[test]
    fn ssh_endpoint_fingerprint_tracks_endpoint_identity() {
        let base = test_ssh_config("example.com", 22, "alice");
        let different_host = test_ssh_config("other.example.com", 22, "alice");
        let different_port = test_ssh_config("example.com", 2222, "alice");
        let different_user = test_ssh_config("example.com", 22, "bob");
        let mut different_encoding = test_ssh_config("example.com", 22, "alice");
        different_encoding.sftp.filename_encoding = "GBK".to_string();
        let mut with_proxy = test_ssh_config("example.com", 22, "alice");
        with_proxy.proxy = Some(ProxySettings {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 1080,
            ..ProxySettings::default()
        });
        let mut with_jump = test_ssh_config("example.com", 22, "alice");
        with_jump.proxy_jump = Some(Box::new(test_ssh_config("jump.example.com", 22, "jump")));

        let base_fp = ssh_endpoint_fingerprint_from_config(&base);
        for config in [
            different_host,
            different_port,
            different_user,
            different_encoding,
            with_proxy,
            with_jump,
        ] {
            assert_ne!(base_fp, ssh_endpoint_fingerprint_from_config(&config));
        }
    }

    #[tokio::test]
    async fn local_copy_temp_cleanup_preserves_existing_target() {
        let dir = temp_test_dir("copy-cleanup-preserves-target");
        let target = dir.join("target.txt");
        let temp = dir.join(".target.txt.nyaterm-tmp-test");
        fs::write(&target, b"original").expect("write target");
        fs::write(&temp, b"partial").expect("write temp");

        cleanup_local_copy_temp(&temp).await;

        assert_eq!(fs::read(&target).expect("read target"), b"original");
        assert!(!temp.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn local_copy_commit_overwrites_file_after_complete_temp_write() {
        let dir = temp_test_dir("copy-commit-overwrite");
        let target = dir.join("target.txt");
        let temp = dir.join(".target.txt.nyaterm-tmp-test");
        fs::write(&target, b"original").expect("write target");
        fs::write(&temp, b"replacement").expect("write temp");

        commit_local_copy_temp(&temp, &target)
            .await
            .expect("commit temp");

        assert_eq!(fs::read(&target).expect("read target"), b"replacement");
        assert!(!temp.exists());
        let backups: Vec<_> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("backup"))
            .collect();
        assert!(backups.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn local_copy_commit_rejects_directory_target_without_deleting_it() {
        let dir = temp_test_dir("copy-commit-dir-target");
        let target = dir.join("target");
        let nested = target.join("existing.txt");
        let temp = dir.join(".target.nyaterm-tmp-test");
        fs::create_dir_all(&target).expect("create target dir");
        fs::write(&nested, b"keep").expect("write nested target");
        fs::write(&temp, b"replacement").expect("write temp");

        let error = commit_local_copy_temp(&temp, &target)
            .await
            .expect_err("directory target should be rejected");

        assert!(
            error
                .to_string()
                .contains("Cannot overwrite existing directory")
        );
        assert_eq!(fs::read(&nested).expect("read nested target"), b"keep");
        assert!(!temp.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn disabled_remote_file_browser_rejects_sftp_commands() {
        let manager = Arc::new(SessionManager::new());
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
        manager
            .add_session(SessionHandle {
                info: SessionInfo {
                    id: "ssh-disabled-files".to_string(),
                    name: "ssh-disabled-files".to_string(),
                    session_type: SessionType::SSH,
                    connected: true,
                    owner_window_label: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    injection_active: true,
                    remote_file_browser_enabled: false,
                },
                cmd_tx,
                ssh_config: None,
                ssh_handle: None,
                cwd: Arc::new(Mutex::new(None)),
                remote_fs: None,
            })
            .await;

        let error = get_home_dir(manager, "ssh-disabled-files")
            .await
            .expect_err("remote file browser should be blocked");

        assert!(
            error
                .to_string()
                .contains("Remote file browser is disabled")
        );
    }
}
