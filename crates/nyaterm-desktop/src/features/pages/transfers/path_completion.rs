use crate::features::NyaTermApp;
use crate::features::transfers::file_operations::FileOperationEndpoint;
use gpui::{Context, Window};
use nyaterm_file_browser_gpui::completion::CompletionStatus;
use nyaterm_files::sftp::RemoteFilePath;
use std::time::Duration;

impl NyaTermApp {
    pub(in crate::features) fn validate_transfer_path_completion(&mut self) {
        let source = self.transfer.browser_view().completion.source.clone();
        if let Some((endpoint, access, _)) = source
            && (!self.transfer.browser_view().path_editing
                || endpoint.session_id() != self.session.active_id()
                || endpoint
                    .session_id()
                    .is_some_and(|id| self.session.is_disconnected(id))
                || endpoint
                    .session_id()
                    .and_then(|id| self.file_access_generation(id))
                    != access)
        {
            self.transfer.path_completion_mut().reset();
        }
    }

    pub(in crate::features) fn refresh_transfer_path_completion(&mut self, cx: &mut Context<Self>) {
        self.validate_transfer_path_completion();
        let browser = self.transfer.browser_view();
        if !browser.path_editing || !browser.completion.edited || browser.completion.suppressed {
            return;
        }
        let Some(field) = self.existing_text_input("transfer.browser.path") else {
            return;
        };
        if !field.read(cx).selection_at_end(cx) {
            return;
        }
        let value = field.read(cx).value(cx);
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        if self.session.is_disconnected(&session) {
            return;
        }
        let endpoint = FileOperationEndpoint::Session(session.clone());
        let current = RemoteFilePath {
            display_path: browser.path.clone(),
            raw_path_token: browser.raw_path_token.clone(),
        };
        let home = browser.home_dir.clone();
        let seed_entries =
            (!browser.loading && browser.error.is_none() && !browser.entries.is_empty())
                .then(|| browser.entries.clone());
        let pending = browser.home_dir_pending;
        let anchor = browser.completion.anchor.clone();
        let generation = self.transfer.path_completion_mut().prepare();
        let service = self.file_browser_service_for_endpoint(&endpoint);
        let query = service.as_ref().ok().and_then(|s| {
            s.completion_query(&value, &current, &home, anchor.as_ref())
                .ok()
        });
        let (Ok(service), Some(query)) = (service, query) else {
            self.transfer.path_completion_mut().presentation.status =
                if pending && value.starts_with('~') {
                    CompletionStatus::WaitingHome
                } else {
                    CompletionStatus::Unavailable
                };
            self.defer_transfer_panel_snapshot_flush(cx);
            cx.notify();
            return;
        };
        let access = self.file_access_generation(&session);
        let state = self.transfer.path_completion_mut();
        let source = (endpoint, access, 0);
        if state.source.as_ref() != Some(&source) {
            state.worker.invalidate();
            if let Some(entries) = seed_entries {
                state.worker.seed(current, &entries);
            }
            state.source = Some(source);
        }
        let worker = state.worker.clone();
        let jobs = self.blocking_jobs.clone();
        let hidden = self.settings.summary().ui_file_explorer_show_hidden_files;
        let task = cx.spawn(async move |app, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let result = worker.request(service, query.clone(), hidden, jobs).await;
            let _ = app.update(cx, |app, cx| {
                if app.session.active_id() != Some(session.as_str())
                    || app.session.is_disconnected(&session)
                    || app.file_access_generation(&session) != access
                    || !app.transfer.browser_view().path_editing
                {
                    return;
                }
                if app
                    .transfer
                    .path_completion_mut()
                    .apply(generation, &query, result, hidden)
                {
                    app.defer_transfer_panel_snapshot_flush(cx);
                    cx.notify();
                }
            });
        });
        self.transfer.path_completion_mut().task = Some(task);
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn accept_transfer_path_candidate(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.validate_transfer_path_completion();
        let Some(field) = self.existing_text_input("transfer.browser.path") else {
            return false;
        };
        if !self.transfer.browser_view().path_editing
            || !field.read(cx).selection_at_end(cx)
            || field.update(cx, |input, cx| input.is_composing(window, cx))
        {
            return false;
        }
        let Some(text) = self.transfer.path_completion_mut().accept(index) else {
            return false;
        };
        self.transfer.update_browser_path_draft(text.clone());
        field.update(cx, |input, cx| {
            input.set_content_silent(&text, cx);
            input.focus_with_cursor_at_end(window, cx);
        });
        self.refresh_transfer_path_completion(cx);
        true
    }

    pub(in crate::features) fn escape_transfer_path_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transfer.path_completion_mut().hide() {
            self.cancel_transfer_browser_path_edit(window, cx);
        }
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn submit_completed_transfer_path(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.validate_transfer_path_completion();
        let Some(field) = self.existing_text_input("transfer.browser.path") else {
            return;
        };
        if !self.transfer.browser_view().path_editing
            || field.update(cx, |input, cx| input.is_composing(window, cx))
        {
            return;
        }
        let browser = self.transfer.browser_view();
        // The draft can be updated before a queued input replacement is painted.
        // Submission must preserve that manual-navigation intent.
        let value = browser.path_draft.clone();
        let selected = (field.read(cx).value(cx) == value && field.read(cx).selection_at_end(cx))
            .then(|| browser.completion.selected_path())
            .flatten();
        if selected.is_none() && value.trim().is_empty() {
            return;
        }
        let current = RemoteFilePath {
            display_path: browser.path.clone(),
            raw_path_token: browser.raw_path_token.clone(),
        };
        let home = browser.home_dir.clone();
        let anchor = browser.completion.anchor.clone();
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        let path = selected.map(Ok).unwrap_or_else(|| {
            self.file_browser_service_for_endpoint(&FileOperationEndpoint::Session(session))
                .and_then(|service| {
                    service.resolve_browser_input(&value, &current, &home, anchor.as_ref())
                })
        });
        match path {
            Ok(path) => {
                self.transfer.finish_browser_path_edit();
                self.forget_text_inputs("transfer.browser.path");
                self.open_transfer_browser_directory_path(path, cx);
                self.restore_transfer_browser_focus(window, cx);
            }
            Err(_) => {
                self.transfer.path_completion_mut().presentation.status =
                    CompletionStatus::Unavailable;
            }
        }
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
    }
}
