//! Checkpoints belong to one task, never to a pathname found on the server.
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::download_path::{DownloadTargetAccess, FileIdentity, TransferDownload};
use crate::ftp::service::{FtpWriteOutcome, RemoteReplacement, TransferMode, temporary_path};
use crate::ftp::{FtpEntry, FtpError, FtpFileService};
use crate::sftp::SftpTransferControl;
use crate::sftp_transfer_types::{FileTransferPhase, SftpTransferOptions};

#[derive(Default, Clone)]
pub(crate) struct ResumeRuntime(Arc<ResumeState>);

#[derive(Default)]
struct ResumeState {
    checkpoints: Mutex<HashMap<(String, PathBuf, bool), Checkpoint>>,
}

static CLEANUP: std::sync::OnceLock<std::sync::mpsc::Sender<Vec<Checkpoint>>> =
    std::sync::OnceLock::new();
fn prepare_cleanup() -> anyhow::Result<()> {
    if CLEANUP.get().is_none() {
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<Checkpoint>>();
        std::thread::Builder::new()
            .name("ftp-checkpoint-cleanup".into())
            .spawn(move || {
                for checkpoints in receiver {
                    for checkpoint in checkpoints {
                        if let Checkpoint::Upload {
                            temporary, pool, ..
                        } = checkpoint
                            && let Some(pool) = pool.upgrade()
                            && let Ok(connection) = pool.acquire(
                                false,
                                &SftpTransferControl::new(),
                                Some(Instant::now() + Duration::from_secs(2)),
                            )
                            && let Ok(mut lease) = crate::ftp::connection::Lease::new(
                                connection,
                                &SftpTransferControl::new(),
                                Some(Instant::now() + Duration::from_secs(2)),
                            )
                        {
                            let _ = lease.command(|stream| stream.rm(&temporary));
                        }
                    }
                }
            })?;
        let _ = CLEANUP.set(sender);
    }
    Ok(())
}
impl Drop for ResumeState {
    fn drop(&mut self) {
        let checkpoints = std::mem::take(
            self.checkpoints
                .get_mut()
                .unwrap_or_else(|e| e.into_inner()),
        );
        if !checkpoints.is_empty() {
            // Initialized by the worker before retaining any checkpoint.
            if let Some(sender) = CLEANUP.get() {
                let _ = sender.send(checkpoints.into_values().collect());
            }
        }
    }
}

enum Checkpoint {
    Download {
        source: FtpEntry,
        staged: TransferDownload,
        stamp: LocalStamp,
    },
    Upload {
        source: LocalStamp,
        temporary: String,
        pool: std::sync::Weak<crate::ftp::pool::Pool>,
    },
}

#[derive(PartialEq, Eq)]
struct LocalStamp {
    identity: FileIdentity,
    length: u64,
    modified: SystemTime,
}
impl LocalStamp {
    fn matches_path_and_file(&self, path: &Path, file: &File) -> anyhow::Result<bool> {
        let current = crate::download_path::open_without_following(path, false, false)?;
        Ok(Self::read(file)? == *self && Self::read(&current)? == *self)
    }

    fn read(file: &File) -> anyhow::Result<Self> {
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), FtpError::InvalidPath);
        Ok(Self {
            identity: FileIdentity::from_file(file)?,
            length: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

pub(crate) fn wait(
    service: &FtpFileService,
    control: &SftpTransferControl,
    delay: Duration,
) -> Result<(), FtpError> {
    let until = Instant::now() + delay;
    loop {
        if control.is_cancelled() {
            return Err(FtpError::Cancelled);
        }
        service.ensure_available()?;
        if !control.is_paused() && Instant::now() >= until {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn retry(
    error: &FtpError,
    attempts: &mut u32,
    options: &SftpTransferOptions,
    service: &FtpFileService,
    control: &SftpTransferControl,
) -> Result<bool, FtpError> {
    if *error == FtpError::Paused {
        wait(service, control, Duration::ZERO)?;
        return Ok(true);
    }
    if !error.is_retryable() || *attempts >= options.max_retries() {
        return Ok(false);
    }
    let delay = Duration::from_secs((1u64 << (*attempts).min(5)).min(30));
    *attempts += 1;
    wait(service, control, delay)?;
    Ok(true)
}

fn boundary(
    service: &FtpFileService,
    remote: &str,
    file: &mut File,
    offset: u64,
    control: &SftpTransferControl,
) -> Result<bool, FtpError> {
    let length = offset.min(64 * 1024);
    if length == 0 {
        return Ok(true);
    }
    let mut bytes = Vec::new();
    let result = service.retrieve_mode(
        remote,
        &mut bytes,
        Some(length),
        control,
        TransferMode {
            offset: offset - length,
            binary: true,
            range: true,
            ..Default::default()
        },
        |_| Ok(()),
    );
    if let Err(error) = result
        && error != FtpError::LimitExceeded
    {
        return Err(error);
    }
    let mut local = vec![0; length as usize];
    file.seek(SeekFrom::Start(offset - length))
        .map_err(crate::ftp::service::io_error)?;
    file.read_exact(&mut local)
        .map_err(crate::ftp::service::io_error)?;
    Ok(bytes == local)
}

impl ResumeRuntime {
    pub(crate) fn discard_upload(&self, service: &FtpFileService, remote: &str, local: &Path) {
        let checkpoint = self.0.checkpoints.lock().ok().and_then(|mut checkpoints| {
            checkpoints.remove(&(remote.to_owned(), local.to_path_buf(), true))
        });
        if let Some(Checkpoint::Upload { temporary, .. }) = checkpoint {
            let _ = service.remove_unlocked(&temporary);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn download(
        &self,
        service: &FtpFileService,
        remote: &str,
        root: &Path,
        local: &Path,
        replace: bool,
        control: &SftpTransferControl,
        options: &SftpTransferOptions,
        binary: bool,
        mut progress: impl FnMut(u64, FileTransferPhase),
    ) -> anyhow::Result<u64> {
        progress(0, FileTransferPhase::WaitingForConnection);
        wait(service, control, Duration::ZERO)?;
        prepare_cleanup()?;
        let key = (remote.to_owned(), local.to_path_buf(), false);
        let source = service.stat_with_control(remote, control)?;
        let checkpoint = self
            .0
            .checkpoints
            .lock()
            .map_err(|_| FtpError::Unavailable)?
            .remove(&key);
        let (staged, mut file) = match checkpoint {
            Some(Checkpoint::Download {
                source: previous,
                staged,
                stamp,
            }) => {
                anyhow::ensure!(previous == source, FtpError::SourceChanged);
                let file = staged.reopen()?;
                anyhow::ensure!(LocalStamp::read(&file)? == stamp, FtpError::SourceChanged);
                (staged, file)
            }
            _ => TransferDownload::prepare(
                root,
                local,
                0,
                if replace {
                    DownloadTargetAccess::capture(root, local)?
                } else {
                    DownloadTargetAccess::CreateNew
                },
            )?,
        };
        let binary = binary || !service.ascii_mode();
        let mut resume = options.resume_broken_transfer && binary && source.modified.is_some();
        let mut attempts = 0;
        let result = loop {
            let attempt = (|| -> Result<u64, FtpError> {
                wait(service, control, Duration::ZERO)?;
                if service.stat_once(remote, control)? != source {
                    return Err(FtpError::SourceChanged);
                }
                let mut offset = file
                    .metadata()
                    .map_err(crate::ftp::service::io_error)?
                    .len();
                if !resume || offset > source.size {
                    offset = 0;
                }
                if offset > 0 && !boundary(service, remote, &mut file, offset, control)? {
                    return Err(FtpError::SourceChanged);
                }
                file.set_len(offset)
                    .map_err(crate::ftp::service::io_error)?;
                file.seek(SeekFrom::Start(offset))
                    .map_err(crate::ftp::service::io_error)?;
                progress(
                    offset,
                    if offset > 0 {
                        FileTransferPhase::Resuming
                    } else {
                        FileTransferPhase::WaitingForConnection
                    },
                );
                let mut done = offset;
                service.retrieve_mode(
                    remote,
                    &mut file,
                    None,
                    control,
                    TransferMode {
                        offset,
                        binary,
                        checkpoint_pause: resume,
                        ..Default::default()
                    },
                    |bytes| {
                        done += bytes as u64;
                        progress(done, FileTransferPhase::Transferring);
                        Ok(())
                    },
                )?;
                if service.stat_once(remote, control)? != source || (binary && done != source.size)
                {
                    return Err(FtpError::SourceChanged);
                }
                file.sync_all().map_err(crate::ftp::service::io_error)?;
                Ok(done)
            })();
            match attempt {
                Err(FtpError::RestartUnsupported) if resume => {
                    resume = false;
                    progress(0, FileTransferPhase::Restarting);
                }
                Err(error) => {
                    if error.is_retryable() {
                        progress(
                            file.metadata().map(|m| m.len()).unwrap_or(0),
                            FileTransferPhase::Reconnecting,
                        );
                    }
                    match retry(&error, &mut attempts, options, service, control) {
                        Ok(true) => continue,
                        Ok(false) => break Err(error),
                        Err(error) => break Err(error),
                    }
                }
                Ok(bytes) => break Ok(bytes),
            }
        };
        match result {
            Ok(bytes) => {
                control.check_cancelled()?;
                drop(file);
                staged.commit(root, local)?;
                Ok(bytes)
            }
            Err(error) => {
                if error.is_retryable() && resume && !control.is_cancelled() {
                    self.0
                        .checkpoints
                        .lock()
                        .map_err(|_| FtpError::Unavailable)?
                        .insert(
                            key,
                            Checkpoint::Download {
                                source,
                                staged,
                                stamp: LocalStamp::read(&file)?,
                            },
                        );
                }
                Err(error.into())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upload(
        &self,
        service: &FtpFileService,
        local: &Path,
        remote: &str,
        replace: bool,
        control: &SftpTransferControl,
        options: &SftpTransferOptions,
        binary: bool,
        expected: Option<&crate::remote_file::RemoteTextRevision>,
        mut progress: impl FnMut(u64, FileTransferPhase),
    ) -> anyhow::Result<FtpWriteOutcome> {
        progress(0, FileTransferPhase::WaitingForConnection);
        wait(service, control, Duration::ZERO)?;
        if let Some(expected) = expected {
            anyhow::ensure!(
                revision_matches(service, remote, expected, control)?,
                FtpError::Conflict
            );
        }
        let mut file = crate::download_path::open_without_following(local, false, false)?;
        let source = LocalStamp::read(&file)?;
        prepare_cleanup()?;
        let key = (remote.to_owned(), local.to_path_buf(), true);
        let checkpoint = self
            .0
            .checkpoints
            .lock()
            .map_err(|_| FtpError::Unavailable)?
            .remove(&key);
        let temporary = match checkpoint {
            Some(Checkpoint::Upload {
                source: previous,
                temporary,
                ..
            }) => {
                if previous != source {
                    return Err(cleanup_failed_upload(
                        service,
                        &temporary,
                        FtpError::SourceChanged.into(),
                    ));
                }
                temporary
            }
            _ => temporary_path(remote),
        };
        let binary = binary || !service.ascii_mode();
        let mut resume = options.resume_broken_transfer && binary;
        let mut attempts = 0;
        let result = loop {
            let attempt = (|| -> anyhow::Result<u64> {
                wait(service, control, Duration::ZERO)?;
                anyhow::ensure!(
                    source.matches_path_and_file(local, &file)?,
                    FtpError::SourceChanged
                );
                let mut offset = match service.stat_once(&temporary, control) {
                    Ok(entry) if resume && entry.size <= source.length => entry.size,
                    Ok(_) | Err(FtpError::NotFound) => 0,
                    Err(error) => return Err(error.into()),
                };
                if offset > 0 && !boundary(service, &temporary, &mut file, offset, control)? {
                    offset = 0;
                }
                file.seek(SeekFrom::Start(offset))?;
                progress(
                    offset,
                    if offset > 0 {
                        FileTransferPhase::Resuming
                    } else {
                        FileTransferPhase::WaitingForConnection
                    },
                );
                let mut done = offset;
                service.store_mode(
                    &temporary,
                    &mut file,
                    control,
                    TransferMode {
                        offset,
                        binary,
                        checkpoint_pause: resume,
                        ..Default::default()
                    },
                    |bytes| {
                        done += bytes as u64;
                        progress(done, FileTransferPhase::Transferring);
                        Ok(())
                    },
                )?;
                anyhow::ensure!(
                    source.matches_path_and_file(local, &file)? && done == source.length,
                    FtpError::SourceChanged
                );
                Ok(done)
            })();
            match attempt {
                Err(error)
                    if error.downcast_ref::<FtpError>() == Some(&FtpError::RestartUnsupported)
                        && resume =>
                {
                    resume = false;
                    progress(0, FileTransferPhase::Restarting);
                }
                Err(error) => {
                    if let Some(ftp) = error.downcast_ref::<FtpError>() {
                        if ftp.is_retryable() {
                            progress(0, FileTransferPhase::Reconnecting);
                        }
                        match retry(ftp, &mut attempts, options, service, control) {
                            Ok(true) => continue,
                            Err(error) => break Err(error.into()),
                            Ok(false) => {}
                        }
                    }
                    break Err(error);
                }
                Ok(bytes) => break Ok(bytes),
            }
        };
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                if error
                    .downcast_ref::<FtpError>()
                    .is_some_and(FtpError::is_retryable)
                    && resume
                    && !control.is_cancelled()
                {
                    self.0
                        .checkpoints
                        .lock()
                        .map_err(|_| FtpError::Unavailable)?
                        .insert(
                            key,
                            Checkpoint::Upload {
                                source,
                                temporary: temporary.clone(),
                                pool: Arc::downgrade(&service.pool),
                            },
                        );
                    // Surface the owned temporary even while a manual retry can reuse
                    // it: the server may be unreachable when the task is removed.
                    return Err(FtpError::RecoveryRequired {
                        temporary_path: temporary,
                        backup_path: None,
                    }
                    .into());
                }
                return Err(cleanup_failed_upload(service, &temporary, error));
            }
        };
        if control.is_cancelled() {
            return Err(cleanup_failed_upload(
                service,
                &temporary,
                FtpError::Cancelled.into(),
            ));
        }
        progress(bytes, FileTransferPhase::Committing);
        let commit = (|| -> Result<Option<String>, FtpError> {
            let _mutation = service.lock_mutations(control)?;
            if let Some(expected) = expected
                && !revision_matches(service, remote, expected, control)?
            {
                return Err(FtpError::Conflict);
            }
            // An ambiguous commit is never retried or cleaned up.
            crate::ftp::replace::commit(&RemoteReplacement(service), &temporary, remote, replace)
        })();
        let backup_path = match commit {
            Ok(backup) => backup,
            Err(error @ FtpError::RecoveryRequired { .. }) => return Err(error.into()),
            Err(error) => return Err(cleanup_failed_upload(service, &temporary, error.into())),
        };
        Ok(FtpWriteOutcome { bytes, backup_path })
    }
}

fn revision_matches(
    service: &FtpFileService,
    path: &str,
    expected: &crate::remote_file::RemoteTextRevision,
    control: &SftpTransferControl,
) -> Result<bool, FtpError> {
    match service.binary_revision(path, control) {
        Ok(revision) => Ok(revision == *expected),
        Err(FtpError::NotFound) => Ok(false),
        Err(error) => Err(error),
    }
}

fn cleanup_failed_upload(
    service: &FtpFileService,
    temporary: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    match service.remove_unlocked(temporary) {
        Ok(()) | Err(FtpError::NotFound) => error,
        Err(_) => FtpError::RecoveryRequired {
            temporary_path: temporary.to_owned(),
            backup_path: None,
        }
        .into(),
    }
}
