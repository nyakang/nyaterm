use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Context, DevicePixels, Entity, EntityInputHandler as _, FocusHandle, IntoElement, KeyBinding,
    KeyUpEvent, Keystroke, Modifiers, PlatformInput, Render, TestAppContext, Window, actions, div,
    point, prelude::*, px, size,
};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_remote_desktop::{
    Framebuffer, RDP_FRAMEBUFFER_LIMITS, RdpSessionConfig, RemoteDesktopViewState, VncSessionConfig,
};

use crate::features::NyaTermApp;
use crate::features::remote_desktop::init_key_bindings;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::SessionLaunchConfig;

actions!(remote_desktop_keyboard_test, [RootTab, RootShiftTab]);

struct KeyboardFixture {
    app: Entity<NyaTermApp>,
    local_focus: FocusHandle,
    navigations: Rc<Cell<usize>>,
}

impl Render for KeyboardFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = self.app.update(cx, |app, cx| {
            app.remote_desktop_view_content("remote".into(), cx)
        });
        div()
            .size_full()
            .flex()
            .key_context("Root")
            .on_action(cx.listener(|this, _: &RootTab, window, cx| {
                this.navigations.set(this.navigations.get() + 1);
                window.focus(&this.local_focus, cx);
            }))
            .on_action(cx.listener(|this, _: &RootShiftTab, window, cx| {
                this.navigations.set(this.navigations.get() + 1);
                window.focus(&this.local_focus, cx);
            }))
            .child(div().w(px(40.)).h_full().track_focus(&self.local_focus))
            .child(div().flex_1().h_full().child(surface))
    }
}

#[test]
fn focused_rdp_and_vnc_surfaces_keep_tab_and_shift_tab_out_of_local_navigation() {
    for protocol in ["rdp", "vnc", "vnc-view-only"] {
        let root = TestTempDir::new("nyaterm-remote-tab");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "remote");
        cx.update(|cx| {
            init_key_bindings(cx);
            cx.bind_keys([
                KeyBinding::new("tab", RootTab, Some("Root")),
                KeyBinding::new("shift-tab", RootShiftTab, Some("Root")),
            ]);
        });
        app.update(&mut cx, |app, _| {
            app.session.metadata_mut("remote").unwrap().launch_config = if protocol == "rdp" {
                SessionLaunchConfig::Rdp(RdpSessionConfig {
                    relay: None,
                    backend: Default::default(),
                    windows_authentication: None,
                    name: "RDP".into(),
                    host: "127.0.0.1".into(),
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
                    host: "127.0.0.1".into(),
                    port: 5900,
                    password: None,
                    security: Default::default(),
                    display: Default::default(),
                    clipboard: Default::default(),
                    reconnect: Default::default(),
                    shared: true,
                    view_only: protocol == "vnc-view-only",
                })
            };
            app.remote_desktop.insert_connecting("remote".into());
            app.remote_desktop.sessions.get_mut("remote").unwrap().state =
                RemoteDesktopViewState::Connected;
        });
        let navigations = Rc::new(Cell::new(0));
        let fixture_app = app.clone();
        let fixture_navigations = navigations.clone();
        let (fixture, cx) = cx.add_window_view(move |window, cx| {
            fixture_app.update(cx, |app, _| {
                let framebuffer = Framebuffer::new(1, 8, 8, RDP_FRAMEBUFFER_LIMITS).unwrap();
                let texture = window
                    .create_dynamic_texture(
                        size(DevicePixels(8), DevicePixels(8)),
                        framebuffer.pixels(),
                        32,
                    )
                    .unwrap();
                let session = app.remote_desktop.sessions.get_mut("remote").unwrap();
                session.framebuffer = Some(framebuffer);
                session.texture = Some(texture);
            });
            KeyboardFixture {
                app: fixture_app,
                local_focus: cx.focus_handle(),
                navigations: fixture_navigations,
            }
        });
        cx.update(|window, cx| {
            let focus = app.read(cx).remote_desktop.focus().clone();
            window.focus(&focus, cx);
            _ = window.draw(cx);
        });
        cx.simulate_mouse_move(point(px(80.), px(80.)), None, Modifiers::default());
        cx.simulate_keystrokes("tab tab shift-tab");
        cx.update(|window, cx| {
            assert_eq!(navigations.get(), 0, "{protocol}");
            assert!(app.read(cx).remote_desktop.focus().is_focused(window));
            // No helper is launched: inspect the RDP mapper to prove the
            // action entered the normal send path despite transport failure.
            if protocol == "rdp" {
                assert!(
                    app.update(cx, |app, _| app
                        .remote_desktop
                        .sessions
                        .get_mut("remote")
                        .unwrap()
                        .keys
                        .key_down("tab", false))
                        .is_some_and(|event| matches!(
                            event,
                            nyaterm_remote_desktop::RdpInputEvent::KeyDown { repeat: true, .. }
                        ))
                );
            }
            window.dispatch_event(
                PlatformInput::KeyUp(KeyUpEvent {
                    keystroke: Keystroke::parse("tab").unwrap(),
                }),
                cx,
            );
            if protocol == "rdp" {
                assert!(
                    app.update(cx, |app, _| app
                        .remote_desktop
                        .sessions
                        .get_mut("remote")
                        .unwrap()
                        .keys
                        .key_up("tab"))
                        .is_none()
                );
            }
        });
        // Hovering local chrome must release system shortcut capture without
        // dropping ordinary remote keyboard focus.
        cx.simulate_keystrokes("tab");
        cx.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::default());
        cx.update(|window, cx| {
            assert!(
                app.read(cx).remote_desktop.focus().is_focused(window),
                "{protocol}"
            );
            if protocol == "rdp" {
                assert!(
                    app.update(cx, |app, _| app
                        .remote_desktop
                        .sessions
                        .get_mut("remote")
                        .unwrap()
                        .keys
                        .key_up("tab"))
                        .is_none()
                );
            }
        });
        cx.simulate_keystrokes("tab shift-tab");
        assert_eq!(navigations.get(), 0, "{protocol}");
        if protocol != "rdp" {
            let input = cx.update(|_, cx| app.read(cx).remote_desktop.inputs["remote"].clone());
            cx.update(|window, cx| {
                input.update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "中", None, window, cx);
                });
            });
            cx.simulate_keystrokes("tab shift-tab");
            cx.update(|window, cx| {
                assert!(input.update(cx, |input, _| input.consumes_key_up("tab")));
                assert!(app.read(cx).remote_desktop.focus().is_focused(window));
            });
            assert_eq!(
                navigations.get(),
                0,
                "{protocol}: IME must keep Tab local to composition"
            );
        }
        cx.update(|window, cx| {
            let focus = fixture.read(cx).local_focus.clone();
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("tab shift-tab");
        assert_eq!(
            navigations.get(),
            2,
            "{protocol}: local navigation must remain available"
        );
    }
}
