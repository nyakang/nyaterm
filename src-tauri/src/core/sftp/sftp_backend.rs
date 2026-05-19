//! SFTP backend: standard SSH SFTP subsystem via russh-sftp.
//!
//! This is the preferred backend when the server supports `Subsystem sftp`.

use super::traits::RemoteFs;
use super::transfer::*;
use super::util::*;
use crate::core::ssh::{SshConnectionHandles, SshRawHandle};
use crate::error::{AppError, AppResult};
use crate::observability::{log_event, StructuredLog, StructuredLogLevel};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags};
use std::sync::Arc;
use tauri::Emitter;

pub(crate) struct SftpBackend {
    ssh_handle: Arc<SshConnectionHandles>,
}

impl SftpBackend {
    pub(crate) fn new(ssh_handle: Arc<SshConnectionHandles>) -> Self {
        Self { ssh_handle }
    }

    /// Attempt to open a throwaway SFTP session to verify subsystem availability.
    pub(crate) async fn probe(ssh_handle: &Arc<SshConnectionHandles>) -> AppResult<()> {
        let sftp = Self::open_sftp_raw(ssh_handle.target_handle()).await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn open_sftp_raw(handle_mtx: SshRawHandle) -> AppResult<SftpSession> {
        let channel = {
            let handle = handle_mtx.lock().await;
            handle
                .channel_open_session()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open SFTP channel: {}", e)))?
        };
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| AppError::Channel(format!("Failed to start SFTP subsystem: {}", e)))?;

        let sftp = SftpSession::new(channel.into_stream()).await?;
        Ok(sftp)
    }

    async fn open_sftp(&self) -> AppResult<SftpSession> {
        Self::open_sftp_raw(self.ssh_handle.target_handle()).await
    }
}

async fn remove_dir_recursive(sftp: &SftpSession, path: &str) -> AppResult<()> {
    let path = path.trim_end_matches('/');
    let dir = sftp.read_dir(path).await?;
    let mut errors: Vec<String> = Vec::new();

    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child = format!("{}/{}", path, name);
        let file_type = entry.file_type();

        if file_type == FileType::Dir {
            if let Err(e) = Box::pin(remove_dir_recursive(sftp, &child)).await {
                errors.push(e.to_string());
            }
        } else {
            if let Err(e) = sftp.remove_file(&child).await {
                errors.push(format!("'{}': {}", child, e));
            }
        }
    }

    if !errors.is_empty() {
        return Err(AppError::Channel(format!(
            "{} item(s) could not be deleted:\n{}",
            errors.len(),
            errors.join("\n")
        )));
    }

    sftp.remove_dir(path)
        .await
        .map_err(|e| AppError::Channel(format!("Failed to remove directory '{}': {}", path, e)))
}

async fn resolve_remote_path(
    sftp: &SftpSession,
    remote_path: &str,
    strategy: &str,
) -> Option<String> {
    let exists = sftp.metadata(remote_path).await.is_ok();
    if !exists {
        return Some(remote_path.to_string());
    }
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
        _ => Some(remote_path.to_string()),
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
    use std::io::SeekFrom;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    use tokio::task::JoinSet;

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let chunk_size = (ts.transfer_buffer_size as u64).max(1) * 1024;
    let concurrency = (ts.download_threads as usize).max(1);

    let result: AppResult<u64> = async {
        if let Some(parent) = std::path::Path::new(&actual_path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;
            }
        }

        let sftp = backend.open_sftp().await?;

        let remote_attrs = sftp.metadata(remote_path).await.ok();
        let total_size = remote_attrs.as_ref().and_then(|m| m.size).unwrap_or(0);
        controller.update_progress(0, total_size);

        let mut local_file = tokio::fs::File::create(&actual_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local file: {}", e)))?;

        if total_size > 0 {
            let _ = local_file.set_len(total_size).await;
        }

        const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
        let mut last_progress = Instant::now();
        let mut bytes_transferred: u64 = 0;

        if total_size > 0 {
            let num_chunks = ((total_size + chunk_size - 1) / chunk_size) as usize;
            let concurrency = concurrency.min(num_chunks);

            let mut handle_pool: Vec<russh_sftp::client::fs::File> =
                Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                handle_pool.push(sftp.open(remote_path).await.map_err(|e| {
                    AppError::Channel(format!("Failed to open remote file: {}", e))
                })?);
            }

            type Task = AppResult<(u64, Vec<u8>, russh_sftp::client::fs::File)>;
            let mut join_set: JoinSet<Task> = JoinSet::new();
            let mut next_offset: u64 = 0;

            while let Some(fh) = handle_pool.pop() {
                if next_offset >= total_size {
                    break;
                }
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let len = chunk_size.min(total_size - next_offset) as usize;
                let offset = next_offset;
                next_offset += len as u64;

                join_set.spawn(async move {
                    let mut f = fh;
                    f.seek(SeekFrom::Start(offset))
                        .await
                        .map_err(|e| AppError::Channel(format!("Seek failed: {}", e)))?;
                    let mut buf = vec![0u8; len];
                    f.read_exact(&mut buf)
                        .await
                        .map_err(|e| AppError::Channel(format!("SFTP read failed: {}", e)))?;
                    Ok((offset, buf, f))
                });
            }

            while let Some(res) = join_set.join_next().await {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let (chunk_offset, data, fh) =
                    res.map_err(|e| AppError::Channel(format!("Task panicked: {}", e)))??;

                local_file
                    .seek(SeekFrom::Start(chunk_offset))
                    .await
                    .map_err(|e| AppError::Channel(format!("Local seek failed: {}", e)))?;
                local_file
                    .write_all(&data)
                    .await
                    .map_err(|e| AppError::Channel(format!("Local write failed: {}", e)))?;

                bytes_transferred += data.len() as u64;
                controller.update_progress(bytes_transferred, total_size);

                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                }

                if next_offset < total_size {
                    wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                    let len = chunk_size.min(total_size - next_offset) as usize;
                    let offset = next_offset;
                    next_offset += len as u64;

                    join_set.spawn(async move {
                        let mut f = fh;
                        f.seek(SeekFrom::Start(offset))
                            .await
                            .map_err(|e| AppError::Channel(format!("Seek failed: {}", e)))?;
                        let mut buf = vec![0u8; len];
                        f.read_exact(&mut buf)
                            .await
                            .map_err(|e| AppError::Channel(format!("SFTP read failed: {}", e)))?;
                        Ok((offset, buf, f))
                    });
                }
            }
        } else {
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

                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = Instant::now();
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
    use std::io::SeekFrom;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    use tokio::task::JoinSet;

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let chunk_size = ((ts.transfer_buffer_size as usize).max(1)) * 1024;
    let concurrency = (ts.upload_threads as usize).max(1);

    let result: AppResult<u64> = async {
        let local_meta = tokio::fs::metadata(local_path).await;
        let total_size = local_meta.as_ref().map(|m| m.len()).unwrap_or(0);
        controller.update_progress(0, total_size);

        let sftp = backend.open_sftp().await?;
        let mut bytes_transferred: u64 = 0;

        const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
        let mut last_progress = Instant::now();

        let mut bootstrap_file = sftp.create(remote_path).await?;
        bootstrap_file
            .shutdown()
            .await
            .map_err(|e| AppError::Channel(format!("SFTP flush failed: {}", e)))?;

        if total_size > 0 {
            let num_chunks = total_size.div_ceil(chunk_size as u64) as usize;
            let concurrency = concurrency.min(num_chunks);

            let mut handle_pool: Vec<(tokio::fs::File, russh_sftp::client::fs::File)> =
                Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                let local_file = tokio::fs::File::open(local_path)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to open local file: {}", e)))?;
                let remote_file = sftp
                    .open_with_flags(remote_path, OpenFlags::WRITE)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?;
                handle_pool.push((local_file, remote_file));
            }

            type Task = AppResult<(usize, tokio::fs::File, russh_sftp::client::fs::File)>;
            let mut join_set: JoinSet<Task> = JoinSet::new();
            let mut next_offset: u64 = 0;

            while let Some((local_fh, remote_fh)) = handle_pool.pop() {
                if next_offset >= total_size {
                    break;
                }
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let len = chunk_size.min((total_size - next_offset) as usize);
                let offset = next_offset;
                next_offset += len as u64;

                join_set.spawn(async move {
                    let mut local = local_fh;
                    let mut remote = remote_fh;
                    local
                        .seek(SeekFrom::Start(offset))
                        .await
                        .map_err(|e| AppError::Channel(format!("Local seek failed: {}", e)))?;
                    let mut buf = vec![0u8; len];
                    local.read_exact(&mut buf).await.map_err(|e| {
                        AppError::Channel(format!("Failed to read local file: {}", e))
                    })?;
                    remote
                        .seek(SeekFrom::Start(offset))
                        .await
                        .map_err(|e| AppError::Channel(format!("Remote seek failed: {}", e)))?;
                    remote
                        .write_all(&buf)
                        .await
                        .map_err(|e| AppError::Channel(format!("SFTP write failed: {}", e)))?;
                    Ok((buf.len(), local, remote))
                });
            }

            while let Some(res) = join_set.join_next().await {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let (written, local_fh, remote_fh) =
                    res.map_err(|e| AppError::Channel(format!("Task panicked: {}", e)))??;

                bytes_transferred += written as u64;
                controller.update_progress(bytes_transferred, total_size);

                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                }

                if next_offset < total_size {
                    wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                    let len = chunk_size.min((total_size - next_offset) as usize);
                    let offset = next_offset;
                    next_offset += len as u64;

                    join_set.spawn(async move {
                        let mut local = local_fh;
                        let mut remote = remote_fh;
                        local
                            .seek(SeekFrom::Start(offset))
                            .await
                            .map_err(|e| AppError::Channel(format!("Local seek failed: {}", e)))?;
                        let mut buf = vec![0u8; len];
                        local.read_exact(&mut buf).await.map_err(|e| {
                            AppError::Channel(format!("Failed to read local file: {}", e))
                        })?;
                        remote
                            .seek(SeekFrom::Start(offset))
                            .await
                            .map_err(|e| AppError::Channel(format!("Remote seek failed: {}", e)))?;
                        remote
                            .write_all(&buf)
                            .await
                            .map_err(|e| AppError::Channel(format!("SFTP write failed: {}", e)))?;
                        Ok((buf.len(), local, remote))
                    });
                } else {
                    let mut remote = remote_fh;
                    let _ = remote.shutdown().await;
                }
            }
        }

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
        let _ = sftp.close().await;

        let mut entries = Vec::new();
        for entry in dir {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let file_type = entry.file_type();
            let is_dir = file_type == FileType::Dir;
            let is_symlink = file_type == FileType::Symlink;
            let type_char = if is_dir {
                'd'
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

            entries.push(FileEntry {
                name,
                is_dir,
                is_symlink,
                size,
                permissions,
                owner: owner_or_id(&attrs.user, attrs.uid),
                group: group_or_id(&attrs.group, attrs.gid),
                mtime,
            });
        }

        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<FileProperties> {
        let sftp = self.open_sftp().await?;
        let attrs = sftp.metadata(path).await?;
        let _ = sftp.close().await;

        let perms = attrs.permissions.unwrap_or(0);
        let type_bits = perms & 0o170000;
        let is_dir = type_bits == 0o040000;
        let is_symlink = type_bits == 0o120000;
        let type_char = if is_dir {
            'd'
        } else if is_symlink {
            'l'
        } else {
            '-'
        };
        let permissions = permissions_to_string(perms, type_char);
        let name = path.split('/').last().unwrap_or(path).to_string();

        Ok(FileProperties {
            name,
            is_dir,
            is_symlink,
            size: attrs.size.unwrap_or(0),
            permissions,
            owner: owner_or_id(&attrs.user, attrs.uid),
            group: group_or_id(&attrs.group, attrs.gid),
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
        let meta = sftp.metadata(path).await?;
        let is_dir = meta
            .permissions
            .map_or(false, |p| (p & 0o170000) == 0o040000);

        if is_dir {
            remove_dir_recursive(&sftp, path).await?;
        } else {
            sftp.remove_file(path).await?;
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
        sftp.symlink(link_path, target_path).await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn chmod(&self, path: &str, mode: &str) -> AppResult<()> {
        let mode_u32 = parse_octal_mode(mode)?;
        let sftp = self.open_sftp().await?;
        apply_remote_mode(&sftp, path, mode_u32).await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
        use tokio::io::AsyncReadExt;

        let sftp = self.open_sftp().await?;
        let attrs = sftp.metadata(path).await?;
        let size = attrs.size.unwrap_or(0);
        let type_bits = attrs.permissions.unwrap_or(0) & SFTP_FILE_TYPE_MASK;
        if type_bits == 0o040000 {
            let _ = sftp.close().await;
            return Err(AppError::Config(
                "Directories are not supported for AI file analysis".to_string(),
            ));
        }
        if size > max_bytes {
            let _ = sftp.close().await;
            return Err(AppError::Config(format!(
                "File is too large for AI analysis ({} bytes > {} bytes)",
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

        if bytes.len() as u64 > max_bytes {
            return Err(AppError::Config(format!(
                "File is too large for AI analysis ({} bytes > {} bytes)",
                bytes.len(),
                max_bytes
            )));
        }
        if bytes.contains(&0) {
            return Err(AppError::Config(
                "Binary files are not supported for AI analysis".to_string(),
            ));
        }
        let content = String::from_utf8(bytes).map_err(|_| {
            AppError::Config("Only UTF-8 text files are supported for AI analysis".to_string())
        })?;

        Ok(RemoteTextFile {
            path: path.to_string(),
            content,
            size,
        })
    }

    async fn download_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let actual_local_path =
            match resolve_local_path(local_path, &transfer_settings.duplicate_strategy) {
                Some(path) => path,
                None => {
                    let file_name = remote_path.split('/').last().unwrap_or(remote_path);
                    let transfer_id = uuid::Uuid::new_v4().to_string();
                    remember_transfer_target_external(
                        transfer_id.clone(),
                        local_path.to_string(),
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
                            local_path: local_path.to_string(),
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
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let sftp_for_resolve = self.open_sftp().await?;
        let actual_remote_path = match resolve_remote_path(
            &sftp_for_resolve,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            Some(path) => path,
            None => {
                let file_name = remote_path.split('/').last().unwrap_or(remote_path);
                let transfer_id = uuid::Uuid::new_v4().to_string();
                let _ = app.emit(
                    "transfer-event",
                    &TransferEvent {
                        id: transfer_id,
                        session_id: session_id.to_string(),
                        file_name: file_name.to_string(),
                        remote_path: remote_path.to_string(),
                        local_path: local_path.to_string(),
                        direction: "upload".to_string(),
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
    ) -> AppResult<()> {
        let total_files = self.count_remote_files(remote_path).await?;
        let directory_controller = create_directory_transfer_controller(
            session_id,
            file_name_from_path(remote_path),
            remote_path,
            local_path,
            "download",
            total_files,
        );
        register_transfer(directory_controller.clone());
        let _ = app.emit(
            "transfer-event",
            &directory_controller.build_event("started", 0, None),
        );

        let mut completed_count = 0;
        let result = self
            .download_remote_directory_inner(
                app,
                session_id,
                remote_path,
                local_path,
                directory_controller.clone(),
                &mut completed_count,
            )
            .await;

        match result {
            Ok(()) => {
                directory_controller.update_item_progress(completed_count, total_files);
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
    ) -> AppResult<()> {
        let total_files = count_local_files(local_path).await?;
        let directory_controller = create_directory_transfer_controller(
            session_id,
            file_name_from_path(local_path),
            remote_path,
            local_path,
            "upload",
            total_files,
        );
        register_transfer(directory_controller.clone());
        let _ = app.emit(
            "transfer-event",
            &directory_controller.build_event("started", 0, None),
        );

        let mut completed_count = 0;
        let result = self
            .upload_local_directory_inner(
                app,
                session_id,
                local_path,
                remote_path,
                directory_controller.clone(),
                &mut completed_count,
            )
            .await;

        match result {
            Ok(()) => {
                directory_controller.update_item_progress(completed_count, total_files);
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

impl SftpBackend {
    async fn count_remote_files(&self, remote_path: &str) -> AppResult<u64> {
        let mut count = 0;
        let mut stack = vec![remote_path.to_string()];
        while let Some(path) = stack.pop() {
            let entries = self.list_dir(&path).await?;
            for entry in entries {
                let child_remote = format!("{}/{}", path.trim_end_matches('/'), entry.name);
                if entry.is_dir {
                    stack.push(child_remote);
                } else if !entry.is_symlink {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    async fn download_remote_directory_inner(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        directory_controller: Arc<TransferController>,
        completed_count: &mut u64,
    ) -> AppResult<()> {
        wait_for_transfer_ready(&directory_controller).await?;

        tokio::fs::create_dir_all(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;

        let entries = self.list_dir(remote_path).await?;

        for entry in entries {
            wait_for_transfer_ready(&directory_controller).await?;

            let child_remote = format!("{}/{}", remote_path.trim_end_matches('/'), entry.name);
            let child_local = format!("{}/{}", local_path.trim_end_matches('/'), entry.name);

            if entry.is_dir {
                Box::pin(self.download_remote_directory_inner(
                    app,
                    session_id,
                    &child_remote,
                    &child_local,
                    directory_controller.clone(),
                    completed_count,
                ))
                .await?;
            } else if !entry.is_symlink {
                let child_controller = create_child_file_transfer_controller(
                    session_id,
                    entry.name.clone(),
                    &child_remote,
                    &child_local,
                    "download",
                    Some(directory_controller.id()),
                );
                let ts = crate::config::load_app_settings(app)
                    .map(|s| s.transfer)
                    .unwrap_or_default();
                download_remote_file_inner_with_controller(
                    self,
                    app,
                    session_id,
                    &child_remote,
                    &child_local,
                    &ts,
                    child_controller,
                    Some(directory_controller.clone()),
                )
                .await?;
                *completed_count += 1;
                let total = directory_controller
                    .item_count_total()
                    .unwrap_or(*completed_count);
                directory_controller.update_item_progress(*completed_count, total);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("progress", 0, None),
                );
            }
        }

        Ok(())
    }

    async fn upload_local_directory_inner(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        directory_controller: Arc<TransferController>,
        completed_count: &mut u64,
    ) -> AppResult<()> {
        wait_for_transfer_ready(&directory_controller).await?;

        let sftp = self.open_sftp().await?;
        let _ = sftp.create_dir(remote_path).await;
        let _ = sftp.close().await;

        let mut read_dir = tokio::fs::read_dir(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to read local dir: {}", e)))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| AppError::Channel(format!("Failed to read dir entry: {}", e)))?
        {
            wait_for_transfer_ready(&directory_controller).await?;

            let file_type = entry
                .file_type()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to get file type: {}", e)))?;
            let entry_name = entry.file_name().to_string_lossy().to_string();
            let child_local = format!(
                "{}/{}",
                local_path.trim_end_matches(['/', '\\']),
                entry_name
            );
            let child_remote = format!("{}/{}", remote_path.trim_end_matches('/'), entry_name);

            if file_type.is_dir() {
                Box::pin(self.upload_local_directory_inner(
                    app,
                    session_id,
                    &child_local,
                    &child_remote,
                    directory_controller.clone(),
                    completed_count,
                ))
                .await?;
            } else if file_type.is_file() {
                let child_controller = create_child_file_transfer_controller(
                    session_id,
                    entry_name,
                    &child_remote,
                    &child_local,
                    "upload",
                    Some(directory_controller.id()),
                );
                let ts = crate::config::load_app_settings(app)
                    .map(|s| s.transfer)
                    .unwrap_or_default();
                upload_local_file_inner_with_controller(
                    self,
                    app,
                    session_id,
                    &child_local,
                    &child_remote,
                    &ts,
                    child_controller,
                    Some(directory_controller.clone()),
                )
                .await?;
                *completed_count += 1;
                let total = directory_controller
                    .item_count_total()
                    .unwrap_or(*completed_count);
                directory_controller.update_item_progress(*completed_count, total);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("progress", 0, None),
                );
            }
        }

        Ok(())
    }
}
