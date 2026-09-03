//! Internal pieces of the SFTP backend moved out of `sftp_backend.rs`.

use super::*;

#[allow(dead_code)]
pub(super) fn remote_same_endpoint_copy_command(
    source_path: &str,
    target_path: &str,
    is_directory: bool,
) -> String {
    let preflight = "case \"$(uname -s 2>/dev/null)\" in CYGWIN*|MINGW*|MSYS*|Windows*) exit 97;; esac; command -v cp >/dev/null 2>&1 || exit 98;";
    if is_directory {
        format!(
            "{} if [ -d {} ]; then cp -a -- {}/. {}/; else cp -a -- {} {}; fi",
            preflight,
            sh_quote(target_path),
            sh_quote(source_path),
            sh_quote(target_path),
            sh_quote(source_path),
            sh_quote(target_path),
        )
    } else {
        format!(
            "{} cp -a -- {} {}",
            preflight,
            sh_quote(source_path),
            sh_quote(target_path),
        )
    }
}

pub(super) async fn resolve_remote_path(
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
            match super::super::duplicate::prompt_duplicate_choice(
                app,
                session_manager,
                session_id,
                remote_path,
                file_name,
                is_directory,
            )
            .await
            {
                Ok(super::super::duplicate::DuplicateChoice::Skip) => None,
                Ok(super::super::duplicate::DuplicateChoice::Overwrite) => {
                    Some(remote_path.to_string())
                }
                Err(_) => None,
            }
        }
        _ => Some(remote_path.to_string()),
    }
}

pub(super) async fn ensure_remote_upload_target_allowed(
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
                super::super::duplicate::prompt_duplicate_choice(
                    app,
                    session_manager,
                    session_id,
                    remote_path,
                    &file_name,
                    is_directory,
                )
                .await,
                Ok(super::super::duplicate::DuplicateChoice::Overwrite)
            )
        }
        _ => true,
    }
}

pub(super) fn copy_remote_sidecar_path(target_path: &str, suffix: &str) -> String {
    let name = target_path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .filter(|part| !part.is_empty())
        .unwrap_or("nyaterm-copy");
    let sidecar = format!(".{name}.nyaterm-{suffix}-{}", uuid::Uuid::new_v4());
    match target_path.rsplit_once('/') {
        Some(("", _)) => format!("/{sidecar}"),
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/{sidecar}"),
        _ => sidecar,
    }
}

pub(super) fn copy_local_sidecar_path(target: &Path, suffix: &str) -> PathBuf {
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

pub(super) async fn cleanup_local_copy_temp(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

pub(super) async fn commit_local_copy_temp(temp_path: &Path, target_path: &Path) -> AppResult<()> {
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
        .map(|_| copy_local_sidecar_path(target_path, "backup"));
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

pub(super) fn copy_transfer_settings(app: &tauri::AppHandle) -> crate::config::TransferSettings {
    crate::config::load_app_settings(app)
        .map(|settings| settings.transfer)
        .unwrap_or_default()
}

pub(super) async fn ensure_remote_dir_exists(sftp: &SftpSession, path: &str) -> AppResult<()> {
    let mut current = String::new();
    for part in normalize_remote_dir_path(path)
        .split('/')
        .filter(|part| !part.is_empty())
    {
        current.push('/');
        current.push_str(part);
        if sftp.metadata(&current).await.is_ok() {
            continue;
        }
        sftp.create_dir(&current)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to create remote dir: {error}")))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) async fn copy_local_file_with_controller(
    app: &tauri::AppHandle,
    source_session_id: &str,
    source_path: &str,
    target_path: &str,
    controller: Arc<TransferController>,
) -> AppResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let result: AppResult<u64> = async {
        let metadata = tokio::fs::metadata(source_path).await.map_err(|error| {
            AppError::Channel(format!("Failed to read source file metadata: {error}"))
        })?;
        let total_size = metadata.len();
        controller.update_progress(0, total_size);

        if let Some(parent) = std::path::Path::new(target_path).parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                AppError::Channel(format!("Failed to create target directory: {error}"))
            })?;
        }

        let mut source = tokio::fs::File::open(source_path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open source file: {error}")))?;
        let mut target = tokio::fs::File::create(target_path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to create target file: {error}")))?;
        let mut buffer = vec![0_u8; 512 * 1024];
        let mut bytes_written = 0_u64;
        let mut last_progress = Instant::now();

        loop {
            wait_for_transfer_ready(&controller).await?;
            let read = source.read(&mut buffer).await.map_err(|error| {
                AppError::Channel(format!("Failed to read source file: {error}"))
            })?;
            if read == 0 {
                break;
            }
            wait_for_transfer_ready(&controller).await?;
            target.write_all(&buffer[..read]).await.map_err(|error| {
                AppError::Channel(format!("Failed to write target file: {error}"))
            })?;
            bytes_written = bytes_written.saturating_add(read as u64);
            controller.update_progress(bytes_written, total_size);
            if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                last_progress = Instant::now();
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("progress", total_size, None),
                );
            }
        }
        target
            .flush()
            .await
            .map_err(|error| AppError::Channel(format!("Failed to flush target file: {error}")))?;

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
            if matches!(error, AppError::Cancelled(_)) {
                let _ = tokio::fs::remove_file(target_path).await;
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
            tracing::debug!(
                target: "user_action",
                action = "copy",
                entity = "local_file",
                session_id = %source_session_id,
                source_path = source_path,
                target_path = target_path,
                "Local file copy ended with error"
            );
            unregister_transfer(&controller.id());
            Err(error)
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RemoteCopyFile {
    source_path: String,
    target_path: String,
    size: u64,
}

impl SftpBackend {
    pub(super) async fn cleanup_remote_copy_temp(&self, sftp: &SftpSession, path: &str) {
        let _ = sftp
            .remove_file_bytes(self.encode_path_for_sftp(path))
            .await;
    }

    pub(super) async fn create_remote_copy_temp_file(
        &self,
        sftp: &SftpSession,
        temp_path: &str,
    ) -> AppResult<russh_sftp::client::fs::File> {
        if self.encoding() != "UTF-8" {
            use russh_sftp::protocol::OpenFlags;
            sftp.open_with_flags_bytes(
                self.encode_path_for_sftp(temp_path),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            )
            .await
            .map_err(|error| {
                AppError::Channel(format!("Failed to create temporary remote file: {error}"))
            })
        } else {
            sftp.create(temp_path).await.map_err(|error| {
                AppError::Channel(format!("Failed to create temporary remote file: {error}"))
            })
        }
    }

    pub(super) async fn commit_remote_copy_temp(
        &self,
        sftp: &SftpSession,
        temp_path: &str,
        target_path: &str,
    ) -> AppResult<()> {
        let target_bytes = self.encode_path_for_sftp(target_path);
        let target_meta = sftp.metadata_bytes(target_bytes.clone()).await.ok();
        if target_meta.as_ref().is_some_and(sftp_attrs_is_dir) {
            self.cleanup_remote_copy_temp(sftp, temp_path).await;
            return Err(AppError::Channel(format!(
                "Cannot overwrite existing remote directory '{target_path}' with a file"
            )));
        }

        let backup_path = target_meta
            .as_ref()
            .map(|_| copy_remote_sidecar_path(target_path, "backup"));
        if let Some(backup) = backup_path.as_ref() {
            sftp.rename_bytes(target_bytes.clone(), self.encode_path_for_sftp(backup))
                .await
                .map_err(|error| {
                    AppError::Channel(format!("Failed to protect existing remote target: {error}"))
                })?;
        }

        let commit_result = sftp
            .rename_bytes(self.encode_path_for_sftp(temp_path), target_bytes.clone())
            .await
            .map_err(|error| AppError::Channel(format!("Failed to commit remote copy: {error}")));

        match commit_result {
            Ok(()) => {
                if let Some(backup) = backup_path {
                    let _ = sftp
                        .remove_file_bytes(self.encode_path_for_sftp(&backup))
                        .await;
                }
                Ok(())
            }
            Err(error) => {
                self.cleanup_remote_copy_temp(sftp, temp_path).await;
                if let Some(backup) = backup_path {
                    let _ = sftp
                        .rename_bytes(self.encode_path_for_sftp(&backup), target_bytes)
                        .await;
                    let _ = sftp
                        .remove_file_bytes(self.encode_path_for_sftp(&backup))
                        .await;
                }
                Err(error)
            }
        }
    }
}

impl SftpBackend {
    #[allow(dead_code)]
    pub(crate) async fn resolve_remote_copy_target_info(
        &self,
        app: &tauri::AppHandle,
        session_manager: &crate::core::SessionManager,
        session_id: &str,
        target_path: &str,
        strategy: &str,
    ) -> Option<CopyResolvedTarget> {
        let sftp = self.open_sftp().await.ok()?;
        let original_existed = sftp
            .metadata_bytes(self.encode_path_for_sftp(target_path))
            .await
            .is_ok();
        let resolved = resolve_remote_path(
            app,
            session_manager,
            &sftp,
            session_id,
            target_path,
            strategy,
        )
        .await;
        let _ = sftp.close().await;
        resolved.map(|path| CopyResolvedTarget {
            existed: if path == target_path {
                original_existed
            } else {
                false
            },
            path,
        })
    }

    #[allow(dead_code)]
    pub(crate) async fn copy_local_file_to_remote(
        &self,
        app: &tauri::AppHandle,
        target_session_id: &str,
        source_path: &str,
        target_path: &str,
        _target_existed: bool,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let settings = copy_transfer_settings(app);
        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(&settings, self.pipeline_depth_override);
        let chunk_size = sftp_payload_size(request_kib);
        let started = Instant::now();
        let controller = create_child_file_transfer_controller(
            transfer_id,
            target_session_id,
            file_name_from_path(source_path),
            target_path,
            source_path,
            "copy",
            None,
        );
        register_transfer(controller.clone());
        let _ = app.emit(
            "transfer-event",
            &controller.build_event("started", 0, None),
        );

        let temp_path = copy_remote_sidecar_path(target_path, "tmp");
        let result: AppResult<u64> = async {
            let local_meta = tokio::fs::metadata(source_path).await;
            let total_size = local_meta.as_ref().map(|meta| meta.len()).unwrap_or(0);
            controller.update_progress(0, total_size);

            let sftp = self
                .open_sftp_with_client_config(sftp_client_config(
                    request_kib,
                    max_concurrent_writes,
                ))
                .await?;
            if let Some(parent) = target_path.rsplit_once('/').map(|(parent, _)| parent) {
                if !parent.is_empty() {
                    ensure_remote_dir_exists(&sftp, parent).await?;
                }
            }

            let mut local_file = tokio::fs::File::open(source_path).await.map_err(|error| {
                AppError::Channel(format!("Failed to open local file: {error}"))
            })?;
            let mut remote_file = self.create_remote_copy_temp_file(&sftp, &temp_path).await?;

            let mut bytes_written = 0_u64;
            let mut last_progress = Instant::now();
            let mut buffer = vec![0_u8; chunk_size];
            loop {
                wait_for_transfer_ready(&controller).await?;
                let read = local_file.read(&mut buffer).await.map_err(|error| {
                    AppError::Channel(format!("Failed to read local file: {error}"))
                })?;
                if read == 0 {
                    break;
                }
                wait_for_sftp_upload_io(
                    &controller,
                    None,
                    remote_file.write_all(&buffer[..read]),
                    |error| AppError::Channel(format!("SFTP write failed: {error}")),
                )
                .await?;
                bytes_written = bytes_written.saturating_add(read as u64);
                controller.update_progress(bytes_written, total_size);
                if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                }
            }
            wait_for_sftp_upload_io(&controller, None, remote_file.shutdown(), |error| {
                AppError::Channel(format!("SFTP flush failed for temporary file: {error}"))
            })
            .await?;
            self.commit_remote_copy_temp(&sftp, &temp_path, target_path)
                .await?;
            if settings.preserve_timestamps {
                if let Ok(ref meta) = local_meta {
                    if let Ok(mtime) = meta.modified() {
                        if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                            if let Ok(mut attrs) = sftp.metadata(target_path).await {
                                attrs.mtime = Some(dur.as_secs() as u32);
                                attrs.atime = meta
                                    .accessed()
                                    .ok()
                                    .and_then(|atime| {
                                        atime.duration_since(std::time::UNIX_EPOCH).ok()
                                    })
                                    .map(|duration| duration.as_secs() as u32);
                                let _ = sftp.set_metadata(target_path, attrs).await;
                            }
                        }
                    }
                }
            }
            let _ = sftp.close().await;
            Ok(bytes_written)
        }
        .await;

        match result {
            Ok(size) => {
                log_transfer_performance(
                    "copy",
                    "file",
                    size,
                    started.elapsed(),
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
            Err(error) => {
                if let Ok(sftp) = self.open_sftp().await {
                    self.cleanup_remote_copy_temp(&sftp, &temp_path).await;
                    let _ = sftp.close().await;
                }
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
                unregister_transfer(&controller.id());
                Err(error)
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn copy_remote_file_to_local(
        &self,
        app: &tauri::AppHandle,
        source_session_id: &str,
        source_path: &str,
        target_path: &str,
        _target_existed: bool,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let settings = copy_transfer_settings(app);
        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(&settings, self.pipeline_depth_override);
        let controller = create_child_file_transfer_controller(
            transfer_id,
            source_session_id,
            file_name_from_path(source_path),
            source_path,
            target_path,
            "copy",
            None,
        );
        register_transfer(controller.clone());
        let _ = app.emit(
            "transfer-event",
            &controller.build_event("started", 0, None),
        );

        let target = PathBuf::from(target_path);
        let temp = copy_local_sidecar_path(&target, "tmp");
        let result: AppResult<u64> = async {
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    AppError::Channel(format!("Failed to create target directory: {error}"))
                })?;
            }

            let sftp = self
                .open_sftp_with_client_config(sftp_client_config(
                    request_kib,
                    max_concurrent_writes,
                ))
                .await?;
            let remote_size = sftp
                .metadata(source_path)
                .await
                .ok()
                .and_then(|attrs| attrs.size);
            let total_size = remote_size.unwrap_or(0);
            controller.update_progress(0, total_size);
            let mut temp_file = tokio::fs::File::create(&temp).await.map_err(|error| {
                AppError::Channel(format!("Failed to create temporary target file: {error}"))
            })?;
            let temp_path = temp.to_string_lossy().to_string();

            let bytes_written = if let Some(total_size) = remote_size {
                if total_size > 0 {
                    let _ = temp_file.set_len(total_size).await;
                }
                download_known_size_to_local_file(
                    &sftp,
                    source_path,
                    &temp_path,
                    &mut temp_file,
                    total_size,
                    request_kib,
                    pipeline_depth,
                    pipeline_depth,
                    &controller,
                    None,
                    &self.path_cache,
                    |current, _delta| controller.update_progress(current, total_size),
                    |current| {
                        controller.update_progress(current, total_size);
                        let _ = app.emit(
                            "transfer-event",
                            &controller.build_event("progress", total_size, None),
                        );
                    },
                )
                .await?
            } else {
                let mut source_file = sftp.open(source_path).await.map_err(|error| {
                    AppError::Channel(format!("Source connection read open failed: {error}"))
                })?;
                let mut buffer = vec![0_u8; sftp_payload_size(request_kib)];
                let mut bytes_written = 0_u64;
                let mut last_progress = Instant::now();
                loop {
                    wait_for_transfer_ready(&controller).await?;
                    let read = source_file.read(&mut buffer).await.map_err(|error| {
                        AppError::Channel(format!(
                            "Source connection disconnected or read failed for {source_path}: {error}"
                        ))
                    })?;
                    if read == 0 {
                        break;
                    }
                    temp_file
                        .write_all(&buffer[..read])
                        .await
                        .map_err(|error| {
                            AppError::Channel(format!(
                                "Failed to write temporary target file: {error}"
                            ))
                        })?;
                    bytes_written = bytes_written.saturating_add(read as u64);
                    controller.update_progress(bytes_written, 0);
                    if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                        last_progress = Instant::now();
                        let _ = app.emit(
                            "transfer-event",
                            &controller.build_event("progress", 0, None),
                        );
                    }
                }
                close_download_remote_file(source_file, source_path, &temp_path, None).await?;
                bytes_written
            };
            temp_file.flush().await.map_err(|error| {
                AppError::Channel(format!("Failed to flush temporary target file: {error}"))
            })?;
            drop(temp_file);
            commit_local_copy_temp(&temp, &target).await?;
            let _ = sftp.close().await;
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
                unregister_transfer(&controller.id());
                Err(error)
            }
        }
    }

    pub(crate) async fn copy_remote_file_to_remote_streaming(
        &self,
        target: &SftpBackend,
        app: &tauri::AppHandle,
        source_session_id: &str,
        source_path: &str,
        target_path: &str,
        _target_existed: bool,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let controller = create_child_file_transfer_controller(
            transfer_id,
            source_session_id,
            file_name_from_path(source_path),
            source_path,
            target_path,
            "copy",
            None,
        );
        register_transfer(controller.clone());
        let _ = app.emit(
            "transfer-event",
            &controller.build_event("started", 0, None),
        );

        let temp_path = copy_remote_sidecar_path(target_path, "tmp");
        let result: AppResult<u64> = async {
            let source_sftp = self.open_sftp().await?;
            let target_sftp = target.open_sftp().await?;
            if let Some(parent) = target_path.rsplit_once('/').map(|(parent, _)| parent) {
                if !parent.is_empty() {
                    ensure_remote_dir_exists(&target_sftp, parent).await?;
                }
            }

            let total_size = source_sftp
                .metadata(source_path)
                .await
                .ok()
                .and_then(|attrs| attrs.size)
                .unwrap_or(0);
            controller.update_progress(0, total_size);

            let mut source_file = source_sftp.open(source_path).await.map_err(|error| {
                AppError::Channel(format!("Source connection read open failed: {error}"))
            })?;
            let mut target_file = target
                .create_remote_copy_temp_file(&target_sftp, &temp_path)
                .await
                .map_err(|error| {
                    AppError::Channel(format!("Target connection write open failed: {error}"))
                })?;
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
            let reader_controller = controller.clone();
            let source_path_owned = source_path.to_string();
            let reader = tokio::spawn(async move {
                let mut buffer = vec![0_u8; 512 * 1024];
                loop {
                    wait_for_transfer_ready(&reader_controller).await?;
                    let read = source_file.read(&mut buffer).await.map_err(|error| {
                        AppError::Channel(format!(
                            "Source connection disconnected or read failed for {source_path_owned}: {error}"
                        ))
                    })?;
                    if read == 0 {
                        break;
                    }
                    tx.send(buffer[..read].to_vec()).await.map_err(|_| {
                        AppError::Channel("Target writer stopped before source completed".to_string())
                    })?;
                }
                AppResult::Ok(())
            });

            let mut bytes_written = 0_u64;
            let mut last_progress = Instant::now();
            while let Some(chunk) = rx.recv().await {
                wait_for_transfer_ready(&controller).await?;
                wait_for_sftp_upload_io(&controller, None, target_file.write_all(&chunk), |error| {
                    AppError::Channel(format!(
                        "Target connection disconnected or write failed for {target_path}: {error}"
                    ))
                })
                .await?;
                bytes_written = bytes_written.saturating_add(chunk.len() as u64);
                controller.update_progress(bytes_written, total_size);
                if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                }
            }

            reader
                .await
                .map_err(|error| AppError::Channel(format!("Source reader task failed: {error}")))??;
            wait_for_sftp_upload_io(&controller, None, target_file.shutdown(), |error| {
                AppError::Channel(format!(
                    "Target connection flush failed for {target_path}: {error}"
                ))
            })
            .await?;
            target
                .commit_remote_copy_temp(&target_sftp, &temp_path, target_path)
                .await?;
            let _ = source_sftp.close().await;
            let _ = target_sftp.close().await;
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
                if let Ok(sftp) = target.open_sftp().await {
                    target.cleanup_remote_copy_temp(&sftp, &temp_path).await;
                    let _ = sftp.close().await;
                }
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
                unregister_transfer(&controller.id());
                Err(error)
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn copy_remote_same_endpoint_fast(
        &self,
        source_path: &str,
        target_path: &str,
        is_directory: bool,
    ) -> AppResult<()> {
        let command = remote_same_endpoint_copy_command(source_path, target_path, is_directory);
        self.exec_ok(&command).await.map(|_| ())
    }

    pub(super) async fn collect_remote_copy_files(
        &self,
        source_root: &str,
        target_root: &str,
    ) -> AppResult<(Vec<RemoteCopyFile>, u64)> {
        let sftp = self.open_sftp().await?;
        let mut files = Vec::new();
        let mut total_size = 0_u64;
        let mut stack = vec![(source_root.to_string(), target_root.to_string())];

        while let Some((source_dir, target_dir)) = stack.pop() {
            let entries = sftp.read_dir(&source_dir).await?;
            for entry in entries {
                let name = entry.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                let source_child = join_remote_child(&source_dir, &name);
                let target_child = join_remote_child(&target_dir, &name);
                let attrs = entry.metadata();
                if sftp_attrs_is_dir(&attrs) {
                    stack.push((source_child, target_child));
                } else {
                    let size = attrs.size.unwrap_or(0);
                    total_size = total_size.saturating_add(size);
                    files.push(RemoteCopyFile {
                        source_path: source_child,
                        target_path: target_child,
                        size,
                    });
                }
            }
        }
        let _ = sftp.close().await;
        Ok((files, total_size))
    }

    #[allow(dead_code)]
    pub(crate) async fn copy_local_directory_to_remote(
        &self,
        app: &tauri::AppHandle,
        target_session_id: &str,
        source_path: &str,
        target_path: &str,
        _target_existed: bool,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let settings = copy_transfer_settings(app);
        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(&settings, self.pipeline_depth_override);
        let chunk_size = sftp_payload_size(request_kib);
        let started = Instant::now();
        let controller = create_directory_transfer_controller(
            transfer_id,
            target_session_id,
            file_name_from_path(source_path),
            target_path,
            source_path,
            "copy",
            0,
            0,
        );
        register_transfer(controller.clone());
        let _ = app.emit(
            "transfer-event",
            &controller.build_event("started", 0, None),
        );

        let result = async {
            let inventory = self
                .collect_local_directory_inventory(source_path, target_path, &controller, &settings)
                .await?;
            let total_files = inventory.total_files;
            let total_size = inventory.total_size;
            let mut completed = 0_u64;
            let mut bytes_written = 0_u64;
            let mut last_progress = Instant::now();

            for file in inventory.files {
                wait_for_transfer_ready(&controller).await?;
                let temp_path = copy_remote_sidecar_path(&file.remote_path, "tmp");
                let sftp = self
                    .open_sftp_with_client_config(sftp_client_config(
                        request_kib,
                        max_concurrent_writes,
                    ))
                    .await?;
                if let Some(parent) = file.remote_path.rsplit_once('/').map(|(parent, _)| parent) {
                    if !parent.is_empty() {
                        ensure_remote_dir_exists(&sftp, parent).await?;
                    }
                }
                let mut source =
                    tokio::fs::File::open(&file.local_path)
                        .await
                        .map_err(|error| {
                            AppError::Channel(format!("Failed to open local source file: {error}"))
                        })?;
                let mut target_file = self.create_remote_copy_temp_file(&sftp, &temp_path).await?;
                let mut buffer = vec![0_u8; chunk_size];
                let write_result: AppResult<()> = async {
                    loop {
                        wait_for_transfer_ready(&controller).await?;
                        let read = source.read(&mut buffer).await.map_err(|error| {
                            AppError::Channel(format!("Failed to read local source file: {error}"))
                        })?;
                        if read == 0 {
                            break;
                        }
                        wait_for_sftp_upload_io(
                            &controller,
                            None,
                            target_file.write_all(&buffer[..read]),
                            |error| AppError::Channel(format!("SFTP write failed: {error}")),
                        )
                        .await?;
                        bytes_written = bytes_written.saturating_add(read as u64);
                        controller.update_progress(bytes_written, total_size);
                        if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                            last_progress = Instant::now();
                            let _ = app.emit(
                                "transfer-event",
                                &controller.build_event("progress", file.size, None),
                            );
                        }
                    }
                    wait_for_sftp_upload_io(&controller, None, target_file.shutdown(), |error| {
                        AppError::Channel(format!("SFTP flush failed for temporary file: {error}"))
                    })
                    .await?;
                    Ok(())
                }
                .await;
                drop(target_file);
                if let Err(error) = write_result {
                    self.cleanup_remote_copy_temp(&sftp, &temp_path).await;
                    let _ = sftp.close().await;
                    return Err(error);
                }
                if let Err(error) = self
                    .commit_remote_copy_temp(&sftp, &temp_path, &file.remote_path)
                    .await
                {
                    let _ = sftp.close().await;
                    return Err(error);
                }
                if settings.preserve_timestamps {
                    if let Some(mtime) = file.mtime {
                        if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                            if let Ok(mut attrs) = sftp.metadata(&file.remote_path).await {
                                attrs.mtime = Some(dur.as_secs() as u32);
                                attrs.atime = file.atime.and_then(|atime| {
                                    atime
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .ok()
                                        .map(|duration| duration.as_secs() as u32)
                                });
                                let _ = sftp.set_metadata(&file.remote_path, attrs).await;
                            }
                        }
                    }
                }
                let _ = sftp.close().await;
                completed = completed.saturating_add(1);
                controller.update_item_progress(completed, total_files);
            }

            Ok(DirectoryTransferSummary {
                completed,
                total_files,
                bytes: bytes_written,
                small_file_concurrency: 1,
            })
        }
        .await;

        match result {
            Ok(summary) => {
                log_transfer_performance(
                    "copy",
                    "directory",
                    summary.bytes,
                    started.elapsed(),
                    request_kib,
                    pipeline_depth,
                    max_concurrent_writes,
                    summary.small_file_concurrency,
                );
                controller.update_progress(summary.bytes, summary.bytes);
                controller.update_item_progress(summary.completed, summary.total_files);
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

    pub(crate) async fn copy_remote_directory_to_local(
        &self,
        app: &tauri::AppHandle,
        source_session_id: &str,
        source_path: &str,
        target_path: &str,
        _target_existed: bool,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let settings = copy_transfer_settings(app);
        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(&settings, self.pipeline_depth_override);
        let started = Instant::now();
        let controller = create_directory_transfer_controller(
            transfer_id,
            source_session_id,
            file_name_from_path(source_path),
            source_path,
            target_path,
            "copy",
            0,
            0,
        );
        register_transfer(controller.clone());
        let _ = app.emit(
            "transfer-event",
            &controller.build_event("started", 0, None),
        );

        let result = async {
            let inventory = self
                .collect_remote_directory_inventory(source_path, target_path, &controller)
                .await?;
            let total_files = inventory.total_files;
            let total_size = inventory.total_size;
            let mut completed = 0_u64;
            let mut bytes_written = 0_u64;
            let mut last_progress = Instant::now();

            for file in inventory.files {
                wait_for_transfer_ready(&controller).await?;
                let target = PathBuf::from(&file.local_path);
                let temp = copy_local_sidecar_path(&target, "tmp");
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        AppError::Channel(format!("Failed to create local target dir: {error}"))
                    })?;
                }
                let sftp = self.open_sftp().await?;
                let mut source_file = sftp.open(&file.remote_path).await.map_err(|error| {
                    AppError::Channel(format!(
                        "Source connection read open failed for {}: {error}",
                        file.remote_path
                    ))
                })?;
                let mut temp_file = tokio::fs::File::create(&temp).await.map_err(|error| {
                    AppError::Channel(format!("Failed to create temporary target file: {error}"))
                })?;
                let mut buffer = vec![0_u8; 512 * 1024];
                let write_result: AppResult<()> = async {
                    loop {
                        wait_for_transfer_ready(&controller).await?;
                        let read = source_file.read(&mut buffer).await.map_err(|error| {
                            AppError::Channel(format!(
                                "Source connection disconnected or read failed for {}: {error}",
                                file.remote_path
                            ))
                        })?;
                        if read == 0 {
                            break;
                        }
                        temp_file
                            .write_all(&buffer[..read])
                            .await
                            .map_err(|error| {
                                AppError::Channel(format!(
                                    "Failed to write temporary target file: {error}"
                                ))
                            })?;
                        bytes_written = bytes_written.saturating_add(read as u64);
                        controller.update_progress(bytes_written, total_size);
                        if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                            last_progress = Instant::now();
                            let _ = app.emit(
                                "transfer-event",
                                &controller.build_event("progress", file.size, None),
                            );
                        }
                    }
                    temp_file.flush().await.map_err(|error| {
                        AppError::Channel(format!("Failed to flush temporary target file: {error}"))
                    })?;
                    Ok(())
                }
                .await;
                drop(temp_file);
                let _ = sftp.close().await;
                if let Err(error) = write_result {
                    cleanup_local_copy_temp(&temp).await;
                    return Err(error);
                }
                if let Err(error) = commit_local_copy_temp(&temp, &target).await {
                    return Err(error);
                }
                if settings.preserve_timestamps {
                    if let Some(mtime) = file.mtime {
                        let modified =
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime as u64);
                        if let Ok(local_file) = std::fs::OpenOptions::new().read(true).open(&target)
                        {
                            let _ = local_file.set_modified(modified);
                        }
                    }
                }
                completed = completed.saturating_add(1);
                controller.update_item_progress(completed, total_files);
            }

            Ok(DirectoryTransferSummary {
                completed,
                total_files,
                bytes: bytes_written,
                small_file_concurrency: 1,
            })
        }
        .await;

        match result {
            Ok(summary) => {
                log_transfer_performance(
                    "copy",
                    "directory",
                    summary.bytes,
                    started.elapsed(),
                    request_kib,
                    pipeline_depth,
                    max_concurrent_writes,
                    summary.small_file_concurrency,
                );
                controller.update_progress(summary.bytes, summary.bytes);
                controller.update_item_progress(summary.completed, summary.total_files);
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

    pub(crate) async fn copy_remote_directory_to_remote_streaming(
        &self,
        target: &SftpBackend,
        app: &tauri::AppHandle,
        source_session_id: &str,
        source_path: &str,
        target_path: &str,
        _target_existed: bool,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (files, total_size) = self
            .collect_remote_copy_files(source_path, target_path)
            .await?;
        let total_files = files.len() as u64;
        let controller = create_directory_transfer_controller(
            transfer_id,
            source_session_id,
            file_name_from_path(source_path),
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

        let active_remote_temp = Arc::new(StdMutex::new(None::<String>));
        let active_remote_temp_for_loop = active_remote_temp.clone();
        let result: AppResult<(u64, u64)> = async {
            let mut bytes_written = 0_u64;
            let mut completed = 0_u64;
            let mut last_progress = Instant::now();

            for file in files {
                wait_for_transfer_ready(&controller).await?;
                let source_sftp = self.open_sftp().await?;
                let target_sftp = target.open_sftp().await?;
                let temp_path = copy_remote_sidecar_path(&file.target_path, "tmp");
                *active_remote_temp_for_loop.lock().unwrap() = Some(temp_path.clone());
                if let Some(parent) = file.target_path.rsplit_once('/').map(|(parent, _)| parent) {
                    if !parent.is_empty() {
                        ensure_remote_dir_exists(&target_sftp, parent).await?;
                    }
                }

                let mut source_file = source_sftp.open(&file.source_path).await.map_err(|error| {
                    AppError::Channel(format!(
                        "Source connection read open failed for {}: {error}",
                        file.source_path
                    ))
                })?;
                let mut target_file = target
                    .create_remote_copy_temp_file(&target_sftp, &temp_path)
                    .await
                    .map_err(|error| {
                        AppError::Channel(format!(
                            "Target connection write open failed for {}: {error}",
                            file.target_path
                        ))
                    })?;
                let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
                let reader_controller = controller.clone();
                let source_path_owned = file.source_path.clone();
                let reader = tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 512 * 1024];
                    loop {
                        wait_for_transfer_ready(&reader_controller).await?;
                        let read = source_file.read(&mut buffer).await.map_err(|error| {
                            AppError::Channel(format!(
                                "Source connection disconnected or read failed for {source_path_owned}: {error}"
                            ))
                        })?;
                        if read == 0 {
                            break;
                        }
                        tx.send(buffer[..read].to_vec()).await.map_err(|_| {
                            AppError::Channel(
                                "Target writer stopped before source completed".to_string(),
                            )
                        })?;
                    }
                    AppResult::Ok(())
                });

                while let Some(chunk) = rx.recv().await {
                    wait_for_transfer_ready(&controller).await?;
                    wait_for_sftp_upload_io(
                        &controller,
                        None,
                        target_file.write_all(&chunk),
                        |error| {
                            AppError::Channel(format!(
                                "Target connection disconnected or write failed for {}: {error}",
                                file.target_path
                            ))
                        },
                    )
                    .await?;
                    bytes_written = bytes_written.saturating_add(chunk.len() as u64);
                    controller.update_progress(bytes_written, total_size);
                    if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                        last_progress = Instant::now();
                        let _ = app.emit(
                            "transfer-event",
                            &controller.build_event("progress", file.size, None),
                        );
                    }
                }
                reader.await.map_err(|error| {
                    AppError::Channel(format!("Source reader task failed: {error}"))
                })??;
                wait_for_sftp_upload_io(&controller, None, target_file.shutdown(), |error| {
                    AppError::Channel(format!(
                        "Target connection flush failed for {}: {error}",
                        file.target_path
                    ))
                })
                .await?;
                target
                    .commit_remote_copy_temp(&target_sftp, &temp_path, &file.target_path)
                    .await?;
                *active_remote_temp_for_loop.lock().unwrap() = None;
                let _ = source_sftp.close().await;
                let _ = target_sftp.close().await;
                completed = completed.saturating_add(1);
                controller.update_item_progress(completed, total_files);
            }

            Ok((bytes_written, completed))
        }
        .await;

        match result {
            Ok((bytes, completed)) => {
                controller.update_progress(bytes, total_size);
                controller.update_item_progress(completed, total_files);
                let _ = app.emit(
                    "transfer-event",
                    &controller.build_event("completed", 0, None),
                );
                unregister_transfer(&controller.id());
                Ok(())
            }
            Err(error) => {
                if let Ok(sftp) = target.open_sftp().await {
                    let temp_path = active_remote_temp.lock().unwrap().clone();
                    if let Some(temp_path) = temp_path {
                        target.cleanup_remote_copy_temp(&sftp, &temp_path).await;
                    }
                    let _ = sftp.close().await;
                }
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
                unregister_transfer(&controller.id());
                Err(error)
            }
        }
    }
}
