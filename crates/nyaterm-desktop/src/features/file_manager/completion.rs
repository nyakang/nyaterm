use super::pane::PaneId;
use super::workspace::FileManagerWorkspace;
use crate::features::NyaTermApp;
use gpui::{Context, Window};
use nyaterm_file_browser_gpui::completion::CompletionStatus;
use std::time::Duration;

impl FileManagerWorkspace {
    pub(super) fn refresh_path_completion(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let state = self.pane(pane);
        if !state.path_editing || !state.completion.edited || state.completion.suppressed {
            return;
        }
        let input = self.inputs(pane).path.read(cx);
        if !input.selection_at_end(cx) {
            return;
        }
        let value = input.value(cx);
        let generation = self.pane_mut(pane).completion.prepare();
        let id = self.id.clone();
        let app = self.app.clone();
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| {
                app.query_file_manager_path(id, pane, generation, value, cx)
            });
        });
        cx.notify();
    }

    pub(super) fn accept_path_candidate(
        &mut self,
        pane: PaneId,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let input = self.inputs(pane).path.clone();
        if !self.pane(pane).path_editing
            || !input.read(cx).selection_at_end(cx)
            || input.update(cx, |input, cx| input.is_composing(window, cx))
        {
            return false;
        }
        let Some(text) = self.pane_mut(pane).completion.accept(index) else {
            return false;
        };
        input.update(cx, |input, cx| {
            input.set_content_silent(&text, cx);
            input.focus_with_cursor_at_end(window, cx);
        });
        self.refresh_path_completion(pane, cx);
        true
    }

    pub(super) fn escape_path_completion(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_mut(pane).completion.hide() {
            self.pane_mut(pane).path_editing = false;
            self.pane_mut(pane).completion.reset();
            self.focus_pane(pane, window, cx);
        }
        cx.notify();
    }

    pub(super) fn submit_path_edit(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if !self.pane(pane).path_editing {
            return;
        }
        let input = self.inputs(pane).path.read(cx);
        let value = input.value(cx);
        let selected = input
            .selection_at_end(cx)
            .then(|| self.pane(pane).completion.selected_path())
            .flatten();
        if selected.is_none() && value.trim().is_empty() {
            return;
        }
        let endpoint = self.pane(pane).endpoint.clone();
        let generation = self.pane(pane).generation;
        let access = self.app.upgrade().and_then(|app| {
            endpoint
                .session_id()
                .and_then(|id| app.read(cx).file_access_generation(id))
        });
        let edit_generation = self.pane(pane).completion.generation;
        let path = self.pane(pane).path.clone();
        let home = self.pane(pane).home_dir.clone();
        let anchor = self.pane(pane).completion.anchor.clone();
        let id = self.id.clone();
        let app = self.app.clone();
        let workspace = cx.weak_entity();
        cx.defer(move |cx| {
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            let pane_state = workspace.read(cx).pane(pane);
            if !pane_state.path_editing
                || pane_state.endpoint != endpoint
                || pane_state.generation != generation
                || pane_state.completion.generation != edit_generation
            {
                return;
            }
            let _ = app.update(cx, |app, cx| {
                if endpoint
                    .session_id()
                    .and_then(|id| app.file_access_generation(id))
                    != access
                {
                    return;
                }
                let resolved = selected.map(Ok).unwrap_or_else(|| {
                    app.file_browser_service_for_endpoint(&endpoint)
                        .and_then(|service| {
                            service.resolve_browser_input(&value, &path, &home, anchor.as_ref())
                        })
                });
                match resolved {
                    Ok(path) => {
                        workspace.update(cx, |workspace, _| {
                            workspace.focus_pending = true;
                        });
                        app.navigate_file_manager_pane(id, pane, path, cx);
                    }
                    Err(_) => workspace.update(cx, |workspace, cx| {
                        workspace.pane_mut(pane).completion.presentation.status =
                            CompletionStatus::Unavailable;
                        cx.notify();
                    }),
                }
            });
        });
    }
}

impl NyaTermApp {
    fn query_file_manager_path(
        &mut self,
        manager: String,
        pane_id: PaneId,
        edit_generation: u64,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&manager) else {
            return;
        };
        let pane = workspace.read(cx).pane(pane_id);
        if !pane.path_editing || pane.completion.generation != edit_generation {
            return;
        }
        let endpoint = pane.endpoint.clone();
        let generation = pane.generation;
        if endpoint
            .session_id()
            .is_some_and(|id| self.session.is_disconnected(id))
        {
            workspace.update(cx, |workspace, cx| {
                workspace.pane_mut(pane_id).completion.presentation.status =
                    CompletionStatus::Unavailable;
                cx.notify();
            });
            return;
        }
        let home_pending = pane.home_dir_pending;
        let service = self.file_browser_service_for_endpoint(&endpoint);
        let query = service.as_ref().ok().and_then(|service| {
            service
                .completion_query(
                    &value,
                    &pane.path,
                    &pane.home_dir,
                    pane.completion.anchor.as_ref(),
                )
                .ok()
        });
        let (Ok(service), Some(query)) = (service, query) else {
            workspace.update(cx, |workspace, cx| {
                workspace.pane_mut(pane_id).completion.presentation.status =
                    if home_pending && value.starts_with('~') {
                        CompletionStatus::WaitingHome
                    } else {
                        CompletionStatus::Unavailable
                    };
                cx.notify();
            });
            return;
        };
        // Constructing the first SSH service registers its access generation.
        let access = endpoint
            .session_id()
            .and_then(|id| self.file_access_generation(id));
        let source = (endpoint.clone(), access, generation);
        let seed_path = pane.path.clone();
        let seed_entries = (!pane.loading && pane.error.is_none() && !pane.entries.is_empty())
            .then(|| pane.entries.clone());
        let worker = workspace.update(cx, |workspace, _| {
            let state = &mut workspace.pane_mut(pane_id).completion;
            if state.source.as_ref() != Some(&source) {
                state.worker.invalidate();
                if let Some(entries) = seed_entries {
                    state.worker.seed(seed_path, &entries);
                }
                state.source = Some(source);
            }
            state.worker.clone()
        });
        let jobs = self.blocking_jobs.clone();
        let hidden = self.settings.summary().ui_file_explorer_show_hidden_files;
        let weak = workspace.downgrade();
        let task = cx.spawn(async move |app, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let result = worker.request(service, query.clone(), hidden, jobs).await;
            let _ = app.update(cx, |app, cx| {
                if endpoint
                    .session_id()
                    .and_then(|id| app.file_access_generation(id))
                    != access
                    || endpoint
                        .session_id()
                        .is_some_and(|id| app.session.is_disconnected(id))
                {
                    return;
                }
                let Some(workspace) = weak.upgrade() else {
                    return;
                };
                workspace.update(cx, |workspace, cx| {
                    let pane = workspace.pane_mut(pane_id);
                    if pane.path_editing
                        && pane.endpoint == endpoint
                        && pane.generation == generation
                        && pane
                            .completion
                            .apply(edit_generation, &query, result, hidden)
                    {
                        cx.notify();
                    }
                });
            });
        });
        workspace.update(cx, |workspace, _| {
            workspace.pane_mut(pane_id).completion.task = Some(task)
        });
    }
}
