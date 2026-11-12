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
    }
}
fn exists(endpoint: &FileTransferEndpoint) -> anyhow::Result<bool> {
    match endpoint {
        FileTransferEndpoint::Adb { service, path } => service.exists(path),
        FileTransferEndpoint::Local(path) => match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        },
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
    let stages = if !matches!(request.source, FileTransferEndpoint::Local(_))
        && !matches!(request.destination, FileTransferEndpoint::Local(_))
    {
        2
    } else {
        1
    };
    let progress = CopyProgress::new(&request.source, &request.destination, stages, callback);
    let inventory = super::inventory::Inventory::scan_with_wait(
        &request.source,
        &request.control,
        super::inventory::MAX_ENTRIES,
        |_| wait_request(&request),
    )?;
    progress.set_inventory(inventory.bytes, inventory.len() as u64);
    let (bytes, copied, target) = node(&request, &progress, &inventory, 0)?;
    if copied {
        progress.finish(bytes);
    }
    Ok(FileCopySummary {
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
    inventory: &super::inventory::Inventory,
    depth: usize,
) -> anyhow::Result<(u64, bool, String)> {
    anyhow::ensure!(
        depth <= super::inventory::MAX_DEPTH,
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
        source.validate_copy_destination(&source_path, &target, &path, directory)?;
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
    if !owned && exists(&destination)? {
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
                let name = path
                    .display_path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("invalid destination name"))?;
                let original = name.to_string();
                let mut found = false;
                for number in 1..=10000 {
                    path = target.child_path(&parent, &format!("{original} ({number})"))?;
                    destination = target.transfer_endpoint(&path);
                    if !exists(&destination)? {
                        found = true;
                        break;
                    }
                }
                anyhow::ensure!(found, "no available copy destination");
            }
        }
    }
    if directory {
        if let Some(local) = &promised_local {
            request.options.download_runtime()?.ensure_directory(
                crate::download_path::root_for_target(local),
                local,
                true,
            )?;
        } else if !exists(&destination)? {
            target.create_dir_remote_path(&path, None)?;
        }
        anyhow::ensure!(
            target.remote_file_properties(&path)?.file_type == SftpFileType::Directory,
            "destination is not a directory"
        );
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
                    "copying an unrepresentable Android filename is unavailable"
                );
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
                .checked_add(node(&nested, progress, inventory, depth + 1)?.0)
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
                }
            } else {
                anyhow::bail!(super::AdbFilePolicyError::TimestampUnavailable);
            }
        }
        progress.finish_item();
        return Ok((total, true, path.display_path));
    }
    if owned
        && let (Some(local), FileTransferEndpoint::Adb { path, .. }) =
            (&promised_local, &request.source)
    {
        let raw = super::raw_path(path)?;
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
    let current = source.remote_file_properties(&source_path)?;
    anyhow::ensure!(
        current.file_type == properties.file_type
            && current.size == properties.size
            && current.modified_at == properties.modified_at,
        crate::file_browser::FileTransferEntryError::SourceChanged
    );
    let bytes = match (&request.source, &destination) {
        (FileTransferEndpoint::Local(local), FileTransferEndpoint::Adb { service, path }) => {
            service.upload_with_options(
                local,
                path,
                replace,
                &request.control,
                request.options.transfer_options(),
                |event| match event {
                    super::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                    super::AdbTransferEvent::Phase(phase) => progress.phase(phase),
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
                    super::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                    super::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                },
            )?
        }
        _ => {
            let temp = tempfile::tempdir()?;
            let local = temp.path().join("content");
            match &request.source {
                FileTransferEndpoint::Adb { service, path } => {
                    service.download_with_options(
                        path,
                        &local,
                        false,
                        &request.control,
                        request.options.transfer_options(),
                        |event| match event {
                            super::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                            super::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                        },
                    )?;
                }
                _ => {
                    FileCopyRequest {
                        source: request.source.clone(),
                        destination: FileTransferEndpoint::Local(local.clone()),
                        control: request.control.clone(),
                        options: request.options.clone(),
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
                        super::AdbTransferEvent::Data(bytes) => progress.add_bytes(bytes),
                        super::AdbTransferEvent::Phase(phase) => progress.phase(phase),
                    },
                )?,
                _ => {
                    FileCopyRequest {
                        source: FileTransferEndpoint::Local(local),
                        destination: destination.clone(),
                        control: request.control.clone(),
                        options: request.options.clone(),
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
                    })?
                    .bytes
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
