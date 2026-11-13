//! Upload ownership is task-local and survives channel replacement and manual retries.
//! SFTP v3 has no stable file ID: these snapshots detect observable changes, not
//! replacements with identical attributes or every concurrent namespace race.

use super::{SftpTransferControl, await_sftp_request};
use crate::sftp_transfer_types::SftpDuplicateCacheKey;
use russh_sftp::client::{SftpSession, fs::File};
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
#[error(
    "Upload retry refused: remote target state could not be verified; start a new upload to resolve the destination again"
)]
pub(super) struct UploadRetryUnsafe;

#[derive(Clone, Default)]
pub(crate) struct UploadRuntime {
    state: Arc<Mutex<UploadState>>,
}

#[derive(Default)]
struct UploadState {
    selected: HashMap<SftpDuplicateCacheKey, Option<String>>,
    entries: HashMap<Vec<u8>, UploadEntry>,
    blocked: bool,
}

#[derive(Clone)]
struct UploadEntry {
    original: Option<FileAttributes>,
    directory: bool,
    planned: bool,
    // Directory size and mtime may change as this task adds children. Keep the
    // pre-attempt identity separately until all workers have stopped.
    directory_identity: Option<RemoteSnapshot>,
    checkpoint: Option<RemoteSnapshot>,
}

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint {
    size: Option<u64>,
    uid: Option<u32>,
    gid: Option<u32>,
    permissions: Option<u32>,
    mtime: Option<u32>,
}

impl From<&FileAttributes> for Fingerprint {
    fn from(attrs: &FileAttributes) -> Self {
        Self {
            size: attrs.size,
            uid: attrs.uid,
            gid: attrs.gid,
            permissions: attrs.permissions,
            mtime: attrs.mtime,
        }
    }
}

#[derive(Clone)]
pub(super) struct RemoteSnapshot {
    attrs: FileAttributes,
    link: Option<(Fingerprint, Vec<u8>)>,
}

impl RemoteSnapshot {
    fn matches(&self, other: &Self) -> bool {
        Fingerprint::from(&self.attrs) == Fingerprint::from(&other.attrs) && self.link == other.link
    }

    fn complete(&self) -> bool {
        let complete = |attrs: &Fingerprint| {
            attrs.size.is_some() && attrs.permissions.is_some() && attrs.mtime.is_some()
        };
        complete(&Fingerprint::from(&self.attrs))
            && self.link.as_ref().is_none_or(|(attrs, _)| complete(attrs))
    }

    fn same_directory(&self, other: &Self) -> bool {
        self.attrs.file_type() == FileType::Dir
            && other.attrs.file_type() == FileType::Dir
            && self.attrs.permissions == other.attrs.permissions
            && self.attrs.uid == other.attrs.uid
            && self.attrs.gid == other.attrs.gid
            && self.link == other.link
    }
}

impl UploadRuntime {
    pub(super) fn selected(&self, key: &SftpDuplicateCacheKey) -> Option<Option<String>> {
        self.state
            .lock()
            .expect("upload state lock")
            .selected
            .get(key)
            .cloned()
    }

    pub(super) fn select(&self, key: SftpDuplicateCacheKey, target: Option<String>) {
        self.state
            .lock()
            .expect("upload state lock")
            .selected
            .insert(key, target);
    }

    pub(super) fn block(&self) {
        self.state.lock().expect("upload state lock").blocked = true;
    }

    pub(super) fn retry_ready(&self) -> bool {
        let state = self.state.lock().expect("upload state lock");
        !state.blocked
            && state
                .entries
                .values()
                .all(|entry| entry.planned || entry.checkpoint.is_some())
    }

    /// Validate the entire previous attempt before resolving paths or mutating
    /// any child. An interrupted request never becomes owned just by reconnecting.
    pub(super) async fn validate(
        &self,
        session: &SftpSession,
        control: &SftpTransferControl,
    ) -> anyhow::Result<()> {
        let entries = {
            let state = self.state.lock().expect("upload state lock");
            anyhow::ensure!(!state.blocked, UploadRetryUnsafe);
            state.entries.clone()
        };
        let result = async {
            for (path, entry) in entries {
                if entry.planned && entry.checkpoint.is_none() {
                    anyhow::ensure!(
                        read_snapshot(session, &path, control).await?.is_none(),
                        UploadRetryUnsafe
                    );
                    continue;
                }
                let expected = entry.checkpoint.ok_or(UploadRetryUnsafe)?;
                anyhow::ensure!(expected.complete(), UploadRetryUnsafe);
                let current = read_snapshot(session, &path, control)
                    .await?
                    .ok_or(UploadRetryUnsafe)?;
                anyhow::ensure!(expected.matches(&current), UploadRetryUnsafe);
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.block();
            return Err(error.context(UploadRetryUnsafe));
        }
        Ok(())
    }

    pub(super) async fn remember_target(
        &self,
        session: &SftpSession,
        path: Vec<u8>,
        directory: bool,
        missing: bool,
        control: &SftpTransferControl,
    ) -> anyhow::Result<()> {
        let current = read_snapshot(session, &path, control).await?;
        if directory && let Some(current) = &current {
            anyhow::ensure!(
                current.attrs.file_type() == FileType::Dir,
                "Upload target is not a directory"
            );
        }
        anyhow::ensure!(!missing || current.is_none(), UploadRetryUnsafe);
        let mut state = self.state.lock().expect("upload state lock");
        state.entries.entry(path).or_insert_with(|| UploadEntry {
            original: current
                .as_ref()
                .filter(|_| !directory)
                .map(|snapshot| snapshot.attrs.clone()),
            directory,
            planned: true,
            directory_identity: directory.then(|| current.clone()).flatten(),
            checkpoint: current,
        });
        Ok(())
    }

    /// Called before OPEN, including exclusive creation. None stays unsafe until
    /// all writes and the final CLOSE have definite responses and a checkpoint.
    pub(super) async fn begin_file(
        &self,
        session: &SftpSession,
        path: &[u8],
        control: &SftpTransferControl,
    ) -> anyhow::Result<(Option<FileAttributes>, Option<RemoteSnapshot>)> {
        let current = read_snapshot(session, path, control).await?;
        if let Some(current) = &current {
            anyhow::ensure!(
                current.attrs.permissions.is_some(),
                "Upload refused: original permissions are unavailable"
            );
            anyhow::ensure!(
                current.attrs.file_type() == FileType::File,
                "Upload replacement requires a regular file"
            );
        }
        let mut state = self.state.lock().expect("upload state lock");
        anyhow::ensure!(!state.blocked, UploadRetryUnsafe);
        let entry = state
            .entries
            .entry(path.to_vec())
            .or_insert_with(|| UploadEntry {
                original: current.as_ref().map(|snapshot| snapshot.attrs.clone()),
                directory: false,
                planned: true,
                directory_identity: None,
                checkpoint: current.clone(),
            });
        anyhow::ensure!(!entry.directory, UploadRetryUnsafe);
        if let Some(expected) = &entry.checkpoint {
            anyhow::ensure!(
                current
                    .as_ref()
                    .is_some_and(|current| expected.matches(current)),
                UploadRetryUnsafe
            );
        } else {
            // Only a genuinely new entry is allowed to start without a snapshot.
            // A previous uncertain OPEN/WRITE must not be reclassified as new.
            anyhow::ensure!(entry.planned && current.is_none(), UploadRetryUnsafe);
        }
        let original = entry.original.clone();
        entry.checkpoint = None;
        entry.planned = false;
        Ok((original, current))
    }

    pub(super) async fn verify_handle(
        &self,
        session: &SftpSession,
        file: &File,
        path: &[u8],
        before: Option<&RemoteSnapshot>,
        control: &SftpTransferControl,
    ) -> anyhow::Result<RemoteSnapshot> {
        let current = read_snapshot(session, path, control)
            .await?
            .ok_or(UploadRetryUnsafe)?;
        let handle = await_sftp_request(control, file.metadata()).await?;
        anyhow::ensure!(handle.file_type() == FileType::File, UploadRetryUnsafe);
        anyhow::ensure!(
            Fingerprint::from(&handle) == Fingerprint::from(&current.attrs),
            UploadRetryUnsafe
        );
        if let Some(before) = before {
            anyhow::ensure!(before.matches(&current), UploadRetryUnsafe);
        } else {
            anyhow::ensure!(current.link.is_none(), UploadRetryUnsafe);
        }
        Ok(current)
    }

    pub(super) async fn file_checkpoint(
        &self,
        session: &SftpSession,
        file: &File,
        path: &[u8],
        opened: &RemoteSnapshot,
        control: &SftpTransferControl,
    ) -> anyhow::Result<RemoteSnapshot> {
        let current = read_snapshot(session, path, control)
            .await?
            .ok_or(UploadRetryUnsafe)?;
        let handle = await_sftp_request(control, file.metadata()).await?;
        anyhow::ensure!(handle.file_type() == FileType::File, UploadRetryUnsafe);
        anyhow::ensure!(
            Fingerprint::from(&handle) == Fingerprint::from(&current.attrs),
            UploadRetryUnsafe
        );
        anyhow::ensure!(current.link == opened.link, UploadRetryUnsafe);
        // Content writes and mode restoration never change ownership, including
        // on a newly created file which has no pre-upload permission baseline.
        anyhow::ensure!(
            current.attrs.uid == opened.attrs.uid && current.attrs.gid == opened.attrs.gid,
            UploadRetryUnsafe
        );
        Ok(current)
    }

    pub(super) fn finish_file(&self, path: &[u8], snapshot: RemoteSnapshot) {
        self.state
            .lock()
            .expect("upload state lock")
            .entries
            .get_mut(path)
            .expect("opened upload entry")
            .checkpoint = Some(snapshot);
    }

    pub(super) async fn ensure_directory(
        &self,
        session: &SftpSession,
        path: Vec<u8>,
        control: &SftpTransferControl,
    ) -> anyhow::Result<()> {
        let current = read_snapshot(session, &path, control).await?;
        if let Some(current) = &current {
            anyhow::ensure!(
                current.attrs.file_type() == FileType::Dir,
                "Upload target is not a directory"
            );
        }
        {
            let mut state = self.state.lock().expect("upload state lock");
            anyhow::ensure!(!state.blocked, UploadRetryUnsafe);
            if let Some(entry) = state.entries.get_mut(&path) {
                anyhow::ensure!(entry.directory, UploadRetryUnsafe);
                if let Some(expected) = &entry.checkpoint {
                    anyhow::ensure!(
                        current
                            .as_ref()
                            .is_some_and(|current| expected.matches(current)),
                        UploadRetryUnsafe
                    );
                } else {
                    anyhow::ensure!(entry.planned && current.is_none(), UploadRetryUnsafe);
                }
                entry.checkpoint = None;
                entry.planned = false;
            } else {
                state.entries.insert(
                    path.clone(),
                    UploadEntry {
                        original: None,
                        directory: true,
                        planned: false,
                        directory_identity: current.clone(),
                        checkpoint: None,
                    },
                );
            }
        }
        if current.is_none() {
            await_sftp_request(control, session.create_dir_bytes(path.clone())).await?;
            let created = read_snapshot(session, &path, control)
                .await?
                .ok_or(UploadRetryUnsafe)?;
            anyhow::ensure!(
                created.attrs.file_type() == FileType::Dir && created.link.is_none(),
                UploadRetryUnsafe
            );
            self.state
                .lock()
                .expect("upload state lock")
                .entries
                .get_mut(&path)
                .expect("created upload directory")
                .directory_identity = Some(created);
        }
        Ok(())
    }

    /// Workers must have finished before this runs; only our own child writes may
    /// explain directory size/mtime changes. Never refresh a file by path alone.
    pub(super) async fn finish_directories(
        &self,
        session: &SftpSession,
        control: &SftpTransferControl,
    ) -> anyhow::Result<()> {
        let entries = self
            .state
            .lock()
            .expect("upload state lock")
            .entries
            .clone();
        for (path, entry) in entries.into_iter().filter(|(_, entry)| entry.directory) {
            let expected = entry.directory_identity.ok_or(UploadRetryUnsafe)?;
            let current = read_snapshot(session, &path, control)
                .await?
                .ok_or(UploadRetryUnsafe)?;
            anyhow::ensure!(expected.same_directory(&current), UploadRetryUnsafe);
            let mut state = self.state.lock().expect("upload state lock");
            let entry = state.entries.get_mut(&path).expect("upload directory");
            entry.directory_identity = Some(current.clone());
            entry.checkpoint = Some(current);
        }
        Ok(())
    }
}

async fn read_snapshot(
    session: &SftpSession,
    path: &[u8],
    control: &SftpTransferControl,
) -> anyhow::Result<Option<RemoteSnapshot>> {
    let attrs = match await_sftp_request(control, session.symlink_metadata_bytes(path.to_vec()))
        .await
    {
        Ok(attrs) => attrs,
        Err(error) if matches!(error.downcast_ref::<russh_sftp::client::error::Error>(), Some(russh_sftp::client::error::Error::Status(status)) if status.status_code == StatusCode::NoSuchFile) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if attrs.file_type() == FileType::Symlink {
        let link = await_sftp_request(control, session.read_link_bytes(path.to_vec())).await?;
        let target = await_sftp_request(control, session.metadata_bytes(path.to_vec())).await?;
        Ok(Some(RemoteSnapshot {
            attrs: target,
            link: Some((Fingerprint::from(&attrs), link)),
        }))
    } else {
        Ok(Some(RemoteSnapshot { attrs, link: None }))
    }
}

/// The high-level client preserves transport errors inside io::Error, but turns
/// received WRITE status failures into plain Other errors. Only the latter have
/// a definite response; a timeout must never be forgotten by a later flush.
pub(super) fn confirmed_failure(error: &anyhow::Error) -> bool {
    if let Some(error) = error.downcast_ref::<russh_sftp::client::error::Error>() {
        return matches!(error, russh_sftp::client::error::Error::Status(_));
    }
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        error.kind() == std::io::ErrorKind::Other
            && error.get_ref().is_some_and(|source| {
                match source.downcast_ref::<russh_sftp::client::error::Error>() {
                    Some(russh_sftp::client::error::Error::Status(_)) => true,
                    Some(_) => false,
                    // check_write_result erases the status type in the pinned
                    // client. Its other plain error denotes an invalid packet.
                    None => error.to_string() != "unexpected response packet",
                }
            })
    })
}
