use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, Window, div, prelude::*,
};
use nyaterm_core::test_support::TestTempDir;

struct ReconnectFixture(Entity<NyaTermApp>);

impl Render for ReconnectFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self
            .0
            .update(cx, |app, cx| app.workspace_view(cx).into_any_element());
        div().size_full().flex().child(content)
    }
}

#[test]
fn ssh_and_telnet_reconnect_keep_terminal_mounted_and_accept_enter_after_failure() {
    use crate::features::runtime_jobs::SessionStartResult;
    use crate::models::SessionLaunchConfig;
    use nyaterm_transport::{SessionEvent, SessionKind, SshSessionConfig, TelnetSessionConfig};
    use std::time::Instant;

    for kind in [SessionKind::Ssh, SessionKind::Telnet] {
        let root = TestTempDir::new("nyaterm-terminal-reconnect-ui");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        cx.update(nyaterm_ui::init_components);
        let app = app_with_visible_local_session(&mut cx, root.path(), "s1");
        app.update(&mut cx, |app, cx| {
            let metadata = app.session.metadata_mut("s1").unwrap();
            metadata.launch_config = if kind == SessionKind::Ssh {
                let config = SshSessionConfig {
                    host: "127.0.0.1".into(),
                    port: 0,
                    ..Default::default()
                };
                metadata.ssh_config = Some(config.clone());
                SessionLaunchConfig::Ssh(Box::new(config))
            } else {
                SessionLaunchConfig::Telnet(TelnetSessionConfig {
                    host: "127.0.0.1".into(),
                    port: 0,
                    ..Default::default()
                })
            };
            app.terminal
                .append_existing_session_text("s1", "previous terminal output\r\n");
            app.session.extend_pending_events([SessionEvent::Exited {
                session_id: "s1".into(),
                reason: "network lost".into(),
            }]);
            app.drain_session_events(cx);
            assert!(app.session.is_disconnected("s1"));
        });
        let host_app = app.clone();
        let (_, visual) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(|_| ReconnectFixture(host_app));
            nyaterm_ui::nya_root(fixture, window, cx)
        });
        visual.update(|window, cx| {
            let _ = window.draw(cx);
            let focus = app.read(cx).terminal.input_focus().clone();
            window.focus(&focus, cx);
        });
        // Exercise the actual mounted key handler, including a second
        // retry after failure. Neither phase may replace the terminal.
        for _ in 0..2 {
            visual.simulate_keystrokes("enter");
            let (request_id, output) = app.read_with(visual, |app, _| {
                assert!(app.session.start_reconnect_is_pending("s1"));
                assert!(app.session.start_reconnect_failure("s1").is_none());
                (
                    app.session
                        .start
                        .pending_entries()
                        .next()
                        .unwrap()
                        .0
                        .clone(),
                    app.terminal.session_output("s1").unwrap().to_owned(),
                )
            });
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(visual.debug_bounds("terminal-canvas-s1").is_some());
            assert!(
                visual
                    .debug_bounds("terminal-reconnect-status-s1")
                    .is_some()
            );
            // Repeated Enter during a pending attempt must not start another job.
            visual.simulate_keystrokes("enter");
            app.update(visual, |app, cx| {
                assert_eq!(app.session.start.pending_count(), 1);
                let now = Instant::now();
                app.apply_session_start_event(
                    SessionStartResult {
                        request_id,
                        connection_name: "fixture".into(),
                        kind,
                        worker_started_at: now,
                        worker_finished_at: now,
                        result: Err("connection refused".into()),
                    },
                    cx,
                );
                assert!(!app.session.start_reconnect_is_pending("s1"));
                assert_eq!(
                    app.session.start_reconnect_failure("s1"),
                    Some("connection refused")
                );
                assert_eq!(app.terminal.session_output("s1").unwrap(), output);
                assert!(output.contains("previous terminal output"));
                assert!(app.session.is_disconnected("s1"));
                cx.notify();
            });
            visual.update(|window, cx| {
                let _ = window.draw(cx);
                assert!(app.read(cx).terminal.input_focus().is_focused(window));
            });
            assert!(visual.debug_bounds("terminal-canvas-s1").is_some());
            assert!(
                visual
                    .debug_bounds("terminal-reconnect-status-s1")
                    .is_some()
            );
        }
    }
}
