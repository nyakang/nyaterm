use super::{SftpTransferControl, await_sftp_request, is_sftp_transfer_cancelled};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The baseline belongs to the logical upload, not a connection or retry.
#[derive(Clone, Default)]
pub(super) struct UploadMetadata {
    originals: Arc<Mutex<HashMap<Vec<u8>, Option<FileAttributes>>>>,
}

impl UploadMetadata {
    pub(super) async fn capture(
        &self,
        session: &SftpSession,
        path: &[u8],
        control: &SftpTransferControl,
    ) -> anyhow::Result<Option<FileAttributes>> {
        if let Some(original) = self
            .originals
            .lock()
            .expect("upload metadata lock")
            .get(path)
            .cloned()
        {
            return Ok(original);
        }
        // stat deliberately follows links, just as the existing SFTP write does.
        let original =
            match await_sftp_request(control, session.metadata_bytes(path.to_vec())).await {
                Ok(attributes) => {
                    anyhow::ensure!(
                        attributes.permissions.is_some(),
                        "Upload refused: original permissions are unavailable"
                    );
                    anyhow::ensure!(
                        attributes.file_type() == FileType::File,
                        "Upload replacement requires a regular file"
                    );
                    Some(attributes)
                }
                Err(error)
                    if matches!(
                        error.downcast_ref::<russh_sftp::client::error::Error>(),
                        Some(russh_sftp::client::error::Error::Status(status))
                            if status.status_code == StatusCode::NoSuchFile
                    ) =>
                {
                    None
                }
                Err(error) if is_sftp_transfer_cancelled(&error) => return Err(error),
                Err(error) => {
                    return Err(error.context("Upload refused: could not read original attributes"));
                }
            };
        let mut originals = self.originals.lock().expect("upload metadata lock");
        Ok(originals.entry(path.to_vec()).or_insert(original).clone())
    }
}

pub(super) fn upload_file_mode(existing: Option<u32>, default: Option<u32>) -> Option<u32> {
    existing.map(|mode| mode & 0o7777).or(default)
}

pub(super) async fn finish_upload_metadata(
    session: &SftpSession,
    path: Vec<u8>,
    original: Option<&FileAttributes>,
    default_mode: Option<u32>,
    control: &SftpTransferControl,
) -> anyhow::Result<()> {
    let Some(mode) = upload_file_mode(original.and_then(|attrs| attrs.permissions), default_mode)
    else {
        return Ok(());
    };
    let result = async {
        let current = await_sftp_request(control, session.metadata_bytes(path.clone())).await?;
        let verified = if current.permissions.map(|mode| mode & 0o7777) != Some(mode) {
            await_sftp_request(
                control,
                session.set_metadata_bytes(
                    path.clone(),
                    FileAttributes {
                        permissions: Some(mode),
                        ..FileAttributes::empty()
                    },
                ),
            )
            .await?;
            await_sftp_request(control, session.metadata_bytes(path)).await?
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
