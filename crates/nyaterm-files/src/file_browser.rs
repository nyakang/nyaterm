use crate::local_fs::LocalFileService;
use crate::remote_file::{
    FileCopyRequest, FileTransferEndpoint, RemoteFileService, RemoteTextDocument,
    RemoteTextRevision, RemoteTextWriteResult,
};
use crate::sftp::{
    RemoteBinaryFile, RemoteFilePath, SftpAttributeUpdate, SftpFileEntry, SftpFileProperties,
    SftpRemoteTextFile, SftpTransferControl,
};
use crate::sftp_transfer_types::{
    SftpPathTransferOptions, SftpTransferProgress, SftpTransferSummary,
};
use anyhow::bail;
use nyaterm_transport::local_shell::LocalShellPathStyle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

#[derive(Clone)]
pub enum WeakFileBrowserService {
    Remote(Weak<RemoteFileService>),
    Adb(Weak<crate::adb::AdbFileService>),
}

impl std::fmt::Debug for WeakFileBrowserService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("WeakFileBrowserService")
            .field(&self.kind())
            .finish()
    }
}

impl From<Weak<RemoteFileService>> for WeakFileBrowserService {
    fn from(service: Weak<RemoteFileService>) -> Self {
        Self::Remote(service)
    }
}

impl WeakFileBrowserService {
    pub fn kind(&self) -> FileBrowserBackendKind {
        match self {
            Self::Remote(_) => FileBrowserBackendKind::Remote,
            Self::Adb(_) => FileBrowserBackendKind::Adb,
        }
    }
    pub fn upgrade(&self) -> Option<FileBrowserService> {
        match self {
            Self::Remote(service) => service.upgrade().map(FileBrowserService::Remote),
            Self::Adb(service) => service
                .upgrade()
                .filter(|s| !s.control.is_cancelled() && s.runtime.is_valid())
                .map(FileBrowserService::Adb),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileBrowserBackendKind {
    Local,
    Remote,
    Adb,
}

/// Entry failures have stable codes and never retain paths or picker diagnostics.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum FileTransferEntryError {
    #[error("file transfer source is unavailable")]
    Unavailable,
    #[error("file transfer backend is unsupported")]
    UnsupportedBackend,
    #[error("file transfer destination is invalid")]
    InvalidDestination,
    #[error("file picker failed")]
    PickerFailed,
    #[error("file picker closed without a response")]
    PickerClosed,
    #[error("file picker is already open")]
    PickerAlreadyOpen,
    #[error("remote drag export is unsupported on this platform")]
    DragPlatformUnsupported,
    #[error("select files before starting a transfer")]
    NoSelection,
    #[error("links and special files cannot be transferred by this operation")]
    UnsupportedFileType,
    #[error("the source changed during transfer")]
    SourceChanged,
    #[error("file transfer exceeded its resource limit")]
    LimitExceeded,
    #[error("the promised destination already exists")]
    TargetExists,
}

impl FileTransferEntryError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "SourceUnavailable",
            Self::UnsupportedBackend => "UnsupportedBackend",
            Self::InvalidDestination => "InvalidDestination",
            Self::PickerFailed => "PickerFailed",
            Self::PickerClosed => "PickerClosed",
            Self::PickerAlreadyOpen => "PickerAlreadyOpen",
            Self::DragPlatformUnsupported => "DragPlatformUnsupported",
            Self::NoSelection => "NoSelection",
            Self::UnsupportedFileType => "UnsupportedFileType",
            Self::SourceChanged => "SourceChanged",
            Self::LimitExceeded => "LimitExceeded",
            Self::TargetExists => "TargetExists",
        }
    }
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
    Adb(Arc<crate::adb::AdbFileService>),
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
    pub fn is_sudo(&self) -> bool {
        matches!(self, Self::Remote(service) if service.access_mode() == crate::sftp::sudo::FileAccessMode::SudoRoot)
    }

    pub fn same_execution_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Remote(left), Self::Remote(right)) => left.same_execution_identity(right),
            (Self::Local(_), Self::Local(_)) | (Self::Adb(_), Self::Adb(_)) => true,
            _ => false,
        }
    }
    pub fn downgrade(&self) -> Option<WeakFileBrowserService> {
        match self {
            Self::Local(_) => None,
            Self::Remote(service) => Some(WeakFileBrowserService::Remote(Arc::downgrade(service))),
            Self::Adb(service) => Some(WeakFileBrowserService::Adb(Arc::downgrade(service))),
        }
    }
    pub fn download_remote_path_with_progress_and_path_options(
        &self,
        remote: &RemoteFilePath,
        local: PathBuf,
        control: SftpTransferControl,
        options: SftpPathTransferOptions,
        progress: impl FnMut(SftpTransferProgress) + Send + 'static,
    ) -> anyhow::Result<SftpTransferSummary> {
        if let Self::Remote(service) = self {
            return service.download_remote_path_with_progress_and_path_options(
                remote, local, control, options, progress,
            );
        }
        control.check_cancelled()?;
        let local = std::path::absolute(local)?;
        if let Some(parent) = local.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let summary = FileCopyRequest {
            source: self.transfer_endpoint(remote),
            destination: FileTransferEndpoint::Local(local.clone()),
            control,
            options,
        }
        .execute_with_progress(progress)?;
        Ok(SftpTransferSummary {
            remote_path: remote.display_path.clone(),
            local_path: PathBuf::from(summary.destination_path),
            bytes: summary.bytes,
            skipped: !summary.copied,
        })
    }

    pub fn upload_path_with_progress_and_path_options(
        &self,
        local: PathBuf,
        remote: &str,
        control: SftpTransferControl,
        options: SftpPathTransferOptions,
        progress: impl FnMut(SftpTransferProgress) + Send + 'static,
    ) -> anyhow::Result<SftpTransferSummary> {
        self.upload_remote_path_with_progress_and_path_options(
            local,
            &RemoteFilePath::new(remote),
            control,
            options,
            progress,
        )
    }

    pub fn upload_remote_path_with_progress_and_path_options(
        &self,
        local: PathBuf,
        remote: &RemoteFilePath,
        control: SftpTransferControl,
        options: SftpPathTransferOptions,
        progress: impl FnMut(SftpTransferProgress) + Send + 'static,
    ) -> anyhow::Result<SftpTransferSummary> {
        if let Self::Remote(service) = self {
            return service.upload_path_with_progress_and_path_options(
                local,
                &remote.display_path,
                control,
                options,
                progress,
            );
        }
        let summary = FileCopyRequest {
            source: FileTransferEndpoint::Local(local.clone()),
            destination: self.transfer_endpoint(remote),
            control,
            options,
        }
        .execute_with_progress(progress)?;
        Ok(SftpTransferSummary {
            remote_path: summary.destination_path,
            local_path: local,
            bytes: summary.bytes,
            skipped: !summary.copied,
        })
    }

    pub fn parent_path(&self, path: &RemoteFilePath) -> anyhow::Result<RemoteFilePath> {
        match self {
            Self::Local(_) => Ok(RemoteFilePath::new(file_browser_parent(
                self.kind(),
                &path.display_path,
            ))),
            Self::Remote(_) | Self::Adb(_) => path.parent(),
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
            Self::Adb(service) => service
                .child_path(directory, name)
                .map_err(crate::adb::file_error),
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
                                props.file_type != crate::sftp::SftpFileType::Symlink,
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
            (Self::Adb(origin), Self::Adb(target)) => origin
                .validate_copy_destination(source, target, destination, directory)
                .map_err(crate::adb::file_error)?,
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
            Self::Adb(service) => service
                .create_dir_remote_path(path, mode)
                .map_err(crate::adb::file_error),
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
            Self::Adb(service) => service
                .create_file_remote_path(path, mode)
                .map_err(crate::adb::file_error),
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
            Self::Adb(_) => FileBrowserBackendKind::Adb,
        }
    }

    pub fn capabilities(&self) -> FileBrowserCapabilities {
        match self {
            Self::Local(_) => FileBrowserCapabilities::LOCAL,
            Self::Remote(_) => FileBrowserCapabilities::REMOTE,
            Self::Adb(_) => FileBrowserCapabilities {
                ordinary_transfer: true,
                create_symlink: false,
                edit_attributes: false,
                direct_external_open: false,
            },
        }
    }

    pub fn home_dir(&self) -> anyhow::Result<String> {
        match self {
            Self::Local(service) => service.home_dir(),
            Self::Remote(service) => service.home_dir(),
            Self::Adb(_) => Ok("/sdcard/".into()),
        }
    }

    pub fn list_dir(&self, path: impl AsRef<str>) -> anyhow::Result<Vec<SftpFileEntry>> {
        match self {
            Self::Local(service) => service.list_dir(resolve_local_path(path.as_ref())),
            Self::Remote(service) => service.list_dir(path),
            Self::Adb(service) => service
                .list_dir_path(&RemoteFilePath::new(path.as_ref()))
                .map_err(crate::adb::file_error),
        }
    }

    pub fn list_dir_path(&self, path: &RemoteFilePath) -> anyhow::Result<Vec<SftpFileEntry>> {
        match self {
            Self::Local(service) => service.list_dir(resolve_local_path(&path.display_path)),
            Self::Remote(service) => service.list_dir_path(path),
            Self::Adb(service) => service.list_dir_path(path).map_err(crate::adb::file_error),
        }
    }

    pub fn create_dir_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.create_dir(resolve_local_path(path.as_ref()), mode),
            Self::Remote(service) => service.create_dir_path(path, mode),
            Self::Adb(service) => service
                .create_dir_remote_path(&RemoteFilePath::new(path.as_ref()), mode)
                .map_err(crate::adb::file_error),
        }
    }

    pub fn create_file_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.create_file(resolve_local_path(path.as_ref()), mode),
            Self::Remote(service) => service.create_file_path(path, mode),
            Self::Adb(service) => service
                .create_file_remote_path(&RemoteFilePath::new(path.as_ref()), mode)
                .map_err(crate::adb::file_error),
        }
    }

    pub fn create_symlink_path(
        &self,
        link_path: impl AsRef<str>,
        target_path: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        match self {
            Self::Adb(_) | Self::Local(_) => {
                bail!("creating symbolic links is unavailable for this backend")
            }
            Self::Remote(service) => service.create_symlink_path(link_path, target_path),
        }
    }

    pub fn delete_remote_path(&self, path: &RemoteFilePath) -> anyhow::Result<()> {
        match self {
            Self::Local(service) => service.delete(resolve_local_path(&path.display_path)),
            Self::Remote(service) => service.delete_remote_path(path),
            Self::Adb(service) => service
                .delete_remote_path(path)
                .map_err(crate::adb::file_error),
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
            Self::Adb(service) => service
                .rename_remote_paths(old_path, new_path)
                .map_err(crate::adb::file_error),
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
            Self::Adb(service) => service
                .rename_remote_paths_no_replace(source, destination)
                .map_err(crate::adb::file_error),
        }
    }

    pub fn remote_file_properties(
        &self,
        path: &RemoteFilePath,
    ) -> anyhow::Result<SftpFileProperties> {
        match self {
            Self::Local(service) => service.properties(resolve_local_path(&path.display_path)),
            Self::Remote(service) => service.remote_file_properties(path),
            Self::Adb(service) => service
                .remote_file_properties(path)
                .map_err(crate::adb::file_error),
        }
    }

    pub fn update_remote_path_attributes(
        &self,
        path: &RemoteFilePath,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        match self {
            Self::Adb(_) | Self::Local(_) => {
                bail!("editing attributes is unavailable for this backend")
            }
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
            Self::Adb(service) => service
                .read_text_file_path(path, max_bytes)
                .map_err(crate::adb::file_error),
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
            Self::Adb(service) => service
                .read_text_document_path(path, max_bytes)
                .map_err(crate::adb::file_error),
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
            Self::Adb(service) => service
                .write_text_document_path(path, content, expected_revision, force)
                .map_err(crate::adb::file_error),
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
            Self::Adb(service) => service
                .read_file_bytes_path(path, max_bytes)
                .map_err(crate::adb::file_error),
        }
    }

    pub fn transfer_endpoint(&self, path: &RemoteFilePath) -> FileTransferEndpoint {
        match self {
            Self::Local(_) => FileTransferEndpoint::Local(resolve_local_path(&path.display_path)),
            Self::Adb(service) => FileTransferEndpoint::Adb {
                service: service.clone(),
                path: path.clone(),
            },
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
            Self::Adb(_) | Self::Local(_) => None,
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
        FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => path
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
        FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
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
        FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
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
        FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
            if path.starts_with('/') { "/" } else { "." }.to_string()
        }
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
        FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
            match posix_path_parts(path) {
                Some(parts) if parts.is_empty() => "/".to_string(),
                Some(parts) => format!("/{}", parts.join("/")),
                None => path.to_string(),
            }
        }
        FileBrowserBackendKind::Local if cfg!(windows) => {
            let path = path.replace('/', "\\");
            wsl_unc_identity(&path).unwrap_or_else(|| path.to_lowercase())
        }
        FileBrowserBackendKind::Local => path.to_string(),
    }
}

// Host/share names use Windows semantics, but the guest path belongs to Linux.
// Canonicalize both WSL aliases and the verbatim form returned by canonicalize.
fn wsl_unc_identity(path: &str) -> Option<String> {
    let unc = if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        &path[8..]
    } else {
        path.strip_prefix(r"\\")?
    };
    let (host, rest) = unc.split_once('\\')?;
    if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
        return None;
    }
    let (distro, guest) = rest.split_once('\\').unwrap_or((rest, ""));
    if distro.is_empty() {
        return None;
    }
    let mut identity = format!(r"\\wsl.localhost\{}", distro.to_lowercase());
    // A share root with and without its final separator denotes the same root.
    if !guest.is_empty() {
        identity.push('\\');
        identity.push_str(guest);
    }
    Some(identity)
}

pub fn file_browser_path_eq(kind: FileBrowserBackendKind, left: &str, right: &str) -> bool {
    file_browser_identity(kind, left) == file_browser_identity(kind, right)
}

/// The path namespace of the actual local launcher, not the host platform.
/// Map only paths belonging to the launcher's namespace. Call on Windows hosts
/// at the file-browser boundary; retain the original cwd in session state.
pub fn local_shell_cwd_to_browser_path(style: LocalShellPathStyle, path: &str) -> Option<String> {
    match style {
        LocalShellPathStyle::Wsl => {
            valid_wsl_browser_cwd(path).then_some(())?;
            let drive_path = path.strip_prefix("/mnt/")?;
            if drive_path.as_bytes().get(1) == Some(&b':') {
                return None;
            }
            windows_path_for_drive(drive_path)
        }
        LocalShellPathStyle::Msys => windows_path_for_drive(path.strip_prefix('/')?),
        LocalShellPathStyle::Native => {
            let rest = path.strip_prefix('/')?;
            (rest.as_bytes().get(1) == Some(&b':')).then_some(())?;
            windows_path_for_drive(rest)
        }
    }
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
        if !remainder.is_empty() && !remainder.starts_with('/') {
            return None;
        }
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

/// Map a WSL cwd that has no drive-letter form (`/mnt`, `/home`, ...) onto the
/// `\\wsl.localhost\<distro>` namespace the Windows host can actually list.
/// Validate before mapping so Windows cannot reshape a guest path into another
/// directory. The drive-letter entry point applies the same validation.
fn wsl_unc_browser_path(distro: &str, path: &str) -> Option<String> {
    if !valid_windows_browser_component(distro) || !valid_wsl_browser_cwd(path) {
        return None;
    }
    let rest = path.strip_prefix('/')?;
    let mut mapped = String::with_capacity(distro.len() + path.len() + 16);
    mapped.push_str(r"\\wsl.localhost\");
    mapped.push_str(distro);
    if !rest.is_empty() {
        mapped.push('\\');
        mapped.push_str(&rest.replace('/', "\\"));
    }
    Some(mapped)
}

fn valid_wsl_browser_cwd(path: &str) -> bool {
    path.starts_with('/')
        && path
            .split('/')
            .filter(|part| !part.is_empty())
            .all(valid_windows_browser_component)
}

fn valid_windows_browser_component(component: &str) -> bool {
    if component.is_empty()
        || component.ends_with([' ', '.'])
        || component
            .chars()
            .any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch))
    {
        return false;
    }
    // Win32 also reserves device names when followed by an extension, and
    // recognizes superscript digits in COM/LPT device names.
    let stem = component.split('.').next().unwrap_or_default();
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

/// Drive-letter translation first, then the WSL UNC namespace for paths the
/// drive mapping cannot represent. Callers on non-Windows hosts must keep the
/// original cwd (`/mnt` and `/home` are ordinary directories there).
pub fn local_shell_cwd_to_browser_path_with_distro(
    style: LocalShellPathStyle,
    path: &str,
    wsl_distro: Option<&str>,
) -> Option<String> {
    match style {
        LocalShellPathStyle::Wsl => local_shell_cwd_to_browser_path(style, path)
            .or_else(|| wsl_unc_browser_path(wsl_distro.unwrap_or_default(), path)),
        _ => local_shell_cwd_to_browser_path(style, path),
    }
}

pub fn file_browser_path_is_root(kind: FileBrowserBackendKind, path: &str) -> bool {
    file_browser_path_eq(kind, path, &file_browser_root(kind, path))
}

pub fn valid_file_browser_child_name(kind: FileBrowserBackendKind, name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
        && (kind != FileBrowserBackendKind::Local
            || !cfg!(windows)
            || (!name.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c))
                && !name.ends_with([' ', '.'])))
}

/// True when copying `source` onto `destination` would overwrite the source, nest a
/// directory inside itself, or operate on a filesystem root.
///
/// Identity is backend-aware: remote and WSL guest paths preserve case; other
/// local Windows paths are case-insensitive and accept both separators. Callers
/// still have to compare endpoints separately — equal strings on different
/// sessions are not the same file.
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
        FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
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
    use super::LocalShellPathStyle;
    #[cfg(windows)]
    use crate::file_browser::file_browser_identity;
    #[cfg(windows)]
    use crate::file_browser::file_browser_path_eq;
    use crate::file_browser::{
        FileBrowserBackendKind, file_browser_copy_destination_invalid, file_browser_join,
        file_browser_name, file_browser_parent, file_browser_path_is_root, file_browser_root,
        local_shell_cwd_to_browser_path, local_shell_cwd_to_browser_path_with_distro,
        valid_file_browser_child_name,
    };

    #[test]
    fn local_shell_namespaces_do_not_alias_linux_directories_to_windows_drives() {
        for path in ["/d", "/d/Projects", "/d:/Projects", "/mnt/d:/Projects"] {
            assert_eq!(
                local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, path),
                None
            );
        }
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/mnt/d/Projects"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Native, "/d/Projects"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/d:relative"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/d"),
            Some(r"D:\".into())
        );
        assert_eq!(
            LocalShellPathStyle::from_program(r"C:\Windows\System32\wsl.exe"),
            LocalShellPathStyle::Wsl
        );
        assert_eq!(
            LocalShellPathStyle::from_program(r"C:\Program Files\Git\bin\bash.exe"),
            LocalShellPathStyle::Msys
        );
        assert_eq!(
            LocalShellPathStyle::from_program(r"C:\cygwin64\bin\bash.exe"),
            LocalShellPathStyle::Native
        );
    }

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
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, "/mnt/d/Codes/RustProjects"),
            Some(r"D:\Codes\RustProjects".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, "/mnt/c"),
            Some(r"C:\".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, "/mnt/d/"),
            Some(r"D:\".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/d:/Projects"),
            Some(r"D:\Projects".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/d/Projects"),
            Some(r"D:\Projects".to_string())
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/D/Codes"),
            Some(r"D:\Codes".to_string())
        );
    }

    #[test]
    fn local_shell_cwd_translation_leaves_unrelated_paths_untouched() {
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, "/mnt/backup/data"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, "/mnt/"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, "/mnt/1/x"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/home/debian"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "/Debian/mnt/d/x"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, r"D:\Codes"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(LocalShellPathStyle::Msys, "relative/path"),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path(
                LocalShellPathStyle::Msys,
                "//wsl.localhost/Debian/home"
            ),
            None
        );
    }

    #[test]
    fn wsl_cwd_translation_falls_back_to_the_wsl_localhost_namespace() {
        fn mapped(path: &str, distro: Option<&str>) -> Option<String> {
            local_shell_cwd_to_browser_path_with_distro(LocalShellPathStyle::Wsl, path, distro)
        }

        // Drive-letter forms keep their native Windows mapping.
        assert_eq!(
            mapped("/mnt/d/Projects", Some("Debian")),
            Some(r"D:\Projects".to_string())
        );
        assert_eq!(mapped("/mnt/c", Some("Debian")), Some(r"C:\".to_string()));

        // WSL-internal directories land on the 9P namespace the host can list.
        assert_eq!(
            mapped("/mnt", Some("Debian")),
            Some(r"\\wsl.localhost\Debian\mnt".to_string())
        );
        assert_eq!(
            mapped("/home/user/notes", Some("Debian")),
            Some(r"\\wsl.localhost\Debian\home\user\notes".to_string())
        );
        assert_eq!(
            mapped("/mnt/c/Projects", Some("Debian")),
            Some(r"C:\Projects".to_string())
        );
        assert_eq!(
            mapped("/etc", Some("Debian")),
            Some(r"\\wsl.localhost\Debian\etc".to_string())
        );
        assert_eq!(
            mapped("/", Some("Debian")),
            Some(r"\\wsl.localhost\Debian".to_string())
        );
        // A user-mounted non-drive directory under /mnt stays browsable.
        assert_eq!(
            mapped("/mnt/backup/data", Some("Arch")),
            Some(r"\\wsl.localhost\Arch\mnt\backup\data".to_string())
        );

        // Without a distribution name there is nothing to map onto.
        assert_eq!(mapped("/mnt", None), None);
        assert_eq!(mapped("/home/user", Some("")), None);

        // Paths that are not absolute POSIX paths, or that carry characters
        // illegal in a Windows path component, stay unmapped.
        assert_eq!(mapped("relative/path", Some("Debian")), None);
        assert_eq!(mapped(r"D:\Projects", Some("Debian")), None);
        assert_eq!(mapped("/mnt/d:/x", Some("Debian")), None);
        // A literal backslash would turn into a separator and point at a
        // different directory, so it stays unmapped too.
        assert_eq!(mapped(r"/home/user/a\b", Some("Debian")), None);

        // Only the WSL style may fall back: MSYS and native `/` roots are not
        // the WSL filesystem.
        assert_eq!(
            local_shell_cwd_to_browser_path_with_distro(
                LocalShellPathStyle::Msys,
                "/home/user",
                Some("Debian")
            ),
            None
        );
        assert_eq!(
            local_shell_cwd_to_browser_path_with_distro(
                LocalShellPathStyle::Native,
                "/home/user",
                Some("Debian")
            ),
            None
        );
    }

    #[test]
    fn wsl_mapping_rejects_ambiguous_components_before_drive_or_unc_translation() {
        for component in [
            "a:b",
            "a\\b",
            "a\0b",
            "a\nb",
            "a\tb",
            "a\u{7f}b",
            "a<b",
            "a>b",
            "a\"b",
            "a|b",
            "a?b",
            "a*b",
            "trailing ",
            "trailing.",
            ".",
            "..",
            "CON",
            "con.txt",
            "NUL",
            "AUX.log",
            "PRN",
            "COM1",
            "com9.txt",
            "LPT1",
            "LPT9.log",
            "COM¹",
            "LPT².txt",
            "COM³",
            "CONIN$",
            "CONOUT$",
            "NUL .txt",
        ] {
            for prefix in ["/home/user", "/mnt/c"] {
                for path in [
                    format!("{prefix}/{component}"),
                    format!("{prefix}/{component}/child"),
                ] {
                    for distro in [None, Some("Ubuntu")] {
                        assert_eq!(
                            local_shell_cwd_to_browser_path_with_distro(
                                LocalShellPathStyle::Wsl,
                                &path,
                                distro
                            ),
                            None,
                            "unexpected mapping: {path:?}"
                        );
                    }
                    assert_eq!(
                        local_shell_cwd_to_browser_path(LocalShellPathStyle::Wsl, &path),
                        None
                    );
                }
            }
        }
        for distro in [
            "",
            ".",
            "..",
            "Ubuntu/other",
            "Ubuntu\\other",
            "Ubuntu:",
            "Ubuntu ",
            "Ubuntu.",
            "NUL",
            "bad\0name",
        ] {
            assert_eq!(
                local_shell_cwd_to_browser_path_with_distro(
                    LocalShellPathStyle::Wsl,
                    "/home/user",
                    Some(distro)
                ),
                None
            );
        }
    }

    #[test]
    fn wsl_mapping_preserves_representable_names() {
        for component in [
            ".config",
            "中文",
            "my notes",
            " leading",
            "console",
            "COM10",
            "LPT0",
            "auxiliary.txt",
        ] {
            for (prefix, expected) in [
                ("/home/user", r"\\wsl.localhost\Ubuntu\home\user"),
                ("/mnt/d", "D:"),
            ] {
                let mapped = local_shell_cwd_to_browser_path_with_distro(
                    LocalShellPathStyle::Wsl,
                    &format!("{prefix}/{component}"),
                    Some("Ubuntu"),
                )
                .unwrap();
                assert_eq!(mapped, format!("{expected}\\{component}"));
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_paths_preserve_guest_case_across_aliases_and_verbatim_prefixes() {
        let backend = FileBrowserBackendKind::Local;
        let path = r"\\wsl.localhost\Ubuntu\home\user\Project";
        assert!(file_browser_path_eq(
            backend,
            r"\\wsl$\中\Project",
            r"\\wsl.localhost\中\Project"
        ));
        for alias in [
            r"\\WSL.LOCALHOST\UBUNTU\home\user\Project",
            r"\\wsl$\ubuntu\home\user\Project",
            "//wsl$/Ubuntu/home/user/Project",
            r"\\?\UNC\wsl.localhost\Ubuntu\home\user\Project",
            r"\\?\unc\WSL$\UBUNTU\home\user\Project",
        ] {
            assert!(file_browser_path_eq(backend, path, alias), "{alias}");
            assert!(file_browser_copy_destination_invalid(
                backend, path, alias, false
            ));
        }
        assert!(!file_browser_path_eq(
            backend,
            path,
            r"\\wsl.localhost\Ubuntu\home\user\project"
        ));
        assert!(!file_browser_path_eq(
            backend,
            path,
            r"\\wsl.localhost\Debian\home\user\Project"
        ));
        assert!(file_browser_copy_destination_invalid(
            backend,
            path,
            r"\\wsl$\ubuntu\home\user\Project\child",
            true
        ));
        assert!(!file_browser_copy_destination_invalid(
            backend,
            path,
            r"\\wsl$\Ubuntu\home\user\project\child",
            true
        ));
        assert!(file_browser_path_eq(
            backend,
            r"\\server\Share\Project",
            r"\\SERVER\share\project"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_paths_keep_the_distribution_as_browser_root() {
        let backend = FileBrowserBackendKind::Local;
        for root in [
            r"\\wsl.localhost\Ubuntu",
            r"\\wsl$\Ubuntu",
            r"\\?\UNC\wsl.localhost\Ubuntu",
        ] {
            assert!(file_browser_path_is_root(backend, root));
            assert!(file_browser_path_is_root(backend, &format!("{root}\\")));
            let home = file_browser_join(backend, root, "home");
            assert!(file_browser_path_eq(
                backend,
                &file_browser_parent(backend, &home),
                root
            ));
            assert!(file_browser_path_eq(
                backend,
                &file_browser_root(backend, &home),
                root
            ));
            assert_eq!(file_browser_name(backend, &home), "home");
            assert!(file_browser_copy_destination_invalid(
                backend, root, &home, true
            ));
        }
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
    use crate::remote_file::RemoteFileService;
    use crate::sftp::RemoteFilePath;
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_transport::SshSessionConfig;

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
