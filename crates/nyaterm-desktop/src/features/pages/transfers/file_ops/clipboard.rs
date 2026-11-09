use crate::features::NyaTermApp;
use crate::features::transfers::TransferFileClipboard;
use crate::features::transfers::clipboard_copy::file_operation_output;
use crate::features::transfers::transfer_jobs::TransferProgressEventSender;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobResult, TransferJobState, TransferJobStatus,
};
use gpui::{Context, Window};
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, FileBrowserService, file_browser_copy_destination_invalid,
    file_browser_join, file_browser_name, file_browser_path_is_root,
};
use nyaterm_files::operations::{VerifiedCopyRequest, copy_verified};
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
};
use rust_i18n::t;
use std::path::PathBuf;

fn invalid_copy_destination(
    kind: FileBrowserBackendKind,
    source: &str,
    target: &str,
    directory: bool,
) -> bool {
    file_browser_copy_destination_invalid(kind, source, target, directory)
}

fn external_file_paths(cx: &gpui::App) -> Vec<PathBuf> {
    cx.read_from_clipboard()
        .and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

fn prefer_external_paths(paths: &[PathBuf], internal: Option<&TransferFileClipboard>) -> bool {
    !paths.is_empty() && internal.is_none_or(|clipboard| clipboard.os_paths_at_capture != paths)
}

fn local_clipboard_entry(path: PathBuf) -> Option<SftpFileEntry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if !path.is_absolute() || matches!(name.as_str(), "." | "..") {
        return None;
    }
    Some(SftpFileEntry {
        name,
        path: path.to_string_lossy().into_owned(),
        file_type: SftpFileType::File,
        size: None,
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    })
}

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn can_paste_transfer_file_clipboard(
        &self,
        cx: &gpui::App,
    ) -> bool {
        self.transfer
            .file_clipboard()
            .is_some_and(|clipboard| !clipboard.entries.is_empty())
            || !external_file_paths(cx).is_empty()
    }

    pub(in crate::features) fn capture_transfer_file_clipboard(
        &mut self,
        cut: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source_session_id) = self.session.active_id_owned() else {
            return;
        };
        if cut && self.session.active_file_browser_backend() != Some(FileBrowserBackendKind::Remote)
        {
            self.shell
                .set_status("cut is only available for remote files");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        }
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(FileBrowserBackendKind::Remote);
        let selected = self.selected_transfer_entries();
        let entries = selected
            .iter()
            .filter(|entry| {
                !entry.path.trim().is_empty()
                    && !file_browser_path_is_root(backend, &entry.path)
                    && !matches!(entry.name.as_str(), "." | "..")
                    && (!cut || !entry.is_symlink())
                    && !selected.iter().any(|parent| {
                        parent.path != entry.path
                            && parent.is_directory()
                            && file_browser_copy_destination_invalid(
                                backend,
                                &parent.path,
                                &entry.path,
                                true,
                            )
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if entries.is_empty() {
            self.shell.set_status("select files to copy or cut");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        }
        let count = entries.len();
        self.transfer
            .set_file_clipboard(source_session_id, entries, cut, external_file_paths(cx));
        self.shell.set_status(format!(
            "{} {count} file item(s)",
            if cut { "cut" } else { "copied" }
        ));
        self.notify_operation_at(
            cx.active_window(),
            "file-clipboard",
            nyaterm_ui::notification::NyaNotificationKind::Success,
            t!("common.operationSucceeded").to_string(),
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn paste_transfer_file_clipboard(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_session_id) = self.session.active_id_owned() else {
            return;
        };
        let os_paths = external_file_paths(cx);
        let internal = self.transfer.file_clipboard().cloned();
        let prefer_local = prefer_external_paths(&os_paths, internal.as_ref());
        let clipboard = if prefer_local {
            TransferFileClipboard {
                source_session_id: target_session_id.clone(),
                entries: os_paths
                    .into_iter()
                    .filter_map(local_clipboard_entry)
                    .collect(),
                cut: false,
                generation: 0,
                os_paths_at_capture: Vec::new(),
            }
        } else if let Some(internal) = internal {
            internal
        } else {
            self.shell.set_status("file clipboard is empty");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        };
        if clipboard.entries.is_empty() {
            self.shell.set_status("file clipboard has no usable paths");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        }
        if self.session.is_disconnected(&target_session_id)
            || (!prefer_local && self.session.is_disconnected(&clipboard.source_session_id))
        {
            self.shell.set_status("file clipboard session disconnected");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        }
        let source_service = if prefer_local {
            Some(FileBrowserService::local())
        } else {
            self.file_browser_service_for_session(&clipboard.source_session_id)
                .ok()
        };
        let target_service = self
            .file_browser_service_for_session(&target_session_id)
            .ok();
        let (Some(source_service), Some(target_service)) = (source_service, target_service) else {
            self.shell
                .set_status("file clipboard session is unavailable");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        };
        if clipboard.cut && target_service.kind() != FileBrowserBackendKind::Remote {
            self.shell.set_status("cut requires a remote destination");
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("common.operationFailed").to_string(),
                cx,
            );
            cx.notify();
            return;
        }
        let source_config = self
            .session
            .metadata(&clipboard.source_session_id)
            .and_then(|metadata| metadata.ssh_config.as_ref());
        let target_config = self
            .session
            .metadata(&target_session_id)
            .and_then(|metadata| metadata.ssh_config.as_ref());
        let same_endpoint = clipboard.source_session_id == target_session_id
            || matches!((source_config, target_config), (Some(a), Some(b))
                if a.host.eq_ignore_ascii_case(&b.host) && a.port == b.port && a.username == b.username)
            || (source_service.kind() == FileBrowserBackendKind::Local
                && target_service.kind() == FileBrowserBackendKind::Local);
        let target_dir = self.transfer_browser_operation_target_directory();
        let duplicate_policy = if clipboard.cut {
            SftpDuplicatePolicy::Skip
        } else {
            self.transfer.duplicate_policy()
        };
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask).then(|| {
            self.session.prompt_duplicate_broker() as std::sync::Arc<dyn SftpDuplicateResolver>
        });
        let options = SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        );
        let mut started = 0;
        for entry in clipboard.entries {
            let name = file_browser_name(source_service.kind(), &entry.path);
            let target_path = file_browser_join(target_service.kind(), &target_dir, &name);
            if same_endpoint
                && invalid_copy_destination(
                    source_service.kind(),
                    &entry.path,
                    &target_path,
                    entry.is_directory(),
                )
            {
                continue;
            }
            let id = self.transfer.next_transfer_job_id("file-clipboard");
            let control = SftpTransferControl::new();
            self.transfer.enqueue_transfer_job(TransferJobState {
                id: id.clone(),
                session_id: Some(clipboard.source_session_id.clone()),
                kind: TransferJobKind::SendTo {
                    source_path: entry.path.clone(),
                    target_session_id: target_session_id.clone(),
                    target_path: target_path.clone(),
                    target_parent_path: target_dir.clone(),
                },
                status: TransferJobStatus::Running,
                detail: format!(
                    "{} {}",
                    if clipboard.cut { "Moving" } else { "Copying" },
                    entry.path
                ),
                created_at_ms: TransferJobState::now_ms(),
                display_name: String::new(),
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: Some(control.clone()),
                speed: Default::default(),
            });
            if clipboard.cut {
                self.transfer
                    .track_cut_job(id.clone(), clipboard.generation, entry.path.clone());
            }
            let request = VerifiedCopyRequest {
                source_service: source_service.clone(),
                target_service: target_service.clone(),
                source: entry.remote_path(),
                target_path: RemoteFilePath::new(target_path),
                cut: clipboard.cut,
                control,
                options: options.clone(),
            };
            let tx = self.transfer.transfer_event_sender();
            let output_target_session_id = target_session_id.clone();
            self.submit_bulk_transfer_job("file-clipboard", id.clone(), tx.clone(), move || {
                let mut progress = TransferProgressEventSender::new(id.clone(), tx.clone());
                let result = copy_verified(request, move |value| progress.send(value))
                    .map(|result| file_operation_output(result, output_target_session_id));
                let _ = tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            });
            started += 1;
        }
        self.shell
            .set_status(format!("started {started} file clipboard job(s)"));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TransferFileClipboard, invalid_copy_destination, local_clipboard_entry,
        prefer_external_paths,
    };
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    use nyaterm_files::sftp::SftpFileType;
    use std::path::PathBuf;

    #[test]
    fn remote_copy_rejects_self_descendant_and_root_after_normalization() {
        assert!(invalid_copy_destination(
            FileBrowserBackendKind::Remote,
            "/src",
            "/src/./",
            false
        ));
        assert!(invalid_copy_destination(
            FileBrowserBackendKind::Remote,
            "/src",
            "/src/child/file",
            true
        ));
        assert!(invalid_copy_destination(
            FileBrowserBackendKind::Remote,
            "/",
            "/other",
            true
        ));
        assert!(!invalid_copy_destination(
            FileBrowserBackendKind::Remote,
            "/src",
            "/other/file",
            true
        ));
    }

    #[test]
    fn newer_external_paths_override_internal_clipboard() {
        let original = PathBuf::from("/original.txt");
        let clipboard = TransferFileClipboard {
            source_session_id: "source".into(),
            entries: Vec::new(),
            cut: false,
            generation: 1,
            os_paths_at_capture: vec![original.clone()],
        };
        assert!(!prefer_external_paths(&[original], Some(&clipboard)));
        assert!(prefer_external_paths(
            &[PathBuf::from("/new.txt")],
            Some(&clipboard)
        ));
        assert!(prefer_external_paths(&[PathBuf::from("/new.txt")], None));
        assert!(!prefer_external_paths(&[], None));
    }

    #[test]
    fn local_clipboard_paths_require_absolute_named_entries() {
        assert!(local_clipboard_entry(PathBuf::from("relative.txt")).is_none());
        assert!(local_clipboard_entry(PathBuf::from("/")).is_none());
        let entry = local_clipboard_entry(std::env::temp_dir().join("example.txt")).unwrap();
        assert_eq!(entry.name, "example.txt");
        assert_eq!(entry.file_type, SftpFileType::File);
    }
}
