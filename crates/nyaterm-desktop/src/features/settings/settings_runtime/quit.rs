use gpui::{Context, Window};
use rust_i18n::t;

use crate::features::{AppLifecycleEvent, NyaTermApp};

impl NyaTermApp {
    pub(crate) fn has_unconfirmed_settings_changes(&self) -> bool {
        self.settings_draft_dirty() || self.has_unconfirmed_theme_changes()
    }

    pub(in crate::features) fn request_settings_quit_confirmation(
        &mut self,
        open_sessions: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.settings_draft_dirty() {
            return false;
        }
        if !self.settings.begin_quit_confirmation(open_sessions) {
            return true;
        }
        self.open_confirm_dialog_with_cancel(
            (
                t!("settings.discardChangesTitle").to_string(),
                t!("settings.discardChangesDesc").to_string(),
                t!("settings.discardChanges").to_string(),
                true,
                |app, window, cx| {
                    if let Some(count) = app.settings.take_quit_confirmation() {
                        app.cancel_settings(cx);
                        let app = cx.entity();
                        window.defer(cx, move |window, cx| {
                            app.update(cx, |app, cx| {
                                app.handle_window_close_request_with_count(count, window, cx)
                            });
                        });
                    }
                    true
                },
                |app, cx| {
                    if app.settings.take_quit_confirmation().is_some() {
                        app.cancel_editor_exit_approval();
                        app.update
                            .update(cx, |update, _| update.install_requested = false);
                        cx.emit(AppLifecycleEvent::ShutdownCancelled);
                    }
                },
            ),
            window,
            cx,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        AppContext as _, Context, IntoElement, Modifiers, Render, TestAppContext, Window, div,
    };
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_ui::NyaDialogWindowExt as _;

    use crate::features::AppLifecycleEvent;
    use crate::features::settings::theme_designer::ThemeIntent;
    use crate::features::test_support::app_with_visible_local_session;

    struct EmptyView;
    impl Render for EmptyView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn settings_quit_cancel_retains_draft_and_discard_resumes_the_original_request() {
        for session_count in [0, 1] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let root = TestTempDir::new("nyaterm-settings-quit");
            let app = app_with_visible_local_session(&mut cx, root.path(), "local");
            let events = Rc::new(RefCell::new(Vec::new()));
            let observed = events.clone();
            let _subscription = cx.update(|cx| {
                cx.subscribe(&app, move |_, event: &AppLifecycleEvent, _| {
                    observed.borrow_mut().push(*event);
                })
            });
            let original = app.update(&mut cx, |app, cx| {
                app.sync_component_theme(cx);
                app.begin_settings_draft(cx);
                let original = app.settings.summary().terminal_scrollback_lines;
                app.set_terminal_scrollback_lines(
                    app.settings
                        .summary()
                        .terminal_scrollback_lines
                        .saturating_add(100),
                    cx,
                );
                assert!(app.settings_draft_dirty());
                original
            });
            let (_, cx) = cx.add_window_view(|window, cx| {
                let view = cx.new(|_| EmptyView);
                nyaterm_ui::nya_root(view, window, cx)
            });
            // No settings UI is rendered in this window: the exit gate must
            // inspect the authoritative draft, not the active tab.
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.handle_window_close_request_with_count(session_count, window, cx);
                    app.handle_window_close_request_with_count(9, window, cx);
                })
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let cancel = cx
                .debug_bounds("nya-dialog-cancel-button")
                .expect("draft cancel");
            cx.simulate_click(cancel.center(), Modifiers::default());
            cx.run_until_parked();
            assert!(
                events
                    .borrow()
                    .iter()
                    .any(|event| matches!(event, AppLifecycleEvent::ShutdownCancelled))
            );
            assert!(
                !events
                    .borrow()
                    .iter()
                    .any(|event| matches!(event, AppLifecycleEvent::ShutdownRequested))
            );
            app.update(cx, |app, _| {
                assert!(app.settings_draft_dirty());
                assert!(app.session.has_session("local"));
                assert_eq!(app.settings.take_quit_confirmation(), None);
                assert!(
                    app.store_blocking_client()
                        .request_fn(nyaterm_store::StoreDomain::Settings, |store| store
                            .load_app_settings_summary())
                        .is_ok()
                );
            });
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.handle_window_close_request_with_count(session_count, window, cx);
                })
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let discard = cx
                .debug_bounds("nya-dialog-action-button")
                .expect("draft discard");
            cx.simulate_click(discard.center(), Modifiers::default());
            cx.run_until_parked();
            app.update(cx, |app, _| {
                assert!(!app.settings_draft_dirty());
                assert_eq!(app.settings.summary().terminal_scrollback_lines, original);
                assert_eq!(app.settings.take_quit_confirmation(), None);
                assert_eq!(
                    app.session.dialog_should_quit_after_close_all(),
                    session_count > 0
                );
            });
            if session_count == 0 {
                assert!(
                    events
                        .borrow()
                        .iter()
                        .any(|event| matches!(event, AppLifecycleEvent::ShutdownRequested))
                );
            } else {
                assert!(
                    !events
                        .borrow()
                        .iter()
                        .any(|event| matches!(event, AppLifecycleEvent::ShutdownRequested))
                );
                cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
            }
        }
    }

    #[test]
    fn quit_query_covers_ordinary_theme_and_submitting_changes_independently() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-settings-quit-query");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            assert!(!app.has_unconfirmed_settings_changes());
            app.begin_settings_draft(cx);
            app.set_terminal_scrollback_lines(
                app.settings
                    .summary()
                    .terminal_scrollback_lines
                    .saturating_add(100),
                cx,
            );
            assert!(app.has_unconfirmed_settings_changes());
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            assert!(app.settings_draft_dirty());
            assert!(app.has_unconfirmed_theme_changes());
            app.discard_theme_designer(cx);
            assert!(!app.has_unconfirmed_theme_changes());
            assert!(app.has_unconfirmed_settings_changes());
            app.cancel_settings(cx);
            assert!(!app.has_unconfirmed_settings_changes());
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            assert!(app.has_unconfirmed_settings_changes());
            app.discard_theme_designer(cx);
            assert!(!app.has_unconfirmed_settings_changes());
            app.settings.theme_designer_mut().submitting = true;
            assert!(app.has_unconfirmed_settings_changes());
            app.settings.theme_designer_mut().submitting = false;
        });
    }

    #[test]
    fn quit_resolves_the_theme_before_prompting_for_ordinary_settings() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-settings-quit-order");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| EmptyView);
            nyaterm_ui::nya_root(view, window, cx)
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.shell
                    .remember_main_window(window.window_handle().downcast().unwrap());
                app.sync_component_theme(cx);
                app.begin_settings_draft(cx);
                app.set_terminal_scrollback_lines(
                    app.settings.summary().terminal_scrollback_lines + 100,
                    cx,
                );
                app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
                app.handle_window_close_request_with_count(1, window, cx);
                app.handle_window_close_request_with_count(9, window, cx);
                assert_eq!(
                    app.settings.theme_designer().pending,
                    Some(ThemeIntent::Quit(1))
                );
                assert_eq!(app.settings.take_quit_confirmation(), None);
                app.resolve_theme_designer_pending(Some(false), cx);
                assert!(!app.has_unconfirmed_theme_changes());
                assert!(app.settings.theme_designer().preview.is_none());
                assert!(app.settings_draft_dirty());
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(window.has_active_nya_dialog(cx));
            app.update(cx, |app, _| {
                assert_eq!(app.settings.take_quit_confirmation(), Some(1));
                assert!(!app.session.dialog_should_quit_after_close_all());
            });
        });
    }

    #[test]
    fn process_quit_visits_another_windows_ordinary_draft_before_releasing_resources() {
        use crate::app_shell::AppShellStartup;
        use crate::app_shell::DesktopController;
        use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId};

        let root = TestTempDir::new("nyaterm-settings-process-quit");
        let first_root = TestTempDir::new("nyaterm-settings-process-first");
        let second_root = TestTempDir::new("nyaterm-settings-process-second");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().into(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let startup = AppShellStartup::prepare(&runtime);
        let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
        let first = app_with_visible_local_session(&mut cx, first_root.path(), "first");
        let second = app_with_visible_local_session(&mut cx, second_root.path(), "second");
        second.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let first_id = WorkspaceId::new();
        let second_id = WorkspaceId::new();
        let first_handle =
            cx.open_window(gpui::size(gpui::px(800.), gpui::px(600.)), |window, cx| {
                let view = cx.new(|_| EmptyView);
                nyaterm_ui::nya_root(view, window, cx)
            });
        let first_shell = controller.update(&mut cx, |controller, cx| {
            controller.register_tab_drag_test_workspace(
                first_id,
                first.clone(),
                first_handle.into(),
                cx,
            )
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| EmptyView);
            nyaterm_ui::nya_root(view, window, cx)
        });
        let second_shell = cx.update(|window, cx| {
            controller.update(cx, |controller, cx| {
                controller.register_tab_drag_test_workspace(
                    second_id,
                    second.clone(),
                    window.window_handle(),
                    cx,
                )
            })
        });
        let cancel_controller = controller.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&second, move |_, event: &AppLifecycleEvent, cx| {
                if matches!(event, AppLifecycleEvent::ShutdownCancelled) {
                    cancel_controller
                        .update(cx, |controller, cx| controller.cancel_editor_quit_gates(cx));
                }
            })
        });
        second.update(cx, |app, cx| {
            app.sync_component_theme(cx);
            app.begin_settings_draft(cx);
            app.set_terminal_scrollback_lines(
                app.settings.summary().terminal_scrollback_lines + 100,
                cx,
            );
        });
        for discard in [false, true] {
            assert!(controller.update(cx, |controller, cx| {
                controller.defer_quit_for_dirty_editors(first_id, cx)
            }));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let button = cx
                .debug_bounds(if discard {
                    "nya-dialog-action-button"
                } else {
                    "nya-dialog-cancel-button"
                })
                .unwrap();
            cx.simulate_click(button.center(), Modifiers::default());
            cx.run_until_parked();
            cx.update(|_, cx| {
                assert_eq!(second.read(cx).has_unconfirmed_settings_changes(), !discard);
                assert!(first.read(cx).session.has_session("first"));
                assert!(second.read(cx).session.has_session("second"));
            });
        }
        assert!(!controller.update(cx, |controller, cx| {
            controller.defer_quit_for_dirty_editors(first_id, cx)
        }));
        drop((first_shell, second_shell));
    }
}
