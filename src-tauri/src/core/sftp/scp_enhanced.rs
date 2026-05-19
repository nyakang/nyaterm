use super::traits::RemoteFs;
use super::transfer::*;
use super::util::*;
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use crate::observability::{log_event, StructuredLog, StructuredLogLevel};
use russh::ChannelMsg;
use std::sync::Arc;
use tauri::Emitter;

pub(crate) struct ScpEnhancedBackend {
    ssh_handle: Arc<SshConnectionHandles>,
}

struct ExecResult {
    exit_code: u32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ScpEnhancedBackend {
    pub(crate) fn new(ssh_handle: Arc<SshConnectionHandles>) -> Self {
        Self { ssh_handle }
    }

    pub(crate) async fn probe(ssh_handle: &Arc<SshConnectionHandles>) -> AppResult<()> {
        let cmd = "command -v sh && command -v find && command -v stat && command -v tar && command -v cat && command -v mkdir && command -v rm && command -v mv";
        let result = exec_command_on(ssh_handle, cmd).await?;
        if result.exit_code != 0 {
            return Err(AppError::Channel(
                "SCP Enhanced: required commands not available on remote host".to_string(),
            ));
        }
        Ok(())
    }

    async fn exec(&self, command: &str) -> AppResult<ExecResult> {
        exec_command_on(&self.ssh_handle, command).await
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

    async fn count_remote_files(&self, remote_path: &str) -> AppResult<u64> {
        let cmd = format!("find {} -type f | wc -l", sh_quote(remote_path));
        let output = self.exec_ok(&cmd).await?;
        let text = String::from_utf8_lossy(&output);
        let count = text.trim().parse::<u64>().unwrap_or(0);
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
                self.download_file_inner(
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
                self.upload_file_inner(
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
                directory_controller.update_item_progress(*completed_count, *completed_count);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("progress", 0, None),
                );
            }
        }

        Ok(())
    }

    async fn download_file_inner(
        &self,
        app: &tauri::AppHandle,
        _session_id: &str,
        remote_path: &str,
        local_path: &str,
        _transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
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
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        AppError::Channel(format!("Failed to create local dir: {}", e))
                    })?;
                }
            }

            let stat_result = self
                .exec(&format!(
                    "LC_ALL=C stat -c '%s' -- {}",
                    sh_quote(remote_path)
                ))
                .await?;
            let total_size = if stat_result.exit_code == 0 {
                String::from_utf8_lossy(&stat_result.stdout)
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(0)
            } else {
                0
            };
            controller.update_progress(0, total_size);

            let mut local_file = tokio::fs::File::create(local_path)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to create local file: {}", e)))?;

            let cmd = format!("cat -- {}", sh_quote(remote_path));
            let handle_mtx = self.ssh_handle.target_handle();
            let mut channel = {
                let handle = handle_mtx.lock().await;
                handle
                    .channel_open_session()
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?
            };
            channel.exec(true, cmd.as_bytes()).await?;

            const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
            let mut last_progress = Instant::now();
            let mut bytes_transferred: u64 = 0;
            let mut stderr_buf = Vec::new();
            let mut exit_code: Option<u32> = None;

            loop {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;

                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => {
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
                    }
                    Some(ChannelMsg::ExtendedData { data, ext }) => {
                        if ext == 1 {
                            stderr_buf.extend_from_slice(&data);
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        exit_code = Some(exit_status);
                    }
                    Some(ChannelMsg::Eof) | None => {
                        if exit_code.is_none() {
                            if let Some(ChannelMsg::ExitStatus { exit_status }) =
                                channel.wait().await
                            {
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

            if exit_code.unwrap_or(1) != 0 {
                let msg = String::from_utf8_lossy(&stderr_buf);
                return Err(AppError::Channel(format!(
                    "Remote cat failed (exit {}): {}",
                    exit_code.unwrap_or(1),
                    msg.trim()
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
        &self,
        app: &tauri::AppHandle,
        _session_id: &str,
        local_path: &str,
        remote_path: &str,
        _transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<()> {
        use std::time::{Duration, Instant};
        use tokio::io::AsyncReadExt;

        register_transfer(controller.clone());
        let _ = app.emit(
            "transfer-event",
            &controller.build_event("started", 0, None),
        );

        let tmp_suffix = uuid::Uuid::new_v4().to_string().replace('-', "");
        let tmp_path = format!("{}.uploading.{}", remote_path, &tmp_suffix[..8]);

        let result: AppResult<u64> = async {
            let local_meta = tokio::fs::metadata(local_path).await.map_err(|e| {
                AppError::Channel(format!("Failed to read local file metadata: {}", e))
            })?;
            let total_size = local_meta.len();
            controller.update_progress(0, total_size);

            let cmd = format!("cat > {}", sh_quote(&tmp_path));
            let handle_mtx = self.ssh_handle.target_handle();
            let mut channel = {
                let handle = handle_mtx.lock().await;
                handle
                    .channel_open_session()
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?
            };
            channel.exec(true, cmd.as_bytes()).await?;

            let mut local_file = tokio::fs::File::open(local_path)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open local file: {}", e)))?;

            const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
            let mut last_progress = Instant::now();
            let mut bytes_transferred: u64 = 0;
            let mut buf = vec![0u8; 32768];

            loop {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;

                let n = local_file
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Channel(format!("Local read failed: {}", e)))?;
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
            let mut stderr_buf = Vec::new();
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::ExtendedData { data, ext }) => {
                        if ext == 1 {
                            stderr_buf.extend_from_slice(&data);
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        exit_code = Some(exit_status);
                    }
                    Some(ChannelMsg::Eof) | None => {
                        if exit_code.is_none() {
                            if let Some(ChannelMsg::ExitStatus { exit_status }) =
                                channel.wait().await
                            {
                                exit_code = Some(exit_status);
                            }
                        }
                        break;
                    }
                    _ => {}
                }
            }

            if exit_code.unwrap_or(1) != 0 {
                let _ = self
                    .exec(&format!("rm -f -- {}", sh_quote(&tmp_path)))
                    .await;
                let msg = String::from_utf8_lossy(&stderr_buf);
                return Err(AppError::Channel(format!(
                    "Remote write failed (exit {}): {}",
                    exit_code.unwrap_or(1),
                    msg.trim()
                )));
            }

            let mv_cmd = format!("mv -f -- {} {}", sh_quote(&tmp_path), sh_quote(remote_path));
            let mv_result = self.exec(&mv_cmd).await?;
            if mv_result.exit_code != 0 {
                let _ = self
                    .exec(&format!("rm -f -- {}", sh_quote(&tmp_path)))
                    .await;
                let msg = String::from_utf8_lossy(&mv_result.stderr);
                return Err(AppError::Channel(format!(
                    "Failed to finalize upload (exit {}): {}",
                    mv_result.exit_code,
                    msg.trim()
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
                    let _ = self
                        .exec(&format!("rm -f -- {}", sh_quote(&tmp_path)))
                        .await;
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
}

async fn exec_command_on(
    ssh_handle: &Arc<SshConnectionHandles>,
    command: &str,
) -> AppResult<ExecResult> {
    let handle_mtx = ssh_handle.target_handle();
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

#[async_trait::async_trait]
impl RemoteFs for ScpEnhancedBackend {
    fn backend_name(&self) -> &'static str {
        "scp-enhanced"
    }

    async fn home_dir(&self) -> AppResult<String> {
        let output = self.exec_ok("echo ~").await?;
        let home = String::from_utf8_lossy(&output).trim().to_string();
        if home.is_empty() {
            return Err(AppError::Config(
                "Failed to determine home directory".to_string(),
            ));
        }
        Ok(home)
    }

    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let cmd = format!(
            "LC_ALL=C find {} -mindepth 1 -maxdepth 1 -printf '%f\\0%y\\0%s\\0%T@\\0%M\\0%u\\0%g\\0%p\\0'",
            sh_quote(path)
        );
        let result = self.exec(&cmd).await?;
        if result.exit_code != 0 && result.stdout.is_empty() {
            let msg = String::from_utf8_lossy(&result.stderr);
            return Err(AppError::Channel(format!(
                "Failed to list directory: {}",
                msg.trim()
            )));
        }

        let raw = String::from_utf8_lossy(&result.stdout);
        let fields: Vec<&str> = raw.split('\0').collect();

        let mut entries = Vec::new();
        let mut i = 0;
        while i + 7 < fields.len() {
            let name = fields[i].to_string();
            let type_char = fields[i + 1];
            let size = fields[i + 2].parse::<u64>().unwrap_or(0);
            let mtime = fields[i + 3]
                .split('.')
                .next()
                .unwrap_or("0")
                .parse::<u64>()
                .unwrap_or(0);
            let permissions = fields[i + 4].to_string();
            let owner = fields[i + 5].to_string();
            let group = fields[i + 6].to_string();
            // fields[i + 7] is the full path, unused

            let is_dir = type_char == "d";
            let is_symlink = type_char == "l";

            if !name.is_empty() {
                entries.push(FileEntry {
                    name,
                    is_dir,
                    is_symlink,
                    size,
                    permissions,
                    owner,
                    group,
                    mtime,
                });
            }

            i += 8;
        }

        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<FileProperties> {
        let cmd = format!(
            "LC_ALL=C stat -c '%n\\0%F\\0%s\\0%Y\\0%X\\0%a\\0%U\\0%G\\0%u\\0%g' -- {}",
            sh_quote(path)
        );
        let output = self.exec_ok(&cmd).await?;
        let raw = String::from_utf8_lossy(&output);
        let fields: Vec<&str> = raw.trim().split('\0').collect();

        if fields.len() < 10 {
            return Err(AppError::Channel(format!(
                "Unexpected stat output: expected 10 fields, got {}",
                fields.len()
            )));
        }

        let name = fields[0].split('/').last().unwrap_or(fields[0]).to_string();
        let file_type = fields[1].to_lowercase();
        let size = fields[2].parse::<u64>().unwrap_or(0);
        let mtime = fields[3].parse::<u64>().unwrap_or(0);
        let atime = fields[4].parse::<u64>().unwrap_or(0);
        let mode_str = fields[5];
        let owner = fields[6].to_string();
        let group = fields[7].to_string();
        let uid = fields[8].to_string();
        let gid = fields[9].to_string();

        let is_dir = file_type.contains("directory");
        let is_symlink = file_type.contains("symbolic link") || file_type.contains("symlink");
        let type_char = if is_dir {
            'd'
        } else if is_symlink {
            'l'
        } else {
            '-'
        };

        let mode_u32 = u32::from_str_radix(mode_str, 8).unwrap_or(0);
        let permissions = permissions_to_string(mode_u32, type_char);

        Ok(FileProperties {
            name,
            is_dir,
            is_symlink,
            size,
            permissions,
            owner,
            group,
            uid,
            gid,
            mtime,
            atime,
        })
    }

    async fn mkdir(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        self.exec_ok(&format!("mkdir -p -- {}", sh_quote(path)))
            .await?;
        if let Some(ref m) = mode {
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
        self.exec_ok(&format!("chmod {} -- {}", sh_quote(mode), sh_quote(path)))
            .await?;
        Ok(())
    }

    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
        let cmd = format!("head -c {} -- {}", max_bytes, sh_quote(path));
        let output = self.exec_ok(&cmd).await?;

        if output.len() as u64 > max_bytes {
            return Err(AppError::Config(format!(
                "File is too large for AI analysis ({} bytes > {} bytes)",
                output.len(),
                max_bytes
            )));
        }
        if output.contains(&0) {
            return Err(AppError::Config(
                "Binary files are not supported for AI analysis".to_string(),
            ));
        }
        let content = String::from_utf8(output.clone()).map_err(|_| {
            AppError::Config("Only UTF-8 text files are supported for AI analysis".to_string())
        })?;
        let size = content.len() as u64;

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
            match self
                .download_file_inner(
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
            match self
                .upload_file_inner(
                    app,
                    session_id,
                    local_path,
                    remote_path,
                    transfer_settings,
                    create_child_file_transfer_controller(
                        session_id,
                        file_name_from_path(remote_path),
                        remote_path,
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
