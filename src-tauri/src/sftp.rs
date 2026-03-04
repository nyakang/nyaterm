//! Remote file operations via the SFTP subsystem (russh-sftp).
//!
//! Reuses the existing SSH connection via channel multiplexing instead of
//! creating a new TCP connection for each operation.

use crate::error::{AppError, AppResult};
use crate::session::SessionManager;
use crate::ssh::SshHandler;
use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use serde::Serialize;
use std::sync::Arc;
use tauri::Emitter;

/// Event payload emitted to the frontend to track file transfer lifecycle.
#[derive(Debug, Clone, Serialize)]
pub struct TransferEvent {
    pub id: String,
    pub session_id: String,
    pub file_name: String,
    /// "upload" or "download"
    pub direction: String,
    /// "started", "progress", "completed", or "error"
    pub status: String,
    pub size: u64,
    pub bytes_transferred: u64,
    pub total_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
}

/// Parsed entry from SFTP readdir for the file explorer.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub permissions: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileProperties {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
    pub uid: String,
    pub gid: String,
    pub mtime: u64,
    pub atime: u64,
}

/// Opens an SFTP session by reusing the existing SSH connection's handle.
async fn open_sftp(
    manager: &SessionManager,
    session_id: &str,
) -> AppResult<SftpSession> {
    let handle = {
        let sessions = manager.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", session_id)))?;

        session
            .ssh_handle
            .as_ref()
            .ok_or_else(|| AppError::Config("Not an SSH session".to_string()))?
            .clone()
            .downcast::<client::Handle<SshHandler>>()
            .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?
    };

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| AppError::Channel(format!("Failed to open SFTP channel: {}", e)))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| AppError::Channel(format!("Failed to start SFTP subsystem: {}", e)))?;

    let sftp = SftpSession::new(channel.into_stream()).await?;
    Ok(sftp)
}

/// Convert a SFTP permission bitmask (u32) to the classic `ls -l` string like `-rwxr-xr-x`.
/// `type_char` should be `'d'` for directories, `'l'` for symlinks, or `'-'` for regular files.
fn permissions_to_string(mode: u32, type_char: char) -> String {
    let mut s = String::with_capacity(10);

    s.push(type_char);

    // Owner
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(match (mode & 0o100 != 0, mode & 0o4000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    // Group
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(match (mode & 0o010 != 0, mode & 0o2000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    // Other
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(match (mode & 0o001 != 0, mode & 0o1000 != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });

    s
}

/// Resolves `$HOME` on the remote host via SFTP `canonicalize(".")`.
pub async fn get_home_dir(
    manager: Arc<SessionManager>,
    session_id: &str,
) -> AppResult<String> {
    let sftp = open_sftp(&manager, session_id).await?;
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

/// Lists a remote directory via SFTP `read_dir`.
pub async fn list_remote_dir(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
) -> AppResult<Vec<FileEntry>> {
    let sftp = open_sftp(&manager, session_id).await?;

    let dir = sftp.read_dir(path).await?;
    let _ = sftp.close().await;

    let mut entries = Vec::new();
    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        // read_dir uses lstat semantics per SFTP spec, so FileType correctly reflects
        // the entry itself (a symlink will be Symlink, not the type of its target).
        let file_type = entry.file_type();
        let is_dir = file_type == FileType::Dir;
        let is_symlink = file_type == FileType::Symlink;
        let type_char = if is_dir { 'd' } else if is_symlink { 'l' } else { '-' };

        let attrs = entry.metadata();
        let size = attrs.size.unwrap_or(0);
        let perms = attrs.permissions.unwrap_or(0);
        let permissions = permissions_to_string(perms, type_char);

        entries.push(FileEntry {
            name,
            is_dir,
            is_symlink,
            size,
            permissions,
        });
    }

    Ok(entries)
}

pub async fn delete_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
) -> AppResult<()> {
    let sftp = open_sftp(&manager, session_id).await?;

    let meta = sftp.metadata(path).await?;
    // Use the full POSIX type mask (S_IFMT = 0o170000) so that symlinks and other
    // special files are never mistakenly treated as directories.
    let is_dir = meta.permissions.map_or(false, |p| (p & 0o170000) == 0o040000);

    if is_dir {
        remove_dir_recursive(&sftp, path).await?;
    } else {
        // Covers regular files, symlinks, devices, etc.
        sftp.remove_file(path).await?;
    }

    let _ = sftp.close().await;
    Ok(())
}

/// Recursively remove a directory and all its contents via SFTP.
///
/// Continues deleting as many entries as possible on partial permission failures,
/// collecting all errors and returning them as one combined message at the end.
/// Symlinks are removed directly without following their targets.
async fn remove_dir_recursive(sftp: &SftpSession, path: &str) -> AppResult<()> {
    // Strip any trailing slashes to avoid double-slash paths like /var/www//file
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
            // Symlinks and regular files are both removed with remove_file so we
            // never accidentally recurse into a symlinked directory.
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

pub async fn rename_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    old_path: &str,
    new_path: &str,
) -> AppResult<()> {
    let sftp = open_sftp(&manager, session_id).await?;
    sftp.rename(old_path, new_path).await?;
    let _ = sftp.close().await;
    Ok(())
}

pub async fn download_remote_file(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
) -> AppResult<()> {
    use std::io::SeekFrom;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    use tokio::task::JoinSet;

    let file_name = remote_path.split('/').last().unwrap_or(remote_path).to_string();
    let transfer_id = uuid::Uuid::new_v4().to_string();

    let make_event = |status: &str, bytes_transferred: u64, total_size: u64, error_msg: Option<String>| {
        TransferEvent {
            id: transfer_id.clone(),
            session_id: session_id.to_string(),
            file_name: file_name.clone(),
            direction: "download".to_string(),
            status: status.to_string(),
            size: total_size,
            bytes_transferred,
            total_size,
            error_msg,
        }
    };

    let _ = app.emit("transfer-event", &make_event("started", 0, 0, None));

    let result: AppResult<u64> = async {
        if let Some(parent) = std::path::Path::new(local_path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;
            }
        }

        let sftp = open_sftp(&manager, session_id).await?;

        let total_size = sftp.metadata(remote_path).await
            .map(|m| m.size.unwrap_or(0))
            .unwrap_or(0);

        let mut local_file = tokio::fs::File::create(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local file: {}", e)))?;

        if total_size > 0 {
            let _ = local_file.set_len(total_size).await;
        }

        const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
        let mut last_progress = Instant::now();
        let mut bytes_transferred: u64 = 0;

        if total_size > 0 {
            // ── Pipelined path ─────────────────────────────────────────────
            //
            // Open CONCURRENCY independent SFTP File handles all sharing the
            // same Arc<RawSftpSession>. RawSftpSession uses a concurrent hash
            // map keyed by request-id so every handle can have one
            // SSH_FXP_READ in flight simultaneously, filling the pipe.
            //
            // Sliding-window pattern: when a task finishes it returns its
            // handle so we immediately re-enqueue the next chunk on it,
            // keeping the pipeline full for the entire transfer.

            // 128 KiB per request (OpenSSH caps responses at ~64 KiB, so each
            // task may do 1-2 round trips; 16 × 64 KiB ≈ 1 MiB in-flight).
            const CHUNK_SIZE: u64 = 128 * 1024;
            const CONCURRENCY: usize = 16;

            let num_chunks = ((total_size + CHUNK_SIZE - 1) / CHUNK_SIZE) as usize;
            let concurrency = CONCURRENCY.min(num_chunks);

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

            // Seed the pipeline
            while let Some(fh) = handle_pool.pop() {
                if next_offset >= total_size {
                    break;
                }
                let len = CHUNK_SIZE.min(total_size - next_offset) as usize;
                let offset = next_offset;
                next_offset += len as u64;

                join_set.spawn(async move {
                    let mut f = fh;
                    // seek(SeekFrom::Start) is a pure local offset update — no
                    // network round trip. The offset is sent as part of the next
                    // SSH_FXP_READ request.
                    f.seek(SeekFrom::Start(offset)).await
                        .map_err(|e| AppError::Channel(format!("Seek failed: {}", e)))?;
                    let mut buf = vec![0u8; len];
                    f.read_exact(&mut buf).await
                        .map_err(|e| AppError::Channel(format!("SFTP read failed: {}", e)))?;
                    Ok((offset, buf, f))
                });
            }

            // Drain results and keep the pipeline full
            while let Some(res) = join_set.join_next().await {
                let (chunk_offset, data, fh) = res
                    .map_err(|e| AppError::Channel(format!("Task panicked: {}", e)))??;

                // All writes are sequential in this loop; seek + write on a
                // single local handle is safe and needs no mutex.
                local_file.seek(SeekFrom::Start(chunk_offset)).await
                    .map_err(|e| AppError::Channel(format!("Local seek failed: {}", e)))?;
                local_file.write_all(&data).await
                    .map_err(|e| AppError::Channel(format!("Local write failed: {}", e)))?;

                bytes_transferred += data.len() as u64;

                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    let _ = app.emit(
                        "transfer-event",
                        &make_event("progress", bytes_transferred, total_size, None),
                    );
                }

                // Refill: recycle the returned handle for the next chunk
                if next_offset < total_size {
                    let len = CHUNK_SIZE.min(total_size - next_offset) as usize;
                    let offset = next_offset;
                    next_offset += len as u64;

                    join_set.spawn(async move {
                        let mut f = fh;
                        f.seek(SeekFrom::Start(offset)).await
                            .map_err(|e| AppError::Channel(format!("Seek failed: {}", e)))?;
                        let mut buf = vec![0u8; len];
                        f.read_exact(&mut buf).await
                            .map_err(|e| AppError::Channel(format!("SFTP read failed: {}", e)))?;
                        Ok((offset, buf, f))
                    });
                }
                // No more chunks → `fh` drops here, triggering SSH_FXP_CLOSE.
            }
        } else {
            // ── Sequential fallback (file size unknown) ─────────────────────
            // Used when the server does not report a size in metadata (e.g.
            // virtual/proc files). Pipelining requires knowing offsets upfront.
            let mut remote_file = sftp.open(remote_path).await
                .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?;

            const SEQ_CHUNK: usize = 512 * 1024;
            let mut buf = vec![0u8; SEQ_CHUNK];
            loop {
                let n = remote_file.read(&mut buf).await
                    .map_err(|e| AppError::Channel(format!("SFTP read failed: {}", e)))?;
                if n == 0 {
                    break;
                }
                local_file.write_all(&buf[..n]).await
                    .map_err(|e| AppError::Channel(format!("Write failed: {}", e)))?;
                bytes_transferred += n as u64;

                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    let _ = app.emit(
                        "transfer-event",
                        &make_event("progress", bytes_transferred, 0, None),
                    );
                }
            }
        }

        local_file.flush().await
            .map_err(|e| AppError::Channel(format!("Flush failed: {}", e)))?;

        let _ = sftp.close().await;

        Ok(bytes_transferred)
    }.await;

    match result {
        Ok(size) => {
            let _ = app.emit("transfer-event", &make_event("completed", size, size, None));
            Ok(())
        }
        Err(e) => {
            let _ = app.emit("transfer-event", &make_event("error", 0, 0, Some(e.to_string())));
            Err(e)
        }
    }
}

pub async fn upload_local_file(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
) -> AppResult<()> {
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let file_name = remote_path.split('/').last().unwrap_or(remote_path).to_string();
    let transfer_id = uuid::Uuid::new_v4().to_string();

    let make_event = |status: &str, bytes_transferred: u64, total_size: u64, error_msg: Option<String>| {
        TransferEvent {
            id: transfer_id.clone(),
            session_id: session_id.to_string(),
            file_name: file_name.clone(),
            direction: "upload".to_string(),
            status: status.to_string(),
            size: total_size,
            bytes_transferred,
            total_size,
            error_msg,
        }
    };

    let _ = app.emit("transfer-event", &make_event("started", 0, 0, None));

    let result: AppResult<u64> = async {
        // Get file size from metadata instead of reading the whole file into memory
        let total_size = tokio::fs::metadata(local_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open local file: {}", e)))?;

        let sftp = open_sftp(&manager, session_id).await?;

        let mut remote_file = sftp.create(remote_path).await?;

        // Match the download chunk size for symmetric throughput
        const CHUNK_SIZE: usize = 512 * 1024;
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut bytes_transferred: u64 = 0;

        const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
        let mut last_progress = Instant::now();

        loop {
            let n = local_file.read(&mut buf).await
                .map_err(|e| AppError::Channel(format!("Failed to read local file: {}", e)))?;
            if n == 0 {
                break;
            }
            remote_file.write_all(&buf[..n]).await
                .map_err(|e| AppError::Channel(format!("SFTP write failed: {}", e)))?;
            bytes_transferred += n as u64;

            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                let _ = app.emit("transfer-event", &make_event("progress", bytes_transferred, total_size, None));
            }
        }

        remote_file.shutdown().await
            .map_err(|e| AppError::Channel(format!("SFTP flush failed: {}", e)))?;

        let _ = sftp.close().await;

        Ok(bytes_transferred)
    }.await;

    match result {
        Ok(size) => {
            let _ = app.emit("transfer-event", &make_event("completed", size, size, None));
            Ok(())
        }
        Err(e) => {
            let _ = app.emit("transfer-event", &make_event("error", 0, 0, Some(e.to_string())));
            Err(e)
        }
    }
}



pub async fn chmod_remote_file(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    mode: &str,
) -> AppResult<()> {
    let mode_u32 = u32::from_str_radix(mode, 8)
        .map_err(|_| AppError::Channel(format!("Invalid octal mode: {}", mode)))?;

    let sftp = open_sftp(&manager, session_id).await?;

    let mut attrs = sftp.metadata(path).await?;
    attrs.permissions = Some(mode_u32);
    sftp.set_metadata(path, attrs).await?;

    let _ = sftp.close().await;
    Ok(())
}

pub async fn get_file_properties(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
) -> AppResult<FileProperties> {
    let sftp = open_sftp(&manager, session_id).await?;
    let attrs = sftp.metadata(path).await?;
    let _ = sftp.close().await;

    let perms = attrs.permissions.unwrap_or(0);
    // Apply S_IFMT mask (0o170000) for reliable type detection across all SFTP servers.
    let type_bits = perms & 0o170000;
    let is_dir = type_bits == 0o040000;
    let is_symlink = type_bits == 0o120000;
    let type_char = if is_dir { 'd' } else if is_symlink { 'l' } else { '-' };
    let permissions = permissions_to_string(perms, type_char);
    let name = path.split('/').last().unwrap_or(path).to_string();

    Ok(FileProperties {
        name,
        is_dir,
        is_symlink,
        size: attrs.size.unwrap_or(0),
        permissions,
        owner: attrs.user.unwrap_or_default(),
        group: attrs.group.unwrap_or_default(),
        uid: attrs.uid.map_or_else(String::new, |v| v.to_string()),
        gid: attrs.gid.map_or_else(String::new, |v| v.to_string()),
        mtime: u64::from(attrs.mtime.unwrap_or(0)),
        atime: u64::from(attrs.atime.unwrap_or(0)),
    })
}
