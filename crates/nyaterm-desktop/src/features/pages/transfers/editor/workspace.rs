use std::fs::{self, DirBuilder};
use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;
use nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED;

use super::helpers::sanitize_local_open_segment;

const WORKSPACE_PREFIX: &str = "nyaterm-external-";

fn external_edit_path_in(root: &Path, file_name: &str) -> PathBuf {
    root.join(format!("{WORKSPACE_PREFIX}{}", uuid::Uuid::new_v4()))
        .join(sanitize_local_open_segment(file_name))
}

pub(super) struct ExternalEditWorkspace {
    pub(super) local_path: PathBuf,
    // Deny rename/delete until the editor is launched on Windows, where
    // even a directory owned by the current user can otherwise become a junction.
    #[cfg(windows)]
    _directory: fs::File,
}

// The shared TestTempDir deliberately only owns system-TEMP children. External
// edit fixtures can live under LocalAppData, so clean only their known file and
// empty parent, after the workspace's directory protection has been released.
#[cfg(test)]
pub(super) struct ExternalEditTestCleanup {
    local_path: PathBuf,
}

#[cfg(test)]
impl ExternalEditTestCleanup {
    pub(super) fn new(workspace: &ExternalEditWorkspace) -> Self {
        Self {
            local_path: workspace.local_path.clone(),
        }
    }
}

#[cfg(test)]
impl Drop for ExternalEditTestCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.local_path);
        let _ = fs::remove_dir(self.local_path.parent().expect("prepared workspace path"));
    }
}

pub(super) fn prepare_external_edit_workspace(
    file_name: &str,
    is_cancelled: impl Fn() -> bool,
) -> anyhow::Result<ExternalEditWorkspace> {
    #[cfg(windows)]
    let fallback = Some(dirs::data_local_dir);
    #[cfg(not(windows))]
    let fallback = None::<fn() -> Option<PathBuf>>;
    prepare_workspace_with_fallback(
        file_name,
        &std::env::temp_dir(),
        fallback,
        is_cancelled,
        finish_workspace,
    )
}

fn prepare_workspace_with_fallback(
    file_name: &str,
    temp_root: &Path,
    fallback_root: Option<impl FnOnce() -> Option<PathBuf>>,
    is_cancelled: impl Fn() -> bool,
    mut finish: impl FnMut(&Path) -> anyhow::Result<ExternalEditWorkspace>,
) -> anyhow::Result<ExternalEditWorkspace> {
    let mut prepare = |root: &Path| {
        anyhow::ensure!(!is_cancelled(), SFTP_TRANSFER_CANCELLED);
        let local_path = external_edit_path_in(root, file_name);
        let workspace = prepare_workspace_in(root, &local_path, &mut finish)?;
        if is_cancelled() {
            drop(workspace);
            let _ = fs::remove_dir(local_path.parent().expect("generated workspace path"));
            anyhow::bail!(SFTP_TRANSFER_CANCELLED);
        }
        Ok(workspace)
    };
    let primary_error = match prepare(temp_root) {
        Ok(workspace) => return Ok(workspace),
        Err(error) => error,
    };
    // Cancellation wins over preparation errors and must never start another attempt.
    anyhow::ensure!(!is_cancelled(), SFTP_TRANSFER_CANCELLED);
    let Some(fallback_root) = fallback_root else {
        return Err(primary_error);
    };
    let root = fallback_root();
    anyhow::ensure!(!is_cancelled(), SFTP_TRANSFER_CANCELLED);
    let root = root.ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to prepare external edit workspace in TEMP: {primary_error:#}; \
             LocalAppData is unavailable"
        )
    })?;
    prepare(&root).map_err(|error| {
        if is_cancelled() {
            anyhow::anyhow!(SFTP_TRANSFER_CANCELLED)
        } else {
            anyhow::anyhow!(
                "Failed to prepare external edit workspace in TEMP: {primary_error:#}; \
                 failed in LocalAppData: {error:#}"
            )
        }
    })
}

fn prepare_workspace_in(
    temp_root: &Path,
    local_path: &Path,
    mut finish: impl FnMut(&Path) -> anyhow::Result<ExternalEditWorkspace>,
) -> anyhow::Result<ExternalEditWorkspace> {
    let relative = local_path.strip_prefix(temp_root)?;
    let components = relative.components().collect::<Vec<_>>();
    anyhow::ensure!(
        matches!(
            components.as_slice(),
            [Component::Normal(_), Component::Normal(_)]
        ),
        "External edit path escapes its private workspace"
    );
    let workspace = local_path.parent().expect("validated workspace path");
    let workspace_name = workspace.file_name().and_then(|name| name.to_str());
    anyhow::ensure!(
        workspace_name
            .and_then(|name| name.strip_prefix(WORKSPACE_PREFIX))
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()),
        "Invalid external edit workspace"
    );

    // Create one unpredictable directory directly beneath the trusted system
    // storage root. Never reuse an existing path or traverse pre-existing app/session
    // directories, which could be symlinks or Windows reparse points.
    let builder = DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder
        .create(workspace)
        .context("Failed to create external edit workspace")?;
    let result = finish(local_path).context("Failed to secure external edit workspace");
    if result.is_err() {
        let _ = fs::remove_dir(workspace);
    }
    result
}

fn finish_workspace(local_path: &Path) -> anyhow::Result<ExternalEditWorkspace> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use std::os::windows::io::AsRawHandle as _;
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE, GetVolumeInformationByHandleW,
        };
        use windows_sys::Win32::System::SystemServices::FILE_PERSISTENT_ACLS;

        let workspace = local_path.parent().expect("validated workspace path");
        let directory = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(workspace)?;
        let metadata = directory.metadata()?;
        anyhow::ensure!(
            metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
            "External edit workspace is not a regular directory"
        );
        let mut flags = 0;
        if unsafe {
            GetVolumeInformationByHandleW(
                directory.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                &mut flags,
                null_mut(),
                0,
            )
        } == 0
        {
            let status = unsafe { GetLastError() };
            anyhow::bail!(
                "Failed to check external edit volume during GetVolumeInformationByHandleW: \
                 win32_error={status} ({})",
                std::io::Error::from_raw_os_error(status as i32)
            );
        }
        anyhow::ensure!(
            flags & FILE_PERSISTENT_ACLS != 0,
            "External edit volume does not support persistent ACLs"
        );
        crate::platform::windows_acl::set_current_user_only(workspace, true)?;
        Ok(ExternalEditWorkspace {
            local_path: local_path.to_path_buf(),
            _directory: directory,
        })
    }
    #[cfg(not(windows))]
    {
        Ok(ExternalEditWorkspace {
            local_path: local_path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        ExternalEditTestCleanup, WORKSPACE_PREFIX, external_edit_path_in, finish_workspace,
        prepare_workspace_in, prepare_workspace_with_fallback,
    };
    use nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED;

    #[test]
    fn usable_temp_does_not_resolve_fallback() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-primary");
        fs::create_dir(root.path())?;
        let workspace = prepare_workspace_with_fallback(
            "../example.txt",
            &root,
            Some(|| -> Option<std::path::PathBuf> { panic!("unexpected fallback") }),
            || false,
            finish_workspace,
        )?;
        assert!(workspace.local_path.starts_with(root.path()));
        assert_eq!(workspace.local_path.file_name().unwrap(), "_example.txt");
        assert!(!workspace.local_path.exists());
        Ok(())
    }

    #[test]
    fn unsupported_acl_and_access_denied_fall_back_without_leaving_failed_directories()
    -> anyhow::Result<()> {
        for reason in [
            "External edit volume does not support persistent ACLs",
            "SetNamedSecurityInfoW: win32_error=5 (Access is denied)",
        ] {
            let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-fallback");
            fs::create_dir(root.path())?;
            let primary = root.join("temp");
            let fallback = root.join("local");
            fs::create_dir(&primary)?;
            fs::create_dir(&fallback)?;
            let mut failed_path = None;
            let workspace = prepare_workspace_with_fallback(
                "example.txt",
                &primary,
                Some(|| Some(fallback.clone())),
                || false,
                |path| {
                    if path.starts_with(&primary) {
                        failed_path = Some(path.to_path_buf());
                        anyhow::bail!(reason.to_string());
                    }
                    finish_workspace(path)
                },
            )?;
            let cleanup = ExternalEditTestCleanup::new(&workspace);
            assert!(workspace.local_path.starts_with(&fallback));
            assert!(workspace.local_path.parent().unwrap().is_dir());
            assert!(!workspace.local_path.exists());
            let failed_path = failed_path.unwrap();
            assert!(!failed_path.parent().unwrap().exists());
            assert_ne!(
                failed_path.parent().unwrap().file_name(),
                workspace.local_path.parent().unwrap().file_name()
            );
            #[cfg(windows)]
            crate::platform::windows_acl::assert_current_user_only(
                workspace.local_path.parent().unwrap(),
                true,
            );
            let local_path = workspace.local_path.clone();
            fs::write(&local_path, b"fallback download")?;
            drop(workspace);
            drop(cleanup);
            assert!(!local_path.parent().unwrap().exists());
        }
        Ok(())
    }

    #[test]
    fn missing_temp_root_uses_fallback_without_creating_parent_directories() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-missing-temp");
        fs::create_dir(root.path())?;
        let missing = root.join("missing");
        let workspace = prepare_workspace_with_fallback(
            "example.txt",
            &missing,
            Some(|| Some(root.path().to_path_buf())),
            || false,
            finish_workspace,
        )?;
        assert!(workspace.local_path.starts_with(root.path()));
        assert!(!missing.exists());
        Ok(())
    }

    #[test]
    fn unavailable_fallback_retains_the_primary_win32_error() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-no-fallback");
        fs::create_dir(root.path())?;
        let error = prepare_workspace_with_fallback(
            "example.txt",
            &root,
            Some(|| None),
            || false,
            |_| anyhow::bail!("SetNamedSecurityInfoW: win32_error=5"),
        )
        .err()
        .expect("preparation must fail");
        let message = format!("{error:#}");
        assert!(message.contains("TEMP"));
        assert!(message.contains("win32_error=5"));
        assert!(message.contains("LocalAppData is unavailable"));
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn missing_local_app_data_directory_retains_both_preparation_failures() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-missing-local");
        fs::create_dir(root.path())?;
        let missing = root.join("missing");
        let error = prepare_workspace_with_fallback(
            "example.txt",
            &root,
            Some(|| Some(missing.clone())),
            || false,
            |_| anyhow::bail!("SetNamedSecurityInfoW: win32_error=5"),
        )
        .err()
        .expect("preparation must fail");
        let message = format!("{error:#}");
        assert!(message.contains("win32_error=5"));
        assert!(message.contains("failed in LocalAppData"));
        assert!(message.contains("Failed to create external edit workspace"));
        assert!(!missing.exists());
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn failed_preparation_never_recursively_deletes_workspace_contents() -> anyhow::Result<()> {
        let root =
            nyaterm_core::test_support::TestTempDir::new("nyaterm-external-cleanup-boundary");
        fs::create_dir(root.path())?;
        let target = external_edit_path_in(&root, "example.txt");
        let result = prepare_workspace_in(&root, &target, |path| {
            fs::write(path, "must survive")?;
            anyhow::bail!("injected permission failure")
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&target)?, b"must survive");
        Ok(())
    }

    #[test]
    fn both_failed_attempts_retain_their_errors_and_leave_no_workspace() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-both-failed");
        fs::create_dir(root.path())?;
        let mut attempts = 0;
        let error = prepare_workspace_with_fallback(
            "example.txt",
            &root,
            Some(|| Some(root.path().to_path_buf())),
            || false,
            |_| {
                attempts += 1;
                anyhow::bail!(
                    "SetNamedSecurityInfoW: win32_error={}",
                    if attempts == 1 { 50 } else { 5 }
                );
            },
        )
        .err()
        .expect("preparation must fail");
        assert_eq!(attempts, 2);
        let message = format!("{error:#}");
        for expected in ["TEMP", "LocalAppData", "win32_error=50", "win32_error=5"] {
            assert!(message.contains(expected), "{message}");
        }
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn cancelled_preparation_never_starts_or_resolves_fallback() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-cancelled");
        fs::create_dir(root.path())?;
        let error = prepare_workspace_with_fallback(
            "example.txt",
            &root,
            Some(|| -> Option<std::path::PathBuf> { panic!("unexpected fallback") }),
            || true,
            |_| panic!("unexpected preparation"),
        )
        .err()
        .expect("preparation must be cancelled");
        assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn cancellation_during_preparation_cleans_up_and_prevents_fallback() -> anyhow::Result<()> {
        for preparation_succeeds in [false, true] {
            let root =
                nyaterm_core::test_support::TestTempDir::new("nyaterm-external-cancel-during");
            fs::create_dir(root.path())?;
            let cancelled = AtomicBool::new(false);
            let error = prepare_workspace_with_fallback(
                "example.txt",
                &root,
                Some(|| -> Option<std::path::PathBuf> { panic!("unexpected fallback") }),
                || cancelled.load(Ordering::Relaxed),
                |path| {
                    cancelled.store(true, Ordering::Relaxed);
                    if preparation_succeeds {
                        finish_workspace(path)
                    } else {
                        anyhow::bail!("SetNamedSecurityInfoW: win32_error=5")
                    }
                },
            )
            .err()
            .expect("preparation must be cancelled");
            assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
            assert_eq!(fs::read_dir(root.path())?.count(), 0);
        }
        Ok(())
    }

    #[test]
    fn cancellation_while_resolving_fallback_prevents_its_creation() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-cancel-fallback");
        fs::create_dir(root.path())?;
        let cancelled = AtomicBool::new(false);
        let mut attempts = 0;
        let error = prepare_workspace_with_fallback(
            "example.txt",
            &root,
            Some(|| {
                cancelled.store(true, Ordering::Relaxed);
                Some(root.path().to_path_buf())
            }),
            || cancelled.load(Ordering::Relaxed),
            |_| {
                attempts += 1;
                anyhow::bail!("unsupported ACLs")
            },
        )
        .err()
        .expect("preparation must be cancelled");
        assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
        assert_eq!(attempts, 1);
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn external_edits_get_distinct_workspaces_and_safe_file_names() {
        let first = external_edit_path_in(&std::env::temp_dir(), "../example.txt");
        let second = external_edit_path_in(&std::env::temp_dir(), "../example.txt");
        assert_ne!(first.parent(), second.parent());
        assert_eq!(first.file_name().unwrap(), "_example.txt");
        assert!(first.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn workspace_exists_and_is_private_before_downloading() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-edit");
        fs::create_dir(root.path())?;
        let directory = root.join(format!("{WORKSPACE_PREFIX}{}", uuid::Uuid::new_v4()));
        let target = directory.join("example.txt");
        let _workspace = prepare_workspace_in(&root, &target, finish_workspace)?;
        assert!(directory.is_dir());
        assert!(!target.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&directory)?.permissions().mode() & 0o777,
                0o700
            );
        }
        #[cfg(windows)]
        crate::platform::windows_acl::assert_current_user_only(&directory, true);

        // Both the shell downloader and external editors stage files in this parent.
        let staged = directory.join(".download.tmp");
        fs::write(&staged, b"downloaded")?;
        fs::rename(&staged, &target)?;
        assert_eq!(fs::read(&target)?, b"downloaded");
        assert!(prepare_workspace_in(&root, &target, finish_workspace).is_err());
        assert_eq!(fs::read(&target)?, b"downloaded");
        Ok(())
    }

    #[test]
    fn workspace_rejects_paths_outside_its_private_directory() -> anyhow::Result<()> {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-boundary");
        fs::create_dir(root.path())?;
        let directory = root.join(format!("{WORKSPACE_PREFIX}{}", uuid::Uuid::new_v4()));
        for target in [
            root.join("file.txt"),
            root.join("other/file.txt"),
            directory.join("../file.txt"),
            directory.join("nested/file.txt"),
            root.path().with_extension("outside").join("file.txt"),
        ] {
            assert!(
                prepare_workspace_in(&root, &target, finish_workspace).is_err(),
                "{target:?}"
            );
        }
        assert!(!directory.exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn workspace_rejects_junctions_and_blocks_directory_replacement() -> anyhow::Result<()> {
        use std::process::Command;

        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-junction");
        fs::create_dir(root.path())?;
        let outside = root.join("outside");
        fs::create_dir(&outside)?;
        let junction = root.join(format!("{WORKSPACE_PREFIX}{}", uuid::Uuid::new_v4()));
        let output = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()?;
        assert!(output.status.success(), "{output:?}");
        assert!(prepare_workspace_in(&root, &junction.join("file.txt"), finish_workspace).is_err());
        assert!(!outside.join("file.txt").exists());
        fs::remove_dir(&junction)?;

        let directory = root.join(format!("{WORKSPACE_PREFIX}{}", uuid::Uuid::new_v4()));
        let workspace = prepare_workspace_in(&root, &directory.join("file.txt"), finish_workspace)?;
        assert!(fs::rename(&directory, root.join("renamed")).is_err());
        assert!(fs::remove_dir(&directory).is_err());
        drop(workspace);
        fs::remove_dir(&directory)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn workspace_rejects_symlink_directories() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-symlink");
        fs::create_dir(root.path())?;
        let outside = root.join("outside");
        fs::create_dir(&outside)?;
        let link = root.join(format!("{WORKSPACE_PREFIX}{}", uuid::Uuid::new_v4()));
        symlink(&outside, &link)?;
        assert!(prepare_workspace_in(&root, &link.join("file.txt"), finish_workspace).is_err());
        assert!(!outside.join("file.txt").exists());
        Ok(())
    }
}
