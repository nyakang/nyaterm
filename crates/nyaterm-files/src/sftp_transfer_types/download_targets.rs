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
struct FileStamp {
    identity: FileIdentity,
    length: u64,
    modified: SystemTime,
}

impl FileStamp {
    fn read(path: &Path) -> anyhow::Result<Self> {
        let file = crate::download_path::open_without_following(path, false, false)?;
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "download target is not a regular file");
        Ok(Self {
            identity: FileIdentity::from_file(&file)?,
            length: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

struct CompletedFile {
    local: FileStamp,
    remote_path: Vec<u8>,
    size: Option<u64>,
    modified: Option<u32>,
}

#[derive(Default)]
pub(crate) struct DownloadRuntime {
    selected: HashMap<SftpDuplicateCacheKey, PathBuf>,
    access: HashMap<PathBuf, DownloadTargetAccess>,
    directories: HashMap<PathBuf, FileIdentity>,
    files: HashMap<PathBuf, CompletedFile>,
}

impl DownloadRuntime {
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
        if promised {
            self.validate_ancestors(path)?;
            if let Some(completed) = self.files.get(path) {
                anyhow::ensure!(
                    FileStamp::read(path)? == completed.local,
                    "task-owned download file was modified or replaced"
                );
                return Ok(DownloadTargetAccess::ReplaceKnownTarget(
                    completed.local.identity.clone(),
                    Some((completed.local.length, completed.local.modified)),
                ));
            }
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

    pub(crate) fn reusable(
        &self,
        path: &Path,
        remote_path: &[u8],
        size: Option<u64>,
        modified: Option<u32>,
    ) -> anyhow::Result<bool> {
        let Some(file) = self.files.get(path) else {
            return Ok(false);
        };
        anyhow::ensure!(
            FileStamp::read(path)? == file.local,
            "task-owned download file was modified or replaced"
        );
        // Missing source timestamps are not sufficient to identify a remote revision.
        Ok(modified.is_some()
            && size.is_some()
            && file.remote_path == remote_path
            && file.size == size
            && file.modified == modified)
    }

    pub(crate) fn completed(
        &mut self,
        path: &Path,
        identity: FileIdentity,
        remote_path: Vec<u8>,
        size: Option<u64>,
        modified: Option<u32>,
    ) -> anyhow::Result<()> {
        let local = FileStamp::read(path)?;
        anyhow::ensure!(
            local.identity == identity,
            "download target was replaced after commit"
        );
        self.access.insert(
            path.to_path_buf(),
            DownloadTargetAccess::ReplaceKnownTarget(identity, None),
        );
        self.files.insert(
            path.to_path_buf(),
            CompletedFile {
                local,
                remote_path,
                size,
                modified,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DownloadRuntime;
    use crate::download_path::FileIdentity;

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
        runtime.completed(
            &target,
            FileIdentity::from_path(&target)?,
            b"/file".to_vec(),
            Some(9),
            Some(42),
        )?;
        assert!(runtime.reusable(&target, b"/file", Some(9), Some(42))?);
        assert!(!runtime.reusable(&target, b"/file", Some(9), Some(43))?);
        assert!(!runtime.reusable(&target, b"/file", Some(9), None)?);
        std::fs::write(&target, b"external modification")?;
        assert!(runtime.access(&target, true).is_err());
        assert!(
            runtime
                .reusable(&target, b"/file", Some(9), Some(42))
                .is_err()
        );
        Ok(())
    }
}
