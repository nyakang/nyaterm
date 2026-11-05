use super::super::NATIVE_EDITOR_MAX_BYTES;
use crate::features::NyaTermApp;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};
use gpui::{Context, Window};
use nyaterm_files::remote_file::{RemoteTextGeneration, RemoteTextRevision};
use nyaterm_files::sftp::RemoteFilePath;

impl NyaTermApp {
    pub(in crate::features) fn activate_transfer_editor_tab(
        &mut self,
        tab_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.activate_editor_tab(tab_id) {
            if let Some(view) = self.transfer.workspace_editor_view(tab_id) {
                view.update(cx, |view, cx| view.request_focus(cx));
            }
            cx.notify();
        }
    }

    pub(in crate::features) fn close_transfer_editor_tab(
        &mut self,
        tab_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.request_workspace_tabs_close(
            vec![crate::models::WorkspaceTabId::Editor(tab_id.to_string()).layout_key()],
            None,
            cx,
        );
    }

    pub(in crate::features) fn close_transfer_editor(&mut self, cx: &mut Context<Self>) {
        let targets = self
            .transfer
            .editor_documents()
            .iter()
            .filter(|doc| doc.display == nyaterm_core::InternalEditorDisplay::Window)
            .map(|doc| crate::models::WorkspaceTabId::Editor(doc.id.clone()).layout_key())
            .collect();
        self.request_workspace_tabs_close(targets, None, cx);
    }

    pub(in crate::features) fn start_sftp_editor_load_job(
        &mut self,
        session_id: Option<String>,
        tab_id: String,
        remote_file_path: RemoteFilePath,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote_path = remote_file_path.display_path.clone();
        let Some(generation) = self
            .transfer
            .editor_tab_snapshot(&tab_id)
            .map(|tab| tab.generation)
        else {
            return;
        };
        let service = match self.transfer_file_browser_service(session_id.as_deref()) {
            Ok(service) => service,
            Err(error) => {
                let error = error.to_string();
                self.transfer
                    .fail_editor_load_tab(&tab_id, generation, error.clone());
                self.shell.set_status(error);
                cx.notify();
                return;
            }
        };
        let id = self.transfer.next_transfer_job_id("sftp-open-text");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id,
            kind: TransferJobKind::LoadEditor {
                remote_path: remote_path.clone(),
                tab_id: tab_id.clone(),
                generation,
            },
            status: TransferJobStatus::Running,
            detail: format!("Opening {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-editor-load",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .read_text_document_path(&remote_file_path, NATIVE_EDITOR_MAX_BYTES)
                    .map(|file| TransferJobOutput::EditorLoaded {
                        tab_id,
                        remote_path,
                        generation,
                        file,
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

    pub(in crate::features) fn save_all_transfer_editor_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_ids = self.transfer.dirty_editor_tab_ids();
        for tab_id in tab_ids {
            self.save_transfer_editor_tab(&tab_id, false, window, cx);
        }
    }

    pub(in crate::features) fn save_transfer_editor_tab(
        &mut self,
        tab_id: &str,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.transfer.editor_tab_snapshot(tab_id) else {
            return;
        };
        if snapshot.loading || snapshot.saving {
            return;
        }
        if !self.transfer.begin_editor_tab_save(tab_id) {
            return;
        }
        let remote_file_path = snapshot.remote_file_path();
        self.start_sftp_editor_save_job(
            snapshot.session_id,
            snapshot.id,
            remote_file_path,
            snapshot.content.to_string(),
            snapshot.revision,
            snapshot.generation,
            force,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::features) fn start_sftp_editor_save_job(
        &mut self,
        session_id: Option<String>,
        tab_id: String,
        remote_file_path: RemoteFilePath,
        content: String,
        expected_revision: Option<RemoteTextRevision>,
        generation: RemoteTextGeneration,
        force: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote_path = remote_file_path.display_path.clone();
        let service = match self.transfer_file_browser_service(session_id.as_deref()) {
            Ok(service) => service,
            Err(error) => {
                self.transfer
                    .fail_editor_operation_tab(&tab_id, generation, error.to_string());
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let id = self.transfer.next_transfer_job_id("sftp-save-text");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id,
            kind: TransferJobKind::SaveEditor {
                remote_path: remote_path.clone(),
                tab_id: tab_id.clone(),
                generation,
            },
            status: TransferJobStatus::Running,
            detail: format!("Saving {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-editor-save",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .write_text_document_path(
                        &remote_file_path,
                        &content,
                        expected_revision.as_ref(),
                        force,
                    )
                    .map(|result| TransferJobOutput::EditorSaved {
                        tab_id,
                        remote_path,
                        generation,
                        result,
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
