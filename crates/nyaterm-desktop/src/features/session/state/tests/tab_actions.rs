use std::time::Instant;

use gpui::{AppContext as _, MouseButton, TestAppContext};
use nyaterm_transport::{LocalSessionConfig, SessionInfo, SessionKind};

use crate::features::runtime_jobs::{SessionStartResult, SessionStartSuccess};
use crate::features::shell::tab_mouse::{TAB_MOUSE_ACTIONS, TabActionTarget};
use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use crate::models::SessionLaunchConfig;
use crate::test_support::TestConfigDir;

use super::super::{
    SavedConnectionStartOptions, SessionStartEndReason, SessionStartEventRequest,
    SessionStartFeatureState,
};
use super::{failed_start, pending};

#[test]
fn start_tab_cancellation_retains_metadata_and_discards_the_old_worker_result() {
    let mut starts = SessionStartFeatureState::new();
    let placement = starts.allocate_tab_placement(3);
    let mut request = pending("connection");
    request.tab_placement = Some(placement);
    request.custom_name = Some("custom".into());
    request.source_connection_id = Some("saved".into());
    request.tab_color = Some(0x123456);
    request.locked = true;
    let attempt = request.attempt.clone();
    starts.register_pending("old".into(), request);
    assert!(starts.cancel_pending("old", "cancelled".into()));
    assert!(!starts.cancel_pending("old", "cancelled again".into()));
    assert!(attempt.check().is_err());
    assert_eq!(starts.active_failed_request_id(), Some("old"));
    let failed = starts.failed("old").unwrap();
    assert_eq!(failed.reason, SessionStartEndReason::Cancelled);
    assert_eq!(failed.pending.custom_name.as_deref(), Some("custom"));
    assert_eq!(failed.pending.tab_color, Some(0x123456));
    assert!(failed.pending.locked);
    assert_eq!(
        failed.pending.tab_placement.unwrap().request_sequence,
        placement.request_sequence
    );
    let mut request = starts.take_failed_for_retry("old").unwrap().pending;
    request.attempt = Default::default();
    starts.begin_preparation("new".into(), request);
    assert!(matches!(
        starts.take_event_request("old"),
        SessionStartEventRequest::Cancelled
    ));
    assert!(starts.failed("old").is_none());
    assert!(starts.attempt("new").check().is_ok());
    assert_eq!(starts.pending_count(), 1);
}

#[test]
fn start_tab_cancellation_releases_only_its_own_preparation_and_invalidates_callbacks() {
    for owns_reservation in [true, false] {
        let mut starts = SessionStartFeatureState::new();
        let placement = starts.allocate_tab_placement(0);
        let other = starts.allocate_tab_placement(1);
        if !owns_reservation {
            let mut sibling = pending("sibling");
            sibling.source_connection_id = Some("saved".into());
            sibling.tab_placement = Some(other);
            starts.begin_preparation("sibling".into(), sibling);
        }
        let mut request = pending("connection");
        request.tab_placement = Some(placement);
        request.source_connection_id = Some("saved".into());
        starts.begin_preparation("preparing".into(), request);
        let options = SavedConnectionStartOptions {
            request_id: Some("preparing".into()),
            ..Default::default()
        };
        assert!(starts.cancel_pending("preparing", "cancelled".into()));
        assert!(!starts.request_can_start(&options));
        assert_eq!(
            starts.saved_connection_is_preparing("saved"),
            !owns_reservation
        );
        assert!(
            !starts.has_cancelled_results(),
            "preparation has no worker to reap"
        );
        starts.close_failed("preparing").unwrap();
        assert!(!starts.has_failed());
    }
}

#[test]
fn start_tab_cancelled_worker_failure_cannot_replace_a_fresh_retry() {
    let root = TestConfigDir::new("nyaterm-start-tab-stale-failure");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    app.update(&mut cx, |app, cx| {
        let mut request = pending("original");
        request.custom_name = Some("renamed".into());
        app.session.start.register_pending("old".into(), request);
        let target = app.session.start_request_target("old").unwrap();
        assert!(app.session.start.cancel_pending("old", "cancelled".into()));
        let mut request = app
            .session
            .start
            .take_failed_for_retry("old")
            .unwrap()
            .pending;
        request.attempt = Default::default();
        app.session.start.begin_preparation("new".into(), request);
        let now = Instant::now();
        app.apply_session_start_event(
            SessionStartResult {
                request_id: "old".into(),
                connection_name: "stale".into(),
                kind: SessionKind::LocalPty,
                worker_started_at: now,
                worker_finished_at: now,
                result: Err("late failure".into()),
            },
            cx,
        );
        assert!(!app.session.start.has_failed());
        assert_eq!(app.session.start.pending_count(), 1);
        assert_eq!(app.session.start_action_request(&target).unwrap().0, "new");
        assert_eq!(
            app.session.tab_action_display_name(&target).as_deref(),
            Some("renamed")
        );
        assert!(app.session.start.attempt("new").check().is_ok());
    });
}

#[test]
fn start_tab_saved_ssh_address_and_restart_capability_follow_available_configuration() {
    let root = TestConfigDir::new("nyaterm-start-tab-ssh-config");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    app.update(&mut cx, |app, _| {
        let connection = serde_json::from_value::<nyaterm_core::SavedConnection>(serde_json::json!({
            "id": "saved", "name": "SSH", "type": "ssh", "host": "saved.example", "port": 22, "username": "user"
        })).unwrap();
        app.connection_state.replace_loaded(vec![connection], Vec::new());
        failed_start(app, "failed", 1, None);
        app.session.start.failed.get_mut("failed").unwrap().pending.source_connection_id = Some("saved".into());
        let target = app.session.start_request_target("failed").unwrap();
        let context = app.start_tab_action_context(&target).unwrap();
        assert!(context.can_restart);
        assert_eq!(context.ssh_host.as_deref(), Some("saved.example"));
        app.connection_state.replace_loaded(Vec::new(), Vec::new());
        let context = app.start_tab_action_context(&target).unwrap();
        assert!(!context.can_restart);
        assert!(context.ssh_host.is_none());
        app.session.start.failed.get_mut("failed").unwrap().pending.launch_config = Some(
            SessionLaunchConfig::Ssh(Box::new(nyaterm_transport::SshSessionConfig {
                host: "retained.example".into(), ..Default::default()
            })));
        let context = app.start_tab_action_context(&target).unwrap();
        assert!(context.can_restart);
        assert_eq!(context.ssh_host.as_deref(), Some("retained.example"));
    });
}

fn success(request_id: &str, session_id: &str, config: SessionLaunchConfig) -> SessionStartResult {
    let now = Instant::now();
    SessionStartResult {
        request_id: request_id.into(),
        connection_name: "connected".into(),
        kind: SessionKind::LocalPty,
        worker_started_at: now,
        worker_finished_at: now,
        result: Ok(SessionStartSuccess {
            session_info: SessionInfo {
                id: session_id.into(),
                name: "connected".into(),
                kind: SessionKind::LocalPty,
                working_dir: None,
                cols: 80,
                rows: 24,
            },
            multiplex_handle: None,
            launch_config: Some(config),
            cleanup: None,
        }),
    }
}

#[test]
fn start_tab_rename_follows_retry_and_success_without_changing_the_active_other_session() {
    let root = TestConfigDir::new("nyaterm-start-tab-rename");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    let cx = session_focus_test_window(&mut cx, &app);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let config = SessionLaunchConfig::Local(LocalSessionConfig::default());
            failed_start(app, "failed", 1, Some(config.clone()));
            let target = app.session.start_request_target("failed").unwrap();
            app.session
                .dialogs
                .open_tab_rename(target.clone(), "before");
            let failed = app.session.start.take_failed_for_retry("failed").unwrap();
            app.session
                .start
                .begin_preparation("retry".into(), failed.pending);
            let options = SavedConnectionStartOptions {
                custom_name: Some("stale snapshot".into()),
                request_id: Some("retry".into()),
                ..Default::default()
            };
            app.session
                .dialogs
                .apply_text_input("rename", "  during preparation  ".into());
            assert!(app.submit_rename_session(cx));
            assert_eq!(
                app.prepare_session_start_options(options)
                    .custom_name
                    .as_deref(),
                Some("during preparation")
            );
            app.session
                .dialogs
                .open_tab_rename(target.clone(), "during preparation");
            app.session
                .dialogs
                .apply_text_input("rename", "after success".into());
            app.apply_session_start_event(success("retry", "connected", config), cx);
            assert_eq!(
                app.session.custom_name("connected"),
                Some("during preparation")
            );
            app.select_session("other".into(), cx);
            assert!(app.submit_rename_session(cx));
            assert_eq!(app.session.custom_name("connected"), Some("after success"));
            assert_eq!(app.session.custom_name("other"), None);
            assert_eq!(app.session.active_id(), Some("other"));
            app.session.dialogs.open_tab_rename(
                TabActionTarget::Session("connected".into()),
                "after success",
            );
            app.session.remove_session_catalog("connected");
            app.session
                .dialogs
                .apply_text_input("rename", String::new());
            assert!(
                app.submit_rename_session(cx),
                "closing the target also ends an empty rename"
            );
            assert!(app.session.dialogs.rename_target().is_none());
            assert_eq!(app.session.custom_name("other"), None);
        })
    });
}

#[test]
fn start_tab_actions_follow_state_and_configuration_and_never_fall_back_to_another_session() {
    let root = TestConfigDir::new("nyaterm-start-tab-policy");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    let cx = session_focus_test_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            failed_start(
                app,
                "failed",
                1,
                Some(SessionLaunchConfig::Local(LocalSessionConfig::default())),
            );
            let target = app.session.start_request_target("failed").unwrap();
            let expected = [false, true, true, false, true, false, true, false, true];
            for (action, enabled) in TAB_MOUSE_ACTIONS.iter().zip(expected) {
                assert_eq!(
                    app.tab_target_action_enabled(&target, action),
                    enabled,
                    "failed: {action}"
                );
            }
            let failed = app.session.start.take_failed_for_retry("failed").unwrap();
            app.session
                .start
                .begin_preparation("retry".into(), failed.pending);
            let expected = [false, true, true, false, false, false, false, true, true];
            for (action, enabled) in TAB_MOUSE_ACTIONS.iter().zip(expected) {
                assert_eq!(
                    app.tab_target_action_enabled(&target, action),
                    enabled,
                    "pending: {action}"
                );
            }
            // A submitted worker can race with cancellation; preparation callbacks
            // are covered separately by request_can_start.
            app.session.start.pending.get_mut("retry").unwrap().preparing = false;
            app.run_tab_target_action(target.clone(), "disconnect_session", window, cx);
            assert!(app.session.start_request_cancelled("retry"));
            assert!(app.quick_switch_items().iter().any(|item| matches!(item,
                crate::models::QuickSwitchItem::Pending { request_id, subtitle, .. }
                    if request_id == "retry" && subtitle.starts_with(rust_i18n::t!("terminal.connectionCancelled").as_ref()))));
            assert!(app.tab_target_action_enabled(&target, "reconnect_session"));
            app.apply_session_start_event(
                success(
                    "retry",
                    "late",
                    SessionLaunchConfig::Local(LocalSessionConfig::default()),
                ),
                cx,
            );
            assert!(!app.session.has_session("late"));
            assert!(app.session.start_request_cancelled("retry"));
            app.run_tab_target_action(target.clone(), "close_tab", window, cx);
            for action in TAB_MOUSE_ACTIONS {
                assert!(
                    !app.tab_target_action_enabled(&target, action),
                    "closed: {action}"
                );
                app.run_tab_target_action(target.clone(), action, window, cx);
            }
            assert!(app.session.has_session("other"));
            assert!(app.session.custom_name("other").is_none());
            assert!(!app.session.is_disconnected("other"));
        })
    });
}

struct StartTabActionsHost {
    app: gpui::Entity<crate::features::NyaTermApp>,
}

impl gpui::Render for StartTabActionsHost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        self.app.update(cx, |app, cx| {
            gpui::div()
                .size_full()
                .relative()
                .flex()
                .flex_col()
                .child(app.session_tab_strip(cx))
                .child(app.tab_actions_overlay(cx))
        })
    }
}

#[test]
fn start_tab_mouse_buttons_open_and_dismiss_menus_and_middle_click_can_close() {
    let root = TestConfigDir::new("nyaterm-start-tab-mouse");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    app.update(&mut cx, |app, cx| {
        failed_start(app, "failed", 1, None);
        app.settings.set_tab_right_click_action("none".into());
        app.settings.set_tab_middle_click_action("close_tab".into());
        app.sync_component_theme(cx);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        let focus = fixture_app.read(cx).terminal.input_focus().clone();
        window.focus(&focus, cx);
        StartTabActionsHost { app: fixture_app }
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let tab = cx.debug_bounds("failed-session-tab-failed").unwrap();
    cx.simulate_mouse_down(tab.center(), MouseButton::Right, Default::default());
    cx.simulate_mouse_up(tab.center(), MouseButton::Right, Default::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("start-tab-actions-menu").is_some());
    // An unavailable action is visibly present and cannot close the menu or launch a job.
    let disabled = cx.debug_bounds("tab-ctx-reconnect").unwrap();
    cx.simulate_click(disabled.center(), Default::default());
    app.read_with(cx, |app, _| {
        assert!(app.session.dialog_tab_actions_target().is_some());
        assert!(!app.session.start.has_pending());
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(app.read(cx).session.dialog_tab_actions_target().is_none());
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
        let _ = window.draw(cx);
    });
    // An unavailable configured right-click action falls back to the same menu.
    app.update(cx, |app, _| {
        app.settings
            .set_tab_right_click_action("multiplex_ssh".into())
    });
    cx.simulate_mouse_down(tab.center(), MouseButton::Right, Default::default());
    cx.simulate_mouse_up(tab.center(), MouseButton::Right, Default::default());
    cx.run_until_parked();
    app.read_with(cx, |app, _| {
        assert!(app.session.dialog_tab_actions_target().is_some())
    });
    // Changing phase dismisses a menu captured against the old request.
    app.update(cx, |app, cx| {
        let failed = app.session.start.take_failed_for_retry("failed").unwrap();
        app.session
            .start
            .begin_preparation("retry".into(), failed.pending);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    app.read_with(cx, |app, _| {
        assert!(app.session.dialog_tab_actions_target().is_none())
    });
    let tab = cx.debug_bounds("pending-session-tab-retry").unwrap();
    cx.simulate_mouse_down(tab.center(), MouseButton::Middle, Default::default());
    cx.simulate_mouse_up(tab.center(), MouseButton::Middle, Default::default());
    app.read_with(cx, |app, _| {
        assert!(!app.session.start.has_pending());
        assert!(!app.session.start.has_failed());
        assert!(app.session.has_session("other"));
    });
}

#[test]
fn start_tab_duplicate_and_retry_keep_the_source_group_while_a_sibling_is_connecting() {
    let root = TestConfigDir::new("nyaterm-start-tab-duplicate");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    let cx = session_focus_test_window(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.reconcile_terminal_windows();
            let group = app.current_terminal_group().unwrap();
            let placement = failed_start(
                app,
                "failed",
                0,
                Some(SessionLaunchConfig::Local(LocalSessionConfig {
                    encoding: "invalid-test-encoding".into(),
                    ..Default::default()
                })),
            );
            app.shell
                .reserve_terminal_start_group(placement.request_sequence, group.clone());
            let target = app.session.start_request_target("failed").unwrap();
            app.run_tab_target_action(target.clone(), "duplicate_session", window, cx);
            assert!(app.session.start.failed("failed").is_some());
            let tabs = app.session_strip_tabs(&Some(group));
            let index = tabs
                .iter()
                .position(|tab| tab.request_id.as_deref() == Some("failed"))
                .unwrap();
            let duplicate = tabs[index + 1].request_id.as_deref().unwrap();
            assert_ne!(duplicate, "failed");
            assert_eq!(
                app.session
                    .start
                    .failed(duplicate)
                    .unwrap()
                    .pending
                    .custom_name
                    .as_deref(),
                Some("custom failed")
            );
            assert!(!app.session.start.failed(duplicate).unwrap().pending.locked);
            let source = app.session.start.failed.get_mut("failed").unwrap();
            source.pending.source_connection_id = Some("saved".into());
            let saved = serde_json::from_value(serde_json::json!({
                "id":"saved", "name":"saved local", "type":"local_terminal", "encoding":"invalid-test-encoding"
            })).unwrap();
            app.connection_state.replace_loaded(vec![saved], Vec::new());
            let mut other = pending("another");
            other.source_connection_id = Some("saved".into());
            app.session.start.register_pending("busy".into(), other);
            assert!(app.tab_target_action_enabled(&target, "duplicate_session"));
            assert!(app.tab_target_action_enabled(&target, "reconnect_session"));
            let before = app.session_strip_tabs(&None).len();
            app.run_tab_target_action(target.clone(), "duplicate_session", window, cx);
            assert_eq!(app.session_strip_tabs(&None).len(), before + 1);
            assert_eq!(app.session.start.pending_count(), 1);
            app.run_tab_target_action(target.clone(), "reconnect_session", window, cx);
            assert_eq!(app.session_strip_tabs(&None).len(), before + 1);
            assert!(app.session.start_action_request(&target).unwrap().2.is_some());
            assert!(app.session.start.attempt("busy").check().is_ok());
            assert_eq!(app.session.start.pending_count(), 1);
        })
    });
}

#[test]
fn start_tab_double_click_respects_settings_and_close_buttons_stop_propagation() {
    use gpui::{MouseDownEvent, MouseUpEvent};
    for (connecting, action, close_button) in [
        (false, "close_tab", false),
        (true, "disconnect_session", false),
        (false, "multiplex_ssh", false),
        (true, "disconnect_session", true),
    ] {
        let root = TestConfigDir::new("nyaterm-start-tab-double-click");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "other");
        app.update(&mut cx, |app, cx| {
            failed_start(app, "target", 1, None);
            if connecting {
                let failed = app.session.start.take_failed_for_retry("target").unwrap();
                app.session
                    .start
                    .begin_preparation("target".into(), failed.pending);
            }
            app.settings.set_tab_double_click_action(action.into());
            app.sync_component_theme(cx);
        });
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            StartTabActionsHost { app: fixture_app }
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let selector = match (connecting, close_button) {
            (true, true) => "pending-session-tab-close-target",
            (true, false) => "pending-session-tab-target",
            (false, _) => "failed-session-tab-target",
        };
        let position = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Default::default(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Default::default(),
            click_count: 2,
        });
        app.read_with(cx, |app, _| {
            assert!(!app.session.start.has_pending());
            assert_eq!(
                app.session.start.has_failed(),
                !close_button && action != "close_tab"
            );
            assert_eq!(
                app.session.start_request_cancelled("target"),
                connecting && !close_button
            );
            assert!(app.session.has_session("other"));
            assert!(!app.session.is_disconnected("other"));
            if action == "multiplex_ssh" {
                assert_eq!(app.session.active_failed_request_id(), Some("target"));
            }
        });
    }
}

#[test]
fn tab_rename_dialog_focuses_and_selects_all_once_when_opened_from_the_context_menu() {
    use nyaterm_ui::NyaDialogWindowExt as _;

    let root = TestConfigDir::new("nyaterm-tab-rename-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "target");
    app.update(&mut cx, |app, cx| {
        app.settings.set_tab_right_click_action("none".into());
        app.sync_component_theme(cx);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(move |cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            StartTabActionsHost { app: fixture_app }
        });
        nyaterm_ui::nya_root(content, window, cx)
    });
    let target = TabActionTarget::Session("target".into());
    for name in ["172.16.0.100", "服务器标签"] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(app.session.rename_tab_target(&target, name.into()));
                window.focus(app.terminal.input_focus(), cx);
                cx.notify();
            });
            let _ = window.draw(cx);
        });
        let tab = cx.debug_bounds("session-tab-target").unwrap();
        cx.simulate_mouse_down(tab.center(), MouseButton::Right, Default::default());
        cx.simulate_mouse_up(tab.center(), MouseButton::Right, Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let rename = cx.debug_bounds("tab-ctx-rename").unwrap();
        cx.simulate_click(rename.center(), Default::default());
        cx.run_until_parked();
        let input = cx.update(|window, cx| {
            let _ = window.draw(cx);
            let input = app.read(cx).existing_text_input("session.rename").unwrap();
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
            let component = input.read(cx).component_state().unwrap();
            assert_eq!(component.read(cx).selected_range(), 0..name.len());
            input
        });
        cx.simulate_input("新名称");
        cx.run_until_parked();
        cx.simulate_keystrokes("home");
        cx.update(|window, cx| {
            assert_eq!(input.read(cx).value(cx), "新名称");
            assert_eq!(app.read(cx).session.dialogs.rename_draft(), "新名称");
            app.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
            let component = input.read(cx).component_state().unwrap();
            assert_eq!(component.read(cx).selected_range(), 0..0);
        });
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            assert!(!input.read(cx).component_focus_handle(cx).is_focused(window));
        });
        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, cx| {
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
            let _ = window.draw(cx);
        });
        let save = cx.debug_bounds("nya-dialog-action-button").unwrap();
        cx.simulate_click(save.center(), Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(app.read(cx).session.custom_name("target"), Some("新名称"));
            assert!(!window.has_active_nya_dialog(cx));
            app.update(cx, |app, cx| {
                app.open_rename_tab_target(target.clone(), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let reopened = app.read(cx).existing_text_input("session.rename").unwrap();
            assert_ne!(input, reopened);
            assert!(
                reopened
                    .read(cx)
                    .component_focus_handle(cx)
                    .is_focused(window)
            );
            let component = reopened.read(cx).component_state().unwrap();
            assert_eq!(component.read(cx).selected_range(), 0.."新名称".len());
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(app.read(cx).existing_text_input("session.rename").is_none());
            assert_eq!(app.read(cx).session.custom_name("target"), Some("新名称"));
        });
    }
}

#[test]
fn tab_rename_deferred_focus_skips_closed_inputs_and_dialogs_covered_by_another_modal() {
    use gpui::IntoElement as _;
    use nyaterm_ui::NyaDialogWindowExt as _;

    let root = TestConfigDir::new("nyaterm-tab-rename-deferred-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "target");
    app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(move |cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            StartTabActionsHost { app: fixture_app }
        });
        nyaterm_ui::nya_root(content, window, cx)
    });
    let closed_input = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_rename_session("target".into(), window, cx);
            let input = app.existing_text_input("session.rename").unwrap();
            app.close_rename_session(cx);
            window.close_nya_dialog(cx);
            app.open_rename_session("target".into(), window, cx);
            input
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(closed_input.read(cx).component_state().is_none());
        let input = app.read(cx).existing_text_input("session.rename").unwrap();
        assert_ne!(closed_input, input);
        assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        app.update(cx, |app, cx| {
            app.close_rename_session(cx);
            window.close_nya_dialog(cx);
        });
    });
    let covered_input = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_rename_session("target".into(), window, cx);
            let input = app.existing_text_input("session.rename").unwrap();
            app.open_content_dialog(
                "Another dialog".into(),
                320.,
                |_, _, _| gpui::div().into_any_element(),
                |_, _| {},
                window,
                cx,
            );
            input
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            !covered_input
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window)
        );
        app.update(cx, |app, cx| app.close_rename_session(cx));
        window.close_all_nya_dialogs(cx);
    });
}

#[test]
fn start_tab_rename_dialog_dismisses_itself_when_its_target_is_closed() {
    use nyaterm_ui::NyaDialogWindowExt as _;
    let root = TestConfigDir::new("nyaterm-start-tab-rename-dismiss");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "other");
    let target = app.update(&mut cx, |app, cx| {
        failed_start(app, "target", 1, None);
        app.sync_component_theme(cx);
        app.session.start_request_target("target").unwrap()
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(move |cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            StartTabActionsHost { app: fixture_app }
        });
        nyaterm_ui::nya_root(content, window, cx)
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.open_rename_tab_target(target, window, cx))
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert!(window.has_active_nya_dialog(cx));
    });
    app.update(cx, |app, cx| {
        app.close_failed_session_start("target".into(), cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(!window.has_active_nya_dialog(cx));
        assert!(app.read(cx).session.dialogs.rename_target().is_none());
        assert!(app.read(cx).session.has_session("other"));
    });
}
