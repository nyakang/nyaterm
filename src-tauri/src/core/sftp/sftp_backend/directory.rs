//! Internal pieces of the SFTP backend moved out of `sftp_backend.rs`.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct RemoteDirectoryFile {
    pub(super) remote_path: String,
    pub(super) local_path: String,
    pub(super) size: u64,
    pub(super) mtime: Option<u32>,
}

#[derive(Clone, Debug)]
pub(super) struct LocalDirectoryFile {
    pub(super) local_path: String,
    pub(super) remote_path: String,
    pub(super) size: u64,
    pub(super) mtime: Option<std::time::SystemTime>,
    pub(super) atime: Option<std::time::SystemTime>,
}

pub(super) struct RemoteDirectoryInventory {
    pub(super) files: Vec<RemoteDirectoryFile>,
    pub(super) total_files: u64,
    pub(super) total_size: u64,
    pub(super) max_open_handles: Option<u64>,
}

pub(super) struct LocalDirectoryInventory {
    pub(super) files: Vec<LocalDirectoryFile>,
    pub(super) total_files: u64,
    pub(super) total_size: u64,
    pub(super) max_open_handles: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DirectoryTransferSummary {
    pub(super) completed: u64,
    pub(super) total_files: u64,
    pub(super) bytes: u64,
    pub(super) small_file_concurrency: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct RemoteRemoveEntry {
    pub(super) display_path: String,
    pub(super) raw_path: Vec<u8>,
}

#[allow(dead_code)]
pub(super) struct RemoveInventory {
    pub(super) files: Vec<RemoteRemoveEntry>,
    pub(super) dirs: Vec<RemoteRemoveEntry>,
}

#[allow(dead_code)]
pub(super) async fn collect_remove_inventory(
    sftp: &SftpSession,
    display_path: &str,
    path_bytes: Vec<u8>,
) -> AppResult<RemoveInventory> {
    let display_path = normalize_remote_dir_path(display_path).to_string();
    let path_bytes = normalize_remote_dir_path_bytes(&path_bytes);
    let dir = match sftp.read_dir_bytes(path_bytes.clone()).await {
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
    let mut dirs = vec![RemoteRemoveEntry {
        display_path: display_path.clone(),
        raw_path: path_bytes.clone(),
    }];

    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child_display = join_remote_child(&display_path, &name);
        let child_bytes = join_remote_child_bytes(&path_bytes, entry.file_name_bytes());
        if entry.file_type() == FileType::Dir {
            let child_inventory =
                Box::pin(collect_remove_inventory(sftp, &child_display, child_bytes)).await?;
            files.extend(child_inventory.files);
            dirs.extend(child_inventory.dirs);
        } else {
            files.push(RemoteRemoveEntry {
                display_path: child_display,
                raw_path: child_bytes,
            });
        }
    }

    Ok(RemoveInventory { files, dirs })
}

#[allow(dead_code)]
pub(super) async fn remove_inventory_concurrent(
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
                if let Err(error) = session.remove_file_bytes(file.raw_path.clone()).await {
                    if let Some(message) = sftp_remove_error(&file.display_path, "file", error) {
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
        .sort_by_key(|dir| std::cmp::Reverse(dir.raw_path.iter().filter(|b| **b == b'/').count()));
    for dir in inventory.dirs {
        let session = pool.session_for(0);
        if let Err(error) = session.remove_dir_bytes(dir.raw_path).await {
            if let Some(message) = sftp_remove_error(&dir.display_path, "directory", error) {
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
    #[allow(dead_code)]
    pub(super) async fn remove_dir_fast(&self, path: &str) -> AppResult<()> {
        let path_ref = RemotePathRef::new(path, None)?;
        self.remove_dir_fast_ref(&path_ref).await
    }

    pub(super) async fn remove_dir_fast_ref(&self, path: &RemotePathRef) -> AppResult<()> {
        let is_utf8_sftp_encoding =
            Encoding::for_label(self.encoding.trim().as_bytes()).unwrap_or(UTF_8) == UTF_8;
        let has_raw_path = path.raw_path().is_some();
        let raw_path_matches_display_path = raw_path_matches_display_path(path);
        let is_safe_target = is_safe_recursive_remove_target(path.display_path());
        if is_utf8_sftp_encoding && raw_path_matches_display_path && is_safe_target {
            let command = format!(
                "rm -rf -- {}",
                sh_quote(normalize_remote_dir_path(path.display_path()))
            );
            tracing::info!(
                remote_path = path.display_path(),
                command = %command,
                "Trying remote rm -rf fast path for directory delete"
            );
            match self.exec_ok(&command).await {
                Ok(_) => {
                    tracing::info!(
                        remote_path = path.display_path(),
                        command = %command,
                        "Remote rm -rf fast path succeeded"
                    );
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(
                        remote_path = path.display_path(),
                        error = %error,
                        "Remote rm -rf fast path failed, falling back to SFTP recursive delete"
                    );
                }
            }
        } else {
            tracing::info!(
                remote_path = path.display_path(),
                is_utf8_sftp_encoding,
                has_raw_path,
                raw_path_matches_display_path,
                is_safe_target,
                "Remote rm -rf fast path not used; falling back to SFTP recursive delete"
            );
        }

        let raw_path = self.remote_path_bytes(path);
        if !is_safe_recursive_remove_target_bytes(&raw_path) {
            return Err(AppError::Channel(format!(
                "Refusing to recursively delete unsafe remote path '{}'",
                path.display_path()
            )));
        }

        self.remove_dir_concurrent_sftp(path.display_path(), raw_path)
            .await
    }

    #[allow(dead_code)]
    pub(super) async fn remove_dir_concurrent_sftp(
        &self,
        path: &str,
        path_bytes: Vec<u8>,
    ) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        let max_open_handles = sftp.max_open_handles();
        let result = collect_remove_inventory(&sftp, path, path_bytes.clone()).await;
        let _ = sftp.close().await;
        let inventory = result?;

        if inventory.files.is_empty() && inventory.dirs.is_empty() {
            return Ok(());
        }

        if inventory.files.is_empty() && inventory.dirs.len() <= 1 {
            let sftp = self.open_sftp().await?;
            let result = sftp
                .remove_dir_bytes(normalize_remote_dir_path_bytes(&path_bytes))
                .await;
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

    pub(super) async fn collect_remote_directory_inventory(
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

    pub(super) async fn collect_remote_directory_inventory_inner(
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

    pub(super) async fn collect_local_directory_inventory(
        &self,
        local_path: &str,
        remote_path: &str,
        directory_controller: &Arc<TransferController>,
        transfer_settings: &crate::config::TransferSettings,
    ) -> AppResult<LocalDirectoryInventory> {
        let (request_kib, _, max_concurrent_writes) =
            sftp_pipeline_config(transfer_settings, self.pipeline_depth_override);
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

    pub(super) async fn collect_local_directory_inventory_inner(
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

    pub(super) async fn download_remote_directory_files(
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

        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(transfer_settings, self.pipeline_depth_override);
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
            request_kib,
            pipeline_depth,
            concurrency,
            self.path_cache.clone(),
        )
        .await;
        pool.close_all().await;
        result
    }

    pub(super) async fn upload_local_directory_files(
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

        let (request_kib, _, max_concurrent_writes) =
            sftp_pipeline_config(transfer_settings, self.pipeline_depth_override);
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
            request_kib,
            concurrency,
        )
        .await;
        pool.close_all().await;
        result
    }
}

pub(super) fn raw_path_matches_display_path(path: &RemotePathRef) -> bool {
    path.raw_path().map_or(true, |raw_path| {
        normalize_remote_dir_path_bytes(raw_path)
            == normalize_remote_dir_path(path.display_path()).as_bytes()
    })
}

pub(super) fn sftp_directory_file_concurrency(
    files_len: usize,
    concurrency: SftpDirectoryConcurrency,
) -> usize {
    files_len.min(concurrency.small_file_concurrency).max(1)
}

pub(super) fn add_directory_transferred_bytes(
    directory_controller: &Arc<TransferController>,
    completed_bytes: &AtomicU64,
    delta: u64,
    total_size: u64,
) -> u64 {
    let bytes_done = completed_bytes.fetch_add(delta, Ordering::SeqCst) + delta;
    directory_controller.update_progress(bytes_done, total_size);
    bytes_done
}

pub(super) async fn wait_for_sftp_upload_io<T, F, M>(
    controller: &Arc<TransferController>,
    parent_controller: Option<&Arc<TransferController>>,
    future: F,
    map_error: M,
) -> AppResult<T>
where
    F: Future<Output = std::io::Result<T>>,
    M: FnOnce(std::io::Error) -> AppError,
{
    tokio::select! {
        result = future => result.map_err(map_error),
        cancelled = wait_for_transfer_cancelled(controller) => match cancelled {
            Err(error) => Err(error),
            Ok(()) => unreachable!("wait_for_transfer_cancelled only returns on cancellation"),
        },
        cancelled = async {
            if let Some(parent) = parent_controller {
                wait_for_transfer_cancelled(parent).await
            } else {
                std::future::pending::<AppResult<()>>().await
            }
        } => match cancelled {
            Err(error) => Err(error),
            Ok(()) => unreachable!("wait_for_transfer_cancelled only returns on cancellation"),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectoryProgressSnapshot {
    pub(super) bytes: u64,
    pub(super) completed: u64,
}

pub(super) fn directory_progress_snapshot(
    completed_bytes: &AtomicU64,
    completed_count: &AtomicU64,
) -> DirectoryProgressSnapshot {
    DirectoryProgressSnapshot {
        bytes: completed_bytes.load(Ordering::SeqCst),
        completed: completed_count.load(Ordering::SeqCst),
    }
}

pub(super) fn directory_transfer_stalled(
    control_state: TransferControlState,
    last_progress: DirectoryProgressSnapshot,
    current_progress: DirectoryProgressSnapshot,
    idle_for: Duration,
    total_files: u64,
) -> bool {
    control_state == TransferControlState::Running
        && current_progress == last_progress
        && current_progress.completed < total_files
        && idle_for >= SFTP_DIRECTORY_STALL_TIMEOUT
}

pub(super) fn handle_directory_worker_result(
    operation: &str,
    result: Result<AppResult<()>, tokio::task::JoinError>,
    first_err: &mut Option<AppError>,
    join_set: &mut tokio::task::JoinSet<AppResult<()>>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            if first_err.is_none() {
                *first_err = Some(error);
                join_set.abort_all();
            }
        }
        Err(error) if error.is_cancelled() && first_err.is_some() => {}
        Err(error) => {
            if first_err.is_none() {
                *first_err = Some(AppError::Channel(format!(
                    "Directory {operation} worker panicked: {error}"
                )));
                join_set.abort_all();
            }
        }
    }
}

pub(super) async fn run_download_directory_workers(
    app: &tauri::AppHandle,
    pool: SftpSessionPool,
    inventory: RemoteDirectoryInventory,
    directory_controller: Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    request_kib: usize,
    requested_pipeline_depth: usize,
    concurrency: SftpDirectoryConcurrency,
    path_cache: Arc<RwLock<HashMap<String, Vec<u8>>>>,
) -> AppResult<DirectoryTransferSummary> {
    let worker_count = sftp_directory_file_concurrency(inventory.files.len(), concurrency);
    let effective_pipeline_depth = sftp_directory_download_pipeline_cap(
        inventory.max_open_handles,
        concurrency.session_pool_size,
        worker_count,
        requested_pipeline_depth,
    );
    let workers_per_session = worker_count
        .div_ceil(concurrency.session_pool_size.max(1))
        .max(1);
    log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "transfer.sftp".to_string(),
        event: "sftp.directory.download.pipeline_budget".to_string(),
        message: "SFTP directory download pipeline budget".to_string(),
        ids: None,
        data: Some(serde_json::json!({
            "max_open_handles": inventory.max_open_handles,
            "handle_reserve": SFTP_HANDLE_RESERVE,
            "session_pool_size": concurrency.session_pool_size,
            "worker_count": worker_count,
            "workers_per_session": workers_per_session,
            "requested_pipeline_depth": requested_pipeline_depth,
            "effective_pipeline_depth": effective_pipeline_depth,
        })),
        error: None,
        client_timestamp: None,
    });

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
        let path_cache = path_cache.clone();
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
                    request_kib,
                    requested_pipeline_depth,
                    effective_pipeline_depth,
                    &path_cache,
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
    let mut last_progress = directory_progress_snapshot(&completed_bytes, &completed_count);
    let mut last_progress_at = Instant::now();
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = join_set.join_next() => {
                let Some(result) = result else {
                    break;
                };
                handle_directory_worker_result("download", result, &mut first_err, &mut join_set);
            }
            _ = watchdog.tick(), if first_err.is_none() => {
                let current_progress = directory_progress_snapshot(&completed_bytes, &completed_count);
                match directory_controller.control_state() {
                    TransferControlState::Cancelled => {
                        first_err = Some(AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.to_string()));
                        join_set.abort_all();
                    }
                    TransferControlState::Paused => {
                        last_progress = current_progress;
                        last_progress_at = Instant::now();
                    }
                    TransferControlState::Running if current_progress != last_progress => {
                        last_progress = current_progress;
                        last_progress_at = Instant::now();
                    }
                    state if directory_transfer_stalled(
                        state,
                        last_progress,
                        current_progress,
                        last_progress_at.elapsed(),
                        total_files,
                    ) => {
                        first_err = Some(AppError::Channel("SFTP transfer stalled".to_string()));
                        join_set.abort_all();
                    }
                    _ => {}
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

pub(super) async fn run_upload_directory_workers(
    app: &tauri::AppHandle,
    pool: SftpSessionPool,
    inventory: LocalDirectoryInventory,
    directory_controller: Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    request_kib: usize,
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
                    request_kib,
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
    let mut last_progress = directory_progress_snapshot(&completed_bytes, &completed_count);
    let mut last_progress_at = Instant::now();
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = join_set.join_next() => {
                let Some(result) = result else {
                    break;
                };
                handle_directory_worker_result("upload", result, &mut first_err, &mut join_set);
            }
            _ = watchdog.tick(), if first_err.is_none() => {
                let current_progress = directory_progress_snapshot(&completed_bytes, &completed_count);
                match directory_controller.control_state() {
                    TransferControlState::Cancelled => {
                        first_err = Some(AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.to_string()));
                        join_set.abort_all();
                    }
                    TransferControlState::Paused => {
                        last_progress = current_progress;
                        last_progress_at = Instant::now();
                    }
                    TransferControlState::Running if current_progress != last_progress => {
                        last_progress = current_progress;
                        last_progress_at = Instant::now();
                    }
                    state if directory_transfer_stalled(
                        state,
                        last_progress,
                        current_progress,
                        last_progress_at.elapsed(),
                        total_files,
                    ) => {
                        first_err = Some(AppError::Channel("SFTP transfer stalled".to_string()));
                        join_set.abort_all();
                    }
                    _ => {}
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

pub(super) async fn download_directory_file_with_session(
    app: &tauri::AppHandle,
    sftp: Arc<ManagedSftpSession>,
    file: RemoteDirectoryFile,
    directory_controller: &Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    completed_bytes: &Arc<AtomicU64>,
    total_size: u64,
    request_kib: usize,
    pipeline_depth: usize,
    max_pipeline_depth: usize,
    path_cache: &RwLock<HashMap<String, Vec<u8>>>,
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
            path_cache,
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

pub(super) async fn upload_directory_file_with_session(
    app: &tauri::AppHandle,
    sftp: Arc<ManagedSftpSession>,
    file: LocalDirectoryFile,
    directory_controller: &Arc<TransferController>,
    transfer_settings: &crate::config::TransferSettings,
    completed_bytes: &Arc<AtomicU64>,
    total_size: u64,
    request_kib: usize,
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
        wait_for_sftp_upload_io(
            directory_controller,
            None,
            remote_file.write_all(&buf[..read]),
            |e| AppError::Channel(format!("SFTP write failed for {}: {}", file.remote_path, e)),
        )
        .await?;
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
    wait_for_sftp_upload_io(directory_controller, None, remote_file.shutdown(), |e| {
        AppError::Channel(format!("SFTP flush failed for {}: {}", file.remote_path, e))
    })
    .await?;

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
