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

use crate::core::SessionManager;
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Information needed to create an independent SSH connection for file
/// operations.  This avoids opening channels on the shell's SSH connection,
/// which can kill the entire transport on servers that don't support
/// multiple channels (e.g. dropbear/Termux on port 8022).
struct SftpConnectionInfo {
    /// Saved-connection ID — used to load credentials and create a fresh
    /// authenticated SSH connection via `create_ssh_handle`.
    connection_id: Option<String>,
    /// Fallback: the existing shared SSH handle (used when connection_id
    /// is not available, e.g. for temporary sessions).
    shared_handle: Arc<SshConnectionHandles>,
    host: String,
    port: u16,
    username: String,
}

pub(crate) use duplicate::TransferDuplicateManager;
pub(crate) use transfer::{active_transfer_count, transfer_target_directory};
pub use transfer::{cancel_transfer, pause_transfer, resume_transfer};
pub(crate) use util::sanitize_download_file_name;
pub use util::{
    FileEntry, FileProperties, RemoteFileAttributeUpdate, RemoteTextFile, WriteRemoteTextResult,
};

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
}

impl AutoRemoteFs {
    pub(crate) fn new(
        ssh_handle: Arc<SshConnectionHandles>,
        host: &str,
        port: u16,
        username: &str,
    ) -> Self {
        Self {
            inner: RwLock::new(None),
            ssh_handle,
            cache_key: cache_key(host, port, username),
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
                return Ok(Box::new(SftpBackend::new(self.ssh_handle.clone())));
            }
            Err(e) => {
                let reason = e.to_string();
                tracing::debug!(error = %reason, "SFTP backend unavailable, trying SCP Enhanced");
                sftp_failure = Some(reason.clone());
                // If the error indicates the transport was disconnected,
                // probing further will only fail again and may have already
                // killed the shell session.  Bail out early.
                if reason.contains("Disconnected") || reason.contains("early eof") {
                    return Err(AppError::Channel(
                        "Terminal connection is working, but the remote file manager could not be initialized".to_string(),
                    ));
                }
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
                if e.to_string().contains("Disconnected") || e.to_string().contains("early eof") {
                    return Err(AppError::Channel(
                        "Terminal connection is working, but the remote file manager could not be initialized".to_string(),
                    ));
                }
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
                        Box::new(SftpBackend::new(self.ssh_handle.clone()))
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
) -> AppResult<SftpConnectionInfo> {
    let sessions = manager.sessions.lock().await;
    let session = sessions
        .get(session_id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", session_id)))?;

    let shared_handle = session
        .ssh_handle
        .as_ref()
        .ok_or_else(|| AppError::Config("Not an SSH session".to_string()))?
        .clone()
        .downcast::<SshConnectionHandles>()
        .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?;

    let (connection_id, host, port, username) = if let Some(ref cfg_any) = session.ssh_config {
        if let Some(cfg) = cfg_any.downcast_ref::<crate::core::ssh::SshConfig>() {
            (cfg.connection_id.clone(), cfg.host.clone(), cfg.port, cfg.username.clone())
        } else {
            (None, "unknown".to_string(), 22, "unknown".to_string())
        }
    } else {
        (None, "unknown".to_string(), 22, "unknown".to_string())
    };

    Ok(SftpConnectionInfo {
        connection_id,
        shared_handle,
        host,
        port,
        username,
    })
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

    let info = get_ssh_info(manager, session_id).await?;

    // Try to create an independent SSH connection for file operations.
    // This is critical for servers that don't support multiple channels on
    // the same connection (e.g. dropbear/Termux on port 8022).  If we open
    // a second channel on the shell's connection, the server may drop the
    // entire transport, killing the terminal session.
    let ssh_handle = if let (Some(conn_id), Some(app)) = (&info.connection_id, manager.app()) {
        match crate::core::ssh::create_ssh_handle(&app, conn_id).await {
            Ok(handle) => {
                tracing::info!(
                    session_id,
                    "Created independent SSH connection for file operations"
                );
                handle
            }
            Err(error) => {
                tracing::warn!(
                    session_id,
                    %error,
                    "Failed to create independent SSH connection for SFTP, \
                     falling back to shared connection"
                );
                info.shared_handle
            }
        }
    } else {
        // No connection_id (temporary session) or no app handle — fall back
        // to the shared connection.
        info.shared_handle
    };

    let auto_fs = Arc::new(AutoRemoteFs::new(ssh_handle, &info.host, info.port, &info.username));

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
) -> AppResult<Vec<FileEntry>> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let entries = fs.list_dir(path).await?;

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

pub async fn delete_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    match fs.remove_file(path).await {
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

pub async fn rename_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    old_path: &str,
    new_path: &str,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.rename(old_path, new_path).await?;

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
) -> AppResult<FileProperties> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    let props = fs.stat(path).await?;

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
    update: RemoteFileAttributeUpdate,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.update_attrs(path, &update).await?;

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
    use super::get_home_dir;
    use crate::config::AiExecutionProfile;
    use crate::core::{SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType};
    use std::sync::Arc;
    use tokio::sync::{Mutex, mpsc};

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
