use super::super::context_menu_policy::{
    SendToCandidate, send_to_candidate_is_eligible, send_to_target_directory,
};
use crate::features::NyaTermApp;
use crate::features::transfers::transfer_jobs::TransferProgressEventSender;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};
use gpui::{Context, Window};
use nyaterm_files::file_browser::{
    FileBrowserService, file_browser_join, file_browser_name, file_browser_parent,
};
use nyaterm_files::remote_file::FileCopyRequest;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
};
use rust_i18n::t;

/// A session the current selection can be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features::pages::transfers) struct TransferSendToTarget {
    pub(in crate::features::pages::transfers) session_id: String,
    /// Display name shown as the submenu entry label.
    pub(in crate::features::pages::transfers) label: String,
}

impl NyaTermApp {
    /// Eligible "Send to" targets for the active browser session.
    ///
    /// A target is any *other* connected local or SSH session that exposes a file
    /// browser backend. The source session is always excluded. Ordered like the
    /// session tabs so the menu is stable.
    pub(in crate::features::pages::transfers) fn transfer_send_to_targets(
        &self,
    ) -> Vec<TransferSendToTarget> {
        let Some(source_session_id) = self.session.active_id() else {
            return Vec::new();
        };
        self.session
            .ordered_sessions()
            .into_iter()
            .filter_map(|session| {
                let has_browser_backend = self
                    .session
                    .file_browser_backend_for_session(&session.id)
                    .is_some();
                let candidate = SendToCandidate {
                    session_id: session.id.clone(),
                    has_browser_backend,
                    is_disconnected: self.session.is_disconnected(&session.id),
                };
                send_to_candidate_is_eligible(source_session_id, &candidate).then(|| {
                    TransferSendToTarget {
                        label: self.session.display_name_by_info(&session),
                        session_id: session.id,
                    }
                })
            })
            .collect()
    }

    /// Resolve the target session's destination directory *synchronously* from its
    /// browser cache when present. `None` means the background job must resolve the
    /// target session's home/cwd instead — the source session's path is never used.
    fn transfer_send_to_cached_target_dir(&self, target_session_id: &str) -> Option<String> {
        let cache = self.transfer.browser_session_cache(target_session_id)?;
        let dir = send_to_target_directory(Some(cache.current_path.as_str()), None);
        (dir != ".").then_some(dir)
    }

    /// Copy the current selection set to another connected local or SSH session.
    pub(in crate::features::pages::transfers) fn start_send_selected_transfers_to_session(
        &mut self,
        target_session_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selected_transfer_entries();
        if entries.is_empty() {
            self.shell
                .set_status(t!("fileTransfer.statusMarkBeforeSending").to_string());
            cx.notify();
            return;
        }

        let source_service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };

        let target_service = match self.file_browser_service_for_session(&target_session_id) {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };

        let cached_target_dir = self.transfer_send_to_cached_target_dir(&target_session_id);
        let source_session_id = self.session.active_id_owned();
        let total = entries.len();
        let duplicate_policy = self.transfer.duplicate_policy();
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask).then(|| {
            self.session.prompt_duplicate_broker() as std::sync::Arc<dyn SftpDuplicateResolver>
        });
        let options = SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        );

        let context = SendToJobContext {
            source_session_id,
            source_service,
            target_session_id,
            target_service,
            cached_target_dir,
            options,
        };
        for entry in entries {
            self.enqueue_send_to_job(&context, entry, cx);
        }
        self.shell
            .set_status(t!("fileTransfer.statusSendJobsStarted", count = total).to_string());
        cx.notify();
    }

    fn enqueue_send_to_job(
        &mut self,
        context: &SendToJobContext,
        entry: SftpFileEntry,
        cx: &mut Context<Self>,
    ) {
        let id = self.transfer.next_transfer_job_id("sftp-send");
        let entry_name = file_browser_name(context.source_service.kind(), &entry.path);
        let source_path = entry.path.clone();
        // Best-effort provisional target for the visible row; the worker resolves
        // the real directory (cache or the target session's home) before copying.
        let provisional_dir = context
            .cached_target_dir
            .clone()
            .unwrap_or_else(|| "~".to_string());
        let provisional_target =
            file_browser_join(context.target_service.kind(), &provisional_dir, &entry_name);
        let provisional_parent =
            file_browser_parent(context.target_service.kind(), &provisional_target);
        let control = SftpTransferControl::new();

        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            // Scope the row to the *source* session so it shows in that session's
            // queue; the target session is recorded in the job kind.
            session_id: context.source_session_id.clone(),
            kind: TransferJobKind::SendTo {
                source_path: source_path.clone(),
                target_session_id: context.target_session_id.clone(),
                target_path: provisional_target.clone(),
                target_parent_path: provisional_parent,
            },
            status: TransferJobStatus::Running,
            detail: t!("fileTransfer.detailSending", path = source_path.clone()).to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: Some(control.clone()),
            speed: Default::default(),
        });

        let source_remote = entry.remote_path();
        let source_service = context.source_service.clone();
        let target_service = context.target_service.clone();
        let target_session_id = context.target_session_id.clone();
        let cached_target_dir = context.cached_target_dir.clone();
        let options = context.options.clone();
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_bulk_transfer_job("sftp-send-to", id.clone(), transfer_tx.clone(), move || {
            let mut progress = TransferProgressEventSender::new(id.clone(), transfer_tx.clone());
            let result = run_send_to(
                SendToRequest {
                    source_service,
                    source_remote,
                    target_service,
                    cached_target_dir,
                    entry_name,
                    target_session_id,
                    control,
                    options,
                },
                move |value| progress.send(value),
            );
            let _ = transfer_tx.unbounded_send(TransferJobResult {
                id,
                event: TransferJobEvent::Finished(result),
            });
        });
        cx.notify();
    }
}

/// Shared inputs for every per-entry "Send to" job in one send action.
struct SendToJobContext {
    source_session_id: Option<String>,
    source_service: FileBrowserService,
    target_session_id: String,
    target_service: FileBrowserService,
    cached_target_dir: Option<String>,
    options: SftpPathTransferOptions,
}

struct SendToRequest {
    source_service: FileBrowserService,
    source_remote: RemoteFilePath,
    target_service: FileBrowserService,
    cached_target_dir: Option<String>,
    entry_name: String,
    target_session_id: String,
    control: SftpTransferControl,
    options: SftpPathTransferOptions,
}

/// Copy one entry to the target session and list the destination directory.
///
/// Backend selection is left to `FileCopyRequest`: SFTP-to-SFTP copies directly,
/// mixed backends stage through a local temp directory.
fn run_send_to(
    request: SendToRequest,
    progress: impl FnMut(nyaterm_files::sftp_transfer_types::SftpTransferProgress) + Send + 'static,
) -> Result<TransferJobOutput, String> {
    let SendToRequest {
        source_service,
        source_remote,
        target_service,
        cached_target_dir,
        entry_name,
        target_session_id,
        control,
        options,
    } = request;
    // Resolve the destination directory from the target session only: the cache
    // when present, otherwise the target session's home. Never the source path.
    let target_dir = match cached_target_dir {
        Some(dir) => dir,
        None => {
            let home = target_service.home_dir().map_err(|error| {
                t!(
                    "fileTransfer.errorResolveTargetHome",
                    error = error.to_string()
                )
                .to_string()
            })?;
            send_to_target_directory(None, Some(home.as_str()))
        }
    };
    let requested_target_path = file_browser_join(target_service.kind(), &target_dir, &entry_name);

    let request = FileCopyRequest {
        source: source_service.transfer_endpoint(&source_remote),
        destination: target_service
            .transfer_endpoint(&RemoteFilePath::new(requested_target_path.clone())),
        control,
        options,
    };
    let summary = request
        .execute_with_progress(progress)
        .map_err(|error| t!("fileTransfer.errorSend", error = error.to_string()).to_string())?;
    let target_path = summary.destination_path;
    let target_parent_path = file_browser_parent(target_service.kind(), &target_path);

    // A best-effort refresh of the destination listing; a failure here does not
    // fail the copy, it just leaves the target cache untouched.
    let entries = target_service
        .list_dir(&target_parent_path)
        .unwrap_or_default();

    Ok(TransferJobOutput::Sent {
        source_path: source_remote.display_path,
        source_parent_path: None,
        source_entries: None,
        target_session_id,
        target_path,
        target_parent_path,
        bytes: summary.bytes,
        used_local_staging: summary.used_local_staging,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::{SendToRequest, run_send_to};
    use crate::models::TransferJobOutput;
    use nyaterm_files::file_browser::FileBrowserService;
    use nyaterm_files::sftp::{RemoteFilePath, SftpTransferControl};
    use nyaterm_files::sftp_transfer_types::SftpPathTransferOptions;
    use std::sync::{Arc, Mutex};

    #[test]
    fn send_to_reports_partial_progress_and_keeps_the_content_size_in_its_output() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-send-progress");
        std::fs::create_dir_all(root.path()).unwrap();
        let source = root.join("source");
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        let payload = vec![7_u8; 512 * 1024];
        std::fs::write(&source, &payload).unwrap();
        let values = Arc::new(Mutex::new(Vec::new()));
        let record = values.clone();
        let output = run_send_to(
            SendToRequest {
                source_service: FileBrowserService::local(),
                target_service: FileBrowserService::local(),
                source_remote: RemoteFilePath::new(source.to_string_lossy()),
                cached_target_dir: Some(target.to_string_lossy().into_owned()),
                entry_name: "copy".into(),
                target_session_id: "target".into(),
                control: SftpTransferControl::new(),
                options: SftpPathTransferOptions::default(),
            },
            move |p| record.lock().unwrap().push(p),
        )
        .unwrap();
        assert!(
            matches!(output, TransferJobOutput::Sent { bytes, .. } if bytes == payload.len() as u64)
        );
        let values = values.lock().unwrap();
        assert!(
            values
                .iter()
                .any(|p| p.bytes_transferred > 0 && p.bytes_transferred < payload.len() as u64)
        );
        assert_eq!(
            values.last().unwrap().bytes_transferred,
            payload.len() as u64
        );
        assert_eq!(std::fs::read(target.join("copy")).unwrap(), payload);
    }
}
