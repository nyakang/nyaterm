use super::pane::PaneId;
use super::workspace::FileManagerWorkspace;
use gpui::{AppContext as _, Context, Entity, Window};
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_ui::{NyaInputEvent, NyaInputState};

pub(super) struct FileManagerPaneInputs {
    pub path: Entity<NyaInputState>,
    pub search: Entity<NyaInputState>,
}

impl FileManagerPaneInputs {
    pub(super) fn new(cx: &mut Context<FileManagerWorkspace>) -> Self {
        Self {
            path: cx.new(|cx| NyaInputState::new(cx, "").submit_on_enter(true)),
            search: cx.new(|cx| {
                NyaInputState::new(cx, "").placeholder(rust_i18n::t!("fileManager.filterName"))
            }),
        }
    }
}

impl FileManagerWorkspace {
    pub(super) fn install_input_subscriptions(&mut self, cx: &mut Context<Self>) {
        for pane in [PaneId::Left, PaneId::Right] {
            let path = self.inputs(pane).path.clone();
            self.input_subscriptions
                .push(cx.subscribe(&path, move |this, _, event, cx| {
                    if let NyaInputEvent::Submitted(value) = event {
                        let endpoint = this.pane(pane).endpoint.clone();
                        let id = this.id.clone();
                        let app = this.app.clone();
                        let workspace = cx.weak_entity();
                        let path = if *value == this.pane(pane).path.display_path {
                            this.pane(pane).path.clone()
                        } else {
                            RemoteFilePath::new(value.clone())
                        };
                        this.focus_pending = true;
                        this.pane_mut(pane).path_editing = false;
                        cx.defer(move |cx| {
                            if let Some(workspace) = workspace.upgrade()
                                && workspace.read(cx).pane(pane).endpoint == endpoint
                            {
                                let _ = app.update(cx, |app, cx| {
                                    app.navigate_file_manager_pane(id, pane, path, cx)
                                });
                            }
                        });
                        cx.notify();
                    }
                }));
            let search = self.inputs(pane).search.clone();
            self.input_subscriptions
                .push(cx.subscribe(&search, move |this, _, event, cx| {
                    if let NyaInputEvent::Changed(value) = event {
                        let state = this.pane_mut(pane);
                        if state.search != *value {
                            state.search = value.clone();
                            state.clear_selection();
                            cx.notify();
                        }
                    }
                }));
        }
    }

    pub(super) fn inputs(&self, pane: PaneId) -> &FileManagerPaneInputs {
        &self.pane_inputs[if pane == PaneId::Left { 0 } else { 1 }]
    }

    pub(super) fn begin_path_edit(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active = pane;
        let path = self.pane(pane).path.display_path.clone();
        self.pane_mut(pane).path_editing = true;
        self.inputs(pane).path.update(cx, |state, cx| {
            state.set_content_silent(&path, cx);
            state.select_all_with_cursor_at_end(window, cx);
        });
        cx.notify();
    }

    pub(super) fn focus_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.active = pane;
        window.focus(&self.pane(pane).focus, cx);
        cx.notify();
    }
}
