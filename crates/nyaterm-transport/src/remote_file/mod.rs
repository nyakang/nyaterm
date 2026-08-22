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

mod shell;

use shell::{ShellRemote, shell_quote};

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
        Self {
            config,
            multiplex,
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

    /// Returns whether remote operations use an already-authenticated SSH link.
    pub fn is_multiplexed(&self) -> bool {
        self.multiplex.is_some()
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

    fn sftp(&self) -> anyhow::Result<SftpService> {
        match self.multiplex.clone() {
            Some(multiplex) => SftpService::with_multiplex(self.config.clone(), multiplex),
            None => Ok(SftpService::new(self.config.clone())),
        }
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
            kind => list_shell_dir(&self.shell(kind), path, kind),
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
            kind => shell_properties(&self.shell(kind), path),
        }
    }

    pub fn create_dir_path(&self, path: impl AsRef<str>, mode: Option<u32>) -> anyhow::Result<()> {
        match self.backend()? {
            RemoteFileBackendKind::Sftp => self.sftp()?.create_dir_path(path, mode),
            kind => {
                let path = shell_quote(path.as_ref());
                let mode = mode.map_or(String::new(), |mode| {
                    format!(" && chmod {mode:o} -- {path}")
                });
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
                let path = shell_quote(path.as_ref());
                let mode = mode.map_or(String::new(), |mode| {
                    format!(" && chmod {mode:o} -- {path}")
                });
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
                    format!("rm -rf -- {}", shell_quote(&path.display_path)),
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
            kind => update_shell_attributes(&self.shell(kind), &path.display_path, &update),
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
                let bytes = self
                    .shell(kind)
                    .exec_ok(format!("cat -- {}", shell_quote(&remote_path)), None)?;
                fs::write(&local_path, &bytes)?;
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
        mut progress: F,
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
                let bytes = self.shell(kind).exec_ok(
                    format!("cat -- {}", shell_quote(&remote_path.display_path)),
                    None,
                )?;
                control.wait_if_paused_blocking()?;
                fs::write(&local_path, &bytes)?;
                progress(SftpTransferProgress {
                    remote_path: remote_path.display_path.clone(),
                    local_path: local_path.clone(),
                    bytes_transferred: bytes.len() as u64,
                    total_bytes: Some(bytes.len() as u64),
                    item_count_completed: Some(1),
                    item_count_total: Some(1),
                });
                Ok(SftpTransferSummary {
                    remote_path: remote_path.display_path,
                    local_path,
                    bytes: bytes.len() as u64,
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
        mut progress: F,
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
                let target = resolve_shell_download_target(
                    &remote_path.display_path,
                    &local_path,
                    options.duplicate_policy(),
                    options.duplicate_resolver(),
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
                    &mut progress,
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
                let bytes = fs::read(&local_path)?;
                let temporary = format!(
                    "{}.nyaterm-upload-{}",
                    remote_path,
                    uuid::Uuid::new_v4().simple()
                );
                let command = format!(
                    "umask 077; cat > {} && test $(wc -c < {}) -eq {} && mv -f -- {} {}",
                    shell_quote(&temporary),
                    shell_quote(&temporary),
                    bytes.len(),
                    shell_quote(&temporary),
                    shell_quote(&remote_path)
                );
                let result = self.shell(kind).exec_ok(command, Some(bytes.clone()));
                if result.is_err() {
                    let _ = self
                        .shell(kind)
                        .exec(format!("rm -f -- {}", shell_quote(&temporary)), None);
                }
                result?;
                Ok(SftpTransferSummary {
                    remote_path,
                    local_path,
                    bytes: bytes.len() as u64,
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
        mut progress: F,
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
            _ => {
                control.wait_if_paused_blocking()?;
                let summary = self.upload_file(&local_path, &remote_path)?;
                control.wait_if_paused_blocking()?;
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

    pub fn upload_remote_file_with_progress_and_control_options<F>(
        &self,
        local_path: impl Into<PathBuf>,
        remote_path: &RemoteFilePath,
        control: SftpTransferControl,
        options: SftpTransferOptions,
        mut progress: F,
    ) -> anyhow::Result<SftpTransferSummary>
    where
        F: FnMut(SftpTransferProgress) + Send + 'static,
    {
        let local_path = local_path.into();
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
            _ => {
                control.wait_if_paused_blocking()?;
                let summary = self.upload_file(&local_path, &remote_path.display_path)?;
                control.wait_if_paused_blocking()?;
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
        mut progress: F,
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
                    &mut progress,
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

#[derive(Debug, Clone)]
pub struct FileCopyRequest {
    pub source: FileTransferEndpoint,
    pub destination: FileTransferEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCopySummary {
    pub bytes: u64,
    pub used_local_staging: bool,
}

impl FileCopyRequest {
    pub fn execute(self) -> anyhow::Result<FileCopySummary> {
        match (self.source, self.destination) {
            (FileTransferEndpoint::Local(source), FileTransferEndpoint::Local(destination)) => {
                Ok(FileCopySummary {
                    bytes: copy_local_path(&source, &destination)?,
                    used_local_staging: false,
                })
            }
            (
                FileTransferEndpoint::Local(source),
                FileTransferEndpoint::Remote { service, path },
            ) => {
                let summary = if source.is_dir() {
                    service.upload_path_with_progress_and_path_options(
                        source,
                        &path.display_path,
                        SftpTransferControl::default(),
                        SftpPathTransferOptions::default(),
                        |_| {},
                    )?
                } else {
                    service.upload_remote_file_with_progress_and_control_options(
                        source,
                        &path,
                        SftpTransferControl::default(),
                        SftpTransferOptions::default(),
                        |_| {},
                    )?
                };
                Ok(FileCopySummary {
                    bytes: summary.bytes,
                    used_local_staging: false,
                })
            }
            (
                FileTransferEndpoint::Remote { service, path },
                FileTransferEndpoint::Local(destination),
            ) => {
                let summary = service.download_remote_path_with_progress_and_path_options(
                    &path,
                    destination,
                    SftpTransferControl::default(),
                    SftpPathTransferOptions::default(),
                    |_| {},
                )?;
                Ok(FileCopySummary {
                    bytes: summary.bytes,
                    used_local_staging: false,
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
                if source.backend()? == RemoteFileBackendKind::Sftp
                    && destination.backend()? == RemoteFileBackendKind::Sftp
                {
                    let bytes = source.sftp()?.copy_remote_path_to(
                        &source_path,
                        &destination.sftp()?,
                        &destination_path,
                        SftpTransferControl::default(),
                        SftpTransferOptions::default(),
                    )?;
                    return Ok(FileCopySummary {
                        bytes,
                        used_local_staging: false,
                    });
                }
                let staging = RemoteCopyStaging::new()?;
                let local = staging.path().join("payload");
                let downloaded = source.download_remote_path_with_progress_and_path_options(
                    &source_path,
                    &local,
                    SftpTransferControl::default(),
                    SftpPathTransferOptions::default(),
                    |_| {},
                )?;
                let uploaded = if downloaded.local_path.is_dir() {
                    destination.upload_path_with_progress_and_path_options(
                        &downloaded.local_path,
                        &destination_path.display_path,
                        SftpTransferControl::default(),
                        SftpPathTransferOptions::default(),
                        |_| {},
                    )?
                } else {
                    destination.upload_remote_file_with_progress_and_control_options(
                        &downloaded.local_path,
                        &destination_path,
                        SftpTransferControl::default(),
                        SftpTransferOptions::default(),
                        |_| {},
                    )?
                };
                Ok(FileCopySummary {
                    bytes: downloaded.bytes.min(uploaded.bytes),
                    used_local_staging: true,
                })
            }
        }
    }
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

fn copy_local_path(source: &Path, destination: &Path) -> anyhow::Result<u64> {
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        let mut bytes = 0;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            bytes += copy_local_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(bytes)
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(fs::copy(source, destination)?)
    }
}

fn download_shell_path<F>(
    service: &RemoteFileService,
    kind: RemoteFileBackendKind,
    remote_path: &str,
    local_path: &Path,
    control: &SftpTransferControl,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress),
{
    control.wait_if_paused_blocking()?;
    let properties = shell_properties(&service.shell(kind), &RemoteFilePath::new(remote_path))?;
    if properties.is_directory() {
        fs::create_dir_all(local_path)?;
        let mut bytes = 0;
        for entry in list_shell_dir(
            &service.shell(kind),
            &RemoteFilePath::new(remote_path),
            kind,
        )? {
            if entry.is_symlink() {
                continue;
            }
            bytes += download_shell_path(
                service,
                kind,
                &entry.path,
                &local_path.join(&entry.name),
                control,
                progress,
            )?;
        }
        Ok(bytes)
    } else {
        let bytes = service
            .shell(kind)
            .exec_ok(format!("cat -- {}", shell_quote(remote_path)), None)?;
        control.wait_if_paused_blocking()?;
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(local_path, &bytes)?;
        progress(SftpTransferProgress {
            remote_path: remote_path.to_string(),
            local_path: local_path.to_path_buf(),
            bytes_transferred: bytes.len() as u64,
            total_bytes: Some(bytes.len() as u64),
            item_count_completed: None,
            item_count_total: None,
        });
        Ok(bytes.len() as u64)
    }
}

fn upload_shell_path<F>(
    service: &RemoteFileService,
    kind: RemoteFileBackendKind,
    local_path: &Path,
    remote_path: &str,
    control: &SftpTransferControl,
    transfer_options: &SftpTransferOptions,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    F: FnMut(SftpTransferProgress),
{
    control.wait_if_paused_blocking()?;
    if local_path.is_dir() {
        service.create_dir_path(remote_path, None)?;
        let mut bytes = 0;
        for entry in fs::read_dir(local_path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            bytes += upload_shell_path(
                service,
                kind,
                &entry.path(),
                &remote_join(remote_path, &name),
                control,
                transfer_options,
                progress,
            )?;
        }
        Ok(bytes)
    } else {
        let summary = service.upload_file(local_path, remote_path)?;
        control.wait_if_paused_blocking()?;
        if let Some(mode) = transfer_options.default_file_mode {
            service.shell(kind).exec_ok(
                format!("chmod {mode:o} -- {}", shell_quote(remote_path)),
                None,
            )?;
        }
        progress(SftpTransferProgress {
            remote_path: remote_path.to_string(),
            local_path: local_path.to_path_buf(),
            bytes_transferred: summary.bytes,
            total_bytes: Some(summary.bytes),
            item_count_completed: None,
            item_count_total: None,
        });
        Ok(summary.bytes)
    }
}

fn resolve_shell_download_target(
    remote_path: &str,
    requested: &Path,
    policy: SftpDuplicatePolicy,
    resolver: Option<&dyn crate::SftpDuplicateResolver>,
) -> anyhow::Result<Option<PathBuf>> {
    if !requested.exists() {
        return Ok(Some(requested.to_path_buf()));
    }
    match duplicate_decision(
        policy,
        resolver,
        SftpTransferDirection::Download,
        remote_path,
        &requested.to_string_lossy(),
        requested.is_dir(),
    )? {
        SftpDuplicateDecision::Overwrite => Ok(Some(requested.to_path_buf())),
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => Ok(Some(unique_local_path(requested))),
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
        let candidate = path.with_file_name(format!(
            "{} ({index}){}",
            path.file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("file"),
            path.extension()
                .and_then(|value| value.to_str())
                .map_or(String::new(), |value| format!(".{value}"))
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
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
                format!("LC_ALL=C ls -la -- {}", shell_quote(&path.display_path)),
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
    for fields in fields.chunks_exact(8) {
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
            "LC_ALL=C stat -c '%F\\0%s\\0%Y\\0%X\\0%a\\0%U\\0%G\\0%u\\0%g' -- {}",
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
        format!("LC_ALL=C ls -ld -- {}", shell_quote(&path.display_path)),
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
            format!("chmod {recursive}{mode:o} -- {}", shell_quote(path)),
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
                "chown {recursive}{} -- {}",
                shell_quote(owner),
                shell_quote(path)
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
                "chgrp {recursive}{} -- {}",
                shell_quote(group),
                shell_quote(path)
            ),
            None,
        )?;
    }
    Ok(())
}

fn write_shell_text(
    shell: &ShellRemote,
    path: &RemoteFilePath,
    content: &str,
    expected_modified_at: Option<u64>,
    expected_size: Option<u64>,
    force: bool,
) -> anyhow::Result<SftpWriteTextResult> {
    let properties = shell_properties(shell, path)?;
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
    let mode = properties.permissions.unwrap_or(0o600) & 0o7777;
    let command = format!(
        "umask 077; cat > {} && test $(wc -c < {}) -eq {} && chmod {mode:o} -- {} && mv -f -- {} {}",
        shell_quote(&temporary),
        shell_quote(&temporary),
        content.len(),
        shell_quote(&temporary),
        shell_quote(&temporary),
        shell_quote(&path.display_path)
    );
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

    use crate::{RemoteFilePath, SftpFileType};

    use super::{
        BackendProbeStage, RemoteFileBackendKind, ensure_safe_delete_target, format_permissions,
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
    fn dangerous_delete_targets_are_rejected() {
        for path in ["", "/", ".", "..", "/tmp/../etc"] {
            assert!(ensure_safe_delete_target(path).is_err(), "{path}");
        }
        assert!(ensure_safe_delete_target("/tmp/file").is_ok());
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
