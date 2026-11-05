//! Frozen file operations, shared by the workspace and the transfer queue.

use super::clipboard_copy::{ClipboardCopyRequest, run_clipboard_copy};
use super::transfer_jobs::TransferProgressEventSender;
use crate::features::NyaTermApp;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};
use gpui::Context;
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, FileBrowserService, file_browser_copy_destination_invalid,
    file_browser_join, file_browser_name, file_browser_parent, valid_file_browser_child_name,
};
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
};
use rust_i18n::t;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::features) enum FileOperationEndpoint {
    Local,
    Session(String),
    /// A saved source with no live session. It must never fall back to local I/O.
    OfflineConnection(String),
}

impl FileOperationEndpoint {
    pub(in crate::features) fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session(id) => Some(id),
            _ => None,
        }
    }

    pub(in crate::features) fn kind(&self) -> FileBrowserBackendKind {
        match self {
            Self::Local => FileBrowserBackendKind::Local,
            _ => FileBrowserBackendKind::Remote,
        }
    }

    pub(in crate::features) fn replace_session(&mut self, old: &str, new: &str) {
        if self.session_id() == Some(old) {
            *self = Self::Session(new.into());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum FileTransferOperation {
    Copy,
    Move,
}

#[derive(Clone)]
pub(in crate::features) struct FileTransferIntent {
    pub source: FileOperationEndpoint,
    pub entries: Vec<SftpFileEntry>,
    pub target: FileOperationEndpoint,
    pub target_directory: RemoteFilePath,
    pub operation: FileTransferOperation,
}

#[derive(Clone)]
pub(in crate::features) enum FileMutation {
    Delete(RemoteFilePath),
    CreateDirectory(String),
    CreateFile(String),
    Rename {
        source: RemoteFilePath,
        name: String,
    },
}

#[derive(Clone)]
pub(in crate::features) struct FileMutationIntent {
    pub endpoint: FileOperationEndpoint,
    pub directory: RemoteFilePath,
    pub mutation: FileMutation,
}

#[derive(Clone)]
pub(in crate::features) enum FileOperationRequest {
    Transfer(FileTransferIntent),
    Mutation(FileMutationIntent),
}

#[derive(Clone)]
pub(in crate::features) struct FileOperationContext {
    pub request: FileOperationRequest,
    pub source_label: String,
    pub target_label: String,
}

impl FileOperationContext {
    pub(in crate::features) fn endpoints(
        &self,
    ) -> (&FileOperationEndpoint, &FileOperationEndpoint) {
        match &self.request {
            FileOperationRequest::Transfer(intent) => (&intent.source, &intent.target),
            FileOperationRequest::Mutation(intent) => (&intent.endpoint, &intent.endpoint),
        }
    }

    pub(in crate::features) fn replace_session(&mut self, old: &str, new: &str) {
        match &mut self.request {
            FileOperationRequest::Transfer(intent) => {
                intent.source.replace_session(old, new);
                intent.target.replace_session(old, new);
            }
            FileOperationRequest::Mutation(intent) => intent.endpoint.replace_session(old, new),
        }
    }

    pub(in crate::features) fn action_key(&self) -> &'static str {
        match &self.request {
            FileOperationRequest::Transfer(intent) => match intent.operation {
                FileTransferOperation::Copy => "fileManager.copy",
                FileTransferOperation::Move => "fileManager.move",
            },
            FileOperationRequest::Mutation(intent) => match &intent.mutation {
                FileMutation::Delete(_) => "fileExplorer.delete",
                FileMutation::CreateDirectory(_) => "fileExplorer.newFolder",
                FileMutation::CreateFile(_) => "fileExplorer.newFile",
                FileMutation::Rename { .. } => "fileExplorer.cmRename",
            },
        }
    }

    pub(in crate::features) fn can_pause(&self) -> bool {
        matches!(self.request, FileOperationRequest::Transfer(_))
    }

    pub(in crate::features) fn is_delete(&self) -> bool {
        matches!(
            &self.request,
            FileOperationRequest::Mutation(FileMutationIntent {
                mutation: FileMutation::Delete(_),
                ..
            })
        )
    }
}

impl NyaTermApp {
    pub(in crate::features) fn submit_file_transfer(
        &mut self,
        _origin: String,
        intent: FileTransferIntent,
        cx: &mut Context<Self>,
    ) {
        // Each queue item retains its own source. A failed item can be retried
        // without replaying siblings that already completed (especially moves).
        let FileTransferIntent {
            source,
            entries,
            target,
            target_directory,
            operation,
        } = intent;
        for entry in entries {
            let item = FileTransferIntent {
                source: source.clone(),
                entries: vec![entry],
                target: target.clone(),
                target_directory: target_directory.clone(),
                operation,
            };
            let context = FileOperationContext {
                source_label: self.endpoint_label(&item.source),
                target_label: self.endpoint_label(&item.target),
                request: FileOperationRequest::Transfer(item),
            };
            self.enqueue_file_operation(context, None, cx);
        }
    }

    pub(in crate::features) fn submit_file_mutation(
        &mut self,
        _origin: String,
        intent: FileMutationIntent,
        cx: &mut Context<Self>,
    ) {
        let label = self.endpoint_label(&intent.endpoint);
        self.enqueue_file_operation(
            FileOperationContext {
                source_label: label.clone(),
                target_label: label,
                request: FileOperationRequest::Mutation(intent),
            },
            None,
            cx,
        );
    }

    fn enqueue_file_operation(
        &mut self,
        context: FileOperationContext,
        retry: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (source, target) = context.endpoints();
        let services = self
            .file_browser_service_for_endpoint(source)
            .and_then(|source_service| {
                self.file_browser_service_for_endpoint(target)
                    .map(|target_service| (source_service, target_service))
            });
        let (source_service, target_service) = match services {
            Ok(services) => services,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let (source_path, target_path, parent, name) = match &context.request {
            FileOperationRequest::Transfer(intent) => {
                let Some(entry) = intent.entries.first() else {
                    return;
                };
                let name = file_browser_name(source_service.kind(), &entry.path);
                let target = file_browser_join(
                    target_service.kind(),
                    &intent.target_directory.display_path,
                    &name,
                );
                (
                    entry.path.clone(),
                    target,
                    intent.target_directory.display_path.clone(),
                    entry.name.clone(),
                )
            }
            FileOperationRequest::Mutation(intent) => {
                let (path, name) = match &intent.mutation {
                    FileMutation::Delete(path) => (
                        path.display_path.clone(),
                        t!("fileExplorer.delete").to_string(),
                    ),
                    FileMutation::CreateDirectory(name) | FileMutation::CreateFile(name) => (
                        file_browser_join(
                            source_service.kind(),
                            &intent.directory.display_path,
                            name,
                        ),
                        name.clone(),
                    ),
                    FileMutation::Rename { source, name } => {
                        (source.display_path.clone(), name.clone())
                    }
                };
                (
                    path.clone(),
                    path,
                    intent.directory.display_path.clone(),
                    name,
                )
            }
        };
        let id = retry.unwrap_or_else(|| self.transfer.next_transfer_job_id("file-operation"));
        let control = SftpTransferControl::new();
        let kind = TransferJobKind::SendTo {
            source_path,
            target_session_id: target.session_id().unwrap_or("local").into(),
            target_path,
            target_parent_path: parent,
        };
        if let Some(job) = self.transfer.transfer_job_mut(&id) {
            job.kind = kind;
            job.status = TransferJobStatus::Running;
            job.progress = None;
            job.summary = None;
            job.speed.reset();
            job.control = Some(control.clone());
            job.detail = t!("fileTransfer.transferring").to_string();
        } else {
            self.transfer.enqueue_transfer_job(TransferJobState {
                id: id.clone(),
                session_id: source.session_id().map(str::to_string),
                kind,
                status: TransferJobStatus::Running,
                detail: t!("fileTransfer.transferring").to_string(),
                created_at_ms: TransferJobState::now_ms(),
                display_name: name,
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: Some(control.clone()),
                speed: Default::default(),
            });
        }
        self.transfer
            .track_file_operation(id.clone(), context.clone());
        let moving = matches!(&context.request, FileOperationRequest::Transfer(i) if i.operation == FileTransferOperation::Move);
        let policy = if moving {
            SftpDuplicatePolicy::Skip
        } else {
            self.transfer.duplicate_policy()
        };
        let resolver = (policy == SftpDuplicatePolicy::Ask).then(|| {
            self.session.prompt_duplicate_broker() as std::sync::Arc<dyn SftpDuplicateResolver>
        });
        let options = SftpPathTransferOptions::new(policy, resolver, self.sftp_transfer_options());
        let tx = self.transfer.transfer_event_sender();
        self.submit_bulk_transfer_job("file-operation", id.clone(), tx.clone(), move || {
            let mut progress = TransferProgressEventSender::new(id.clone(), tx.clone());
            let result = execute_file_operation(
                context.request,
                source_service,
                target_service,
                control,
                options,
                move |value| progress.send(value),
            );
            let _ = tx.unbounded_send(TransferJobResult {
                id,
                event: TransferJobEvent::Finished(result),
            });
        });
        cx.notify();
    }

    pub(in crate::features) fn retry_file_operation(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(context) = self.transfer.file_operation(id).cloned() else {
            return false;
        };
        if self.transfer.transfer_job(id).is_some_and(|job| {
            matches!(
                job.status,
                TransferJobStatus::Failed | TransferJobStatus::Cancelled
            )
        }) {
            self.enqueue_file_operation(context, Some(id.into()), cx);
        }
        true
    }

    /// File-operation completion does not borrow the active sidebar's context.
    /// Directory reload failures are independent from the successful operation.
    pub(in crate::features) fn finish_file_operation(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(context) = self.transfer.file_operation(id).cloned() else {
            return;
        };
        let mut directories = Vec::new();
        match context.request {
            FileOperationRequest::Transfer(intent) => {
                if let Some(entry) = intent.entries.first() {
                    let parent = if intent.source.kind() == FileBrowserBackendKind::Remote {
                        entry.remote_path().parent().ok()
                    } else {
                        Some(RemoteFilePath::new(file_browser_parent(
                            intent.source.kind(),
                            &entry.path,
                        )))
                    };
                    if let Some(parent) = parent {
                        directories.push((intent.source.clone(), parent));
                    }
                }
                directories.push((intent.target, intent.target_directory));
            }
            FileOperationRequest::Mutation(intent) => {
                directories.push((intent.endpoint, intent.directory))
            }
        }
        for (endpoint, path) in directories {
            self.refresh_file_manager_directory(&endpoint, &path, cx);
        }
        self.release_unreferenced_hidden_sessions(cx);
    }
}

fn execute_file_operation(
    request: FileOperationRequest,
    source_service: FileBrowserService,
    target_service: FileBrowserService,
    control: SftpTransferControl,
    options: SftpPathTransferOptions,
    progress: impl FnMut(nyaterm_files::sftp_transfer_types::SftpTransferProgress) + Send + 'static,
) -> Result<TransferJobOutput, String> {
    control
        .wait_if_paused_blocking()
        .map_err(|error| error.to_string())?;
    match request {
        FileOperationRequest::Transfer(intent) => {
            let entry = intent.entries.first().ok_or("no source selected")?;
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
            if intent.source == intent.target
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
                nyaterm_files::sftp::SftpFileType::File
                    | nyaterm_files::sftp::SftpFileType::Directory
            ) {
                return Err("copy and move do not support symbolic links or special files".into());
            }
            if entry.is_symlink()
                || !matches!(
                    entry.file_type,
                    nyaterm_files::sftp::SftpFileType::File
                        | nyaterm_files::sftp::SftpFileType::Directory
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
            if moving && intent.source == intent.target {
                if target_service
                    .list_dir_path(&intent.target_directory)
                    .map_err(|error| error.to_string())?
                    .iter()
                    .any(|e| {
                        nyaterm_files::file_browser::file_browser_path_eq(
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
            run_clipboard_copy(
                ClipboardCopyRequest {
                    source_service,
                    target_service,
                    source: entry.remote_path(),
                    target_path,
                    target_session_id: intent.target.session_id().unwrap_or("local").into(),
                    cut: moving,
                    control,
                    options,
                },
                progress,
            )
        }
        FileOperationRequest::Mutation(intent) => {
            let parent = &intent.directory.display_path;
            let result = match intent.mutation {
                FileMutation::Delete(path) => {
                    if nyaterm_files::file_browser::file_browser_path_is_root(
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
                            .child_path(&intent.directory, &name)
                            .map_err(|error| error.to_string())?,
                        Some(0o755),
                    )
                }
                FileMutation::CreateFile(name) => {
                    validate_name(source_service.kind(), &name)?;
                    let target = source_service
                        .child_path(&intent.directory, &name)
                        .map_err(|error| error.to_string())?;
                    reject_existing(&source_service, &intent.directory, &target.display_path)?;
                    source_service.create_file_remote_path(&target, Some(0o644))
                }
                FileMutation::Rename { source, name } => {
                    validate_name(source_service.kind(), &name)?;
                    let target = source_service
                        .child_path(&intent.directory, &name)
                        .map_err(|error| error.to_string())?;
                    reject_existing(&source_service, &intent.directory, &target.display_path)?;
                    source_service.rename_remote_paths_no_replace(&source, &target)
                }
            };
            result.map_err(|error| error.to_string())?;
            Ok(completed_output(parent.clone(), parent.clone()))
        }
    }
}

fn completed_output(source_path: String, target_path: String) -> TransferJobOutput {
    TransferJobOutput::Sent {
        source_path,
        target_path,
        source_parent_path: None,
        source_entries: None,
        target_session_id: String::new(),
        target_parent_path: String::new(),
        bytes: 0,
        used_local_staging: false,
        entries: Vec::new(),
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
        .any(|entry| {
            nyaterm_files::file_browser::file_browser_path_eq(service.kind(), &entry.path, target)
        })
    {
        Err("destination already exists; source was kept".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
