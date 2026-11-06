use crate::features::NyaTermApp;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus, TransferMoveEntry, TransferMoveState,
};
use gpui::{Context, Window};
use nyaterm_core::truncate_preview;
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, file_browser_join, file_browser_name, file_browser_parent,
    file_browser_path_is_root,
};
use nyaterm_files::sftp::RemoteFilePath;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn open_transfer_move_dialog(
        &mut self,
        identity: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("transfer.move.");
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(FileBrowserBackendKind::Remote);
        let entry = if self.settings.summary().ui_file_explorer_view_mode
            == nyaterm_core::TransferBrowserViewMode::Tree
        {
            self.selected_transfer_entries()
                .into_iter()
                .find(|entry| entry.path == identity || entry.matches_identity(&identity))
        } else {
            self.transfer
                .browser_view()
                .entries
                .iter()
                .find(|entry| entry.matches_identity(&identity))
                .cloned()
        };
        let old_path = entry
            .as_ref()
            .map(|entry| entry.path.clone())
            .unwrap_or(identity);
        let name = file_browser_name(backend, &old_path);
        if old_path.trim().is_empty()
            || file_browser_path_is_root(backend, &old_path)
            || name == "."
            || name == ".."
        {
            self.shell
                .set_status(t!("fileTransfer.statusCannotMove", path = old_path).to_string());
            cx.notify();
            return;
        }
        self.transfer.open_move_dialog(TransferMoveState {
            old_path: old_path.clone(),
            raw_path_token: entry.and_then(|entry| entry.raw_path_token),
            name: name.clone(),
            value: old_path,
            additional_entries: Vec::new(),
        });
        self.shell
            .set_status(t!("fileTransfer.statusMoveOpened").to_string());
        self.open_form_dialog(
            (
                t!("fileExplorer.moveTo", name = truncate_preview(&name, 48)).to_string(),
                384.,
                t!("common.save").to_string(),
                |app, _, cx| app.transfer_move_dialog_content(cx),
                |app, window, cx| app.submit_transfer_move(window, cx),
                |app, cx| app.close_transfer_move_dialog(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    /// Open the move dialog for the current selection set (Tauri
    /// `openMoveDialog(getContextMenuEntries)`).
    ///
    /// A single selected item keeps the rename-style behavior (the dialog edits
    /// the full destination path). More than one item switches to a batch move:
    /// the dialog edits a destination *directory* and every entry is moved into
    /// it under its own name.
    pub(in crate::features) fn open_transfer_move_dialog_for_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self
            .selected_transfer_entries()
            .into_iter()
            .filter(|entry| {
                let backend = self
                    .session
                    .active_file_browser_backend()
                    .unwrap_or(FileBrowserBackendKind::Remote);
                let name = file_browser_name(backend, &entry.path);
                !entry.path.trim().is_empty()
                    && !file_browser_path_is_root(backend, &entry.path)
                    && name != "."
                    && name != ".."
            })
            .collect::<Vec<_>>();

        let Some(first) = entries.first().cloned() else {
            self.shell
                .set_status(t!("fileTransfer.statusMarkBeforeMoving").to_string());
            cx.notify();
            return;
        };

        if entries.len() == 1 {
            self.open_transfer_move_dialog(first.path.clone(), window, cx);
            return;
        }

        self.forget_text_inputs("transfer.move.");
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(FileBrowserBackendKind::Remote);
        let additional_entries = entries
            .iter()
            .skip(1)
            .map(|entry| TransferMoveEntry {
                old_path: entry.path.clone(),
                raw_path_token: entry.raw_path_token.clone(),
                name: file_browser_name(backend, &entry.path),
            })
            .collect::<Vec<_>>();
        // The default destination directory is the parent the selection lives in.
        let default_dir = file_browser_parent(backend, &first.path);
        self.transfer.open_move_dialog(TransferMoveState {
            old_path: first.path.clone(),
            raw_path_token: first.raw_path_token.clone(),
            name: file_browser_name(backend, &first.path),
            value: default_dir,
            additional_entries,
        });
        self.shell
            .set_status(t!("fileTransfer.statusBatchMoveOpened").to_string());
        self.open_form_dialog(
            (
                t!("fileExplorer.moveMultipleTo", count = entries.len()).to_string(),
                384.,
                t!("common.save").to_string(),
                |app, _, cx| app.transfer_move_dialog_content(cx),
                |app, window, cx| app.submit_transfer_move(window, cx),
                |app, cx| app.close_transfer_move_dialog(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_move_dialog(&mut self, cx: &mut Context<Self>) {
        self.forget_text_inputs("transfer.move.");
        self.transfer.close_move_dialog();
        self.shell
            .set_status(t!("fileTransfer.statusMoveCancelled").to_string());
        cx.notify();
    }

    pub(in crate::features) fn submit_transfer_move(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.transfer.move_dialog().cloned() else {
            self.shell
                .set_status(t!("fileTransfer.statusMoveInactive").to_string());
            cx.notify();
            return true;
        };
        let new_path = state.value.trim().to_string();
        if new_path.is_empty() {
            self.shell
                .set_status(t!("fileTransfer.statusMovePathRequired").to_string());
            cx.notify();
            return false;
        }

        // Batch move: `value` is a destination directory and each selected entry
        // is moved into it under its own name.
        if !state.additional_entries.is_empty() {
            let backend = self
                .session
                .active_file_browser_backend()
                .unwrap_or(FileBrowserBackendKind::Remote);
            let target_dir = new_path;
            self.transfer.close_move_dialog();
            let mut started = 0;
            let all_entries = std::iter::once(TransferMoveEntry {
                old_path: state.old_path.clone(),
                raw_path_token: state.raw_path_token.clone(),
                name: state.name.clone(),
            })
            .chain(state.additional_entries)
            .collect::<Vec<_>>();
            for entry in all_entries {
                let destination = file_browser_join(backend, &target_dir, &entry.name);
                if destination == entry.old_path {
                    continue;
                }
                self.start_sftp_move_job(
                    RemoteFilePath {
                        display_path: entry.old_path,
                        raw_path_token: entry.raw_path_token,
                    },
                    destination,
                    window,
                    cx,
                );
                started += 1;
            }
            self.shell
                .set_status(t!("fileTransfer.statusMoveJobsStarted", count = started).to_string());
            cx.notify();
            return true;
        }

        if new_path == state.old_path {
            self.transfer.close_move_dialog();
            self.shell
                .set_status(t!("fileTransfer.statusMoveUnchanged").to_string());
            cx.notify();
            return true;
        }
        self.transfer.close_move_dialog();
        self.start_sftp_move_job(
            RemoteFilePath {
                display_path: state.old_path,
                raw_path_token: state.raw_path_token,
            },
            new_path,
            window,
            cx,
        );
        true
    }

    /// Apply an edit from the move dialog's path box.
    pub(in crate::features) fn apply_transfer_move_path(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.set_move_value(text) {
            cx.notify();
        }
    }

    pub(in crate::features) fn start_sftp_move_job(
        &mut self,
        old_path: RemoteFilePath,
        new_path: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let backend = service.kind();
        let old_display_path = old_path.display_path.clone();
        let parent_path = file_browser_parent(backend, &old_display_path);
        let id = self.transfer.next_transfer_job_id("sftp-move");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::Move {
                old_path: old_display_path.clone(),
                new_path: new_path.clone(),
                parent_path: parent_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: t!("fileTransfer.detailMoving", path = old_display_path.clone()).to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell.set_status(
            t!(
                "fileTransfer.statusMoveStarted",
                from = old_display_path.clone(),
                to = new_path.clone()
            )
            .to_string(),
        );
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-move",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .rename_remote_paths(&old_path, &RemoteFilePath::new(&new_path))
                    .and_then(|_| service.list_dir(&parent_path))
                    .map(|entries| TransferJobOutput::Moved {
                        old_path: old_display_path,
                        new_path,
                        parent_path,
                        entries,
                    })
                    .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn open_selected_transfer_delete_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(FileBrowserBackendKind::Remote);
        let paths = self
            .selected_transfer_entries()
            .into_iter()
            .map(|entry| entry.remote_path())
            .filter(|path| {
                let name = file_browser_name(backend, &path.display_path);
                !path.display_path.trim().is_empty()
                    && !file_browser_path_is_root(backend, &path.display_path)
                    && name != "."
                    && name != ".."
            })
            .collect::<Vec<_>>();
        if paths.is_empty() {
            self.shell
                .set_status(t!("fileTransfer.statusMarkBeforeDeleting").to_string());
            cx.notify();
            return;
        }
        let delete_count = paths.len();
        let title = if delete_count == 1 {
            t!(
                "fileExplorer.sureDelete",
                name = file_browser_name(backend, &paths[0].display_path)
            )
            .to_string()
        } else {
            t!("fileExplorer.sureDeleteMultiple", count = delete_count).to_string()
        };
        let preview = paths
            .iter()
            .take(6)
            .map(|path| truncate_preview(&file_browser_name(backend, &path.display_path), 72))
            .collect::<Vec<_>>()
            .join("\n");
        let remaining = delete_count.saturating_sub(6);
        let mut detail = t!("common.deletingConfirmDesc").to_string();
        if delete_count > 1 {
            detail.push_str("\n\n");
            detail.push_str(&preview);
            if remaining > 0 {
                detail.push('\n');
                detail.push_str(&t!("fileExplorer.moreItems", count = remaining));
            }
        }
        self.shell
            .set_status(t!("fileTransfer.statusDeleteConfirmOpened").to_string());
        self.open_confirm_dialog(
            (
                title,
                detail,
                t!("fileExplorer.delete").to_string(),
                true,
                move |app, window, cx| {
                    app.start_sftp_delete_batch(paths.clone(), window, cx);
                    true
                },
            ),
            window,
            cx,
        );
        cx.notify();
    }

    fn start_sftp_delete_batch(
        &mut self,
        paths: Vec<RemoteFilePath>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let backend = service.kind();
        let batch_id = self.transfer.next_transfer_job_id("sftp-delete-batch");
        let jobs = paths
            .into_iter()
            .map(|path| (self.transfer.next_transfer_job_id("sftp-delete"), path))
            .collect::<Vec<_>>();
        self.transfer
            .begin_delete_batch(batch_id.clone(), backend, jobs.clone());
        self.shell
            .set_status(t!("fileTransfer.statusDeleteJobsStarted", count = jobs.len()).to_string());
        for (id, remote_path) in jobs {
            let service = service.clone();
            let remote_display_path = remote_path.display_path.clone();
            self.transfer.enqueue_transfer_job(TransferJobState {
                id: id.clone(),
                session_id: self.session.active_id_owned(),
                kind: TransferJobKind::Delete {
                    remote_path: remote_display_path.clone(),
                    batch_id: batch_id.clone(),
                },
                status: TransferJobStatus::Running,
                detail: t!(
                    "fileTransfer.detailDeleting",
                    path = remote_display_path.clone()
                )
                .to_string(),
                created_at_ms: TransferJobState::now_ms(),
                display_name: String::new(),
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: Some(nyaterm_files::sftp::SftpTransferControl::new()),
                speed: Default::default(),
            });
            let transfer_tx = self.transfer.transfer_event_sender();
            self.submit_bulk_transfer_job(
                "sftp-delete",
                id.clone(),
                transfer_tx.clone(),
                move || {
                    let result = service
                        .delete_remote_path(&remote_path)
                        .map(|_| TransferJobOutput::Deleted)
                        .map_err(|error| error.to_string());
                    let _ = transfer_tx.unbounded_send(TransferJobResult {
                        id,
                        event: TransferJobEvent::Finished(result),
                    });
                },
            );
        }
        cx.notify();
    }
}
