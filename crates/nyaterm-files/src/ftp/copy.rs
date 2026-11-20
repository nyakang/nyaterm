//! Leaf transfers for the shared bounded directory-copy engine.
use std::path::Path;

use crate::download_path::{DownloadTargetAccess, TransferDownload};
use crate::ftp::{FtpError, FtpFileService};
use crate::remote_file::copy_progress::CopyProgress;
use crate::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};

pub(crate) fn download(
    service: &FtpFileService,
    remote: &str,
    root: &Path,
    local: &Path,
    replace: bool,
    control: &SftpTransferControl,
    progress: &CopyProgress,
) -> anyhow::Result<u64> {
    control.check_cancelled()?;
    let access = if replace {
        DownloadTargetAccess::capture(root, local)?
    } else {
        DownloadTargetAccess::CreateNew
    };
    let (temporary, mut file) = TransferDownload::prepare(root, local, 0, access)?;
    let bytes = service
        .retrieve(remote, &mut file, None, control, |bytes| {
            progress.add_bytes(bytes as u64);
            Ok(())
        })
        .map_err(transfer_error)?;
    control.wait_if_paused_blocking()?;
    file.sync_all()?;
    drop(file);
    control.check_cancelled()?;
    temporary.commit(root, local)?;
    Ok(bytes)
}

pub(crate) fn upload(
    service: &FtpFileService,
    local: &Path,
    remote: &str,
    replace: bool,
    control: &SftpTransferControl,
    progress: &CopyProgress,
) -> anyhow::Result<crate::ftp::service::FtpWriteOutcome> {
    control.check_cancelled()?;
    let mut file = crate::download_path::open_without_following(local, false, false)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        crate::file_browser::FileTransferEntryError::UnsupportedFileType
    );
    let outcome = service
        .write_from_reader(remote, replace, &mut file, control, |bytes| {
            progress.add_bytes(bytes as u64);
            Ok(())
        })
        .map_err(transfer_error)?;
    Ok(outcome)
}

fn transfer_error(error: FtpError) -> anyhow::Error {
    if error == FtpError::Cancelled {
        anyhow::anyhow!(SFTP_TRANSFER_CANCELLED)
    } else {
        anyhow::Error::from(error)
    }
}
