//! FTP leaves share checkpoints only with retries of their owning task.
use crate::ftp::{FtpError, FtpFileService};
use crate::remote_file::copy_progress::CopyProgress;
use crate::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};
use crate::sftp_transfer_types::SftpPathTransferOptions;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) fn download(
    service: &FtpFileService,
    remote: &str,
    root: &Path,
    local: &Path,
    replace: bool,
    control: &SftpTransferControl,
    options: &SftpPathTransferOptions,
    progress: &CopyProgress,
) -> anyhow::Result<u64> {
    let mut previous = 0;
    options
        .ftp_runtime
        .download(
            service,
            remote,
            root,
            local,
            replace,
            control,
            options.transfer_options(),
            options.ftp_binary,
            |bytes, phase| {
                progress.replace_file_bytes(previous, bytes);
                previous = bytes;
                progress.phase(phase);
            },
        )
        .map_err(transfer_error)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn upload(
    service: &FtpFileService,
    local: &Path,
    remote: &str,
    replace: bool,
    control: &SftpTransferControl,
    options: &SftpPathTransferOptions,
    progress: &CopyProgress,
) -> anyhow::Result<crate::ftp::service::FtpWriteOutcome> {
    let mut previous = 0;
    options
        .ftp_runtime
        .upload(
            service,
            local,
            remote,
            replace,
            control,
            options.transfer_options(),
            options.ftp_binary,
            None,
            |bytes, phase| {
                progress.replace_file_bytes(previous, bytes);
                previous = bytes;
                progress.phase(phase);
            },
        )
        .map_err(transfer_error)
}

fn transfer_error(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<FtpError>() == Some(&FtpError::Cancelled) {
        anyhow::anyhow!(SFTP_TRANSFER_CANCELLED)
    } else {
        error
    }
}
