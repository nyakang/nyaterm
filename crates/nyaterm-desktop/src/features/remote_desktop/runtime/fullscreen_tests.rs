use gpui::{AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, Window};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_remote_desktop::{PixelFormat, RDP_FRAMEBUFFER_LIMITS, RdpFrameEvent};
use nyaterm_ui::NyaWindowHandle;

use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;

struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

fn host_window(cx: &mut TestAppContext) -> NyaWindowHandle {
    cx.add_window(|window, cx| {
        let view = cx.new(|_| EmptyView);
        nyaterm_ui::nya_root(view, window, cx)
    })
}

fn two_windows(
    cx: &mut TestAppContext,
    app: &Entity<NyaTermApp>,
) -> (NyaWindowHandle, NyaWindowHandle) {
    let main = host_window(cx);
    let fullscreen = host_window(cx);
    app.update(cx, |app, _| {
        app.shell.remember_main_window(main);
        app.remote_desktop.insert_connecting("main-session".into());
        app.remote_desktop
            .insert_connecting("fullscreen-session".into());
        app.remote_desktop.fullscreen_session = Some("fullscreen-session".into());
        app.remote_desktop.fullscreen_window = Some(fullscreen);
    });
    (main, fullscreen)
}

#[test]
fn fullscreen_and_main_sessions_keep_separate_frame_and_input_windows() {
    let root = TestTempDir::new("nyaterm-fullscreen-window-ownership");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "main-session");
    let (main, fullscreen) = two_windows(&mut cx, &app);

    let visited = NyaTermApp::update_in_remote_desktop_windows(
        &app.downgrade(),
        &mut cx,
        |app, window, _| {
            let own = if window.window_handle() == main.into() {
                "main-session"
            } else {
                "fullscreen-session"
            };
            let other = if own == "main-session" {
                "fullscreen-session"
            } else {
                "main-session"
            };
            assert!(app.remote_desktop_session_in_window(own, window));
            assert!(!app.remote_desktop_session_in_window(other, window));
            assert_eq!(app.remote_desktop_input_session(window), Some(own));
            app.reset_rdp_framebuffer(own, 1, 8, 8, RDP_FRAMEBUFFER_LIMITS, window);
            app.apply_rdp_frame_batch(
                own,
                vec![RdpFrameEvent::Bitmap {
                    epoch: 1,
                    full: false,
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                    stride: 8,
                    format: PixelFormat::Bgra8,
                    pixels: vec![1; 16].into(),
                }],
                window,
            );
            let session = &app.remote_desktop.sessions[own];
            assert!(session.texture.is_some());
            assert_eq!(session.texture_window, Some(window.window_handle()));
            assert!(!session.texture_dirty);
            assert!(!session.uploads_stopped);
            window.window_handle()
        },
    )
    .unwrap();
    assert_eq!(visited, vec![main.into(), fullscreen.into()]);

    app.update(&mut cx, |app, _| {
        let texture = app.remote_desktop.sessions["fullscreen-session"].texture;
        app.remote_desktop.release_inactive_textures("main-session");
        assert_eq!(
            app.remote_desktop.sessions["fullscreen-session"].texture,
            texture
        );
        assert!(app.remote_desktop.pending_texture_removals.is_empty());
    });
}

#[test]
fn moving_or_closing_a_session_keeps_the_retired_textures_original_window() {
    let root = TestTempDir::new("nyaterm-fullscreen-texture-retirement");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "main-session");
    let (main, fullscreen) = two_windows(&mut cx, &app);
    let old_texture = fullscreen
        .update(&mut cx, |_, window, cx| {
            app.update(cx, |app, _| {
                app.reset_rdp_framebuffer(
                    "fullscreen-session",
                    1,
                    8,
                    8,
                    RDP_FRAMEBUFFER_LIMITS,
                    window,
                );
                app.remote_desktop.sessions["fullscreen-session"]
                    .texture
                    .unwrap()
            })
        })
        .unwrap();
    app.update(&mut cx, |app, _| {
        app.remote_desktop
            .invalidate_session_textures("fullscreen-session");
        let retired = &app.remote_desktop.pending_texture_removals;
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].owner, Some(fullscreen.into()));
        app.remote_desktop.fullscreen_session = None;
        app.remote_desktop.fullscreen_window = None;
    });
    main.update(&mut cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            for retired in app.remote_desktop.pending_texture_removals.drain(..) {
                retired.remove(window, cx);
            }
            app.prepare_remote_desktop_textures("fullscreen-session", window);
            assert_eq!(
                app.remote_desktop.sessions["fullscreen-session"].texture_window,
                Some(main.into())
            );
            app.remote_desktop.remove_session("fullscreen-session");
            assert_eq!(app.remote_desktop.pending_texture_removals.len(), 1);
            assert_eq!(
                app.remote_desktop.pending_texture_removals[0].owner,
                Some(main.into())
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    fullscreen
        .update(&mut cx, |_, window, _| {
            assert!(
                window
                    .update_dynamic_texture(
                        old_texture,
                        gpui::Bounds::new(
                            gpui::Point::default(),
                            gpui::size(gpui::DevicePixels(2), gpui::DevicePixels(2))
                        ),
                        &[1; 16],
                        8,
                    )
                    .is_err(),
                "the original window must release the retired atlas entry"
            );
        })
        .unwrap();
}
