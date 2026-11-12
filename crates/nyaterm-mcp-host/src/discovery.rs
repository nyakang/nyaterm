use std::path::{Path, PathBuf};
use std::sync::Arc;

use nyaterm_mcp_protocol::DiscoveryDocument;

/// Native permission policy supplied by the embedding application.
pub trait DiscoveryPermissions: Send + Sync {
    fn set_private(&self, path: &Path, directory: bool) -> anyhow::Result<()>;
}

pub struct DiscoveryStore {
    permissions: Arc<dyn DiscoveryPermissions>,
    directory: PathBuf,
    file: PathBuf,
}

impl DiscoveryStore {
    pub fn new(config_dir: &Path, permissions: Arc<dyn DiscoveryPermissions>) -> Self {
        let directory = config_dir.join("mcp");
        let file = directory.join("discovery.json");
        Self {
            directory,
            file,
            permissions,
        }
    }

    pub fn remove(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(&self.file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Ok(entries) = std::fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".discovery-") && name.ends_with(".tmp") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Ok(())
    }

    pub fn write(&self, document: &DiscoveryDocument) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        self.permissions.set_private(&self.directory, true)?;
        let temporary = self
            .directory
            .join(format!(".discovery-{}.tmp", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(document)?;
        std::fs::write(&temporary, bytes)?;
        if let Err(error) = self.permissions.set_private(&temporary, false) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }

        atomic_replace(&temporary, &self.file).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            anyhow::Error::from(error)
        })?;
        self.permissions.set_private(&self.file, false)
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DiscoveryPermissions, DiscoveryStore};
    use nyaterm_mcp_protocol::DiscoveryDocument;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Permissions {
        calls: Mutex<Vec<bool>>,
        fail_at: Option<usize>,
    }
    impl DiscoveryPermissions for Permissions {
        fn set_private(&self, _path: &Path, directory: bool) -> anyhow::Result<()> {
            let mut calls = self.calls.lock().unwrap();
            calls.push(directory);
            anyhow::ensure!(
                self.fail_at != Some(calls.len()),
                "permission fixture failure"
            );
            Ok(())
        }
    }
    fn document(token: &str) -> DiscoveryDocument {
        DiscoveryDocument {
            version: 1,
            pid: 1,
            host: "127.0.0.1".into(),
            port: 1,
            token: token.into(),
            generation: "fixture".into(),
            permission_mode: "observer".into(),
        }
    }
    #[test]
    fn replacement_keeps_document_format_and_permission_order_and_removal_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let permissions = Arc::new(Permissions::default());
        let store = DiscoveryStore::new(root.path(), permissions.clone());
        store.write(&document("first")).unwrap();
        store.write(&document("second")).unwrap();
        assert_eq!(
            *permissions.calls.lock().unwrap(),
            [true, false, false, true, false, false]
        );
        let actual: DiscoveryDocument =
            serde_json::from_slice(&std::fs::read(&store.file).unwrap()).unwrap();
        assert_eq!(actual.token, "second");
        assert_eq!(std::fs::read_dir(&store.directory).unwrap().count(), 1);
        std::fs::write(store.directory.join(".discovery-stale.tmp"), b"fixture").unwrap();
        store.remove().unwrap();
        store.remove().unwrap();
        assert_eq!(std::fs::read_dir(&store.directory).unwrap().count(), 0);
    }
    #[test]
    fn permission_failures_propagate_and_temporary_files_are_removed() {
        for fail_at in 1..=3 {
            let root = tempfile::tempdir().unwrap();
            let store = DiscoveryStore::new(
                root.path(),
                Arc::new(Permissions {
                    calls: Mutex::default(),
                    fail_at: Some(fail_at),
                }),
            );
            assert!(store.write(&document("fixture")).is_err());
            assert!(std::fs::read_dir(&store.directory).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            }));
            store.remove().unwrap();
        }
    }
}
