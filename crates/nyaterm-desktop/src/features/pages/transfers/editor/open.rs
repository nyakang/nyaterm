use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Context, Window};
use nyaterm_core::AiCustomActionConfig;
use nyaterm_transport::{RemoteTextGeneration, SftpFileEntry, SftpFileType, SftpTransferControl};

use crate::features::NyaTermApp;
use crate::models::{
    TransferEditorField, TransferEditorState, TransferJobEvent, TransferJobKind, TransferJobOutput,
    TransferJobResult, TransferJobState, TransferJobStatus, TransferUnknownFileState,
};

use super::super::helpers::remote_file_name;
use super::helpers::{
    RemoteFileTextKind, open_local_path_with_editor, remote_file_text_kind,
    sanitize_local_open_segment,
};

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
        match remote_file_text_kind(&entry.name) {
            RemoteFileTextKind::Text => self.open_transfer_editor_direct(entry, window, cx),
            RemoteFileTextKind::Binary => {
                self.transfer
                    .set_browser_status("known binary file opened externally");
                self.open_transfer_external(entry, window, cx);
            }
            RemoteFileTextKind::Unknown => {
                self.transfer
                    .open_unknown_file_dialog(TransferUnknownFileState { entry });
                self.shell
                    .set_status("confirm how to open unknown remote file".to_string());
                self.open_transfer_unknown_file_component_dialog(window, cx);
                cx.notify();
            }
        }
    }

    pub(in crate::features) fn open_transfer_editor_direct(
        &mut self,
        entry: SftpFileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.is_directory() {
            self.shell
                .set_status("directories cannot be opened in the text editor".to_string());
            cx.notify();
            return;
        }
        let session_id = self.session.active_id_owned();
        let remote_file_path = entry.remote_path();
        let tab_id =
            TransferEditorState::tab_id_for_remote_path(session_id.as_deref(), &remote_file_path);
        if self
            .transfer
            .editor_workspace()
            .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.id == tab_id))
        {
            self.transfer.activate_editor_tab(&tab_id);
            let status = format!("remote text file already open: {}", entry.path);
            self.open_remote_file_editor_window(cx);
            if !self.transfer.editor_window_is_open()
                && !self.transfer.editor_window_open_is_pending()
            {
                window.focus(self.transfer.editor_focus(), cx);
            }
            self.transfer.set_browser_status(status.clone());
            self.shell.set_status(status);
            cx.notify();
            return;
        }
        self.transfer.select_browser_path(entry.path.clone());
        self.transfer.set_remote_path(entry.path.clone());
        let tab = TransferEditorState {
            id: tab_id.clone(),
            session_id: session_id.clone(),
            remote_path: entry.path.clone(),
            raw_path_token: entry.raw_path_token.clone(),
            name: entry.name.clone(),
            content: String::new(),
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
            focused_field: TransferEditorField::Content,
        };
        self.transfer.open_editor_tab(tab);
        self.shell
            .set_status(format!("opening remote text file {}", entry.path));
        self.open_remote_file_editor_window(cx);
        if !self.transfer.editor_window_is_open() && !self.transfer.editor_window_open_is_pending()
        {
            window.focus(self.transfer.editor_focus(), cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.transfer.take_unknown_file_dialog() else {
            cx.notify();
            return;
        };
        self.open_transfer_external(state.entry, window, cx);
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
        self.open_transfer_editor_direct(state.entry, window, cx);
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

    pub(in crate::features) fn open_active_transfer_editor_external(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.transfer.active_editor_tab().cloned() else {
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

    pub(in crate::features::pages::transfers) fn open_transfer_external_for_session(
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
            match open_local_path_with_editor(&local_path, &default_editor) {
                Ok(()) => {
                    self.transfer
                        .set_browser_status(format!("opened {} externally", entry.path));
                    self.shell
                        .set_status(format!("opened local file {} externally", entry.path));
                }
                Err(error) => self.shell.set_status(error),
            }
            cx.notify();
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
        let local_path = self.transfer_external_open_path(&entry, session_id.as_deref());
        let default_editor = self.settings.summary().transfer_default_editor.clone();
        let transfer_options = self.sftp_transfer_options();
        let id = self.transfer.next_transfer_job_id("sftp-open-external");
        let control = SftpTransferControl::new();
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id,
            kind: TransferJobKind::OpenExternal {
                remote_path: remote_path.clone(),
                local_path: local_path.clone(),
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
        self.submit_transfer_blocking_job(
            "sftp-open-external",
            id.clone(),
            finished_tx.clone(),
            move || {
                let progress_id = id.clone();
                let result = service
                    .download_remote_file_with_progress_and_control_options(
                        &remote_file_path,
                        local_path.clone(),
                        control,
                        transfer_options,
                        move |progress| {
                            let _ = progress_tx.unbounded_send(TransferJobResult {
                                id: progress_id.clone(),
                                event: TransferJobEvent::Progress(progress),
                            });
                        },
                    )
                    .map_err(|error| error.to_string())
                    .and_then(|_| {
                        open_local_path_with_editor(&local_path, &default_editor).map(|_| {
                            TransferJobOutput::ExternalOpened {
                                remote_path: remote_path.clone(),
                                raw_path_token: remote_file_path.raw_path_token.clone(),
                                local_path: local_path.clone(),
                            }
                        })
                    });
                let _ = finished_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn transfer_external_open_path(
        &self,
        entry: &SftpFileEntry,
        session_id: Option<&str>,
    ) -> PathBuf {
        let session_id = session_id
            .map(sanitize_local_open_segment)
            .unwrap_or_else(|| "session".to_string());
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let fallback_name;
        let raw_file_name = if entry.name.trim().is_empty() {
            fallback_name = remote_file_name(&entry.path);
            fallback_name.as_str()
        } else {
            entry.name.as_str()
        };
        let file_name = sanitize_local_open_segment(raw_file_name);
        std::env::temp_dir()
            .join("nyaterm")
            .join(session_id)
            .join(timestamp_ms.to_string())
            .join(file_name)
    }
}
