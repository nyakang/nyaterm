//! Frozen external-edit targets; desktop owns launch, watching and approval.
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::file_browser::FileBrowserService;
use crate::ftp::{FtpError, FtpFileService};
use crate::remote_file::{RemoteTextMetadata, RemoteTextRevision};
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::{SftpTransferOptions, SftpTransferProgress, SftpTransferSummary};

pub struct ExternalEditTarget {
    service: FileBrowserService,
    path: RemoteFilePath,
    state: Mutex<EditState>,
    runtime: crate::ftp::resume::ResumeRuntime,
}
struct EditState {
    revision: Option<RemoteTextRevision>,
    conflict: bool,
    approved: bool,
}
impl std::fmt::Debug for ExternalEditTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExternalEditTarget { .. }")
    }
}

impl FileBrowserService {
    pub fn download_external_edit(
        &self,
        remote: &RemoteFilePath,
        local: &Path,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: impl FnMut(SftpTransferProgress) + Send + 'static,
    ) -> anyhow::Result<Arc<ExternalEditTarget>> {
        let runtime = crate::ftp::resume::ResumeRuntime::default();
        let revision = match self {
            Self::Remote(service) => {
                service.download_remote_file_with_progress_and_control_options(
                    remote, local, control, options, progress,
                )?;
                None
            }
            Self::Ftp(service) => {
                let before = service.stat_with_control(&remote.display_path, &control)?;
                runtime.download(
                    service,
                    &remote.display_path,
                    crate::download_path::root_for_target(local),
                    local,
                    false,
                    &control,
                    &options,
                    true,
                    reporter(remote, local, before.size, progress),
                )?;
                let revision = local_revision(service, remote, local, &control)?;
                anyhow::ensure!(
                    service.stat_with_control(&remote.display_path, &control)? == before,
                    FtpError::SourceChanged
                );
                Some(revision)
            }
            _ => anyhow::bail!("external editing is unavailable for this backend"),
        };
        Ok(Arc::new(ExternalEditTarget {
            service: self.clone(),
            path: remote.clone(),
            state: Mutex::new(EditState {
                revision,
                conflict: false,
                approved: false,
            }),
            runtime,
        }))
    }
}

impl ExternalEditTarget {
    pub fn has_conflict(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| state.conflict)
    }
    /// Called only by desktop after an explicit conflict confirmation.
    pub fn approve_conflict(&self) {
        if let Ok(mut state) = self.state.try_lock() {
            state.approved = state.conflict;
        }
    }
    pub fn upload(
        &self,
        local: &Path,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: impl FnMut(SftpTransferProgress) + Send + 'static,
    ) -> anyhow::Result<SftpTransferSummary> {
        let mut state = loop {
            control.check_cancelled()?;
            match self.state.try_lock() {
                Ok(state) => break state,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(_) => anyhow::bail!(FtpError::Unavailable),
            }
        };
        match &self.service {
            FileBrowserService::Remote(service) => service
                .upload_remote_edit_with_progress_and_control_options(
                    local.to_path_buf(),
                    &self.path,
                    control,
                    options,
                    progress,
                ),
            FileBrowserService::Ftp(service) => {
                let approved = std::mem::take(&mut state.approved);
                if state.conflict && !approved {
                    anyhow::bail!(FtpError::Conflict);
                }
                // Editors often replace the file on save. Upload an immutable sibling
                // snapshot so a later save cannot change bytes already in flight.
                let before = crate::local_revision::read_local_file_version(local, || {
                    control.is_cancelled()
                })?;
                let mut snapshot =
                    tempfile::NamedTempFile::new_in(local.parent().ok_or(FtpError::InvalidPath)?)?;
                let mut source = crate::download_path::open_without_following(local, false, false)?;
                use std::io::{Read as _, Write as _};
                let mut buffer = [0; 64 * 1024];
                loop {
                    control.check_cancelled()?;
                    let count = source.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    snapshot.write_all(&buffer[..count])?;
                }
                snapshot.flush()?;
                let version =
                    crate::local_revision::read_local_file_version(snapshot.path(), || {
                        control.is_cancelled()
                    })?;
                let after = crate::local_revision::read_local_file_version(local, || {
                    control.is_cancelled()
                })?;
                anyhow::ensure!(
                    before.digest == version.digest && before == after,
                    FtpError::SourceChanged
                );
                let result = self.runtime.upload(
                    service,
                    snapshot.path(),
                    &self.path.display_path,
                    true,
                    &control,
                    &options,
                    true,
                    if approved {
                        None
                    } else {
                        state.revision.as_ref()
                    },
                    reporter(&self.path, local, version.len, progress),
                );
                match result {
                    Ok(outcome) => {
                        let metadata =
                            service.stat_with_control(&self.path.display_path, &control)?;
                        state.revision = Some(RemoteTextRevision {
                            metadata: RemoteTextMetadata {
                                size: version.len,
                                modified_at: metadata
                                    .modified
                                    .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|v| v.as_secs()),
                            },
                            content_sha256: version.digest,
                        });
                        state.conflict = false;
                        Ok(SftpTransferSummary {
                            remote_path: self.path.display_path.clone(),
                            local_path: local.to_path_buf(),
                            bytes: outcome.bytes,
                            skipped: false,
                            retained_backups: outcome.backup_path.into_iter().collect(),
                        })
                    }
                    Err(error) => {
                        self.runtime.discard_upload(
                            service,
                            &self.path.display_path,
                            snapshot.path(),
                        );
                        if error.downcast_ref::<FtpError>() == Some(&FtpError::Conflict) {
                            state.conflict = true;
                        }
                        Err(error)
                    }
                }
            }
            _ => anyhow::bail!("external editing is unavailable for this backend"),
        }
    }
}

fn local_revision(
    service: &FtpFileService,
    path: &RemoteFilePath,
    local: &Path,
    control: &SftpTransferControl,
) -> anyhow::Result<RemoteTextRevision> {
    let version = crate::local_revision::read_local_file_version(local, || control.is_cancelled())?;
    let metadata = service.stat_with_control(&path.display_path, control)?;
    anyhow::ensure!(version.len == metadata.size, FtpError::SourceChanged);
    Ok(RemoteTextRevision {
        metadata: RemoteTextMetadata {
            size: version.len,
            modified_at: metadata
                .modified
                .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|v| v.as_secs()),
        },
        content_sha256: version.digest,
    })
}

fn reporter(
    remote: &RemoteFilePath,
    local: &Path,
    size: u64,
    mut progress: impl FnMut(SftpTransferProgress),
) -> impl FnMut(u64, crate::sftp_transfer_types::FileTransferPhase) {
    let remote = remote.display_path.clone();
    let local = local.to_path_buf();
    move |bytes, phase| {
        progress(SftpTransferProgress {
            phase: Some(phase),
            remote_path: remote.clone(),
            local_path: local.clone(),
            bytes_transferred: bytes,
            total_bytes: Some(size),
            item_count_completed: Some(0),
            item_count_total: Some(1),
        })
    }
}
