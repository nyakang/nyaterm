use rust_i18n::t;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus, TransferNewSymlinkState, TransferRenameState, TransferSymlinkField,
};
use gpui::{Context, KeyDownEvent, Window};
use nyaterm_transport::{
    FileBrowserBackendKind, RemoteFilePath, file_browser_join, file_browser_name,
    file_browser_parent, file_browser_path_is_root,
};

use super::super::helpers::{remote_child_path, valid_remote_child_name};

impl NyaTermApp {
    pub(in crate::features) fn open_transfer_new_symlink_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent_path = self.transfer_browser_operation_target_directory();
        self.open_transfer_new_symlink_dialog_at(parent_path, window, cx);
    }

    pub(in crate::features::pages::transfers) fn open_transfer_new_symlink_dialog_at(
        &mut self,
        parent_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .open_new_symlink_dialog(TransferNewSymlinkState {
                parent_path,
                name: "new-link".to_string(),
                target: String::new(),
                focused_field: TransferSymlinkField::Name,
            });
        self.forget_text_inputs("transfer.new-symlink.");
        self.shell
            .set_status("remote symlink creation opened".to_string());
        self.open_form_dialog(
            (
                t!("fileExplorer.newSymlink").to_string(),
                480.,
                t!("common.confirm").to_string(),
                |app, _, cx| app.transfer_new_symlink_dialog_content(cx),
                |app, window, cx| app.submit_transfer_new_symlink(window, cx),
                |app, cx| app.close_transfer_new_symlink_dialog(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_new_symlink_dialog(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.transfer.close_new_symlink_dialog();
        self.forget_text_inputs("transfer.new-symlink.");
        self.shell
            .set_status("remote symlink creation cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn submit_transfer_new_symlink(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.transfer.new_symlink_dialog().cloned() else {
            self.shell
                .set_status("no remote symlink creation is active".to_string());
            cx.notify();
            return true;
        };
        let name = state.name.trim().to_string();
        let target_path = state.target.trim().to_string();
        if !valid_remote_child_name(&name) {
            self.shell
                .set_status("symlink name must be a single non-empty name".to_string());
            cx.notify();
            return false;
        }
        if target_path.is_empty() {
            self.shell
                .set_status("symlink target cannot be empty".to_string());
            cx.notify();
            return false;
        }
        self.transfer.close_new_symlink_dialog();
        let link_path = remote_child_path(&state.parent_path, &name);
        self.start_sftp_symlink_job(link_path, target_path, state.parent_path, window, cx);
        true
    }

    /// Apply an edit from one of the symlink dialog's boxes.
    ///
    /// A remote name and a link target have different length limits, and both
    /// are enforced here rather than by the box.
    pub(in crate::features) fn apply_transfer_new_symlink_input(
        &mut self,
        field: TransferSymlinkField,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let value = match field {
            TransferSymlinkField::Name => text.chars().take(255).collect(),
            TransferSymlinkField::Target => text.chars().take(1024).collect(),
        };
        if self.transfer.set_new_symlink_input(field, value) {
            cx.notify();
        }
    }

    pub(in crate::features) fn start_sftp_symlink_job(
        &mut self,
        link_path: String,
        target_path: String,
        parent_path: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_remote_file_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let id = self.transfer.next_transfer_job_id("sftp-symlink");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::Symlink {
                link_path: link_path.clone(),
                target_path: target_path.clone(),
                parent_path: parent_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Linking {link_path} -> {target_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell
            .set_status(format!("remote symlink creation started: {link_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-create-symlink",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .create_symlink_path(&link_path, &target_path)
                    .and_then(|_| service.list_dir(&parent_path))
                    .map(|entries| TransferJobOutput::CreatedSymlink {
                        link_path,
                        target_path,
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

    pub(in crate::features) fn open_transfer_rename_dialog(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("transfer.rename.");
        self.ensure_panel_open(crate::models::NavItem::Transfers);
        let entries = self.selected_transfer_entries();
        let [entry] = entries.as_slice() else {
            self.shell
                .set_status("select exactly one file entry before renaming".to_string());
            cx.notify();
            return;
        };
        if !self.open_transfer_rename_for_entry(entry.clone(), cx) {
            return;
        }
        self.transfer.schedule_rename_focus();
        self.ensure_pending_focus_clock(cx);
        cx.notify();
    }

    pub(in crate::features) fn open_transfer_rename_for_path_and_focus(
        &mut self,
        old_path: String,
        cx: &mut Context<Self>,
    ) {
        if self.open_transfer_rename_for_path(old_path, cx) {
            self.transfer.schedule_rename_focus();
            self.ensure_pending_focus_clock(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn open_transfer_rename_for_path(
        &mut self,
        identity: String,
        cx: &mut Context<Self>,
    ) -> bool {
        self.forget_text_inputs("transfer.rename.");
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
        let initial_name = file_browser_name(backend, &old_path);
        if initial_name.is_empty()
            || file_browser_path_is_root(backend, &old_path)
            || initial_name == "."
            || initial_name == ".."
        {
            self.shell.set_status(format!("cannot rename {old_path}"));
            cx.notify();
            return false;
        }
        let raw_path_token = entry.and_then(|entry| entry.raw_path_token);
        self.ensure_text_input(
            format!("transfer.rename.{old_path}"),
            &initial_name,
            TextInputSetup::default(),
            cx,
        );
        self.transfer.open_rename_dialog(TransferRenameState {
            old_path,
            raw_path_token,
            value: initial_name.clone(),
            initial_name,
        });
        self.shell.set_status("remote rename opened".to_string());
        true
    }

    fn open_transfer_rename_for_entry(
        &mut self,
        entry: nyaterm_transport::SftpFileEntry,
        cx: &mut Context<Self>,
    ) -> bool {
        let opened = self.open_transfer_rename_for_path(entry.path.clone(), cx);
        if opened && let Some(state) = self.transfer.rename_dialog_mut() {
            state.raw_path_token = entry.raw_path_token;
        }
        opened
    }

    pub(in crate::features) fn close_transfer_rename_dialog(&mut self, cx: &mut Context<Self>) {
        self.forget_text_inputs("transfer.rename.");
        self.transfer.close_rename_dialog();
        self.shell.set_status("remote rename cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn dismiss_transfer_rename_if_open(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.transfer.rename_dialog_is_open() {
            return false;
        }
        self.close_transfer_rename_dialog(cx);
        true
    }

    pub(in crate::features) fn submit_transfer_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.transfer.rename_dialog().cloned() else {
            self.shell
                .set_status("no remote rename is active".to_string());
            cx.notify();
            return;
        };
        let new_name = state.value.trim().to_string();
        if new_name.is_empty() {
            self.shell
                .set_status("remote name cannot be empty".to_string());
            cx.notify();
            return;
        }
        if !self.valid_transfer_browser_child_name(&new_name) {
            self.shell
                .set_status("remote name must be a single file or directory name".to_string());
            cx.notify();
            return;
        }
        if new_name == state.initial_name {
            self.transfer.close_rename_dialog();
            self.shell.set_status("remote rename unchanged".to_string());
            cx.notify();
            return;
        }
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(FileBrowserBackendKind::Remote);
        let new_path = file_browser_join(
            backend,
            &file_browser_parent(backend, &state.old_path),
            &new_name,
        );
        self.transfer.close_rename_dialog();
        self.start_sftp_rename_job(
            RemoteFilePath {
                display_path: state.old_path,
                raw_path_token: state.raw_path_token,
            },
            new_path,
            window,
            cx,
        );
    }

    pub(in crate::features) fn handle_transfer_rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return;
        }

        // The box owns the text; the row owns the keys that cancel or commit.
        match keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.close_transfer_rename_dialog(cx);
            }
            "enter" => {
                cx.stop_propagation();
                self.submit_transfer_rename(window, cx);
            }
            _ => {}
        }
    }

    /// Apply an edit from the inline rename box.
    pub(in crate::features) fn apply_transfer_rename_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .transfer
            .set_rename_value(text.chars().take(255).collect())
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn start_sftp_rename_job(
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
        let id = self.transfer.next_transfer_job_id("sftp-rename");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::Rename {
                old_path: old_display_path.clone(),
                new_path: new_path.clone(),
                parent_path: parent_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Renaming {old_display_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell.set_status(format!(
            "remote rename started: {old_display_path} -> {new_path}"
        ));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-rename",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .rename_remote_paths(&old_path, &RemoteFilePath::new(&new_path))
                    .and_then(|_| service.list_dir(&parent_path))
                    .map(|entries| TransferJobOutput::Renamed {
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
}
