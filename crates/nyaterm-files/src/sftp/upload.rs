//! Direct uploads retain one verified handle through writing and metadata restoration.

use super::upload_metadata::finish_upload_metadata;
use super::upload_runtime::{UploadRetryUnsafe, UploadRuntime, confirmed_failure};
use super::{
    SftpPathCodec, SftpTransferControl, await_sftp_request, sftp_timestamp_secs,
    sftp_upload_buffer_size,
};
use crate::sftp_transfer_types::{SftpTransferOptions, SftpTransferProgress};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[expect(clippy::too_many_arguments)]
pub(super) async fn upload_local_file<F>(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    runtime: &UploadRuntime,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    upload_local_file_bytes(
        sftp,
        codec.encode_path(remote_path)?,
        local_path,
        remote_path,
        control,
        options,
        runtime,
        progress,
    )
    .await
}

#[expect(clippy::too_many_arguments)]
pub(super) async fn upload_local_file_bytes<F>(
    sftp: &SftpSession,
    raw_path: Vec<u8>,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    runtime: &UploadRuntime,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send,
{
    control.wait_if_paused().await?;
    let mut local = tokio::fs::File::open(local_path).await?;
    let local_metadata = local.metadata().await.ok();
    let total_bytes = local_metadata.as_ref().map(|metadata| metadata.len());
    let (original, before) = runtime.begin_file(sftp, &raw_path, control).await?;
    let flags = if before.is_none() {
        OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE
    } else {
        OpenFlags::WRITE
    };
    let mut remote =
        await_sftp_request(control, sftp.open_with_flags_bytes(raw_path.clone(), flags)).await?;
    let opened = runtime
        .verify_handle(sftp, &remote, &raw_path, before.as_ref(), control)
        .await?;
    let mut uncertain = false;
    let mut drained = false;
    let result = async {
        if before.is_some() {
            let truncated = await_sftp_request(
                control,
                remote.set_metadata(FileAttributes {
                    size: Some(0),
                    ..FileAttributes::empty()
                }),
            )
            .await;
            uncertain = truncated
                .as_ref()
                .err()
                .is_some_and(|error| !confirmed_failure(error));
            truncated?;
        }
        let mut buffer = vec![0_u8; sftp_upload_buffer_size(options)];
        let mut bytes = 0_u64;
        progress(SftpTransferProgress {
            phase: None,
            remote_path: remote_path.to_string(),
            local_path: local_path.to_path_buf(),
            bytes_transferred: bytes,
            total_bytes,
            item_count_completed: None,
            item_count_total: None,
        });
        loop {
            control.wait_if_paused().await?;
            let read = local.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            let written = await_sftp_request(control, remote.write_all(&buffer[..read])).await;
            uncertain = written
                .as_ref()
                .err()
                .is_some_and(|error| !confirmed_failure(error));
            written?;
            control.wait_if_paused().await?;
            bytes += read as u64;
            progress(SftpTransferProgress {
                phase: None,
                remote_path: remote_path.to_string(),
                local_path: local_path.to_path_buf(),
                bytes_transferred: bytes,
                total_bytes,
                item_count_completed: None,
                item_count_total: None,
            });
        }
        let flushed = await_sftp_request(control, remote.flush()).await;
        uncertain = flushed
            .as_ref()
            .err()
            .is_some_and(|error| !confirmed_failure(error));
        flushed?;
        drained = true;
        let metadata = finish_upload_metadata(
            &remote,
            original.as_ref(),
            options.default_file_mode,
            control,
        )
        .await;
        uncertain = metadata
            .as_ref()
            .err()
            .is_some_and(|error| !confirmed_failure(error));
        metadata?;
        if options.preserve_timestamps
            && let Some(metadata) = local_metadata.as_ref()
            && let Some(mtime) = metadata.modified().ok().and_then(sftp_timestamp_secs)
        {
            let atime = metadata
                .accessed()
                .ok()
                .and_then(sftp_timestamp_secs)
                .unwrap_or(mtime);
            // Preserve the existing best-effort timestamp behavior, but an
            // unconfirmed request may not produce a retry checkpoint.
            let preserved = await_sftp_request(
                control,
                remote.set_metadata(FileAttributes {
                    atime: Some(atime),
                    mtime: Some(mtime),
                    ..FileAttributes::empty()
                }),
            )
            .await;
            uncertain = preserved
                .as_ref()
                .err()
                .is_some_and(|error| !confirmed_failure(error));
        }
        Ok::<_, anyhow::Error>(bytes)
    }
    .await;

    if uncertain || control.is_cancelled() {
        runtime.block();
        return result.and_then(|_| Err(UploadRetryUnsafe.into()));
    }
    // A failed WRITE consumes one ACK. Drain the remaining bounded window; a
    // timeout is permanently unsafe even if a later request could succeed.
    if !drained {
        for _ in 0..=64 {
            match await_sftp_request(control, remote.flush()).await {
                Ok(()) => {
                    drained = true;
                    break;
                }
                Err(error) if confirmed_failure(&error) => {}
                Err(_) => break,
            }
        }
    }
    if !drained {
        runtime.block();
        return result.and_then(|_| Err(UploadRetryUnsafe.into()));
    }
    let checkpoint = runtime
        .file_checkpoint(sftp, &remote, &raw_path, &opened, control)
        .await;
    let closed = await_sftp_request(control, remote.shutdown()).await;
    match (checkpoint, closed) {
        (Ok(checkpoint), Ok(())) => runtime.finish_file(&raw_path, checkpoint),
        _ => {
            runtime.block();
            return result.and_then(|_| Err(UploadRetryUnsafe.into()));
        }
    }
    result
}
