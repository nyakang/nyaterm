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
use crate::observability::{log_event, StructuredLog, StructuredLogLevel};
use russh::ChannelMsg;
use std::sync::Arc;
use tauri::Emitter;

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
                directory_controller.update_item_progress(*completed_count, *completed_count);
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
                directory_controller.update_item_progress(*completed_count, *completed_count);
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
    let handle_mtx = ssh_handle.target_handle();
    let mut channel = {
        let handle = handle_mtx.lock().await;
        handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?
    };

    channel.exec(true, command.as_bytes()).await?;

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

fn parse_ls_line(line: &str) -> Option<FileEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("total ") {
        return None;
    }

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 9 {
        return None;
    }

    let perms = parts[0];
    if perms.len() < 10 {
        return None;
    }

    let is_dir = perms.starts_with('d');
    let is_symlink = perms.starts_with('l');

    let owner = parts[2].to_string();
    let group = parts[3].to_string();
    let size: u64 = parts[4].parse().unwrap_or(0);

    // parts[5..8] are month/day/time-or-year; everything from index 8 onward is the name
    let raw_name = parts[8..].join(" ");
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
        raw_name
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
    })
}

fn parse_ls_line_to_properties(line: &str, path: &str) -> AppResult<FileProperties> {
    let line = line.trim();
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 9 {
        return Err(AppError::Channel(format!(
            "Failed to parse stat output for '{}'",
            path
        )));
    }

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

    let raw_name = parts[8..].join(" ");
    let name = if is_symlink {
        if let Some(pos) = raw_name.find(" -> ") {
            raw_name[..pos].to_string()
        } else {
            raw_name.to_string()
        }
    } else {
        raw_name
    };

    Ok(FileProperties {
        name,
        is_dir,
        is_symlink,
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
            &format!("mv -f -- {} {}", sh_quote(&tmp_path), sh_quote(remote_path)),
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

async fn resolve_remote_path(
    ssh_handle: &Arc<SshConnectionHandles>,
    remote_path: &str,
    strategy: &str,
) -> Option<String> {
    let exists = resolve_remote_exists(ssh_handle, remote_path).await;
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
                if !resolve_remote_exists(ssh_handle, &candidate).await {
                    return Some(candidate);
                }
            }
            Some(remote_path.to_string())
        }
        _ => Some(remote_path.to_string()),
    }
}

#[async_trait::async_trait]
impl RemoteFs for ScpNormalBackend {
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
        let output = self
            .exec_ok(&format!("LC_ALL=C ls -la -- {}", sh_quote(path)))
            .await?;
        let mut entries = Vec::new();
        for line in output.lines() {
            if let Some(entry) = parse_ls_line(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<FileProperties> {
        let output = self
            .exec_ok(&format!("LC_ALL=C ls -lad -- {}", sh_quote(path)))
            .await?;
        let line = output
            .lines()
            .find(|l| !l.trim().is_empty() && !l.starts_with("total "))
            .ok_or_else(|| AppError::Channel(format!("No stat output for '{}'", path)))?;
        parse_ls_line_to_properties(line, path)
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

    async fn chmod(&self, path: &str, mode: &str) -> AppResult<()> {
        let _ = parse_octal_mode(mode)?;
        self.exec_ok(&format!("chmod {} -- {}", sh_quote(mode), sh_quote(path)))
            .await?;
        Ok(())
    }

    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
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
        let size = bytes.len() as u64;

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
                    let file_name = file_name_from_path(remote_path);
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
                            file_name,
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
            match download_file_inner(
                &self.ssh_handle,
                app,
                remote_path,
                &actual_local_path,
                create_child_file_transfer_controller(
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
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let actual_remote_path = match resolve_remote_path(
            &self.ssh_handle,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            Some(path) => path,
            None => {
                let file_name = file_name_from_path(remote_path);
                let transfer_id = uuid::Uuid::new_v4().to_string();
                let _ = app.emit(
                    "transfer-event",
                    &TransferEvent {
                        id: transfer_id,
                        session_id: session_id.to_string(),
                        file_name,
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
}
