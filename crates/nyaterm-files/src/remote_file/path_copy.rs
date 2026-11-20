use crate::file_browser::{FileBrowserService, valid_file_browser_child_name};
use crate::remote_file::copy_progress::CopyProgress;
use crate::remote_file::{FileCopyRequest, FileCopySummary, FileTransferEndpoint};
use crate::sftp::{RemoteFilePath, SftpFileType};
use crate::sftp_transfer_types::{
    SftpDuplicateCacheKey, SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest,
    SftpTransferDirection, SftpTransferProgress,
};
use std::path::PathBuf;

fn browser(endpoint: &FileTransferEndpoint) -> (FileBrowserService, RemoteFilePath) {
    match endpoint {
        FileTransferEndpoint::Local(path) => (
            FileBrowserService::local(),
            RemoteFilePath::new(path.to_string_lossy()),
        ),
        FileTransferEndpoint::Adb { service, path } => {
            (FileBrowserService::Adb(service.clone()), path.clone())
        }
        FileTransferEndpoint::Remote { service, path } => {
            (FileBrowserService::Remote(service.clone()), path.clone())
        }
        FileTransferEndpoint::Ftp { service, path } => {
            (FileBrowserService::Ftp(service.clone()), path.clone())
        }
    }
}
fn properties(
    endpoint: &FileTransferEndpoint,
    control: &crate::sftp::SftpTransferControl,
) -> anyhow::Result<crate::sftp::SftpFileProperties> {
    match endpoint {
        FileTransferEndpoint::Ftp { service, path } => {
            Ok(crate::file_browser::properties_from_ftp_entry(
                &service.stat_with_control(&path.display_path, control)?,
            ))
        }
        _ => {
            let (service, path) = browser(endpoint);
            service.remote_file_properties(&path)
        }
    }
}
fn exists(
    endpoint: &FileTransferEndpoint,
    control: &crate::sftp::SftpTransferControl,
) -> anyhow::Result<bool> {
    match endpoint {
        FileTransferEndpoint::Adb { service, path } => service.exists(path),
        FileTransferEndpoint::Local(path) => match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        },
        FileTransferEndpoint::Ftp { service, path } => Ok(service
            .list_dir_with_control(&path.parent()?.display_path, control)?
            .iter()
            .any(|entry| entry.path == path.display_path)),
        FileTransferEndpoint::Remote { .. } => {
            let (service, path) = browser(endpoint);
            Ok(service
                .list_dir_path(&path.parent()?)?
                .into_iter()
                .any(|entry| entry.remote_path() == path || entry.path == path.display_path))
        }
    }
}

fn wait_request(request: &FileCopyRequest) -> anyhow::Result<()> {
    loop {
        request.control.check_cancelled()?;
        for endpoint in [&request.source, &request.destination] {
            if let FileTransferEndpoint::Adb { service, .. } = endpoint {
                service.runtime.check()?;
                service.control.check_cancelled()?;
            }
        }
        for endpoint in [&request.source, &request.destination] {
            if let FileTransferEndpoint::Ftp { service, .. } = endpoint {
                service.ensure_available()?;
            }
        }
        if !request.control.is_paused() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub(crate) fn execute(
    request: FileCopyRequest,
    callback: impl FnMut(SftpTransferProgress) + Send + 'static,
) -> anyhow::Result<FileCopySummary> {
    wait_request(&request)?;
    let (source, source_path) = browser(&request.source);
    let (target, target_path) = browser(&request.destination);
    let directory = properties(&request.source, &request.control)?.is_directory();
    source.validate_copy_destination_with_control(
        &source_path,
        &target,
        &target_path,
        directory,
        &request.control,
    )?;
    let has_ftp = matches!(request.source, FileTransferEndpoint::Ftp { .. })
        || matches!(request.destination, FileTransferEndpoint::Ftp { .. });
    if has_ftp {
        let options = request.options.transfer_options();
        anyhow::ensure!(
            !options.resume_broken_transfer
                && !(matches!(request.destination, FileTransferEndpoint::Ftp { .. })
                    && options.default_file_mode.is_some())
                && !(options.preserve_timestamps
                    && matches!(request.destination, FileTransferEndpoint::Ftp { .. })),
            crate::ftp::FtpError::UnsupportedSettings
        );
    }
    let stages = if !matches!(request.source, FileTransferEndpoint::Local(_))
        && !matches!(request.destination, FileTransferEndpoint::Local(_))
    {
        2
    } else {
        1
    };
    let progress = CopyProgress::new(&request.source, &request.destination, stages, callback);
    let inventory = super::copy_inventory::Inventory::scan_with_wait(
        &request.source,
        &request.control,
        super::copy_inventory::MAX_ENTRIES,
        |_| wait_request(&request),
    )?;
    let ascii = [&request.source, &request.destination].iter().any(|endpoint| matches!(endpoint, FileTransferEndpoint::Ftp { service, .. } if service.ascii_mode()));
    progress.set_inventory(
        if ascii { None } else { inventory.bytes },
        inventory.len() as u64,
    );
    let local_root = match &request.destination {
        FileTransferEndpoint::Local(path) => {
            Some(crate::download_path::root_for_target(path).to_path_buf())
        }
        _ => None,
    };
    let mut retained_backups = Vec::new();
    let (bytes, copied, target) = node(
        &request,
        &progress,
        &inventory,
        0,
        local_root.as_deref(),
        &mut retained_backups,
    )?;
    if copied {
        progress.finish(bytes);
    }
    Ok(FileCopySummary {
        retained_backups,
        bytes,
        copied,
        destination_path: target,
        used_local_staging: !matches!(request.source, FileTransferEndpoint::Local(_))
            && !matches!(request.destination, FileTransferEndpoint::Local(_)),
    })
}

fn node(
    request: &FileCopyRequest,
    progress: &CopyProgress,
    inventory: &super::copy_inventory::Inventory,
    depth: usize,
    local_root: Option<&std::path::Path>,
    retained_backups: &mut Vec<String>,
) -> anyhow::Result<(u64, bool, String)> {
    anyhow::ensure!(
        depth <= super::copy_inventory::MAX_DEPTH,
        crate::file_browser::FileTransferEntryError::LimitExceeded
    );
    wait_request(request)?;
    let (source, source_path) = browser(&request.source);
    if let FileTransferEndpoint::Local(path) = &request.source {
        anyhow::ensure!(
            path.to_str().is_some(),
            crate::file_browser::FileTransferEntryError::InvalidDestination
        );
    }
    let properties = inventory.properties(&request.source)?;
    if depth == 0
        && let Some(expected) = request.options.expected_source_type()
    {
        anyhow::ensure!(
            properties.file_type == expected,
            crate::file_browser::FileTransferEntryError::SourceChanged
        );
    }
    anyhow::ensure!(
        matches!(
            properties.file_type,
            SftpFileType::File | SftpFileType::Directory
        ),
        crate::file_browser::FileTransferEntryError::UnsupportedFileType
    );
    let directory = properties.is_directory();
    let mut destination = request.destination.clone();
    let (target, mut path) = browser(&destination);
    let cache_key = SftpDuplicateCacheKey::Copy {
        source_kind: source.kind(),
        source_path: if source.kind() == crate::file_browser::FileBrowserBackendKind::Local {
            source_path.display_path.as_bytes().to_vec()
        } else {
            source_path
                .raw_path()?
                .unwrap_or_else(|| source_path.display_path.as_bytes().to_vec())
        },
        target_kind: target.kind(),
        target_path: if target.kind() == crate::file_browser::FileBrowserBackendKind::Local {
            path.display_path.as_bytes().to_vec()
        } else {
            path.raw_path()?
                .unwrap_or_else(|| path.display_path.as_bytes().to_vec())
        },
        is_directory: directory,
    };
    if depth == 0 {
        source.validate_copy_destination_with_control(
            &source_path,
            &target,
            &path,
            directory,
            &request.control,
        )?;
    }
    let mut replace = false;
    let promised_local = if request.options.is_promised_download() {
        match &destination {
            FileTransferEndpoint::Local(local) => Some(local.clone()),
            _ => None,
        }
    } else {
        None
    };
    let owned = if let Some(local) = &promised_local {
        request
            .options
            .download_runtime()?
            .validate_ancestors(local)?;
        request.options.download_runtime()?.owns(local, directory)?
    } else {
        false
    };
    if !owned && exists(&destination, &request.control)? {
        let decision = match request.options.duplicate_policy() {
            SftpDuplicatePolicy::Overwrite => SftpDuplicateDecision::Overwrite,
            SftpDuplicatePolicy::Skip => SftpDuplicateDecision::Skip,
            SftpDuplicatePolicy::Rename => SftpDuplicateDecision::Rename,
            SftpDuplicatePolicy::Ask => {
                if let Some(decision) = request
                    .options
                    .cached_duplicate_decision(&cache_key)
                    .map_err(anyhow::Error::msg)?
                {
                    decision
                } else {
                    let decision = request
                        .options
                        .duplicate_resolver()
                        .ok_or_else(|| anyhow::anyhow!("file conflict requires a decision"))?
                        .resolve_duplicate(&SftpDuplicateRequest {
                            direction: match (&request.source, &request.destination) {
                                (FileTransferEndpoint::Local(_), _) => {
                                    SftpTransferDirection::Upload
                                }
                                (_, FileTransferEndpoint::Local(_)) => {
                                    SftpTransferDirection::Download
                                }
                                _ => SftpTransferDirection::Copy,
                            },
                            source_path: source_path.display_path.clone(),
                            target_path: path.display_path.clone(),
                            is_directory: directory,
                        })
                        .map_err(anyhow::Error::msg)?;
                    request
                        .options
                        .remember_duplicate_decision(cache_key.clone(), decision)
                        .map_err(anyhow::Error::msg)?;
                    decision
                }
            }
        };
        match decision {
            SftpDuplicateDecision::Skip => return Ok((0, false, path.display_path)),
            SftpDuplicateDecision::Overwrite => replace = true,
            SftpDuplicateDecision::Rename => {
                let parent = target.parent_path(&path)?;
                let name = if target.kind() == crate::file_browser::FileBrowserBackendKind::Local {
                    std::path::Path::new(&path.display_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                } else {
                    path.display_path.trim_end_matches('/').rsplit('/').next()
                }
                .ok_or_else(|| anyhow::anyhow!("invalid destination name"))?;
                let original = name.to_string();
                let mut found = false;
                for number in 1..=10000 {
                    let renamed = match (!directory).then(|| original.rsplit_once('.')).flatten() {
                        Some((stem, extension)) if !stem.is_empty() => {
                            format!("{stem} ({number}).{extension}")
                        }
                        _ => format!("{original} ({number})"),
                    };
                    path = target.child_path(&parent, &renamed)?;
                    destination = target.transfer_endpoint(&path);
                    if !exists(&destination, &request.control)? {
                        found = true;
                        break;
                    }
                }
                anyhow::ensure!(found, "no available copy destination");
            }
        }
    }
    source.validate_copy_destination_with_control(
        &source_path,
        &target,
        &path,
        directory,
        &request.control,
    )?;
    if let (Some(root), FileTransferEndpoint::Local(local)) = (local_root, &destination) {
        crate::download_path::ensure_no_symlink(root, local)?;
        request
            .options
            .download_runtime()?
            .validate_ancestors(local)?;
    }
    if directory {
        if let Some(local) = &promised_local {
            request.options.download_runtime()?.ensure_directory(
                crate::download_path::root_for_target(local),
                local,
                true,
            )?;
        } else if !exists(&destination, &request.control)? {
            match &target {
                FileBrowserService::Ftp(service) => {
                    service.create_dir_with_control(&path.display_path, &request.control)?
                }
                _ => target.create_dir_remote_path(&path, None)?,
            }
        }
        anyhow::ensure!(
            self::properties(&destination, &request.control)?.file_type == SftpFileType::Directory,
            "destination is not a directory"
        );
        if let FileTransferEndpoint::Local(local) = &destination {
            request.options.download_runtime()?.track_directory(local)?;
        }
        let mut total = 0u64;
        let entries = inventory.children(&request.source)?;
        for endpoint in entries {
            let (_, child_source_path) = browser(&endpoint);
            let child_properties = inventory.properties(&endpoint)?;
            let entry = crate::sftp::SftpFileEntry {
                name: child_properties.name,
                path: child_source_path.display_path.clone(),
                raw_path_token: child_source_path.raw_path_token.clone(),
                file_type: child_properties.file_type,
                size: child_properties.size,
                permissions: child_properties.permissions,
                modified_at: child_properties.modified_at,
                owner: String::new(),
                group: String::new(),
                symlink_target_is_directory: false,
            };
            request.control.check_cancelled()?;
            if matches!(entry.name.as_str(), "." | "..") {
                continue;
            }
            // Never use a lossy display name to construct a different write path.
            if source.kind() != crate::file_browser::FileBrowserBackendKind::Local
                && let Some(raw) = entry.remote_path().raw_path()?
            {
                let name = raw.rsplit(|b| *b == b'/').next().unwrap_or_default();
                anyhow::ensure!(
                    std::str::from_utf8(name).is_ok(),
                    "copying an unrepresentable filename is unavailable"
                );
            }
            if target.kind() == crate::file_browser::FileBrowserBackendKind::Local {
                crate::download_path::validate_name(&entry.name)?;
            }
            anyhow::ensure!(
                valid_file_browser_child_name(target.kind(), &entry.name),
                "destination cannot represent filename"
            );
            let child = target.child_path(&path, &entry.name)?;
            let nested = FileCopyRequest {
                source: source.transfer_endpoint(&entry.remote_path()),
                destination: target.transfer_endpoint(&child),
                control: request.control.clone(),
                options: request
                    .options
                    .with_transfer_options(request.options.transfer_options().clone()),
            };
            total = total
                .checked_add(
                    node(
                        &nested,
                        progress,
                        inventory,
                        depth + 1,
                        local_root,
                        retained_backups,
                    )?
                    .0,
                )
                .ok_or_else(|| anyhow::anyhow!("copy size overflow"))?;
        }
        if request.options.transfer_options().preserve_timestamps {
            if let Some(modified) = properties.modified_at {
                match &target {
                    FileBrowserService::Adb(service) => service.set_modified(&path, modified)?,
                    FileBrowserService::Local(_) => {
                        std::fs::File::open(PathBuf::from(&path.display_path))?.set_modified(
                            std::time::UNIX_EPOCH
                                + std::time::Duration::from_secs(u64::from(modified)),
                        )?;
                    }
                    FileBrowserService::Remote(service) => {
                        service.set_modified_remote_path(&path, modified)?
                    }
                    FileBrowserService::Ftp(_) => {
                        anyhow::bail!(crate::ftp::FtpError::UnsupportedSettings)
                    }
                }
            } else {
                anyhow::bail!(crate::adb::AdbFilePolicyError::TimestampUnavailable);
            }
        }
        progress.finish_item();
        return Ok((total, true, path.display_path));
    }
    if owned
        && let (Some(local), FileTransferEndpoint::Adb { path, .. }) =
            (&promised_local, &request.source)
    {
        let raw = crate::adb::raw_path(path)?;
        anyhow::ensure!(
            request.options.download_runtime()?.reusable(
                local,
                &raw,
                properties.size,
                properties.modified_at
            )?,
            crate::file_browser::FileTransferEntryError::SourceChanged
        );
        let bytes = properties.size.unwrap_or(0);
        progress.finish_item();
        return Ok((bytes, true, local.to_string_lossy().into_owned()));
    }
    let current = self::properties(&request.source, &request.control)?;
    anyhow::ensure!(
        current.file_type == properties.file_type
            && current.size == properties.size
            && current.modified_at == properties.modified_at,
        crate::file_browser::FileTransferEntryError::SourceChanged
    );
    let bytes = match (&request.source, &destination) {
        (FileTransferEndpoint::Ftp { service, path }, FileTransferEndpoint::Local(local)) => {
            crate::ftp::copy::download(
                service,
                &path.display_path,
                local_root.unwrap_or_else(|| crate::download_path::root_for_target(local)),
                local,
                replace,
                &request.control,
                progress,
            )?
        }
        (FileTransferEndpoint::Local(local), FileTransferEndpoint::Ftp { service, path }) => {
            let outcome = crate::ftp::copy::upload(
                service,
                local,
                &path.display_path,
                replace,
                &request.control,
                progress,
            )?;
            retained_backups.extend(outcome.backup_path);
            outcome.bytes
        }
        (FileTransferEndpoint::Local(local), FileTransferEndpoint::Adb { service, path }) => {
            service.upload_with_options(
                local,
                path,
                replace,
                &request.control,
                request.options.transfer_options(),
                |event| match event {
                    crate::adb::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                    crate::adb::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                },
            )?
        }
        (FileTransferEndpoint::Adb { service, path }, FileTransferEndpoint::Local(local)) => {
            service.download_with_options(
                path,
                local,
                replace,
                &request.control,
                request.options.transfer_options(),
                |event| match event {
                    crate::adb::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                    crate::adb::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                },
            )?
        }
        _ => {
            let temp = PrivateStaging::new(request)?;
            let local = temp.workspace.local_path().to_path_buf();
            match &request.source {
                FileTransferEndpoint::Adb { service, path } => {
                    service.download_with_options(
                        path,
                        &local,
                        false,
                        &request.control,
                        request.options.transfer_options(),
                        |event| match event {
                            crate::adb::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                            crate::adb::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                        },
                    )?;
                }
                _ => {
                    FileCopyRequest {
                        source: request.source.clone(),
                        destination: FileTransferEndpoint::Local(local.clone()),
                        control: request.control.clone(),
                        options: request.options.for_directory_contents(),
                    }
                    .execute_with_progress({
                        let progress = progress.clone();
                        let mut reported = 0;
                        move |value| {
                            let delta = value.bytes_transferred.saturating_sub(reported);
                            reported = value.bytes_transferred;
                            if delta > 0 {
                                progress.add_bytes(delta);
                            }
                        }
                    })?;
                }
            }
            match &destination {
                FileTransferEndpoint::Adb { service, path } => service.upload_with_options(
                    &local,
                    path,
                    replace,
                    &request.control,
                    request.options.transfer_options(),
                    |event| match event {
                        crate::adb::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                        crate::adb::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                    },
                )?,
                _ => {
                    let summary = FileCopyRequest {
                        source: FileTransferEndpoint::Local(local),
                        destination: destination.clone(),
                        control: request.control.clone(),
                        options: request.options.for_resolved_copy(replace),
                    }
                    .execute_with_progress({
                        let progress = progress.clone();
                        let mut reported = 0;
                        move |value| {
                            let delta = value.bytes_transferred.saturating_sub(reported);
                            reported = value.bytes_transferred;
                            if delta > 0 {
                                progress.add_bytes(delta);
                            }
                        }
                    })?;
                    anyhow::ensure!(
                        summary.copied,
                        crate::file_browser::FileTransferEntryError::TargetExists
                    );
                    retained_backups.extend(summary.retained_backups);
                    summary.bytes
                }
            }
        }
    };
    if let Some(local) = &promised_local {
        request.options.download_runtime()?.completed(
            local,
            crate::download_path::FileIdentity::from_path(local)?,
            source_path
                .raw_path()?
                .unwrap_or_else(|| source_path.display_path.as_bytes().to_vec()),
            properties.size,
            properties.modified_at,
        )?;
    }
    progress.finish_item();
    Ok((bytes, true, path.display_path))
}

// The workspace holds the Windows directory handle through both legs. Drop that
// handle before removing the known staged file and its unique empty directory.
struct PrivateStaging {
    workspace: crate::external_edit::workspace::ExternalEditWorkspace,
    _cleanup: StagingCleanup,
}
struct StagingCleanup(std::path::PathBuf);
impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
impl PrivateStaging {
    fn new(request: &FileCopyRequest) -> anyhow::Result<Self> {
        let workspace = crate::external_edit::workspace::prepare_external_edit_workspace(
            "content",
            &std::env::temp_dir(),
            None::<fn() -> Option<std::path::PathBuf>>,
            || request.control.is_cancelled(),
            |path| request.options.secure_staging_directory(path),
        )?;
        let cleanup = StagingCleanup(workspace.local_path().to_path_buf());
        Ok(Self {
            workspace,
            _cleanup: cleanup,
        })
    }
}
