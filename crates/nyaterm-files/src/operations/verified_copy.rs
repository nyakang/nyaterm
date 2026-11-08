//! Verified copy/delete execution shared by file-manager and clipboard jobs.
use super::{FileOperationResult, FileOperationSummary};
use crate::file_browser::{FileBrowserService, file_browser_name, file_browser_path_is_root};
use crate::remote_file::FileCopyRequest;
use crate::sftp::{RemoteFilePath, SftpFileType, SftpTransferControl};
use crate::sftp_transfer_types::SftpPathTransferOptions;
use std::collections::BTreeMap;
type FileInventory = BTreeMap<String, (SftpFileType, Option<u64>, Option<u32>)>;
fn inventory(
    service: &FileBrowserService,
    path: &RemoteFilePath,
    control: &SftpTransferControl,
) -> Result<FileInventory, String> {
    let mut found = BTreeMap::new();
    let mut pending = vec![(String::new(), path.clone())];
    while let Some((relative, path)) = pending.pop() {
        control
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        let props = service
            .remote_file_properties(&path)
            .map_err(|error| error.to_string())?;
        if !matches!(
            props.file_type,
            SftpFileType::File | SftpFileType::Directory
        ) {
            return Err("cut does not support symbolic links or special files".into());
        }
        if found
            .insert(
                relative.clone(),
                (props.file_type, props.size, props.modified_at),
            )
            .is_some()
        {
            return Err("cannot verify ambiguous display filenames; source was kept".into());
        }
        if props.file_type == SftpFileType::Directory {
            for entry in service
                .list_dir_path(&path)
                .map_err(|error| error.to_string())?
            {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                pending.push((format!("{relative}/{}", entry.name), entry.remote_path()));
            }
        }
    }
    Ok(found)
}

fn check_no_symlink_ancestors(
    service: &FileBrowserService,
    path: &RemoteFilePath,
) -> Result<(), String> {
    let mut current = path.clone();
    loop {
        let props = service
            .remote_file_properties(&current)
            .map_err(|error| error.to_string())?;
        if props.file_type == SftpFileType::Symlink {
            return Err("cut through a symbolic link is not supported".into());
        }
        if file_browser_path_is_root(service.kind(), &current.display_path) {
            break;
        }
        let parent = service
            .parent_path(&current)
            .map_err(|error| error.to_string())?;
        if parent == current {
            break;
        }
        current = parent;
    }
    Ok(())
}

fn copied_tree_matches(before: &FileInventory, after: &FileInventory) -> bool {
    before.len() == after.len()
        && before.iter().all(|(path, (kind, size, _))| {
            after.get(path).is_some_and(|(other_kind, other_size, _)| {
                kind == other_kind && (kind == &SftpFileType::Directory || size == other_size)
            })
        })
}

pub struct VerifiedCopyRequest {
    pub source_service: FileBrowserService,
    pub target_service: FileBrowserService,
    pub source: RemoteFilePath,
    pub target_path: RemoteFilePath,
    pub cut: bool,
    pub control: SftpTransferControl,
    pub options: SftpPathTransferOptions,
}

pub fn copy_verified(
    request: VerifiedCopyRequest,
    progress: impl FnMut(crate::sftp_transfer_types::SftpTransferProgress) + Send + 'static,
) -> Result<FileOperationResult, String> {
    let VerifiedCopyRequest {
        source_service,
        target_service,
        source,
        target_path,
        cut,
        control,
        options,
    } = request;
    let before = if cut {
        let parent = source_service
            .parent_path(&source)
            .map_err(|error| error.to_string())?;
        check_no_symlink_ancestors(&source_service, &parent)?;
        let target_parent = target_service
            .parent_path(&target_path)
            .map_err(|error| error.to_string())?;
        check_no_symlink_ancestors(&target_service, &target_parent)?;
        let target_name = file_browser_name(target_service.kind(), &target_path.display_path);
        if target_service
            .list_dir_path(&target_parent)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|entry| entry.name == target_name)
        {
            return Err("destination already exists; source was kept".into());
        }
        Some(inventory(&source_service, &source, &control)?)
    } else {
        None
    };
    let summary = FileCopyRequest {
        source: source_service.transfer_endpoint(&source),
        destination: target_service.transfer_endpoint(&target_path),
        control: control.clone(),
        options,
    }
    .execute_with_progress(progress)
    .map_err(|error| error.to_string())?;
    if !summary.copied && cut {
        return Err("copy skipped; source was kept".into());
    }
    if !summary.copied {
        return Ok(FileOperationResult::Skipped(
            crate::sftp_transfer_types::SftpTransferSummary {
                remote_path: summary.destination_path,
                local_path: Default::default(),
                bytes: 0,
                skipped: true,
            },
        ));
    }
    let mut source_parent_path = None;
    let mut source_entries = None;
    if let Some(before) = before {
        control
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        let after_source = inventory(&source_service, &source, &control)?;
        if after_source != before {
            return Err("source changed during copy; source was kept".into());
        }
        let destination = target_path.clone();
        let after_target = inventory(&target_service, &destination, &control)?;
        if !copied_tree_matches(&before, &after_target) {
            return Err("copy verification failed; source was kept".into());
        }
        control
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        if inventory(&source_service, &source, &control)? != before {
            return Err("source changed during verification; source was kept".into());
        }
        source_service
            .delete_remote_path(&source)
            .map_err(|error| error.to_string())?;
        let parent = source_service
            .parent_path(&source)
            .map_err(|error| error.to_string())?;
        source_entries = source_service.list_dir_path(&parent).ok();
        source_parent_path = Some(parent.display_path);
    }
    let target_parent = target_service
        .parent_path(&target_path)
        .map_err(|error| error.to_string())?;
    let target_parent_path = target_parent.display_path.clone();
    let entries = target_service
        .list_dir_path(&target_parent)
        .unwrap_or_default();
    Ok(FileOperationResult::Completed(FileOperationSummary {
        source_path: source.display_path,
        source_parent_path,
        source_entries,
        target_path: summary.destination_path,
        target_parent_path,
        bytes: summary.bytes,
        used_local_staging: summary.used_local_staging,
        entries,
    }))
}

#[cfg(test)]
mod tests;
