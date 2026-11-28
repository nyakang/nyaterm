//! Task-owned destinations, shared only by retries of the same download.

use crate::download_path::{
    DownloadTargetAccess, FileIdentity, ensure_no_symlink, root_for_target,
};
use crate::sftp_transfer_types::SftpDuplicateCacheKey;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileStamp {
    identity: FileIdentity,
    length: u64,
    modified: SystemTime,
}

impl FileStamp {
    pub(crate) fn read(path: &Path) -> anyhow::Result<Self> {
        let file = crate::download_path::open_without_following(path, false, false)?;
        Self::from_file(&file)
    }

    pub(crate) fn from_file(file: &fs::File) -> anyhow::Result<Self> {
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "download target is not a regular file");
        Ok(Self {
            identity: FileIdentity::from_file(file)?,
            length: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DownloadSource {
    // Non-SFTP callers retain their existing task-scoped source contract.
    pub(crate) sftp_service_id: Option<u64>,
    pub(crate) path: Vec<u8>,
    pub(crate) size: Option<u64>,
    pub(crate) modified: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DownloadState {
    Partial,
    Complete,
    Invalidated,
}

struct DownloadFile {
    local: FileStamp,
    source: DownloadSource,
    state: DownloadState,
}

#[derive(Default)]
pub(crate) struct DownloadRuntime {
    selected: HashMap<SftpDuplicateCacheKey, PathBuf>,
    access: HashMap<PathBuf, DownloadTargetAccess>,
    directories: HashMap<PathBuf, FileIdentity>,
    files: HashMap<PathBuf, DownloadFile>,
}

impl DownloadRuntime {
    pub(crate) fn track_directory(&mut self, path: &Path) -> anyhow::Result<()> {
        self.validate_ancestors(path)?;
        let identity = FileIdentity::from_path(path)?;
        if let Some(previous) = self.directories.get(path) {
            anyhow::ensure!(*previous == identity, "download directory was replaced");
        } else {
            self.directories.insert(path.to_path_buf(), identity);
        }
        Ok(())
    }

    pub(crate) fn selected(&self, key: &SftpDuplicateCacheKey) -> Option<PathBuf> {
        self.selected.get(key).cloned()
    }

    pub(crate) fn select(
        &mut self,
        key: SftpDuplicateCacheKey,
        path: &Path,
        access: Option<DownloadTargetAccess>,
    ) {
        self.selected.insert(key, path.to_path_buf());
        if let Some(access) = access {
            self.access.entry(path.to_path_buf()).or_insert(access);
        }
    }

    pub(crate) fn validate_ancestors(&self, path: &Path) -> anyhow::Result<()> {
        for ancestor in path.ancestors().skip(1) {
            if let Some(identity) = self.directories.get(ancestor) {
                anyhow::ensure!(
                    FileIdentity::from_path(ancestor)? == *identity,
                    "task-owned download directory was replaced"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn access(
        &self,
        path: &Path,
        promised: bool,
    ) -> anyhow::Result<DownloadTargetAccess> {
        self.validate_ancestors(path)?;
        if let Some(file) = self.files.get(path) {
            anyhow::ensure!(
                FileStamp::read(path)? == file.local,
                "task-owned download file was modified or replaced"
            );
            return Ok(DownloadTargetAccess::ReplaceKnownTarget(
                file.local.identity.clone(),
                Some((file.local.length, file.local.modified)),
            ));
        }
        if promised {
            return Ok(DownloadTargetAccess::CreateNew);
        }
        self.access.get(path).cloned().map_or_else(
            || DownloadTargetAccess::capture(root_for_target(path), path),
            Ok,
        )
    }

    pub(crate) fn ensure_directory(
        &mut self,
        root: &Path,
        path: &Path,
        promised: bool,
    ) -> anyhow::Result<()> {
        ensure_no_symlink(root, path)?;
        if let Some(identity) = self.directories.get(path) {
            if path.exists() {
                anyhow::ensure!(
                    FileIdentity::from_path(path)? == *identity,
                    "task-owned download directory was replaced"
                );
                anyhow::ensure!(
                    fs::symlink_metadata(path)?.is_dir(),
                    "download target is not a directory"
                );
                return Ok(());
            }
            self.directories.remove(path);
        }
        if !promised && path.is_dir() {
            return Ok(());
        }
        if !promised && let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            ensure_no_symlink(root, path)?;
        }
        // create_dir, rather than create_dir_all, must not adopt a racing target.
        fs::create_dir(path)?;
        ensure_no_symlink(root, path)?;
        self.directories
            .insert(path.to_path_buf(), FileIdentity::from_path(path)?);
        Ok(())
    }

    pub(crate) fn owns(&self, path: &Path, is_directory: bool) -> anyhow::Result<bool> {
        if is_directory {
            if let Some(identity) = self.directories.get(path) {
                anyhow::ensure!(
                    FileIdentity::from_path(path)? == *identity,
                    "task-owned download directory was replaced"
                );
                return Ok(true);
            }
        } else if let Some(completed) = self.files.get(path) {
            anyhow::ensure!(
                FileStamp::read(path)? == completed.local,
                "task-owned download file was modified or replaced"
            );
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn reusable(&self, path: &Path, source: &DownloadSource) -> anyhow::Result<bool> {
        let Some(file) = self.files.get(path) else {
            return Ok(false);
        };
        anyhow::ensure!(
            FileStamp::read(path)? == file.local,
            "task-owned download file was modified or replaced"
        );
        // Missing source timestamps are not sufficient to identify a remote revision.
        Ok(file.state == DownloadState::Complete
            && source.modified.is_some()
            && source.size == Some(file.local.length)
            && file.source == *source)
    }

    pub(crate) fn resume_offset(
        &mut self,
        path: &Path,
        source: &DownloadSource,
    ) -> anyhow::Result<u64> {
        let Some(file) = self.files.get_mut(path) else {
            return Ok(0);
        };
        anyhow::ensure!(
            FileStamp::read(path)? == file.local,
            "task-owned download file was modified or replaced"
        );
        if file.state != DownloadState::Partial {
            return Ok(0);
        }
        // This is an ownership and metadata check, not a content revision proof.
        if source.sftp_service_id.is_none()
            || source.modified.is_none()
            || source
                .size
                .is_none_or(|size| file.local.length == 0 || file.local.length >= size)
            || file.source != *source
        {
            file.state = DownloadState::Invalidated;
            return Ok(0);
        }
        Ok(file.local.length)
    }

    pub(crate) fn invalidate(&mut self, path: &Path) {
        if let Some(file) = self.files.get_mut(path) {
            file.state = DownloadState::Invalidated;
        }
    }

    pub(crate) fn partial(
        &mut self,
        path: &Path,
        local: FileStamp,
        source: DownloadSource,
    ) -> anyhow::Result<()> {
        self.record(path, local, source, DownloadState::Partial)
    }

    pub(crate) fn completed(
        &mut self,
        path: &Path,
        local: FileStamp,
        source: DownloadSource,
    ) -> anyhow::Result<()> {
        self.record(path, local, source, DownloadState::Complete)
    }

    fn record(
        &mut self,
        path: &Path,
        local: FileStamp,
        source: DownloadSource,
        state: DownloadState,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            FileStamp::read(path)? == local,
            "download target was modified or replaced after commit"
        );
        // The file record now owns authorization; do not mirror its identity in access.
        self.access.remove(path);
        self.files.insert(
            path.to_path_buf(),
            DownloadFile {
                local,
                source,
                state,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadRuntime, DownloadSource, FileStamp};

    #[test]
    fn promised_directories_reuse_only_task_owned_identity_and_reject_external_entries()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("folder");
        let mut runtime = DownloadRuntime::default();
        runtime.ensure_directory(root.path(), &target, true)?;
        runtime.ensure_directory(root.path(), &target, true)?;
        std::fs::rename(&target, root.path().join("previous"))?;
        std::fs::create_dir(&target)?;
        assert!(
            runtime
                .ensure_directory(root.path(), &target, true)
                .is_err()
        );
        assert!(runtime.access(&target.join("child"), true).is_err());
        assert!(
            DownloadRuntime::default()
                .ensure_directory(root.path(), &target, true)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn completed_files_require_matching_local_stamp_and_remote_revision() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        std::fs::write(&target, b"completed")?;
        let mut runtime = DownloadRuntime::default();
        let source = DownloadSource {
            sftp_service_id: Some(1),
            path: b"/file".to_vec(),
            size: Some(9),
            modified: Some(42),
        };
        runtime.completed(&target, FileStamp::read(&target)?, source.clone())?;
        assert!(runtime.reusable(&target, &source)?);
        assert!(!runtime.reusable(
            &target,
            &DownloadSource {
                modified: Some(43),
                ..source.clone()
            }
        )?);
        assert!(!runtime.reusable(
            &target,
            &DownloadSource {
                modified: None,
                ..source.clone()
            }
        )?);
        std::fs::write(&target, b"external modification")?;
        assert!(runtime.access(&target, true).is_err());
        assert!(runtime.reusable(&target, &source).is_err());
        Ok(())
    }

    #[test]
    fn partials_never_authorize_complete_reuse_even_at_the_full_length() -> anyhow::Result<()> {
        for length in [0, 4, 8] {
            let root = tempfile::tempdir()?;
            let target = root.path().join("file");
            std::fs::write(&target, vec![7; length])?;
            let source = DownloadSource {
                sftp_service_id: Some(1),
                path: b"/file".to_vec(),
                size: Some(8),
                modified: Some(42),
            };
            let mut runtime = DownloadRuntime::default();
            runtime.partial(&target, FileStamp::read(&target)?, source.clone())?;
            assert!(!runtime.reusable(&target, &source)?);
            assert_eq!(
                runtime.resume_offset(&target, &source)?,
                if length == 4 { 4 } else { 0 }
            );
            assert!(runtime.owns(&target, false)?);
        }
        Ok(())
    }

    #[test]
    fn a_post_commit_edit_cannot_be_adopted_as_a_new_checkpoint() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        std::fs::write(&target, b"part")?;
        let file = std::fs::File::open(&target)?;
        let stamp = FileStamp::from_file(&file)?;
        std::fs::write(&target, b"external edit")?;
        let source = DownloadSource {
            sftp_service_id: Some(1),
            path: b"/file".to_vec(),
            size: Some(20),
            modified: Some(42),
        };
        let mut runtime = DownloadRuntime::default();
        assert!(runtime.partial(&target, stamp, source.clone()).is_err());
        assert!(!runtime.owns(&target, false)?);
        assert_eq!(runtime.resume_offset(&target, &source)?, 0);
        Ok(())
    }

    #[test]
    fn incomplete_metadata_and_invalidated_checkpoints_cannot_regain_resume_by_matching_again()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        std::fs::write(&target, b"part")?;
        let source = DownloadSource {
            sftp_service_id: Some(1),
            path: b"/file".to_vec(),
            size: Some(8),
            modified: Some(42),
        };
        for changed in [
            DownloadSource {
                sftp_service_id: Some(2),
                ..source.clone()
            },
            DownloadSource {
                path: b"/other".to_vec(),
                ..source.clone()
            },
            DownloadSource {
                size: Some(9),
                ..source.clone()
            },
            DownloadSource {
                modified: Some(43),
                ..source.clone()
            },
            DownloadSource {
                size: None,
                ..source.clone()
            },
            DownloadSource {
                modified: None,
                ..source.clone()
            },
        ] {
            let mut runtime = DownloadRuntime::default();
            runtime.partial(&target, FileStamp::read(&target)?, source.clone())?;
            assert_eq!(runtime.resume_offset(&target, &changed)?, 0);
            assert_eq!(runtime.resume_offset(&target, &source)?, 0);
            assert!(runtime.owns(&target, false)?);
        }
        for unknown in [
            DownloadSource {
                size: None,
                ..source.clone()
            },
            DownloadSource {
                modified: None,
                ..source.clone()
            },
        ] {
            let mut runtime = DownloadRuntime::default();
            runtime.partial(&target, FileStamp::read(&target)?, unknown.clone())?;
            assert_eq!(runtime.resume_offset(&target, &unknown)?, 0);
        }
        Ok(())
    }
}
