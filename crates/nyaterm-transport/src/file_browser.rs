use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::bail;

use crate::{
    FileTransferEndpoint, LocalFileService, RemoteBinaryFile, RemoteFilePath, RemoteFileService,
    RemoteTextDocument, RemoteTextRevision, RemoteTextWriteResult, SftpAttributeUpdate,
    SftpFileEntry, SftpFileProperties, SftpRemoteTextFile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileBrowserBackendKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileBrowserCapabilities {
    pub ordinary_transfer: bool,
    pub create_symlink: bool,
    pub edit_attributes: bool,
    pub direct_external_open: bool,
}

impl FileBrowserCapabilities {
    pub const LOCAL: Self = Self {
        ordinary_transfer: false,
        create_symlink: false,
        edit_attributes: false,
        direct_external_open: true,
    };
    pub const REMOTE: Self = Self {
        ordinary_transfer: true,
        create_symlink: true,
        edit_attributes: true,
        direct_external_open: false,
    };
}

#[derive(Clone)]
pub enum FileBrowserService {
    Local(LocalFileService),
    Remote(Arc<RemoteFileService>),
}

impl std::fmt::Debug for FileBrowserService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("FileBrowserService")
            .field(&self.kind())
            .finish()
    }
}

impl FileBrowserService {
    pub fn parent_path(&self, path: &RemoteFilePath) -> anyhow::Result<RemoteFilePath> {
        match self {
            Self::Local(_) => Ok(RemoteFilePath::new(file_browser_parent(
                self.kind(),
                &path.display_path,
            ))),
            Self::Remote(_) => path.parent(),
        }
    }

    pub fn child_path(
        &self,
        directory: &RemoteFilePath,
        name: &str,
    ) -> anyhow::Result<RemoteFilePath> {
        anyhow::ensure!(
            valid_file_browser_child_name(self.kind(), name),
            "invalid file name"
        );
        match self {
            Self::Local(_) => Ok(RemoteFilePath::new(file_browser_join(
                self.kind(),
                &directory.display_path,
                name,
            ))),
            Self::Remote(service) => service.child_path(directory, name),
        }
    }

    pub fn copy_child_path(
        &self,
        directory: &RemoteFilePath,
        source_service: &Self,
        source: &RemoteFilePath,
        name: &str,
    ) -> anyhow::Result<RemoteFilePath> {
        match (self, source_service) {
            (Self::Remote(target), Self::Remote(origin)) => {
                target.copy_child_path(directory, origin, source, name)
            }
            _ => self.child_path(directory, name),
        }
    }

    /// Blocking destination validation. Resolve local aliases and reject remote
    /// symlink ancestors when two sessions address the same server directory.
    pub fn validate_copy_destination(
        &self,
        source: &RemoteFilePath,
        target: &Self,
        destination: &RemoteFilePath,
        directory: bool,
    ) -> anyhow::Result<()> {
        match (self, target) {
            (Self::Local(_), Self::Local(_)) => {
                let source = std::fs::canonicalize(resolve_local_path(&source.display_path))?;
                let destination = canonical_target(&resolve_local_path(&destination.display_path))?;
                anyhow::ensure!(
                    source != destination && !(directory && destination.starts_with(&source)),
                    "source and destination overlap"
                );
            }
            (Self::Remote(origin), Self::Remote(destination_service))
                if origin.endpoint_key() == destination_service.endpoint_key() =>
            {
                anyhow::ensure!(
                    !file_browser_copy_destination_invalid(
                        FileBrowserBackendKind::Remote,
                        &source.display_path,
                        &destination.display_path,
                        directory
                    ),
                    "source and destination overlap"
                );
                if directory {
                    for (service, path) in
                        [(self, source.parent()?), (target, destination.parent()?)]
                    {
                        let mut current = path;
                        loop {
                            let props = service.remote_file_properties(&current)?;
                            anyhow::ensure!(
                                props.file_type != crate::SftpFileType::Symlink,
                                "copying through symbolic link ancestors is not supported"
                            );
                            let parent = current.parent()?;
                            if parent == current {
                                break;
                            }
                            current = parent;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn create_dir_remote_path(
        &self,
        path: &RemoteFilePath,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => {
                service.create_dir(resolve_local_path(&path.display_path), mode)
            }
            Self::Remote(service) => service.create_dir_remote_path(path, mode),
        }
    }

    pub fn create_file_remote_path(
        &self,
        path: &RemoteFilePath,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => {
                service.create_file(resolve_local_path(&path.display_path), mode)
            }
            Self::Remote(service) => service.create_file_remote_path(path, mode),
        }
    }

    pub fn local() -> Self {
        Self::Local(LocalFileService)
    }

    pub fn remote(service: RemoteFileService) -> Self {
        Self::Remote(Arc::new(service))
    }

    pub fn kind(&self) -> FileBrowserBackendKind {
        match self {
            Self::Local(_) => FileBrowserBackendKind::Local,
            Self::Remote(_) => FileBrowserBackendKind::Remote,
        }
    }

    pub fn capabilities(&self) -> FileBrowserCapabilities {
        match self {
            Self::Local(_) => FileBrowserCapabilities::LOCAL,
            Self::Remote(_) => FileBrowserCapabilities::REMOTE,
        }
    }

    pub fn home_dir(&self) -> anyhow::Result<String> {
        match self {
            Self::Local(service) => service.home_dir(),
            Self::Remote(service) => service.home_dir(),
        }
    }

    pub fn list_dir(&self, path: impl AsRef<str>) -> anyhow::Result<Vec<SftpFileEntry>> {
        match self {
            Self::Local(service) => service.list_dir(resolve_local_path(path.as_ref())),
            Self::Remote(service) => service.list_dir(path),
        }
    }

    pub fn list_dir_path(&self, path: &RemoteFilePath) -> anyhow::Result<Vec<SftpFileEntry>> {
        match self {
            Self::Local(service) => service.list_dir(resolve_local_path(&path.display_path)),
            Self::Remote(service) => service.list_dir_path(path),
        }
    }

    pub fn create_dir_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.create_dir(resolve_local_path(path.as_ref()), mode),
            Self::Remote(service) => service.create_dir_path(path, mode),
        }
    }

    pub fn create_file_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.create_file(resolve_local_path(path.as_ref()), mode),
            Self::Remote(service) => service.create_file_path(path, mode),
        }
    }

    pub fn create_symlink_path(
        &self,
        link_path: impl AsRef<str>,
        target_path: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(_) => bail!("creating symbolic links is unavailable for local browsing"),
            Self::Remote(service) => service.create_symlink_path(link_path, target_path),
        }
    }

    pub fn delete_remote_path(&self, path: &RemoteFilePath) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.delete(resolve_local_path(&path.display_path)),
            Self::Remote(service) => service.delete_remote_path(path),
        }
    }

    pub fn rename_remote_paths(
        &self,
        old_path: &RemoteFilePath,
        new_path: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.rename(
                resolve_local_path(&old_path.display_path),
                resolve_local_path(&new_path.display_path),
            ),
            Self::Remote(service) => service.rename_remote_paths(old_path, new_path),
        }
    }

    /// Rename without replacing a concurrently-created destination.
    pub fn rename_remote_paths_no_replace(
        &self,
        source: &RemoteFilePath,
        destination: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(_) => rename_local_no_replace(
                &resolve_local_path(&source.display_path),
                &resolve_local_path(&destination.display_path),
            ),
            Self::Remote(service) => service.rename_remote_paths_no_replace(source, destination),
        }
    }

    pub fn remote_file_properties(
        &self,
        path: &RemoteFilePath,
    ) -> anyhow::Result<SftpFileProperties> {
        match self {
            Self::Local(service) => service.properties(resolve_local_path(&path.display_path)),
            Self::Remote(service) => service.remote_file_properties(path),
        }
    }

    pub fn update_remote_path_attributes(
        &self,
        path: &RemoteFilePath,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(_) => bail!("editing attributes is unavailable for local browsing"),
            Self::Remote(service) => service.update_remote_path_attributes(path, update),
        }
    }

    pub fn read_text_file_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        match self {
            Self::Local(service) => {
                service.read_text_file(resolve_local_path(&path.display_path), max_bytes)
            }
            Self::Remote(service) => service.read_text_file_path(path, max_bytes),
        }
    }

    pub fn read_text_document_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteTextDocument> {
        match self {
            Self::Local(service) => {
                service.read_text_document(resolve_local_path(&path.display_path), max_bytes)
            }
            Self::Remote(service) => service.read_text_document_path(path, max_bytes),
        }
    }

    pub fn write_text_document_path(
        &self,
        path: &RemoteFilePath,
        content: impl AsRef<str>,
        expected_revision: Option<&RemoteTextRevision>,
        force: bool,
    ) -> anyhow::Result<RemoteTextWriteResult> {
        match self {
            Self::Local(service) => service.write_text_document(
                resolve_local_path(&path.display_path),
                content,
                expected_revision,
                force,
            ),
            Self::Remote(service) => {
                service.write_text_document_path(path, content, expected_revision, force)
            }
        }
    }

    pub fn read_file_bytes_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        match self {
            Self::Local(service) => {
                service.read_file_bytes(resolve_local_path(&path.display_path), max_bytes)
            }
            Self::Remote(service) => service.read_file_bytes_path(path, max_bytes),
        }
    }

    pub fn transfer_endpoint(&self, path: &RemoteFilePath) -> FileTransferEndpoint {
        match self {
            Self::Local(_) => FileTransferEndpoint::Local(resolve_local_path(&path.display_path)),
            Self::Remote(service) => FileTransferEndpoint::Remote {
                service: service.clone(),
                path: path.clone(),
            },
        }
    }

    pub fn local_path(&self, path: &RemoteFilePath) -> Option<PathBuf> {
        matches!(self, Self::Local(_)).then(|| resolve_local_path(&path.display_path))
    }

    pub fn remote_service(&self) -> Option<Arc<RemoteFileService>> {
        match self {
            Self::Local(_) => None,
            Self::Remote(service) => Some(service.clone()),
        }
    }
}

fn resolve_local_path(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|current| current.join(&path))
            .unwrap_or(path)
    }
}

pub fn file_browser_name(kind: FileBrowserBackendKind, path: &str) -> String {
    match kind {
        FileBrowserBackendKind::Local => Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string()),
        FileBrowserBackendKind::Remote => path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .to_string(),
    }
}

pub fn file_browser_parent(kind: FileBrowserBackendKind, path: &str) -> String {
    match kind {
        FileBrowserBackendKind::Local => Path::new(path)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .filter(|parent| !parent.is_empty())
            .unwrap_or_else(|| path.to_string()),
        FileBrowserBackendKind::Remote => {
            let path = path.trim_end_matches('/');
            match path.rfind('/') {
                Some(0) => "/".to_string(),
                Some(index) => path[..index].to_string(),
                None => ".".to_string(),
            }
        }
    }
}

pub fn file_browser_join(kind: FileBrowserBackendKind, parent: &str, child: &str) -> String {
    match kind {
        FileBrowserBackendKind::Local => {
            Path::new(parent).join(child).to_string_lossy().into_owned()
        }
        FileBrowserBackendKind::Remote => {
            let absolute = parent.starts_with('/');
            let parent = parent.trim_end_matches('/');
            match parent {
                "" if absolute => format!("/{child}"),
                "" | "." => child.to_string(),
                parent => format!("{parent}/{child}"),
            }
        }
    }
}

pub fn file_browser_root(kind: FileBrowserBackendKind, path: &str) -> String {
    match kind {
        FileBrowserBackendKind::Remote => if path.starts_with('/') { "/" } else { "." }.to_string(),
        FileBrowserBackendKind::Local => {
            let path = Path::new(path);
            path.ancestors()
                .last()
                .filter(|root| !root.as_os_str().is_empty())
                .map(|root| root.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        }
    }
}

pub fn file_browser_identity(kind: FileBrowserBackendKind, path: &str) -> String {
    match kind {
        FileBrowserBackendKind::Remote => match posix_path_parts(path) {
            Some(parts) if parts.is_empty() => "/".to_string(),
            Some(parts) => format!("/{}", parts.join("/")),
            None => path.to_string(),
        },
        FileBrowserBackendKind::Local if cfg!(windows) => path.replace('/', "\\").to_lowercase(),
        FileBrowserBackendKind::Local => path.to_string(),
    }
}

pub fn file_browser_path_eq(kind: FileBrowserBackendKind, left: &str, right: &str) -> bool {
    file_browser_identity(kind, left) == file_browser_identity(kind, right)
}

/// Translate a POSIX working directory reported by a local Windows shell into
/// the Windows path shape the local file browser uses. Covers WSL's
/// `/mnt/<drive>` automounts, drive-letter OSC 7 payloads such as `/d:/x`, and
/// MSYS-style single-letter roots like `/d/x`.
///
/// Returns `None` when the path needs no translation. The rules are
/// deliberately platform-agnostic: only callers on a Windows host may apply
/// them, because `/mnt/...` and `/d/...` are ordinary directories elsewhere.
pub fn local_shell_cwd_to_browser_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    if let Some(tail) = rest.strip_prefix("mnt/") {
        return windows_path_for_drive(tail);
    }
    windows_path_for_drive(rest)
}

fn windows_path_for_drive(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let drive = chars.next()?;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let mut remainder = chars.as_str();
    if let Some(tail) = remainder.strip_prefix(':') {
        remainder = tail;
    } else if !remainder.is_empty() && !remainder.starts_with('/') {
        return None;
    }
    let mut mapped = String::with_capacity(path.len() + 2);
    mapped.push(drive.to_ascii_uppercase());
    mapped.push_str(":\\");
    mapped.push_str(
        remainder
            .strip_prefix('/')
            .unwrap_or(remainder)
            .replace('/', "\\")
            .as_str(),
    );
    Some(mapped)
}

pub fn file_browser_path_is_root(kind: FileBrowserBackendKind, path: &str) -> bool {
    file_browser_path_eq(kind, path, &file_browser_root(kind, path))
}

pub fn valid_file_browser_child_name(kind: FileBrowserBackendKind, name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && (kind == FileBrowserBackendKind::Remote || !cfg!(windows) || !name.contains('\\'))
}

/// True when copying `source` onto `destination` would overwrite the source, nest a
/// directory inside itself, or operate on a filesystem root.
///
/// Identity is backend-aware: remote paths stay POSIX, local Windows paths are
/// case-insensitive and accept both separators. Callers still have to compare
/// endpoints separately — equal strings on different sessions are not the same file.
pub fn file_browser_copy_destination_invalid(
    kind: FileBrowserBackendKind,
    source: &str,
    destination: &str,
    source_is_directory: bool,
) -> bool {
    if file_browser_path_is_root(kind, source) {
        return true;
    }
    if file_browser_path_eq(kind, source, destination) {
        return true;
    }
    if !source_is_directory {
        return false;
    }
    match kind {
        FileBrowserBackendKind::Remote => {
            let Some(source_parts) = posix_path_parts(source) else {
                return true;
            };
            let Some(destination_parts) = posix_path_parts(destination) else {
                return true;
            };
            destination_parts.starts_with(&source_parts)
        }
        FileBrowserBackendKind::Local => {
            let source_id = file_browser_identity(kind, source);
            let destination_id = file_browser_identity(kind, destination);
            let separator = if cfg!(windows) { '\\' } else { '/' };
            let prefix = if source_id.ends_with(separator) {
                source_id
            } else {
                format!("{source_id}{separator}")
            };
            destination_id.starts_with(&prefix)
        }
    }
}

fn posix_path_parts(path: &str) -> Option<Vec<&str>> {
    if !path.starts_with('/') || path.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::file_browser_identity;
    use super::{
        FileBrowserBackendKind, file_browser_copy_destination_invalid, file_browser_join,
        file_browser_name, file_browser_parent, file_browser_path_is_root, file_browser_root,
        local_shell_cwd_to_browser_path, valid_file_browser_child_name,
    };

    #[test]
    fn remote_paths_keep_posix_semantics() {
        assert_eq!(
            file_browser_join(FileBrowserBackendKind::Remote, "/", "tmp"),
            "/tmp"
        );
        assert_eq!(
            file_browser_parent(FileBrowserBackendKind::Remote, "/tmp"),
            "/"
        );
        assert_eq!(
            file_browser_name(FileBrowserBackendKind::Remote, "/tmp/a"),
            "a"
        );
        assert!(valid_file_browser_child_name(
            FileBrowserBackendKind::Remote,
            "a\\b"
        ));
        assert_eq!(
            file_browser_root(FileBrowserBackendKind::Remote, "/tmp/a"),
            "/"
        );
        assert!(file_browser_path_is_root(
            FileBrowserBackendKind::Remote,
            "/"
        ));
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Remote,
            "/src",
            "/src/./",
            false
        ));
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Remote,
            "/src",
            "/src/child/file",
            true
        ));
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Remote,
            "/",
            "/other",
            true
        ));
        assert!(!file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Remote,
            "/src",
            "/other/file",
            true
        ));
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Remote,
            "/src\0hidden",
            "/other",
            true
        ));
    }

    #[cfg(windows)]
    #[test]
    fn local_paths_keep_windows_roots_and_separators() {
        assert_eq!(
            file_browser_parent(FileBrowserBackendKind::Local, r"C:\tmp"),
            r"C:\"
        );
        assert_eq!(
            file_browser_join(FileBrowserBackendKind::Local, r"C:\", "tmp"),
            r"C:\tmp"
        );
        assert_eq!(
            file_browser_name(FileBrowserBackendKind::Local, r"C:\tmp\a"),
            "a"
        );
        assert!(!valid_file_browser_child_name(
            FileBrowserBackendKind::Local,
            "a\\b"
        ));
        assert_eq!(
            file_browser_root(FileBrowserBackendKind::Local, r"C:\tmp\a"),
            r"C:\"
        );
        assert!(file_browser_path_is_root(
            FileBrowserBackendKind::Local,
            r"C:\"
        ));
        assert_eq!(
            file_browser_identity(FileBrowserBackendKind::Local, r"C:\TMP\A"),
            file_browser_identity(FileBrowserBackendKind::Local, r"c:/tmp/a")
        );
        assert_eq!(
            file_browser_root(
                FileBrowserBackendKind::Local,
                r"\\server\share\folder\file.txt"
            ),
            r"\\server\share\"
        );
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Local,
            r"C:\src",
            r"C:\src\child",
            true
        ));
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Local,
            r"C:\src",
            r"c:/src/child",
            true
        ));
        assert!(file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Local,
            r"C:\",
            r"D:\other",
            true
        ));
        assert!(!file_browser_copy_destination_invalid(
            FileBrowserBackendKind::Local,
            r"C:\src",
            r"C:\other\file",
            true
        ));
    }

    #[test]
    fn local_shell_cwd_translation_covers_wsl_drive_and_msys_forms() {
        assert_eq!(
            local_shell_cwd_to_browser_path("/mnt/d/Codes/RustProjects"),
            Some(r"D:\Codes\RustProjects".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path("/mnt/c"),
            Some(r"C:\".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path("/mnt/d/"),
            Some(r"D:\".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path("/d:/Projects"),
            Some(r"D:\Projects".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path("/d/Projects"),
            Some(r"D:\Projects".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path("/D/Codes"),
            Some(r"D:\Codes".to_string())
        );
    }

    #[test]
    fn local_shell_cwd_translation_leaves_unrelated_paths_untouched() {
        assert_eq!(local_shell_cwd_to_browser_path("/mnt/backup/data"), None);
        assert_eq!(local_shell_cwd_to_browser_path("/mnt/"), None);
        assert_eq!(local_shell_cwd_to_browser_path("/mnt/1/x"), None);
        assert_eq!(local_shell_cwd_to_browser_path("/home/debian"), None);
        assert_eq!(local_shell_cwd_to_browser_path("/Debian/mnt/d/x"), None);
        assert_eq!(local_shell_cwd_to_browser_path(r"D:\Codes"), None);
        assert_eq!(local_shell_cwd_to_browser_path("relative/path"), None);
        assert_eq!(
            local_shell_cwd_to_browser_path("//wsl.localhost/Debian/home"),
            None
        );
    }
}

#[cfg(windows)]
fn rename_local_no_replace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    anyhow::ensure!(
        !source[..source.len() - 1].contains(&0)
            && !destination[..destination.len() - 1].contains(&0),
        "invalid path"
    );
    // std::fs::rename enables REPLACE_EXISTING on Windows. Pass zero flags so
    // another writer's destination is protected by the operating system.
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            0,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_local_no_replace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    // Both C strings remain alive through the syscall. The exclusive flag is
    // necessary: an existence preflight alone cannot protect another writer.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn rename_local_no_replace(_: &Path, _: &Path) -> anyhow::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "exclusive rename is unavailable on this platform",
    )
    .into())
}

#[cfg(test)]
mod operation_path_tests {
    use super::FileBrowserService;
    use crate::{RemoteFilePath, RemoteFileService, SshSessionConfig};
    use nyaterm_core::test_support::TestTempDir;

    #[test]
    fn opaque_copy_target_keeps_both_raw_parent_and_raw_name() {
        let service =
            FileBrowserService::remote(RemoteFileService::new(SshSessionConfig::default()));
        let parent = RemoteFilePath::from_raw("/target/�", b"/target/\xff");
        let source = RemoteFilePath::from_raw("/source/�", b"/source/\xfe");
        let target = service
            .copy_child_path(&parent, &service, &source, "�")
            .unwrap();
        assert_eq!(target.raw_path().unwrap().unwrap(), b"/target/\xff/\xfe");
        assert_eq!(
            service
                .parent_path(&target)
                .unwrap()
                .raw_path()
                .unwrap()
                .unwrap(),
            b"/target/\xff"
        );
        let child = service.child_path(&parent, "新.txt").unwrap();
        let mut expected = b"/target/\xff/".to_vec();
        expected.extend("新.txt".as_bytes());
        assert_eq!(child.raw_path().unwrap().unwrap(), expected);
    }

    #[test]
    fn opaque_copy_does_not_encode_a_lossy_parent_display() {
        let mut config = SshSessionConfig::default();
        config.sftp.filename_encoding = "GBK".into();
        let service = FileBrowserService::remote(RemoteFileService::new(config));
        let parent = RemoteFilePath::from_raw("/target/�", b"/target/\xff");
        let source = RemoteFilePath::from_raw("/source/�", b"/source/\xfe");
        let target = service
            .copy_child_path(&parent, &service, &source, "�")
            .unwrap();
        assert_eq!(target.raw_path().unwrap().unwrap(), b"/target/\xff/\xfe");
    }

    #[test]
    fn exclusive_rename_preserves_existing_file_and_directory() {
        let root = TestTempDir::new("nyaterm-exclusive-rename");
        std::fs::create_dir_all(root.path()).unwrap();
        let service = FileBrowserService::local();
        for directory in [false, true] {
            let source = root.join(if directory { "source-dir" } else { "source" });
            let target = root.join(if directory { "target-dir" } else { "target" });
            if directory {
                std::fs::create_dir(&source).unwrap();
                std::fs::create_dir(&target).unwrap();
            } else {
                std::fs::write(&source, b"source").unwrap();
                std::fs::write(&target, b"target").unwrap();
            }
            assert!(
                service
                    .rename_remote_paths_no_replace(
                        &RemoteFilePath::new(source.to_string_lossy()),
                        &RemoteFilePath::new(target.to_string_lossy())
                    )
                    .is_err()
            );
            assert!(source.exists());
            assert!(target.exists());
            if !directory {
                assert_eq!(std::fs::read(&target).unwrap(), b"target");
            }
        }
    }
}

fn canonical_target(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = absolute.as_path();
    let mut names = Vec::new();
    loop {
        match std::fs::canonicalize(current) {
            Ok(mut resolved) => {
                for name in names.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(error.into());
                };
                names.push(name.to_os_string());
                let Some(parent) = current.parent() else {
                    return Err(error.into());
                };
                current = parent;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
