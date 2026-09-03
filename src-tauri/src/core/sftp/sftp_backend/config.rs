//! Internal pieces of the SFTP backend moved out of `sftp_backend.rs`.

use super::*;

pub(super) const SFTP_MIN_REQUEST_KIB: usize = 64;
pub(super) const SFTP_MAX_REQUEST_KIB: usize = 256;
pub(super) const SFTP_PIPELINE_TARGET_KIB: usize = 1024;
pub(super) const SFTP_WRITE_PIPELINE_TARGET_KIB: usize = 2048;
pub(super) const SFTP_MIN_PIPELINE_DEPTH: usize = 4;
pub(super) const SFTP_AUTO_MAX_PIPELINE_DEPTH: usize = 16;
pub(super) const SFTP_USER_MAX_PIPELINE_DEPTH: usize = 64;
pub(super) const SFTP_MIN_CONCURRENT_WRITES: usize = 8;
pub(super) const SFTP_MAX_CONCURRENT_WRITES: usize = 16;
pub(super) const SFTP_PACKET_OVERHEAD_RESERVE: usize = 1024;
pub(super) const TRANSFER_PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
pub(super) const SFTP_SMALL_FILE_THRESHOLD: u64 = 512 * 1024;
pub(super) const SFTP_DEFAULT_SMALL_FILE_CONCURRENCY: usize = 16;
pub(super) const SFTP_MAX_SMALL_FILE_CONCURRENCY: usize = 16;
pub(super) const SFTP_SMALL_FILE_WORKERS_PER_SESSION: usize = 8;
pub(super) const SFTP_DEFAULT_SESSION_POOL_SIZE: usize = 2;
pub(super) const SFTP_MAX_SESSION_POOL_SIZE: usize = 4;
pub(super) const SFTP_LARGE_FILE_CONCURRENCY: usize = 2;
pub(super) const SFTP_HANDLE_RESERVE: usize = 8;
pub(super) const SFTP_DIRECTORY_STALL_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const SFTP_CHANNEL_OPEN_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(150),
    Duration::from_millis(300),
];

pub(super) fn sftp_pipeline_config(
    ts: &crate::config::TransferSettings,
    pipeline_depth_override: Option<u32>,
) -> (usize, usize, usize) {
    let request_kib =
        (ts.transfer_buffer_size as usize).clamp(SFTP_MIN_REQUEST_KIB, SFTP_MAX_REQUEST_KIB);
    let automatic_pipeline_depth = SFTP_PIPELINE_TARGET_KIB
        .div_ceil(request_kib)
        .clamp(SFTP_MIN_PIPELINE_DEPTH, SFTP_AUTO_MAX_PIPELINE_DEPTH);
    let automatic_max_concurrent_writes = SFTP_WRITE_PIPELINE_TARGET_KIB
        .div_ceil(request_kib)
        .clamp(SFTP_MIN_CONCURRENT_WRITES, SFTP_MAX_CONCURRENT_WRITES);
    let pipeline_depth_override = pipeline_depth_override
        .map(|value| (value as usize).clamp(SFTP_MIN_PIPELINE_DEPTH, SFTP_USER_MAX_PIPELINE_DEPTH));
    let pipeline_depth = pipeline_depth_override.unwrap_or(automatic_pipeline_depth);
    let max_concurrent_writes = pipeline_depth_override.unwrap_or(automatic_max_concurrent_writes);
    (request_kib, pipeline_depth, max_concurrent_writes)
}

pub(super) fn sftp_client_config(
    request_kib: usize,
    max_concurrent_writes: usize,
) -> SftpClientConfig {
    SftpClientConfig {
        max_packet_len: (request_kib * 1024) as u32,
        max_concurrent_writes,
        ..SftpClientConfig::default()
    }
}

pub(super) fn is_sftp_not_found(error: &SftpError) -> bool {
    matches!(
        error,
        SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile
    )
}

#[allow(dead_code)]
pub(super) fn ignore_sftp_not_found(result: Result<(), SftpError>) -> AppResult<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_sftp_not_found(&error) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn is_retryable_sftp_channel_open_error(error: &russh::Error) -> bool {
    matches!(
        error,
        russh::Error::ChannelOpenFailure(
            ChannelOpenFailure::ConnectFailed | ChannelOpenFailure::ResourceShortage
        )
    )
}

#[allow(dead_code)]
pub(super) fn sftp_remove_error(path: &str, kind: &str, error: SftpError) -> Option<String> {
    if is_sftp_not_found(&error) {
        None
    } else {
        Some(format!("Failed to remove {kind} '{}': {error}", path))
    }
}

pub(super) fn sftp_payload_size(request_kib: usize) -> usize {
    (request_kib * 1024)
        .saturating_sub(SFTP_PACKET_OVERHEAD_RESERVE)
        .max(32 * 1024)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SftpDirectoryConcurrency {
    pub(super) session_pool_size: usize,
    pub(super) small_file_concurrency: usize,
    pub(super) large_file_concurrency: usize,
}

pub(super) fn sftp_directory_concurrency(
    max_open_handles: Option<u64>,
) -> SftpDirectoryConcurrency {
    let server_limit = max_open_handles
        .map(|handles| handles.saturating_sub(SFTP_HANDLE_RESERVE as u64) as usize)
        .unwrap_or(SFTP_DEFAULT_SMALL_FILE_CONCURRENCY)
        .max(1);
    let session_pool_size = SFTP_DEFAULT_SESSION_POOL_SIZE
        .min(SFTP_MAX_SESSION_POOL_SIZE)
        .min(server_limit)
        .max(1);
    let small_file_concurrency = server_limit
        .min(session_pool_size * SFTP_SMALL_FILE_WORKERS_PER_SESSION)
        .min(SFTP_MAX_SMALL_FILE_CONCURRENCY)
        .max(1);
    let large_file_concurrency = SFTP_LARGE_FILE_CONCURRENCY
        .min(small_file_concurrency)
        .max(1);

    SftpDirectoryConcurrency {
        session_pool_size,
        small_file_concurrency,
        large_file_concurrency,
    }
}

pub(super) fn sftp_directory_download_pipeline_cap(
    max_open_handles: Option<u64>,
    session_pool_size: usize,
    worker_count: usize,
    requested_pipeline_depth: usize,
) -> usize {
    let requested_pipeline_depth = requested_pipeline_depth.max(1);

    let Some(max_open_handles) = max_open_handles else {
        return requested_pipeline_depth;
    };

    let available_per_session = max_open_handles
        .saturating_sub(SFTP_HANDLE_RESERVE as u64)
        .max(1) as usize;
    let session_pool_size = session_pool_size.max(1);
    let workers_per_session = worker_count.div_ceil(session_pool_size).max(1);
    let budget_per_worker = available_per_session
        .checked_div(workers_per_session)
        .unwrap_or(1)
        .max(1);

    requested_pipeline_depth.min(budget_per_worker).max(1)
}
