//! Reconnect recovery and delayed wake behavior, independent of attached devices.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use gpui::{Entity, TestAppContext};
use nyaterm_transport::{SerialSessionConfig, TelnetSessionConfig};
use nyaterm_ui::NyaDialogWindowExt as _;

use super::serial_prompt_tests::{fail_serial_reconnect, hosted, prepare_serial_reconnect};
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::{SessionStartResult, SessionStartSuccess};
use crate::features::session::SavedConnectionStartOptions;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::SessionLaunchConfig;
use crate::test_support::{TestAppCleanup, TestConfigDir};

#[test]
fn serial_reconnect_tries_custom_paths_while_device_discovery_is_blocked() {
    let root = TestConfigDir::new("nyaterm-serial-reconnect-during-discovery");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    app.update(&mut cx, |app, cx| {
        app.request_connection_serial_ports_refresh_with_loader(
            move || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                Err("enumeration unavailable".into())
            },
            cx,
        );
    });
    cx.run_until_parked();
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            let SessionLaunchConfig::Serial(config) =
                &mut app.session.metadata_mut("existing").unwrap().launch_config
            else {
                panic!("serial");
            };
            config.port_name = "/dev/serial/by-id/nyaterm-test-missing-device".into();
            config.data_bits = 7;
            config.parity = "even".into();
            config.stop_bits = "2".into();
            config.encoding = "GB18030".into();
            let expected = config.clone();
            app.reconnect_session("existing".into(), window, cx);
            app.reconnect_session("existing".into(), window, cx);
            assert!(app.connection_state.serial_ports_loading());
            assert!(app.session.dialogs.serial_connect_prompt().is_none());
            assert_eq!(app.session.start.pending_count(), 1);
            let pending = app.session.start.pending_entries().next().unwrap().1;
            let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
                panic!("serial");
            };
            assert_eq!(config, &expected);
        });
    });
    release_tx.send(()).unwrap();
}

#[test]
fn temporary_serial_reconnect_keeps_a_new_selection_and_framing_after_failure() {
    let root = TestConfigDir::new("nyaterm-temporary-serial-reconnect-selection");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            let metadata = app.session.metadata_mut("existing").unwrap();
            metadata.source_connection_id = None;
            let SessionLaunchConfig::Serial(config) = &mut metadata.launch_config else {
                panic!("serial");
            };
            config.data_bits = 7;
            config.parity = "even".into();
            app.open_serial_reconnect_prompt("existing".into(), cx);
            app.apply_serial_prompt_port("/nyaterm-missing-selected-device".into(), cx);
            app.apply_serial_prompt_baud_rate("38400".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            fail_serial_reconnect(app, cx);
            app.reconnect_session("existing".into(), window, cx);
            let pending = app.session.start.pending_entries().next().unwrap().1;
            let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
                panic!("serial");
            };
            assert_eq!(config.port_name, "/nyaterm-missing-selected-device");
            assert_eq!(config.baud_rate, 38400);
            assert_eq!(config.data_bits, 7);
            assert_eq!(config.parity, "even");
            assert!(pending.source_connection_id.is_none());
            assert!(pending.locked);
        });
    });
}

#[test]
fn serial_reconnect_failure_does_not_open_a_picker_for_an_inactive_or_closed_session() {
    for close in [false, true] {
        let root = TestConfigDir::new("nyaterm-serial-reconnect-stale-picker");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let mut cx = hosted(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                prepare_serial_reconnect(app);
                app.reconnect_session("existing".into(), window, cx);
                fail_serial_reconnect(app, cx);
                if close {
                    app.remove_session_state("existing", cx);
                } else {
                    let metadata = app.session.metadata("existing").unwrap().clone();
                    app.session.register_session_metadata("other", metadata);
                    app.session.select_active_session("other");
                }
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            app.read_with(cx, |app, _| {
                assert!(app.session.dialogs.serial_connect_prompt().is_none());
                assert_eq!(app.session.start.pending_count(), 0);
            });
        });
    }
}

#[test]
fn serial_reconnect_picker_revalidates_a_saved_source_before_submitting() {
    let root = TestConfigDir::new("nyaterm-serial-reconnect-picker-source-changed");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            app.open_serial_reconnect_prompt("existing".into(), cx);
            app.connection_state.replace_loaded(vec![], vec![]);
            assert!(app.submit_serial_connect_prompt(window, cx));
            assert_eq!(app.session.start.pending_count(), 0);
            assert!(!app.session.is_disconnected("existing"));
        });
    });
}

/// A raw TCP byte sink exercises the actual queued writer on every OS. Only
/// desktop metadata is serial; attached hardware and serial drivers are unused.
fn loopback_serial_backend(
    app: &Entity<NyaTermApp>,
    cx: &mut TestAppContext,
) -> (String, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let manager = app.read_with(cx, |app, _| app.session.manager_handle());
    let info = manager
        .create_telnet_session(TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp: true,
            ..Default::default()
        })
        .unwrap();
    let (peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    app.update(cx, |app, _| {
        let mut metadata = app.session.metadata("existing").unwrap().clone();
        metadata.launch_config = SessionLaunchConfig::Serial(SerialSessionConfig {
            port_name: "/nyaterm-test-missing-serial-device".into(),
            ..Default::default()
        });
        metadata.source_connection_id = None;
        metadata.disconnected = false;
        app.session.register_session_metadata(&info.id, metadata);
    });
    (info.id, peer)
}

fn assert_no_bytes(peer: &mut TcpStream) {
    let mut byte = [0];
    match peer.read(&mut byte) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) => {}
        result => panic!("unexpected serial input: {result:?}, byte={byte:?}"),
    }
}

#[test]
fn opted_in_serial_reconnect_wake_is_delayed_once_and_never_broadcast() {
    let root = TestConfigDir::new("nyaterm-serial-wake-exact-bytes");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let (id, mut peer) = loopback_serial_backend(&app, &mut cx);
    let (_, mut other) = loopback_serial_backend(&app, &mut cx);
    app.update(&mut cx, |app, cx| {
        app.settings.toggle_terminal_serial_reconnect_send_enter();
        app.sync_input.toggle_broadcast_to_all();
        app.schedule_serial_reconnect_enter(id.clone(), cx);
        app.schedule_serial_reconnect_enter(id.clone(), cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(299));
    cx.run_until_parked();
    assert_no_bytes(&mut peer);
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    let mut byte = [0];
    peer.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [b'\r']);
    assert_no_bytes(&mut peer);
    assert_no_bytes(&mut other);
}

#[test]
fn serial_reconnect_wake_is_cancelled_by_all_user_and_script_input_paths() {
    for input in [
        "logical",
        "raw",
        "sensitive",
        "framed",
        "literal",
        "automated",
    ] {
        let root = TestConfigDir::new("nyaterm-serial-wake-competing-input");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let (id, mut peer) = loopback_serial_backend(&app, &mut cx);
        app.update(&mut cx, |app, cx| {
            app.settings.toggle_terminal_serial_reconnect_send_enter();
            app.schedule_serial_reconnect_enter(id.clone(), cx);
            match input {
                "logical" => app.write_session_input_recorded(&id, b"x"),
                "raw" => app.write_session_raw_input_recorded(&id, b"x"),
                "sensitive" => app.write_session_sensitive_input(&id, b"x"),
                "framed" => app.write_session_wire_input_recorded_as(&id, b"x", b"x"),
                "literal" => app.write_session_literal_input_recorded(&id, b"x"),
                "automated" => app.write_automated_terminal_input(&id, b"x", None),
                _ => unreachable!(),
            }
            .unwrap();
        });
        let mut byte = [0];
        peer.read_exact(&mut byte).unwrap();
        assert_eq!(byte, [b'x']);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        assert_no_bytes(&mut peer);
    }
}

#[test]
fn serial_reconnect_wake_rechecks_settings_and_session_lifetime() {
    for change in [
        "disabled",
        "turned-off",
        "off-on",
        "disconnected",
        "removed",
        "nonserial",
        "reconnect",
    ] {
        let root = TestConfigDir::new("nyaterm-serial-wake-invalidated");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let (id, mut peer) = loopback_serial_backend(&app, &mut cx);
        let mut cx = hosted(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                if change != "disabled" {
                    app.settings.toggle_terminal_serial_reconnect_send_enter();
                }
                app.schedule_serial_reconnect_enter(id.clone(), cx);
                match change {
                    "disabled" => {}
                    "turned-off" => app.settings.toggle_terminal_serial_reconnect_send_enter(),
                    "off-on" => {
                        app.toggle_terminal_serial_reconnect_send_enter(cx);
                        app.toggle_terminal_serial_reconnect_send_enter(cx);
                    }
                    "disconnected" => app.mark_session_disconnected(&id, cx),
                    "removed" => app.remove_session_state(&id, cx),
                    "nonserial" => {
                        app.session.metadata_mut(&id).unwrap().launch_config =
                            SessionLaunchConfig::Local(Default::default())
                    }
                    "reconnect" => app.reconnect_session(id.clone(), window, cx),
                    _ => unreachable!(),
                }
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        assert_no_bytes(&mut peer);
    }
}

#[test]
fn first_serial_connect_does_not_send_a_reconnect_wake() {
    let root = TestConfigDir::new("nyaterm-serial-first-connect-no-wake");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let (id, mut peer) = loopback_serial_backend(&app, &mut cx);
    app.update(&mut cx, |app, cx| {
        app.settings.toggle_terminal_serial_reconnect_send_enter();
        let config = app.session.metadata(&id).unwrap().launch_config.clone();
        app.session.remove_session_catalog(&id);
        app.begin_background_session_start(
            "Board".into(),
            config,
            None,
            Default::default(),
            SavedConnectionStartOptions::default(),
            cx,
        );
        let request_id = app
            .session
            .start
            .pending_entries()
            .next()
            .unwrap()
            .0
            .clone();
        let mut info = app
            .session
            .manager()
            .list_sessions()
            .unwrap()
            .into_iter()
            .find(|info| info.id == id)
            .unwrap();
        info.kind = nyaterm_transport::SessionKind::Serial;
        let now = std::time::Instant::now();
        app.apply_session_start_event(
            SessionStartResult {
                request_id,
                connection_name: "Board".into(),
                kind: nyaterm_transport::SessionKind::Serial,
                worker_started_at: now,
                worker_finished_at: now,
                result: Ok(SessionStartSuccess {
                    session_info: info,
                    multiplex_handle: None,
                    launch_config: None,
                    cleanup: None,
                    ftp_service: None,
                }),
            },
            cx,
        );
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_no_bytes(&mut peer);
}

#[test]
fn serial_reconnect_wakes_only_after_an_accepted_history_handoff() {
    for (accepted, enabled) in [(true, true), (false, true), (true, false)] {
        let root = TestConfigDir::new("nyaterm-serial-reconnect-history-handoff-wake");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let (id, mut peer) = loopback_serial_backend(&app, &mut cx);
        let mut cx = hosted(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                prepare_serial_reconnect(app);
                if enabled {
                    app.settings.toggle_terminal_serial_reconnect_send_enter();
                }
                app.session.remove_session_catalog(&id);
                app.reconnect_session_with_serial_selection(
                    "existing".into(),
                    Some(crate::features::session::SerialConnectSelection {
                        port_name: "/nyaterm-missing-new-serial-device".into(),
                        baud_rate: 38400,
                    }),
                    window,
                    cx,
                );
                let request_id = app
                    .session
                    .start
                    .pending_entries()
                    .next()
                    .unwrap()
                    .0
                    .clone();
                let mut info = app
                    .session
                    .manager()
                    .list_sessions()
                    .unwrap()
                    .into_iter()
                    .find(|info| info.id == id)
                    .unwrap();
                info.kind = nyaterm_transport::SessionKind::Serial;
                let now = std::time::Instant::now();
                app.apply_session_start_event(
                    SessionStartResult {
                        request_id,
                        connection_name: "Board".into(),
                        kind: nyaterm_transport::SessionKind::Serial,
                        worker_started_at: now,
                        worker_finished_at: now,
                        result: Ok(SessionStartSuccess {
                            session_info: info,
                            multiplex_handle: None,
                            launch_config: None,
                            cleanup: None,
                            ftp_service: None,
                        }),
                    },
                    cx,
                );
                assert!(app.complete_reconnected_session_frame("existing", &id, accepted, cx));
                if !accepted {
                    let config = app
                        .session
                        .start
                        .serial_reconnect_config("existing")
                        .unwrap();
                    assert_eq!(config.port_name, "/nyaterm-missing-new-serial-device");
                    assert_eq!(config.baud_rate, 38400);
                }
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        if accepted && enabled {
            let mut byte = [0];
            peer.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [b'\r']);
        }
        assert_no_bytes(&mut peer);
    }
}
