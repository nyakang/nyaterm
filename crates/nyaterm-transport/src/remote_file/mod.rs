use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    RemoteBinaryFile, RemoteFilePath, SftpAttributeUpdate, SftpDuplicateDecision,
    SftpDuplicatePolicy, SftpDuplicateRequest, SftpFileEntry, SftpFileProperties, SftpFileType,
    SftpPathTransferOptions, SftpRemoteTextFile, SftpService, SftpTransferControl,
    SftpTransferDirection, SftpTransferOptions, SftpTransferProgress, SftpTransferSummary,
    SftpWriteTextResult, SshMultiplexHandle, SshSessionConfig,
};

pub(crate) mod copy_progress;
#[cfg(test)]
mod copy_tests;
#[cfg(test)]
mod editor_save_tests;
#[cfg(test)]
mod normal_listing_tests;
mod revision;
mod shell;
#[cfg(test)]
mod upload_tests;

use copy_progress::CopyProgress;
pub(crate) use revision::metadata_is_stable;
pub use revision::{
    RemoteTextDocument, RemoteTextGeneration, RemoteTextMetadata, RemoteTextRevision,
    RemoteTextWriteResult,
};
use shell::metadata::{
    shell_file_properties, shell_file_replace_command, shell_file_upload_command,
};
use shell::{ShellRemote, shell_quote, shell_quote_path};

pub const REMOTE_FILE_MANAGER_UNAVAILABLE: &str =
    "Terminal connection is working, but the remote file manager could not be initialized";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteFileBackendKind {
    Sftp,
    ScpEnhanced,
    ScpNormal,
}

impl RemoteFileBackendKind {
    pub fn cache_name(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::ScpEnhanced => "scp_enhanced",
            Self::ScpNormal => "scp_normal",
        }
    }

    pub fn from_cache_name(value: &str) -> Option<Self> {
        match value {
            "sftp" => Some(Self::Sftp),
            "scp_enhanced" => Some(Self::ScpEnhanced),
            "scp_normal" => Some(Self::ScpNormal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFileBackendPreference {
    pub backend: RemoteFileBackendKind,
    pub sftp_unavailable: bool,
    pub last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendProbeStage {
    Cached,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BackendSelection {
    backend: RemoteFileBackendKind,
    from_cache: bool,
    sftp_failure: Option<String>,
}

fn select_backend_with_probe<E>(
    cached: Option<RemoteFileBackendKind>,
    mut probe: impl FnMut(RemoteFileBackendKind, BackendProbeStage) -> Result<(), E>,
) -> Result<BackendSelection, ()>
where
    E: std::fmt::Display,
{
    if let Some(cached) = cached
        && probe(cached, BackendProbeStage::Cached).is_ok()
    {
        return Ok(BackendSelection {
            backend: cached,
            from_cache: true,
            sftp_failure: None,
        });
    }

    let mut sftp_failure = None;
    for candidate in [
        RemoteFileBackendKind::Sftp,
        RemoteFileBackendKind::ScpEnhanced,
        RemoteFileBackendKind::ScpNormal,
    ] {
        match probe(candidate, BackendProbeStage::Full) {
            Ok(()) => {
                return Ok(BackendSelection {
                    backend: candidate,
                    from_cache: false,
                    sftp_failure,
                });
            }
            Err(error) if candidate == RemoteFileBackendKind::Sftp => {
                sftp_failure = Some(error.to_string());
            }
            Err(_) => {}
        }
    }
    Err(())
}

pub trait RemoteFileBackendPreferenceStore: Send + Sync {
    fn load_backend(
        &self,
        endpoint_key: &str,
    ) -> anyhow::Result<Option<RemoteFileBackendPreference>>;

    fn save_backend(
        &self,
        endpoint_key: &str,
        preference: &RemoteFileBackendPreference,
    ) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct RemoteFileService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    sftp_service: SftpService,
    preference_store: Option<Arc<dyn RemoteFileBackendPreferenceStore>>,
    selected: Arc<Mutex<Option<RemoteFileBackendKind>>>,
}

impl std::fmt::Debug for RemoteFileService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteFileService")
            .field("endpoint_key", &self.endpoint_key())
            .field("selected_backend", &self.selected_backend())
            .finish_non_exhaustive()
    }
}

impl RemoteFileService {
    /// Join using the destination's filename encoding, retaining an opaque parent.
    pub fn child_path(
        &self,
        directory: &RemoteFilePath,
        name: &str,
    ) -> anyhow::Result<RemoteFilePath> {
        anyhow::ensure!(
            !name.is_empty()
                && name != "."
                && name != ".."
                && !name.contains('/')
                && !name.contains('\0'),
            "invalid remote name"
        );
        let display = crate::file_browser::file_browser_join(
            crate::file_browser::FileBrowserBackendKind::Remote,
            &directory.display_path,
            name,
        );
        if let Some(mut raw) = directory.raw_path()? {
            if raw.last() != Some(&b'/') {
                raw.push(b'/');
            }
            raw.extend(
                crate::sftp::SftpPathCodec::from_ssh_config(&self.config)?.encode_path(name)?,
            );
            Ok(RemoteFilePath::from_raw(display, &raw))
        } else {
            Ok(RemoteFilePath::new(display))
        }
    }

    pub fn copy_child_path(
        &self,
        directory: &RemoteFilePath,
        source: &Self,
        path: &RemoteFilePath,
        name: &str,
    ) -> anyhow::Result<RemoteFilePath> {
        let codec = crate::sftp::SftpPathCodec::from_ssh_config(&self.config)?;
        let source_codec = crate::sftp::SftpPathCodec::from_ssh_config(&source.config)?;
        if codec.same_encoding(&source_codec)
            && let Some(raw) = path.raw_path()?
        {
            let name = raw.rsplit(|byte| *byte == b'/').next().unwrap_or_default();
            anyhow::ensure!(
                !name.is_empty() && name != b"." && name != b".." && !name.contains(&0),
                "invalid source filename"
            );
            let mut parent = match directory.raw_path()? {
                Some(raw) => raw,
                None => codec.encode_path(&directory.display_path)?,
            };
            if parent.last() != Some(&b'/') {
                parent.push(b'/');
            }
            parent.extend(name);
            let display = crate::file_browser::file_browser_join(
                crate::file_browser::FileBrowserBackendKind::Remote,
                &directory.display_path,
                path.display_path.rsplit('/').next().unwrap_or_default(),
            );
            return Ok(RemoteFilePath::from_raw(display, &parent));
        }
        self.child_path(directory, name)
    }

    /// String-only fallback protocols cannot address an opaque filename safely.
    pub fn require_display_path(&self, path: &RemoteFilePath) -> anyhow::Result<()> {
        if let Some(raw) = path.raw_path()? {
            anyhow::ensure!(
                raw == crate::sftp::SftpPathCodec::from_ssh_config(&self.config)?
                    .encode_path(&path.display_path)?,
                "this backend cannot address an opaque path"
            );
        }
        Ok(())
    }

    pub fn create_dir_remote_path(
        &self,
        path: &RemoteFilePath,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.create_dir_remote_path(path, mode),
            _ => {
                self.require_display_path(path)?;
                self.create_dir_path(&path.display_path, mode)
            }
        }
    }

    pub fn create_file_remote_path(
        &self,
        path: &RemoteFilePath,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.create_file_remote_path(path, mode),
            kind => {
                self.require_display_path(path)?;
                let path = shell_quote_path(&path.display_path);
                let mode = mode.map_or(String::new(), |mode| format!(" && chmod {mode:o} {path}"));
                self.shell(kind)
                    .exec_ok(format!("set -C; : > {path}{mode}"), None)?;
                Ok(())
            }
        }
    }

    pub fn new(config: SshSessionConfig) -> Self {
        Self::from_parts(config, None, None)
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        Ok(Self::from_parts(config, Some(multiplex), None))
    }

    pub fn with_preference_store(
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
        preference_store: Arc<dyn RemoteFileBackendPreferenceStore>,
    ) -> anyhow::Result<Self> {
        if let Some(multiplex) = multiplex.as_ref() {
            multiplex.ensure_matches_config(&config)?;
        }
        Ok(Self::from_parts(config, multiplex, Some(preference_store)))
    }

    fn from_parts(
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
        preference_store: Option<Arc<dyn RemoteFileBackendPreferenceStore>>,
    ) -> Self {
        let sftp_service = match multiplex.clone() {
            Some(handle) => SftpService::with_multiplex(config.clone(), handle)
                .expect("multiplex compatibility was validated by the constructor"),
            None => SftpService::new(config.clone()),
        };
        Self {
            config,
            multiplex,
            sftp_service,
            preference_store,
            selected: Arc::new(Mutex::new(None)),
        }
    }

    pub fn selected_backend(&self) -> Option<RemoteFileBackendKind> {
        *self
            .selected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn endpoint_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.config.host, self.config.port, self.config.username
        )
    }

    fn backend(&self) -> anyhow::Result<RemoteFileBackendKind> {
        let mut selected = self
            .selected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(backend) = *selected {
            return Ok(backend);
        }

        let endpoint_key = self.endpoint_key();
        let cached = self.preference_store.as_ref().and_then(|store| {
            match store.load_backend(&endpoint_key) {
                Ok(value) => value.map(|value| value.backend),
                Err(error) => {
                    tracing::warn!(
                        endpoint = %endpoint_key,
                        stage = "cache_read",
                        error = %error,
                        "remote file backend cache read failed"
                    );
                    None
                }
            }
        });
        let selection = select_backend_with_probe(cached, |candidate, stage| {
            let stage_name = match stage {
                BackendProbeStage::Cached => "cached_probe",
                BackendProbeStage::Full => "full_probe",
            };
            tracing::info!(
                endpoint = %endpoint_key,
                backend = candidate.cache_name(),
                stage = stage_name,
                "probing remote file backend"
            );
            let result = self.probe(candidate);
            if let Err(error) = result.as_ref() {
                tracing::info!(
                    endpoint = %endpoint_key,
                    backend = candidate.cache_name(),
                    stage = "probe_failed",
                    error = %error,
                    "remote file backend probe failed"
                );
            }
            result
        })
        .map_err(|()| anyhow::anyhow!(REMOTE_FILE_MANAGER_UNAVAILABLE))?;

        if !selection.from_cache {
            let preference = RemoteFileBackendPreference {
                backend: selection.backend,
                sftp_unavailable: selection.backend != RemoteFileBackendKind::Sftp,
                last_failure_reason: selection.sftp_failure,
            };
            if let Some(store) = self.preference_store.as_ref()
                && let Err(error) = store.save_backend(&endpoint_key, &preference)
            {
                tracing::warn!(
                    endpoint = %endpoint_key,
                    backend = selection.backend.cache_name(),
                    stage = "cache_write",
                    error = %error,
                    "remote file backend cache write failed"
                );
            }
        }
        tracing::info!(
            endpoint = %endpoint_key,
            backend = selection.backend.cache_name(),
            stage = "selected",
            "remote file backend selected"
        );
        *selected = Some(selection.backend);
        Ok(selection.backend)
    }

    fn probe(&self, backend: RemoteFileBackendKind) -> anyhow::Result<()> {
        match backend {
            RemoteFileBackendKind::Sftp => self.sftp()?.list_dir(".").map(|_| ()),
            kind => self.shell(kind).probe(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_export_service(sftp_service: SftpService) -> Self {
        let mut service = Self::new(SshSessionConfig::default());
        service.sftp_service = sftp_service;
        *service.selected.lock().unwrap() = Some(RemoteFileBackendKind::Sftp);
        service
    }

    pub(crate) fn export_sftp_service(&self) -> SftpService {
        self.sftp_service.clone()
    }

    fn sftp(&self) -> anyhow::Result<SftpService> {
        Ok(self.sftp_service.clone())
    }

    fn shell(&self, kind: RemoteFileBackendKind) -> ShellRemote {
        ShellRemote::new(self.config.clone(), self.multiplex.clone(), kind)
    }

    pub fn home_dir(&self) -> anyhow::Result<String> {
        let backend = self.backend()?;
        let output = self.shell(backend).exec_ok("printf '%s' \"$HOME\"", None)?;
        Ok(String::from_utf8(output)?.trim().to_string())
    }

    pub fn list_dir(&self, path: impl AsRef<str>) -> anyhow::Result<Vec<SftpFileEntry>> {
        self.list_dir_path(&RemoteFilePath::new(path.as_ref()))
    }

    pub fn list_dir_path(&self, path: &RemoteFilePath) -> anyhow::Result<Vec<SftpFileEntry>> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.list_dir_path(path),
            kind => {
                self.require_display_path(path)?;
                list_shell_dir(&self.shell(kind), path, kind)
            }
        }
    }

    pub fn file_properties(&self, path: impl AsRef<str>) -> anyhow::Result<SftpFileProperties> {
        self.remote_file_properties(&RemoteFilePath::new(path.as_ref()))
    }

    pub fn remote_file_properties(
        &self,
        path: &RemoteFilePath,
    ) -> anyhow::Result<SftpFileProperties> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.remote_file_properties(path),
            kind => {
                let shell = self.shell(kind);
                self.require_display_path(path)?;
                let mut properties = shell_properties(&shell, path)?;
                if properties.file_type == SftpFileType::Symlink {
                    let output = shell.exec_ok(
                        format!("readlink -- {}", shell_quote(&path.display_path)),
                        None,
                    )?;
                    let text = String::from_utf8(output)?;
                    properties.symlink_target =
                        Some(text.strip_suffix('\n').unwrap_or(&text).to_string());
                }
                Ok(properties)
            }
        }
    }

    pub fn create_dir_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.create_dir_path(path, mode),
            kind => {
                let path = shell_quote_path(path.as_ref());
                let mode = mode.map_or(String::new(), |mode| format!(" && chmod {mode:o} {path}"));
                self.shell(kind)
                    .exec_ok(format!("mkdir -p -- {path}{mode}"), None)?;
                Ok(())
            }
        }
    }

    pub fn create_file_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.create_file_path(path, mode),
            kind => {
                let path = shell_quote_path(path.as_ref());
                let mode = mode.map_or(String::new(), |mode| format!(" && chmod {mode:o} {path}"));
                self.shell(kind)
                    .exec_ok(format!(": > {path}{mode}"), None)?;
                Ok(())
            }
        }
    }

    pub fn create_symlink_path(
        &self,
        link_path: impl AsRef<str>,
        target_path: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.create_symlink_path(link_path, target_path),
            kind => {
                self.shell(kind).exec_ok(
                    format!(
                        "ln -s -- {} {}",
                        shell_quote(target_path.as_ref()),
                        shell_quote(link_path.as_ref())
                    ),
                    None,
                )?;
                Ok(())
            }
        }
    }

    pub fn delete_path(&self, path: impl AsRef<str>) -> anyhow::Result<()> {
        self.delete_remote_path(&RemoteFilePath::new(path.as_ref()))
    }

    pub fn delete_remote_path(&self, path: &RemoteFilePath) -> anyhow::Result<()> {
        ensure_safe_delete_target(&path.display_path)?;
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.delete_remote_path(path),
            kind => {
                self.shell(kind).exec_ok(
                    {
                        self.require_display_path(path)?;
                        format!("rm -rf -- {}", shell_quote(&path.display_path))
                    },
                    None,
                )?;
                Ok(())
            }
        }
    }

    pub fn rename_path(
        &self,
        old_path: impl AsRef<str>,
        new_path: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        self.rename_remote_paths(
            &RemoteFilePath::new(old_path.as_ref()),
            &RemoteFilePath::new(new_path.as_ref()),
        )
    }

    pub fn rename_remote_paths(
        &self,
        old_path: &RemoteFilePath,
        new_path: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.rename_remote_paths(old_path, new_path),
            kind => {
                self.require_display_path(old_path)?;
                self.require_display_path(new_path)?;
                self.shell(kind).exec_ok(
                    format!(
                        "mv -- {} {}",
                        shell_quote(&old_path.display_path),
                        shell_quote(&new_path.display_path)
                    ),
                    None,
                )?;
                Ok(())
            }
        }
    }

    pub fn rename_remote_paths_no_replace(
        &self,
        source: &RemoteFilePath,
        destination: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        match self.backend()? {
            // SFTP v3 RENAME must fail when newpath exists. Do not use the
            // posix-rename extension, which permits replacement.
            RemoteFileBackendKind::Sftp => self.sftp()?.rename_remote_paths(source, destination),
            kind => {
                self.require_display_path(source)?;
                self.require_display_path(destination)?;
                let old = shell_quote(&source.display_path);
                let new = shell_quote(&destination.display_path);
                self.shell(kind).exec_ok(format!("if [ -e {new} ] || [ -L {new} ]; then exit 1; fi; mv -nT -- {old} {new} && [ ! -e {old} ] && [ ! -L {old} ]"), None)?;
                Ok(())
            }
        }
    }

    pub fn update_path_attributes(
        &self,
        path: impl AsRef<str>,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        self.update_remote_path_attributes(&RemoteFilePath::new(path.as_ref()), update)
    }

    pub fn update_remote_path_attributes(
        &self,
        path: &RemoteFilePath,
        update: SftpAttributeUpdate,
    ) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.update_remote_path_attributes(path, update),
            kind => {
                let shell = self.shell(kind);
                if let Some(target) = update.symlink_target.as_deref() {
                    if target.is_empty() || target.contains('\0') {
                        anyhow::bail!("invalid symbolic link target");
                    }
                    if update.mode.is_some()
                        || update.owner.is_some()
                        || update.group.is_some()
                        || update.recursive
                    {
                        anyhow::bail!("save the link target separately from target attributes");
                    }
                    let original = shell_quote(&path.display_path);
                    let temporary = shell_quote(&format!(
                        "{}.nyaterm-{}",
                        path.display_path,
                        uuid::Uuid::new_v4()
                    ));
                    shell.exec_ok(format!("test -L {original} && ln -s -- {} {temporary} && {{ test -L {original} && mv -Tf -- {temporary} {original}; }}; code=$?; rm -f -- {temporary}; exit $code", shell_quote(target)), None)?;
                    Ok(())
                } else {
                    update_shell_attributes(&shell, &path.display_path, &update)
                }
            }
        }
    }

    pub fn read_text_file(
        &self,
        path: impl AsRef<str>,
        max_bytes: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        self.read_text_file_path(&RemoteFilePath::new(path.as_ref()), max_bytes)
    }

    pub fn read_text_file_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.read_text_file_path(path, max_bytes),
            kind => {
                let properties = shell_properties(&self.shell(kind), path)?;
                if properties.is_directory() {
                    anyhow::bail!("Directories cannot be opened as text");
                }
                if properties.size.unwrap_or(0) > max_bytes {
                    anyhow::bail!("File is too large to open as text");
                }
                let bytes = self
                    .shell(kind)
                    .exec_ok(format!("cat -- {}", shell_quote(&path.display_path)), None)?;
                if bytes.contains(&0) {
                    anyhow::bail!("Binary files are not supported by the built-in editor");
                }
                let content = String::from_utf8(bytes)?;
                Ok(SftpRemoteTextFile {
                    path: path.display_path.clone(),
                    size: content.len() as u64,
                    content,
                    modified_at: u64::from(properties.modified_at.unwrap_or(0)),
                })
            }
        }
    }

    pub fn read_text_document_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteTextDocument> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.read_text_document_path(path, max_bytes),
            kind => read_shell_text_document(&self.shell(kind), path, max_bytes),
        }
    }

    pub fn read_file_bytes(
        &self,
        path: impl AsRef<str>,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        self.read_file_bytes_path(&RemoteFilePath::new(path.as_ref()), max_bytes)
    }

    pub fn read_file_bytes_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.read_file_bytes_path(path, max_bytes),
            kind => {
                let properties = shell_properties(&self.shell(kind), path)?;
                if properties.is_directory() {
                    anyhow::bail!("Directories cannot be previewed");
                }
                if properties.size.unwrap_or(0) > max_bytes {
                    anyhow::bail!("File is too large to preview");
                }
                let content_bytes = self
                    .shell(kind)
                    .exec_ok(format!("cat -- {}", shell_quote(&path.display_path)), None)?;
                Ok(RemoteBinaryFile {
                    path: path.display_path.clone(),
                    size: content_bytes.len() as u64,
                    content_bytes,
                    modified_at: u64::from(properties.modified_at.unwrap_or(0)),
                })
            }
        }
    }

    pub fn write_text_file(
        &self,
        path: impl AsRef<str>,
        content: impl AsRef<str>,
        expected_modified_at: Option<u64>,
        expected_size: Option<u64>,
        force: bool,
    ) -> anyhow::Result<SftpWriteTextResult> {
        let path = RemoteFilePath::new(path.as_ref());
        self.write_text_file_path(&path, content, expected_modified_at, expected_size, force)
    }

    pub fn write_text_file_path(
        &self,
        path: &RemoteFilePath,
        content: impl AsRef<str>,
        expected_modified_at: Option<u64>,
        expected_size: Option<u64>,
        force: bool,
    ) -> anyhow::Result<SftpWriteTextResult> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.write_text_file_path(
                path,
                content,
                expected_modified_at,
                expected_size,
                force,
            ),
            kind => write_shell_text(
                &self.shell(kind),
                path,
                content.as_ref(),
                expected_modified_at,
                expected_size,
                force,
            ),
        }
    }

    pub fn write_text_document_path(
        &self,
        path: &RemoteFilePath,
        content: impl AsRef<str>,
        expected_revision: Option<&RemoteTextRevision>,
        force: bool,
    ) -> anyhow::Result<RemoteTextWriteResult> {
        if !force && expected_revision.is_none() {
            anyhow::bail!("Remote text revision is required for a safe save");
        }
        let expected_revision = if force { None } else { expected_revision };
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.write_text_document_path(
                path,
                content,
                expected_revision.cloned(),
                force,
            ),
            kind => write_shell_text_document(
                &self.shell(kind),
                path,
                content.as_ref(),
                expected_revision,
                force,
            ),
        }
    }

    pub fn download_file(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
    ) -> anyhow::Result<SftpTransferSummary> {
        let remote_path = remote_path.as_ref().to_string();
        let local_path = local_path.into();
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.download_file(&remote_path, local_path),
            kind => {
                let download_root = crate::download_path::root_for_target(&local_path);
                let properties =
                    shell_properties(&self.shell(kind), &RemoteFilePath::new(&remote_path))?;
                ensure_supported_download_source(properties.file_type)?;
                let bytes = self
                    .shell(kind)
                    .exec_ok(format!("cat -- {}", shell_quote(&remote_path)), None)?;
                crate::download_path::TransferDownload::write(download_root, &local_path, &bytes)?;
                Ok(SftpTransferSummary {
                    remote_path,
                    local_path,
                    bytes: bytes.len() as u64,
                    skipped: false,
                })
            }
        }
    }

    pub fn download_file_with_progress_and_control_options<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_remote_file_with_progress_and_control_options(
            &RemoteFilePath::new(remote_path.as_ref()),
            local_path,
            control,
            options,
            progress,
        )
    }

    pub fn download_remote_file_with_progress_and_control_options<F>(
        &self,
        remote_path: &RemoteFilePath,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let remote_path = remote_path.clone();
        let local_path = local_path.into();
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self
                .sftp()?
                .download_remote_file_with_progress_and_control_options(
                    &remote_path,
                    local_path,
                    control,
                    options,
                    progress,
                ),
            kind => {
                control.wait_if_paused_blocking()?;
                let download_root = crate::download_path::root_for_target(&local_path);
                let properties = shell_properties(&self.shell(kind), &remote_path)?;
                ensure_supported_download_source(properties.file_type)?;
                anyhow::ensure!(
                    !properties.is_directory(),
                    "remote source is not a regular file"
                );
                let bytes = download_shell_path(
                    self,
                    kind,
                    &remote_path.display_path,
                    &local_path,
                    &control,
                    progress,
                    download_root,
                    &options,
                )?;
                Ok(SftpTransferSummary {
                    remote_path: remote_path.display_path,
                    local_path,
                    bytes,
                    skipped: false,
                })
            }
        }
    }

    pub fn download_path_with_progress_and_path_options<F>(
        &self,
        remote_path: impl AsRef<str>,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        options: SftpPathTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        self.download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new(remote_path.as_ref()),
            local_path,
            control,
            options,
            progress,
        )
    }

    pub fn download_remote_path_with_progress_and_path_options<F>(
        &self,
        remote_path: &RemoteFilePath,
        local_path: impl Into<PathBuf>,
        control: SftpTransferControl,
        options: SftpPathTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let remote_path = remote_path.clone();
        let local_path = local_path.into();
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self
                .sftp()?
                .download_remote_path_with_progress_and_path_options(
                    &remote_path,
                    local_path,
                    control,
                    options,
                    progress,
                ),
            kind => {
                let download_root = crate::download_path::root_for_target(&local_path);
                let properties = shell_properties(&self.shell(kind), &remote_path)?;
                ensure_supported_download_source(properties.file_type)?;
                let target = resolve_shell_download_target(
                    &remote_path.display_path,
                    &local_path,
                    properties.is_directory(),
                    &options,
                )?;
                let Some(target) = target else {
                    return Ok(SftpTransferSummary {
                        remote_path: remote_path.display_path,
                        local_path,
                        bytes: 0,
                        skipped: true,
                    });
                };
                let bytes = download_shell_path(
                    self,
                    kind,
                    &remote_path.display_path,
                    &target,
                    &control,
                    progress,
                    download_root,
                    options.transfer_options(),
                )?;
                Ok(SftpTransferSummary {
                    remote_path: remote_path.display_path,
                    local_path: target,
                    bytes,
                    skipped: false,
                })
            }
        }
    }

    pub fn upload_file(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
    ) -> anyhow::Result<SftpTransferSummary> {
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.upload_file(local_path, &remote_path),
            kind => {
                let bytes = self.shell(kind).upload_stream(
                    &local_path,
                    &remote_path,
                    SftpTransferControl::new(),
                    &SftpTransferOptions::default(),
                    |_| {},
                )?;
                Ok(SftpTransferSummary {
                    remote_path,
                    local_path,
                    bytes,
                    skipped: false,
                })
            }
        }
    }

    pub fn upload_file_with_progress_and_control_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        match self.backend()? {
            RemoteFileBackendKind::Sftp => {
                self.sftp()?.upload_file_with_progress_and_control_options(
                    local_path,
                    remote_path,
                    control,
                    options,
                    progress,
                )
            }
            kind => {
                anyhow::ensure!(
                    fs::metadata(&local_path)?.is_file(),
                    "upload source is not a regular file"
                );
                let bytes = upload_shell_path(
                    self,
                    kind,
                    &local_path,
                    &remote_path,
                    &control,
                    &options,
                    progress,
                )?;
                Ok(SftpTransferSummary {
                    remote_path: remote_path.to_string(),
                    local_path,
                    bytes,
                    skipped: false,
                })
            }
        }
    }

    pub fn upload_remote_file_with_progress_and_control_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: &RemoteFilePath,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let local_path = local_path.into();
        if self.backend()? != RemoteFileBackendKind::Sftp {
            self.require_display_path(remote_path)?;
        }
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self
                .sftp()?
                .upload_remote_file_with_progress_and_control_options(
                    local_path,
                    remote_path,
                    control,
                    options,
                    progress,
                ),
            kind => {
                anyhow::ensure!(
                    fs::metadata(&local_path)?.is_file(),
                    "upload source is not a regular file"
                );
                let bytes = upload_shell_path(
                    self,
                    kind,
                    &local_path,
                    &remote_path.display_path,
                    &control,
                    &options,
                    progress,
                )?;
                Ok(SftpTransferSummary {
                    remote_path: remote_path.display_path.to_string(),
                    local_path,
                    bytes,
                    skipped: false,
                })
            }
        }
    }

    /// Sync an external edit to an existing remote file. Shell backends must
    /// restore and verify the original ownership and mode before replacing it.
    pub fn upload_remote_edit_with_progress_and_control_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: &RemoteFilePath,
        control: SftpTransferControl,
        mut options: SftpTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let local_path = local_path.into();
        // A configured mode for new uploads must not change an edited file's mode.
        options.default_file_mode = None;
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self
                .sftp()?
                .upload_remote_file_with_progress_and_control_options(
                    local_path,
                    remote_path,
                    control,
                    options,
                    progress,
                ),
            kind => {
                control.wait_if_paused_blocking()?;
                let shell = self.shell(kind);
                let bytes = fs::read(&local_path)?;
                let properties = shell_file_properties(&shell, remote_path)?;
                anyhow::ensure!(
                    properties.file_type == SftpFileType::File,
                    "External edit sync requires an existing regular file"
                );
                let temporary = format!(
                    "{}.nyaterm-edit-{}",
                    remote_path.display_path,
                    uuid::Uuid::new_v4().simple()
                );
                let command = shell_file_upload_command(
                    &properties,
                    &temporary,
                    &remote_path.display_path,
                    bytes.len(),
                )?;
                control.wait_if_paused_blocking()?;
                let byte_count = bytes.len() as u64;
                if let Err(error) = shell.exec_ok(command, Some(bytes)) {
                    let _ = shell.exec(format!("rm -f -- {}", shell_quote(&temporary)), None);
                    return Err(error);
                }
                let summary = SftpTransferSummary {
                    remote_path: remote_path.display_path.clone(),
                    local_path,
                    bytes: byte_count,
                    skipped: false,
                };
                progress(SftpTransferProgress {
                    remote_path: summary.remote_path.clone(),
                    local_path: summary.local_path.clone(),
                    bytes_transferred: summary.bytes,
                    total_bytes: Some(summary.bytes),
                    item_count_completed: Some(1),
                    item_count_total: Some(1),
                });
                Ok(summary)
            }
        }
    }

    pub fn upload_path_with_progress_and_path_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: impl AsRef<str>,
        control: SftpTransferControl,
        options: SftpPathTransferOptions,
        progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let local_path = local_path.into();
        let remote_path = remote_path.as_ref().to_string();
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.upload_path_with_progress_and_path_options(
                local_path,
                remote_path,
                control,
                options,
                progress,
            ),
            kind => {
                let target = resolve_shell_upload_target(
                    self,
                    kind,
                    &local_path,
                    &remote_path,
                    options.duplicate_policy(),
                    options.duplicate_resolver(),
                )?;
                let Some(target) = target else {
                    return Ok(SftpTransferSummary {
                        remote_path,
                        local_path,
                        bytes: 0,
                        skipped: true,
                    });
                };
                let bytes = upload_shell_path(
                    self,
                    kind,
                    &local_path,
                    &target,
                    &control,
                    options.transfer_options(),
                    progress,
                )?;
                Ok(SftpTransferSummary {
                    remote_path: target,
                    local_path,
                    bytes,
                    skipped: false,
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum FileTransferEndpoint {
    Local(PathBuf),
    Remote {
        service: Arc<RemoteFileService>,
        path: RemoteFilePath,
    },
}

#[derive(Clone)]
pub struct FileCopyRequest {
    pub source: FileTransferEndpoint,
    pub destination: FileTransferEndpoint,
    pub control: SftpTransferControl,
    pub options: SftpPathTransferOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCopySummary {
    pub bytes: u64,
    pub used_local_staging: bool,
    pub destination_path: String,
    /// A conflict policy can return successfully without copying anything.
    pub copied: bool,
}

impl FileCopyRequest {
    pub fn execute(self) -> anyhow::Result<FileCopySummary> {
        self.execute_with_progress(|_| {})
    }

    /// Reports cumulative work for the entire copy. Staged remote copies count
    /// both the download and upload legs; the summary counts content bytes once.
    pub fn execute_with_progress<F>(self, progress: F) -> anyhow::Result<FileCopySummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let browser_endpoint = |endpoint: &FileTransferEndpoint| match endpoint {
            FileTransferEndpoint::Local(path) => (
                crate::FileBrowserService::local(),
                RemoteFilePath::new(path.to_string_lossy()),
            ),
            FileTransferEndpoint::Remote { service, path } => (
                crate::FileBrowserService::Remote(service.clone()),
                path.clone(),
            ),
        };
        let (source_service, source_path) = browser_endpoint(&self.source);
        let (target_service, target_path) = browser_endpoint(&self.destination);
        let directory = source_service
            .remote_file_properties(&source_path)?
            .is_directory();
        source_service.validate_copy_destination(
            &source_path,
            &target_service,
            &target_path,
            directory,
        )?;
        let staged = match (&self.source, &self.destination) {
            (
                FileTransferEndpoint::Remote {
                    service: source, ..
                },
                FileTransferEndpoint::Remote {
                    service: destination,
                    ..
                },
            ) => {
                source.backend()? != RemoteFileBackendKind::Sftp
                    || destination.backend()? != RemoteFileBackendKind::Sftp
            }
            _ => false,
        };
        let progress = CopyProgress::new(
            &self.source,
            &self.destination,
            if staged { 2 } else { 1 },
            progress,
        );
        progress.scan(&self.source, &self.control)?;
        let summary = self.execute_reporting(progress.clone())?;
        if summary.copied {
            progress.finish(summary.bytes);
        }
        Ok(summary)
    }

    fn execute_reporting(self, progress: CopyProgress) -> anyhow::Result<FileCopySummary> {
        let control = self.control;
        let options = self.options;
        match (self.source, self.destination) {
            (FileTransferEndpoint::Local(source), FileTransferEndpoint::Local(destination)) => {
                let Some(destination) = resolve_copy_local_target(
                    &source,
                    &destination,
                    &options,
                    SftpTransferDirection::Copy,
                )?
                else {
                    return Ok(FileCopySummary {
                        bytes: 0,
                        used_local_staging: false,
                        destination_path: destination.to_string_lossy().into_owned(),
                        copied: false,
                    });
                };
                control.wait_if_paused_blocking()?;
                Ok(FileCopySummary {
                    bytes: copy_local_path(
                        &source,
                        &destination,
                        &control,
                        options.transfer_options(),
                        &progress,
                    )?,
                    used_local_staging: false,
                    destination_path: destination.to_string_lossy().into_owned(),
                    copied: true,
                })
            }
            (
                FileTransferEndpoint::Local(source),
                FileTransferEndpoint::Remote { service, path },
            ) => {
                let Some(path) = resolve_copy_remote_target(
                    &source.to_string_lossy(),
                    source.is_dir(),
                    &service,
                    &path,
                    &options,
                    SftpTransferDirection::Upload,
                )?
                else {
                    return Ok(FileCopySummary {
                        bytes: 0,
                        used_local_staging: false,
                        destination_path: path.display_path,
                        copied: false,
                    });
                };
                let bytes = if source.is_dir() {
                    upload_copy_directory(
                        &service,
                        &source,
                        &path,
                        &control,
                        options.transfer_options(),
                        &progress,
                    )?
                } else {
                    service
                        .upload_remote_file_with_progress_and_control_options(
                            source,
                            &path,
                            control,
                            options.transfer_options().clone(),
                            {
                                let progress = progress.clone();
                                move |value| progress.update(value)
                            },
                        )?
                        .bytes
                };
                Ok(FileCopySummary {
                    bytes,
                    used_local_staging: false,
                    destination_path: path.display_path,
                    copied: true,
                })
            }
            (
                FileTransferEndpoint::Remote { service, path },
                FileTransferEndpoint::Local(destination),
            ) => {
                let summary = service.download_remote_path_with_progress_and_path_options(
                    &path,
                    destination,
                    control,
                    options,
                    {
                        let progress = progress.clone();
                        move |value| progress.update(value)
                    },
                )?;
                Ok(FileCopySummary {
                    bytes: summary.bytes,
                    used_local_staging: false,
                    destination_path: summary.local_path.to_string_lossy().into_owned(),
                    copied: !summary.skipped,
                })
            }
            (
                FileTransferEndpoint::Remote {
                    service: source,
                    path: source_path,
                },
                FileTransferEndpoint::Remote {
                    service: destination,
                    path: destination_path,
                },
            ) => {
                let source_is_directory =
                    source.remote_file_properties(&source_path)?.is_directory();
                let Some(destination_path) = resolve_copy_remote_target(
                    &source_path.display_path,
                    source_is_directory,
                    &destination,
                    &destination_path,
                    &options,
                    SftpTransferDirection::Copy,
                )?
                else {
                    return Ok(FileCopySummary {
                        bytes: 0,
                        used_local_staging: false,
                        destination_path: destination_path.display_path,
                        copied: false,
                    });
                };
                if source.backend()? == RemoteFileBackendKind::Sftp
                    && destination.backend()? == RemoteFileBackendKind::Sftp
                {
                    let bytes = source.sftp()?.copy_remote_path_to(
                        &source_path,
                        &destination.sftp()?,
                        &destination_path,
                        control,
                        options.transfer_options().clone(),
                        progress.clone(),
                    )?;
                    return Ok(FileCopySummary {
                        bytes,
                        used_local_staging: false,
                        destination_path: destination_path.display_path,
                        copied: true,
                    });
                }
                let staging = RemoteCopyStaging::new()?;
                let local = staging.path().join("payload");
                let downloaded = source.download_remote_path_with_progress_and_path_options(
                    &source_path,
                    &local,
                    control.clone(),
                    SftpPathTransferOptions::default(),
                    {
                        let progress = progress.clone();
                        move |value| progress.update(value)
                    },
                )?;
                progress.next_stage(downloaded.bytes);
                let uploaded_bytes = if downloaded.local_path.is_dir() {
                    upload_copy_directory(
                        &destination,
                        &downloaded.local_path,
                        &destination_path,
                        &control,
                        options.transfer_options(),
                        &progress,
                    )?
                } else {
                    destination
                        .upload_remote_file_with_progress_and_control_options(
                            &downloaded.local_path,
                            &destination_path,
                            control,
                            options.transfer_options().clone(),
                            {
                                let progress = progress.clone();
                                move |value| progress.update(value)
                            },
                        )?
                        .bytes
                };
                Ok(FileCopySummary {
                    bytes: downloaded.bytes.min(uploaded_bytes),
                    used_local_staging: true,
                    destination_path: destination_path.display_path,
                    copied: true,
                })
            }
        }
    }
}

/// A raw destination directory cannot be routed through the older string-only
/// recursive upload API. Join every child with the destination codec instead.
fn upload_copy_directory(
    service: &RemoteFileService,
    source: &Path,
    destination: &RemoteFilePath,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &CopyProgress,
) -> anyhow::Result<u64> {
    let mut pending = vec![(source.to_path_buf(), destination.clone())];
    let mut bytes = 0_u64;
    while let Some((local, remote)) = pending.pop() {
        control.wait_if_paused_blocking()?;
        let metadata = fs::symlink_metadata(&local)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "copying symbolic links is not supported"
        );
        if metadata.is_dir() {
            if let Err(error) = service.create_dir_remote_path(&remote, Some(0o755)) {
                let properties = service.remote_file_properties(&remote)?;
                if properties.file_type != SftpFileType::Directory {
                    return Err(error);
                }
            }
            for child in fs::read_dir(&local)? {
                let child = child?;
                let name = child.file_name().into_string().map_err(|_| {
                    anyhow::anyhow!("local filename cannot be represented at destination")
                })?;
                pending.push((child.path(), service.child_path(&remote, &name)?));
            }
        } else {
            anyhow::ensure!(metadata.is_file(), "copying special files is not supported");
            let reporter = progress.clone();
            let mut previous = 0;
            let uploaded = service.upload_remote_file_with_progress_and_control_options(
                local,
                &remote,
                control.clone(),
                options.clone(),
                move |value| {
                    reporter.add_bytes(value.bytes_transferred.saturating_sub(previous));
                    previous = value.bytes_transferred;
                },
            )?;
            bytes = bytes.saturating_add(uploaded.bytes);
        }
        progress.finish_item();
    }
    Ok(bytes)
}

struct RemoteCopyStaging(PathBuf);

impl RemoteCopyStaging {
    fn new() -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join(format!("nyaterm-copy-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for RemoteCopyStaging {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            tracing::warn!(error = %error, "failed to remove remote copy staging directory");
        }
    }
}

fn copy_local_path(
    source: &Path,
    destination: &Path,
    control: &SftpTransferControl,
    options: &SftpTransferOptions,
    progress: &CopyProgress,
) -> anyhow::Result<u64> {
    use std::io::{Read as _, Write as _};

    control.wait_if_paused_blocking()?;
    let metadata = fs::symlink_metadata(source)?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "copying local symbolic links is not supported"
    );
    let bytes = if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        let mut bytes = 0_u64;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            bytes = bytes.saturating_add(copy_local_path(
                &entry.path(),
                &destination.join(entry.file_name()),
                control,
                options,
                progress,
            )?);
        }
        bytes
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if destination.exists() {
            anyhow::ensure!(
                !same_file::is_same_file(source, destination)?,
                "cannot copy a local file onto itself"
            );
        }
        let mut input = crate::download_path::open_without_following(source, false, false)?;
        let root = crate::download_path::root_for_target(destination);
        let (temporary, mut output) =
            crate::download_path::DownloadTemporary::create(root, destination)?;
        let mut buffer = vec![0_u8; options.buffer_size_bytes()];
        let mut bytes = 0_u64;
        loop {
            control.wait_if_paused_blocking()?;
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            bytes = bytes.saturating_add(read as u64);
            progress.add_bytes(read as u64);
        }
        output.sync_all()?;
        drop(output);
        control.wait_if_paused_blocking()?;
        temporary.commit(root, destination)?;
        fs::set_permissions(destination, metadata.permissions())?;
        bytes
    };
    progress.finish_item();
    Ok(bytes)
}

fn resolve_copy_local_target(
    source: &Path,
    requested: &Path,
    options: &SftpPathTransferOptions,
    direction: SftpTransferDirection,
) -> anyhow::Result<Option<PathBuf>> {
    if source == requested || (source.is_dir() && requested.starts_with(source)) {
        anyhow::bail!("cannot copy a local path onto or into itself");
    }
    if !requested.exists() {
        return Ok(Some(requested.to_path_buf()));
    }
    match duplicate_decision(
        options.duplicate_policy(),
        options.duplicate_resolver(),
        direction,
        &source.to_string_lossy(),
        &requested.to_string_lossy(),
        source.is_dir(),
    )? {
        SftpDuplicateDecision::Overwrite => Ok(Some(requested.to_path_buf())),
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => Ok(Some(unique_local_path(requested))),
    }
}

fn resolve_copy_remote_target(
    source_path: &str,
    source_is_directory: bool,
    service: &RemoteFileService,
    requested: &RemoteFilePath,
    options: &SftpPathTransferOptions,
    direction: SftpTransferDirection,
) -> anyhow::Result<Option<RemoteFilePath>> {
    let parent = requested.parent()?;
    // A denied stat is not proof that a target is absent. Listing the parent
    // also preserves opaque child identities and reports permission/disconnect errors.
    let entries = service.list_dir_path(&parent)?;
    let exists = |path: &RemoteFilePath| {
        entries.iter().any(|entry| {
            let other = entry.remote_path();
            match (&other.raw_path_token, &path.raw_path_token) {
                (Some(_), Some(_)) => other.identity_key() == path.identity_key(),
                _ => other.display_path == path.display_path,
            }
        })
    };
    if !exists(requested) {
        return Ok(Some(requested.clone()));
    }
    match duplicate_decision(
        options.duplicate_policy(),
        options.duplicate_resolver(),
        direction,
        source_path,
        &requested.display_path,
        source_is_directory,
    )? {
        SftpDuplicateDecision::Overwrite => Ok(Some(requested.clone())),
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => {
            for index in 1.. {
                let parent = requested.parent()?;
                let name = crate::file_browser::file_browser_name(
                    crate::file_browser::FileBrowserBackendKind::Remote,
                    &requested.display_path,
                );
                let candidate = service.child_path(&parent, &format!("{name} ({index})"))?;
                if !exists(&candidate) {
                    return Ok(Some(candidate));
                }
            }
            unreachable!()
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn download_shell_path<F>(
    service: &RemoteFileService,
    kind: RemoteFileBackendKind,
    remote_path: &str,
    local_path: &Path,
    control: &SftpTransferControl,
    progress: F,
    download_root: &Path,
    options: &SftpTransferOptions,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send + 'static,
{
    let shell = service.shell(kind);
    let callback = Arc::new(Mutex::new(progress));
    let mut pending = vec![(remote_path.to_string(), local_path.to_path_buf())];
    let mut bytes = 0_u64;
    let mut completed = 0_u64;
    while let Some((remote, local)) = pending.pop() {
        control.wait_if_paused_blocking()?;
        crate::download_path::ensure_no_symlink(download_root, &local)?;
        let properties = shell_properties(&shell, &RemoteFilePath::new(&remote))?;
        ensure_supported_download_source(properties.file_type)?;
        if properties.is_directory() {
            fs::create_dir_all(&local)?;
            crate::download_path::ensure_no_symlink(download_root, &local)?;
            let mut target_keys = HashSet::new();
            for entry in list_shell_dir(&shell, &RemoteFilePath::new(&remote), kind)? {
                control.wait_if_paused_blocking()?;
                if !crate::download_path::validate_directory_entry(&entry.name, entry.file_type)? {
                    continue;
                }
                ensure_unique_download_name(&mut target_keys, &entry.name)?;
                ensure_supported_download_source(entry.file_type)?;
                pending.push((entry.path, local.join(entry.name)));
            }
        } else {
            if let Some(parent) = local.parent() {
                fs::create_dir_all(parent)?;
            }
            let report = callback.clone();
            let remote_name = remote.clone();
            let local_name = local.clone();
            let offset = bytes;
            bytes = bytes.saturating_add(shell.download_stream(
                &remote,
                &local,
                download_root,
                properties.size,
                if options.preserve_timestamps {
                    properties.modified_at
                } else {
                    None
                },
                control.clone(),
                move |current| {
                    report.lock().expect("shell progress lock")(SftpTransferProgress {
                        remote_path: remote_name.clone(),
                        local_path: local_name.clone(),
                        bytes_transferred: offset.saturating_add(current),
                        total_bytes: None,
                        item_count_completed: Some(completed),
                        item_count_total: None,
                    });
                },
            )?);
        }
        completed = completed.saturating_add(1);
        callback.lock().expect("shell progress lock")(SftpTransferProgress {
            remote_path: remote,
            local_path: local,
            bytes_transferred: bytes,
            total_bytes: None,
            item_count_completed: Some(completed),
            item_count_total: None,
        });
    }
    callback.lock().expect("shell progress lock")(SftpTransferProgress {
        remote_path: remote_path.to_string(),
        local_path: local_path.to_path_buf(),
        bytes_transferred: bytes,
        total_bytes: Some(bytes),
        item_count_completed: Some(completed),
        item_count_total: Some(completed),
    });
    Ok(bytes)
}

fn ensure_supported_download_source(file_type: SftpFileType) -> anyhow::Result<()> {
    match file_type {
        SftpFileType::File | SftpFileType::Directory => Ok(()),
        SftpFileType::Symlink => anyhow::bail!("symbolic links are not supported in downloads"),
        SftpFileType::Other => anyhow::bail!("remote source is not a regular file or directory"),
    }
}

fn ensure_unique_download_name(
    target_keys: &mut HashSet<String>,
    name: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        target_keys.insert(crate::download_path::target_key(name)),
        "recursive download contains conflicting local names"
    );
    Ok(())
}

fn upload_shell_path<F>(
    service: &RemoteFileService,
    kind: RemoteFileBackendKind,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    transfer_options: &SftpTransferOptions,
    progress: F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress) + Send + 'static,
{
    let shell = service.shell(kind);
    let callback = Arc::new(Mutex::new(progress));
    let mut pending = vec![(local_path.to_path_buf(), remote_path.to_string())];
    let mut bytes = 0_u64;
    let mut completed = 0_u64;
    while let Some((local, remote)) = pending.pop() {
        control.wait_if_paused_blocking()?;
        if local.is_dir() {
            service.create_dir_path(&remote, None)?;
            for entry in fs::read_dir(&local)? {
                control.wait_if_paused_blocking()?;
                let entry = entry?;
                pending.push((
                    entry.path(),
                    remote_join(&remote, &entry.file_name().to_string_lossy()),
                ));
            }
        } else {
            let report = callback.clone();
            let remote_name = remote.clone();
            let local_name = local.clone();
            let offset = bytes;
            bytes = bytes.saturating_add(shell.upload_stream(
                &local,
                &remote,
                control.clone(),
                transfer_options,
                move |current| {
                    report.lock().expect("shell progress lock")(SftpTransferProgress {
                        remote_path: remote_name.clone(),
                        local_path: local_name.clone(),
                        bytes_transferred: offset.saturating_add(current),
                        total_bytes: None,
                        item_count_completed: Some(completed),
                        item_count_total: None,
                    });
                },
            )?);
        }
        completed = completed.saturating_add(1);
        callback.lock().expect("shell progress lock")(SftpTransferProgress {
            remote_path: remote,
            local_path: local,
            bytes_transferred: bytes,
            total_bytes: None,
            item_count_completed: Some(completed),
            item_count_total: None,
        });
    }
    callback.lock().expect("shell progress lock")(SftpTransferProgress {
        remote_path: remote_path.to_string(),
        local_path: local_path.to_path_buf(),
        bytes_transferred: bytes,
        total_bytes: Some(bytes),
        item_count_completed: Some(completed),
        item_count_total: Some(completed),
    });
    Ok(bytes)
}

fn resolve_shell_download_target(
    remote_path: &str,
    requested: &Path,
    is_directory: bool,
    options: &SftpPathTransferOptions,
) -> anyhow::Result<Option<PathBuf>> {
    let key = crate::sftp_transfer_types::SftpDuplicateCacheKey::Download {
        remote_path: remote_path.as_bytes().to_vec(),
        local_path: requested.to_path_buf(),
        is_directory,
    };
    if !crate::download_path::target_exists(requested)?
        && options.reserve_download_target(requested, &key)?
    {
        return Ok(Some(requested.to_path_buf()));
    }
    match duplicate_decision(
        options.duplicate_policy(),
        options.duplicate_resolver(),
        SftpTransferDirection::Download,
        remote_path,
        &requested.to_string_lossy(),
        is_directory,
    )? {
        SftpDuplicateDecision::Overwrite => {
            anyhow::ensure!(
                options.reserve_download_target(requested, &key)?,
                "download target is reserved by another item in this batch"
            );
            Ok(Some(requested.to_path_buf()))
        }
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => {
            for index in 1..=999 {
                let candidate = local_conflict_candidate(requested, index);
                if !crate::download_path::target_exists(&candidate)?
                    && options.reserve_download_target(&candidate, &key)?
                {
                    return Ok(Some(candidate));
                }
            }
            anyhow::bail!("unable to find a non-conflicting local download path")
        }
    }
}

fn resolve_shell_upload_target(
    service: &RemoteFileService,
    kind: RemoteFileBackendKind,
    local_path: &Path,
    requested: &str,
    policy: SftpDuplicatePolicy,
    resolver: Option<&dyn crate::SftpDuplicateResolver>,
) -> anyhow::Result<Option<String>> {
    let exists = shell_properties(&service.shell(kind), &RemoteFilePath::new(requested)).is_ok();
    if !exists {
        return Ok(Some(requested.to_string()));
    }
    match duplicate_decision(
        policy,
        resolver,
        SftpTransferDirection::Upload,
        &local_path.to_string_lossy(),
        requested,
        local_path.is_dir(),
    )? {
        SftpDuplicateDecision::Overwrite => Ok(Some(requested.to_string())),
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => Ok(Some(unique_remote_path(service, kind, requested))),
    }
}

fn duplicate_decision(
    policy: SftpDuplicatePolicy,
    resolver: Option<&dyn crate::SftpDuplicateResolver>,
    direction: SftpTransferDirection,
    source: &str,
    target: &str,
    is_directory: bool,
) -> anyhow::Result<SftpDuplicateDecision> {
    match policy {
        SftpDuplicatePolicy::Overwrite => Ok(SftpDuplicateDecision::Overwrite),
        SftpDuplicatePolicy::Skip => Ok(SftpDuplicateDecision::Skip),
        SftpDuplicatePolicy::Rename => Ok(SftpDuplicateDecision::Rename),
        SftpDuplicatePolicy::Ask => resolver
            .ok_or_else(|| anyhow::anyhow!("duplicate resolver is required"))?
            .resolve_duplicate(&SftpDuplicateRequest {
                direction,
                source_path: source.to_string(),
                target_path: target.to_string(),
                is_directory,
            })
            .map_err(anyhow::Error::msg),
    }
}

fn unique_local_path(path: &Path) -> PathBuf {
    for index in 1.. {
        let candidate = local_conflict_candidate(path, index);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn local_conflict_candidate(path: &Path, index: usize) -> PathBuf {
    path.with_file_name(format!(
        "{} ({index}){}",
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("file"),
        path.extension()
            .and_then(|value| value.to_str())
            .map_or(String::new(), |value| format!(".{value}"))
    ))
}

fn unique_remote_path(
    service: &RemoteFileService,
    kind: RemoteFileBackendKind,
    path: &str,
) -> String {
    for index in 1.. {
        let candidate = format!("{path} ({index})");
        if shell_properties(&service.shell(kind), &RemoteFilePath::new(&candidate)).is_err() {
            return candidate;
        }
    }
    unreachable!()
}

fn list_shell_dir(
    shell: &ShellRemote,
    path: &RemoteFilePath,
    kind: RemoteFileBackendKind,
) -> anyhow::Result<Vec<SftpFileEntry>> {
    let mut entries = match kind {
        RemoteFileBackendKind::ScpEnhanced => parse_enhanced_listing(
            path,
            &shell.exec_ok(
                format!(
                    "LC_ALL=C find {} -mindepth 1 -maxdepth 1 -printf '%f\\0%y\\0%s\\0%T@\\0%m\\0%u\\0%g\\0%p\\0'",
                    shell_quote(&path.display_path)
                ),
                None,
            )?,
        ),
        RemoteFileBackendKind::ScpNormal => parse_normal_listing(
            path,
            &shell.exec_ok(
                // Numeric identities keep whitespace in account names out of the columns.
                format!("LC_ALL=C ls -la -n {}", shell_quote_path(&path.display_path)),
                None,
            )?,
        ),
        RemoteFileBackendKind::Sftp => unreachable!(),
    }?;
    for entry in entries.iter_mut().filter(|entry| entry.is_symlink()) {
        entry.symlink_target_is_directory = shell
            .exec(format!("test -d {}", shell_quote(&entry.path)), None)?
            .exit_status
            == Some(0);
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn parse_enhanced_listing(
    parent: &RemoteFilePath,
    output: &[u8],
) -> anyhow::Result<Vec<SftpFileEntry>> {
    let fields = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut entries = Vec::new();
    for fields in fields.as_chunks::<8>().0 {
        let name = String::from_utf8_lossy(fields[0]).into_owned();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let kind = fields[1].first().copied();
        let file_type = match kind {
            Some(b'd') => SftpFileType::Directory,
            Some(b'l') => SftpFileType::Symlink,
            Some(b'f') => SftpFileType::File,
            _ => SftpFileType::Other,
        };
        entries.push(SftpFileEntry {
            path: remote_join(&parent.display_path, &name),
            name,
            file_type,
            size: parse_bytes(fields[2]),
            modified_at: parse_decimal_seconds(fields[3]),
            permissions: parse_octal_bytes(fields[4]),
            owner: String::from_utf8_lossy(fields[5]).into_owned(),
            group: String::from_utf8_lossy(fields[6]).into_owned(),
            raw_path_token: None,
            symlink_target_is_directory: false,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn parse_normal_listing(
    parent: &RemoteFilePath,
    output: &[u8],
) -> anyhow::Result<Vec<SftpFileEntry>> {
    let text = String::from_utf8_lossy(output);
    let mut entries = Vec::new();
    for line in text.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 9 || columns[0].len() < 10 || line.starts_with("total ") {
            continue;
        }
        let raw_name = columns[8..].join(" ");
        let name = raw_name
            .split(" -> ")
            .next()
            .unwrap_or(&raw_name)
            .to_string();
        if name == "." || name == ".." {
            continue;
        }
        let file_type = match columns[0].as_bytes()[0] {
            b'd' => SftpFileType::Directory,
            b'l' => SftpFileType::Symlink,
            b'-' => SftpFileType::File,
            _ => SftpFileType::Other,
        };
        entries.push(SftpFileEntry {
            path: remote_join(&parent.display_path, &name),
            name,
            file_type,
            size: columns[4].parse().ok(),
            modified_at: None,
            permissions: symbolic_permissions_to_mode(columns[0]),
            owner: columns[2].to_string(),
            group: columns[3].to_string(),
            raw_path_token: None,
            symlink_target_is_directory: false,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn shell_properties(
    shell: &ShellRemote,
    path: &RemoteFilePath,
) -> anyhow::Result<SftpFileProperties> {
    if shell.kind() == RemoteFileBackendKind::ScpNormal {
        return normal_shell_properties(shell, path);
    }
    let output = shell.exec_ok(
        format!(
            "LC_ALL=C stat --printf '%F\\0%s\\0%Y\\0%X\\0%a\\0%U\\0%G\\0%u\\0%g' -- {}",
            shell_quote(&path.display_path)
        ),
        None,
    )?;
    let fields = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.len() < 9 {
        anyhow::bail!("Unexpected remote stat output");
    }
    let description = String::from_utf8_lossy(fields[0]).to_ascii_lowercase();
    let file_type = if description.contains("directory") {
        SftpFileType::Directory
    } else if description.contains("symbolic link") {
        SftpFileType::Symlink
    } else if description.contains("regular") {
        SftpFileType::File
    } else {
        SftpFileType::Other
    };
    let permissions = parse_octal_bytes(fields[4]);
    let symlink_target_is_directory = file_type == SftpFileType::Symlink
        && shell
            .exec(format!("test -d {}", shell_quote(&path.display_path)), None)?
            .exit_status
            == Some(0);
    Ok(SftpFileProperties {
        symlink_target: None,
        name: path
            .display_path
            .rsplit('/')
            .next()
            .unwrap_or(&path.display_path)
            .to_string(),
        path: path.display_path.clone(),
        file_type,
        size: parse_bytes(fields[1]),
        permissions,
        permissions_symbolic: permissions
            .map(|mode| format_permissions(file_type, mode))
            .unwrap_or_else(|| "-".to_string()),
        owner: String::from_utf8_lossy(fields[5]).into_owned(),
        group: String::from_utf8_lossy(fields[6]).into_owned(),
        uid: parse_bytes(fields[7]).and_then(|value| value.try_into().ok()),
        gid: parse_bytes(fields[8]).and_then(|value| value.try_into().ok()),
        modified_at: parse_bytes(fields[2]).and_then(|value| value.try_into().ok()),
        accessed_at: parse_bytes(fields[3]).and_then(|value| value.try_into().ok()),
        raw_path_token: path.raw_path_token.clone(),
        symlink_target_is_directory,
    })
}

fn normal_shell_properties(
    shell: &ShellRemote,
    path: &RemoteFilePath,
) -> anyhow::Result<SftpFileProperties> {
    let output = shell.exec_ok(
        format!(
            "LC_ALL=C ls -ld -n {}",
            shell_quote_path(&path.display_path)
        ),
        None,
    )?;
    let line = String::from_utf8_lossy(&output)
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Unexpected remote ls output"))?
        .to_string();
    let columns = line.split_whitespace().collect::<Vec<_>>();
    if columns.len() < 9 || columns[0].len() < 10 {
        anyhow::bail!("Unexpected remote ls output");
    }
    let file_type = match columns[0].as_bytes()[0] {
        b'd' => SftpFileType::Directory,
        b'l' => SftpFileType::Symlink,
        b'-' => SftpFileType::File,
        _ => SftpFileType::Other,
    };
    let permissions = symbolic_permissions_to_mode(columns[0]);
    let symlink_target_is_directory = file_type == SftpFileType::Symlink
        && shell
            .exec(format!("test -d {}", shell_quote(&path.display_path)), None)?
            .exit_status
            == Some(0);
    Ok(SftpFileProperties {
        symlink_target: None,
        name: path
            .display_path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&path.display_path)
            .to_string(),
        path: path.display_path.clone(),
        file_type,
        size: columns[4].parse().ok(),
        permissions,
        permissions_symbolic: columns[0].to_string(),
        owner: columns[2].to_string(),
        group: columns[3].to_string(),
        uid: None,
        gid: None,
        modified_at: None,
        accessed_at: None,
        raw_path_token: path.raw_path_token.clone(),
        symlink_target_is_directory,
    })
}

fn update_shell_attributes(
    shell: &ShellRemote,
    path: &str,
    update: &SftpAttributeUpdate,
) -> anyhow::Result<()> {
    let recursive = if update.recursive { "-R " } else { "" };
    if let Some(mode) = update.mode {
        shell.exec_ok(
            format!("chmod {recursive}{mode:o} {}", shell_quote_path(path)),
            None,
        )?;
    }
    if let Some(owner) = update
        .owner
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        shell.exec_ok(
            format!(
                "chown {recursive}-- {} {}",
                shell_quote(owner),
                shell_quote_path(path)
            ),
            None,
        )?;
    }
    if let Some(group) = update
        .group
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        shell.exec_ok(
            format!(
                "chgrp {recursive}-- {} {}",
                shell_quote(group),
                shell_quote_path(path)
            ),
            None,
        )?;
    }
    Ok(())
}

fn shell_text_metadata(properties: &SftpFileProperties) -> RemoteTextMetadata {
    RemoteTextMetadata {
        size: properties.size.unwrap_or(0),
        modified_at: properties.modified_at.map(u64::from),
    }
}

fn read_shell_text_document(
    shell: &ShellRemote,
    path: &RemoteFilePath,
    max_bytes: u64,
) -> anyhow::Result<RemoteTextDocument> {
    let before = shell_properties(shell, path)?;
    if before.is_directory() {
        anyhow::bail!("Directories cannot be opened as text");
    }
    let before_metadata = shell_text_metadata(&before);
    if before_metadata.size > max_bytes {
        anyhow::bail!("File is too large to open as text");
    }
    let bytes = shell.exec_ok(format!("cat -- {}", shell_quote(&path.display_path)), None)?;
    if bytes.len() as u64 > max_bytes {
        anyhow::bail!("File is too large to open as text");
    }
    if bytes.contains(&0) {
        anyhow::bail!("Binary files are not supported by the built-in editor");
    }
    let after = shell_properties(shell, path)?;
    let after_metadata = shell_text_metadata(&after);
    if !metadata_is_stable(&before_metadata, &after_metadata)
        || after_metadata.size != bytes.len() as u64
    {
        anyhow::bail!("Remote file changed while it was being read");
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("Only UTF-8 text files are supported"))?;
    let revision = RemoteTextRevision::from_bytes(content.as_bytes(), after_metadata);
    Ok(RemoteTextDocument {
        path: path.display_path.clone(),
        content,
        revision,
    })
}

fn write_shell_text_document(
    shell: &ShellRemote,
    path: &RemoteFilePath,
    content: &str,
    expected_revision: Option<&RemoteTextRevision>,
    force: bool,
) -> anyhow::Result<RemoteTextWriteResult> {
    let expected_revision = match (force, expected_revision) {
        (false, None) => anyhow::bail!("Remote text revision is required for a safe save"),
        (true, _) => None,
        (false, revision) => revision,
    };
    let temporary = format!(
        "{}.nyaterm-edit-{}",
        path.display_path,
        uuid::Uuid::new_v4().simple()
    );
    let cleanup_temporary = || {
        let _ = shell.exec(format!("rm -f -- {}", shell_quote(&temporary)), None);
    };
    let write_temporary = format!(
        "umask 077; cat > {} && test $(wc -c < {}) -eq {}",
        shell_quote(&temporary),
        shell_quote(&temporary),
        content.len()
    );
    if let Err(error) = shell.exec_ok(write_temporary, Some(content.as_bytes().to_vec())) {
        cleanup_temporary();
        return Err(error);
    }

    if let Some(expected) = expected_revision {
        let current_properties = match shell_properties(shell, path) {
            Ok(properties) => properties,
            Err(error) => {
                cleanup_temporary();
                return Err(error);
            }
        };
        if shell_text_metadata(&current_properties) != expected.metadata {
            cleanup_temporary();
            return Ok(RemoteTextWriteResult::Conflict);
        }
        let current = match read_shell_text_document(shell, path, expected.metadata.size) {
            Ok(current) => current,
            Err(error) => {
                cleanup_temporary();
                return Err(error);
            }
        };
        if current.revision != *expected {
            cleanup_temporary();
            return Ok(RemoteTextWriteResult::Conflict);
        }
    }

    let replacement_properties = match shell_file_properties(shell, path) {
        Ok(properties) => properties,
        Err(error) => {
            cleanup_temporary();
            return Err(error);
        }
    };
    if expected_revision
        .is_some_and(|expected| shell_text_metadata(&replacement_properties) != expected.metadata)
    {
        cleanup_temporary();
        return Ok(RemoteTextWriteResult::Conflict);
    }
    // POSIX shell fallback has no universal compare-and-swap primitive. Keep the
    // verified revision check adjacent to the final atomic rename.
    let replace =
        match shell_file_replace_command(&replacement_properties, &temporary, &path.display_path) {
            Ok(command) => command,
            Err(error) => {
                cleanup_temporary();
                return Err(error);
            }
        };
    if let Err(error) = shell.exec_ok(replace, None) {
        cleanup_temporary();
        return Err(error);
    }
    let metadata = shell_text_metadata(&shell_properties(shell, path)?);
    if metadata.size != content.len() as u64 {
        anyhow::bail!("Remote text save verification failed");
    }
    Ok(RemoteTextWriteResult::Saved {
        revision: RemoteTextRevision::from_bytes(content.as_bytes(), metadata),
    })
}

fn write_shell_text(
    shell: &ShellRemote,
    path: &RemoteFilePath,
    content: &str,
    expected_modified_at: Option<u64>,
    expected_size: Option<u64>,
    force: bool,
) -> anyhow::Result<SftpWriteTextResult> {
    let properties = shell_file_properties(shell, path)?;
    let modified_at = u64::from(properties.modified_at.unwrap_or(0));
    let size = properties.size.unwrap_or(0);
    if !force
        && (expected_modified_at.is_some_and(|expected| expected != modified_at)
            || expected_size.is_some_and(|expected| expected != size))
    {
        return Ok(SftpWriteTextResult::Conflict { modified_at, size });
    }
    let temporary = format!(
        "{}.nyaterm-edit-{}",
        path.display_path,
        uuid::Uuid::new_v4().simple()
    );
    let command =
        shell_file_upload_command(&properties, &temporary, &path.display_path, content.len())?;
    if let Err(error) = shell.exec_ok(command, Some(content.as_bytes().to_vec())) {
        let _ = shell.exec(format!("rm -f -- {}", shell_quote(&temporary)), None);
        return Err(error);
    }
    let properties = shell_properties(shell, path)?;
    Ok(SftpWriteTextResult::Saved {
        modified_at: u64::from(properties.modified_at.unwrap_or(0)),
        size: properties.size.unwrap_or(content.len() as u64),
    })
}

fn ensure_safe_delete_target(path: &str) -> anyhow::Result<()> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty()
        || matches!(trimmed, "." | "..")
        || trimmed == "/"
        || trimmed.split('/').any(|component| component == "..")
    {
        anyhow::bail!("Refusing to delete unsafe remote path");
    }
    Ok(())
}

fn remote_join(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{}/{child}", parent.trim_end_matches('/'))
    }
}

fn parse_bytes(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}

fn parse_decimal_seconds(value: &[u8]) -> Option<u32> {
    std::str::from_utf8(value)
        .ok()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn parse_octal_bytes(value: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(value).ok()?.trim(), 8).ok()
}

fn symbolic_permissions_to_mode(value: &str) -> Option<u32> {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() < 10 {
        return None;
    }
    let mut mode = 0;
    for (index, bit) in [
        (1, 0o400),
        (2, 0o200),
        (3, 0o100),
        (4, 0o040),
        (5, 0o020),
        (6, 0o010),
        (7, 0o004),
        (8, 0o002),
        (9, 0o001),
    ] {
        if matches!(chars[index], 'r' | 'w' | 'x' | 's' | 't') {
            mode |= bit;
        }
    }
    if matches!(chars[3], 's' | 'S') {
        mode |= 0o4000;
    }
    if matches!(chars[6], 's' | 'S') {
        mode |= 0o2000;
    }
    if matches!(chars[9], 't' | 'T') {
        mode |= 0o1000;
    }
    Some(mode)
}

fn format_permissions(file_type: SftpFileType, mode: u32) -> String {
    let mut result = String::with_capacity(10);
    result.push(match file_type {
        SftpFileType::Directory => 'd',
        SftpFileType::Symlink => 'l',
        _ => '-',
    });
    result.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    result.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    result.push(special_execute(mode, 0o100, 0o4000, 's', 'S'));
    result.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    result.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    result.push(special_execute(mode, 0o010, 0o2000, 's', 'S'));
    result.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    result.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    result.push(special_execute(mode, 0o001, 0o1000, 't', 'T'));
    result
}

fn special_execute(mode: u32, execute: u32, special: u32, both: char, only_special: char) -> char {
    match (mode & execute != 0, mode & special != 0) {
        (true, true) => both,
        (false, true) => only_special,
        (true, false) => 'x',
        (false, false) => '-',
    }
}

fn sort_entries(entries: &mut [SftpFileEntry]) {
    entries.sort_by(|left, right| {
        (!left.is_directory())
            .cmp(&(!right.is_directory()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use crate::{
        FileCopyRequest, FileTransferEndpoint, RemoteFilePath, SftpDuplicatePolicy, SftpFileType,
        SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions,
    };

    use super::{
        BackendProbeStage, RemoteFileBackendKind, ensure_safe_delete_target,
        ensure_supported_download_source, ensure_unique_download_name, format_permissions,
        parse_enhanced_listing, parse_normal_listing, select_backend_with_probe,
        symbolic_permissions_to_mode,
    };

    #[test]
    fn enhanced_listing_uses_nul_delimited_records() {
        let output =
            b"folder\x00d\x000\x001700000000.0\x004755\x00alice\x00staff\x00/tmp/folder\x00";
        let entries = parse_enhanced_listing(&RemoteFilePath::new("/tmp"), output).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_type, SftpFileType::Directory);
        assert_eq!(entries[0].path, "/tmp/folder");
    }

    #[test]
    fn normal_listing_preserves_spaces_and_symlink_names() {
        let output = b"-rw-r--r-- 1 alice staff 12 Jan 1 00:00 a file.txt\nlrwxrwxrwx 1 alice staff 3 Jan 1 00:00 link name -> dir\n";
        let entries = parse_normal_listing(&RemoteFilePath::new("/tmp"), output).unwrap();
        assert_eq!(entries[0].name, "a file.txt");
        assert_eq!(entries[1].name, "link name");
    }

    #[test]
    fn normal_listing_parses_numeric_ownership_and_names_with_spaces() {
        let output = concat!(
            "total 12\n",
            "drwxr-xr-x 2 1000 1001 4096 Jan 1 00:00 .\n",
            "drwxr-xr-x 2 1000 1001 4096 Jan 1 00:00 ..\n",
            "-rw-r--r-- 1 1000 1001 12 Jan 1 00:00 a file.txt\n",
            "drwxr-x--- 2 1000 1001 4096 Jan 1 2025 a directory\n",
            "lrwxrwxrwx 1 1000 1001 11 Jan 1 00:00 link name -> a directory\n",
        );
        let entries =
            parse_normal_listing(&RemoteFilePath::new("/tmp"), output.as_bytes()).unwrap();
        assert_eq!(entries.len(), 3);
        for (entry, (name, file_type, size, permissions)) in entries.iter().zip([
            ("a directory", SftpFileType::Directory, 4096, 0o750),
            ("a file.txt", SftpFileType::File, 12, 0o644),
            ("link name", SftpFileType::Symlink, 11, 0o777),
        ]) {
            assert_eq!(entry.name, name);
            assert_eq!(entry.path, format!("/tmp/{name}"));
            assert_eq!(entry.file_type, file_type);
            assert_eq!(entry.size, Some(size));
            assert_eq!(entry.permissions, Some(permissions));
            assert_eq!(entry.owner, "1000");
            assert_eq!(entry.group, "1001");
        }
    }

    #[test]
    fn dangerous_delete_targets_are_rejected() {
        for path in ["", "/", ".", "..", "/tmp/../etc"] {
            assert!(ensure_safe_delete_target(path).is_err(), "{path}");
        }
        assert!(ensure_safe_delete_target("/tmp/file").is_ok());
    }

    #[test]
    fn downloads_reject_symlink_sources_and_conflicting_names() {
        assert!(ensure_supported_download_source(SftpFileType::File).is_ok());
        assert!(ensure_supported_download_source(SftpFileType::Directory).is_ok());
        assert!(ensure_supported_download_source(SftpFileType::Symlink).is_err());
        assert!(ensure_supported_download_source(SftpFileType::Other).is_err());

        let mut target_keys = std::collections::HashSet::new();
        assert!(ensure_unique_download_name(&mut target_keys, "report.txt").is_ok());
        assert!(ensure_unique_download_name(&mut target_keys, "report.txt").is_err());
        #[cfg(any(target_os = "macos", windows))]
        assert!(ensure_unique_download_name(&mut target_keys, "REPORT.TXT").is_err());
    }

    #[test]
    fn shell_download_jobs_reserve_renamed_targets_before_commit() -> anyhow::Result<()> {
        for names in [["foo", "foo (1)"], ["foo (1)", "foo"]] {
            let root = std::env::temp_dir()
                .join(format!("nyaterm-shell-reserved-{}", nyaterm_core::uuid()));
            std::fs::create_dir(&root)?;
            std::fs::write(root.join("foo"), b"existing")?;
            let options = crate::SftpPathTransferOptions::new(
                crate::SftpDuplicatePolicy::Rename,
                None,
                crate::SftpTransferOptions::default(),
            );
            let mut targets = Vec::new();
            for name in names {
                let target = super::resolve_shell_download_target(
                    name,
                    &root.join(name),
                    false,
                    &options.clone_for_download_batch(),
                )?
                .expect("selected target");
                targets.push((name, target));
            }
            assert_ne!(targets[0].1, targets[1].1);
            for (name, target) in &targets {
                crate::download_path::staged_write_file(&root, target, name.as_bytes())?;
            }
            assert_eq!(std::fs::read(root.join("foo"))?, b"existing");
            for (name, target) in targets {
                assert_eq!(std::fs::read(target)?, name.as_bytes());
            }
            std::fs::remove_dir_all(root)?;
        }
        Ok(())
    }

    #[test]
    fn local_copy_applies_skip_and_rename_duplicate_policies() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-file-copy");
        std::fs::create_dir_all(root.path()).unwrap();
        let source = root.join("source.txt");
        let target = root.join("target.txt");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&target, "target").unwrap();

        let request = |policy| FileCopyRequest {
            source: FileTransferEndpoint::Local(source.clone()),
            destination: FileTransferEndpoint::Local(target.clone()),
            control: SftpTransferControl::default(),
            options: SftpPathTransferOptions::new(policy, None, SftpTransferOptions::default()),
        };
        let skipped = request(SftpDuplicatePolicy::Skip).execute().unwrap();
        assert!(!skipped.copied);
        assert_eq!(skipped.bytes, 0);
        assert_eq!(skipped.destination_path, target.to_string_lossy());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "target");
        let renamed = request(SftpDuplicatePolicy::Rename).execute().unwrap();
        assert!(renamed.copied);
        assert_eq!(
            renamed.destination_path,
            root.join("target (1).txt").to_string_lossy()
        );
        assert_eq!(
            std::fs::read_to_string(root.join("target (1).txt")).unwrap(),
            "source"
        );
    }

    #[test]
    fn special_permission_bits_round_trip() {
        let mode = symbolic_permissions_to_mode("-rwsr-sr-t").unwrap();
        assert_eq!(mode, 0o7755);
        assert_eq!(format_permissions(SftpFileType::File, mode), "-rwsr-sr-t");
    }

    #[test]
    fn cached_backend_is_probed_before_the_full_fallback_order() {
        let mut attempts = Vec::new();
        let selected = select_backend_with_probe(
            Some(RemoteFileBackendKind::ScpEnhanced),
            |backend, stage| {
                attempts.push((backend, stage));
                if stage == BackendProbeStage::Cached {
                    Err("stale cache")
                } else if backend == RemoteFileBackendKind::ScpNormal {
                    Ok(())
                } else {
                    Err("unavailable")
                }
            },
        )
        .unwrap();

        assert_eq!(selected.backend, RemoteFileBackendKind::ScpNormal);
        assert_eq!(
            attempts,
            vec![
                (
                    RemoteFileBackendKind::ScpEnhanced,
                    BackendProbeStage::Cached
                ),
                (RemoteFileBackendKind::Sftp, BackendProbeStage::Full),
                (RemoteFileBackendKind::ScpEnhanced, BackendProbeStage::Full),
                (RemoteFileBackendKind::ScpNormal, BackendProbeStage::Full),
            ]
        );
    }

    #[test]
    fn each_backend_can_be_selected_and_complete_failure_is_reported() {
        for expected in [
            RemoteFileBackendKind::Sftp,
            RemoteFileBackendKind::ScpEnhanced,
            RemoteFileBackendKind::ScpNormal,
        ] {
            let selected = select_backend_with_probe(None, |backend, _| {
                (backend == expected).then_some(()).ok_or("unavailable")
            })
            .unwrap();
            assert_eq!(selected.backend, expected);
        }

        assert!(select_backend_with_probe(None, |_, _| Err::<(), _>("unavailable")).is_err());
    }

    #[test]
    fn a_working_cached_backend_skips_the_full_probe() {
        let mut attempts = Vec::new();
        let selected = select_backend_with_probe(
            Some(RemoteFileBackendKind::ScpEnhanced),
            |backend, stage| {
                attempts.push((backend, stage));
                Ok::<(), Infallible>(())
            },
        )
        .unwrap();

        assert!(selected.from_cache);
        assert_eq!(
            attempts,
            vec![(
                RemoteFileBackendKind::ScpEnhanced,
                BackendProbeStage::Cached
            )]
        );
    }
}
