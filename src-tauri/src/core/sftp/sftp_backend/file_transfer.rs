//! Internal pieces of the SFTP backend moved out of `sftp_backend.rs`.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DownloadReadProgress {
    Continue(u64),
    Complete,
}

pub(super) fn unexpected_download_eof_error(
    remote_path: &str,
    local_path: &str,
    remote_size: u64,
    bytes_written: u64,
    request_kib: usize,
    payload_bytes: usize,
) -> AppError {
    AppError::Channel(format!(
        "Unexpected EOF while downloading {remote_path}: expected {remote_size} bytes, got {bytes_written} bytes (local_path={local_path}, request_kib={request_kib}, payload_bytes={payload_bytes})"
    ))
}

pub(super) fn classify_download_read_progress(
    remote_path: &str,
    local_path: &str,
    remote_size: u64,
    offset: u64,
    bytes_written: u64,
    bytes_read: usize,
    request_kib: usize,
    payload_bytes: usize,
) -> AppResult<DownloadReadProgress> {
    if offset >= remote_size {
        return Ok(DownloadReadProgress::Complete);
    }

    if bytes_read == 0 {
        return Err(unexpected_download_eof_error(
            remote_path,
            local_path,
            remote_size,
            bytes_written,
            request_kib,
            payload_bytes,
        ));
    }

    let next_offset = offset.saturating_add(bytes_read as u64);
    if next_offset >= remote_size {
        Ok(DownloadReadProgress::Complete)
    } else {
        Ok(DownloadReadProgress::Continue(next_offset))
    }
}

pub(super) fn ensure_download_complete(
    remote_path: &str,
    local_path: &str,
    remote_size: u64,
    bytes_written: u64,
    request_kib: usize,
    payload_bytes: usize,
) -> AppResult<()> {
    if bytes_written == remote_size {
        Ok(())
    } else {
        Err(unexpected_download_eof_error(
            remote_path,
            local_path,
            remote_size,
            bytes_written,
            request_kib,
            payload_bytes,
        ))
    }
}
pub(super) fn log_transfer_performance(
    direction: &str,
    kind: &str,
    bytes: u64,
    elapsed: Duration,
    request_kib: usize,
    pipeline_depth: usize,
    max_concurrent_writes: usize,
    concurrent_tasks: usize,
) {
    let elapsed_secs = elapsed.as_secs_f64().max(0.001);
    let mbps = bytes as f64 / 1024.0 / 1024.0 / elapsed_secs;
    log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "transfer.performance".to_string(),
        event: "sftp.transfer.completed".to_string(),
        message: "SFTP transfer performance summary".to_string(),
        ids: None,
        data: Some(serde_json::json!({
            "backend": "sftp",
            "direction": direction,
            "kind": kind,
            "bytes": bytes,
            "elapsed_ms": elapsed.as_millis(),
            "average_mbps": mbps,
            "request_kib": request_kib,
            "payload_bytes": sftp_payload_size(request_kib),
            "pipeline_depth": pipeline_depth,
            "max_concurrent_writes": max_concurrent_writes,
            "concurrent_tasks": concurrent_tasks,
        })),
        error: None,
        client_timestamp: None,
    });
}

pub(super) async fn cleanup_cancelled_upload(
    backend: &SftpBackend,
    remote_path: &str,
) -> AppResult<()> {
    let sftp = backend.open_sftp().await?;
    if sftp.metadata(remote_path).await.is_ok() {
        let _ = sftp.remove_file(remote_path).await;
    }
    let _ = sftp.close().await;
    Ok(())
}

pub(super) async fn close_download_remote_file(
    mut remote_file: russh_sftp::client::fs::File,
    remote_path: &str,
    local_path: &str,
    offset: Option<u64>,
) -> AppResult<()> {
    use tokio::io::AsyncWriteExt;

    remote_file.shutdown().await.map_err(|error| {
        let offset_context = offset
            .map(|offset| format!(", offset={offset}"))
            .unwrap_or_default();
        AppError::Channel(format!(
            "Failed to close remote file handle for '{remote_path}' (local_path={local_path}{offset_context}): {error}"
        ))
    })
}

pub(super) async fn read_sftp_chunk(
    remote_file: russh_sftp::client::fs::File,
    offset: u64,
    len: usize,
    remote_path: String,
    remote_size: u64,
    payload_bytes: usize,
) -> AppResult<(u64, Vec<u8>, bool, russh_sftp::client::fs::File)> {
    let mut data = Vec::with_capacity(len);
    let mut read_offset = offset;
    let end_offset = offset.saturating_add(len as u64).min(remote_size);
    let mut completed_range = true;

    while read_offset < end_offset {
        let remaining = (end_offset - read_offset) as usize;
        let chunk = remote_file
            .read_at(read_offset, remaining.min(payload_bytes))
            .await
            .map_err(|e| {
                AppError::Channel(format!(
                    "SFTP read failed for {remote_path} at offset {read_offset}: {e}"
                ))
            })?;
        if chunk.is_empty() {
            completed_range = false;
            break;
        }
        read_offset = match classify_download_read_progress(
            &remote_path,
            "",
            end_offset,
            read_offset,
            data.len() as u64,
            chunk.len(),
            0,
            payload_bytes,
        )? {
            DownloadReadProgress::Continue(next_offset) => next_offset,
            DownloadReadProgress::Complete => end_offset,
        };
        data.extend_from_slice(&chunk);
    }

    Ok((offset, data, completed_range, remote_file))
}

pub(super) async fn download_known_size_to_local_file<F, G>(
    sftp: &SftpSession,
    remote_path: &str,
    local_path: &str,
    local_file: &mut tokio::fs::File,
    total_size: u64,
    request_kib: usize,
    pipeline_depth: usize,
    max_pipeline_depth: usize,
    controller: &Arc<TransferController>,
    parent_controller: Option<&Arc<TransferController>>,
    path_cache: &RwLock<HashMap<String, Vec<u8>>>,
    mut on_bytes: F,
    mut on_progress_interval: G,
) -> AppResult<u64>
where
    F: FnMut(u64, u64),
    G: FnMut(u64),
{
    use std::io::SeekFrom;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    use tokio::task::JoinSet;

    if total_size == 0 {
        ensure_download_complete(
            remote_path,
            local_path,
            total_size,
            0,
            request_kib,
            sftp_payload_size(request_kib),
        )?;
        return Ok(0);
    }

    let chunk_size = sftp_payload_size(request_kib) as u64;
    let num_chunks = total_size.div_ceil(chunk_size) as usize;
    let concurrency = pipeline_depth
        .min(max_pipeline_depth.max(1))
        .min(num_chunks)
        .max(1);

    // Look up raw bytes path from cache for non-UTF-8 file names
    let cache = path_cache.read().await;
    let raw_path = cache.get(remote_path).cloned();
    drop(cache);

    let mut handle_pool: Vec<russh_sftp::client::fs::File> = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        handle_pool.push(if let Some(ref bytes) = raw_path {
            sftp.open_bytes(bytes.clone())
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?
        } else {
            sftp.open(remote_path)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?
        });
    }

    type Task = AppResult<(u64, Vec<u8>, bool, russh_sftp::client::fs::File)>;
    let mut join_set: JoinSet<Task> = JoinSet::new();
    let mut next_offset: u64 = 0;
    let mut last_progress = Instant::now();
    let mut bytes_transferred: u64 = 0;

    while let Some(fh) = handle_pool.pop() {
        if next_offset >= total_size {
            break;
        }
        wait_for_transfer_chain(controller, parent_controller).await?;
        let len = chunk_size.min(total_size - next_offset) as usize;
        let offset = next_offset;
        next_offset += len as u64;
        join_set.spawn(read_sftp_chunk(
            fh,
            offset,
            len,
            remote_path.to_string(),
            total_size,
            chunk_size as usize,
        ));
    }

    while let Some(res) = join_set.join_next().await {
        wait_for_transfer_chain(controller, parent_controller).await?;
        let (chunk_offset, data, completed_range, fh) =
            res.map_err(|e| AppError::Channel(format!("Task panicked: {}", e)))??;

        if !data.is_empty() {
            local_file
                .seek(SeekFrom::Start(chunk_offset))
                .await
                .map_err(|e| AppError::Channel(format!("Local seek failed: {}", e)))?;
            local_file
                .write_all(&data)
                .await
                .map_err(|e| AppError::Channel(format!("Local write failed: {}", e)))?;

            let delta = data.len() as u64;
            bytes_transferred = bytes_transferred.saturating_add(delta);
            on_bytes(bytes_transferred, delta);
        }

        if !completed_range {
            return Err(unexpected_download_eof_error(
                remote_path,
                local_path,
                total_size,
                bytes_transferred,
                request_kib,
                chunk_size as usize,
            ));
        }

        if next_offset < total_size {
            wait_for_transfer_chain(controller, parent_controller).await?;
            let len = chunk_size.min(total_size - next_offset) as usize;
            let offset = next_offset;
            next_offset += len as u64;
            join_set.spawn(read_sftp_chunk(
                fh,
                offset,
                len,
                remote_path.to_string(),
                total_size,
                chunk_size as usize,
            ));
        } else {
            close_download_remote_file(fh, remote_path, local_path, Some(chunk_offset)).await?;
        }

        if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
            last_progress = Instant::now();
            on_progress_interval(bytes_transferred);
        }
    }

    ensure_download_complete(
        remote_path,
        local_path,
        total_size,
        bytes_transferred,
        request_kib,
        chunk_size as usize,
    )?;

    Ok(bytes_transferred)
}

pub(super) async fn download_remote_file_inner_with_controller(
    backend: &SftpBackend,
    app: &tauri::AppHandle,
    _session_id: &str,
    remote_path: &str,
    actual_path: &str,
    ts: &crate::config::TransferSettings,
    controller: Arc<TransferController>,
    parent_controller: Option<Arc<TransferController>>,
) -> AppResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let (request_kib, pipeline_depth, max_concurrent_writes) =
        sftp_pipeline_config(ts, backend.pipeline_depth_override);
    let chunk_size = sftp_payload_size(request_kib) as u64;
    let transfer_started = Instant::now();

    let result: AppResult<u64> = async {
        if let Some(parent) = std::path::Path::new(&actual_path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to create local dir: {}", e)))?;
            }
        }

        let sftp = backend
            .open_sftp_with_client_config(sftp_client_config(request_kib, max_concurrent_writes))
            .await?;

        let remote_attrs = sftp.metadata(remote_path).await.ok();
        let remote_size = remote_attrs.as_ref().and_then(|m| m.size);
        let total_size = remote_size.unwrap_or(0);
        controller.update_progress(0, total_size);

        let mut local_file = tokio::fs::File::create(&actual_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create local file: {}", e)))?;

        if total_size > 0 {
            let _ = local_file.set_len(total_size).await;
        }

        let mut bytes_transferred: u64 = 0;

        if let Some(total_size) = remote_size {
            bytes_transferred = download_known_size_to_local_file(
                &sftp,
                remote_path,
                actual_path,
                &mut local_file,
                total_size,
                request_kib,
                pipeline_depth,
                pipeline_depth,
                &controller,
                parent_controller.as_ref(),
                &backend.path_cache,
                |current, _delta| {
                    controller.update_progress(current, total_size);
                },
                |current| {
                    controller.update_progress(current, total_size);
                    emit_parent_progress(app, parent_controller.as_ref());
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                },
            )
            .await?;
        } else {
            // Look up raw bytes path from cache for non-UTF-8 file names
            let cache = backend.path_cache.read().await;
            let raw_path = cache.get(remote_path).cloned();
            drop(cache);

            let mut last_progress = Instant::now();
            let mut remote_file = if let Some(ref bytes) = raw_path {
                sftp.open_bytes(bytes.clone())
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?
            } else {
                sftp.open(remote_path)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to open remote file: {}", e)))?
            };

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

                if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    emit_parent_progress(app, parent_controller.as_ref());
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", 0, None),
                    );
                }
            }

            close_download_remote_file(remote_file, remote_path, actual_path, None).await?;
        }

        local_file
            .flush()
            .await
            .map_err(|e| AppError::Channel(format!("Flush failed: {}", e)))?;

        if let Some(remote_size) = remote_size {
            ensure_download_complete(
                remote_path,
                actual_path,
                remote_size,
                bytes_transferred,
                request_kib,
                chunk_size as usize,
            )?;
        }

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
            log_transfer_performance(
                "download",
                "file",
                size,
                transfer_started.elapsed(),
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

pub(super) async fn upload_local_file_inner_with_controller(
    backend: &SftpBackend,
    app: &tauri::AppHandle,
    _session_id: &str,
    local_path: &str,
    remote_path: &str,
    ts: &crate::config::TransferSettings,
    controller: Arc<TransferController>,
    parent_controller: Option<Arc<TransferController>>,
) -> AppResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_transfer(controller.clone());
    let _ = app.emit(
        "transfer-event",
        &controller.build_event("started", 0, None),
    );

    let (request_kib, pipeline_depth, max_concurrent_writes) =
        sftp_pipeline_config(ts, backend.pipeline_depth_override);
    let chunk_size = sftp_payload_size(request_kib);
    let transfer_started = Instant::now();

    let result: AppResult<u64> = async {
        let local_meta = tokio::fs::metadata(local_path).await;
        let total_size = local_meta.as_ref().map(|m| m.len()).unwrap_or(0);
        controller.update_progress(0, total_size);

        let sftp = backend
            .open_sftp_with_client_config(sftp_client_config(request_kib, max_concurrent_writes))
            .await?;
        let mut bytes_transferred: u64 = 0;

        let mut last_progress = Instant::now();
        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open local file: {}", e)))?;
        let mut remote_file = if backend.encoding() != "UTF-8" {
            let path_bytes = backend.encode_path_for_sftp(remote_path);
            use russh_sftp::protocol::OpenFlags;
            sftp.open_with_flags_bytes(
                path_bytes,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            )
            .await
            .map_err(|e| AppError::Channel(format!("Failed to create remote file: {}", e)))?
        } else {
            sftp.create(remote_path)
                .await
                .map_err(|e| AppError::Channel(format!("Failed to create remote file: {}", e)))?
        };

        if total_size > 0 {
            let mut buf = vec![0u8; chunk_size];
            loop {
                wait_for_transfer_chain(&controller, parent_controller.as_ref()).await?;
                let read = local_file
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Channel(format!("Failed to read local file: {}", e)))?;
                if read == 0 {
                    break;
                }

                // russh-sftp >= 2.3 pipelines write ACKs internally according to
                // client::Config::max_concurrent_writes; shutdown below drains them.
                wait_for_sftp_upload_io(
                    &controller,
                    parent_controller.as_ref(),
                    remote_file.write_all(&buf[..read]),
                    |e| AppError::Channel(format!("SFTP write failed: {}", e)),
                )
                .await?;

                bytes_transferred += read as u64;
                controller.update_progress(bytes_transferred, total_size);

                if last_progress.elapsed() >= TRANSFER_PROGRESS_INTERVAL {
                    last_progress = Instant::now();
                    emit_parent_progress(app, parent_controller.as_ref());
                    let _ = app.emit(
                        "transfer-event",
                        &controller.build_event("progress", total_size, None),
                    );
                }
            }
        }

        wait_for_sftp_upload_io(
            &controller,
            parent_controller.as_ref(),
            remote_file.shutdown(),
            |e| AppError::Channel(format!("SFTP flush failed: {}", e)),
        )
        .await?;

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
            log_transfer_performance(
                "upload",
                "file",
                size,
                transfer_started.elapsed(),
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
