pub(super) use nyaterm_files::external_edit::workspace::ExternalEditWorkspace;
use nyaterm_files::external_edit::workspace::prepare_external_edit_workspace as prepare;
use std::path::{Path, PathBuf};

pub(super) fn prepare_external_edit_workspace(
    file_name: &str,
    is_cancelled: impl Fn() -> bool,
) -> anyhow::Result<ExternalEditWorkspace> {
    #[cfg(windows)]
    let fallback = Some(dirs::data_local_dir);
    #[cfg(not(windows))]
    let fallback = None::<fn() -> Option<PathBuf>>;
    prepare(
        file_name,
        &std::env::temp_dir(),
        fallback,
        is_cancelled,
        secure_directory,
    )
}
fn secure_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        crate::platform::windows_acl::set_current_user_only(path, true)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
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
            local_path: workspace.local_path().to_path_buf(),
        }
    }
}

#[cfg(test)]
impl Drop for ExternalEditTestCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.local_path);
        let _ = std::fs::remove_dir(self.local_path.parent().expect("prepared workspace path"));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{ExternalEditTestCleanup, prepare_external_edit_workspace};
    #[test]
    fn external_workspace_is_private_and_protected_until_launch() {
        let workspace = prepare_external_edit_workspace("example.txt", || false).unwrap();
        let cleanup = ExternalEditTestCleanup::new(&workspace);
        let directory = workspace.local_path().parent().unwrap();
        crate::platform::windows_acl::assert_current_user_only(directory, true);
        assert!(std::fs::rename(directory, directory.with_extension("moved")).is_err());
        drop(workspace);
        drop(cleanup);
    }
}
