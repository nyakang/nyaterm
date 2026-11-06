//! Bulk downloads publish the staged file directly; editor saves retain their
//! separate inode-preserving, durable write contract.
use super::{DownloadTargetAccess, FileIdentity, ensure_no_symlink, open_without_following};
#[cfg(unix)]
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::Path;

pub(crate) struct TransferDownload {
    path: tempfile::TempPath,
    access: DownloadTargetAccess,
}

impl TransferDownload {
    pub(crate) async fn prepare_with_access(
        root: &Path,
        target: &Path,
        resume_offset: u64,
        access: DownloadTargetAccess,
    ) -> anyhow::Result<(Self, tokio::fs::File)> {
        let root = root.to_path_buf();
        let target = target.to_path_buf();
        let (staged, file) = tokio::task::spawn_blocking(move || {
            Self::prepare(&root, &target, resume_offset, access)
        })
        .await??;
        Ok((staged, tokio::fs::File::from_std(file)))
    }

    fn prepare(
        root: &Path,
        target: &Path,
        resume_offset: u64,
        access: DownloadTargetAccess,
    ) -> anyhow::Result<(Self, std::fs::File)> {
        validate_access(root, target, &access)?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".nyaterm-download-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = match &access {
                DownloadTargetAccess::CreateNew => 0o666,
                DownloadTargetAccess::ReplaceKnownTarget(_, _) => {
                    fs::metadata(target)?.permissions().mode() & 0o777
                }
            };
            builder.permissions(fs::Permissions::from_mode(mode));
        }
        let temporary = builder.tempfile_in(super::root_for_target(target))?;
        #[cfg(unix)]
        if matches!(access, DownloadTargetAccess::ReplaceKnownTarget(_, _)) {
            // umask applies to new files; an existing file's ordinary mode is preserved.
            use std::os::unix::fs::PermissionsExt as _;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(
                    fs::metadata(target)?.permissions().mode() & 0o777,
                ))?;
        }
        let (mut file, path) = temporary.into_parts();
        let staged = Self { path, access };
        if resume_offset > 0 {
            validate_access(root, target, &staged.access)?;
            anyhow::ensure!(
                matches!(
                    staged.access,
                    DownloadTargetAccess::ReplaceKnownTarget(_, _)
                ),
                "new download target cannot supply a resume prefix"
            );
            let source = open_without_following(target, false, false)?;
            if let DownloadTargetAccess::ReplaceKnownTarget(identity, _) = &staged.access {
                anyhow::ensure!(
                    FileIdentity::from_file(&source)? == *identity,
                    "local resume source changed"
                );
            }
            let copied = std::io::copy(&mut source.take(resume_offset), &mut file)?;
            anyhow::ensure!(copied == resume_offset, "local resume source changed");
            file.flush()?;
        }
        Ok::<_, anyhow::Error>((staged, file))
    }

    pub(crate) fn write(root: &Path, target: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let access = DownloadTargetAccess::capture(root, target)?;
        let (staged, mut file) = Self::prepare(root, target, 0, access)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        staged.commit(root, target).map(|_| ())
    }

    pub(crate) async fn commit_async(
        self,
        root: &Path,
        target: &Path,
    ) -> anyhow::Result<FileIdentity> {
        let root = root.to_path_buf();
        let target = target.to_path_buf();
        tokio::task::spawn_blocking(move || self.commit(&root, &target)).await?
    }

    fn commit(self, root: &Path, target: &Path) -> anyhow::Result<FileIdentity> {
        validate_access(root, target, &self.access)?;
        ensure_no_symlink(root, &self.path)?;
        let identity = FileIdentity::from_path(&self.path)?;
        match self.access {
            DownloadTargetAccess::CreateNew => self.path.persist_noclobber(target)?,
            DownloadTargetAccess::ReplaceKnownTarget(_, _) => self.path.persist(target)?,
        }
        anyhow::ensure!(
            FileIdentity::from_path(target)? == identity,
            "download target was replaced after commit"
        );
        Ok(identity)
    }
}

fn validate_access(
    root: &Path,
    target: &Path,
    access: &DownloadTargetAccess,
) -> anyhow::Result<()> {
    ensure_no_symlink(root, target)?;
    match access {
        DownloadTargetAccess::CreateNew => {
            anyhow::ensure!(
                !super::target_exists(target)?,
                "download target appeared before commit"
            );
        }
        DownloadTargetAccess::ReplaceKnownTarget(identity, stamp) => {
            let file = open_without_following(target, false, false)?;
            anyhow::ensure!(
                file.metadata()?.is_file(),
                "download target is not a regular file"
            );
            anyhow::ensure!(
                FileIdentity::from_file(&file)? == *identity,
                "download target was replaced during transfer"
            );
            if let Some((length, modified)) = stamp {
                let metadata = file.metadata()?;
                anyhow::ensure!(
                    metadata.len() == *length && metadata.modified()? == *modified,
                    "task-owned download file was modified before commit"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TransferDownload;
    use crate::download_path::{DownloadTargetAccess, FileIdentity};
    use tokio::io::AsyncWriteExt as _;

    #[tokio::test]
    async fn publishes_the_staged_file_and_replaces_existing_identity() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        std::fs::write(&target, b"old")?;
        let old = std::fs::File::open(&target)?;
        let old_identity = FileIdentity::from_file(&old)?;
        drop(old);
        let access = DownloadTargetAccess::capture(root.path(), &target)?;
        let (temporary, mut file) =
            TransferDownload::prepare_with_access(root.path(), &target, 0, access).await?;
        file.write_all(b"new contents").await?;
        file.flush().await?;
        let file = file.into_std().await;
        let staged_identity = FileIdentity::from_file(&file)?;
        drop(file);
        assert_eq!(
            temporary.commit_async(root.path(), &target).await?,
            staged_identity
        );
        assert_ne!(old_identity, staged_identity);
        assert_eq!(std::fs::read(target)?, b"new contents");
        Ok(())
    }

    #[tokio::test]
    async fn new_target_race_preserves_the_external_file_and_cleans_staging() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        let (temporary, file) = TransferDownload::prepare_with_access(
            root.path(),
            &target,
            0,
            DownloadTargetAccess::CreateNew,
        )
        .await?;
        drop(file);
        std::fs::write(&target, b"external")?;
        assert!(temporary.commit_async(root.path(), &target).await.is_err());
        assert_eq!(std::fs::read(&target)?, b"external");
        assert_eq!(std::fs::read_dir(root.path())?.count(), 1);
        Ok(())
    }
    #[cfg(windows)]
    #[test]
    fn locked_target_failure_preserves_original_and_removes_staging() -> anyhow::Result<()> {
        use std::os::windows::fs::OpenOptionsExt as _;
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        std::fs::write(&target, b"original")?;
        // The holder permits reads and writes but forbids rename/delete.
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&target)?;
        assert!(TransferDownload::write(root.path(), &target, b"new").is_err());
        assert_eq!(std::fs::read(&target)?, b"original");
        assert_eq!(std::fs::read_dir(root.path())?.count(), 1);
        drop(holder);
        TransferDownload::write(root.path(), &target, b"new")?;
        assert_eq!(std::fs::read(&target)?, b"new");
        Ok(())
    }

    #[test]
    fn resume_copies_the_prefix_and_commits_timestamp_without_recopying() -> anyhow::Result<()> {
        use std::io::Write as _;
        use std::time::{Duration, UNIX_EPOCH};
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        std::fs::write(&target, b"prefix obsolete")?;
        let access = DownloadTargetAccess::capture(root.path(), &target)?;
        let (temporary, mut file) = TransferDownload::prepare(root.path(), &target, 6, access)?;
        file.write_all(b" suffix")?;
        let timestamp = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        file.set_modified(timestamp)?;
        let identity = FileIdentity::from_file(&file)?;
        drop(file);
        assert_eq!(temporary.commit(root.path(), &target)?, identity);
        assert_eq!(std::fs::read(&target)?, b"prefix suffix");
        assert_eq!(std::fs::metadata(&target)?.modified()?, timestamp);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bulk_permissions_follow_umask_and_preserve_only_ordinary_overwrite_bits()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir()?;
        let reference = root.path().join("reference");
        std::fs::write(&reference, b"normal")?;
        let target = root.path().join("new");
        TransferDownload::write(root.path(), &target, b"new")?;
        assert_eq!(
            std::fs::metadata(&target)?.permissions().mode() & 0o777,
            std::fs::metadata(&reference)?.permissions().mode() & 0o777
        );
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o6751))?;
        TransferDownload::write(root.path(), &target, b"replacement")?;
        assert_eq!(
            std::fs::metadata(&target)?.permissions().mode() & 0o7777,
            0o751
        );
        Ok(())
    }
}
