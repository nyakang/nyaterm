use gpui::{
    Context, Entity, FocusHandle, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*,
};
use nyaterm_transport::LocalSessionConfig;
use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

use super::failed_start;
use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::SessionLaunchConfig;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct FailureKeyboardHost {
    app: Entity<NyaTermApp>,
    other_focus: FocusHandle,
}

impl Render for FailureKeyboardHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self
            .app
            .update(cx, |app, cx| app.workspace_view(cx).into_any_element());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().track_focus(&self.other_focus))
            .child(content)
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn press_enter(cx: &mut VisualTestContext) {
    // Button activation completes on key-up; simulate_keystrokes only sends key-down.
    let keystroke = Keystroke::parse("enter").unwrap();
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
}

fn failed_id(app: &Entity<NyaTermApp>, cx: &VisualTestContext) -> String {
    app.read_with(cx, |app, _| {
        app.session.active_failed_request_id().unwrap().to_string()
    })
}

#[test]
fn failure_page_enter_retries_once_and_respects_other_focus_and_pending_state() {
    let root = TestConfigDir::new("nyaterm-failure-keyboard");
    let mut cx = TestAppContext::single();
    cx.update(nyaterm_ui::init_components);
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        failed_start(
            app,
            "failure",
            1,
            Some(SessionLaunchConfig::Local(LocalSessionConfig {
                encoding: "invalid-test-encoding".into(),
                ..LocalSessionConfig::default()
            })),
        );
    });
    let fixture_app = app.clone();
    let (host, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        FailureKeyboardHost {
            app: fixture_app,
            other_focus: cx.focus_handle(),
        }
    });
    draw(cx);
    let bounds = cx.debug_bounds("failed-session-reconnect-target").unwrap();
    assert_eq!(bounds.size.height, gpui::px(28.));
    press_enter(cx);
    draw(cx);
    let retried = failed_id(&app, cx);
    assert_ne!(retried, "failure");
    assert_eq!(
        app.read_with(cx, |app, _| app.session.start.failed_entries().count()),
        1
    );

    let key = Keystroke::parse("enter").unwrap();
    cx.simulate_event(KeyDownEvent {
        keystroke: key.clone(),
        is_held: true,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke: key });
    draw(cx);
    assert_eq!(
        failed_id(&app, cx),
        retried,
        "holding Enter must not retry a fast failure again"
    );

    cx.update(|window, cx| {
        let other_focus = host.read(cx).other_focus.clone();
        window.focus(&other_focus, cx);
        app.update(cx, |_, cx| cx.notify());
        let _ = window.draw(cx);
        assert!(host.read(cx).other_focus.is_focused(window));
    });
    press_enter(cx);
    assert_eq!(failed_id(&app, cx), retried);

    // Selecting a failed tab deliberately hands focus back from an input.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_session("source".into(), cx);
        });
        let other_focus = host.read(cx).other_focus.clone();
        window.focus(&other_focus, cx);
    });
    draw(cx);
    let selector = Box::leak(format!("failed-session-tab-{retried}").into_boxed_str());
    let tab = cx.debug_bounds(selector).unwrap();
    cx.simulate_click(tab.center(), Default::default());
    draw(cx);
    press_enter(cx);
    draw(cx);
    assert_ne!(failed_id(&app, cx), retried);

    app.update(cx, |app, cx| {
        let id = app.session.active_failed_request_id().unwrap().to_string();
        let failed = app.session.start.take_failed_for_retry(&id).unwrap();
        app.session
            .start
            .begin_preparation("pending".into(), failed.pending);
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("failed-session-reconnect-target").is_none());
    press_enter(cx);
    press_enter(cx);
    assert!(app.read_with(cx, |app, _| app.session.start_has_active_pending()));
}

#[test]
fn failure_page_enter_retries_an_existing_session() {
    let root = TestConfigDir::new("nyaterm-reconnect-failure-keyboard");
    let mut cx = TestAppContext::single();
    cx.update(nyaterm_ui::init_components);
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.session.metadata_mut("source").unwrap().launch_config =
            SessionLaunchConfig::Local(LocalSessionConfig {
                encoding: "invalid-test-encoding".into(),
                ..LocalSessionConfig::default()
            });
        app.session
            .start
            .set_reconnect_failure("source".into(), "synthetic failure".into(), None);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        FailureKeyboardHost {
            app: fixture_app,
            other_focus: cx.focus_handle(),
        }
    });
    draw(cx);
    press_enter(cx);
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_ne!(
            app.session.start_reconnect_failure("source"),
            Some("synthetic failure")
        );
        assert!(app.session.has_session("source"));
    });
}

#[test]
fn failure_page_does_not_take_enter_from_a_modal_dialog() {
    let root = TestConfigDir::new("nyaterm-failure-modal-focus");
    let mut cx = TestAppContext::single();
    cx.update(nyaterm_ui::init_components);
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "source");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        failed_start(
            app,
            "failure",
            1,
            Some(SessionLaunchConfig::Local(LocalSessionConfig {
                encoding: "invalid-test-encoding".into(),
                ..LocalSessionConfig::default()
            })),
        );
    });
    let fixture_app = app.clone();
    let fixture = cx.new(|cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        FailureKeyboardHost {
            app: fixture_app,
            other_focus: cx.focus_handle(),
        }
    });
    let (_, cx) = cx.add_window_view(move |window, cx| nya_root(fixture.clone(), window, cx));
    cx.update(|window, cx| {
        window.open_nya_dialog(cx, |dialog, _, _| {
            dialog.title("Test modal").content("Keep dialog focus")
        });
    });
    draw(cx);
    press_enter(cx);
    assert_eq!(failed_id(&app, cx), "failure");
    cx.update(|window, cx| {
        window.close_all_nya_dialogs(cx);
        app.update(cx, |app, cx| {
            window.focus(app.terminal.input_focus(), cx);
            cx.notify();
        });
    });
    draw(cx);
    press_enter(cx);
    assert_ne!(failed_id(&app, cx), "failure");
}
