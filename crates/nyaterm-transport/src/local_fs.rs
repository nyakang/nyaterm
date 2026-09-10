use std::fs;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, anyhow, bail};

use super::{
    RemoteBinaryFile, RemoteTextDocument, RemoteTextMetadata, RemoteTextRevision,
    RemoteTextWriteResult, SftpFileEntry, SftpFileProperties, SftpFileType, SftpRemoteTextFile,
    SftpWriteTextResult,
};

#[derive(Debug, Clone, Default)]
pub struct LocalFileService;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDirectoryChild {
    pub name: String,
    pub path: String,
    pub is_symlink: bool,
}

impl LocalFileService {
    pub fn home_dir(&self) -> anyhow::Result<String> {
        dirs::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .map(path_to_string)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| anyhow!("failed to determine local home directory"))
    }

    pub fn list_dir(&self, path: impl AsRef<Path>) -> anyhow::Result<Vec<SftpFileEntry>> {
        let path = path.as_ref();
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)
            .with_context(|| format!("failed to list local directory '{}'", path.display()))?
        {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.is_empty() {
                continue;
            }
            if let Ok(file_entry) = file_entry_from_path(&entry.path(), name) {
                entries.push(file_entry);
            }
        }
        entries.sort_by(|left, right| {
            (left.file_type != SftpFileType::Directory)
                .cmp(&(right.file_type != SftpFileType::Directory))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }

    pub fn list_child_directories(
        &self,
        path: impl AsRef<Path>,
        show_hidden_files: bool,
    ) -> anyhow::Result<Vec<LocalDirectoryChild>> {
        let path = path.as_ref();
        let mut entries = Vec::new();
        for entry in fs::read_dir(path).with_context(|| {
            format!(
                "failed to list local child directories '{}'",
                path.display()
            )
        })? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            if !show_hidden_files && name.starts_with('.') {
                continue;
            }
            let entry_path = entry.path();
            let symlink_metadata = match fs::symlink_metadata(&entry_path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let metadata = fs::metadata(&entry_path).unwrap_or_else(|_| symlink_metadata.clone());
            if !metadata.is_dir() {
                continue;
            }
            entries.push(LocalDirectoryChild {
                name,
                path: path_to_string(entry_path),
                is_symlink: symlink_metadata.file_type().is_symlink(),
            });
        }
        entries.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }

    pub fn create_file(&self, path: impl AsRef<Path>, mode: Option<u32>) -> anyhow::Result<()> {
        let path = path.as_ref();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("failed to create local file '{}'", path.display()))?;
        set_local_mode_if_supported(path, mode)?;
        Ok(())
    }

    pub fn create_dir(&self, path: impl AsRef<Path>, mode: Option<u32>) -> anyhow::Result<()> {
        let path = path.as_ref();
        fs::create_dir(path)
            .with_context(|| format!("failed to create local directory '{}'", path.display()))?;
        set_local_mode_if_supported(path, mode)?;
        Ok(())
    }

    pub fn rename(
        &self,
        old_path: impl AsRef<Path>,
        new_path: impl AsRef<Path>,
    ) -> anyhow::Result<()> {
        let old_path = old_path.as_ref();
        let new_path = new_path.as_ref();
        ensure_safe_local_target(old_path)?;
        ensure_safe_local_target(new_path)?;
        if old_path.is_dir() && new_path.starts_with(old_path) {
            bail!("cannot move a local directory into itself");
        }
        fs::rename(old_path, new_path).with_context(|| {
            format!(
                "failed to rename local path '{}' to '{}'",
                old_path.display(),
                new_path.display()
            )
        })
    }

    pub fn delete(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        ensure_safe_local_target(path)?;
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect local path '{}'", path.display()))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(path)
                .with_context(|| format!("failed to delete local directory '{}'", path.display()))
        } else {
            fs::remove_file(path)
                .with_context(|| format!("failed to delete local file '{}'", path.display()))
        }
    }

    pub fn read_file_bytes(
        &self,
        path: impl AsRef<Path>,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect local file '{}'", path.display()))?;
        if metadata.is_dir() {
            bail!("cannot read a directory as bytes");
        }
        if metadata.len() > max_bytes {
            bail!("file is too large to preview ({} bytes)", metadata.len());
        }
        let content_bytes = fs::read(path)
            .with_context(|| format!("failed to read local file '{}'", path.display()))?;
        if content_bytes.len() as u64 > max_bytes {
            bail!("file grew beyond the preview limit while being read");
        }
        Ok(RemoteBinaryFile {
            path: path_to_string(path),
            size: content_bytes.len() as u64,
            content_bytes,
            modified_at: modified_time_secs(&metadata),
        })
    }

    pub fn read_text_document(
        &self,
        path: impl AsRef<Path>,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteTextDocument> {
        let file = self.read_file_bytes(path, max_bytes)?;
        if file.content_bytes.contains(&0) {
            bail!("binary files are not supported by the built-in editor");
        }
        let content = String::from_utf8(file.content_bytes.clone())
            .map_err(|_| anyhow!("file is not valid UTF-8 text"))?;
        let metadata = RemoteTextMetadata {
            size: file.size,
            modified_at: Some(file.modified_at),
        };
        Ok(RemoteTextDocument {
            path: file.path,
            content,
            revision: RemoteTextRevision::from_bytes(&file.content_bytes, metadata),
        })
    }

    pub fn write_text_document(
        &self,
        path: impl AsRef<Path>,
        content: impl AsRef<str>,
        expected_revision: Option<&RemoteTextRevision>,
        force: bool,
    ) -> anyhow::Result<RemoteTextWriteResult> {
        let path = path.as_ref();
        if !force && expected_revision.is_none() {
            bail!("local text revision is required for a safe save");
        }
        if !force {
            let metadata = fs::metadata(path)
                .with_context(|| format!("failed to inspect local file '{}'", path.display()))?;
            if metadata.is_dir() {
                bail!("cannot write a directory as text");
            }
            let bytes = fs::read(path)
                .with_context(|| format!("failed to read local file '{}'", path.display()))?;
            let current = RemoteTextRevision::from_bytes(
                &bytes,
                RemoteTextMetadata {
                    size: bytes.len() as u64,
                    modified_at: Some(modified_time_secs(&metadata)),
                },
            );
            if expected_revision != Some(&current) {
                return Ok(RemoteTextWriteResult::Conflict);
            }
        }
        fs::write(path, content.as_ref())
            .with_context(|| format!("failed to write local file '{}'", path.display()))?;
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect local file '{}'", path.display()))?;
        let bytes = content.as_ref().as_bytes();
        Ok(RemoteTextWriteResult::Saved {
            revision: RemoteTextRevision::from_bytes(
                bytes,
                RemoteTextMetadata {
                    size: bytes.len() as u64,
                    modified_at: Some(modified_time_secs(&metadata)),
                },
            ),
        })
    }

    pub fn properties(&self, path: impl AsRef<Path>) -> anyhow::Result<SftpFileProperties> {
        file_properties_from_path(path.as_ref())
    }

    pub fn read_text_file(
        &self,
        path: impl AsRef<Path>,
        max_bytes: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect local file '{}'", path.display()))?;
        if metadata.is_dir() {
            bail!("cannot read a directory as text");
        }
        if metadata.len() > max_bytes {
            bail!("file is too large to open ({} bytes)", metadata.len());
        }
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read local file '{}'", path.display()))?;
        let content =
            String::from_utf8(bytes).map_err(|_| anyhow!("file is not valid UTF-8 text"))?;
        Ok(SftpRemoteTextFile {
            path: path_to_string(path),
            content,
            size: metadata.len(),
            modified_at: modified_time_secs(&metadata),
        })
    }

    pub fn write_text_file(
        &self,
        path: impl AsRef<Path>,
        content: &str,
        expected_modified_at: Option<u64>,
        expected_size: Option<u64>,
        force: bool,
    ) -> anyhow::Result<SftpWriteTextResult> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect local file '{}'", path.display()))?;
        let current_modified_at = modified_time_secs(&metadata);
        let current_size = metadata.len();
        let has_conflict = expected_modified_at
            .is_some_and(|modified_at| modified_at != current_modified_at)
            || expected_size.is_some_and(|size| size != current_size);
        if has_conflict && !force {
            return Ok(SftpWriteTextResult::Conflict {
                modified_at: current_modified_at,
                size: current_size,
            });
        }
        fs::write(path, content)
            .with_context(|| format!("failed to write local file '{}'", path.display()))?;
        let next_metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect local file '{}'", path.display()))?;
        Ok(SftpWriteTextResult::Saved {
            modified_at: modified_time_secs(&next_metadata),
            size: next_metadata.len(),
        })
    }
}

fn ensure_safe_local_target(path: &Path) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() {
        bail!("refusing to operate on an empty local path");
    }
    let mut meaningful = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => meaningful = true,
            Component::ParentDir | Component::CurDir => {
                bail!("refusing to operate on a relative traversal path")
            }
            Component::Prefix(_) | Component::RootDir => {}
        }
    }
    if !meaningful {
        bail!("refusing to operate on a local filesystem root");
    }
    Ok(())
}

fn file_entry_from_path(path: &Path, name: String) -> anyhow::Result<SftpFileEntry> {
    let symlink_metadata = fs::symlink_metadata(path)?;
    let metadata = fs::metadata(path).unwrap_or_else(|_| symlink_metadata.clone());
    Ok(SftpFileEntry {
        name,
        path: path_to_string(path),
        file_type: file_type_from_metadata(&metadata, &symlink_metadata),
        size: (!metadata.is_dir()).then_some(metadata.len()),
        permissions: permissions_mode(&metadata),
        owner: owner_string(&metadata),
        group: group_string(&metadata),
        modified_at: modified_time_secs(&metadata).try_into().ok(),
        raw_path_token: Some(local_path_identity(path)),
        symlink_target_is_directory: symlink_metadata.file_type().is_symlink() && metadata.is_dir(),
    })
}

fn file_properties_from_path(path: &Path) -> anyhow::Result<SftpFileProperties> {
    let symlink_metadata = fs::symlink_metadata(path)?;
    let metadata = fs::metadata(path).unwrap_or_else(|_| symlink_metadata.clone());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path_to_string(path));
    Ok(SftpFileProperties {
        symlink_target: fs::read_link(path)
            .ok()
            .map(|target| path_to_string(&target)),
        name,
        path: path_to_string(path),
        file_type: file_type_from_metadata(&metadata, &symlink_metadata),
        size: (!metadata.is_dir()).then_some(metadata.len()),
        permissions: permissions_mode(&metadata),
        permissions_symbolic: permissions_string(&metadata, metadata.is_dir()),
        owner: owner_string(&metadata),
        group: group_string(&metadata),
        uid: uid(&metadata),
        gid: gid(&metadata),
        modified_at: modified_time_secs(&metadata).try_into().ok(),
        accessed_at: accessed_time_secs(&metadata).try_into().ok(),
        raw_path_token: Some(local_path_identity(path)),
        symlink_target_is_directory: symlink_metadata.file_type().is_symlink() && metadata.is_dir(),
    })
}

fn file_type_from_metadata(
    metadata: &fs::Metadata,
    symlink_metadata: &fs::Metadata,
) -> SftpFileType {
    if symlink_metadata.file_type().is_symlink() {
        SftpFileType::Symlink
    } else if metadata.is_dir() {
        SftpFileType::Directory
    } else if metadata.is_file() {
        SftpFileType::File
    } else {
        SftpFileType::Other
    }
}

fn path_to_string(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().to_string()
}

fn local_path_identity(path: &Path) -> String {
    let path = path_to_string(path);
    #[cfg(windows)]
    let path = path.to_lowercase();
    format!("local-path:{path}")
}

fn system_time_secs(time: std::io::Result<std::time::SystemTime>) -> u64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

fn modified_time_secs(metadata: &fs::Metadata) -> u64 {
    system_time_secs(metadata.modified())
}

fn accessed_time_secs(metadata: &fs::Metadata) -> u64 {
    system_time_secs(metadata.accessed())
}

#[cfg(unix)]
fn permissions_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;

    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn permissions_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn permissions_string(metadata: &fs::Metadata, is_dir: bool) -> String {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = metadata.permissions().mode();
    let mut output = String::with_capacity(10);
    output.push(if is_dir { 'd' } else { '-' });
    for (read, write, exec) in [
        (0o400, 0o200, 0o100),
        (0o040, 0o020, 0o010),
        (0o004, 0o002, 0o001),
    ] {
        output.push(if mode & read != 0 { 'r' } else { '-' });
        output.push(if mode & write != 0 { 'w' } else { '-' });
        output.push(if mode & exec != 0 { 'x' } else { '-' });
    }
    output
}

#[cfg(not(unix))]
fn permissions_string(metadata: &fs::Metadata, is_dir: bool) -> String {
    let mut output = String::from(if is_dir { "d" } else { "-" });
    output.push_str(if metadata.permissions().readonly() {
        "r-xr-xr-x"
    } else {
        "rwxrwxrwx"
    });
    output
}

#[cfg(unix)]
fn uid(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;

    Some(metadata.uid())
}

#[cfg(not(unix))]
fn uid(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn gid(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;

    Some(metadata.gid())
}

#[cfg(not(unix))]
fn gid(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

fn owner_string(metadata: &fs::Metadata) -> String {
    uid(metadata).map(|uid| uid.to_string()).unwrap_or_default()
}

fn group_string(metadata: &fs::Metadata) -> String {
    gid(metadata).map(|gid| gid.to_string()).unwrap_or_default()
}

#[cfg(unix)]
fn set_local_mode_if_supported(path: &Path, mode: Option<u32>) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(mode) = mode else {
        return Ok(());
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to set permissions for '{}'", path.display()))
}

#[cfg(not(unix))]
fn set_local_mode_if_supported(_path: &Path, _mode: Option<u32>) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::LocalFileService;
    use crate::{RemoteTextWriteResult, SftpFileType, SftpWriteTextResult};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nyaterm-local-fs-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn local_service_lists_and_mutates_files() {
        let service = LocalFileService;
        let root = temp_dir("mutates");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("one.txt");
        service.create_file(&file, None).unwrap();
        service
            .write_text_file(&file, "hello", None, None, true)
            .unwrap();

        let entries = service.list_dir(&root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "one.txt");
        assert_eq!(entries[0].file_type, SftpFileType::File);

        let renamed = root.join("two.txt");
        service.rename(&file, &renamed).unwrap();
        assert_eq!(
            service.read_text_file(&renamed, 1024).unwrap().content,
            "hello"
        );

        service.delete(&renamed).unwrap();
        assert!(service.list_dir(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_service_detects_write_conflicts() {
        let service = LocalFileService;
        let root = temp_dir("conflict");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("doc.txt");
        std::fs::write(&file, "old").unwrap();
        let loaded = service.read_text_file(&file, 1024).unwrap();
        std::fs::write(&file, "newer").unwrap();

        let result = service
            .write_text_file(
                &file,
                "mine",
                Some(loaded.modified_at),
                Some(loaded.size),
                false,
            )
            .unwrap();
        assert!(matches!(result, SftpWriteTextResult::Conflict { .. }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_document_revision_detects_same_size_changes() {
        let service = LocalFileService;
        let root = temp_dir("revision");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("doc.txt");
        std::fs::write(&file, "same").unwrap();
        let loaded = service.read_text_document(&file, 1024).unwrap();
        std::fs::write(&file, "diff").unwrap();

        assert_eq!(
            service
                .write_text_document(&file, "mine", Some(&loaded.revision), false)
                .unwrap(),
            RemoteTextWriteResult::Conflict
        );
        assert!(matches!(
            service
                .write_text_document(&file, "mine", Some(&loaded.revision), true)
                .unwrap(),
            RemoteTextWriteResult::Saved { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_service_rejects_filesystem_root_deletion() {
        let service = LocalFileService;
        let root = std::path::PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
        assert!(service.delete(root).is_err());
    }
}
