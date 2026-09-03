//! Unified `RemoteFs` trait that all remote file system backends implement.

use super::transfer::TransferController;
use super::util::{
    FileEntry, FileProperties, RemoteBinaryFile, RemoteFileAttributeUpdate, RemotePathRef,
    RemoteTextFile, WriteRemoteTextResult,
};
use crate::error::AppResult;
use std::any::Any;
use std::sync::Arc;

/// Common interface for remote file system operations.
///
/// Each backend (SFTP, SCP Enhanced, SCP Normal) implements this trait so the
/// upper-level orchestrator can switch between them transparently.
#[async_trait::async_trait]
pub(crate) trait RemoteFs: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn backend_name(&self) -> &'static str;

    async fn home_dir(&self) -> AppResult<String>;
    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>>;
    async fn list_dir_ref(&self, path: &RemotePathRef) -> AppResult<Vec<FileEntry>> {
        self.list_dir(path.display_path()).await
    }
    async fn stat(&self, path: &str) -> AppResult<FileProperties>;
    async fn stat_ref(&self, path: &RemotePathRef) -> AppResult<FileProperties> {
        self.stat(path.display_path()).await
    }
    async fn mkdir(&self, path: &str, mode: Option<String>) -> AppResult<()>;
    async fn remove_file(&self, path: &str) -> AppResult<()>;
    async fn remove_file_ref(&self, path: &RemotePathRef) -> AppResult<()> {
        self.remove_file(path.display_path()).await
    }
    async fn rename(&self, old_path: &str, new_path: &str) -> AppResult<()>;
    async fn rename_ref(
        &self,
        old_path: &RemotePathRef,
        new_path: &RemotePathRef,
    ) -> AppResult<()> {
        self.rename(old_path.display_path(), new_path.display_path())
            .await
    }
    async fn create_file(&self, path: &str, mode: Option<String>) -> AppResult<()>;
    async fn create_symlink(&self, link_path: &str, target_path: &str) -> AppResult<()>;
    async fn create_symlink_ref(
        &self,
        link_path: &RemotePathRef,
        target_path: &str,
    ) -> AppResult<()> {
        self.create_symlink(link_path.display_path(), target_path)
            .await
    }
    async fn update_symlink_target_ref(
        &self,
        path: &RemotePathRef,
        target_path: &str,
    ) -> AppResult<()> {
        replace_symlink_target(&RemoteFsSymlinkOps(self), path, target_path).await
    }
    async fn update_attrs(&self, path: &str, update: &RemoteFileAttributeUpdate) -> AppResult<()>;
    async fn update_attrs_ref(
        &self,
        path: &RemotePathRef,
        update: &RemoteFileAttributeUpdate,
    ) -> AppResult<()> {
        self.update_attrs(path.display_path(), update).await
    }
    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile>;
    async fn read_file_bytes(&self, path: &str, max_bytes: u64) -> AppResult<RemoteBinaryFile>;
    async fn write_file_text(
        &self,
        path: &str,
        content: &str,
        expected_mtime: Option<u64>,
        expected_size: Option<u64>,
        expected_hash: Option<&str>,
        force: bool,
    ) -> AppResult<WriteRemoteTextResult>;

    async fn download_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()>;

    async fn upload_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()>;

    async fn download_directory(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_id: Option<String>,
    ) -> AppResult<()>;

    async fn upload_directory(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()>;

    async fn copy_remote_file_to_local_with_controller(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<u64>;

    async fn copy_local_file_to_remote_with_controller(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<u64>;
}

#[async_trait::async_trait]
trait SymlinkReplacementOps: Send + Sync {
    async fn stat(&self, path: &RemotePathRef) -> AppResult<FileProperties>;
    async fn create(&self, path: &RemotePathRef, target_path: &str) -> AppResult<()>;
    async fn rename(&self, old_path: &RemotePathRef, new_path: &RemotePathRef) -> AppResult<()>;
    async fn remove(&self, path: &RemotePathRef) -> AppResult<()>;
}

struct RemoteFsSymlinkOps<'a, T: RemoteFs + ?Sized>(&'a T);

#[async_trait::async_trait]
impl<T: RemoteFs + ?Sized> SymlinkReplacementOps for RemoteFsSymlinkOps<'_, T> {
    async fn stat(&self, path: &RemotePathRef) -> AppResult<FileProperties> {
        self.0.stat_ref(path).await
    }

    async fn create(&self, path: &RemotePathRef, target_path: &str) -> AppResult<()> {
        self.0.create_symlink_ref(path, target_path).await
    }

    async fn rename(&self, old_path: &RemotePathRef, new_path: &RemotePathRef) -> AppResult<()> {
        self.0.rename_ref(old_path, new_path).await
    }

    async fn remove(&self, path: &RemotePathRef) -> AppResult<()> {
        self.0.remove_file_ref(path).await
    }
}

async fn ensure_symlink(
    fs: &(impl SymlinkReplacementOps + ?Sized),
    path: &RemotePathRef,
) -> AppResult<()> {
    let properties = fs.stat(path).await.map_err(|error| {
        crate::error::AppError::Channel(format!(
            "Failed to verify symbolic link '{}': {error}",
            path.display_path()
        ))
    })?;
    if properties.is_symlink {
        Ok(())
    } else {
        Err(crate::error::AppError::Config(format!(
            "Remote path '{}' is no longer a symbolic link; refusing to replace it",
            path.display_path()
        )))
    }
}

async fn cleanup_after_failure(
    fs: &(impl SymlinkReplacementOps + ?Sized),
    path: &RemotePathRef,
    primary_error: crate::error::AppError,
) -> crate::error::AppError {
    match fs.remove(path).await {
        Ok(()) => primary_error,
        Err(cleanup_error) => crate::error::AppError::Channel(format!(
            "{primary_error}; cleanup of '{}' also failed: {cleanup_error}",
            path.display_path()
        )),
    }
}

async fn replace_symlink_target(
    fs: &(impl SymlinkReplacementOps + ?Sized),
    original: &RemotePathRef,
    target_path: &str,
) -> AppResult<()> {
    if target_path.trim().is_empty() {
        return Err(crate::error::AppError::Config(
            "Symbolic link target cannot be empty".to_string(),
        ));
    }

    ensure_symlink(fs, original).await?;

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let temp = original.sibling(&format!(".nyaterm-link-{suffix}"));
    let backup = original.sibling(&format!(".nyaterm-backup-{suffix}"));

    fs.create(&temp, target_path).await.map_err(|error| {
        crate::error::AppError::Channel(format!(
            "Failed to create temporary symbolic link '{}': {error}",
            temp.display_path()
        ))
    })?;

    if let Err(error) = ensure_symlink(fs, original).await {
        return Err(cleanup_after_failure(fs, &temp, error).await);
    }

    if let Err(error) = fs.rename(original, &backup).await {
        let error = crate::error::AppError::Channel(format!(
            "Failed to move original symbolic link '{}' to backup: {error}",
            original.display_path()
        ));
        return Err(cleanup_after_failure(fs, &temp, error).await);
    }

    if let Err(replacement_error) = fs.rename(&temp, original).await {
        return match fs.rename(&backup, original).await {
            Ok(()) => {
                let error = crate::error::AppError::Channel(format!(
                    "Failed to replace symbolic link '{}': {replacement_error}; the original link was restored",
                    original.display_path()
                ));
                Err(cleanup_after_failure(fs, &temp, error).await)
            }
            Err(rollback_error) => Err(crate::error::AppError::Channel(format!(
                "Failed to replace symbolic link '{}': {replacement_error}; rollback from '{}' also failed: {rollback_error}",
                original.display_path(),
                backup.display_path()
            ))),
        };
    }

    if let Err(error) = fs.remove(&backup).await {
        tracing::warn!(
            original_path = original.display_path(),
            backup_path = backup.display_path(),
            error = %error,
            "Symbolic link target was updated, but backup cleanup failed"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use std::collections::HashSet;
    use std::sync::Mutex;

    struct RecordingOps {
        actions: Mutex<Vec<String>>,
        failures: Mutex<HashSet<String>>,
        is_symlink: bool,
    }

    impl Default for RecordingOps {
        fn default() -> Self {
            Self {
                actions: Mutex::new(Vec::new()),
                failures: Mutex::new(HashSet::new()),
                is_symlink: true,
            }
        }
    }

    impl RecordingOps {
        fn failing(actions: &[&str]) -> Self {
            Self {
                actions: Mutex::new(Vec::new()),
                failures: Mutex::new(actions.iter().map(|value| (*value).to_string()).collect()),
                is_symlink: true,
            }
        }

        fn regular_file() -> Self {
            Self {
                is_symlink: false,
                ..Self::default()
            }
        }

        fn record(&self, action: String, failure_key: &str) -> AppResult<()> {
            self.actions.lock().unwrap().push(action);
            if self.failures.lock().unwrap().contains(failure_key) {
                Err(AppError::Channel(format!("injected {failure_key} failure")))
            } else {
                Ok(())
            }
        }

        fn actions(&self) -> Vec<String> {
            self.actions.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl SymlinkReplacementOps for RecordingOps {
        async fn stat(&self, path: &RemotePathRef) -> AppResult<FileProperties> {
            self.record(format!("stat:{}", path.display_path()), "stat")?;
            Ok(FileProperties {
                name: "current".to_string(),
                is_dir: false,
                is_symlink: self.is_symlink,
                symlink_target: self.is_symlink.then(|| "releases/v2".to_string()),
                size: 0,
                permissions: "lrwxrwxrwx".to_string(),
                owner: "root".to_string(),
                group: "root".to_string(),
                uid: "0".to_string(),
                gid: "0".to_string(),
                mtime: 0,
                atime: 0,
            })
        }

        async fn create(&self, path: &RemotePathRef, target_path: &str) -> AppResult<()> {
            self.record(
                format!("create:{}->{target_path}", path.display_path()),
                "create",
            )
        }

        async fn rename(
            &self,
            old_path: &RemotePathRef,
            new_path: &RemotePathRef,
        ) -> AppResult<()> {
            let failure_key = if old_path.display_path().contains("nyaterm-link") {
                "commit"
            } else if old_path.display_path().contains("nyaterm-backup") {
                "rollback"
            } else {
                "backup"
            };
            self.record(
                format!(
                    "rename:{}->{}",
                    old_path.display_path(),
                    new_path.display_path()
                ),
                failure_key,
            )
        }

        async fn remove(&self, path: &RemotePathRef) -> AppResult<()> {
            let failure_key = if path.display_path().contains("nyaterm-backup") {
                "remove_backup"
            } else {
                "remove_temp"
            };
            self.record(format!("remove:{}", path.display_path()), failure_key)
        }
    }

    fn original() -> RemotePathRef {
        RemotePathRef::new("/opt/app/current", None).unwrap()
    }

    #[tokio::test]
    async fn replacement_creates_temp_before_moving_original() {
        let ops = RecordingOps::default();
        replace_symlink_target(&ops, &original(), " releases/v3 ")
            .await
            .unwrap();
        let actions = ops.actions();
        let create = actions
            .iter()
            .position(|action| action.starts_with("create:"))
            .unwrap();
        let backup = actions
            .iter()
            .position(|action| action.starts_with("rename:/opt/app/current->"))
            .unwrap();
        assert!(create < backup);
        assert!(actions[create].ends_with("-> releases/v3 "));
        assert!(
            actions
                .last()
                .unwrap()
                .starts_with("remove:/opt/app/.nyaterm-backup-")
        );
    }

    #[tokio::test]
    async fn missing_or_non_symlink_original_is_never_recreated() {
        let missing = RecordingOps::failing(&["stat"]);
        replace_symlink_target(&missing, &original(), "releases/v3")
            .await
            .unwrap_err();
        assert_eq!(missing.actions(), ["stat:/opt/app/current"]);

        let regular = RecordingOps::regular_file();
        let error = replace_symlink_target(&regular, &original(), "releases/v3")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer a symbolic link"));
        assert_eq!(regular.actions(), ["stat:/opt/app/current"]);
    }

    #[tokio::test]
    async fn temporary_link_failure_leaves_original_untouched() {
        let ops = RecordingOps::failing(&["create"]);
        replace_symlink_target(&ops, &original(), "releases/v3")
            .await
            .unwrap_err();
        let actions = ops.actions();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], "stat:/opt/app/current");
        assert!(actions[1].starts_with("create:/opt/app/.nyaterm-link-"));
    }

    #[tokio::test]
    async fn replacement_failure_rolls_back_original() {
        let ops = RecordingOps::failing(&["commit"]);
        let error = replace_symlink_target(&ops, &original(), "releases/v3")
            .await
            .unwrap_err();
        let actions = ops.actions();
        assert!(error.to_string().contains("original link was restored"));
        assert!(actions.iter().any(|action| {
            action.starts_with("rename:/opt/app/.nyaterm-backup-")
                && action.ends_with("->/opt/app/current")
        }));
        assert!(
            actions
                .last()
                .unwrap()
                .starts_with("remove:/opt/app/.nyaterm-link-")
        );
    }

    #[tokio::test]
    async fn replacement_and_rollback_failures_are_both_reported() {
        let ops = RecordingOps::failing(&["commit", "rollback"]);
        let error = replace_symlink_target(&ops, &original(), "releases/v3")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("injected commit failure"));
        assert!(error.contains("injected rollback failure"));
    }

    #[tokio::test]
    async fn failed_backup_move_cleans_temp_without_committing() {
        let ops = RecordingOps::failing(&["backup"]);
        replace_symlink_target(&ops, &original(), "releases/v3")
            .await
            .unwrap_err();
        let actions = ops.actions();
        assert!(
            actions
                .last()
                .unwrap()
                .starts_with("remove:/opt/app/.nyaterm-link-")
        );
        assert!(!actions.iter().any(|action| {
            action.starts_with("rename:/opt/app/.nyaterm-link-")
                && action.ends_with("->/opt/app/current")
        }));
    }

    #[tokio::test]
    async fn backup_cleanup_failure_does_not_undo_successful_replacement() {
        let ops = RecordingOps::failing(&["remove_backup"]);
        replace_symlink_target(&ops, &original(), "missing-release")
            .await
            .unwrap();
        assert!(
            ops.actions()
                .last()
                .unwrap()
                .starts_with("remove:/opt/app/.nyaterm-backup-")
        );
    }
}
