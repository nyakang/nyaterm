use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus, TransferNewFileState, TransferNewFolderState,
};
use gpui::{Context, Window};

use nyaterm_transport::file_browser_join;

impl NyaTermApp {
    pub(in crate::features) fn open_transfer_new_folder_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent_path = self.transfer_browser_operation_target_directory();
        self.open_transfer_new_folder_dialog_at(parent_path, window, cx);
    }

    pub(in crate::features::pages::transfers) fn open_transfer_new_folder_dialog_at(
        &mut self,
        parent_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .open_new_folder_dialog(TransferNewFolderState {
                parent_path,
                value: String::new(),
                mode: 0o755,
                open_after_create: false,
            });
        // The box owns its text, so it has to be dropped for the next dialog to
        // open empty.
        self.forget_text_inputs("transfer.new-folder.");
        self.shell
            .set_status("remote folder creation opened".to_string());
        self.open_form_dialog(
            (
                t!("fileExplorer.newFolder").to_string(),
                500.,
                t!("common.confirm").to_string(),
                |app, _, cx| app.transfer_new_folder_dialog_content(cx),
                |app, window, cx| app.submit_transfer_new_folder(window, cx),
                |app, cx| app.close_transfer_new_folder_dialog(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_new_folder_dialog(&mut self, cx: &mut Context<Self>) {
        self.transfer.close_new_folder_dialog();
        self.forget_text_inputs("transfer.new-folder.");
        self.shell
            .set_status("remote folder creation cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn submit_transfer_new_folder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.transfer.new_folder_dialog().cloned() else {
            self.shell
                .set_status("no remote folder creation is active".to_string());
            cx.notify();
            return true;
        };
        let name = state.value.trim().to_string();
        if !self.valid_transfer_browser_child_name(&name) {
            self.shell
                .set_status("folder name must be a single non-empty name".to_string());
            cx.notify();
            return false;
        }
        self.transfer.close_new_folder_dialog();
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(nyaterm_transport::FileBrowserBackendKind::Remote);
        let remote_path = file_browser_join(backend, &state.parent_path, &name);
        self.start_sftp_mkdir_job(
            remote_path,
            state.parent_path,
            state.mode,
            state.open_after_create,
            window,
            cx,
        );
        true
    }

    /// Apply an edit from the new-folder dialog's name box.
    pub(in crate::features) fn apply_transfer_new_folder_name(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        // A remote name has a length limit, and the box will happily take more.
        if self
            .transfer
            .set_new_folder_name(text.chars().take(255).collect())
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn start_sftp_mkdir_job(
        &mut self,
        remote_path: String,
        parent_path: String,
        mode: u32,
        open_after_create: bool,
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
        let id = self.transfer.next_transfer_job_id("sftp-mkdir");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::Mkdir {
                remote_path: remote_path.clone(),
                parent_path: parent_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Creating {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell
            .set_status(format!("remote folder creation started: {remote_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-create-directory",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = {
                    let list_path = if open_after_create {
                        remote_path.clone()
                    } else {
                        parent_path.clone()
                    };
                    service
                        .create_dir_path(&remote_path, Some(mode))
                        .and_then(|_| service.list_dir(&list_path))
                }
                .map(|entries| TransferJobOutput::CreatedDirectory {
                    remote_path,
                    parent_path,
                    entries,
                    open_after_create,
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

    pub(in crate::features) fn open_transfer_new_file_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent_path = self.transfer_browser_operation_target_directory();
        self.open_transfer_new_file_dialog_at(parent_path, window, cx);
    }

    pub(in crate::features::pages::transfers) fn open_transfer_new_file_dialog_at(
        &mut self,
        parent_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer.open_new_file_dialog(TransferNewFileState {
            parent_path,
            value: String::new(),
            mode: 0o644,
            open_after_create: false,
        });
        self.forget_text_inputs("transfer.new-file.");
        self.shell
            .set_status("remote file creation opened".to_string());
        self.open_form_dialog(
            (
                t!("fileExplorer.newFile").to_string(),
                500.,
                t!("common.confirm").to_string(),
                |app, _, cx| app.transfer_new_file_dialog_content(cx),
                |app, window, cx| app.submit_transfer_new_file(window, cx),
                |app, cx| app.close_transfer_new_file_dialog(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_new_file_dialog(&mut self, cx: &mut Context<Self>) {
        self.transfer.close_new_file_dialog();
        self.forget_text_inputs("transfer.new-file.");
        self.shell
            .set_status("remote file creation cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn submit_transfer_new_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.transfer.new_file_dialog().cloned() else {
            self.shell
                .set_status("no remote file creation is active".to_string());
            cx.notify();
            return true;
        };
        let name = state.value.trim().to_string();
        if !self.valid_transfer_browser_child_name(&name) {
            self.shell
                .set_status("file name must be a single non-empty name".to_string());
            cx.notify();
            return false;
        }
        self.transfer.close_new_file_dialog();
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(nyaterm_transport::FileBrowserBackendKind::Remote);
        let remote_path = file_browser_join(backend, &state.parent_path, &name);
        self.start_sftp_create_file_job(
            remote_path,
            state.parent_path,
            state.mode,
            state.open_after_create,
            window,
            cx,
        );
        true
    }

    /// Apply an edit from the new-file dialog's name box.
    pub(in crate::features) fn apply_transfer_new_file_name(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .transfer
            .set_new_file_name(text.chars().take(255).collect())
        {
            cx.notify();
        }
    }
    pub(in crate::features) fn start_sftp_create_file_job(
        &mut self,
        remote_path: String,
        parent_path: String,
        mode: u32,
        open_after_create: bool,
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
        let id = self.transfer.next_transfer_job_id("sftp-create-file");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::CreateFile {
                remote_path: remote_path.clone(),
                parent_path: parent_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Creating {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell
            .set_status(format!("remote file creation started: {remote_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-create-file",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .create_file_path(&remote_path, Some(mode))
                    .and_then(|_| service.list_dir(&parent_path))
                    .map(|entries| TransferJobOutput::CreatedFile {
                        remote_path,
                        parent_path,
                        entries,
                        open_after_create,
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
