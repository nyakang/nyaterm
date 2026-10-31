use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use gpui::{AppContext as _, TestAppContext};
use nyaterm_core::AiExecutionProfile;
use nyaterm_transport::{
    LocalSessionConfig, RemoteFileBackendPreference, RemoteFileBackendPreferenceStore,
    SessionEvent, SessionKind, SessionManager, SshAgentPrompt, SshAgentPromptPhase,
    SshCredentialPrompt, SshCredentialPromptKind, SshCredentialPromptReason, SshHostKey,
    SshKeyboardInteractivePrompt, SshKeyboardInteractiveRequest, SshSessionConfig,
};

use crate::features::runtime_jobs::SessionStartResult;
use crate::features::session::{HostKeyPromptChoice, HostKeyPromptIssue};
use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use crate::models::{
    SessionEventBridge, SessionLaunchConfig, SessionRuntimeMetadata, StartupCommandAction,
    TabActionsSubmenu, TerminalFramePipeline, WorkspaceSplitDirection,
};
use crate::temporary_ssh_link::TemporaryLinkProtocol;
use crate::test_support::{TestConfigDir, blocking_test_store};

use crate::app_shell::session_hub::ssh_connections::SshConnectionPool;

use super::{
    CredentialPromptState, FailedSessionStart, HostKeyPromptRequest,
    KeyboardInteractivePromptState, NativeOtpCodePreview, NativeOtpProvider,
    PendingReconnectCompletion, PendingSessionStart, PromptResolution, RenameSessionSubmission,
    SessionFeatureFocus, SessionFeatureState, SessionPromptState, SessionStartEventRequest,
    SessionStartFeatureState,
};

#[test]
fn host_key_prompt_accept_and_reject_restore_terminal_input_focus() {
    let root = TestConfigDir::new("nyaterm-host-key-focus");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "terminal");
    let cx = session_focus_test_window(&mut cx, &app);

    for choice in [HostKeyPromptChoice::Accept, HostKeyPromptChoice::Reject] {
        let (response_tx, response_rx) = mpsc::channel();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.session.prompts.active_host_key_prompt = Some(HostKeyPromptRequest {
                    id: "host-key".into(),
                    host_key: SshHostKey {
                        host: "example.test".into(),
                        port: 22,
                        host_identifier: "example.test".into(),
                        key_type: "ssh-ed25519".into(),
                        key_base64: "test-key".into(),
                        fingerprint: "SHA256:test".into(),
                    },
                    issue: HostKeyPromptIssue::Unknown,
                    response_tx,
                });
                window.focus(app.session.dialog_tab_actions_focus(), cx);

                // A stale button must leave the prompt and its focus untouched.
                app.resolve_host_key_prompt("stale".into(), choice, window, cx);
                assert!(app.session.dialog_tab_actions_focus().is_focused(window));
                assert!(response_rx.try_recv().is_err());

                app.resolve_host_key_prompt("host-key".into(), choice, window, cx);
                assert_eq!(response_rx.try_recv().unwrap(), choice);
                assert!(app.session.prompt_active_host_key().is_none());
                assert!(app.terminal.input_focus().is_focused(window));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(app.read(cx).terminal.input_focus().is_focused(window));
        });
    }
}

#[test]
fn host_key_prompt_overlay_keeps_its_content_height() {
    use crate::features::NyaTermApp;
    use gpui::{
        Context, Entity, IntoElement, Render, VisualTestContext, Window, div, prelude::*, px,
    };

    struct HostKeyHost(Entity<NyaTermApp>);

    impl Render for HostKeyHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let overlay = self.0.update(cx, |app, cx| {
                app.ssh_auth_prompt_overlay(cx).into_any_element()
            });
            div().relative().size_full().child(overlay)
        }
    }

    let root = TestConfigDir::new("nyaterm-host-key-layout");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "terminal");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        let (response_tx, _) = mpsc::channel();
        app.session.prompts.active_host_key_prompt = Some(HostKeyPromptRequest {
            id: "host-key-layout".into(),
            host_key: SshHostKey {
                host: "example.test".into(),
                port: 22,
                host_identifier: "example.test".into(),
                key_type: "ssh-ed25519".into(),
                key_base64: "fixture-key".into(),
                fingerprint: "SHA256:fixture".into(),
            },
            issue: HostKeyPromptIssue::Unknown,
            response_tx,
        });
    });
    let (_, cx) = cx.add_window_view(|_, _| HostKeyHost(app));
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
    let card = cx.debug_bounds("ssh-host-key-card").unwrap();
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(card.size.height > px(100.));
    assert!(card.top() >= px(12.));
    assert!(card.bottom() <= viewport.height - px(12.));
}

#[test]
fn consuming_an_entire_sideband_prefix_keeps_ui_routing_after_the_drain_completes() {
    for prefix in [b":".as_slice(), b"*", b"::TRZ"] {
        let root = TestConfigDir::new("nyaterm-sideband-prefix-routing");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "prefix-session");
        cx.update_entity(&app, |app, cx| {
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::Output {
                    session_id: "prefix-session".into(),
                    data: prefix.to_vec(),
                });
            // This path consumes events without expiring detector prefixes.
            app.drain_session_events_for_input_wake(cx);
            assert!(app.session.pending_events_are_empty());
            assert_eq!(app.session.event_bridge_queued_event_count(), 0);
            assert!(app.session.has_protocol_runtime_sessions());
            assert!(
                app.session
                    .event_bridge
                    .session_requires_ui_for_test("prefix-session")
            );
        });
    }
}

fn pending(name: &str) -> PendingSessionStart {
    PendingSessionStart {
        attempt: Default::default(),
        connection_name: name.to_string(),
        launch_config: None,
        requested_at: Instant::now(),
        kind: SessionKind::LocalPty,
        ai_execution_profile: AiExecutionProfile::default(),
        custom_name: None,
        tab_color: None,
        locked: false,
        after_session_id: None,
        insert_index: None,
        seed_output: None,
        startup_command: None,
        multiplex_key: None,
        source_connection_id: None,
        workspace_split: None,
        tab_placement: None,
        reconnect_session_id: None,
    }
}

fn failed_start(
    app: &mut crate::features::NyaTermApp,
    id: &str,
    index: usize,
    config: Option<SessionLaunchConfig>,
) -> super::SessionStartTabPlacement {
    let placement = app.session.start.allocate_tab_placement(index);
    let mut request = pending(id);
    request.tab_placement = Some(placement);
    request.launch_config = config;
    request.custom_name = Some(format!("custom {id}"));
    request.tab_color = Some(0x123456);
    request.locked = true;
    app.session.start.register_pending(id.to_string(), request);
    let SessionStartEventRequest::Pending {
        pending,
        was_active,
    } = app.session.start.take_event_request(id)
    else {
        unreachable!()
    };
    app.session.start.record_failure(
        id.to_string(),
        pending,
        "test failure".to_string(),
        was_active,
        false,
    );
    placement
}

#[test]
fn failed_retry_keeps_one_tab_and_preserves_presentation_on_preparation_failure() {
    let root = TestConfigDir::new("nyaterm-failed-retry");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    let cx = session_focus_test_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let placement = failed_start(
                app,
                "failure",
                1,
                Some(SessionLaunchConfig::Local(LocalSessionConfig {
                    encoding: "invalid-test-encoding".into(),
                    ..LocalSessionConfig::default()
                })),
            );
            app.retry_failed_session_start("failure".into(), window, cx);
            assert!(!app.session.start.has_pending());
            assert_eq!(app.session.start.failed_entries().count(), 1);
            let new_id = app.session.active_failed_request_id().unwrap().to_string();
            assert_ne!(new_id, "failure");
            let failed = app.session.start.failed(&new_id).unwrap();
            assert_eq!(
                failed.pending.tab_placement.unwrap().request_sequence,
                placement.request_sequence
            );
            assert_eq!(
                failed.pending.custom_name.as_deref(),
                Some("custom failure")
            );
            assert_eq!(failed.pending.tab_color, Some(0x123456));
            assert!(failed.pending.locked);
            assert!(failed.pending.reconnect_session_id.is_none());
            assert_ne!(failed.error, "test failure");
            app.retry_failed_session_start("failure".into(), window, cx);
            assert_eq!(
                app.session.active_failed_request_id(),
                Some(new_id.as_str())
            );
        })
    });
}

#[test]
fn failed_retry_replaces_the_moved_tab_on_success_with_a_fresh_attempt() {
    use crate::features::runtime_jobs::SessionStartSuccess;
    use crate::features::shell::session_tabs::SessionStripTabId;
    use nyaterm_transport::SessionInfo;
    let root = TestConfigDir::new("nyaterm-failed-retry-success");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    let cx = session_focus_test_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let config = SessionLaunchConfig::Local(LocalSessionConfig {
                shell_path: Some(
                    root.path()
                        .join("missing-test-shell")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ..Default::default()
            });
            let placement = failed_start(app, "failure", 1, Some(config.clone()));
            app.session
                .start
                .failed("failure")
                .unwrap()
                .pending
                .attempt
                .cancel();
            let key = SessionStripTabId::Start(placement.request_sequence).layout_key();
            app.commit_session_strip_order(&None, &[key.clone(), "source".into()], &key, cx);
            app.retry_failed_session_start("failure".into(), window, cx);
            assert!(!app.session.start.has_failed());
            assert_eq!(app.session.start.pending_count(), 1);
            let request_id = app.session.start.active_pending.clone().unwrap();
            assert_ne!(request_id, "failure");
            assert!(app.session.start.attempt(&request_id).check().is_ok());
            assert_eq!(app.session_strip_tabs(&None)[0].id.layout_key(), key);
            app.retry_failed_session_start("failure".into(), window, cx);
            assert_eq!(app.session.start.pending_count(), 1);
            let now = Instant::now();
            app.apply_session_start_event(
                SessionStartResult {
                    request_id,
                    connection_name: "retry".into(),
                    kind: SessionKind::LocalPty,
                    worker_started_at: now,
                    worker_finished_at: now,
                    result: Ok(SessionStartSuccess {
                        session_info: SessionInfo {
                            id: "connected".into(),
                            name: "retry".into(),
                            kind: SessionKind::LocalPty,
                            working_dir: None,
                            cols: 80,
                            rows: 24,
                        },
                        multiplex_handle: None,
                        launch_config: Some(config),
                        cleanup: None,
                    }),
                },
                cx,
            );
            assert!(!app.session.start.has_pending());
            assert!(!app.session.start.has_failed());
            assert_eq!(app.workspace_tab_ids(), ["connected", "source"]);
            assert_eq!(app.session.custom_name("connected"), Some("custom failure"));
            assert_eq!(app.session.tab_color("connected"), Some(0x123456));
            assert!(app.session.tab_is_locked("connected"));
            assert_eq!(app.session.active_id(), Some("connected"));
        })
    });
}

#[test]
fn failed_retry_uses_current_saved_configuration_and_retains_deleted_sources() {
    let root = TestConfigDir::new("nyaterm-failed-saved-retry");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    let cx = session_focus_test_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            failed_start(app, "deleted", 1, None);
            app.retry_failed_session_start("deleted".into(), window, cx);
            assert!(app.session.start.failed("deleted").is_some());
            assert!(!app.session.start.has_pending());

            let connection =
                serde_json::from_value::<nyaterm_core::SavedConnection>(serde_json::json!({
                    "id": "saved", "name": "updated", "type": "local_terminal", "shell_args": "'"
                }))
                .unwrap();
            app.connection_state
                .replace_loaded(vec![connection], Vec::new());
            failed_start(app, "saved-failure", 2, None);
            app.session
                .start
                .failed
                .get_mut("saved-failure")
                .unwrap()
                .pending
                .source_connection_id = Some("saved".into());
            app.retry_failed_session_start("saved-failure".into(), window, cx);
            let failed = app.session.start.active_failed().unwrap();
            assert_eq!(
                failed.pending.source_connection_id.as_deref(),
                Some("saved")
            );
            assert!(!app.session.start.has_pending());
            assert_ne!(failed.error, "test failure");
            assert!(!app.session.start.saved_connection_is_preparing("saved"));
        })
    });
}

#[test]
fn closing_retry_preparation_prevents_stale_callbacks_and_worker_results() {
    let mut starts = SessionStartFeatureState::new();
    starts.begin_retry_preparation("preparing".into(), pending("retry"));
    let options = super::SavedConnectionStartOptions {
        retry_request_id: Some("preparing".into()),
        ..Default::default()
    };
    assert!(starts.retry_is_pending(&options));
    let attempt = starts.attempt("preparing");
    starts.close_pending("preparing").unwrap();
    assert!(attempt.check().is_err());
    assert!(!starts.retry_is_pending(&options));
    assert!(
        !starts.has_cancelled_results(),
        "no worker was submitted during preparation"
    );

    starts.begin_retry_preparation("worker".into(), pending("retry"));
    starts.finish_retry_preparation("worker");
    starts.close_pending("worker").unwrap();
    assert!(matches!(
        starts.take_event_request("worker"),
        SessionStartEventRequest::Cancelled
    ));
    assert!(!starts.has_failed());
}

struct FailureViewHost {
    app: gpui::Entity<crate::features::NyaTermApp>,
    strip: bool,
}

struct StartupWorkspaceHost {
    app: gpui::Entity<crate::features::NyaTermApp>,
}

impl gpui::Render for StartupWorkspaceHost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{IntoElement as _, ParentElement as _, Styled as _};
        let workspace = self
            .app
            .update(cx, |app, cx| app.workspace_view(cx).into_any_element());
        gpui::div().size_full().flex().flex_col().child(workspace)
    }
}

#[test]
fn grouped_connection_failure_and_retry_fill_and_center_in_the_content_area() {
    use gpui::px;

    let root = TestConfigDir::new("nyaterm-startup-workspace-layout");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.reconcile_terminal_windows();
        let group = app.current_terminal_group().unwrap();
        let placement = failed_start(app, "failure", 1, None);
        app.shell
            .reserve_terminal_start_group(placement.request_sequence, group);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        StartupWorkspaceHost { app: fixture_app }
    });
    for (panel, message) in [
        ("failed-workspace-state", "failed-workspace-message"),
        ("pending-workspace-state", "pending-workspace-name"),
    ] {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        let content = app.read_with(cx, |app, _| {
            let group = app.current_terminal_group().unwrap();
            let mut bounds = app.shell.terminal_split_bounds(&group).unwrap();
            bounds.origin.y += px(36.);
            bounds.size.height -= px(36.);
            bounds
        });
        let panel = cx.debug_bounds(panel).unwrap();
        let message = cx.debug_bounds(message).unwrap();
        assert_eq!(panel, content);
        assert!(
            (message.center().y - panel.center().y).abs() < panel.size.height / 6.,
            "the connection status must stay near the content area's vertical center"
        );
        app.update(cx, |app, cx| {
            if let Some(failed) = app.session.start.take_failed_for_retry("failure") {
                app.session
                    .start
                    .register_pending("retry".into(), failed.pending);
                cx.notify();
            }
        });
    }
}

impl gpui::Render for FailureViewHost {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _};
        self.app.update(cx, |app, cx| {
            window.focus(app.terminal.input_focus(), cx);
            let content = if self.strip {
                app.session_tab_strip(cx)
            } else {
                app.failed_workspace_state(cx).into_any_element()
            };
            gpui::div()
                .size_full()
                .flex()
                .flex_col()
                .track_focus(app.terminal.input_focus())
                .on_key_down(cx.listener(|app, event, window, cx| {
                    app.cancel_internal_drag_on_escape(event, window, cx);
                }))
                .child(app.session_tab_drag_listener_layer(cx))
                .child(content)
        })
    }
}

#[test]
fn selected_failed_and_retry_tabs_replace_the_previous_tab_highlight() {
    use gpui::VisualTestContext;
    use nyaterm_core::InternalEditorDisplay;

    use crate::features::test_support::editor_document;
    use crate::models::{QuickSwitchItem, WorkspaceTabId};

    let draw_and_assert =
        |cx: &mut VisualTestContext, selected: &'static str, unselected: &[&'static str]| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds(selected).is_some(), "selected: {selected}");
            for selector in unselected {
                assert!(
                    cx.debug_bounds(selector).is_none(),
                    "unselected: {selector}"
                );
            }
        };
    for (grouped, editor) in [(false, false), (true, false), (true, true)] {
        let root = TestConfigDir::new("nyaterm-startup-tab-selection");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "source");
        let (formal_tab, formal_highlight) = app.update(&mut cx, |app, cx| {
            app.sync_component_theme(cx);
            if grouped {
                app.reconcile_terminal_windows();
            }
            let (tab, highlight) = if editor {
                let mut document =
                    editor_document("source", "/test.txt", InternalEditorDisplay::Workspace);
                document.id = "editor".into();
                let id = document.id.clone();
                app.transfer.open_editor_tab(document);
                app.open_workspace_editor_tab(&id, cx);
                ("editor-tab-editor", "editor-tab-active-editor")
            } else {
                ("session-tab-source", "session-tab-active-source")
            };
            let group = app.current_terminal_group();
            let placement = failed_start(app, "failure", 1, None);
            if let Some(group) = group {
                app.shell
                    .reserve_terminal_start_group(placement.request_sequence, group);
            }
            (tab, highlight)
        });
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            StartupWorkspaceHost { app: fixture_app }
        });
        draw_and_assert(cx, "failed-session-tab-active-failure", &[formal_highlight]);
        app.read_with(cx, |app, _| {
            let selected = app
                .quick_switch_items()
                .into_iter()
                .filter_map(|item| match item {
                    QuickSwitchItem::Session {
                        id, active: true, ..
                    } => Some(id),
                    QuickSwitchItem::Pending {
                        request_id,
                        active: true,
                        ..
                    } => Some(request_id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(selected, ["failure"]);
        });
        let bounds = cx.debug_bounds(formal_tab).unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        draw_and_assert(cx, formal_highlight, &["failed-session-tab-active-failure"]);
        let bounds = cx.debug_bounds("failed-session-tab-failure").unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        draw_and_assert(cx, "failed-session-tab-active-failure", &[formal_highlight]);
        app.update(cx, |app, cx| {
            let failed = app.session.start.take_failed_for_retry("failure").unwrap();
            app.session
                .start
                .begin_retry_preparation("retry".into(), failed.pending);
            cx.notify();
        });
        draw_and_assert(cx, "pending-session-tab-active-retry", &[formal_highlight]);
        app.update(cx, |app, cx| {
            app.session.simulate_start_failure("retry");
            cx.notify();
        });
        draw_and_assert(cx, "failed-session-tab-active-retry", &[formal_highlight]);
        app.update(cx, |app, cx| {
            app.close_failed_session_start("retry".into(), cx)
        });
        draw_and_assert(cx, formal_highlight, &["failed-session-tab-active-retry"]);
        app.read_with(cx, |app, _| {
            assert!(matches!(
                app.selected_session_strip_tab(&app.current_terminal_group()),
                Some(
                    crate::features::shell::session_tabs::SessionStripTabId::Workspace(
                        WorkspaceTabId::Session(_)
                            | WorkspaceTabId::Editor(_)
                            | WorkspaceTabId::FileManager(_)
                    )
                )
            ));
        });
    }
}

#[test]
fn failure_view_reconnect_button_retries_the_selected_request() {
    let root = TestConfigDir::new("nyaterm-failure-button");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    app.update(&mut cx, |app, cx| {
        failed_start(
            app,
            "button",
            1,
            Some(SessionLaunchConfig::Local(LocalSessionConfig {
                encoding: "invalid-test-encoding".into(),
                ..Default::default()
            })),
        );
        app.sync_component_theme(cx);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        FailureViewHost {
            app: fixture_app,
            strip: false,
        }
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let button = cx.debug_bounds("failed-session-reconnect-target").unwrap();
    assert!(button.size.width > gpui::px(0.) && button.size.height > gpui::px(0.));
    cx.simulate_click(button.center(), Default::default());
    app.update(cx, |app, _| {
        assert!(app.session.start.failed("button").is_none());
        assert_eq!(app.session.start.failed_entries().count(), 1);
        assert!(!app.session.start.has_pending());
    });
}

#[test]
fn real_failed_tabs_drag_both_directions_cancel_and_never_detach() {
    use crate::app_shell::{AppShellStartup, DesktopController};
    use gpui::{MouseButton, point, px};
    use nyaterm_core::{AppRuntime, RuntimeMode};
    for (failed_source, action) in [
        (true, "commit"),
        (false, "commit"),
        (true, "escape"),
        (true, "close"),
        (true, "outside"),
    ] {
        let root = TestConfigDir::new("nyaterm-failure-drag");
        let controller_root = TestConfigDir::new("nyaterm-failure-drag-controller");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            controller_root.path().to_path_buf(),
            controller_root.path().join("config"),
            controller_root.path().join("logs"),
            controller_root.path().join("cache"),
            None,
        );
        let startup = AppShellStartup::prepare(&runtime);
        let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
        let app = app_with_visible_local_session(&mut cx, root.path(), "a");
        let key = app.update(&mut cx, |app, cx| {
            let metadata = app.session.metadata("a").unwrap().clone();
            app.session.register_session_metadata("b", metadata);
            let placement = failed_start(app, "failure", 1, None);
            app.desktop_controller = Some(controller.downgrade());
            app.sync_component_theme(cx);
            crate::features::shell::session_tabs::SessionStripTabId::Start(
                placement.request_sequence,
            )
            .layout_key()
        });
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            FailureViewHost {
                app: fixture_app,
                strip: true,
            }
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let bounds = cx
            .debug_bounds(if failed_source {
                "failed-session-tab-failure"
            } else {
                "session-tab-a"
            })
            .unwrap();
        let start = bounds.origin + point(px(30.), px(16.));
        let end = if action == "outside" {
            point(px(-40.), start.y)
        } else {
            point(
                cx.debug_bounds("session-tab-b").unwrap().right() - px(12.),
                start.y,
            )
        };
        cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            start + point(px(10.), px(0.)),
            Some(MouseButton::Left),
            Default::default(),
        );
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
        if failed_source {
            cx.read(|cx| assert!(controller.read(cx).tab_drag_source().is_none()));
        }
        if action == "escape" {
            cx.simulate_keystrokes("escape");
        }
        if action == "close" {
            app.update(cx, |app, cx| {
                app.session.remove_session_catalog("b");
                cx.notify();
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
        cx.run_until_parked();
        // Releases outside a drop target are cleaned by the existing hover clock.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        app.update(cx, |app, _| {
            let expected: Vec<String> = match (failed_source, action) {
                (true, "commit") => vec!["a".into(), "b".into(), key.clone()],
                (false, "commit") => vec![key.clone(), "b".into(), "a".into()],
                (_, "close") => vec!["a".into(), key.clone()],
                _ => vec!["a".into(), key.clone(), "b".into()],
            };
            assert_eq!(
                app.session_strip_tabs(&None)
                    .iter()
                    .map(|tab| tab.id.layout_key())
                    .collect::<Vec<_>>(),
                expected,
                "{failed_source} {action}"
            );
            assert!(
                !app.session.tab_drag_is_pending(),
                "{failed_source} {action}"
            );
        });
        cx.read(|cx| assert!(controller.read(cx).tab_drag_source().is_none()));
    }
}

#[test]
fn mixed_failed_pending_editor_and_session_tabs_keep_dragged_order_on_completion() {
    use crate::features::shell::session_tabs::SessionStripTabId;
    use crate::models::{TerminalWindowNode, WorkspaceTabId};
    let root = TestConfigDir::new("nyaterm-failed-order");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "a");
    cx.update_entity(&app, |app, cx| {
        let metadata = app.session.metadata("a").unwrap().clone();
        app.session.register_session_metadata("b", metadata.clone());
        let document = crate::features::test_support::editor_document(
            "a",
            "/editor.txt",
            nyaterm_core::InternalEditorDisplay::Workspace,
        );
        let editor = WorkspaceTabId::Editor(document.id.clone()).layout_key();
        app.transfer.open_editor_tab(document);
        app.shell
            .restore_terminal_window_tree(Some(TerminalWindowNode::Leaf {
                id: "group".into(),
                tab_ids: vec!["a".into(), "b".into(), editor.clone()],
                active_tab_id: Some("a".into()),
            }));
        let x = failed_start(app, "x", 1, None);
        let y = failed_start(app, "y", 3, None);
        let z = app.session.start.allocate_tab_placement(5);
        let mut request = pending("z");
        request.tab_placement = Some(z);
        app.session.start.register_pending("z".into(), request);
        for placement in [x, y, z] {
            app.shell
                .reserve_terminal_start_group(placement.request_sequence, "group".into());
        }
        let x_key = SessionStripTabId::Start(x.request_sequence).layout_key();
        let y_key = SessionStripTabId::Start(y.request_sequence).layout_key();
        let z_key = SessionStripTabId::Start(z.request_sequence).layout_key();
        let group = Some("group".into());
        let order = vec![
            y_key.clone(),
            editor.clone(),
            z_key.clone(),
            "a".into(),
            x_key.clone(),
            "b".into(),
        ];
        app.commit_session_strip_order(&group, &order, &x_key, cx);
        assert_eq!(
            app.session_strip_tabs(&group)
                .iter()
                .map(|tab| tab.id.layout_key())
                .collect::<Vec<_>>(),
            order
        );
        assert_eq!(app.session.active_failed_request_id(), Some("x"));
        assert_eq!(
            app.workspace_tab_ids(),
            [editor.clone(), "a".into(), "b".into()]
        );
        assert_eq!(app.session.resolve_start_tab_placement(z).insert_index, 2);

        // The callback still has its original placement snapshot (index 5).
        let _ = app.session.start.take_event_request("z");
        app.register_session_for_start("finished", metadata, Some(z), None);
        let expected = vec![
            y_key,
            editor,
            "finished".into(),
            "a".into(),
            x_key,
            "b".into(),
        ];
        assert_eq!(
            app.session_strip_tabs(&group)
                .iter()
                .map(|tab| tab.id.layout_key())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(
            app.workspace_tab_ids()
                .iter()
                .all(|key| !key.starts_with("\0start:"))
        );
    });
}

fn test_otp_provider(root: &Path) -> Arc<NativeOtpProvider> {
    let store = blocking_test_store(root);
    Arc::new(NativeOtpProvider::new(store))
}

fn prompt_state(cx: &TestAppContext, root: &Path) -> SessionPromptState {
    SessionPromptState::new(test_otp_provider(root), cx.update(|cx| cx.focus_handle()))
}

fn session_state(cx: &TestAppContext, root: &Path) -> SessionFeatureState {
    session_state_with_runtime(
        cx,
        root,
        Arc::new(SessionManager::new()),
        SshConnectionPool::default(),
    )
}

fn session_state_with_runtime(
    cx: &TestAppContext,
    root: &Path,
    manager: Arc<SessionManager>,
    ssh_connections: SshConnectionPool,
) -> SessionFeatureState {
    let event_bridge = SessionEventBridge::spawn(
        Arc::clone(&manager),
        TerminalFramePipeline::default(),
        None,
        "utf-8".to_string(),
        10_000,
    );
    let focus = || cx.update(|cx| cx.focus_handle());
    SessionFeatureState::new(
        manager,
        ssh_connections,
        event_bridge,
        test_otp_provider(root),
        SessionFeatureFocus {
            credential: focus(),
            tab_actions: focus(),
            color_picker: focus(),
            info: focus(),
        },
    )
}

fn session_metadata(name: &str, multiplex_key: Option<&str>) -> SessionRuntimeMetadata {
    let config = LocalSessionConfig {
        name: name.to_string(),
        ..LocalSessionConfig::default()
    };
    SessionRuntimeMetadata {
        ssh_config: None,
        ssh_multiplex_key: multiplex_key.map(ToOwned::to_owned),
        source_connection_id: None,
        ai_execution_profile: AiExecutionProfile::Posix,
        launch_config: SessionLaunchConfig::Local(config),
        disconnected: false,
    }
}

fn rdp_session_config(name: &str) -> nyaterm_remote_desktop::RdpSessionConfig {
    nyaterm_remote_desktop::RdpSessionConfig {
        relay: None,
        backend: Default::default(),
        windows_authentication: None,
        name: name.into(),
        host: "127.0.0.1".into(),
        port: 3389,
        username: String::new(),
        domain: String::new(),
        password: None,
        use_nla: false,
        certificate_policy: Default::default(),
        administrative_session: false,
        display: Default::default(),
        clipboard: Default::default(),
        reconnect: Default::default(),
    }
}

fn remote_desktop_metadata(connection_id: &str) -> SessionRuntimeMetadata {
    SessionRuntimeMetadata {
        ssh_config: None,
        ssh_multiplex_key: None,
        source_connection_id: Some(connection_id.to_string()),
        ai_execution_profile: AiExecutionProfile::Posix,
        launch_config: SessionLaunchConfig::Rdp(rdp_session_config("rdp")),
        disconnected: false,
    }
}

struct EmptyRemoteFilePreferenceStore;

impl RemoteFileBackendPreferenceStore for EmptyRemoteFilePreferenceStore {
    fn load_backend(
        &self,
        _endpoint_key: &str,
    ) -> anyhow::Result<Option<RemoteFileBackendPreference>> {
        Ok(None)
    }

    fn save_backend(
        &self,
        _endpoint_key: &str,
        _preference: &RemoteFileBackendPreference,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

fn credential_prompt_state(id: &str) -> CredentialPromptState {
    let (response_tx, _response_rx) = mpsc::channel();
    CredentialPromptState {
        id: id.to_string(),
        prompt: SshCredentialPrompt {
            host: "example.test".to_string(),
            port: 22,
            username: "nya".to_string(),
            connection_name: "example".to_string(),
            kind: SshCredentialPromptKind::Password,
            reason: SshCredentialPromptReason::MissingPassword,
            attempt: 1,
            prompt_text: None,
            echo: false,
        },
        response_tx: response_tx.into(),
        authentication: None,
        value: String::new(),
    }
}

/// Answering a prompt frees the single active slot the next queued one needs,
/// and nothing is enqueued at that moment, so the activation pass has to be woken
/// explicitly. Without this the second prompt of a pair never appears.
#[test]
fn resolving_a_prompt_wakes_the_activation_pass() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());
    let mut wake_rx = prompts
        .take_wake_receiver()
        .expect("the state holds its receiver until the drain starts");

    prompts.activate_credential(credential_prompt_state("credential-1"));
    prompts.arm_wake();
    assert!(
        wake_rx.try_recv().is_err(),
        "activating does not itself free the slot"
    );

    assert!(prompts.take_credential().is_some());
    assert!(
        wake_rx.try_recv().is_ok(),
        "freeing the active slot must wake the activation pass"
    );

    // A mismatched resolution leaves the slot occupied, so it must not wake.
    prompts.activate_credential(credential_prompt_state("credential-2"));
    prompts.arm_wake();
    assert!(matches!(
        prompts.take_host_key_resolution("absent"),
        PromptResolution::Inactive
    ));
    assert!(
        wake_rx.try_recv().is_err(),
        "a resolution that frees nothing must not wake"
    );
}

#[test]
fn credential_prompt_owner_isolates_input_and_clears_focus_on_take() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());

    prompts.activate_credential(credential_prompt_state("credential-1"));
    assert!(prompts.credential_focus_is_pending());
    assert!(!prompts.apply_credential_input("credential-2", "wrong".to_string()));
    assert_eq!(
        prompts
            .active_credential()
            .expect("credential prompt should remain active")
            .value,
        ""
    );

    assert!(prompts.apply_credential_input("credential-1", "secret".to_string()));
    let prompt = prompts
        .take_credential()
        .expect("matching credential prompt should be taken");
    assert_eq!(prompt.id, "credential-1");
    assert_eq!(prompt.value, "secret");
    assert!(!prompts.credential_focus_is_pending());
    assert!(prompts.active_credential().is_none());
}

#[test]
fn mismatched_host_key_resolution_preserves_active_prompt() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());
    let (response_tx, _response_rx) = mpsc::channel();
    prompts.active_host_key_prompt = Some(HostKeyPromptRequest {
        id: "host-key-1".to_string(),
        host_key: SshHostKey {
            host: "example.test".to_string(),
            port: 22,
            host_identifier: "example.test".to_string(),
            key_type: "ssh-ed25519".to_string(),
            key_base64: "test-key".to_string(),
            fingerprint: "SHA256:test".to_string(),
        },
        issue: HostKeyPromptIssue::Unknown,
        response_tx,
    });

    assert!(matches!(
        prompts.take_host_key_resolution("host-key-2"),
        PromptResolution::Changed
    ));
    assert_eq!(
        prompts
            .active_host_key()
            .expect("mismatched resolution must restore the prompt")
            .id,
        "host-key-1"
    );
}

#[test]
fn mismatched_agent_resolution_preserves_active_prompt() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());
    let broker = prompts.agent_broker();
    let session = broker
        .begin_prompt(&SshAgentPrompt {
            host: "example.test".to_string(),
            port: 22,
            username: "nya".to_string(),
            connection_name: "example".to_string(),
            phase: SshAgentPromptPhase::Sign,
            attempt: 1,
            message: String::new(),
        })
        .expect("begin agent prompt");
    let target = prompts
        .activate_next_agent()
        .expect("activate agent prompt");

    assert_eq!(target, "nya@example.test:22");
    assert!(matches!(
        prompts.take_agent_resolution("agent-other"),
        PromptResolution::Changed
    ));
    assert_eq!(
        prompts
            .active_agent()
            .expect("mismatched resolution must preserve the prompt")
            .target(),
        "nya@example.test:22"
    );

    let active_id = prompts
        .active_agent()
        .expect("agent prompt remains active")
        .id
        .clone();
    drop(session);
    assert!(matches!(
        prompts.take_agent_resolution(&active_id),
        PromptResolution::Changed
    ));
    assert!(prompts.reconcile_agent());
}

/// The TOTP clock sleeps to the code's own step boundary rather than polling.
///
/// This replaced the last thing on the runtime tick's control plane. Waking on the
/// boundary is both the latest moment that is still correct and the earliest worth
/// waking for; a clock that waited a whole period from *now* would show a stale code
/// for up to that period.
#[test]
fn totp_clock_sleeps_to_the_next_step_boundary() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());
    prompts.activate_keyboard_interactive(totp_prompt_for_test(30));

    // 30s period: one second past a boundary leaves 29 to wait, and the last second
    // of a step leaves one.
    assert_eq!(prompts.keyboard_totp_seconds_to_next_step(31), Some(29));
    assert_eq!(prompts.keyboard_totp_seconds_to_next_step(59), Some(1));
    assert_eq!(
        prompts.keyboard_totp_seconds_to_next_step(60),
        Some(30),
        "landing exactly on a boundary waits a full step, never zero"
    );
}

/// No prompt showing a TOTP code means no clock at all.
#[test]
fn no_totp_clock_without_a_code_on_screen() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());
    assert_eq!(prompts.keyboard_totp_seconds_to_next_step(31), None);

    // A keyboard-interactive prompt that is not TOTP must not run one either.
    let mut prompt = totp_prompt_for_test(30);
    prompt.otp_type = Some("hotp".to_string());
    prompts.activate_keyboard_interactive(prompt);
    assert_eq!(
        prompts.keyboard_totp_seconds_to_next_step(31),
        None,
        "only a TOTP code changes on a wall-clock step"
    );
}

fn totp_prompt_for_test(period: u64) -> KeyboardInteractivePromptState {
    let (response_tx, _response_rx) = mpsc::channel();
    // The receiver is dropped, which is fine: these tests never send a response.
    KeyboardInteractivePromptState {
        id: "keyboard-totp".to_string(),
        request: SshKeyboardInteractiveRequest {
            host: "example.test".to_string(),
            port: 22,
            username: "nya".to_string(),
            connection_name: "example".to_string(),
            name: "verification".to_string(),
            instructions: String::new(),
            round: 1,
            prompts: vec![SshKeyboardInteractivePrompt {
                prompt: "Code".to_string(),
                echo: false,
            }],
            otp_id: Some("otp-1".to_string()),
        },
        response_tx,
        responses: vec![String::new()],
        focused_index: 0,
        otp_code: Some("123456".to_string()),
        otp_type: Some("totp".to_string()),
        otp_period: period,
        otp_time_step: None,
        otp_error: None,
    }
}

#[test]
fn otp_missing_entry_preserves_manual_timing_but_clears_refresh_timing() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut prompts = prompt_state(&cx, test_dir.path());
    let (response_tx, _response_rx) = mpsc::channel();
    prompts.activate_keyboard_interactive(KeyboardInteractivePromptState {
        id: "keyboard-1".to_string(),
        request: SshKeyboardInteractiveRequest {
            host: "example.test".to_string(),
            port: 22,
            username: "nya".to_string(),
            connection_name: "example".to_string(),
            name: "verification".to_string(),
            instructions: String::new(),
            round: 1,
            prompts: vec![SshKeyboardInteractivePrompt {
                prompt: "Code".to_string(),
                echo: false,
            }],
            otp_id: Some("otp-1".to_string()),
        },
        response_tx,
        responses: vec![String::new()],
        focused_index: 0,
        otp_code: Some("test-code".to_string()),
        otp_type: Some("totp".to_string()),
        otp_period: 30,
        otp_time_step: Some(7),
        otp_error: None,
    });

    assert!(!prompts.apply_keyboard_interactive_otp_result(Ok(None), false));
    assert_eq!(
        prompts
            .active_keyboard_interactive()
            .expect("keyboard prompt should remain active")
            .otp_time_step,
        Some(7)
    );

    assert!(!prompts.apply_keyboard_interactive_otp_result(Ok(None), true));
    let active = prompts
        .active_keyboard_interactive()
        .expect("keyboard prompt should remain active");
    assert!(active.otp_time_step.is_none());
    assert_eq!(active.otp_error.as_deref(), Some("OTP entry not found"));

    assert!(prompts.apply_keyboard_interactive_otp_result(
        Ok(Some(NativeOtpCodePreview {
            code: "next-code".to_string(),
            otp_type: "totp".to_string(),
            period: 30,
            time_step: Some(8),
        })),
        true,
    ));
    let active = prompts
        .active_keyboard_interactive()
        .expect("keyboard prompt should remain active");
    assert_eq!(active.otp_time_step, Some(8));
    assert!(active.otp_error.is_none());
}

#[test]
fn session_state_owns_live_runtime_and_initializes_transient_state() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let focus = || cx.update(|cx| cx.focus_handle());
    let manager = Arc::new(SessionManager::new());
    let event_bridge = SessionEventBridge::spawn(
        Arc::clone(&manager),
        TerminalFramePipeline::default(),
        None,
        "utf-8".to_string(),
        10_000,
    );
    let otp_provider = test_otp_provider(test_dir.path());
    let mut sessions = SessionFeatureState::new(
        Arc::clone(&manager),
        SshConnectionPool::default(),
        event_bridge,
        Arc::clone(&otp_provider),
        SessionFeatureFocus {
            credential: focus(),
            tab_actions: focus(),
            color_picker: focus(),
            info: focus(),
        },
    );

    assert!(Arc::ptr_eq(&sessions.manager_handle(), &manager));
    assert!(!sessions.start_has_pending());
    assert!(!sessions.start.has_failed());
    assert!(sessions.command_history_for("missing").is_none());
    assert!(sessions.active_id().is_none());
    assert!(sessions.active_ssh_config().is_none());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::SendOnly
    );
    assert!(sessions.order.is_empty());
    assert!(sessions.metadata.is_empty());
    assert_eq!(sessions.protocol_runtime_counts(), (0, 0, 0));
    assert!(Arc::ptr_eq(&sessions.prompt_otp_provider(), &otp_provider));
    assert!(sessions.prompt_active_credential().is_none());
    assert!(sessions.prompt_active_keyboard_interactive().is_none());
    assert!(!sessions.restore_is_complete());
    assert!(sessions.mark_restore_complete());
    assert!(!sessions.mark_restore_complete());

    sessions.extend_pending_events([
        SessionEvent::Output {
            session_id: "session-a".to_string(),
            data: vec![1, 2, 3],
        },
        SessionEvent::Error {
            session_id: "session-a".to_string(),
            message: "test error".to_string(),
        },
    ]);
    assert_eq!(sessions.pending_event_count(), 2);
    assert_eq!(sessions.pending_event_output_bytes(), 3);
    assert!(matches!(
        sessions.pop_pending_event(),
        Some(SessionEvent::Output { data, .. }) if data == vec![1, 2, 3]
    ));

    sessions.record_command_history("session-a", "  git status  ");
    sessions.record_command_history("session-a", "cargo check");
    assert_eq!(
        sessions.command_history_for("session-a"),
        Some(["cargo check".to_string(), "git status".to_string()].as_slice())
    );
    sessions.migrate_command_history("session-a", "session-b");
    sessions.remove_command_from_all_history("git status");
    assert_eq!(
        sessions.command_history_for("session-b"),
        Some(["cargo check".to_string()].as_slice())
    );

    assert!(sessions.begin_reconnect_action("session-b".to_string()));
    assert_eq!(sessions.busy_action("session-b"), Some("reconnect"));
    assert!(!sessions.begin_disconnect_action("session-b".to_string()));
    sessions.finish_busy_action("session-b");
    assert!(!sessions.session_is_busy("session-b"));

    sessions
        .dialogs
        .open_startup_command(StartupCommandAction::Multiplex, 75_000);
    assert_eq!(sessions.dialogs.startup_command_delay_ms(), 60_000);
    assert!(
        sessions
            .dialogs
            .apply_text_input("startup-command", "  uptime  ".to_string())
    );
    let (action, request) = sessions
        .dialogs
        .take_startup_command()
        .expect("non-empty command should be accepted");
    assert_eq!(action, StartupCommandAction::Multiplex);
    assert_eq!(request.command, "uptime");
    assert_eq!(request.delay_ms, 60_000);
    assert!(sessions.dialogs.take_startup_command().is_none());

    sessions
        .dialogs
        .open_tab_actions("session-a".to_string(), Some((10.0, 20.0)));
    assert!(
        sessions
            .dialogs
            .select_tab_actions_submenu(TabActionsSubmenu::Ai)
    );
    sessions.dialogs.request_quit_after_close_all();
    sessions.dialogs.open_close_all_sessions_confirm();
    assert!(sessions.dialogs.tab_actions_session_id().is_none());
    assert!(sessions.dialogs.take_close_all_sessions_confirm());
    assert!(!sessions.dialogs.should_quit_after_close_all());

    sessions
        .dialogs
        .open_rename("session-a".to_string(), "original");
    assert!(
        sessions
            .dialogs
            .apply_text_input("rename", "   ".to_string())
    );
    assert!(matches!(
        sessions.dialogs.take_rename_submission(),
        RenameSessionSubmission::Empty
    ));
    assert!(
        sessions
            .dialogs
            .apply_text_input("rename", "renamed".to_string())
    );
    assert!(matches!(
        sessions.dialogs.take_rename_submission(),
        RenameSessionSubmission::Ready { target: crate::features::shell::tab_mouse::TabActionTarget::Session(session_id), name }
            if session_id == "session-a" && name == "renamed"
    ));

    sessions.dialogs.open_temporary_ssh_link();
    sessions
        .dialogs
        .apply_temporary_ssh_link("user@example.test".to_string());
    sessions
        .dialogs
        .reject_temporary_ssh_link("temporarySsh.invalid");
    assert_eq!(
        sessions.dialogs.temporary_ssh_link_error(),
        Some("temporarySsh.invalid")
    );
    sessions
        .dialogs
        .set_temporary_link_protocol(TemporaryLinkProtocol::Serial);
    assert_eq!(
        sessions.dialogs.temporary_link_protocol(),
        TemporaryLinkProtocol::Serial
    );
    assert_eq!(sessions.dialogs.temporary_ssh_link_error(), None);
    sessions
        .dialogs
        .apply_temporary_serial_port_name("COM7".to_string());
    sessions
        .dialogs
        .apply_temporary_serial_baud_rate("115200x".to_string());
    assert_eq!(sessions.dialogs.temporary_serial_port_name(), "COM7");
    assert_eq!(sessions.dialogs.temporary_serial_baud_rate(), "115200");
    sessions.dialogs.close_temporary_ssh_link();
    assert_eq!(
        sessions.dialogs.temporary_link_protocol(),
        TemporaryLinkProtocol::Ssh
    );
    assert!(sessions.dialogs.temporary_ssh_link_draft().is_empty());
    assert!(sessions.dialogs.temporary_serial_port_name().is_empty());
    assert_eq!(sessions.dialogs.temporary_serial_baud_rate(), "115200");
}

#[test]
fn session_catalog_registration_and_reordering_stay_synchronized() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    sessions.register_session_metadata("session-a", session_metadata("first", None));
    sessions.register_session_metadata("session-b", session_metadata("second", None));
    sessions.register_session_metadata("session-c", session_metadata("third", None));
    sessions.register_session_metadata("session-a", session_metadata("updated", None));

    assert_eq!(
        sessions.session_order(),
        ["session-a", "session-b", "session-c"]
    );
    assert_eq!(sessions.ordered_sessions().len(), 3);
    assert_eq!(sessions.session_info("session-a").unwrap().name, "updated");

    assert!(sessions.move_session_after("session-a", "session-c"));
    assert_eq!(
        sessions.session_order(),
        ["session-b", "session-c", "session-a"]
    );
    assert!(sessions.move_session_to_index("session-c", 0));
    assert_eq!(
        sessions.session_order(),
        ["session-c", "session-b", "session-a"]
    );
    assert!(!sessions.move_session_after("missing", "session-a"));
}

#[test]
fn transferring_split_session_catalog_preserves_order_metadata_and_pending_events() {
    let test_dir = TestConfigDir::new("nyaterm-session-transfer-test");
    let target_dir = TestConfigDir::new("nyaterm-session-transfer-target-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut source = session_state(&cx, test_dir.path());
    let mut target = session_state(&cx, target_dir.path());
    source.register_session_metadata("root", session_metadata("root session", None));
    source.register_session_metadata("leaf", session_metadata("split leaf", None));
    source.register_session_metadata("keep", session_metadata("unmoved", None));
    target.register_session_metadata("existing", session_metadata("existing", None));
    source.set_custom_name("root".into(), "renamed".into());
    source.set_tab_color("root", Some(0x123456));
    source.set_tab_locked("root", true);
    source.extend_pending_events([SessionEvent::CwdChanged {
        session_id: "leaf".into(),
        cwd: "/workspace".into(),
    }]);

    let ids = vec!["root".to_string(), "leaf".to_string()];
    assert!(source.can_transfer_sessions(&ids).is_ok());
    let bridge_events = source.pause_sessions_for_transfer(&ids);
    let bundle = source
        .detach_sessions_for_transfer(&ids, bridge_events)
        .unwrap();
    target.attach_sessions_from_transfer(bundle, Some(1));

    assert_eq!(source.session_order(), ["keep"]);
    assert_eq!(target.session_order(), ["existing", "root", "leaf"]);
    assert_eq!(target.display_name("root").as_deref(), Some("renamed"));
    assert_eq!(target.tab_color("root"), Some(0x123456));
    assert!(target.tab_is_locked("root"));
    assert_eq!(source.pending_event_count(), 0);
    assert_eq!(target.pending_event_count(), 1);
}

#[test]
fn session_catalog_inserts_out_of_order_completions_at_reserved_positions() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    let mut starts = SessionStartFeatureState::new();
    let session_a = starts.allocate_tab_placement(0);
    let session_b = starts.allocate_tab_placement(1);
    let session_c = starts.allocate_tab_placement(2);

    sessions.register_session_metadata_for_start(
        "session-c",
        session_metadata("third", None),
        Some(session_c),
        None,
    );
    sessions.register_session_metadata_for_start(
        "session-b",
        session_metadata("second", None),
        Some(session_b),
        None,
    );
    assert_eq!(sessions.session_order(), ["session-b", "session-c"]);

    sessions.register_session_metadata_for_start(
        "session-a",
        session_metadata("first", None),
        Some(session_a),
        None,
    );
    assert_eq!(
        sessions.session_order(),
        ["session-a", "session-b", "session-c"]
    );
}

#[test]
fn session_catalog_keeps_existing_tabs_before_reserved_appends() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    let mut starts = SessionStartFeatureState::new();
    sessions.register_session_metadata("session-x", session_metadata("existing", None));
    let session_a = starts.allocate_tab_placement(1);
    let session_b = starts.allocate_tab_placement(2);

    sessions.register_session_metadata_for_start(
        "session-b",
        session_metadata("second", None),
        Some(session_b),
        None,
    );
    sessions.register_session_metadata_for_start(
        "session-a",
        session_metadata("first", None),
        Some(session_a),
        None,
    );

    assert_eq!(
        sessions.session_order(),
        ["session-x", "session-a", "session-b"]
    );
}

#[test]
fn session_catalog_owns_presentation_and_active_history_queries() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    let mut metadata = session_metadata("fallback", None);
    let ssh = SshSessionConfig {
        name: "catalog name".to_string(),
        username: "nya".to_string(),
        host: "example.test".to_string(),
        port: 2222,
        ..SshSessionConfig::default()
    };
    metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(ssh));
    metadata.disconnected = true;
    sessions.register_session_metadata("session-a", metadata);

    let info = sessions
        .session_info("session-a")
        .expect("registered metadata should produce session info");
    assert_eq!(sessions.display_name_by_info(&info), "catalog name");
    sessions.set_dynamic_title("session-a", Some("dynamic title".to_string()));
    assert_eq!(
        sessions.display_name("session-a").as_deref(),
        Some("dynamic title")
    );
    sessions.set_custom_name("session-a".to_string(), "custom name".to_string());
    assert_eq!(
        sessions.display_name("session-a").as_deref(),
        Some("custom name")
    );
    assert_eq!(
        sessions.endpoint("session-a").as_deref(),
        Some("nya@example.test:2222")
    );
    assert_eq!(
        sessions.ssh_host("session-a").as_deref(),
        Some("example.test")
    );
    assert_eq!(
        sessions.ssh_address("session-a").as_deref(),
        Some("ssh -p 2222 nya@example.test")
    );
    assert!(sessions.is_disconnected("session-a"));

    sessions.update_cwd("session-a", "/srv/app".to_string());
    assert_eq!(
        sessions.tab_tooltip_lines("session-a"),
        [
            "nya@example.test:2222",
            "ssh -p 2222 nya@example.test",
            "Disconnected — press Enter to reconnect",
            "cwd /srv/app",
        ]
    );

    sessions.select_active_session("session-a");
    sessions.record_command_history("session-a", "git status");
    sessions.record_command_history("session-a", "cargo check");
    assert_eq!(
        sessions.active_command_history_snapshot(),
        ["cargo check", "git status"]
    );
    assert_eq!(
        sessions.active_command_history_entry(1).as_deref(),
        Some("git status")
    );
}

#[test]
fn active_session_selection_derives_configuration_from_the_catalog() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    let mut metadata = session_metadata("first", None);
    metadata.ssh_config = Some(SshSessionConfig::default());
    metadata.ai_execution_profile = AiExecutionProfile::Auto;
    sessions.register_session_metadata("session-a", metadata);

    assert!(sessions.select_active_session_if_none("session-a"));
    assert_eq!(sessions.active_id(), Some("session-a"));
    assert!(sessions.active_ssh_config().is_some());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::Auto
    );
    assert!(!sessions.select_active_session_if_none("session-b"));

    sessions.register_session_metadata("session-a", session_metadata("updated", None));
    assert_eq!(sessions.active_id(), Some("session-a"));
    assert!(sessions.active_ssh_config().is_none());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::Posix
    );

    assert_eq!(
        sessions.select_active_session("missing").as_deref(),
        Some("session-a")
    );
    assert_eq!(sessions.active_id(), Some("missing"));
    assert!(sessions.active_ssh_config().is_none());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::SendOnly
    );

    assert_eq!(sessions.clear_active_session().as_deref(), Some("missing"));
    assert!(sessions.active_id().is_none());
    assert!(sessions.active_ssh_config().is_none());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::SendOnly
    );
}

#[test]
fn session_disconnect_transition_is_idempotent() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    assert!(sessions.mark_session_disconnected("missing").is_none());
    sessions.register_session_metadata("session-a", session_metadata("first", Some("multiplex-a")));
    sessions
        .register_session_metadata("session-b", session_metadata("second", Some("multiplex-a")));

    let changed = sessions
        .mark_session_disconnected("session-a")
        .expect("registered session should transition");
    assert!(!changed.already_disconnected);
    assert!(sessions.metadata("session-a").unwrap().disconnected);

    let unchanged = sessions
        .mark_session_disconnected("session-a")
        .expect("disconnected session should remain registered");
    assert!(unchanged.already_disconnected);

    sessions
        .mark_session_disconnected("session-b")
        .expect("shared session should transition");
}

#[test]
fn remote_file_service_requires_live_multiplex_for_session_scoped_ssh() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    let config = SshSessionConfig {
        name: "ssh".to_string(),
        host: "example.test".to_string(),
        username: "tester".to_string(),
        ..SshSessionConfig::default()
    };
    let mut metadata = session_metadata("ssh", Some("multiplex-a"));
    metadata.ssh_config = Some(config.clone());
    metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(config.clone()));
    sessions.register_session_metadata("session-a", metadata);

    let error = sessions
        .remote_file_service_for_session(
            "session-a",
            config,
            Arc::new(EmptyRemoteFilePreferenceStore),
        )
        .expect_err("missing session-scoped handle should not fall back to dedicated SSH");
    assert!(error.to_string().contains("reconnect the SSH session"));
}

#[test]
fn reconnect_presentation_migration_preserves_destination_overrides() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    sessions.set_custom_name("old".to_string(), "old name".to_string());
    sessions.set_custom_name("new".to_string(), "new name".to_string());
    sessions.set_dynamic_title("old", Some("old title".to_string()));
    sessions.set_dynamic_title("new", Some("new title".to_string()));
    assert!(sessions.update_cwd("old", "/old".to_string()));
    assert!(!sessions.update_cwd("old", "/old".to_string()));
    assert!(sessions.update_cwd("new", "/new".to_string()));
    sessions.set_tab_color("old", Some(0x112233));
    sessions.set_tab_color("new", Some(0x445566));
    assert!(sessions.set_tab_locked("old", true));
    sessions.record_command_history("old", "pwd");

    sessions.migrate_session_presentation("old", "new");

    assert_eq!(sessions.custom_name("new"), Some("new name"));
    assert_eq!(sessions.dynamic_title("new"), Some("old title"));
    assert!(sessions.dynamic_title("old").is_none());
    assert_eq!(sessions.cwd("new"), Some("/old"));
    assert!(sessions.cwd("old").is_none());
    assert_eq!(sessions.tab_color("new"), Some(0x445566));
    assert!(sessions.tab_is_locked("new"));
    assert!(!sessions.tab_is_locked("old"));
    assert_eq!(
        sessions.command_history_for("new"),
        Some(["pwd".to_string()].as_slice())
    );
    assert!(sessions.command_history_for("old").is_none());
}

#[test]
fn provisional_reconnect_keeps_one_visible_tab_until_replacement() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    sessions.register_session_metadata("old", session_metadata("ssh", None));

    sessions.register_provisional_reconnect("new", session_metadata("ssh", None));
    assert!(sessions.has_session("new"));
    assert_eq!(sessions.session_order(), &["old".to_string()]);

    sessions.replace_session_order_id("old", "new");
    sessions.remove_session_catalog("old");
    assert_eq!(sessions.session_order(), &["new".to_string()]);
}

#[test]
fn removing_session_catalog_clears_all_session_scoped_entries() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    let mut metadata = session_metadata("first", Some("multiplex-a"));
    metadata.ssh_config = Some(SshSessionConfig::default());
    metadata.ai_execution_profile = AiExecutionProfile::Auto;
    sessions.register_session_metadata("session-a", metadata);
    sessions.set_custom_name("session-a".to_string(), "custom".to_string());
    sessions.set_dynamic_title("session-a", Some("dynamic".to_string()));
    sessions.update_cwd("session-a", "/workspace".to_string());
    sessions.set_tab_color("session-a", Some(0x112233));
    assert!(sessions.set_tab_locked("session-a", true));
    sessions.record_command_history("session-a", "pwd");
    assert!(sessions.begin_reconnect_action("session-a".to_string()));
    sessions.zmodem_state_mut_or_default("session-a");
    sessions.trzsz_state_mut_or_default("session-a");
    assert_eq!(sessions.protocol_runtime_counts(), (1, 1, 0));
    sessions.select_active_session("session-a");
    assert!(sessions.active_ssh_config().is_some());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::Auto
    );
    assert_eq!(
        sessions.remove_session_catalog("session-a").as_deref(),
        Some("multiplex-a")
    );
    assert!(!sessions.has_session("session-a"));
    assert!(sessions.session_order().is_empty());
    assert!(sessions.custom_name("session-a").is_none());
    assert!(sessions.dynamic_title("session-a").is_none());
    assert!(sessions.cwd("session-a").is_none());
    assert!(sessions.tab_color("session-a").is_none());
    assert!(!sessions.tab_is_locked("session-a"));
    assert!(sessions.command_history_for("session-a").is_none());
    assert!(!sessions.session_is_busy("session-a"));
    assert!(sessions.active_id().is_none());
    assert!(sessions.active_ssh_config().is_none());
    assert_eq!(
        sessions.active_ai_execution_profile(),
        AiExecutionProfile::SendOnly
    );
    assert_eq!(sessions.protocol_runtime_counts(), (0, 0, 0));
}

#[test]
fn closing_the_active_session_hands_over_to_the_most_recent_visit() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    sessions.register_session_metadata("session-a", session_metadata("first", None));
    sessions.register_session_metadata("session-b", session_metadata("second", None));
    sessions.register_session_metadata("session-c", session_metadata("third", None));
    assert_eq!(
        sessions.session_order(),
        ["session-a", "session-b", "session-c"]
    );

    sessions.select_active_session("session-a");
    sessions.select_active_session("session-b");
    sessions.select_active_session("session-c");

    // Tab order alone would jump to the leftmost tab; the user came from B.
    sessions.remove_session_catalog("session-c");
    assert!(sessions.active_id().is_none());
    assert_eq!(
        sessions.next_session_after("session-c").as_deref(),
        Some("session-b")
    );
    assert_eq!(sessions.next_live_session().as_deref(), Some("session-b"));

    // Re-activating a session moves it off the stack instead of duplicating it, so
    // A -> B -> A hands back to B rather than to A itself.
    sessions.select_active_session("session-b");
    sessions.select_active_session("session-a");
    sessions.select_active_session("session-b");
    sessions.remove_session_catalog("session-b");
    assert_eq!(
        sessions.next_session_after("session-b").as_deref(),
        Some("session-a")
    );

    sessions.remove_session_catalog("session-a");
    assert!(sessions.next_session_after("session-a").is_none());
    assert!(sessions.next_live_session().is_none());
}

#[test]
fn session_handover_falls_back_to_tab_order_without_activation_history() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    sessions.register_session_metadata("session-a", session_metadata("first", None));
    sessions.register_session_metadata("session-b", session_metadata("second", None));

    assert_eq!(
        sessions.next_session_after("session-a").as_deref(),
        Some("session-b")
    );

    // A visit to a session that is closed later leaves no stale handover target.
    sessions.select_active_session("session-b");
    sessions.select_active_session("session-a");
    sessions.remove_session_catalog("session-b");
    assert_eq!(sessions.next_session_after("session-a"), None);
}

#[test]
fn tab_lock_and_drag_state_have_single_owner() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());

    assert!(sessions.set_tab_locked("tab-a", true));
    assert!(sessions.tab_is_locked("tab-a"));
    assert!(!sessions.set_tab_locked("tab-a", true));
    assert!(sessions.set_tab_locked("tab-a", false));
    assert!(!sessions.tab_is_locked("tab-a"));

    assert!(sessions.set_tab_drag_source("tab-a".to_string()));
    assert!(sessions.tab_drag_source_is("tab-a"));
    assert!(sessions.tab_drag_is_pending());
    assert!(!sessions.set_tab_drag_source("tab-a".to_string()));
    assert!(sessions.clear_tab_drag());
    assert!(!sessions.clear_tab_drag());

    sessions.set_tab_drag_source("tab-a".to_string());
    sessions.remove_session_catalog("tab-a");
    assert!(!sessions.tab_drag_is_pending());
}

#[test]
fn closing_a_split_root_can_migrate_logical_tab_presentation() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    sessions.set_custom_name("root".to_string(), "logical tab".to_string());
    sessions.set_custom_name("survivor".to_string(), "pane".to_string());
    sessions.set_tab_color("root", Some(0x112233));
    sessions.set_tab_locked("root", true);

    sessions.migrate_tab_root_presentation("root", "survivor");

    assert_eq!(sessions.custom_name("survivor"), Some("logical tab"));
    assert_eq!(sessions.tab_color("survivor"), Some(0x112233));
    assert!(sessions.tab_is_locked("survivor"));
    assert!(sessions.custom_name("root").is_none());
    assert!(sessions.tab_color("root").is_none());
    assert!(!sessions.tab_is_locked("root"));
}

#[test]
fn tab_group_reorder_moves_split_sessions_as_one_block() {
    let test_dir = TestConfigDir::new("nyaterm-session-state-test");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, test_dir.path());
    for id in ["a", "a-child", "b", "c", "c-child"] {
        sessions.register_session_metadata(id, session_metadata(id, None));
    }

    assert!(sessions.move_session_group_relative(
        &["a".to_string(), "a-child".to_string()],
        &["c".to_string(), "c-child".to_string()],
        true,
    ));
    assert_eq!(
        sessions.session_order(),
        ["b", "c", "c-child", "a", "a-child"]
    );
    assert!(sessions.move_session_group_to_end(&["b".to_string()]));
    assert_eq!(
        sessions.session_order(),
        ["c", "c-child", "a", "a-child", "b"]
    );
}

#[test]
fn session_start_state_owns_channel_selection_and_cancellation() {
    let mut starts = SessionStartFeatureState::new();
    starts
        .pending
        .insert("request-1".to_string(), pending("local shell"));

    assert!(starts.select_pending("request-1"));
    assert_eq!(
        starts.pending_display_name().as_deref(),
        Some("local shell")
    );

    let mut rx = starts
        .take_event_receiver()
        .expect("the state holds its receiver until the drain starts");
    starts
        .sender()
        .unbounded_send(SessionStartResult {
            request_id: "request-1".to_string(),
            connection_name: "local shell".to_string(),
            kind: SessionKind::LocalPty,
            worker_started_at: Instant::now(),
            worker_finished_at: Instant::now(),
            result: Err("cancelled".to_string()),
        })
        .expect("session start event channel should stay connected");
    assert_eq!(
        rx.try_recv()
            .expect("session start result should reach its owner")
            .request_id,
        "request-1"
    );

    let closed = starts
        .close_pending("request-1")
        .expect("selected pending start should close");
    assert_eq!(closed.connection_name, "local shell");
    assert!(starts.has_cancelled_results());
    assert!(matches!(
        starts.take_event_request("request-1"),
        SessionStartEventRequest::Cancelled
    ));
    assert!(!starts.has_cancelled_results());
    assert!(!starts.has_pending());
    assert!(!starts.has_active_pending());
}

#[test]
fn session_start_registration_owns_fresh_and_reconnect_selection() {
    let mut fresh = SessionStartFeatureState::new();
    assert!(!fresh.register_pending("request-fresh".to_string(), pending("fresh")));
    assert!(fresh.request_is_active("request-fresh"));
    assert_eq!(fresh.pending_count(), 1);

    let mut reconnect = SessionStartFeatureState::new();
    let mut reconnect_request = pending("reconnect");
    reconnect_request.reconnect_session_id = Some("session-old".to_string());
    assert!(reconnect.register_pending("request-reconnect".to_string(), reconnect_request));
    assert!(!reconnect.has_active_pending());
    assert!(reconnect.reconnect_is_pending("session-old"));
}

#[test]
fn concurrent_session_starts_keep_the_latest_request_active() {
    let mut starts = SessionStartFeatureState::new();
    starts.register_pending("request-a".to_string(), pending("slow-a"));
    starts.register_pending("request-b".to_string(), pending("fast-b"));

    assert_eq!(starts.pending_count(), 2);
    assert!(!starts.request_is_active("request-a"));
    assert!(starts.request_is_active("request-b"));

    let SessionStartEventRequest::Pending {
        was_active: a_was_active,
        ..
    } = starts.take_event_request("request-a")
    else {
        panic!("request A should remain independently addressable");
    };
    assert!(!a_was_active);
    assert!(!starts.complete_success(a_was_active, false));
    assert!(starts.request_is_active("request-b"));

    let SessionStartEventRequest::Pending {
        was_active: b_was_active,
        ..
    } = starts.take_event_request("request-b")
    else {
        panic!("request B should remain independently addressable");
    };
    assert!(b_was_active);
    assert!(starts.complete_success(b_was_active, false));
}

#[test]
fn older_success_does_not_replace_the_selected_newer_failure() {
    let mut starts = SessionStartFeatureState::new();
    starts.register_pending("request-a".to_string(), pending("slow-a"));
    starts.register_pending("request-b".to_string(), pending("fast-b"));

    let SessionStartEventRequest::Pending {
        pending: pending_b,
        was_active: b_was_active,
    } = starts.take_event_request("request-b")
    else {
        panic!("request B should remain independently addressable");
    };
    assert!(!starts.record_failure(
        "request-b".to_string(),
        pending_b,
        "connection failed".to_string(),
        b_was_active,
        false,
    ));
    assert!(starts.request_is_active("request-b"));

    let SessionStartEventRequest::Pending {
        was_active: a_was_active,
        ..
    } = starts.take_event_request("request-a")
    else {
        panic!("request A should remain independently addressable");
    };
    assert!(!a_was_active);
    assert!(!starts.complete_success(a_was_active, true));
    assert!(starts.request_is_active("request-b"));
}

#[test]
fn session_start_results_route_normal_and_reconnect_failures_atomically() {
    let mut fresh = SessionStartFeatureState::new();
    fresh.register_pending("request-fresh".to_string(), pending("fresh"));
    let SessionStartEventRequest::Pending {
        pending: pending_state,
        was_active,
    } = fresh.take_event_request("request-fresh")
    else {
        panic!("fresh result should retain pending metadata");
    };
    assert!(was_active);
    assert!(!fresh.record_failure(
        "request-fresh".to_string(),
        pending_state,
        "connection failed".to_string(),
        was_active,
        false,
    ));
    assert!(fresh.has_failed());
    assert!(fresh.has_active_failed());
    assert_eq!(
        fresh
            .active_failed()
            .expect("active failure should be retained")
            .error,
        "connection failed"
    );

    let mut reconnect = SessionStartFeatureState::new();
    let mut reconnect_request = pending("reconnect");
    reconnect_request.reconnect_session_id = Some("session-old".to_string());
    reconnect.register_pending("request-reconnect".to_string(), reconnect_request);
    let SessionStartEventRequest::Pending {
        pending: pending_state,
        was_active,
    } = reconnect.take_event_request("request-reconnect")
    else {
        panic!("reconnect result should retain pending metadata");
    };
    assert!(!was_active);
    assert!(reconnect.record_failure(
        "request-reconnect".to_string(),
        pending_state,
        "reconnect failed".to_string(),
        was_active,
        true,
    ));
    assert!(!reconnect.has_failed());
    assert_eq!(
        reconnect.reconnect_failure("session-old"),
        Some("reconnect failed")
    );
}

#[test]
fn session_start_success_and_workspace_split_are_single_owner_transitions() {
    let mut starts = SessionStartFeatureState::new();
    assert!(!starts.complete_success(false, false));

    starts.set_pending_workspace_split(
        WorkspaceSplitDirection::Horizontal,
        "session-source".to_string(),
    );
    assert!(matches!(
        starts.take_pending_workspace_split(),
        Some((WorkspaceSplitDirection::Horizontal, source)) if source == "session-source"
    ));
    assert!(starts.take_pending_workspace_split().is_none());
}

#[test]
fn session_start_state_reserves_saved_connections_during_preparation() {
    let mut starts = SessionStartFeatureState::new();
    let connection_1 = starts.allocate_tab_placement(0);
    let connection_2 = starts.allocate_tab_placement(1);

    assert!(starts.reserve_saved_connection_start("connection-1", connection_1));
    assert!(starts.saved_connection_is_preparing("connection-1"));
    assert!(!starts.reserve_saved_connection_start("connection-1", connection_1));
    assert!(starts.reserve_saved_connection_start("connection-2", connection_2));
    assert_eq!(starts.visible_tab_reservation_count(), 2);
    assert_eq!(connection_1.insert_index, 0);
    assert_eq!(connection_2.insert_index, 1);
    assert!(connection_1.request_sequence < connection_2.request_sequence);

    starts.release_saved_connection_start("connection-1");
    assert!(!starts.saved_connection_is_preparing("connection-1"));
    assert!(starts.reserve_saved_connection_start("connection-1", connection_1));
}

#[test]
fn pending_registration_promotes_saved_preparation_without_changing_placement() {
    let mut starts = SessionStartFeatureState::new();
    let placement = starts.allocate_tab_placement(3);
    assert!(starts.reserve_saved_connection_start("connection-1", placement));

    let mut request = pending("saved");
    request.source_connection_id = Some("connection-1".to_string());
    request.tab_placement = Some(placement);
    starts.register_pending("request-1".to_string(), request);

    assert!(!starts.saved_connection_is_preparing("connection-1"));
    assert_eq!(starts.visible_tab_reservation_count(), 1);
    let pending = starts
        .pending
        .get("request-1")
        .expect("saved preparation should become pending");
    assert_eq!(
        pending
            .tab_placement
            .expect("pending start should retain its tab placement")
            .insert_index,
        3
    );
}

#[test]
fn unrelated_start_from_same_connection_does_not_release_preparation() {
    let mut starts = SessionStartFeatureState::new();
    let preparing = starts.allocate_tab_placement(0);
    let duplicate = starts.allocate_tab_placement(1);
    assert!(starts.reserve_saved_connection_start("connection-1", preparing));

    let mut duplicate_request = pending("duplicate");
    duplicate_request.source_connection_id = Some("connection-1".to_string());
    duplicate_request.tab_placement = Some(duplicate);
    starts.register_pending("request-duplicate".to_string(), duplicate_request);

    assert!(starts.saved_connection_is_preparing("connection-1"));
    assert_eq!(starts.visible_tab_reservation_count(), 2);
}

#[test]
fn reconnect_targets_are_owned_by_each_pending_request() {
    let mut starts = SessionStartFeatureState::new();
    let mut reconnect_a = pending("reconnect-a");
    reconnect_a.reconnect_session_id = Some("session-a".to_string());
    let mut reconnect_b = pending("reconnect-b");
    reconnect_b.reconnect_session_id = Some("session-b".to_string());

    assert!(starts.register_pending("request-a".to_string(), reconnect_a));
    assert!(starts.register_pending("request-b".to_string(), reconnect_b));
    assert!(starts.reconnect_is_pending("session-a"));
    assert!(starts.reconnect_is_pending("session-b"));

    assert!(!starts.register_pending("request-fresh".to_string(), pending("fresh")));
    assert!(starts.request_is_active("request-fresh"));
    assert!(
        starts
            .pending
            .get("request-fresh")
            .is_some_and(|pending| pending.reconnect_session_id.is_none())
    );
}

#[test]
fn reconnect_stays_pending_until_frame_migration_finishes() {
    let mut starts = SessionStartFeatureState::new();
    starts.record_reconnect_completion(PendingReconnectCompletion {
        old_id: "old".to_string(),
        new_id: "new".to_string(),
        connection_name: "ssh".to_string(),
        kind: SessionKind::Ssh,
        source_connection_id: None,
        workspace_split: None,
        startup_command: None,
    });

    assert!(starts.reconnect_is_pending("old"));
    assert!(starts.take_reconnect_completion("new").is_some());
    assert!(!starts.reconnect_is_pending("old"));
}

#[test]
fn closing_pending_starts_preserves_non_reconnect_and_failed_fallback_order() {
    let mut starts = SessionStartFeatureState::new();
    starts
        .pending
        .insert("request-active".to_string(), pending("active"));
    starts
        .pending
        .insert("request-normal".to_string(), pending("normal"));
    let mut reconnect = pending("reconnect");
    reconnect.reconnect_session_id = Some("old-session".to_string());
    starts
        .pending
        .insert("request-reconnect".to_string(), reconnect);
    starts.failed.insert(
        "request-failed".to_string(),
        FailedSessionStart {
            pending: pending("failed"),
            error: "failed".to_string(),
            reason: super::SessionStartEndReason::Failed,
        },
    );

    assert!(starts.select_pending("request-active"));
    starts
        .close_pending("request-active")
        .expect("active start should close");
    assert_eq!(starts.active_pending.as_deref(), Some("request-normal"));
    assert!(starts.active_failed.is_none());

    starts
        .close_pending("request-normal")
        .expect("normal start should close");
    assert!(starts.active_pending.is_none());
    assert_eq!(starts.active_failed.as_deref(), Some("request-failed"));
    assert!(starts.pending.contains_key("request-reconnect"));
}

#[test]
fn moved_ssh_connection_survives_source_shutdown_and_repeated_transfers() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let source_dir = TestConfigDir::new("nyaterm-ssh-move-source");
    let target_dir = TestConfigDir::new("nyaterm-ssh-move-target");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut source = session_state(&cx, source_dir.path());
    let pool = source.ssh_connection_pool();
    let mut target = session_state_with_runtime(
        &cx,
        target_dir.path(),
        source.manager_handle(),
        pool.clone(),
    );
    let count = Arc::new(AtomicUsize::new(0));
    let disconnected = Arc::clone(&count);
    let connection = pool.register_for_test("ssh-shared", move || {
        disconnected.fetch_add(1, Ordering::SeqCst);
    });
    source.register_session_metadata("moved", session_metadata("ssh", Some("ssh-shared")));
    source.register_ssh_connection("moved".into(), connection);
    let ids = vec!["moved".into()];

    // The bundle itself owns the lease, including while a failed attach rolls back.
    let events = source.pause_sessions_for_transfer(&ids);
    let bundle = source.detach_sessions_for_transfer(&ids, events).unwrap();
    assert!(source.protocols.ssh_connections.is_empty());
    pool.finish_disconnects();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    source.attach_sessions_from_transfer(bundle, None);

    for _ in 0..2 {
        let events = source.pause_sessions_for_transfer(&ids);
        let bundle = source.detach_sessions_for_transfer(&ids, events).unwrap();
        target.attach_sessions_from_transfer(bundle, None);
        assert!(target.ssh_connection_for_session("moved").is_some());
        let events = target.pause_sessions_for_transfer(&ids);
        let bundle = target.detach_sessions_for_transfer(&ids, events).unwrap();
        source.attach_sessions_from_transfer(bundle, None);
    }
    let events = source.pause_sessions_for_transfer(&ids);
    let bundle = source.detach_sessions_for_transfer(&ids, events).unwrap();
    target.attach_sessions_from_transfer(bundle, None);
    source.shutdown_workers();
    drop(source);
    pool.finish_disconnects();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(target.ssh_connection_for_session("moved").is_some());
    target.shutdown_workers();
    pool.finish_disconnects();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn closing_and_disconnecting_sessions_in_other_windows_release_only_their_leases() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let source_dir = TestConfigDir::new("nyaterm-ssh-shared-source");
    let target_dir = TestConfigDir::new("nyaterm-ssh-shared-target");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut source = session_state(&cx, source_dir.path());
    let pool = source.ssh_connection_pool();
    let mut target = session_state_with_runtime(
        &cx,
        target_dir.path(),
        source.manager_handle(),
        pool.clone(),
    );
    let count = Arc::new(AtomicUsize::new(0));
    let disconnected = Arc::clone(&count);
    let connection = pool.register_for_test("shared", move || {
        disconnected.fetch_add(1, Ordering::SeqCst);
    });
    for id in ["first", "second", "third"] {
        source.register_session_metadata(id, session_metadata(id, Some("shared")));
        source.register_ssh_connection(id.into(), connection.clone());
    }
    drop(connection);
    let ids = vec!["third".into()];
    let events = source.pause_sessions_for_transfer(&ids);
    let bundle = source.detach_sessions_for_transfer(&ids, events).unwrap();
    target.attach_sessions_from_transfer(bundle, None);
    source.mark_session_disconnected("first").unwrap();
    assert!(source.ssh_connection_for_session("first").is_none());
    source.remove_session_catalog("second");
    source.shutdown_workers();
    pool.finish_disconnects();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(target.ssh_connection_for_session("third").is_some());
    target.mark_session_disconnected("third").unwrap();
    pool.finish_disconnects();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    target.mark_session_disconnected("third").unwrap();
    target.remove_session_catalog("third");
    pool.finish_disconnects();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn window_shutdown_marks_pending_starts_cancelled_before_late_results_arrive() {
    let dir = TestConfigDir::new("nyaterm-late-start-shutdown");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, dir.path());
    sessions
        .start
        .register_pending("pending".into(), pending("ssh"));
    sessions.shutdown_workers();
    assert!(!sessions.start.has_pending());
    assert!(sessions.start.sender().is_closed());
    assert!(matches!(
        sessions.start.take_event_request("pending"),
        SessionStartEventRequest::Cancelled
    ));
}

#[test]
fn reconnect_releases_old_lease_without_disconnecting_shared_sibling_or_replacement() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = TestConfigDir::new("nyaterm-ssh-reconnect-leases");
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut sessions = session_state(&cx, dir.path());
    let pool = sessions.ssh_connection_pool();
    let old_disconnects = Arc::new(AtomicUsize::new(0));
    let old_count = Arc::clone(&old_disconnects);
    let old = pool.register_for_test("old-connection", move || {
        old_count.fetch_add(1, Ordering::SeqCst);
    });
    for id in ["old", "sibling"] {
        sessions.register_session_metadata(id, session_metadata(id, Some("old-connection")));
        sessions.register_ssh_connection(id.into(), old.clone());
    }
    drop(old);
    sessions.mark_session_disconnected("old").unwrap();
    let new_disconnects = Arc::new(AtomicUsize::new(0));
    let new_count = Arc::clone(&new_disconnects);
    let new = pool.register_for_test("new-connection", move || {
        new_count.fetch_add(1, Ordering::SeqCst);
    });
    sessions.register_ssh_connection("new".into(), new);
    sessions.register_provisional_reconnect("new", session_metadata("new", Some("new-connection")));
    sessions.replace_session_order_id("old", "new");
    sessions.remove_session_catalog("old");
    pool.finish_disconnects();
    assert_eq!(old_disconnects.load(Ordering::SeqCst), 0);
    assert_eq!(new_disconnects.load(Ordering::SeqCst), 0);
    assert!(sessions.ssh_connection_for_session("new").is_some());
    assert!(sessions.ssh_connection_for_session("sibling").is_some());
    sessions.remove_session_catalog("sibling");
    pool.finish_disconnects();
    assert_eq!(old_disconnects.load(Ordering::SeqCst), 1);
    assert_eq!(new_disconnects.load(Ordering::SeqCst), 0);
    sessions.shutdown_workers();
    pool.finish_disconnects();
    assert_eq!(new_disconnects.load(Ordering::SeqCst), 1);
}

#[test]
fn telnet_reconnect_request_retains_custom_keepalive_through_event_routing() {
    let mut starts = SessionStartFeatureState::new();
    let mut request = pending("Telnet reconnect");
    request.reconnect_session_id = Some("old-telnet".into());
    request.launch_config = Some(SessionLaunchConfig::Telnet(
        nyaterm_transport::TelnetSessionConfig {
            keep_alive_interval_secs: 45,
            ..Default::default()
        },
    ));
    starts.register_pending("telnet-request".into(), request);
    assert!(starts.reconnect_is_pending("old-telnet"));
    let SessionStartEventRequest::Pending { pending, .. } =
        starts.take_event_request("telnet-request")
    else {
        panic!("Pending reconnect expected");
    };
    let pending = pending.expect("registered request");
    assert_eq!(pending.reconnect_session_id.as_deref(), Some("old-telnet"));
    let Some(SessionLaunchConfig::Telnet(config)) = pending.launch_config else {
        panic!("Telnet launch config expected");
    };
    assert_eq!(config.keep_alive_interval_secs, 45);
}

#[test]
fn zmodem_spawn_failure_preserves_ui_pending_and_bridge_consumer() {
    let root = TestConfigDir::new("nyaterm-zmodem-spawn-failure");
    let cx = TestAppContext::single();
    let mut state = session_state(&cx, root.path());
    state.extend_pending_events([SessionEvent::Output {
        session_id: "z".into(),
        data: b"pending".to_vec(),
    }]);
    let _failure = nyaterm_core::runtime::worker::fail_after(0);
    assert!(
        state
            .start_zmodem_runtime(
                "z",
                nyaterm_transport::ZmodemTransfer::new(
                    nyaterm_transport::ZmodemDirection::Download,
                    &[]
                )
            )
            .is_err()
    );
    assert_eq!(state.pending_event_count(), 1);
    assert!(
        matches!(state.pop_pending_event(), Some(SessionEvent::Output { data, .. }) if data == b"pending")
    );
}

#[test]
fn zmodem_handoff_preserves_ui_then_bridge_order_and_other_sessions() {
    let root = TestConfigDir::new("nyaterm-zmodem-handoff-order");
    let cx = TestAppContext::single();
    let mut state = session_state(&cx, root.path());
    state.extend_pending_events([
        SessionEvent::Output {
            session_id: "z".into(),
            data: b"ui".to_vec(),
        },
        SessionEvent::Output {
            session_id: "other".into(),
            data: b"other".to_vec(),
        },
    ]);
    state.push_event_bridge_ui_event_for_test(SessionEvent::Output {
        session_id: "z".into(),
        data: b"bridge".to_vec(),
    });
    let worker = state
        .start_zmodem_runtime(
            "z",
            nyaterm_transport::ZmodemTransfer::new(
                nyaterm_transport::ZmodemDirection::Download,
                &[],
            ),
        )
        .unwrap();
    assert_eq!(state.pending_event_count(), 1);
    worker.cancel("test");
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    let result = loop {
        if let Some(event) = worker.try_recv_event()
            && event.done
        {
            break event;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    let bytes: Vec<u8> = result
        .pending_events
        .iter()
        .flat_map(|event| match event {
            SessionEvent::Output { data, .. } => data.clone(),
            _ => panic!("output"),
        })
        .collect();
    assert_eq!(bytes, b"uibridge");
    state.finish_zmodem_runtime("z", result.pending_events);
    assert_eq!(state.pending_event_count(), 3);
}

mod tab_actions;

#[test]
fn independent_rdp_instances_can_reference_the_same_saved_connection() {
    let root = TestConfigDir::new("nyaterm-rdp-multiple-instances");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let first = app_with_visible_local_session(&mut cx, root.path(), "local");
    let other_root = TestConfigDir::new("nyaterm-rdp-other-window");
    let second = app_with_visible_local_session(&mut cx, other_root.path(), "other");
    first.update(&mut cx, |app, _| {
        app.session
            .register_session_metadata("rdp-one", remote_desktop_metadata("saved-rdp"));
        app.session
            .register_session_metadata("rdp-two", remote_desktop_metadata("saved-rdp"));
        assert!(app.session.has_session("rdp-one"));
        assert!(app.session.has_session("rdp-two"));
        app.session.remove_session_catalog("rdp-one");
        assert!(app.session.has_session("rdp-two"));
    });
    second.update(&mut cx, |app, _| {
        app.session
            .register_session_metadata("rdp-three", remote_desktop_metadata("saved-rdp"));
        assert!(app.session.has_session("rdp-three"));
    });
}
