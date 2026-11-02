use std::collections::HashMap;

use nyaterm_transport::RemoteTextGeneration;
use nyaterm_ui::document_snapshot::NyaDocumentSnapshot;

use gpui::{AppContext as _, Context, Window, div, prelude::*};
use nyaterm_core::InternalEditorDisplay;
use nyaterm_ui::{NyaButton, NyaDialogWindowExt};
use rust_i18n::t;

use crate::features::{AppLifecycleEvent, NyaTermApp};
use crate::models::{NavItem, WorkspaceTabId};

pub(in crate::features) struct PendingWorkspaceClose {
    targets: Vec<String>,
    approved: HashMap<String, EditorCloseApproval>,
    waiting: Option<String>,
    exit_session_count: Option<usize>,
    prompt_open: bool,
}

pub(in crate::features) struct EditorCloseApproval {
    generation: RemoteTextGeneration,
    content: NyaDocumentSnapshot,
}

impl EditorCloseApproval {
    fn matches(&self, document: &crate::models::TransferEditorState) -> bool {
        self.generation == document.generation && self.content.same_version(&document.content)
    }
}

impl NyaTermApp {
    pub(crate) fn has_unconfirmed_editor_changes(&self) -> bool {
        self.transfer.editor_documents().iter().any(|document| {
            document.saving
                || (document.dirty
                    && !self
                        .shell
                        .workspace
                        .editor_exit_approvals
                        .get(&document.id)
                        .is_some_and(|approval| approval.matches(document)))
        })
    }

    pub(crate) fn cancel_editor_exit_approval(&mut self) {
        self.settings.take_quit_confirmation();
        self.shell.workspace.editor_exit_approvals.clear();
    }

    fn approve_editor_close(&mut self, id: &str) {
        if let Some(document) = self.transfer.editor_tab_snapshot(id)
            && let Some(close) = self.shell.workspace.editor_close.as_mut()
        {
            close.approved.insert(
                id.to_string(),
                EditorCloseApproval {
                    generation: document.generation,
                    content: document.content,
                },
            );
        }
    }

    pub(in crate::features) fn workspace_tab_ids(&self) -> Vec<String> {
        self.shell
            .terminal_window_tree()
            .map(|tree| tree.collect_tab_ids())
            .unwrap_or_else(|| {
                self.ordered_tab_sessions()
                    .into_iter()
                    .map(|session| session.id)
                    .collect()
            })
    }

    pub(in crate::features) fn active_workspace_tab(&self) -> Option<String> {
        self.shell
            .workspace
            .focused_group
            .as_deref()
            .and_then(|group| self.shell.terminal_group_tabs(group))
            .and_then(|(_, active)| active)
            .or_else(|| {
                self.session
                    .active_id()
                    .filter(|id| !self.shell.session_tab_is_hidden(id))
                    .map(|id| self.tab_root_for_session(id))
            })
            .or_else(|| {
                let tree = self.shell.terminal_window_tree_ref()?;
                let group = tree.first_leaf_id()?;
                tree.leaf_tabs(&group)?.1.map(str::to_string)
            })
    }

    pub(in crate::features) fn workspace_tab_session(&self, key: &str) -> Option<String> {
        match WorkspaceTabId::from_layout_key(key) {
            WorkspaceTabId::Session(id) => Some(id),
            WorkspaceTabId::Editor(id) => self.transfer.editor_tab_snapshot(&id)?.session_id,
            WorkspaceTabId::FileManager(_) | WorkspaceTabId::Settings => None,
        }
    }

    pub(in crate::features) fn activate_workspace_tab(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        match WorkspaceTabId::from_layout_key(key) {
            WorkspaceTabId::Session(id) => {
                self.select_session(id, cx);
            }
            WorkspaceTabId::Editor(id) => {
                let Some(document) = self.transfer.editor_tab_snapshot(&id) else {
                    return;
                };
                if document.display == InternalEditorDisplay::Window {
                    self.transfer.activate_editor_tab(&id);
                    self.open_remote_file_editor_window(cx);
                    return;
                }
                // The connection remains contextual state for side panels. It is
                // never the keyboard target while an editor owns the focused tab.
                if let Some(session) = &document.session_id
                    && self.session.has_session(session)
                    && self.session.active_id() != Some(session)
                {
                    let tree = self.shell.terminal_window_tree();
                    let focused = self.shell.workspace.focused_group.clone();
                    self.activate_session_id(session, cx);
                    self.shell.restore_terminal_window_tree(tree);
                    self.shell.workspace.focused_group = focused;
                }
                self.session.clear_active_start_selection();
                self.shell.workspace.focused_group =
                    self.shell.sync_terminal_windows_active_tab(key);
                self.shell.set_pane_focus_mode(false);
                self.shell.navigation.selected_nav = NavItem::Workspace;
                self.shell.request_session_tab_scroll_into_view();
                self.close_action_link_menu(cx);
                let view = self.transfer.workspace_editor_view(&id);
                if let Some(view) = view {
                    view.update(cx, |view, cx| view.request_focus(cx));
                }
                cx.notify();
            }
            WorkspaceTabId::FileManager(id) => self.activate_file_manager_tab(&id, cx),
            WorkspaceTabId::Settings => self.activate_settings_workspace_tab(cx),
        }
    }

    pub(in crate::features) fn open_workspace_editor_tab(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.transfer.editor_tab_snapshot(id) else {
            return;
        };
        let key = WorkspaceTabId::Editor(id.to_string()).layout_key();
        if self.workspace_tab_ids().contains(&key) {
            self.activate_workspace_tab(&key, cx);
            return;
        }
        let current = self.active_workspace_tab();
        let session = document.session_id.as_deref();
        let after = current
            .as_ref()
            .filter(|key| self.workspace_tab_session(key).as_deref() == session)
            .cloned()
            .or_else(|| {
                session
                    .map(|id| self.tab_root_for_session(id))
                    .filter(|key| self.workspace_tab_ids().contains(key))
            })
            .or(current);
        self.ensure_terminal_windows_root();
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            let group = after
                .as_deref()
                .and_then(|key| tree.leaf_for_tab(key))
                .map(str::to_string)
                .or_else(|| tree.first_leaf_id());
            tree.ensure_tab(&key, group.as_deref());
            if let Some(after) = after {
                let before = tree.leaf_tab_ids_for_tab(&after).and_then(|tabs| {
                    tabs.iter()
                        .position(|tab| tab == &after)
                        .and_then(|index| tabs.get(index + 1))
                        .cloned()
                });
                if let Some(before) = before {
                    tree.place_tab_before(&key, &before);
                }
            }
            self.shell.restore_terminal_window_tree(Some(tree));
        }
        self.activate_workspace_tab(&key, cx);
        self.persist_open_tabs();
    }

    pub(in crate::features) fn request_workspace_tabs_close(
        &mut self,
        targets: Vec<String>,
        exit_session_count: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.shell.workspace.editor_close.is_some() {
            return;
        }
        // Settings owns a draft shared with the whole app and has its own
        // confirmed close path; a mass-close must never discard it silently.
        let targets = targets
            .into_iter()
            .filter(|key| !WorkspaceTabId::from_layout_key(key).is_settings())
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return;
        }
        self.shell.workspace.editor_close = Some(PendingWorkspaceClose {
            targets,
            approved: HashMap::new(),
            waiting: None,
            exit_session_count,
            prompt_open: false,
        });
        self.continue_workspace_tabs_close(cx);
    }

    pub(in crate::features) fn cancel_workspace_tabs_close(&mut self, cx: &mut Context<Self>) {
        let quitting = self
            .shell
            .workspace
            .editor_close
            .take()
            .is_some_and(|close| close.exit_session_count.is_some());
        if quitting {
            self.cancel_editor_exit_approval();
            self.update
                .update(cx, |update, _| update.install_requested = false);
            cx.emit(AppLifecycleEvent::ShutdownCancelled);
        }
        self.focus_workspace_editor(cx);
        cx.notify();
    }

    pub(in crate::features) fn continue_workspace_tabs_close(&mut self, cx: &mut Context<Self>) {
        let Some(mut close) = self.shell.workspace.editor_close.take() else {
            return;
        };
        if close.prompt_open {
            self.shell.workspace.editor_close = Some(close);
            return;
        }
        if let Some(id) = close.waiting.take() {
            if let Some(document) = self.transfer.editor_tab_snapshot(&id) {
                if document.saving {
                    close.waiting = Some(id);
                    self.shell.workspace.editor_close = Some(close);
                    return;
                }
                if document.dirty
                    || document.conflict
                    || document.error.is_some()
                    || document.backup_warning_path.is_some()
                {
                    self.shell.workspace.editor_close = Some(close);
                    self.cancel_workspace_tabs_close(cx);
                    self.activate_workspace_tab(&WorkspaceTabId::Editor(id).layout_key(), cx);
                    return;
                }
            }
            if let Some(document) = self.transfer.editor_tab_snapshot(&id) {
                close.approved.insert(
                    id,
                    EditorCloseApproval {
                        generation: document.generation,
                        content: document.content,
                    },
                );
            }
        }
        let next =
            close
                .targets
                .iter()
                .find_map(|key| match WorkspaceTabId::from_layout_key(key) {
                    WorkspaceTabId::Editor(id) => self
                        .transfer
                        .editor_tab_snapshot(&id)
                        .filter(|document| {
                            document.saving
                                || (document.dirty
                                    && !close
                                        .approved
                                        .get(&id)
                                        .is_some_and(|approval| approval.matches(document)))
                        })
                        .map(|_| id),
                    _ => None,
                });
        self.shell.workspace.editor_close = Some(close);
        if let Some(id) = next {
            if self
                .transfer
                .editor_tab_snapshot(&id)
                .is_some_and(|document| document.saving)
            {
                if let Some(close) = self.shell.workspace.editor_close.as_mut() {
                    close.waiting = Some(id);
                }
                return;
            }
            if let Some(close) = self.shell.workspace.editor_close.as_mut() {
                close.prompt_open = true;
            }
            let app = cx.entity();
            let handle = self.shell.main_window();
            if let Some(handle) = handle {
                cx.defer(move |cx| {
                    // Opening a dialog reads the component root. Do not lease
                    // that root through the typed WindowHandle::update here.
                    let _ = cx.update_window(handle.into(), |_, window, cx| {
                        app.update(cx, |app, cx| {
                            app.open_workspace_editor_close_dialog(id, window, cx)
                        });
                    });
                });
            } else {
                self.cancel_workspace_tabs_close(cx);
            }
            return;
        }
        let close = self
            .shell
            .workspace
            .editor_close
            .take()
            .expect("close transaction retained");
        // No layout is removed until every document has been accepted.
        if close.exit_session_count.is_none() {
            for key in &close.targets {
                match WorkspaceTabId::from_layout_key(key) {
                    WorkspaceTabId::Editor(id) => {
                        self.remove_workspace_editor_tab(&id, cx);
                        self.transfer.remove_editor_document(&id);
                    }
                    WorkspaceTabId::FileManager(id) => {
                        self.remove_workspace_file_manager_tab(&id, cx);
                    }
                    WorkspaceTabId::Session(id) => self.close_session(id, cx),
                    // Filtered out in `request_workspace_tabs_close`.
                    WorkspaceTabId::Settings => {}
                }
            }
            self.release_unreferenced_editor_sessions(cx);
            self.reconcile_terminal_windows();
            self.persist_open_tabs();
        }
        if close.exit_session_count.is_some() {
            // Keep documents and connections intact until the entire process has
            // accepted exit. A cancellation in another window must be reversible.
            self.shell.workspace.editor_exit_approvals = close.approved;
            let app = cx.entity();
            if let Some(handle) = self.shell.main_window() {
                cx.defer(move |cx| {
                    let _ = cx.update_window(handle.into(), |_, window, cx| {
                        app.update(cx, |app, cx| {
                            app.handle_window_close_request_with_count(0, window, cx)
                        });
                    });
                });
            }
        }
        self.focus_workspace_editor(cx);
        cx.notify();
    }

    fn open_workspace_editor_close_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.workspace.editor_close.is_none() {
            return;
        }
        window.activate_window();
        let Some(document) = self.transfer.editor_tab_snapshot(&id) else {
            self.continue_workspace_tabs_close(cx);
            return;
        };
        let app = cx.weak_entity();
        let handled = std::rc::Rc::new(std::cell::Cell::new(false));
        window.open_nya_dialog(cx, move |dialog, _, _| {
            let save_app = app.clone();
            let discard_app = app.clone();
            let cancel_app = app.clone();
            let save_id = id.clone();
            let discard_id = id.clone();
            let saved = handled.clone();
            let discarded = handled.clone();
            let cancelled = handled.clone();
            dialog
                .title(t!("fileEditor.unsavedTitle").to_string())
                .width(440.)
                .content(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(document.remote_path.clone())
                        .child(t!("fileEditor.unsavedDesc").to_string())
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    NyaButton::new(
                                        "editor-close-cancel",
                                        t!("common.cancel").to_string(),
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            cancelled.set(true);
                                            window.close_nya_dialog(cx);
                                            let _ = cancel_app.update(cx, |app, cx| {
                                                app.cancel_workspace_tabs_close(cx)
                                            });
                                        },
                                    ),
                                )
                                .child(
                                    NyaButton::new(
                                        "editor-close-discard",
                                        t!("fileEditor.discard").to_string(),
                                    )
                                    .disabled(document.saving)
                                    .on_click(
                                        move |_, window, cx| {
                                            discarded.set(true);
                                            window.close_nya_dialog(cx);
                                            let _ = discard_app.update(cx, |app, cx| {
                                                if let Some(close) =
                                                    app.shell.workspace.editor_close.as_mut()
                                                {
                                                    close.prompt_open = false;
                                                }
                                                app.approve_editor_close(&discard_id);
                                                app.continue_workspace_tabs_close(cx);
                                            });
                                        },
                                    ),
                                )
                                .child(
                                    NyaButton::new(
                                        "editor-close-save",
                                        t!("fileEditor.saveAndClose").to_string(),
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            saved.set(true);
                                            window.close_nya_dialog(cx);
                                            let _ = save_app.update(cx, |app, cx| {
                                                if let Some(close) =
                                                    app.shell.workspace.editor_close.as_mut()
                                                {
                                                    close.prompt_open = false;
                                                    close.waiting = Some(save_id.clone());
                                                }
                                                app.save_transfer_editor_tab(
                                                    &save_id, false, window, cx,
                                                );
                                                app.continue_workspace_tabs_close(cx);
                                            });
                                        },
                                    ),
                                ),
                        ),
                )
                .on_close({
                    let app = app.clone();
                    let handled = handled.clone();
                    move |_, _, cx| {
                        if !handled.get() {
                            let _ = app.update(cx, |app, cx| app.cancel_workspace_tabs_close(cx));
                        }
                    }
                })
        });
    }

    pub(in crate::features) fn remove_workspace_editor_tab(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = WorkspaceTabId::Editor(id.to_string()).layout_key();
        let Some(mut tree) = self.shell.terminal_window_tree() else {
            return;
        };
        let current = self.active_workspace_tab();
        let session = self.workspace_tab_session(&key);
        let preferred = tree.leaf_tab_ids_for_tab(&key).and_then(|tabs| {
            let index = tabs.iter().position(|tab| tab == &key)?;
            tabs[..index]
                .iter()
                .rev()
                .chain(tabs[index + 1..].iter())
                .find(|tab| self.workspace_tab_session(tab) == session)
                .cloned()
        });
        let next = tree.remove_tab(&key);
        self.shell.restore_terminal_window_tree(next);
        if current.as_deref() == Some(&key) {
            let next = preferred
                .or_else(|| {
                    self.shell
                        .workspace
                        .focused_group
                        .as_deref()
                        .and_then(|group| self.shell.terminal_group_tabs(group))
                        .and_then(|(_, active)| active)
                })
                .or_else(|| self.workspace_tab_ids().first().cloned());
            if let Some(next) = next {
                self.activate_workspace_tab(&next, cx);
            }
        }
    }

    pub(in crate::features) fn hide_referenced_session_tab(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let sessions = self.tab_tree_session_ids(id);
        if !sessions
            .iter()
            .any(|id| self.workspace_references_session(id, cx))
        {
            return false;
        }
        if self.tab_tree_is_locked(id) {
            self.notify_locked_tab_close_blocked(cx);
            return true;
        }
        let active = self.active_workspace_tab();
        for session in &sessions {
            self.shell.workspace.hidden_sessions.insert(session.clone());
        }
        self.shell.workspace.pane_roots.remove(id);
        self.shell
            .workspace
            .tab_owner
            .retain(|session, owner| !sessions.contains(session) && owner != id);
        self.sync_workspace_split_from_active_tab();
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            let next = tree.remove_tab(id);
            self.shell.restore_terminal_window_tree(next);
        }
        self.reconcile_terminal_windows();
        if active.as_deref() == Some(id)
            && let Some(next) = self.workspace_tab_ids().first().cloned()
        {
            self.activate_workspace_tab(&next, cx);
        }
        self.release_unreferenced_hidden_sessions(cx);
        self.persist_open_tabs();
        cx.notify();
        true
    }

    pub(in crate::features) fn release_unreferenced_hidden_sessions(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.release_unreferenced_editor_sessions(cx);
    }

    pub(in crate::features) fn release_unreferenced_editor_sessions(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let released = self
            .shell
            .workspace
            .hidden_sessions
            .iter()
            .filter(|id| {
                !self.workspace_references_session(id, cx)
                    && !self.transfer.transfer_jobs().iter().any(|job| {
                        job.session_id.as_ref() == Some(*id)
                            && matches!(
                                job.status,
                                crate::models::TransferJobStatus::Running
                                    | crate::models::TransferJobStatus::Paused
                                    | crate::models::TransferJobStatus::Cancelling
                            )
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        for id in released {
            self.close_session(id.clone(), cx);
            if !self.session.has_session(&id) {
                self.shell.workspace.hidden_sessions.remove(&id);
            }
        }
    }

    pub(in crate::features) fn focus_workspace_editor(&self, cx: &mut Context<Self>) {
        let Some(key) = self.active_workspace_tab() else {
            return;
        };
        if let WorkspaceTabId::Editor(id) = WorkspaceTabId::from_layout_key(&key)
            && let Some(view) = self.transfer.workspace_editor_view(&id)
        {
            view.update(cx, |view, cx| view.request_focus(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, Entity, IntoElement, Modifiers, Render, TestAppContext, Window, div, prelude::*,
    };
    use nyaterm_core::{InternalEditorDisplay, test_support::TestTempDir};
    use nyaterm_transport::RemoteTextWriteResult;
    use nyaterm_ui::NyaDialogWindowExt as _;

    use crate::features::NyaTermApp;
    use crate::features::test_support::{
        app_with_visible_local_session, editor_document as document,
    };
    use crate::models::{SmartSplitMode, TabDockEdge, TabDockZone, WorkspaceTabId};

    struct EditorCloseHost {
        app: Entity<NyaTermApp>,
        display: InternalEditorDisplay,
    }

    impl Render for EditorCloseHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(self.app.update(cx, |app, cx| match self.display {
                    InternalEditorDisplay::Workspace => app.workspace_view(cx).into_any_element(),
                    InternalEditorDisplay::Window => app.transfer_editor_window_view(cx),
                }))
        }
    }

    #[test]
    fn clicking_dirty_editor_close_shows_confirmation_and_cancel_keeps_document() {
        for display in [
            InternalEditorDisplay::Workspace,
            InternalEditorDisplay::Window,
        ] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let root = TestTempDir::new("nyaterm-editor-close-confirmation");
            let app = app_with_visible_local_session(&mut cx, root.path(), "local");
            let id = app.update(&mut cx, |app, cx| {
                app.sync_component_theme(cx);
                let mut doc = document("local", "/dirty.txt", display);
                doc.id = "confirmation-document".to_string();
                let id = doc.id.clone();
                app.transfer.open_editor_tab(doc);
                app.transfer
                    .sync_editor_content(&id, "unsaved editor content");
                if display == InternalEditorDisplay::Workspace {
                    app.open_workspace_editor_tab(&id, cx);
                }
                id
            });
            let (_, cx) = cx.add_window_view(|window, cx| {
                let host = cx.new(|cx| {
                    cx.observe(&app, |_, _, cx| cx.notify()).detach();
                    EditorCloseHost {
                        app: app.clone(),
                        display,
                    }
                });
                nyaterm_ui::nya_root(host, window, cx)
            });
            let handle = cx.update(|window, _| window.window_handle().downcast().unwrap());
            app.update(cx, |app, _| app.shell.remember_main_window(handle));
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let selector = match display {
                InternalEditorDisplay::Workspace => "editor-tab-close-confirmation-document",
                InternalEditorDisplay::Window => "editor-window-tab-close-confirmation-document",
            };
            let close = cx
                .debug_bounds(selector)
                .expect("editor close button is rendered");
            cx.simulate_mouse_move(close.center(), None, Modifiers::default());
            cx.simulate_click(close.center(), Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                assert!(window.has_active_nya_dialog(cx));
            });
            app.read_with(cx, |app, _| {
                assert!(app.shell.workspace.editor_close.is_some());
                assert!(app.transfer.editor_tab_snapshot(&id).unwrap().dirty);
            });
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
            app.read_with(cx, |app, _| {
                assert!(app.shell.workspace.editor_close.is_none());
                let document = app.transfer.editor_tab_snapshot(&id).unwrap();
                assert!(document.dirty);
                assert_eq!(document.content.as_str(), "unsaved editor content");
            });
        }
    }

    #[test]
    fn workspace_files_share_tabs_without_becoming_terminal_sessions() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-workspace-editor-tabs");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            let first = document("local", "/first.txt", InternalEditorDisplay::Workspace);
            let second = document("local", "/second.txt", InternalEditorDisplay::Workspace);
            let first_id = first.id.clone();
            let second_id = second.id.clone();
            let first_key = WorkspaceTabId::Editor(first_id.clone()).layout_key();
            let second_key = WorkspaceTabId::Editor(second_id.clone()).layout_key();
            app.transfer.open_editor_tab(first);
            app.open_workspace_editor_tab(&first_id, cx);
            app.transfer.open_editor_tab(second);
            app.open_workspace_editor_tab(&second_id, cx);
            assert_eq!(
                app.workspace_tab_ids(),
                vec!["local".to_string(), first_key.clone(), second_key.clone()]
            );
            assert_eq!(app.active_workspace_tab(), Some(second_key.clone()));
            assert_eq!(app.session.session_order().len(), 1);
            assert!(app.visible_terminal_session_ids().is_empty());
            app.select_relative_terminal_group_tab(-1, cx);
            assert_eq!(app.active_workspace_tab(), Some(first_key.clone()));
            app.select_terminal_group_tab(0, cx);
            assert_eq!(app.active_workspace_tab().as_deref(), Some("local"));
            app.activate_workspace_tab(&second_key, cx);
            app.request_workspace_tabs_close(vec![second_key], None, cx);
            assert_eq!(app.active_workspace_tab(), Some(first_key));
            assert!(app.transfer.editor_tab_snapshot(&second_id).is_none());
        });
    }

    #[test]
    fn workspace_reconcile_preserves_file_docking_and_omits_files_from_saved_layout() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-workspace-editor-docking");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            let file = document("local", "/file.txt", InternalEditorDisplay::Workspace);
            let id = file.id.clone(); let key = WorkspaceTabId::Editor(id.clone()).layout_key();
            app.transfer.open_editor_tab(file); app.open_workspace_editor_tab(&id, cx);
            let group = app.current_terminal_group().unwrap();
            app.dock_tab_on_terminal_window_leaf(key.clone(), group, TabDockZone::Edge(TabDockEdge::Right), cx);
            let docked = app.shell.terminal_group_for_tab(&key).unwrap();
            app.reconcile_terminal_windows();
            assert_eq!(app.shell.terminal_group_for_tab(&key), Some(docked));
            assert!(app.terminal_windows_is_multi_leaf());
            assert!(matches!(app.shell.serialize_terminal_window_layout(&["local".to_string()]),
                Some(nyaterm_core::RestorableTerminalWindowNode::Leaf { tab_indexes, active_tab_index: Some(0) }) if tab_indexes == vec![0]));
            app.apply_smart_split(SmartSplitMode::Horizontal, cx);
            assert_eq!(app.active_workspace_tab(), Some(key.clone()));
            assert!(app.workspace_tab_ids().contains(&key));
            app.unsplit_workspace(cx);
            assert_eq!(app.active_workspace_tab(), Some(key.clone()));
            app.request_workspace_tabs_close(vec![key], None, cx);
            assert!(!app.terminal_windows_is_multi_leaf());
        });
    }

    #[test]
    fn closing_source_tab_preserves_file_service_and_never_restores_the_hidden_tab() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-workspace-editor-session-reference");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            let file = document("local", "/file.txt", InternalEditorDisplay::Workspace);
            let id = file.id.clone();
            let key = WorkspaceTabId::Editor(id.clone()).layout_key();
            app.transfer.open_editor_tab(file);
            app.open_workspace_editor_tab(&id, cx);
            app.close_session("local".to_string(), cx);
            assert!(app.session.has_session("local"));
            assert!(app.file_browser_service_for_session("local").is_ok());
            for _ in 0..3 {
                app.reconcile_terminal_windows();
            }
            assert_eq!(app.workspace_tab_ids(), vec![key]);
            assert!(app.ordered_tab_sessions().is_empty());
            assert!(app.serialize_open_tabs().is_empty());
            app.request_workspace_tabs_close(app.workspace_tab_ids(), None, cx);
            assert!(!app.session.has_session("local"));
            assert!(app.workspace_tab_ids().is_empty());
            app.release_unreferenced_editor_sessions(cx);
            assert!(app.workspace_tab_ids().is_empty());
        });
    }

    #[test]
    fn window_selection_and_saving_are_independent_of_workspace_files() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-workspace-editor-document-targets");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let window = document("local", "/window.txt", InternalEditorDisplay::Window);
            let workspace = document("local", "/workspace.txt", InternalEditorDisplay::Workspace);
            let window_id = window.id.clone();
            let workspace_id = workspace.id.clone();
            let generation = workspace.generation;
            app.transfer.open_editor_tab(window);
            app.transfer.open_editor_tab(workspace);
            app.open_workspace_editor_tab(&workspace_id, cx);
            app.transfer
                .sync_editor_content(&window_id, "window changes");
            app.transfer
                .sync_editor_content(&workspace_id, "workspace changes");
            assert_eq!(app.transfer.active_editor_tab().unwrap().id, window_id);
            assert_eq!(
                app.transfer.window_editor_workspace().unwrap().tabs.len(),
                1
            );
            app.transfer.begin_editor_tab_save(&workspace_id);
            app.transfer.complete_editor_save_tab(
                &workspace_id,
                generation,
                RemoteTextWriteResult::Conflict,
            );
            assert!(
                app.transfer
                    .editor_tab_snapshot(&workspace_id)
                    .unwrap()
                    .conflict
            );
            assert!(
                !app.transfer
                    .editor_tab_snapshot(&window_id)
                    .unwrap()
                    .conflict
            );
            assert_eq!(
                app.transfer
                    .editor_tab_snapshot(&window_id)
                    .unwrap()
                    .content
                    .as_str(),
                "window changes"
            );
        });
    }

    #[test]
    fn cancelled_bulk_close_keeps_clean_and_dirty_tabs_in_place() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-workspace-editor-close-transaction");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            let clean = document("local", "/clean.txt", InternalEditorDisplay::Workspace);
            let mut dirty = document("local", "/dirty.txt", InternalEditorDisplay::Workspace);
            dirty.dirty = true;
            for file in [clean, dirty] {
                let id = file.id.clone();
                app.transfer.open_editor_tab(file);
                app.open_workspace_editor_tab(&id, cx);
            }
            let before = app.shell.terminal_window_tree();
            // No native main window: the close gate cancels rather than discarding data.
            app.request_workspace_tabs_close(app.workspace_tab_ids(), None, cx);
            assert_eq!(app.shell.terminal_window_tree(), before);
            assert_eq!(app.transfer.editor_documents().len(), 2);
        });
    }
    #[test]
    fn exit_discard_approval_keeps_documents_and_is_invalidated_by_edits_or_cancel() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-exit-approval");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut doc = document("local", "/a.txt", InternalEditorDisplay::Workspace);
            doc.dirty = true;
            let id = doc.id.clone();
            let key = WorkspaceTabId::Editor(id.clone()).layout_key();
            app.transfer.open_editor_tab(doc);
            app.open_workspace_editor_tab(&id, cx);
            let tree = app.shell.terminal_window_tree();
            app.shell.workspace.editor_close = Some(super::PendingWorkspaceClose {
                targets: vec![key],
                approved: Default::default(),
                waiting: None,
                exit_session_count: Some(1),
                prompt_open: false,
            });
            app.approve_editor_close(&id);
            app.continue_workspace_tabs_close(cx);
            assert!(!app.has_unconfirmed_editor_changes());
            assert_eq!(app.shell.terminal_window_tree(), tree);
            assert!(app.session.has_session("local"));
            assert!(app.transfer.editor_tab_snapshot(&id).unwrap().dirty);
            app.transfer.sync_editor_content(&id, "later edit");
            assert!(app.has_unconfirmed_editor_changes());
            app.cancel_editor_exit_approval();
            assert!(app.has_unconfirmed_editor_changes());
        });
    }

    #[test]
    fn closing_during_save_waits_and_a_failed_save_keeps_the_layout() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-close-save");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut doc = document("local", "/a.txt", InternalEditorDisplay::Workspace);
            doc.dirty = true;
            let id = doc.id.clone();
            let generation = doc.generation;
            app.transfer.open_editor_tab(doc);
            app.open_workspace_editor_tab(&id, cx);
            app.transfer.begin_editor_tab_save(&id);
            let tree = app.shell.terminal_window_tree();
            app.request_workspace_tabs_close(app.workspace_tab_ids(), None, cx);
            assert!(app.shell.workspace.editor_close.is_some());
            app.transfer
                .fail_editor_operation_tab(&id, generation, "failed".to_string());
            app.continue_workspace_tabs_close(cx);
            assert!(app.shell.workspace.editor_close.is_none());
            assert_eq!(app.shell.terminal_window_tree(), tree);
            assert!(app.transfer.editor_tab_snapshot(&id).unwrap().dirty);
        });
    }

    #[test]
    fn focusing_files_from_different_sources_preserves_each_groups_active_document() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-multiple-groups");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let metadata = app.session.metadata("local").unwrap().clone();
            app.register_session("other", metadata);
            app.terminal
                .seed_session_view("other".to_string(), String::new(), "UTF-8");
            let first = document("local", "/file.txt", InternalEditorDisplay::Workspace);
            let second = document("other", "/file.txt", InternalEditorDisplay::Workspace);
            let first_id = first.id.clone();
            let second_id = second.id.clone();
            let first_key = WorkspaceTabId::Editor(first_id.clone()).layout_key();
            let second_key = WorkspaceTabId::Editor(second_id.clone()).layout_key();
            app.transfer.open_editor_tab(first);
            app.open_workspace_editor_tab(&first_id, cx);
            let group = app.current_terminal_group().unwrap();
            app.dock_tab_on_terminal_window_leaf(
                first_key.clone(),
                group,
                TabDockZone::Edge(TabDockEdge::Right),
                cx,
            );
            let first_group = app.current_terminal_group().unwrap();
            app.transfer.open_editor_tab(second);
            app.open_workspace_editor_tab(&second_id, cx);
            let second_group = app.current_terminal_group().unwrap();
            assert_ne!(first_group, second_group);
            for key in [&first_key, &second_key] {
                app.activate_workspace_tab(key, cx);
                assert_eq!(
                    app.shell.terminal_group_tabs(&first_group).unwrap().1,
                    Some(first_key.clone())
                );
                assert_eq!(
                    app.shell.terminal_group_tabs(&second_group).unwrap().1,
                    Some(second_key.clone())
                );
            }
            let before = app.workspace_tab_ids();
            app.open_workspace_editor_tab(&first_id, cx);
            assert_eq!(app.workspace_tab_ids(), before);
            assert!(!app.send_terminal_input(b"must not reach terminal".to_vec(), cx));
        });
    }
    #[test]
    fn closing_last_loading_file_does_not_reveal_source_while_job_finishes() {
        use crate::models::{TransferJobKind, TransferJobState, TransferJobStatus};
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-hidden-source-job");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut file = document("local", "/loading.txt", InternalEditorDisplay::Workspace);
            file.loading = true;
            let id = file.id.clone();
            app.transfer.open_editor_tab(file);
            app.open_workspace_editor_tab(&id, cx);
            app.transfer.enqueue_transfer_job(TransferJobState {
                id: "loading-job".into(),
                session_id: Some("local".into()),
                kind: TransferJobKind::Download {
                    remote_path: "/loading.txt".into(),
                    raw_path_token: None,
                    local_path: root.path().join("loading.txt"),
                },
                status: TransferJobStatus::Running,
                detail: String::new(),
                created_at_ms: TransferJobState::now_ms(),
                display_name: String::new(),
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: None,
                speed: Default::default(),
            });
            app.close_session("local".into(), cx);
            app.shell.workspace.focused_group = None;
            assert_eq!(
                app.active_workspace_tab(),
                Some(WorkspaceTabId::Editor(id).layout_key())
            );
            app.request_workspace_tabs_close(app.workspace_tab_ids(), None, cx);
            assert!(app.session.has_session("local"));
            assert!(app.workspace_tab_ids().is_empty());
            assert!(app.active_workspace_tab().is_none());
            assert!(app.visible_terminal_session_ids().is_empty());
            app.shell.set_pane_focus_mode(true);
            assert!(app.visible_terminal_session_ids().is_empty());
            app.transfer.transfer_job_mut("loading-job").unwrap().status =
                TransferJobStatus::Completed;
            app.release_unreferenced_editor_sessions(cx);
            app.release_unreferenced_editor_sessions(cx);
            assert!(!app.session.has_session("local"));
        });
    }

    #[test]
    fn closing_referenced_legacy_pane_keeps_connection_and_document() {
        use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-legacy-pane-close");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let metadata = app.session.metadata("local").unwrap().clone();
            app.register_session("other", metadata);
            app.shell.insert_workspace_pane_root(
                "local".into(),
                WorkspacePaneNode::Split {
                    id: "legacy".into(),
                    direction: WorkspaceSplitDirection::Horizontal,
                    ratio_percent: 50,
                    first: Box::new(WorkspacePaneNode::leaf("local")),
                    second: Box::new(WorkspacePaneNode::leaf("other")),
                },
            );
            app.activate_session_id("local", cx);
            let file = document("local", "/file.txt", InternalEditorDisplay::Window);
            let id = file.id.clone();
            app.transfer.open_editor_tab(file);
            app.close_tab_active_pane("local", cx);
            assert!(app.session.has_session("local"));
            assert!(app.transfer.editor_tab_snapshot(&id).is_some());
            assert!(app.shell.session_tab_is_hidden("local"));
            assert_eq!(
                app.ordered_tab_sessions()
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>(),
                ["other"]
            );
            assert_eq!(app.session.active_id(), Some("other"));
        });
    }
    #[test]
    fn closing_session_activates_adjacent_file_instead_of_its_source_terminal() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-close-neighbor-session");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let metadata = app.session.metadata("local").unwrap().clone();
            app.register_session("other", metadata);
            let file = document("local", "/file.txt", InternalEditorDisplay::Workspace);
            let id = file.id.clone();
            let key = WorkspaceTabId::Editor(id.clone()).layout_key();
            app.transfer.open_editor_tab(file);
            app.open_workspace_editor_tab(&id, cx);
            app.activate_workspace_tab("other", cx);
            app.close_session("other".into(), cx);
            assert_eq!(app.active_workspace_tab(), Some(key));
            assert!(app.visible_terminal_session_ids().is_empty());
        });
    }

    #[test]
    fn file_manager_tabs_stay_out_of_terminal_apis_and_open_tab_snapshots() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-file-manager-tabs");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            let serialized_before = app.serialize_open_tabs().len();
            app.open_file_manager_tab(None, cx);
            let id = app.file_manager.active_id().unwrap().to_string();
            let key = WorkspaceTabId::FileManager(id.clone()).layout_key();
            assert!(app.workspace_tab_ids().contains(&key));
            assert!(app.workspace_tab_session(&key).is_none());
            assert!(!WorkspaceTabId::from_layout_key(&key).is_terminal_session());
            assert_eq!(app.serialize_open_tabs().len(), serialized_before);
            app.transfer.track_file_operation(
                "fm-job".into(),
                crate::features::transfers::file_operations::FileOperationContext {
                    request: crate::features::transfers::file_operations::FileOperationRequest::Mutation(
                        crate::features::transfers::file_operations::FileMutationIntent {
                            endpoint: crate::features::file_manager::PaneEndpoint::Local,
                            directory: nyaterm_transport::RemoteFilePath::new("/tmp"),
                            mutation: crate::features::transfers::file_operations::FileMutation::CreateDirectory("out".into()),
                        }),
                    source_label: "local".into(), target_label: "local".into(),
                },
            );
            app.remove_workspace_file_manager_tab(&id, cx);
            assert!(!app.workspace_tab_ids().contains(&key));
            assert!(app.transfer.file_operation("fm-job").is_some());
        });
    }

    #[test]
    fn file_manager_session_references_hide_then_release_and_follow_id_changes() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-file-manager-refs");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            app.open_file_manager_tab(None, cx);
            let manager_id = app.file_manager.active_id().unwrap().to_string();
            if let Some(workspace) = app.file_manager.workspace(&manager_id) {
                workspace.update(cx, |workspace, _| {
                    workspace
                        .pane_mut(crate::features::file_manager::PaneId::Right)
                        .set_endpoint(crate::features::file_manager::PaneEndpoint::Session(
                            "local".into(),
                        ));
                });
            }
            assert!(app.workspace_references_session("local", cx));
            app.close_session("local".into(), cx);
            assert!(app.session.has_session("local"));
            app.file_manager.replace_session_id("local", "local-2", cx);
            if let Some(workspace) = app.file_manager.workspace(&manager_id) {
                let workspace = workspace.read(cx);
                assert_eq!(
                    workspace
                        .pane(crate::features::file_manager::PaneId::Right)
                        .endpoint,
                    crate::features::file_manager::PaneEndpoint::Session("local-2".into())
                );
            }
            app.remove_workspace_file_manager_tab(&manager_id, cx);
            assert!(!app.workspace_references_session("local-2", cx));
        });
    }
}
