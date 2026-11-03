use std::io::Read;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AppContext as _, Entity, Modifiers, ScrollDelta, ScrollEvent, TestAppContext, point, px,
};
use nyaterm_core::{AppRuntime, RuntimeMode};
use nyaterm_terminal::TerminalScreen;
use nyaterm_transport::TelnetSessionConfig;

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::models::TerminalProtocolState;
use crate::test_support::{TestAppCleanup, TestConfigDir};

use super::{TerminalPaintedHitTestGeometry, TerminalSurface};

fn zoom_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.to_path_buf(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
}

fn zoom_surface(
    cx: &mut TestAppContext,
    app: &Entity<NyaTermApp>,
    id: &str,
) -> Entity<TerminalSurface> {
    cx.update_entity(app, |app, cx| {
        let output = (0..80)
            .map(|row| format!("row {row}\r\n"))
            .collect::<String>();
        app.session.select_active_session(id);
        app.terminal
            .seed_session_view(id.to_string(), output, "UTF-8");
        let bounds = gpui::bounds(point(px(0.0), px(0.0)), gpui::size(px(400.0), px(240.0)));
        app.remember_terminal_surface_bounds_for_session(Some(id), bounds);
        app.ensure_terminal_surface(id, cx)
    })
}

fn wheel_event(y: f32) -> ScrollEvent {
    ScrollEvent {
        position: point(px(8.0), px(8.0)),
        delta: ScrollDelta::Lines(point(0.0, y)),
        modifiers: if cfg!(target_os = "macos") {
            Modifiers::command()
        } else {
            Modifiers::control()
        },
        ..ScrollEvent::default()
    }
}

#[test]
fn wheel_zoom_defers_resize_and_clamps_without_scrolling() {
    let root = TestConfigDir::new("nyaterm-wheel-zoom");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = zoom_app(&mut cx, root.path());
    let surface = zoom_surface(&mut cx, &app, "zoom");
    for (size, direction, expected) in [(14, 3.0, 15), (14, -3.0, 13), (72, 1.0, 72), (8, -1.0, 8)]
    {
        cx.update_entity(&app, |app, _| {
            let mut settings = app.settings.summary().clone();
            settings.terminal_font_size = size;
            settings.interaction_terminal_zoom_enabled = true;
            app.settings.replace_summary(settings);
        });
        cx.update_entity(&surface, |surface, cx| {
            surface.handle_scroll_wheel(&wheel_event(direction), cx);
            // Parent font updates must wait until this surface's lease is released.
            assert_eq!(app.read(cx).settings.summary().terminal_font_size, size);
            assert_eq!(surface.scroll_offset, 0);
        });
        cx.run_until_parked();
        assert_eq!(
            cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size),
            expected
        );
        assert_eq!(
            cx.read_entity(&surface, |surface, _| surface.scroll_offset),
            0
        );
    }
}

#[test]
fn wheel_zoom_rechecks_the_switch_before_deferred_resize() {
    let root = TestConfigDir::new("nyaterm-wheel-zoom-disabled");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = zoom_app(&mut cx, root.path());
    let surface = zoom_surface(&mut cx, &app, "zoom");
    let before = cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size);
    cx.update_entity(&surface, |surface, cx| {
        surface.handle_scroll_wheel(&wheel_event(1.0), cx);
        app.update(cx, |app, _| app.settings.toggle_terminal_zoom_enabled());
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size),
        before
    );

    cx.update_entity(&surface, |surface, cx| {
        surface.scrollback_len = 100;
        surface.handle_scroll_wheel(&wheel_event(1.0), cx);
        assert!(
            surface.scroll_offset > 0,
            "disabled zoom restores ordinary scrolling"
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size),
        before
    );
}

#[test]
fn wheel_zoom_consumes_mouse_reporting_events_and_disabled_zoom_restores_reports() {
    let root = TestConfigDir::new("nyaterm-wheel-zoom-wire");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = zoom_app(&mut cx, root.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (manager, id) = cx.update_entity(&app, |app, _| {
        let manager = app.session.manager_handle();
        let info = manager
            .create_telnet_session(TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp: true,
                ..TelnetSessionConfig::default()
            })
            .unwrap();
        let mut settings = app.settings.summary().clone();
        settings.interaction_mouse_events_require_alt = false;
        app.settings.replace_summary(settings);
        (manager, info.id)
    });
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let surface = zoom_surface(&mut cx, &app, &id);
    let protocol = TerminalProtocolState {
        mouse_reporting: true,
        mouse_sgr: true,
        ..TerminalProtocolState::default()
    };
    cx.update_entity(&app, |app, _| {
        app.terminal.view.views.get_mut(&id).unwrap().protocol_state = protocol;
    });
    cx.update_entity(&surface, |surface, _| surface.set_protocol_state(protocol));
    cx.update_entity(&surface, |surface, cx| {
        surface.handle_scroll_wheel(&wheel_event(1.0), cx)
    });
    cx.run_until_parked();
    // A sentinel ordered after the deferred callback proves no mouse bytes preceded it.
    cx.update_entity(&app, |app, _| {
        app.write_session_protocol_response(&id, b"ZOOM").unwrap()
    });
    let mut sentinel = [0; 4];
    stream.read_exact(&mut sentinel).unwrap();
    assert_eq!(&sentinel, b"ZOOM");

    cx.update_entity(&app, |app, _| app.settings.toggle_terminal_zoom_enabled());
    cx.update_entity(&surface, |surface, _| {
        // This headless fixture has no rendered frame; provide the geometry
        // a real paint would retain for server-facing mouse hit testing.
        let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
        surface.painted_hit_test_geometry = Some(TerminalPaintedHitTestGeometry {
            grid_bounds: Some(gpui::bounds(
                point(px(0.0), px(0.0)),
                gpui::size(px(400.0), px(240.0)),
            )),
            display_offset: 0,
            viewport_anchor_row: 0,
            snapshot_rows: snapshot.row_count(),
            viewport_rows: snapshot.row_count(),
            visual_y_offset: 0.0,
            cell_width: 8.0,
            cell_height: 16.0,
            revision: surface.revision,
        });
        surface.painted_hit_test_snapshot = Some(snapshot);
    });
    cx.update_entity(&surface, |surface, cx| {
        surface.handle_scroll_wheel(&wheel_event(1.0), cx)
    });
    cx.run_until_parked();
    cx.update_entity(&app, |app, _| {
        app.write_session_protocol_response(&id, b"END").unwrap()
    });
    let expected = b"\x1b[<80;2;1MEND";
    let mut actual = vec![0; expected.len()];
    stream.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    manager.close(&id).unwrap();
}
