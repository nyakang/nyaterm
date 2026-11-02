use std::io::Write;
use std::path::{Component, Path, PathBuf};

use super::{CloudSyncError, CloudSyncRemote, SYNC_SNAPSHOTS_DIR};

#[derive(Debug, Clone)]
pub struct LocalDirectoryRemote {
    root_dir: PathBuf,
}

impl LocalDirectoryRemote {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    fn path_for(&self, path: &str) -> Result<PathBuf, CloudSyncError> {
        let mut local = self.root_dir.clone();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(part) => local.push(part),
                Component::CurDir => {}
                _ => {
                    return Err(CloudSyncError::InvalidRemotePath {
                        path: path.to_string(),
                    });
                }
            }
        }
        Ok(local)
    }
}

impl CloudSyncRemote for LocalDirectoryRemote {
    fn write_sync_file(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let destination = self.path_for(path)?;
        let parent = destination
            .parent()
            .ok_or_else(|| CloudSyncError::InvalidRemotePath {
                path: path.to_owned(),
            })?;
        // A crash can leave a staging file, but never a partial publication in
        // the listing. Hard-link creation atomically installs without replacing
        // another writer's object. Both paths are on the same filesystem.
        let staging = parent.join(".sync-v4-staging");
        std::fs::create_dir_all(&staging).map_err(|source| CloudSyncError::CreateDir {
            path: staging.clone(),
            source,
        })?;
        let temporary = staging.join(uuid::Uuid::new_v4().to_string());
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            match std::fs::hard_link(&temporary, &destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::fs::read(&destination)? != bytes {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "immutable sync object differs",
                        ));
                    }
                }
                Err(error) => return Err(error),
            }
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&temporary);
        result.map_err(|source| CloudSyncError::WriteFile {
            path: destination,
            source,
        })
    }
    fn list_sync_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        let directory = self.path_for(path)?;
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(CloudSyncError::ReadFile {
                    path: directory,
                    source,
                });
            }
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| CloudSyncError::ReadFile {
                path: directory.clone(),
                source,
            })?;
            if entry
                .file_type()
                .map_err(|source| CloudSyncError::ReadFile {
                    path: entry.path(),
                    source,
                })?
                .is_file()
            {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| CloudSyncError::Remote("Invalid V4 object filename".into()))?;
                files.push(remote_path(path, &name));
            }
        }
        Ok(files)
    }
    fn provider(&self) -> &'static str {
        "local_directory"
    }

    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        let path = self.path_for(path)?;
        std::fs::create_dir_all(&path).map_err(|source| CloudSyncError::CreateDir {
            path: path.clone(),
            source,
        })
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        let path = self.path_for(path)?;
        if !path.exists() {
            return Ok(None);
        }
        std::fs::read(&path)
            .map(Some)
            .map_err(|source| CloudSyncError::ReadFile { path, source })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let path = self.path_for(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| CloudSyncError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&path, bytes).map_err(|source| CloudSyncError::WriteFile { path, source })
    }

    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        let path = self.path_for(path)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CloudSyncError::WriteFile { path, source }),
        }
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        let directory = self.path_for(path)?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let prefix = path.trim().trim_matches('/');
        std::fs::read_dir(&directory)
            .map_err(|source| CloudSyncError::ReadFile {
                path: directory.clone(),
                source,
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_file())
                    .map(|_| entry.file_name().to_string_lossy().into_owned())
            })
            .map(|name| Ok(remote_path(prefix, &name)))
            .collect()
    }
}

pub fn remote_path(base_root: &str, child: &str) -> String {
    let root = base_root.trim().trim_matches('/');
    let child = child.trim().trim_start_matches('/');
    if root.is_empty() {
        child.to_string()
    } else if child.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{child}")
    }
}

pub fn drive_remote_segments(base_root: &str, child: &str) -> Vec<String> {
    remote_path(base_root, child)
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn google_drive_query_literal(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

pub fn legacy_sync_snapshot_file(revision: &str) -> String {
    format!("{SYNC_SNAPSHOTS_DIR}{revision}.redb.enc")
}

#[cfg(test)]
mod tests {
    use super::LocalDirectoryRemote;
    use crate::cloud_sync::CloudSyncRemote;
    use crate::test_support::TestTempDir;

    #[test]
    fn interrupted_staging_does_not_expose_a_partial_immutable_object() {
        let dir = TestTempDir::new("nyaterm-v4-staged-publication");
        let remote = LocalDirectoryRemote::new(dir.path());
        let prefix = "sync/v4/writers/actor";
        let staging = dir.path().join(prefix).join(".sync-v4-staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("interrupted"), b"partial").unwrap();
        assert!(remote.list_sync_files(prefix).unwrap().is_empty());
        let path = format!("{prefix}/1-object.bin");
        remote.write_sync_file(&path, b"complete").unwrap();
        remote.write_sync_file(&path, b"complete").unwrap();
        assert!(remote.write_sync_file(&path, b"different").is_err());
        assert_eq!(remote.read_if_exists(&path).unwrap().unwrap(), b"complete");
        assert_eq!(remote.list_sync_files(prefix).unwrap(), vec![path]);
    }
}
