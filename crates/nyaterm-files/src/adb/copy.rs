use crate::file_browser::{FileBrowserService, valid_file_browser_child_name};
use crate::remote_file::copy_progress::CopyProgress;
use crate::remote_file::{FileCopyRequest, FileCopySummary, FileTransferEndpoint};
use crate::sftp::{RemoteFilePath, SftpFileType};
use crate::sftp_transfer_types::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpTransferDirection,
    SftpTransferProgress,
};

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
    progress.scan(&request.source, &request.control)?;
    let (bytes, copied, target) = node(&request, &progress, 0)?;
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
    depth: usize,
) -> anyhow::Result<(u64, bool, String)> {
    anyhow::ensure!(depth < 128, "Android directory nesting exceeds limit");
    request.control.wait_if_paused_blocking()?;
    let (source, source_path) = browser(&request.source);
    let properties = source.remote_file_properties(&source_path)?;
    anyhow::ensure!(
        matches!(
            properties.file_type,
            SftpFileType::File | SftpFileType::Directory
        ),
        "copying Android symbolic links or special files is unavailable"
    );
    let directory = properties.is_directory();
    let mut destination = request.destination.clone();
    let (target, mut path) = browser(&destination);
    source.validate_copy_destination(&source_path, &target, &path, directory)?;
    let mut replace = false;
    if exists(&destination)? {
        let decision = match request.options.duplicate_policy() {
            SftpDuplicatePolicy::Overwrite => SftpDuplicateDecision::Overwrite,
            SftpDuplicatePolicy::Skip => SftpDuplicateDecision::Skip,
            SftpDuplicatePolicy::Rename => SftpDuplicateDecision::Rename,
            SftpDuplicatePolicy::Ask => request
                .options
                .duplicate_resolver()
                .ok_or_else(|| anyhow::anyhow!("file conflict requires a decision"))?
                .resolve_duplicate(&SftpDuplicateRequest {
                    direction: SftpTransferDirection::Copy,
                    source_path: source_path.display_path.clone(),
                    target_path: path.display_path.clone(),
                    is_directory: directory,
                })
                .map_err(anyhow::Error::msg)?,
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
        if !exists(&destination)? {
            target.create_dir_remote_path(&path, None)?;
        }
        anyhow::ensure!(
            target.remote_file_properties(&path)?.file_type == SftpFileType::Directory,
            "destination is not a directory"
        );
        let mut total = 0u64;
        for entry in source.list_dir_path(&source_path)? {
            request.control.check_cancelled()?;
            if matches!(entry.name.as_str(), "." | "..") {
                continue;
            }
            // Never use a lossy display name to construct a different write path.
            if let Some(raw) = entry.remote_path().raw_path()? {
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
                options: request.options.clone(),
            };
            total = total
                .checked_add(node(&nested, progress, depth + 1)?.0)
                .ok_or_else(|| anyhow::anyhow!("copy size overflow"))?;
        }
        progress.finish_item();
        return Ok((total, true, path.display_path));
    }
    let bytes = match (&request.source, &destination) {
        (FileTransferEndpoint::Local(local), FileTransferEndpoint::Adb { service, path }) => {
            service.upload(local, path, replace, &request.control, |n| {
                progress.add_bytes(n)
            })?
        }
        (FileTransferEndpoint::Adb { service, path }, FileTransferEndpoint::Local(local)) => {
            service.download(path, local, replace, &request.control, |n| {
                progress.add_bytes(n)
            })?
        }
        _ => {
            let temp = tempfile::tempdir()?;
            let local = temp.path().join("content");
            match &request.source {
                FileTransferEndpoint::Adb { service, path } => {
                    service.download(path, &local, false, &request.control, |n| {
                        progress.add_bytes(n)
                    })?;
                }
                _ => {
                    FileCopyRequest {
                        source: request.source.clone(),
                        destination: FileTransferEndpoint::Local(local.clone()),
                        control: request.control.clone(),
                        options: request.options.clone(),
                    }
                    .execute()?;
                }
            }
            match &destination {
                FileTransferEndpoint::Adb { service, path } => {
                    service.upload(&local, path, replace, &request.control, |n| {
                        progress.add_bytes(n)
                    })?
                }
                _ => {
                    FileCopyRequest {
                        source: FileTransferEndpoint::Local(local),
                        destination: destination.clone(),
                        control: request.control.clone(),
                        options: request.options.clone(),
                    }
                    .execute()?
                    .bytes
                }
            }
        }
    };
    progress.finish_item();
    Ok((bytes, true, path.display_path))
}
