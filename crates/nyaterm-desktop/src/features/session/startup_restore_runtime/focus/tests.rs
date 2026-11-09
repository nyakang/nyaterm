use gpui::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use nyaterm_core::{InternalEditorDisplay, RestorableOpenTab};
use nyaterm_ui::{NyaInput, nya_root};

use crate::entities::startup_restore::StartupRestoreFocusHandoff;
use crate::features::NyaTermApp;
use crate::features::session::auth_runtime::CredentialPromptState;
use crate::features::test_support::{app_with_visible_ssh_session, editor_document};
use crate::features::text_inputs::TextInputSetup;
use crate::models::{SessionLaunchConfig, WorkspaceTabId};
use crate::test_support::{TestAppCleanup, TestConfigDir};
use nyaterm_remote_desktop::{RdpSessionConfig, VncSessionConfig};
use nyaterm_transport::{SshCredentialPrompt, SshCredentialPromptKind, SshCredentialPromptReason};

struct FocusHost {
    app: Entity<NyaTermApp>,
    list_focus: FocusHandle,
}

impl Render for FocusHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            let surface = match app
                .active_workspace_tab()
                .as_deref()
                .map(WorkspaceTabId::from_layout_key)
            {
                Some(WorkspaceTabId::Editor(id)) => Some(app.workspace_editor_content(&id, cx)),
                Some(WorkspaceTabId::FileManager(id)) => app
                    .file_manager
                    .workspace(&id)
                    .map(|view| view.into_any_element()),
                Some(WorkspaceTabId::Settings) => {
                    Some(app.settings_panel.clone().into_any_element())
                }
                _ => None,
            };
            let search = app.existing_text_input("startup.search");
            app.ensure_startup_restore_focus_handoff(window, cx);
            div()
                .size_full()
                .child(div().track_focus(app.terminal.input_focus()))
                .child(div().track_focus(app.remote_desktop.focus()))
                .child(div().track_focus(app.session.prompt_credential_focus()))
                .child(div().track_focus(&self.list_focus))
                .children(
                    search.map(|field| div().w(px(200.)).h(px(32.)).child(NyaInput::new(&field))),
                )
                .children(surface)
        })
    }
}

fn focus_window<'a>(
    cx: &'a mut TestAppContext,
    app: &Entity<NyaTermApp>,
) -> (Entity<FocusHost>, &'a mut VisualTestContext) {
    let app = app.clone();
    app.update(cx, |app, cx| app.sync_component_theme(cx));
    let mut host = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            FocusHost {
                app,
                list_focus: cx.focus_handle(),
            }
        });
        host = Some(view.clone());
        nya_root(view, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (host.unwrap(), cx)
}

fn begin_restore(app: &mut NyaTermApp, cx: &mut Context<NyaTermApp>) {
    app.sync_component_theme(cx);
    let mut settings = app.settings.summary().clone();
    settings.startup_restore = true;
    settings.startup_restore_window_layout = true;
    app.settings.replace_summary(settings);
    app.stores.startup_restore.update(cx, |store, _| {
        store.mark_open_tabs_restored();
        store.set_loaded_window_layouts(None, None);
        store.set_queue(vec![
            serde_json::from_value(serde_json::json!({"session_type": "ssh"})).unwrap(),
        ]);
        store.request_focus_handoff();
        store.pop_next_tab();
    });
}

fn finish_restore(app: &mut NyaTermApp, window: &mut Window, cx: &mut Context<NyaTermApp>) {
    assert!(app.pump_startup_restore_queue_if_ready(window, cx));
    app.reconcile_terminal_windows();
    app.ensure_startup_restore_focus_handoff(window, cx);
}

fn draw_handoff(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        window.simulate_next_frame(cx);
    });
}

fn handoff_finished(app: &NyaTermApp, cx: &gpui::App) -> bool {
    matches!(
        app.stores.startup_restore.read(cx).focus_handoff(),
        StartupRestoreFocusHandoff::Finished
    )
}

#[test]
fn restored_ssh_gets_focus_after_mount_and_does_not_repeat_on_reconnect() {
    let root = TestConfigDir::new("nyaterm-startup-ssh-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, begin_restore);
    let (host, cx) = focus_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            finish_restore(app, window, cx);
            assert!(!app.terminal.input_focus().is_focused(window));
            assert!(matches!(
                app.stores.startup_restore.read(cx).focus_handoff(),
                StartupRestoreFocusHandoff::Scheduled(_)
            ));
        });
    });
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
        assert!(handoff_finished(app.read(cx), cx));
        let focus = host.read(cx).list_focus.clone();
        window.focus(&focus, cx);
        app.update(cx, |app, cx| {
            assert!(!app.pump_startup_restore_queue_if_ready(window, cx));
            app.ensure_startup_restore_focus_handoff(window, cx);
        });
    });
    draw_handoff(cx);
    cx.update(|window, cx| assert!(host.read(cx).list_focus.is_focused(window)));
}

#[test]
fn startup_focus_waits_for_session_preparation_and_both_layouts() {
    let root = TestConfigDir::new("nyaterm-startup-layout-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "left");
    app.update(&mut cx, |app, cx| {
        begin_restore(app, cx);
        let metadata = app.session.metadata("left").unwrap().clone();
        app.register_session("right", metadata);
        app.activate_session_id("right", cx);
        let placement = app.session.start.allocate_tab_placement(2);
        assert!(
            app.session
                .start_reserve_saved_connection("preparing", placement)
        );
    });
    let (_, cx) = focus_window(&mut cx, &app);
    draw_handoff(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(!app.terminal.input_focus().is_focused(window));
            assert!(!app.pump_startup_restore_queue_if_ready(window, cx));
            app.session.start_release_saved_connection("preparing");
            finish_restore(app, window, cx);
            // Simulate the asynchronous compatibility-reader path after the
            // sessions finish. Neither layout is ready to hand off input yet.
            app.shell.set_workspace_pane_layout_restored(false);
            app.shell.mark_terminal_windows_restore_pending();
        });
    });
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert!(!app.read(cx).terminal.input_focus().is_focused(window));
        app.update(cx, |app, cx| {
            app.shell.set_workspace_pane_layout_restored(true);
            app.ensure_startup_restore_focus_handoff(window, cx);
            assert!(matches!(
                app.stores.startup_restore.read(cx).focus_handoff(),
                StartupRestoreFocusHandoff::Waiting(_)
            ));
            app.shell.complete_terminal_windows_restore();
            app.ensure_startup_restore_focus_handoff(window, cx);
        });
    });
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert_eq!(app.read(cx).session.active_id(), Some("right"));
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
    });
}

#[test]
fn search_and_connection_list_focus_are_preserved_during_restore_and_after_scheduling() {
    for search in [false, true] {
        for after_scheduling in [false, true] {
            let root = TestConfigDir::new("nyaterm-startup-control-focus");
            let mut cx = TestAppContext::single();
            let _cleanup = TestAppCleanup::new(&cx);
            let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
            app.update(&mut cx, |app, cx| {
                begin_restore(app, cx);
                app.text_input("startup.search", "", TextInputSetup::default(), cx);
            });
            let (host, cx) = focus_window(&mut cx, &app);
            draw_handoff(cx);
            cx.update(|window, cx| {
                if after_scheduling {
                    app.update(cx, |app, cx| finish_restore(app, window, cx));
                }
                let focus = if search {
                    app.read(cx)
                        .existing_text_input("startup.search")
                        .unwrap()
                        .read(cx)
                        .component_focus_handle(cx)
                } else {
                    host.read(cx).list_focus.clone()
                };
                window.focus(&focus, cx);
                if !after_scheduling {
                    app.update(cx, |app, cx| {
                        app.ensure_startup_restore_focus_handoff(window, cx);
                        finish_restore(app, window, cx);
                    });
                }
            });
            draw_handoff(cx);
            cx.update(|window, cx| {
                assert!(!app.read(cx).terminal.input_focus().is_focused(window));
                assert!(handoff_finished(app.read(cx), cx));
                if search {
                    assert!(app.read(cx).text_input_focused(window, cx));
                } else {
                    assert!(host.read(cx).list_focus.is_focused(window));
                }
            });
        }
    }
}

#[test]
fn changing_the_active_session_after_scheduling_cancels_startup_focus() {
    let root = TestConfigDir::new("nyaterm-startup-pane-change");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "left");
    app.update(&mut cx, |app, cx| {
        begin_restore(app, cx);
        let metadata = app.session.metadata("left").unwrap().clone();
        app.register_session("right", metadata);
    });
    let (_, cx) = focus_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            finish_restore(app, window, cx);
            app.activate_session_id("right", cx);
        })
    });
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert_eq!(app.read(cx).session.active_id(), Some("right"));
        assert!(!app.read(cx).terminal.input_focus().is_focused(window));
        assert!(handoff_finished(app.read(cx), cx));
    });
}

#[test]
fn background_restore_hands_off_on_first_activation_without_activating_the_window() {
    let root = TestConfigDir::new("nyaterm-startup-background-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, begin_restore);
    let (_, cx) = focus_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.observe_startup_restore_focus_activation(window, cx)
        })
    });
    cx.deactivate_window();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            finish_restore(app, window, cx);
            assert!(!window.is_window_active());
        })
    });
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert!(!app.read(cx).terminal.input_focus().is_focused(window));
        window.activate_window();
    });
    cx.run_until_parked();
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
        assert!(handoff_finished(app.read(cx), cx));
    });
}

#[test]
fn a_scheduled_handoff_waits_if_the_window_deactivates_before_the_frame() {
    let root = TestConfigDir::new("nyaterm-startup-deactivated-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, begin_restore);
    let (host, cx) = focus_window(&mut cx, &app);
    cx.update(|window, cx| app.update(cx, |app, cx| finish_restore(app, window, cx)));
    cx.deactivate_window();
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert!(!app.read(cx).terminal.input_focus().is_focused(window));
        assert!(matches!(
            app.read(cx).stores.startup_restore.read(cx).focus_handoff(),
            StartupRestoreFocusHandoff::Waiting(Some(_))
        ));
        let focus = host.read(cx).list_focus.clone();
        window.focus(&focus, cx);
        window.activate_window();
    });
    draw_handoff(cx);
    cx.update(|window, cx| {
        assert!(host.read(cx).list_focus.is_focused(window));
        assert!(handoff_finished(app.read(cx), cx));
    });
}

#[test]
fn authentication_lock_and_modal_dialog_take_priority_over_startup_focus() {
    for blocker in [
        "credential",
        "pending-focus",
        "lock",
        "dialog",
        "quick-switch",
    ] {
        let root = TestConfigDir::new("nyaterm-startup-blocked-focus");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
        app.update(&mut cx, begin_restore);
        let (_, cx) = focus_window(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                finish_restore(app, window, cx);
                match blocker {
                    "credential" => {
                        let (response_tx, _response_rx) = std::sync::mpsc::channel();
                        app.session
                            .prompts
                            .activate_credential(CredentialPromptState {
                                id: "startup-auth".into(),
                                prompt: SshCredentialPrompt {
                                    host: "example.test".into(),
                                    port: 22,
                                    username: "tester".into(),
                                    connection_name: "test".into(),
                                    kind: SshCredentialPromptKind::Password,
                                    reason: SshCredentialPromptReason::MissingPassword,
                                    attempt: 1,
                                    prompt_text: None,
                                    echo: false,
                                },
                                response_tx: response_tx.into(),
                                authentication: None,
                                value: String::new(),
                            });
                        window.focus(app.session.prompt_credential_focus(), cx);
                    }
                    "pending-focus" => app.session.prompt_request_credential_focus_for_test(),
                    "lock" => app.lock_app(window, cx),
                    "dialog" => app.open_confirm_dialog(
                        (
                            "Test".into(),
                            "Test".into(),
                            "OK".into(),
                            false,
                            |_, _, _| true,
                        ),
                        window,
                        cx,
                    ),
                    "quick-switch" => app.open_quick_switch(window, cx),
                    _ => unreachable!(),
                }
                if blocker == "dialog" {
                    // Even before a dialog assigns its field focus, the modal owns
                    // input and must prevent an automatic workspace handoff.
                    window.blur(cx);
                }
            })
        });
        draw_handoff(cx);
        cx.update(|window, cx| {
            assert!(
                !app.read(cx).terminal.input_focus().is_focused(window),
                "{blocker}"
            );
            assert!(handoff_finished(app.read(cx), cx), "{blocker}");
            if blocker == "credential" {
                assert!(
                    app.read(cx)
                        .session
                        .prompt_credential_focus()
                        .is_focused(window)
                );
            }
        });
    }
}

#[test]
fn rdp_and_vnc_restore_focus_the_remote_surface() {
    for protocol in ["rdp", "vnc"] {
        let root = TestConfigDir::new("nyaterm-startup-remote-focus");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_ssh_session(&mut cx, root.path(), "remote");
        app.update(&mut cx, |app, cx| {
            begin_restore(app, cx);
            app.remote_desktop.insert_disconnected("remote".into());
            app.session.metadata_mut("remote").unwrap().launch_config = if protocol == "rdp" {
                SessionLaunchConfig::Rdp(RdpSessionConfig {
                    relay: None,
                    backend: Default::default(),
                    windows_authentication: None,
                    name: "RDP".into(),
                    host: "example.test".into(),
                    port: 3389,
                    username: String::new(),
                    domain: String::new(),
                    password: None,
                    use_nla: true,
                    certificate_policy: Default::default(),
                    administrative_session: false,
                    display: Default::default(),
                    clipboard: Default::default(),
                    reconnect: Default::default(),
                })
            } else {
                SessionLaunchConfig::Vnc(VncSessionConfig {
                    username: String::new(),
                    relay: None,
                    name: "VNC".into(),
                    host: "example.test".into(),
                    port: 5900,
                    password: None,
                    security: Default::default(),
                    display: Default::default(),
                    clipboard: Default::default(),
                    reconnect: Default::default(),
                    shared: true,
                    view_only: false,
                })
            };
        });
        let (_, cx) = focus_window(&mut cx, &app);
        cx.update(|window, cx| app.update(cx, |app, cx| finish_restore(app, window, cx)));
        draw_handoff(cx);
        cx.update(|window, cx| {
            assert!(
                app.read(cx).remote_desktop.focus().is_focused(window),
                "{protocol}"
            );
            assert!(!app.read(cx).terminal.input_focus().is_focused(window));
        });
    }
}

#[test]
fn restored_workspace_tabs_keep_their_own_surface_focus() {
    for tab in ["editor", "files", "settings"] {
        let root = TestConfigDir::new("nyaterm-startup-workspace-focus");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
        app.update(&mut cx, begin_restore);
        let (_, cx) = focus_window(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(app.pump_startup_restore_queue_if_ready(window, cx));
                app.reconcile_terminal_windows();
                match tab {
                    "editor" => {
                        let document =
                            editor_document("ssh", "/test.txt", InternalEditorDisplay::Workspace);
                        let id = document.id.clone();
                        app.transfer.open_editor_tab(document);
                        app.open_workspace_editor_tab(&id, cx);
                    }
                    "files" => app.open_file_manager_tab(None, cx),
                    "settings" => app.open_settings_workspace_tab(cx),
                    _ => unreachable!(),
                }
                app.ensure_startup_restore_focus_handoff(window, cx);
            })
        });
        draw_handoff(cx);
        cx.update(|window, cx| {
            assert!(
                !app.read(cx).terminal.input_focus().is_focused(window),
                "{tab}"
            );
            assert!(window.focused(cx).is_some(), "{tab}");
            if tab == "settings" {
                assert!(
                    app.read(cx)
                        .settings_panel
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
                );
            }
            assert!(handoff_finished(app.read(cx), cx));
        });
    }
}

#[test]
fn empty_disabled_and_failed_restores_do_not_assign_terminal_focus() {
    for scenario in ["empty", "disabled", "failed"] {
        let root = TestConfigDir::new("nyaterm-startup-no-focus");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
        app.update(&mut cx, |app, cx| {
            let mut settings = app.settings.summary().clone();
            settings.startup_restore = scenario != "disabled";
            app.settings.replace_summary(settings);
            app.session.remove_session_catalog("ssh");
            app.stores.startup_restore.update(cx, |store, _| {
                if scenario == "failed" {
                    let tab: RestorableOpenTab = serde_json::from_value(
                        serde_json::json!({"connection_id": "missing", "session_type": "ssh"}),
                    )
                    .unwrap();
                    store.set_loaded_open_tabs(vec![tab]);
                } else {
                    store.set_loaded_open_tabs(Vec::new());
                }
            });
        });
        let (_, cx) = focus_window(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.resume_startup_restore_if_unlocked(window, cx)
            })
        });
        draw_handoff(cx);
        cx.update(|window, cx| {
            assert!(
                !app.read(cx).terminal.input_focus().is_focused(window),
                "{scenario}"
            );
            assert!(app.read(cx).session.restore_is_complete());
        });
    }
}
