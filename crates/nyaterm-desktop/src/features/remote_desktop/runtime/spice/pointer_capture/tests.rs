use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::RemoteDesktopSessionState;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{SessionLaunchConfig, session::SpiceLaunchConfig};
use gpui::{
    Context, Entity, IntoElement, Keystroke, PlatformInput, PointerLockEvent, Render,
    TestAppContext, Window, div, point, prelude::*,
};
use nyaterm_core::{
    secret::SecretString, spice::settings::SpiceEndpoint, test_support::TestTempDir,
};
use nyaterm_remote_desktop::{
    RemoteDesktopViewState,
    spice::protocol::{SpiceMouseMode, SpiceSessionConfig},
};

struct PointerFixture(Entity<NyaTermApp>);

impl Render for PointerFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |app, cx| {
            div()
                .size_full()
                .track_focus(app.remote_desktop.focus())
                .child(app.spice_pointer_listener_layer(cx))
        })
    }
}

#[test]
fn spice_pointer_capture_releases_on_shortcut_revocation_and_session_changes() {
    let root = TestTempDir::new("nyaterm-spice-pointer");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "guest");
    app.update(&mut cx, |app, _| {
        app.session.metadata_mut("guest").unwrap().launch_config =
            SessionLaunchConfig::Spice(SpiceLaunchConfig {
                name: "guest".into(),
                session: SpiceSessionConfig {
                    session_id: "guest".into(),
                    endpoint: SpiceEndpoint {
                        host: SecretString::new("localhost"),
                        port: Some(5900),
                        tls_port: None,
                        proxy: None,
                        extra: Default::default(),
                    }
                    .into(),
                    username: SecretString::default().into(),
                    password: SecretString::default().into(),
                    settings: Default::default(),
                    credential_lifetime: Default::default(),
                    tcp_relay: None,
                    tls_relay: None,
                },
            });
        app.remote_desktop.sessions.insert(
            "guest".into(),
            RemoteDesktopSessionState {
                spice: Some(Default::default()),
                state: RemoteDesktopViewState::Connected,
                ..Default::default()
            },
        );
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        window.activate_window();
        let focus = fixture_app.read(cx).remote_desktop.focus().clone();
        window.focus(&focus, cx);
        PointerFixture(fixture_app)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let focus = app.read(cx).remote_desktop.focus().clone();
        window.focus(&focus, cx);
        let _ = window.draw(cx);
        app.update(cx, |app, cx| {
            assert!(window.is_window_active(), "fixture window must be active");
            assert!(
                app.remote_desktop.focus.is_focused(window),
                "fixture must own focus"
            );
            assert!(
                !app.root_overlay_visibility(cx).obscures_native_surface(),
                "fixture must be unobscured"
            );
            assert!(app.spice_relative_input("guest"));
            app.capture_spice_pointer("guest", window, cx);
            assert!(window.is_pointer_locked());
            assert!(app.spice_pointer_owned("guest"));
            assert!(!app.release_spice_pointer_shortcut(
                &Keystroke::parse("ctrl-alt-q").unwrap(),
                window,
                cx
            ));
            assert!(app.release_spice_pointer_shortcut(
                &Keystroke::parse("shift-f12").unwrap(),
                window,
                cx
            ));
            assert!(!window.is_pointer_locked());
            assert!(!app.spice_pointer_owned("guest"));
            app.capture_spice_pointer("guest", window, cx);
        });
        window.dispatch_event(PlatformInput::PointerLock(PointerLockEvent::Unlocked), cx);
        assert!(!window.is_pointer_locked());
        assert!(app.read(cx).remote_desktop.spice_pointer_capture.is_none());
        app.update(cx, |app, cx| {
            app.capture_spice_pointer("guest", window, cx);
            app.release_remote_keys("guest");
            assert!(!app.spice_pointer_owned("guest"));
            app.reconcile_spice_pointer(window, cx);
            assert!(!window.is_pointer_locked());
            app.capture_spice_pointer("guest", window, cx);
            app.remote_desktop
                .sessions
                .get_mut("guest")
                .unwrap()
                .spice
                .as_mut()
                .unwrap()
                .capabilities
                .mouse_mode = SpiceMouseMode::Absolute;
            app.reconcile_spice_pointer(window, cx);
            assert!(!window.is_pointer_locked());
            app.capture_spice_pointer("guest", window, cx);
            assert!(!window.is_pointer_locked());
            app.remote_desktop
                .sessions
                .get_mut("guest")
                .unwrap()
                .spice
                .as_mut()
                .unwrap()
                .capabilities
                .mouse_mode = SpiceMouseMode::Relative;
            app.capture_spice_pointer("guest", window, cx);
            app.remote_desktop.sessions.get_mut("guest").unwrap().state =
                RemoteDesktopViewState::Disconnected;
            app.reconcile_spice_pointer(window, cx);
            assert!(!window.is_pointer_locked());
        });
        let position = window.mouse_position();
        window.dispatch_event(
            PlatformInput::PointerLock(PointerLockEvent::Motion {
                delta: point(1000., -1000.),
            }),
            cx,
        );
        assert_eq!(window.mouse_position(), position);
    });
}
