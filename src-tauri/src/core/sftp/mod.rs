//! Auto-fallback remote file system.
//!
//! Transparently picks the best available backend for each SSH session:
//! SFTP subsystem → SCP Enhanced (find/stat/tar) → SCP Normal (ls/cat).
//! The upper layers and the frontend never need to know which protocol is in use.

mod cache;
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

use crate::core::ssh::SshConnectionHandles;
use crate::core::SessionManager;
use crate::error::{AppError, AppResult};
use std::sync::Arc;
use tokio::sync::RwLock;

pub(crate) use transfer::{active_transfer_count, transfer_target_directory};
pub use transfer::{cancel_transfer, pause_transfer, resume_transfer};
pub use util::{FileEntry, FileProperties, RemoteTextFile};

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
) -> AppResult<(Arc<SshConnectionHandles>, String, u16, String)> {
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

    let (host, port, username) = if let Some(ref cfg_any) = session.ssh_config {
        if let Some(cfg) = cfg_any.downcast_ref::<crate::core::ssh::SshConfig>() {
            (cfg.host.clone(), cfg.port, cfg.username.clone())
        } else {
            ("unknown".to_string(), 22, "unknown".to_string())
        }
    } else {
        ("unknown".to_string(), 22, "unknown".to_string())
    };

    Ok((ssh_handle, host, port, username))
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
        if let Some(ref fs) = session.remote_fs {
            return Ok(fs.clone());
        }
    }

    let (ssh_handle, host, port, username) = get_ssh_info(manager, session_id).await?;
    let auto_fs = Arc::new(AutoRemoteFs::new(ssh_handle, &host, port, &username));

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
    fs.remove_file(path).await?;

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
    )
    .await
}

pub async fn upload_local_file(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let transfer_settings = crate::config::load_app_settings(&app)
        .map(|s| s.transfer)
        .unwrap_or_default();
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.upload_file(
        &app,
        session_id,
        local_path,
        remote_path,
        &transfer_settings,
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
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.chmod(path, mode).await?;

    tracing::debug!(
        target: "user_action",
        action = "update",
        entity = "remote_permissions",
        session_id = %session_id,
        remote_path = path,
        requested_mode = mode,
        "User changed remote permissions"
    );

    Ok(())
}

pub async fn download_remote_directory(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.download_directory(&app, session_id, remote_path, local_path)
        .await
}

pub async fn upload_local_directory(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
) -> AppResult<()> {
    let auto_fs = get_or_create_auto_fs(&manager, session_id).await?;
    let guard = auto_fs.backend().await?;
    let fs = guard.as_ref().unwrap();
    fs.upload_directory(&app, session_id, local_path, remote_path)
        .await
}
