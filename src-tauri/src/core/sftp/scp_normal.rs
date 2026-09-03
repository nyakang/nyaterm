//! SCP Normal backend: last-resort fallback for minimal/BusyBox systems.
//!
//! Uses only basic commands (`ls`, `cat`, `mkdir`, `rm`, `mv`) so it works on
//! virtually any POSIX-like system even when `find`, `stat`, and `tar` are
//! unavailable.

use super::traits::RemoteFs;
use super::transfer::*;
use super::util::*;
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use crate::observability::{StructuredLog, StructuredLogLevel, log_event};
use russh::ChannelMsg;
use std::sync::Arc;
use tauri::{Emitter, Manager};

pub(crate) struct ScpNormalBackend {
    ssh_handle: Arc<SshConnectionHandles>,
}

struct ExecResult {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<u32>,
}

impl ScpNormalBackend {
    pub(crate) fn new(ssh_handle: Arc<SshConnectionHandles>) -> Self {
        Self { ssh_handle }
    }

    pub(crate) async fn probe(ssh_handle: &Arc<SshConnectionHandles>) -> AppResult<()> {
        let result = exec_command(
            ssh_handle,
            "command -v ls && command -v cat && command -v mkdir && command -v rm && command -v mv",
        )
        .await?;
        if result.exit_code != Some(0) {
            return Err(AppError::Channel(
                "SCP normal probe failed: required commands not available".to_string(),
            ));
        }
        Ok(())
    }

    async fn exec(&self, command: &str) -> AppResult<ExecResult> {
        exec_command(&self.ssh_handle, command).await
    }

    async fn exec_with_stdin(&self, command: &str, stdin: &[u8]) -> AppResult<ExecResult> {
        exec_command_with_stdin(&self.ssh_handle, command, Some(stdin)).await
    }

    async fn exec_ok(&self, command: &str) -> AppResult<String> {
        let result = self.exec(command).await?;
        if result.exit_code != Some(0) {
            let stderr_text = String::from_utf8_lossy(&result.stderr);
            return Err(AppError::Channel(format!(
                "Command failed (exit {}): {}",
                result.exit_code.unwrap_or(255),
                stderr_text.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    }

    async fn count_remote_files(&self, remote_path: &str) -> AppResult<u64> {
        let mut count = 0u64;
        let mut stack = vec![remote_path.to_string()];
        while let Some(path) = stack.pop() {
            let entries = self.list_dir(&path).await?;
            for entry in entries {
                let child = format!("{}/{}", path.trim_end_matches('/'), entry.name);
                if entry.is_dir {
                    stack.push(child);
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
            let child_local = append_safe_local_child_path(local_path, &entry.name);

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
                    None,
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
                download_file_inner(
                    &self.ssh_handle,
                    app,
                    &child_remote,
                    &child_local,
                    child_controller,
                    Some(directory_controller.clone()),
                    &ts,
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

        let _ = self
            .exec(&format!("mkdir -p -- {}", sh_quote(remote_path)))
            .await;

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
                    None,
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
                upload_file_inner(
                    &self.ssh_handle,
                    app,
                    &child_local,
                    &child_remote,
                    child_controller,
                    Some(directory_controller.clone()),
                    &ts,
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

async fn exec_command(
    ssh_handle: &Arc<SshConnectionHandles>,
    command: &str,
) -> AppResult<ExecResult> {
    exec_command_with_stdin(ssh_handle, command, None).await
}

async fn exec_command_with_stdin(
    ssh_handle: &Arc<SshConnectionHandles>,
    command: &str,
    stdin: Option<&[u8]>,
) -> AppResult<ExecResult> {
    let handle_mtx = ssh_handle.target_handle();
    let mut channel = {
        let handle = handle_mtx.lock().await;
        handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?
    };

    channel.exec(true, command.as_bytes()).await?;
    if let Some(stdin) = stdin {
        channel.data(stdin).await?;
        channel.eof().await?;
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: Option<u32> = None;

    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
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
        stdout,
        stderr,
        exit_code,
    })
}

fn split_ls_fields(line: &str) -> Option<(Vec<&str>, &str)> {
    let bytes = line.as_bytes();
    let mut fields = Vec::with_capacity(8);
    let mut cursor = 0;
    while fields.len() < 8 {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if start == cursor {
            return None;
        }
        fields.push(&line[start..cursor]);
    }
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    (cursor < bytes.len()).then(|| (fields, &line[cursor..]))
}

fn list_dir_command(path: &str) -> String {
    format!("LC_ALL=C ls -la -n -- {}", sh_quote(path))
}

fn stat_command(path: &str) -> String {
    format!("LC_ALL=C ls -lad -n -- {}", sh_quote(path))
}

fn parse_ls_line(line: &str) -> Option<FileEntry> {
    let line = line.trim_start();
    if line.trim_end().is_empty() || line.starts_with("total ") {
        return None;
    }

    let (parts, raw_name) = split_ls_fields(line)?;

    let perms = parts[0];
    if perms.len() < 10 {
        return None;
    }

    let is_dir = perms.starts_with('d');
    let is_symlink = perms.starts_with('l');

    let owner = parts[2].to_string();
    let group = parts[3].to_string();
    let size: u64 = parts[4].parse().unwrap_or(0);

    if raw_name.is_empty() {
        return None;
    }

    let name = if is_symlink {
        if let Some(pos) = raw_name.find(" -> ") {
            raw_name[..pos].to_string()
        } else {
            raw_name.to_string()
        }
    } else {
        raw_name.to_string()
    };

    if name == "." || name == ".." {
        return None;
    }

    Some(FileEntry {
        name,
        is_dir,
        is_symlink,
        size,
        permissions: perms.to_string(),
        owner,
        group,
        mtime: 0,
        raw_path_token: None,
    })
}

fn remote_child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn parse_ls_line_to_properties(line: &str, path: &str) -> AppResult<FileProperties> {
    let line = line.trim_start();
    let Some((parts, raw_name)) = split_ls_fields(line) else {
        return Err(AppError::Channel(format!(
            "Failed to parse stat output for '{}'",
            path
        )));
    };

    let perms = parts[0];
    if perms.len() < 10 {
        return Err(AppError::Channel(format!(
            "Invalid permissions field for '{}'",
            path
        )));
    }

    let is_dir = perms.starts_with('d');
    let is_symlink = perms.starts_with('l');

    let owner = parts[2].to_string();
    let group = parts[3].to_string();
    let size: u64 = parts[4].parse().unwrap_or(0);

    let (name, symlink_target) = if is_symlink {
        if let Some(pos) = raw_name.find(" -> ") {
            (
                raw_name[..pos].to_string(),
                Some(raw_name[pos + " -> ".len()..].to_string()),
            )
        } else {
            return Err(AppError::Channel(format!(
                "Symbolic link target was missing from stat output for '{}'",
                path
            )));
        }
    } else {
        (raw_name.to_string(), None)
    };

    Ok(FileProperties {
        name,
        is_dir,
        is_symlink,
        symlink_target,
        size,
        permissions: perms.to_string(),
        owner: owner.clone(),
        group: group.clone(),
        uid: owner,
        gid: group,
        mtime: 0,
        atime: 0,
    })
}

async fn stat_remote_properties(
    ssh_handle: &Arc<SshConnectionHandles>,
    path: &str,
) -> AppResult<FileProperties> {
    let result = exec_command(ssh_handle, &stat_command(path)).await?;
    if result.exit_code != Some(0) {
        let stderr_text = String::from_utf8_lossy(&result.stderr);
        return Err(AppError::Channel(format!(
            "Failed to stat remote file: {}",
            stderr_text.trim()
        )));
    }
    let line = String::from_utf8_lossy(&result.stdout)
        .lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with("total "))
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Channel(format!("No stat output for '{}'", path)))?;
    parse_ls_line_to_properties(&line, path)
}

async fn download_file_inner(
    ssh_handle: &Arc<SshConnectionHandles>,
    app: &tauri::AppHandle,
    remote_path: &str,
    local_path: &str,
    controller: Arc<TransferController>,
    parent_controller: Option<Arc<TransferController>>,
    _ts: &crate::config::TransferSettings,
) -> AppResult<()> {
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let result: AppResult<u64> = async {
        if let Some(parent) = std::path::Path::new(local_path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;
            }
        }

        let handle_mtx = ssh_handle.target_handle();
        let mut channel = {
            let handle = handle_mtx.lock().await;
            handle
                .channel_open_session()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?
        };

        let cmd = format!("cat -- {}", sh_quote(remote_path));
        channel.exec(true, cmd.as_bytes()).await?;

        let mut local_file = tokio::fs::File::create(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local file: {}", e)))?;

        const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
        let mut last_progress = Instant::now();
        let mut bytes_transferred: u64 = 0;
        let mut remote_stderr = Vec::new();
        let mut exit_code: Option<u32> = None;

        loop {
            if let Some(ref parent) = parent_controller {
                wait_for_transfer_ready(parent).await?;
            }
            wait_for_transfer_ready(&controller).await?;

            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    local_file.write_all(&data).await.map_err(|e| {
                        AppError::Channel(format!("Failed to write local file: {}", e))
                    })?;
                    bytes_transferred += data.len() as u64;
                    controller.update_progress(bytes_transferred, 0);

                    if last_progress.elapsed() >= PROGRESS_INTERVAL {
                        last_progress = Instant::now();
                        emit_parent_progress(app, parent_controller.as_ref());
                        let _ = app.emit(
                            "transfer-event",
                            &controller.build_event("progress", 0, None),
                        );
                    }
                }
                Some(ChannelMsg::ExtendedData { data, ext }) => {
                    if ext == 1 {
                        remote_stderr.extend_from_slice(&data);
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

        local_file
            .flush()
            .await
            .map_err(|e| AppError::Channel(format!("Flush failed: {}", e)))?;

        if exit_code != Some(0) && bytes_transferred == 0 {
            let stderr_text = String::from_utf8_lossy(&remote_stderr);
            return Err(AppError::Channel(format!(
                "Download failed: {}",
                stderr_text.trim()
            )));
        }

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
                cleanup_cancelled_download(local_path).await;
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

async fn upload_file_inner(
    ssh_handle: &Arc<SshConnectionHandles>,
    app: &tauri::AppHandle,
    local_path: &str,
    remote_path: &str,
    controller: Arc<TransferController>,
    parent_controller: Option<Arc<TransferController>>,
    _ts: &crate::config::TransferSettings,
) -> AppResult<()> {
    use std::time::{Duration, Instant};
    use tokio::io::AsyncReadExt;

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let tmp_path = format!("{}.__nyaterm_tmp_{}", remote_path, uuid::Uuid::new_v4());

    let result: AppResult<u64> = async {
        let local_meta = tokio::fs::metadata(local_path).await;
        let total_size = local_meta.as_ref().map(|m| m.len()).unwrap_or(0);
        controller.update_progress(0, total_size);
        let original_props = stat_remote_properties(ssh_handle, remote_path).await.ok();

        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open local file: {}", e)))?;

        let handle_mtx = ssh_handle.target_handle();
        let mut channel = {
            let handle = handle_mtx.lock().await;
            handle
                .channel_open_session()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?
        };

        let cmd = format!("cat > {}", sh_quote(&tmp_path));
        channel.exec(true, cmd.as_bytes()).await?;

        const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
        let mut last_progress = Instant::now();
        let mut bytes_transferred: u64 = 0;
        let chunk_size: usize = 32 * 1024;
        let mut buf = vec![0u8; chunk_size];

        loop {
            if let Some(ref parent) = parent_controller {
                wait_for_transfer_ready(parent).await?;
            }
            wait_for_transfer_ready(&controller).await?;

            let n = local_file
                .read(&mut buf)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to read local file: {}", e)))?;
            if n == 0 {
                break;
            }

            channel
                .data(&buf[..n])
                .await
                .map_err(|e| AppError::Channel(format!("Channel write failed: {}", e)))?;

            bytes_transferred += n as u64;
            controller.update_progress(bytes_transferred, total_size);

            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                emit_parent_progress(app, parent_controller.as_ref());
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("progress", total_size, None),
                );
            }
        }

        channel
            .eof()
            .await
            .map_err(|e| AppError::Channel(format!("Channel EOF failed: {}", e)))?;

        let mut exit_code: Option<u32> = None;
        loop {
            match channel.wait().await {
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

        if exit_code != Some(0) {
            let _ = exec_command(ssh_handle, &format!("rm -f -- {}", sh_quote(&tmp_path))).await;
            return Err(AppError::Channel(format!(
                "Upload cat exited with code {}",
                exit_code.unwrap_or(255)
            )));
        }

        let mv_result = exec_command(
            ssh_handle,
            &scp_finalize_replace_command(&tmp_path, remote_path, original_props.as_ref()),
        )
        .await?;

        if mv_result.exit_code != Some(0) {
            let _ = exec_command(ssh_handle, &format!("rm -f -- {}", sh_quote(&tmp_path))).await;
            let stderr_text = String::from_utf8_lossy(&mv_result.stderr);
            return Err(AppError::Channel(format!(
                "Failed to move uploaded file: {}",
                stderr_text.trim()
            )));
        }

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
                let _ =
                    exec_command(ssh_handle, &format!("rm -f -- {}", sh_quote(&tmp_path))).await;
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

async fn resolve_remote_exists(ssh_handle: &Arc<SshConnectionHandles>, remote_path: &str) -> bool {
    exec_command(ssh_handle, &format!("test -e {}", sh_quote(remote_path)))
        .await
        .map(|r| r.exit_code == Some(0))
        .unwrap_or(false)
}

async fn resolve_remote_is_directory(
    ssh_handle: &Arc<SshConnectionHandles>,
    remote_path: &str,
) -> bool {
    exec_command(ssh_handle, &format!("test -d {}", sh_quote(remote_path)))
        .await
        .map(|r| r.exit_code == Some(0))
        .unwrap_or(false)
}

async fn resolve_remote_path(
    app: &tauri::AppHandle,
    session_manager: &crate::core::SessionManager,
    ssh_handle: &Arc<SshConnectionHandles>,
    session_id: &str,
    remote_path: &str,
    strategy: &str,
) -> Option<String> {
    let exists = resolve_remote_exists(ssh_handle, remote_path).await;
    if !exists {
        return Some(remote_path.to_string());
    }
    let file_name = file_name_from_path(remote_path);
    let is_directory = resolve_remote_is_directory(ssh_handle, remote_path).await;
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
                if !resolve_remote_exists(ssh_handle, &candidate).await {
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
                &file_name,
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
    ssh_handle: &Arc<SshConnectionHandles>,
    session_id: &str,
    remote_path: &str,
    strategy: &str,
) -> bool {
    if !resolve_remote_exists(ssh_handle, remote_path).await {
        return true;
    }

    let file_name = file_name_from_path(remote_path);
    let is_directory = resolve_remote_is_directory(ssh_handle, remote_path).await;

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

#[async_trait::async_trait]
impl RemoteFs for ScpNormalBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn backend_name(&self) -> &'static str {
        "scp-normal"
    }

    async fn home_dir(&self) -> AppResult<String> {
        let output = self.exec_ok("echo ~").await?;
        let home = output.trim().to_string();
        if home.is_empty() {
            Err(AppError::Config(
                "Failed to determine home directory".to_string(),
            ))
        } else {
            Ok(home)
        }
    }

    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let listing_path = remote_dir_listing_path(path);
        let output = self.exec_ok(&list_dir_command(&listing_path)).await?;
        let mut entries = Vec::new();
        for line in output.lines() {
            if let Some(mut entry) = parse_ls_line(line) {
                if entry.is_symlink {
                    let child_path = remote_child_path(path, &entry.name);
                    entry.is_dir = self
                        .exec(&format!("test -d {}", sh_quote(&child_path)))
                        .await
                        .map_or(entry.is_dir, |result| result.exit_code == Some(0));
                }
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<FileProperties> {
        let output = self.exec_ok(&stat_command(path)).await?;
        let line = output
            .lines()
            .find(|l| !l.trim().is_empty() && !l.starts_with("total "))
            .ok_or_else(|| AppError::Channel(format!("No stat output for '{}'", path)))?;
        let mut props = parse_ls_line_to_properties(line, path)?;
        if props.is_symlink {
            props.is_dir = self
                .exec(&format!("test -d {}", sh_quote(path)))
                .await
                .map_or(props.is_dir, |result| result.exit_code == Some(0));
        }
        Ok(props)
    }

    async fn mkdir(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        self.exec_ok(&format!("mkdir -p -- {}", sh_quote(path)))
            .await?;
        if let Some(ref m) = mode {
            let _ = parse_octal_mode(m)?;
            self.exec_ok(&format!("chmod {} -- {}", sh_quote(m), sh_quote(path)))
                .await?;
        }
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> AppResult<()> {
        self.exec_ok(&format!("rm -rf -- {}", sh_quote(path)))
            .await?;
        Ok(())
    }

    async fn rename(&self, old_path: &str, new_path: &str) -> AppResult<()> {
        self.exec_ok(&format!(
            "mv -- {} {}",
            sh_quote(old_path),
            sh_quote(new_path)
        ))
        .await?;
        Ok(())
    }

    async fn create_file(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        self.exec_ok(&format!("touch -- {}", sh_quote(path)))
            .await?;
        if let Some(ref m) = mode {
            let _ = parse_octal_mode(m)?;
            self.exec_ok(&format!("chmod {} -- {}", sh_quote(m), sh_quote(path)))
                .await?;
        }
        Ok(())
    }

    async fn create_symlink(&self, link_path: &str, target_path: &str) -> AppResult<()> {
        self.exec_ok(&format!(
            "ln -s -- {} {}",
            sh_quote(target_path),
            sh_quote(link_path)
        ))
        .await?;
        Ok(())
    }

    async fn update_attrs(&self, path: &str, update: &RemoteFileAttributeUpdate) -> AppResult<()> {
        let flag = if update.recursive { "-R " } else { "" };
        if let Some(mode) = update
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            let _ = parse_octal_mode(mode)?;
            self.exec_ok(&format!(
                "chmod {}{} -- {}",
                flag,
                sh_quote(mode),
                sh_quote(path)
            ))
            .await?;
        }
        if let Some(owner) = update
            .owner
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            self.exec_ok(&format!(
                "chown {}{} -- {}",
                flag,
                sh_quote(owner),
                sh_quote(path)
            ))
            .await?;
        }
        if let Some(group) = update
            .group
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            self.exec_ok(&format!(
                "chgrp {}{} -- {}",
                flag,
                sh_quote(group),
                sh_quote(path)
            ))
            .await?;
        }
        Ok(())
    }

    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
        let props = self.stat(path).await?;
        if props.is_dir {
            return Err(AppError::Config(
                "Directories cannot be opened as text".to_string(),
            ));
        }
        if props.size > max_bytes {
            return Err(AppError::Config(format!(
                "File is too large to open as text ({} bytes > {} bytes)",
                props.size, max_bytes
            )));
        }
        let cmd = format!(
            "dd bs=1 count={} if={} 2>/dev/null",
            max_bytes,
            sh_quote(path)
        );
        let result = self.exec(&cmd).await?;

        if result.exit_code != Some(0) && result.stdout.is_empty() {
            let stderr_text = String::from_utf8_lossy(&result.stderr);
            return Err(AppError::Channel(format!(
                "Failed to read file: {}",
                stderr_text.trim()
            )));
        }

        let bytes = result.stdout;
        ensure_text_bytes(&bytes, max_bytes)?;

        let file_hash = content_hash(&bytes);
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Config("Only UTF-8 text files are supported".to_string()))?;

        Ok(RemoteTextFile {
            path: path.to_string(),
            content,
            size: props.size,
            mtime: props.mtime,
            mtime_nanos: None,
            content_hash: file_hash,
        })
    }

    async fn read_file_bytes(&self, path: &str, max_bytes: u64) -> AppResult<RemoteBinaryFile> {
        let props = self.stat(path).await?;
        if props.is_dir {
            return Err(AppError::Config(
                "Directories cannot be previewed".to_string(),
            ));
        }
        if props.size > max_bytes {
            return Err(AppError::Config(format!(
                "File is too large to preview ({} bytes > {} bytes)",
                props.size, max_bytes
            )));
        }

        let cmd = format!(
            "dd bs=1 count={} if={} 2>/dev/null",
            props.size,
            sh_quote(path)
        );
        let result = self.exec(&cmd).await?;

        if result.exit_code != Some(0) && result.stdout.is_empty() {
            let stderr_text = String::from_utf8_lossy(&result.stderr);
            return Err(AppError::Channel(format!(
                "Failed to read file: {}",
                stderr_text.trim()
            )));
        }

        Ok(RemoteBinaryFile {
            path: path.to_string(),
            content_bytes: result.stdout,
            size: props.size,
            mtime: props.mtime,
            mtime_nanos: None,
        })
    }

    async fn write_file_text(
        &self,
        path: &str,
        content: &str,
        expected_mtime: Option<u64>,
        expected_size: Option<u64>,
        expected_hash: Option<&str>,
        force: bool,
    ) -> AppResult<WriteRemoteTextResult> {
        let props = self.stat(path).await?;
        if !force
            && (expected_mtime.is_some_and(|mtime| props.mtime != 0 && mtime != props.mtime)
                || expected_size.is_some_and(|size| size != props.size))
        {
            return Ok(WriteRemoteTextResult::conflict(
                props.mtime,
                props.size,
                None,
            ));
        }
        if !force {
            if let Some(expected_hash) = expected_hash {
                let cmd = format!(
                    "dd bs=1 count={} if={} 2>/dev/null",
                    props.size,
                    sh_quote(path)
                );
                let result = self.exec(&cmd).await?;
                if result.exit_code != Some(0) && result.stdout.is_empty() {
                    let stderr_text = String::from_utf8_lossy(&result.stderr);
                    return Err(AppError::Channel(format!(
                        "Failed to read file: {}",
                        stderr_text.trim()
                    )));
                }
                if content_hash(&result.stdout) != expected_hash {
                    return Ok(WriteRemoteTextResult::conflict(
                        props.mtime,
                        props.size,
                        None,
                    ));
                }
            }
        }

        let tmp = format!(
            "{}.nyaterm-edit-{}",
            path,
            uuid::Uuid::new_v4().to_string().replace('-', "")
        );
        let cmd = format!(
            "umask 077; cat > {} && {}",
            sh_quote(&tmp),
            scp_finalize_replace_command(&tmp, path, Some(&props))
        );
        let result = self.exec_with_stdin(&cmd, content.as_bytes()).await?;
        if result.exit_code != Some(0) {
            let stderr_text = String::from_utf8_lossy(&result.stderr);
            let _ = self.exec(&format!("rm -f -- {}", sh_quote(&tmp))).await;
            return Err(AppError::Channel(format!(
                "Failed to write remote file (exit {}): {}",
                result.exit_code.unwrap_or(255),
                stderr_text.trim()
            )));
        }
        let props = self.stat(path).await?;
        Ok(WriteRemoteTextResult::saved(
            props.mtime,
            props.size,
            None,
            content_hash(content.as_bytes()),
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
                    let file_name = file_name_from_path(remote_path);
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
                            file_name,
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
            match download_file_inner(
                &self.ssh_handle,
                app,
                remote_path,
                &actual_local_path,
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
                transfer_settings,
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
        let actual_remote_path = match resolve_remote_path(
            app,
            session_manager.inner(),
            &self.ssh_handle,
            session_id,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            Some(path) => path,
            None => return Ok(()),
        };

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
            match upload_file_inner(
                &self.ssh_handle,
                app,
                local_path,
                &actual_remote_path,
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
                transfer_settings,
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
        let total_files = self.count_remote_files(remote_path).await?;
        let directory_controller = create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name_from_path(remote_path),
            remote_path,
            local_path,
            "download",
            total_files,
            0,
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
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let session_manager = app.state::<Arc<crate::core::SessionManager>>();
        if !ensure_remote_upload_target_allowed(
            app,
            session_manager.inner(),
            &self.ssh_handle,
            session_id,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            return Ok(());
        }

        let local_stats = collect_local_directory_stats(local_path).await?;
        let directory_controller = create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name_from_path(local_path),
            remote_path,
            local_path,
            "upload",
            local_stats.file_count,
            local_stats.total_size,
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
                directory_controller
                    .update_progress(local_stats.total_size, local_stats.total_size);
                directory_controller.update_item_progress(completed_count, local_stats.file_count);
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
                    let _ = exec_command(
                        &self.ssh_handle,
                        &format!("rm -rf -- {}", sh_quote(remote_path)),
                    )
                    .await;
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

    async fn copy_remote_file_to_local_with_controller(
        &self,
        app: &tauri::AppHandle,
        _session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<u64> {
        download_file_inner(
            &self.ssh_handle,
            app,
            remote_path,
            local_path,
            controller,
            parent_controller,
            transfer_settings,
        )
        .await?;
        tokio::fs::metadata(local_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| AppError::Channel(format!("Failed to read copied file size: {error}")))
    }

    async fn copy_local_file_to_remote_with_controller(
        &self,
        app: &tauri::AppHandle,
        _session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<u64> {
        upload_file_inner(
            &self.ssh_handle,
            app,
            local_path,
            remote_path,
            controller,
            parent_controller,
            transfer_settings,
        )
        .await?;
        tokio::fs::metadata(local_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| AppError::Channel(format!("Failed to read copied file size: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{list_dir_command, parse_ls_line, parse_ls_line_to_properties, stat_command};

    #[test]
    fn ls_parser_handles_numeric_owner_and_group() {
        let entry = parse_ls_line("-rw-r--r-- 1 1000 100513 203 Aug 3 16:37 .zshrc").unwrap();
        assert_eq!(entry.name, ".zshrc");
        assert_eq!(entry.owner, "1000");
        assert_eq!(entry.group, "100513");
        assert_eq!(entry.size, 203);
        assert!(!entry.is_dir);
    }

    #[test]
    fn ls_parser_keeps_hidden_directory_name() {
        let entry = parse_ls_line("drwx------ 4 1000 100513 4096 Sep 1 09:31 .copilot").unwrap();
        assert_eq!(entry.name, ".copilot");
        assert!(entry.is_dir);
    }

    #[test]
    fn ls_parser_keeps_spaces_in_file_name() {
        let entry =
            parse_ls_line("-rw-r--r-- 1 1000 100513 123 Sep 1 09:31 hello world.txt").unwrap();
        assert_eq!(entry.name, "hello world.txt");
        assert!(!entry.is_dir);
    }

    #[test]
    fn ls_parser_keeps_spaces_in_directory_name() {
        let entry = parse_ls_line("drwxr-xr-x 2 1000 100513 4096 Sep 1 09:31 My Folder").unwrap();
        assert_eq!(entry.name, "My Folder");
        assert!(entry.is_dir);
    }

    #[test]
    fn ls_parser_strips_symlink_target_from_name() {
        let entry =
            parse_ls_line("lrwxrwxrwx 1 1000 100513 11 Aug 29 12:00 current -> releases/v2")
                .unwrap();
        assert_eq!(entry.name, "current");
        assert!(entry.is_symlink);
    }

    #[test]
    fn ls_commands_request_numeric_owner_and_group() {
        assert_eq!(
            list_dir_command("/home/user/My Folder"),
            "LC_ALL=C ls -la -n -- '/home/user/My Folder'"
        );
        assert_eq!(
            stat_command("/home/user/My Folder"),
            "LC_ALL=C ls -lad -n -- '/home/user/My Folder'"
        );
    }

    #[test]
    fn ls_properties_parser_keeps_relative_symlink_target() {
        let props = parse_ls_line_to_properties(
            "lrwxrwxrwx 1 root root 11 Aug 29 12:00 current -> releases/v2",
            "/opt/app/current",
        )
        .unwrap();
        assert_eq!(props.name, "current");
        assert!(props.is_symlink);
        assert_eq!(props.symlink_target.as_deref(), Some("releases/v2"));
    }

    #[test]
    fn ls_properties_parser_keeps_dangling_and_spaced_target() {
        let props = parse_ls_line_to_properties(
            "lrwxrwxrwx 1 root root 19 Aug 29 12:00 current -> missing release  ",
            "/opt/app/current",
        )
        .unwrap();
        assert_eq!(props.symlink_target.as_deref(), Some("missing release  "));
    }

    #[test]
    fn ls_properties_parser_sets_no_target_for_regular_file() {
        let props = parse_ls_line_to_properties(
            "-rw-r--r-- 1 root root 12 Aug 29 12:00 config.toml",
            "/opt/app/config.toml",
        )
        .unwrap();
        assert!(!props.is_symlink);
        assert_eq!(props.symlink_target, None);
    }
}
