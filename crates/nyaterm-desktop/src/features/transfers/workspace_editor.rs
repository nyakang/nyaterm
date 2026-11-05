use super::remote_text_editor::RemoteTextEditor;
use crate::features::NyaTermApp;
use crate::models::WorkspaceTabId;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*,
    px, rgb,
};
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

/// Retained per document, including while another tab hides its surface.
pub(in crate::features) struct WorkspaceEditorView {
    app: gpui::WeakEntity<NyaTermApp>,
    id: String,
    editor: Option<Entity<RemoteTextEditor>>,
    focus_requested: bool,
    _app_observation: Subscription,
    editor_observation: Option<Subscription>,
}

impl WorkspaceEditorView {
    pub(in crate::features) fn new(
        app: Entity<NyaTermApp>,
        id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let observation = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app: app.downgrade(),
            id,
            editor: None,
            focus_requested: true,
            _app_observation: observation,
            editor_observation: None,
        }
    }

    pub(in crate::features) fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_requested = true;
        cx.notify();
    }

    pub(in crate::features) fn execute_edit_command(
        &self,
        command: &crate::features::layout::title_bar::definition::TitleMenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::features::layout::title_bar::definition::TitleMenuCommand;
        let Some(editor) = &self.editor else {
            return;
        };
        self.focus(window, cx);
        match command {
            TitleMenuCommand::TerminalCopy => {
                window.dispatch_action(Box::new(nyaterm_ui::NyaCopy), cx)
            }
            TitleMenuCommand::TerminalPaste => {
                window.dispatch_action(Box::new(nyaterm_ui::NyaPaste), cx)
            }
            TitleMenuCommand::TerminalSelectAll => {
                window.dispatch_action(Box::new(nyaterm_ui::NyaSelectAll), cx)
            }
            TitleMenuCommand::TerminalFind => {
                editor.update(cx, |editor, cx| editor.open_search(cx))
            }
            _ => {}
        }
    }

    pub(in crate::features) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            window.focus(&editor.read(cx).focus_handle(cx), cx);
        }
    }
}

impl Render for WorkspaceEditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let Some(document) = app.read(cx).transfer.editor_tab_snapshot(&self.id) else {
            return div().into_any_element();
        };
        if self.editor.is_none() {
            let editor = cx.new(|cx| RemoteTextEditor::new(app.clone(), &document, window, cx));
            self.editor_observation = Some(cx.observe(&editor, |_, _, cx| cx.notify()));
            self.editor = Some(editor);
        }
        let editor = self
            .editor
            .as_ref()
            .expect("document editor created")
            .clone();
        editor.update(cx, |editor, cx| editor.sync_from_tab(&document, cx));
        let active = match document.display {
            nyaterm_core::InternalEditorDisplay::Workspace => {
                app.read(cx).active_workspace_tab().as_deref()
                    == Some(
                        WorkspaceTabId::Editor(self.id.clone())
                            .layout_key()
                            .as_str(),
                    )
            }
            nyaterm_core::InternalEditorDisplay::Window => app
                .read(cx)
                .transfer
                .active_editor_tab()
                .is_some_and(|doc| doc.id == self.id),
        };
        if self.focus_requested && active {
            self.focus_requested = false;
            self.focus(window, cx);
        }
        let palette = app.read(cx).theme_palette();
        let font_size = app
            .read(cx)
            .settings
            .summary()
            .transfer_internal_editor_font_size;
        let font_app = app.clone();
        let (line, column) = editor.read(cx).cursor_position(cx);
        let save_id = self.id.clone();
        let reload_id = self.id.clone();
        let force_id = self.id.clone();
        let reload_app = app.clone();
        let force_app = app.clone();
        let all_app = app.clone();
        let external_app = app.clone();
        let external_id = self.id.clone();
        let mut surface = div()
            .id("workspace-file-editor")
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_xs()
                            .child(document.remote_path.clone()),
                    )
                    .child(
                        NyaButton::new(
                            "workspace-editor-reload",
                            t!("fileEditor.reload").to_string(),
                        )
                        .small()
                        .disabled(document.loading || document.saving)
                        .on_click(move |_, window, cx| {
                            reload_app.update(cx, |app, cx| {
                                app.reload_editor_document(&reload_id, false, window, cx)
                            });
                        }),
                    )
                    .child(
                        NyaButton::new(
                            "workspace-editor-open-external",
                            t!("fileEditor.openExternal").to_string(),
                        )
                        .small()
                        .disabled(document.loading || document.saving)
                        .on_click(move |_, _, cx| {
                            external_app.update(cx, |app, cx| {
                                app.open_editor_document_external(&external_id, cx)
                            });
                        }),
                    )
                    .child(
                        NyaButton::new(
                            "workspace-editor-save-all",
                            t!("fileEditor.saveAll").to_string(),
                        )
                        .small()
                        .on_click(move |_, window, cx| {
                            all_app.update(cx, |app, cx| {
                                app.save_all_transfer_editor_tabs(window, cx)
                            });
                        }),
                    )
                    .child(
                        NyaButton::new("workspace-editor-save", t!("common.save").to_string())
                            .small()
                            .disabled(document.loading || document.saving)
                            .on_click(move |_, window, cx| {
                                app.update(cx, |app, cx| {
                                    app.save_transfer_editor_tab(&save_id, false, window, cx)
                                });
                            }),
                    ),
            )
            .when_some(document.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(palette.danger))
                        .child(error),
                )
            })
            .when_some(document.backup_warning_path.clone(), |this, path| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .child(t!("fileEditor.backupCleanupWarning", path = path).to_string()),
                )
            });
        if document.conflict {
            surface = surface.child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .gap_2()
                    .child(t!("fileEditor.conflictDesc").to_string())
                    .child(
                        NyaButton::new(
                            "workspace-editor-force-save",
                            t!("fileEditor.forceSave").to_string(),
                        )
                        .small()
                        .on_click(move |_, window, cx| {
                            force_app.update(cx, |app, cx| {
                                app.confirm_editor_force_save(&force_id, window, cx)
                            });
                        }),
                    ),
            );
        }
        surface
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(editor.clone()),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(t!("fileEditor.lineColumn", line = line, column = column).to_string())
                    .child(
                        div().flex().items_center().gap_1().children(
                            [
                                (
                                    "editor-font-decrease",
                                    "−".to_string(),
                                    t!("fileEditor.fontDecrease").to_string(),
                                    font_size <= 8,
                                    -1,
                                ),
                                (
                                    "editor-font-increase",
                                    "+".to_string(),
                                    t!("fileEditor.fontIncrease").to_string(),
                                    font_size >= 72,
                                    1,
                                ),
                                (
                                    "editor-font-reset",
                                    t!("fileEditor.fontResetLabel").to_string(),
                                    t!("fileEditor.fontReset").to_string(),
                                    false,
                                    0,
                                ),
                            ]
                            .into_iter()
                            .map(
                                |(id, label, tooltip, disabled, delta)| {
                                    let app = font_app.clone();
                                    let editor = editor.clone();
                                    let button = NyaButton::new(id, label)
                                        .variant(NyaButtonVariant::Ghost)
                                        .compact()
                                        .height(px(24.))
                                        .tooltip(tooltip)
                                        .disabled(disabled)
                                        .on_click(move |_, window, cx| {
                                            app.update(cx, |app, cx| {
                                                if delta == 0 {
                                                    app.reset_transfer_internal_editor_font_size(
                                                        cx,
                                                    );
                                                } else {
                                                    app.adjust_transfer_internal_editor_font_size(
                                                        delta, cx,
                                                    );
                                                }
                                            });
                                            window.focus(&editor.read(cx).focus_handle(cx), cx);
                                        });
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .when(delta == 1, |this| {
                                            this.child(
                                                div().text_xs().child(format!("{font_size} px")),
                                            )
                                        })
                                        .child(
                                            div()
                                                .debug_selector(move || id.to_string())
                                                .child(button),
                                        )
                                },
                            ),
                        ),
                    )
                    .child(if document.loading {
                        t!("common.loading").to_string()
                    } else if document.saving {
                        t!("common.saving").to_string()
                    } else if document.dirty {
                        t!("fileEditor.unsaved").to_string()
                    } else {
                        t!("fileEditor.saved").to_string()
                    }),
            )
            .into_any_element()
    }
}

impl NyaTermApp {
    pub(in crate::features) fn workspace_editor_content(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view = if let Some(view) = self.transfer.workspace_editor_view(id) {
            view
        } else {
            let app = cx.entity();
            let view = cx.new(|view_cx| WorkspaceEditorView::new(app, id.to_string(), view_cx));
            self.transfer
                .set_workspace_editor_view(id.to_string(), view.clone());
            view
        };
        view.into_any_element()
    }

    pub(in crate::features) fn reload_editor_document(
        &mut self,
        id: &str,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.transfer.editor_tab_snapshot(id) else {
            return;
        };
        if document.loading || document.saving {
            return;
        }
        if document.dirty && !confirmed {
            let id = id.to_string();
            self.open_confirm_dialog_with_cancel(
                (
                    t!("fileEditor.reloadDirtyTitle").to_string(),
                    t!("fileEditor.reloadDirtyDesc").to_string(),
                    t!("fileEditor.discardAndReload").to_string(),
                    true,
                    move |app: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                        app.reload_editor_document(&id, true, window, cx);
                        true
                    },
                    |_, _| {},
                ),
                window,
                cx,
            );
            return;
        }
        self.transfer.begin_editor_document_reload(id);
        self.start_sftp_editor_load_job(
            document.session_id.clone(),
            document.id.clone(),
            document.remote_file_path(),
            window,
            cx,
        );
    }

    pub(in crate::features) fn confirm_editor_force_save(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id.to_string();
        self.open_confirm_dialog_with_cancel(
            (
                t!("fileEditor.conflictTitle").to_string(),
                t!("fileEditor.conflictDesc").to_string(),
                t!("fileEditor.forceSave").to_string(),
                true,
                move |app: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                    app.save_transfer_editor_tab(&id, true, window, cx);
                    true
                },
                |_, _| {},
            ),
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::features::NyaTermApp;
    use crate::features::test_support::{app_with_visible_local_session, editor_document};
    use crate::models::{TabDockEdge, TabDockZone, WorkspaceTabId};
    use futures::StreamExt as _;
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
        Window, div, prelude::*,
    };
    use nyaterm_core::{InternalEditorDisplay, test_support::TestTempDir};

    struct WorkspaceHost {
        app: Entity<NyaTermApp>,
    }
    impl Render for WorkspaceHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().child(
                self.app
                    .update(cx, |app, cx| app.workspace_view(cx).into_any_element()),
            )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    struct EditorHost {
        app: Entity<NyaTermApp>,
        id: String,
    }

    impl Render for EditorHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.app
                    .update(cx, |app, cx| app.workspace_editor_content(&self.id, cx)),
            )
        }
    }

    #[test]
    fn editor_font_controls_restore_focus_preserve_undo_and_ignore_modified_wheels() {
        use gpui::{Modifiers, ScrollDelta, ScrollEvent, point, px};

        for display in [
            InternalEditorDisplay::Workspace,
            InternalEditorDisplay::Window,
        ] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let root = TestTempDir::new("nyaterm-editor-font-controls");
            let app = app_with_visible_local_session(&mut cx, root.path(), "local");
            let (id, terminal_size) = app.update(&mut cx, |app, cx| {
                app.sync_component_theme(cx);
                app.begin_settings_draft(cx);
                let mut doc = editor_document("local", "/font.txt", display);
                doc.content = "line\n".repeat(500).into();
                let id = doc.id.clone();
                app.transfer.open_editor_tab(doc);
                if display == InternalEditorDisplay::Workspace {
                    app.open_workspace_editor_tab(&id, cx);
                }
                (id, app.settings.summary().terminal_font_size)
            });
            let host_app = app.clone();
            let host_id = id.clone();
            let (_, cx) = cx.add_window_view(move |window, cx| {
                let host = cx.new(|_| EditorHost {
                    app: host_app,
                    id: host_id,
                });
                nyaterm_ui::nya_root(host, window, cx)
            });
            draw(cx);
            let view = app.read_with(cx, |app, _| {
                app.transfer.workspace_editor_view(&id).unwrap()
            });
            let editor = view.read_with(cx, |view, _| view.editor.clone().unwrap());
            let state = editor.read_with(cx, |editor, _| editor.document_for_test());
            cx.update(|window, cx| {
                state.update(cx, |doc, cx| {
                    doc.apply_edit("猫".into(), 11..11, window, cx)
                })
            });
            draw(cx);
            let selection = state.read_with(cx, |doc, cx| doc.selected_range(cx));
            for (id, expected) in [
                ("editor-font-increase", 14),
                ("editor-font-decrease", 13),
                ("editor-font-reset", 13),
            ] {
                let bounds = cx.debug_bounds(id).unwrap();
                cx.simulate_click(bounds.center(), Modifiers::default());
                draw(cx);
                cx.update(|window, cx| {
                    assert_eq!(
                        app.read(cx)
                            .settings
                            .summary()
                            .transfer_internal_editor_font_size,
                        expected
                    );
                    assert_eq!(
                        app.read(cx).settings.summary().terminal_font_size,
                        terminal_size
                    );
                    assert!(editor.read(cx).focus_handle(cx).is_focused(window));
                    assert_eq!(state.read(cx).selected_range(cx), selection);
                });
            }
            // Include both boundaries, where the old bubbling handler changed
            // the font, as well as scrolling through the middle of the document.
            for modifiers in [
                Modifiers::default(),
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
            ] {
                for delta in [100_000., -100., -100., -100_000., -100., 100_000.] {
                    cx.simulate_event(ScrollEvent {
                        position: point(px(120.), px(120.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                        modifiers,
                        ..Default::default()
                    });
                    draw(cx);
                    assert_eq!(
                        app.read_with(cx, |app, _| app
                            .settings
                            .summary()
                            .transfer_internal_editor_font_size),
                        13
                    );
                }
            }
            cx.update(|window, cx| state.update(cx, |doc, cx| doc.undo(window, cx)));
            draw(cx);
            assert_eq!(
                state.read_with(cx, |doc, cx| doc.value(cx)),
                "line\n".repeat(500)
            );
            if display == InternalEditorDisplay::Workspace {
                cx.update(|window, cx| app.update(cx, |app, cx| {
                    app.set_transfer_internal_editor_font_size(21, cx);
                    app.execute_title_menu_command(&crate::features::layout::title_bar::definition::TitleMenuCommand::ResetZoom, window, cx);
                    assert_eq!(app.settings.summary().transfer_internal_editor_font_size, 13);
                    assert_eq!(app.settings.summary().terminal_font_size, terminal_size);
                }));
            }
        }
    }

    #[test]
    fn child_editor_font_shortcuts_honor_custom_bindings_without_changing_the_main_terminal() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-font-shortcuts");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let terminal_size = app.read_with(&cx, |app, _| app.settings.summary().terminal_font_size);
        let (_, main_cx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|_| WorkspaceHost { app: app.clone() });
            nyaterm_ui::nya_root(host, window, cx)
        });
        let main = main_cx.update(|window, _| window.window_handle().downcast().unwrap());
        let id = app.update(&mut cx, |app, cx| {
            app.shell.remember_main_window(main);
            app.ensure_shortcut_interceptor(cx);
            app.sync_component_theme(cx);
            app.begin_settings_draft(cx);
            let doc = editor_document("local", "/font.txt", InternalEditorDisplay::Window);
            let id = doc.id.clone();
            app.transfer.open_editor_tab(doc);
            id
        });
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|_| EditorHost {
                app: app.clone(),
                id: id.clone(),
            });
            nyaterm_ui::nya_root(host, window, cx)
        });
        let child = vcx.update(|window, _| window.window_handle().downcast().unwrap());
        app.update(vcx, |app, _| app.transfer.finish_editor_window_open(child));
        draw(vcx);
        let primary = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        for (keys, expected) in [
            ("equal", 14),
            ("-", 13),
            ("equal", 14),
            ("0", 13),
            ("shift-equal", 14),
        ] {
            vcx.simulate_keystrokes(&format!("{primary}-{keys}"));
            draw(vcx);
            assert_eq!(
                app.read_with(vcx, |app, _| app
                    .settings
                    .summary()
                    .transfer_internal_editor_font_size),
                expected
            );
        }
        vcx.update(|_, cx| {
            crate::shortcuts::rebuild_keymap(
                &std::collections::HashMap::from([("view.zoomIn".into(), "Ctrl+Alt+U".into())]),
                cx,
            )
        });
        vcx.simulate_keystrokes("ctrl-equal");
        vcx.simulate_keystrokes("ctrl-alt-u");
        draw(vcx);
        assert_eq!(
            app.read_with(vcx, |app, _| app
                .settings
                .summary()
                .transfer_internal_editor_font_size),
            15
        );
        assert_eq!(
            app.read_with(vcx, |app, _| app.settings.summary().terminal_font_size),
            terminal_size
        );
    }

    #[test]
    fn workspace_editor_keeps_selection_and_undo_across_switching_and_docking() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-retained-view");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        let id = app.update(&mut cx, |app, cx| {
            app.sync_component_theme(cx);
            let doc = editor_document(
                "local",
                "/a.custom-extension",
                InternalEditorDisplay::Workspace,
            );
            let id = doc.id.clone();
            app.transfer.open_editor_tab(doc);
            app.open_workspace_editor_tab(&id, cx);
            id
        });
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|cx| {
                cx.observe(&host_app, |_, _, cx| cx.notify()).detach();
                WorkspaceHost { app: host_app }
            });
            nyaterm_ui::nya_root(host, window, cx)
        });
        draw(cx);
        let view = app.read_with(cx, |app, _| {
            app.transfer.workspace_editor_view(&id).unwrap()
        });
        let editor = view.read_with(cx, |view, _| view.editor.clone().unwrap());
        let state = editor.read_with(cx, |editor, _| editor.document_for_test());
        cx.update(|window, cx| {
            state.update(cx, |doc, cx| {
                doc.apply_edit("中文".into(), 8..8, window, cx)
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_transfer_editor(
                    nyaterm_files::sftp::SftpFileEntry {
                        name: "a.custom-extension".into(),
                        path: "/a.custom-extension".into(),
                        file_type: nyaterm_files::sftp::SftpFileType::File,
                        size: Some(8),
                        permissions: None,
                        owner: String::new(),
                        group: String::new(),
                        modified_at: None,
                        raw_path_token: None,
                        symlink_target_is_directory: false,
                    },
                    window,
                    cx,
                );
                assert!(app.transfer.unknown_file_dialog().is_none());
                assert_eq!(app.transfer.editor_documents().len(), 1);
                assert!(app.transfer.transfer_jobs().is_empty());
            });
        });
        let value = state.read_with(cx, |doc, cx| doc.value(cx));
        let selection = state.read_with(cx, |doc, cx| doc.selected_range(cx));
        app.update(cx, |app, cx| {
            app.select_session("local".to_string(), cx);
            app.activate_workspace_tab(&WorkspaceTabId::Editor(id.clone()).layout_key(), cx);
            let group = app.current_terminal_group().unwrap();
            app.dock_tab_on_terminal_window_leaf(
                WorkspaceTabId::Editor(id.clone()).layout_key(),
                group,
                TabDockZone::Edge(TabDockEdge::Right),
                cx,
            );
        });
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.transfer.workspace_editor_view(&id).unwrap().entity_id(),
                view.entity_id()
            )
        });
        assert_eq!(state.read_with(cx, |doc, cx| doc.value(cx)), value);
        assert_eq!(
            state.read_with(cx, |doc, cx| doc.selected_range(cx)),
            selection
        );
        cx.update(|window, cx| state.update(cx, |doc, cx| doc.undo(window, cx)));
        draw(cx);
        assert_eq!(state.read_with(cx, |doc, cx| doc.value(cx)), "original");
        cx.update(|window, cx| {
            app.read_with(cx, |app, _| {
                assert!(!app.terminal.input_focus().is_focused(window))
            })
        });
    }

    #[test]
    fn workspace_save_uses_document_source_after_source_tab_closes() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-editor-source-save");
        let path = root.path().join("document.txt");
        std::fs::create_dir_all(root.path()).unwrap();
        std::fs::write(&path, "original").unwrap();
        let baseline = nyaterm_files::file_browser::FileBrowserService::local()
            .read_text_document_path(
                &nyaterm_files::sftp::RemoteFilePath::new(path.to_str().unwrap()),
                1024,
            )
            .unwrap();
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        let (id, mut receiver) = app.update(&mut cx, |app, cx| {
            let mut doc = editor_document(
                "local",
                path.to_str().unwrap(),
                InternalEditorDisplay::Workspace,
            );
            doc.revision = Some(baseline.revision);
            let id = doc.id.clone();
            app.transfer.open_editor_tab(doc);
            app.open_workspace_editor_tab(&id, cx);
            app.transfer
                .sync_editor_content(&id, "saved after source closed");
            app.close_session("local".to_string(), cx);
            (id, app.transfer.take_transfer_event_receiver().unwrap())
        });
        let cx = crate::features::test_support::session_focus_test_window(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.save_transfer_editor_tab(&id, false, window, cx)
            })
        });
        let event = futures::executor::block_on(receiver.next()).unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.apply_transfer_event(event, window, cx);
            })
        });
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "saved after source closed"
        );
        app.update(cx, |app, cx| {
            assert!(!app.transfer.editor_tab_snapshot(&id).unwrap().dirty);
            assert!(app.ordered_tab_sessions().is_empty());
            app.request_workspace_tabs_close(
                vec![WorkspaceTabId::Editor(id).layout_key()],
                None,
                cx,
            );
            assert!(!app.session.has_session("local"));
        });
    }
}
