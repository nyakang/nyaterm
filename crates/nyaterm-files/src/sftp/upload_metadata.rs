use super::{SftpTransferControl, await_sftp_request, is_sftp_transfer_cancelled};
use russh_sftp::client::fs::File;
use russh_sftp::protocol::FileAttributes;

pub(super) fn upload_file_mode(existing: Option<u32>, default: Option<u32>) -> Option<u32> {
    existing.map(|mode| mode & 0o7777).or(default)
}

pub(super) async fn finish_upload_metadata(
    file: &File,
    original: Option<&FileAttributes>,
    default_mode: Option<u32>,
    control: &SftpTransferControl,
) -> anyhow::Result<()> {
    let Some(mode) = upload_file_mode(original.and_then(|attrs| attrs.permissions), default_mode)
    else {
        return Ok(());
    };
    let result = async {
        let current = await_sftp_request(control, file.metadata()).await?;
        let verified = if current.permissions.map(|mode| mode & 0o7777) != Some(mode) {
            await_sftp_request(
                control,
                file.set_metadata(FileAttributes {
                    permissions: Some(mode),
                    ..FileAttributes::empty()
                }),
            )
            .await?;
            await_sftp_request(control, file.metadata()).await?
        } else {
            current
        };
        anyhow::ensure!(
            verified.permissions.map(|mode| mode & 0o7777) == Some(mode),
            "remote permissions verification failed"
        );
        if let Some(original) = original {
            anyhow::ensure!(
                original.uid.is_none_or(|uid| verified.uid == Some(uid))
                    && original.gid.is_none_or(|gid| verified.gid == Some(gid)),
                "remote ownership verification failed"
            );
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.map_err(|error| {
        if is_sftp_transfer_cancelled(&error) {
            return error;
        }
        error.context("Upload attributes could not be restored or verified; remote contents may already have changed")
    })
}
