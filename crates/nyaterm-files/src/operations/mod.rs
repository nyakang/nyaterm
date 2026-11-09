//! UI-independent file operation execution and verified move policy.
//!
//! Callers resolve logical endpoints to services before submitting work. The
//! `same_endpoint` flag preserves rename eligibility without exposing application
//! session identifiers. Queue ownership, retries and presentation stay with callers.

use crate::file_browser::{
    FileBrowserBackendKind, FileBrowserService, file_browser_copy_destination_invalid,
    file_browser_name, valid_file_browser_child_name,
};
use crate::sftp::{RemoteFilePath, SftpFileEntry, SftpTransferControl};
use crate::sftp_transfer_types::{
    SftpPathTransferOptions, SftpTransferProgress, SftpTransferSummary,
};

mod verified_copy;
pub use verified_copy::{VerifiedCopyRequest, copy_verified};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTransferOperation {
    Copy,
    Move,
}

#[derive(Clone)]
pub struct FileTransferRequest {
    pub entry: SftpFileEntry,
    pub target_directory: RemoteFilePath,
    pub operation: FileTransferOperation,
    pub same_endpoint: bool,
}

#[derive(Clone)]
pub enum FileMutation {
    Delete(RemoteFilePath),
    CreateDirectory(String),
    CreateFile(String),
    Rename {
        source: RemoteFilePath,
        name: String,
    },
}

#[derive(Clone)]
pub enum FileOperationRequest {
    Transfer(FileTransferRequest),
    Mutation {
        directory: RemoteFilePath,
        mutation: FileMutation,
    },
}

pub struct FileOperationSummary {
    pub source_path: String,
    pub target_path: String,
    pub source_parent_path: Option<String>,
    pub source_entries: Option<Vec<SftpFileEntry>>,
    pub target_parent_path: String,
    pub bytes: u64,
    pub used_local_staging: bool,
    pub entries: Vec<SftpFileEntry>,
}

pub enum FileOperationResult {
    Completed(FileOperationSummary),
    Skipped(SftpTransferSummary),
    Applied {
        source_path: String,
        target_path: String,
    },
}

impl std::fmt::Debug for FileOperationResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed(summary) => formatter
                .debug_struct("Completed")
                .field("bytes", &summary.bytes)
                .field("used_local_staging", &summary.used_local_staging)
                .finish_non_exhaustive(),
            Self::Skipped(_) => formatter.write_str("Skipped { .. }"),
            Self::Applied { .. } => formatter.write_str("Applied { .. }"),
        }
    }
}

pub fn execute_file_operation(
    request: FileOperationRequest,
    source_service: FileBrowserService,
    target_service: FileBrowserService,
    control: SftpTransferControl,
    options: SftpPathTransferOptions,
    progress: impl FnMut(SftpTransferProgress) + Send + 'static,
) -> Result<FileOperationResult, String> {
    control
        .wait_if_paused_blocking()
        .map_err(|error| error.to_string())?;
    match request {
        FileOperationRequest::Transfer(intent) => {
            let entry = &intent.entry;
            let name = file_browser_name(source_service.kind(), &entry.path);
            if !valid_file_browser_child_name(target_service.kind(), &name) {
                return Err("invalid destination name".into());
            }
            let target_path = target_service
                .copy_child_path(
                    &intent.target_directory,
                    &source_service,
                    &entry.remote_path(),
                    &name,
                )
                .map_err(|error| error.to_string())?;
            let target = target_path.display_path.clone();
            if intent.same_endpoint
                && file_browser_copy_destination_invalid(
                    source_service.kind(),
                    &entry.path,
                    &target,
                    entry.is_directory(),
                )
            {
                return Err("source and destination overlap".into());
            }
            let source_properties = source_service
                .remote_file_properties(&entry.remote_path())
                .map_err(|error| error.to_string())?;
            if !matches!(
                source_properties.file_type,
                crate::sftp::SftpFileType::File | crate::sftp::SftpFileType::Directory
            ) {
                return Err("copy and move do not support symbolic links or special files".into());
            }
            if entry.is_symlink()
                || !matches!(
                    entry.file_type,
                    crate::sftp::SftpFileType::File | crate::sftp::SftpFileType::Directory
                )
            {
                return Err("copy and move do not support symbolic links or special files".into());
            }
            source_service
                .validate_copy_destination(
                    &entry.remote_path(),
                    &target_service,
                    &target_path,
                    source_properties.is_directory(),
                )
                .map_err(|error| error.to_string())?;
            let properties = target_service
                .remote_file_properties(&intent.target_directory)
                .map_err(|error| error.to_string())?;
            if !properties.is_directory() {
                return Err("destination is not a directory".into());
            }
            if properties.permissions.is_some_and(|mode| mode & 0o222 == 0) {
                return Err("destination is read-only".into());
            }
            let moving = intent.operation == FileTransferOperation::Move;
            if moving && intent.same_endpoint {
                if target_service
                    .list_dir_path(&intent.target_directory)
                    .map_err(|error| error.to_string())?
                    .iter()
                    .any(|e| {
                        crate::file_browser::file_browser_path_eq(
                            target_service.kind(),
                            &e.path,
                            &target,
                        )
                    })
                {
                    return Err("destination already exists; source was kept".into());
                }
                control
                    .check_cancelled()
                    .map_err(|error| error.to_string())?;
                match source_service
                    .rename_remote_paths_no_replace(&entry.remote_path(), &target_path)
                {
                    Ok(()) => return Ok(completed_output(entry.path.clone(), target)),
                    Err(error) => {
                        // SFTP v3 may report a generic Failure for EXDEV. Fall
                        // back only after confirming the source still exists and
                        // the destination remains absent; copy verifies before delete.
                        if source_service
                            .remote_file_properties(&entry.remote_path())
                            .is_err()
                        {
                            return Err(error.to_string());
                        }
                        reject_existing(&target_service, &intent.target_directory, &target)?;
                    }
                }
            }
            copy_verified(
                VerifiedCopyRequest {
                    source_service,
                    target_service,
                    source: entry.remote_path(),
                    target_path,
                    cut: moving,
                    control,
                    options,
                },
                progress,
            )
        }
        FileOperationRequest::Mutation {
            directory,
            mutation,
        } => {
            let parent = &directory.display_path;
            let result = match mutation {
                FileMutation::Delete(path) => {
                    if crate::file_browser::file_browser_path_is_root(
                        source_service.kind(),
                        &path.display_path,
                    ) {
                        return Err("cannot delete a filesystem root".into());
                    }
                    source_service.delete_remote_path(&path)
                }
                FileMutation::CreateDirectory(name) => {
                    validate_name(source_service.kind(), &name)?;
                    source_service.create_dir_remote_path(
                        &source_service
                            .child_path(&directory, &name)
                            .map_err(|error| error.to_string())?,
                        Some(0o755),
                    )
                }
                FileMutation::CreateFile(name) => {
                    validate_name(source_service.kind(), &name)?;
                    let target = source_service
                        .child_path(&directory, &name)
                        .map_err(|error| error.to_string())?;
                    reject_existing(&source_service, &directory, &target.display_path)?;
                    source_service.create_file_remote_path(&target, Some(0o644))
                }
                FileMutation::Rename { source, name } => {
                    validate_name(source_service.kind(), &name)?;
                    let target = source_service
                        .child_path(&directory, &name)
                        .map_err(|error| error.to_string())?;
                    reject_existing(&source_service, &directory, &target.display_path)?;
                    source_service.rename_remote_paths_no_replace(&source, &target)
                }
            };
            result.map_err(|error| error.to_string())?;
            Ok(completed_output(parent.clone(), parent.clone()))
        }
    }
}

fn completed_output(source_path: String, target_path: String) -> FileOperationResult {
    FileOperationResult::Applied {
        source_path,
        target_path,
    }
}

fn validate_name(kind: FileBrowserBackendKind, name: &str) -> Result<(), String> {
    if valid_file_browser_child_name(kind, name) {
        Ok(())
    } else {
        Err("invalid file name".into())
    }
}

fn reject_existing(
    service: &FileBrowserService,
    directory: &RemoteFilePath,
    target: &str,
) -> Result<(), String> {
    if service
        .list_dir_path(directory)
        .map_err(|error| error.to_string())?
        .iter()
        .any(|entry| crate::file_browser::file_browser_path_eq(service.kind(), &entry.path, target))
    {
        Err("destination already exists; source was kept".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
