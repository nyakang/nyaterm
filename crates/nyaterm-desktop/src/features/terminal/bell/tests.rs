use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_terminal::TerminalEffects;
use nyaterm_ui::NyaWindowHandle;

use super::{BELL_INTERVAL, TerminalBellState};
use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;

#[test]
fn bell_bursts_are_dropped_until_the_playback_interval_expires() {
    let mut bell = TerminalBellState::default();
    let now = Instant::now();
    assert!(bell.request(now));
    assert!(!bell.request(now + BELL_INTERVAL));
    let played = now + BELL_INTERVAL;
    assert!(bell.finish(played, true));
    assert!(!bell.finish(played, true));
    assert!(!bell.request(played + BELL_INTERVAL - Duration::from_nanos(1)));
    assert!(bell.request(played + BELL_INTERVAL));
    assert!(bell.finish(played + BELL_INTERVAL, true));
}

#[test]
fn suppressed_bells_are_discarded_without_delaying_the_next_allowed_bell() {
    let mut bell = TerminalBellState::default();
    let now = Instant::now();
    assert!(bell.request(now));
    assert!(!bell.finish(now, false));
    assert!(!bell.finish(now, true));
    assert!(bell.request(now));
    assert!(bell.finish(now, true));
}

fn attach_window(cx: &mut TestAppContext, app: &Entity<NyaTermApp>) -> NyaWindowHandle {
    let window = cx.add_window(|window, cx| {
        let content = cx.new(|_| EmptyView);
        nyaterm_ui::nya_root(content, window, cx)
    });
    app.update(cx, |app, _| app.shell.remember_main_window(window));
    window
}

struct EmptyView;

impl gpui::Render for EmptyView {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}

fn emit_bell(app: &mut NyaTermApp, cx: &mut gpui::Context<NyaTermApp>) {
    app.apply_terminal_effects(
        "bell-session",
        TerminalEffects {
            bell: true,
            ..TerminalEffects::default()
        },
        false,
        cx,
    );
}

#[test]
fn terminal_effect_delivery_rings_once_for_a_burst() {
    let root = TestTempDir::new("nyaterm-terminal-bell-delivery");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "bell-session");
    let _window = attach_window(&mut cx, &app);
    app.update(&mut cx, |app, cx| {
        app.session.clear_active_session();
        let other = app.session.metadata("bell-session").unwrap().clone();
        app.session
            .register_session_metadata("other-session", other);
        app.apply_terminal_effects("bell-session", TerminalEffects::default(), false, cx);
        assert!(!app.terminal.bell.pending);
        emit_bell(app, cx);
        app.request_terminal_bell("other-session", cx);
        emit_bell(app, cx);
        assert!(app.terminal.bell.pending);
        assert!(app.terminal.bell.last_played.is_none());
    });
    cx.run_until_parked();
    app.read_with(&cx, |app, _| {
        assert!(!app.terminal.bell.pending);
        assert!(app.terminal.bell.last_played.is_some());
    });
}

#[test]
fn closed_windows_discard_pending_bells() {
    let root = TestTempDir::new("nyaterm-terminal-bell-closed-window");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "bell-session");
    let window = attach_window(&mut cx, &app);
    window
        .update(&mut cx, |_, window, cx| {
            app.update(cx, emit_bell);
            window.remove_window();
        })
        .unwrap();
    cx.run_until_parked();
    app.read_with(&cx, |app, _| {
        assert!(!app.terminal.bell.pending);
        assert!(app.terminal.bell.last_played.is_none());
    });
}

#[test]
fn disabled_and_locked_bells_are_never_queued_for_later_playback() {
    let root = TestTempDir::new("nyaterm-terminal-bell-disabled");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "bell-session");
    let _window = attach_window(&mut cx, &app);
    app.update(&mut cx, |app, cx| {
        app.settings.toggle_terminal_bell();
        emit_bell(app, cx);
        assert!(!app.terminal.bell.pending);
        app.settings.toggle_terminal_bell();
        app.security.activate_screen_lock(String::new());
        emit_bell(app, cx);
        assert!(!app.terminal.bell.pending);
        app.security.deactivate_screen_lock();
    });
    cx.run_until_parked();
    app.read_with(&cx, |app, _| {
        assert!(app.terminal.bell.last_played.is_none())
    });
    app.update(&mut cx, emit_bell);
    cx.run_until_parked();
    app.read_with(&cx, |app, _| {
        assert!(app.terminal.bell.last_played.is_some())
    });
}

#[test]
fn terminal_bell_rechecks_settings_lock_and_session_before_playback() {
    for change in 0..3 {
        let root = TestTempDir::new("nyaterm-terminal-bell-suppressed");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "bell-session");
        let _window = attach_window(&mut cx, &app);
        app.update(&mut cx, |app, cx| {
            emit_bell(app, cx);
            match change {
                0 => app.settings.toggle_terminal_bell(),
                1 => app.security.activate_screen_lock(String::new()),
                _ => {
                    app.session.remove_session_catalog("bell-session");
                }
            }
        });
        cx.run_until_parked();
        app.read_with(&cx, |app, _| {
            assert!(!app.terminal.bell.pending);
            assert!(app.terminal.bell.last_played.is_none());
        });
    }
}
