use super::super::helpers::remote_file_name;
use super::helpers::{RemoteFileTextKind, open_local_path_with_editor, remote_file_text_kind};
use super::workspace::{ExternalEditWorkspace, prepare_external_edit_workspace};
use crate::features::NyaTermApp;
use crate::models::{
    TransferEditorField, TransferEditorState, TransferJobEvent, TransferJobKind, TransferJobOutput,
    TransferJobResult, TransferJobState, TransferJobStatus, TransferUnknownFileState,
};
use gpui::{Context, Window};
use nyaterm_core::AiCustomActionConfig;
use nyaterm_files::remote_file::RemoteTextGeneration;
use nyaterm_files::sftp::{
    SFTP_TRANSFER_CANCELLED, SftpFileEntry, SftpFileType, SftpTransferControl,
};
use std::path::Path;

impl NyaTermApp {
    pub(in crate::features) fn enabled_transfer_file_ai_actions_for_entry(
        &self,
        entry: &SftpFileEntry,
    ) -> Vec<AiCustomActionConfig> {
        if !self.ai.settings_config().enabled
            || entry.is_directory()
            || entry
                .size
                .is_some_and(|size| size > self.ai.settings_config().max_ai_file_size_bytes)
        {
            return Vec::new();
        }

        self.ai
            .settings_config()
            .file_ai_actions
            .iter()
            .filter(|action| action.enabled && !action.name.trim().is_empty())
            .cloned()
            .collect()
    }

    pub(in crate::features) fn start_transfer_file_ai_action(
        &mut self,
        entry: SftpFileEntry,
        action: AiCustomActionConfig,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.is_directory() {
            self.transfer
                .set_browser_status("AI file actions require a file");
            self.shell
                .set_status("directories cannot be sent to file AI actions".to_string());
            cx.notify();
            return;
        }
        if entry
            .size
            .is_some_and(|size| size > self.ai.settings_config().max_ai_file_size_bytes)
        {
            self.transfer
                .set_browser_status("file exceeds AI file size limit");
            self.shell
                .set_status(format!("{} is too large for AI file actions", entry.path));
            cx.notify();
            return;
        }
        if !self.ai.settings_config().enabled {
            self.transfer.set_browser_status("AI assistant is disabled");
            self.shell
                .set_status("AI assistant is disabled".to_string());
            cx.notify();
            return;
        }
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };

        self.transfer.select_browser_path(entry.path.clone());
        self.transfer.set_remote_path(entry.path.clone());

        let remote_file_path = entry.remote_path();
        let remote_path = entry.path.clone();
        let action_id = action.id.clone();
        let action_name = action.name.clone();
        let prompt = action.prompt.clone();
        let max_bytes = self.ai.settings_config().max_ai_file_size_bytes;
        let id = self.transfer.next_transfer_job_id("sftp-ai-file");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::AiFileAction {
                remote_path: remote_path.clone(),
                action_id: action_id.clone(),
                action_name: action_name.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Preparing AI file action {action_name} for {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.transfer
            .set_browser_status(format!("loading {remote_path} for AI"));
        self.shell
            .set_status(format!("remote file AI action started: {remote_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-ai-file-load",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .read_text_file_path(&remote_file_path, max_bytes)
                    .map(|file| TransferJobOutput::AiFileActionLoaded {
                        remote_path,
                        action_id,
                        action_name,
                        prompt,
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

    pub(in crate::features) fn open_selected_transfer_default(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.selected_transfer_entry() else {
            self.shell
                .set_status("select a remote file first".to_string());
            cx.notify();
            return;
        };
        self.open_transfer_default(entry, window, cx);
    }

    pub(in crate::features) fn open_selected_transfer_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.selected_transfer_entry() else {
            self.shell
                .set_status("select a remote file first".to_string());
            cx.notify();
            return;
        };
        self.open_transfer_editor(entry, window, cx);
    }

    pub(in crate::features) fn open_selected_transfer_external(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.selected_transfer_entry() else {
            self.shell
                .set_status("select a remote file first".to_string());
            cx.notify();
            return;
        };
        self.open_transfer_external(entry, window, cx);
    }

    pub(in crate::features) fn show_transfer_open_internal_menu_entry(
        &self,
        entry: &SftpFileEntry,
    ) -> bool {
        !entry.is_directory() && self.settings.summary().transfer_editor_type == "external"
    }

    pub(in crate::features) fn show_transfer_open_external_menu_entry(
        &self,
        entry: &SftpFileEntry,
    ) -> bool {
        !entry.is_directory() && self.settings.summary().transfer_editor_type == "internal"
    }

    pub(in crate::features) fn open_transfer_default(
        &mut self,
        entry: SftpFileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.summary().transfer_editor_type == "internal" {
            self.open_transfer_editor(entry, window, cx);
        } else {
            self.open_transfer_external(entry, window, cx);
        }
    }

    pub(in crate::features) fn open_transfer_editor(
        &mut self,
        entry: SftpFileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = self.session.active_id_owned();
        self.open_transfer_editor_for_session(entry, session_id, window, cx);
    }

    pub(in crate::features) fn open_transfer_editor_for_session(
        &mut self,
        entry: SftpFileEntry,
        session_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !entry.is_directory()
            && self.focus_existing_transfer_editor(&entry, session_id.as_deref(), cx)
        {
            return;
        }
        match remote_file_text_kind(&entry.name) {
            RemoteFileTextKind::Text => {
                self.open_transfer_editor_direct_for_session(entry, session_id, window, cx)
            }
            RemoteFileTextKind::Binary => {
                self.transfer
                    .set_browser_status("known binary file opened externally");
                self.open_transfer_external_for_session(entry, session_id, cx);
            }
            RemoteFileTextKind::Unknown => {
                self.transfer
                    .open_unknown_file_dialog(TransferUnknownFileState { entry, session_id });
                self.shell
                    .set_status("confirm how to open unknown remote file".to_string());
                self.open_transfer_unknown_file_component_dialog(window, cx);
                cx.notify();
            }
        }
    }

    fn focus_existing_transfer_editor(
        &mut self,
        entry: &SftpFileEntry,
        session_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(document) = self
            .transfer
            .editor_documents()
            .iter()
            .find(|document| {
                document.session_id.as_deref() == session_id
                    && document.remote_file_path().identity_key()
                        == entry.remote_path().identity_key()
            })
            .cloned()
        {
            let tab_id = document.id.clone();
            let status = format!("remote text file already open: {}", entry.path);
            if document.display == nyaterm_core::InternalEditorDisplay::Window {
                self.transfer.activate_editor_tab(&tab_id);
                self.open_remote_file_editor_window(cx);
            } else {
                self.activate_workspace_tab(
                    &crate::models::WorkspaceTabId::Editor(tab_id.clone()).layout_key(),
                    cx,
                );
            }
            self.transfer.set_browser_status(status.clone());
            self.shell.set_status(status);
            cx.notify();
            return true;
        }
        false
    }

    pub(in crate::features) fn open_transfer_editor_direct_for_session(
        &mut self,
        entry: SftpFileEntry,
        session_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.is_directory() {
            self.shell
                .set_status("directories cannot be opened in the text editor".to_string());
            cx.notify();
            return;
        }
        let remote_file_path = entry.remote_path();
        let tab_id =
            TransferEditorState::tab_id_for_remote_path(session_id.as_deref(), &remote_file_path);
        if self.focus_existing_transfer_editor(&entry, session_id.as_deref(), cx) {
            return;
        }
        let tab = TransferEditorState {
            display: self.settings.summary().transfer_internal_editor_display,
            id: tab_id.clone(),
            session_id: session_id.clone(),
            remote_path: entry.path.clone(),
            raw_path_token: entry.raw_path_token.clone(),
            name: entry.name.clone(),
            content: Default::default(),
            search_query: String::new(),
            active_match: 0,
            revision: None,
            generation: RemoteTextGeneration::next(),
            loading: true,
            saving: false,
            dirty: false,
            conflict: false,
            close_after_save: false,
            reload_confirm: false,
            error: None,
            backup_warning_path: None,
            focused_field: TransferEditorField::Content,
        };
        self.transfer.open_editor_tab(tab);
        self.shell
            .set_status(format!("opening remote text file {}", entry.path));
        if self.settings.summary().transfer_internal_editor_display
            == nyaterm_core::InternalEditorDisplay::Window
        {
            self.open_remote_file_editor_window(cx);
        } else {
            self.open_workspace_editor_tab(&tab_id, cx);
        }
        self.start_sftp_editor_load_job(session_id, tab_id, remote_file_path, window, cx);
        cx.notify();
    }

    pub(in crate::features) fn cancel_transfer_unknown_file(&mut self, cx: &mut Context<Self>) {
        if self.transfer.unknown_file_dialog().is_some() {
            self.transfer.close_unknown_file_dialog();
            self.shell
                .set_status("unknown file open cancelled".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn open_unknown_transfer_file_external(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.transfer.take_unknown_file_dialog() else {
            cx.notify();
            return;
        };
        self.open_transfer_external_for_session(state.entry, state.session_id, cx);
    }

    pub(in crate::features) fn open_unknown_transfer_file_internal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.transfer.take_unknown_file_dialog() else {
            cx.notify();
            return;
        };
        self.open_transfer_editor_direct_for_session(state.entry, state.session_id, window, cx);
    }

    pub(in crate::features) fn open_transfer_external(
        &mut self,
        entry: SftpFileEntry,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.is_directory() {
            self.shell
                .set_status("directories cannot be opened in an external editor".to_string());
            cx.notify();
            return;
        }
        let session_id = self.session.active_id_owned();
        self.open_transfer_external_for_session(entry, session_id, cx);
    }

    pub(in crate::features) fn open_editor_document_external(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.transfer.editor_tab_snapshot(id) else {
            return;
        };
        let entry = SftpFileEntry {
            name: tab.name,
            path: tab.remote_path,
            file_type: SftpFileType::File,
            size: tab.revision.as_ref().map(|revision| revision.metadata.size),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: tab.revision.as_ref().and_then(|revision| {
                revision
                    .metadata
                    .modified_at
                    .and_then(|value| value.try_into().ok())
            }),
            raw_path_token: tab.raw_path_token,
            symlink_target_is_directory: false,
        };
        self.open_transfer_external_for_session(entry, tab.session_id, cx);
    }

    pub(in crate::features) fn open_transfer_external_for_session(
        &mut self,
        entry: SftpFileEntry,
        session_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let service = match self.transfer_file_browser_service(session_id.as_deref()) {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                return;
            }
        };
        let remote_file_path = entry.remote_path();
        if let Some(local_path) = service.local_path(&remote_file_path) {
            let default_editor = self.settings.summary().transfer_default_editor.clone();
            let request = cx
                .background_executor()
                .spawn(async move { open_local_path_with_editor(&local_path, &default_editor) });
            cx.spawn(async move |this, cx| {
                let result = request.await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(()) => {
                            this.transfer
                                .set_browser_status(format!("opened {} externally", entry.path));
                            this.shell
                                .set_status(format!("opened local file {} externally", entry.path));
                        }
                        Err(error) => this.shell.set_status(error),
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        let Some(service) = service.remote_service() else {
            self.shell
                .set_status("file browser service is unavailable".to_string());
            return;
        };
        self.transfer.select_browser_path(entry.path.clone());
        self.transfer.set_remote_path(entry.path.clone());
        let remote_path = entry.path.clone();
        let file_name = if entry.name.trim().is_empty() {
            remote_file_name(&entry.path)
        } else {
            entry.name.clone()
        };
        let default_editor = self.settings.summary().transfer_default_editor.clone();
        let transfer_options = self.sftp_transfer_options();
        let id = self.transfer.next_transfer_job_id("sftp-open-external");
        let control = SftpTransferControl::new();
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id,
            kind: TransferJobKind::OpenExternal {
                remote_path: remote_path.clone(),
                local_path: None,
            },
            status: TransferJobStatus::Running,
            detail: format!("Opening {remote_path} externally"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: Some(control.clone()),
            speed: Default::default(),
        });
        self.shell
            .set_status(format!("downloading {remote_path} for external open"));

        let progress_tx = self.transfer.transfer_event_sender();
        let finished_tx = self.transfer.transfer_event_sender();
        self.submit_bulk_transfer_job(
            "sftp-open-external",
            id.clone(),
            finished_tx.clone(),
            move || {
                let progress_id = id.clone();
                let result = run_external_open(
                    &control,
                    || prepare_external_edit_workspace(&file_name, || control.is_cancelled())
                        .map_err(|error| format!("{error:#}")),
                    |local_path| {
                        let _ = progress_tx.unbounded_send(TransferJobResult {
                            id: id.clone(),
                            event: TransferJobEvent::ExternalWorkspacePrepared {
                                local_path: local_path.to_path_buf(),
                            },
                        });
                        service
                            .download_remote_file_with_progress_and_control_options(
                                &remote_file_path,
                                local_path,
                                control.clone(),
                                transfer_options,
                                move |progress| {
                                    let _ = progress_tx.unbounded_send(TransferJobResult {
                                        id: progress_id.clone(),
                                        event: TransferJobEvent::Progress(progress),
                                    });
                                },
                            )
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                    |local_path| {
                        let baseline = crate::features::transfers::external_sync_runtime::read_external_editor_version(
                            local_path,
                            || control.is_cancelled(),
                        )
                        .map_err(|error| {
                            if error.kind() == std::io::ErrorKind::Interrupted {
                                SFTP_TRANSFER_CANCELLED.to_string()
                            } else {
                                error.to_string()
                            }
                        })?;
                        if control.is_cancelled() {
                            return Err(SFTP_TRANSFER_CANCELLED.to_string());
                        }
                        open_local_path_with_editor(local_path, &default_editor).map(
                            |_| TransferJobOutput::ExternalOpened {
                                baseline,
                                remote_path: remote_path.clone(),
                                raw_path_token: remote_file_path.raw_path_token.clone(),
                                local_path: local_path.to_path_buf(),
                            },
                        )
                    },
                );
                let _ = finished_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }
}

// Keep the directory protection alive through both I/O stages. Only preparation owns
// root selection; a download or launch error must never repeat that selection.
fn run_external_open<T>(
    control: &SftpTransferControl,
    prepare: impl FnOnce() -> Result<ExternalEditWorkspace, String>,
    download: impl FnOnce(&Path) -> Result<(), String>,
    launch: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    if control.is_cancelled() {
        return Err(SFTP_TRANSFER_CANCELLED.to_string());
    }
    let workspace = prepare()?;
    if control.is_cancelled() {
        return Err(SFTP_TRANSFER_CANCELLED.to_string());
    }
    download(&workspace.local_path)?;
    if control.is_cancelled() {
        return Err(SFTP_TRANSFER_CANCELLED.to_string());
    }
    launch(&workspace.local_path)
}

#[cfg(test)]
mod tests {
    use super::run_external_open;
    use crate::features::pages::transfers::editor::workspace::{
        ExternalEditTestCleanup, prepare_external_edit_workspace,
    };
    use nyaterm_files::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};
    use std::cell::Cell;

    #[test]
    fn failed_workspace_preparation_never_downloads_or_launches() {
        let attempts = Cell::new(0);
        let result: Result<(), String> = run_external_open(
            &SftpTransferControl::new(),
            || {
                attempts.set(attempts.get() + 1);
                Err("TEMP win32_error=50; LocalAppData win32_error=5".into())
            },
            |_| panic!("download must not run without a private workspace"),
            |_| panic!("editor must not launch without a private workspace"),
        );
        assert_eq!(attempts.get(), 1);
        assert_eq!(
            result.unwrap_err(),
            "TEMP win32_error=50; LocalAppData win32_error=5"
        );
    }

    #[test]
    fn download_failure_or_cancellation_never_launches_or_prepares_again() -> anyhow::Result<()> {
        for cancel in [false, true] {
            let control = SftpTransferControl::new();
            let workspace = prepare_external_edit_workspace("example.txt", || false)?;
            let _cleanup = ExternalEditTestCleanup::new(&workspace);
            let expected = workspace.local_path.clone();
            let attempts = Cell::new(0);
            let result: Result<(), String> = run_external_open(
                &control,
                || {
                    attempts.set(attempts.get() + 1);
                    Ok(workspace)
                },
                |path| {
                    assert_eq!(path, expected);
                    if cancel {
                        control.cancel();
                        Ok(())
                    } else {
                        Err("download failed".into())
                    }
                },
                |_| panic!("editor must not launch after failed or cancelled download"),
            );
            assert_eq!(attempts.get(), 1);
            assert_eq!(
                result.unwrap_err(),
                if cancel {
                    SFTP_TRANSFER_CANCELLED
                } else {
                    "download failed"
                }
            );
        }
        Ok(())
    }

    #[test]
    fn launch_failure_keeps_the_downloaded_file_and_never_prepares_again() -> anyhow::Result<()> {
        let workspace = prepare_external_edit_workspace("example.txt", || false)?;
        let _cleanup = ExternalEditTestCleanup::new(&workspace);
        let expected = workspace.local_path.clone();
        let attempts = Cell::new(0);
        let result: Result<(), String> = run_external_open(
            &SftpTransferControl::new(),
            || {
                attempts.set(attempts.get() + 1);
                Ok(workspace)
            },
            |path| std::fs::write(path, b"downloaded").map_err(|error| error.to_string()),
            |path| {
                assert_eq!(path, expected);
                assert_eq!(std::fs::read(path).unwrap(), b"downloaded");
                #[cfg(windows)]
                assert!(
                    std::fs::rename(
                        path.parent().unwrap(),
                        path.parent().unwrap().with_extension("replaced")
                    )
                    .is_err()
                );
                Err("launch failed".into())
            },
        );
        assert_eq!(attempts.get(), 1);
        assert_eq!(result.unwrap_err(), "launch failed");
        assert_eq!(std::fs::read(&expected)?, b"downloaded");
        Ok(())
    }
}
