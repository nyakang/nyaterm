//! The connect-time serial dialog is a deferral, not a start: nothing may be
//! registered, focused, or left behind until the device and framing are known.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use nyaterm_core::{ConnectionType, SavedConnection};
use nyaterm_transport::SessionEvent;
use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

use crate::features::NyaTermApp;
use crate::features::session::SavedConnectionStartOptions;
use crate::features::session::serial_connect_prompt::SERIAL_PROMPT_CUSTOM_PORT;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::SessionLaunchConfig;
use crate::test_support::{TestAppCleanup, TestConfigDir};

/// Nothing renders app state: the prompt must reach the remembered main window
/// even from a test harness that never drew it.
struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A window that counts as the main window, which is what the dialog needs.
fn hosted(cx: &mut TestAppContext, app: &Entity<NyaTermApp>) -> VisualTestContext {
    cx.update(nyaterm_ui::init_components);
    let app = app.clone();
    let handle = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host);
        nya_root(host, window, cx)
    });
    let mut cx = VisualTestContext::from_window(handle.into(), cx);
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.shell.remember_main_window(handle.downcast().unwrap())
        })
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx
}

fn serial(port_name: &str, baud_rate: u32, prompt: serde_json::Value) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": "board", "name": "Board", "type": "serial",
        "port_name": port_name, "baud_rate": baud_rate,
        "connect_prompt": prompt,
    }))
    .unwrap()
}

fn open_dialog(
    cx: &mut TestAppContext,
    app: &Entity<NyaTermApp>,
    connection: SavedConnection,
    options: SavedConnectionStartOptions,
) -> VisualTestContext {
    let mut cx = hosted(cx, app);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(
                !app.continue_saved_connection_start(connection, options, cx),
                "a deferred start must report that it did not start"
            );
        })
    });
    cx.run_until_parked();
    cx
}

#[test]
fn deferring_the_device_waits_in_a_dialog_without_registering_a_start() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-defer");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let tabs_before = app.read_with(&cx, |app, _| app.session_strip_tabs(&None).len());
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|window, cx| {
        assert!(window.has_active_nya_dialog(cx));
        app.read_with(cx, |app, _| {
            let prompt = app
                .session
                .dialogs
                .serial_connect_prompt()
                .expect("prompt kept until answered");
            assert_eq!(prompt.connection.id, "board");
            assert!(prompt.ask_port);
            assert!(!prompt.ask_baud_rate);
            // Saved values only seed the fields; they are not a decision yet.
            assert_eq!(prompt.port_name, "COM7");
            assert_eq!(prompt.baud_rate, "115200");
            assert_eq!(app.session.start.pending_count(), 0);
            assert_eq!(app.session_strip_tabs(&None).len(), tabs_before);
        });
    });
}

#[test]
fn answering_the_dialog_starts_the_saved_connection_on_the_chosen_device() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-submit");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    // Everything else the caller asked for has to survive the round trip.
    let options = SavedConnectionStartOptions {
        reconnect_session_id: Some("existing".into()),
        ..Default::default()
    };
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial(
            "COM7",
            115200,
            serde_json::json!({"port": true, "baud_rate": true}),
        ),
        options,
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.apply_serial_prompt_port("COM9".into(), cx);
            app.apply_serial_prompt_baud_rate("57600".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            assert!(app.session.dialogs.serial_connect_prompt().is_none());
            assert_eq!(app.session.start.pending_count(), 1);
            let (_, pending) = app.session.start.pending_entries().next().unwrap();
            let Some(SessionLaunchConfig::Serial(config)) = pending.launch_config.as_ref() else {
                panic!("serial launch config expected");
            };
            assert_eq!(config.port_name, "COM9");
            assert_eq!(config.baud_rate, 57600);
            assert_eq!(pending.reconnect_session_id.as_deref(), Some("existing"));
        });
    });
}

#[test]
fn an_empty_device_keeps_the_dialog_open_with_a_readable_error() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-validation");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.apply_serial_prompt_port("  ".into(), cx);
            assert!(!app.submit_serial_connect_prompt(window, cx));
            let prompt = app
                .session
                .dialogs
                .serial_connect_prompt()
                .expect("a rejected answer keeps the dialog");
            assert_eq!(prompt.error, Some("serialPrompt.portRequired"));
            assert_eq!(app.session.start.pending_count(), 0);
            // Editing clears the complaint instead of leaving a stale one.
            app.apply_serial_prompt_port("/dev/ttyUSB0".into(), cx);
            assert!(
                app.session
                    .dialogs
                    .serial_connect_prompt()
                    .unwrap()
                    .error
                    .is_none()
            );
        });
    });
}

#[test]
fn cancelling_the_dialog_leaves_no_prompt_and_no_tab() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-cancel");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let tabs_before = app.read_with(&cx, |app, _| app.session_strip_tabs(&None).len());
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"baud_rate": true})),
        Default::default(),
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(!window.has_active_nya_dialog(cx));
        app.read_with(cx, |app, _| {
            assert!(app.session.dialogs.serial_connect_prompt().is_none());
            assert_eq!(app.session.start.pending_count(), 0);
            assert_eq!(app.session_strip_tabs(&None).len(), tabs_before);
        });
    });
}

/// Session restore, MCP/automation, and retry must never block on a dialog.
#[test]
fn unattended_starts_never_prompt_and_report_the_missing_device() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-unattended");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let connection = serial("", 115200, serde_json::json!({"port": true}));
    app.update(&mut cx, |app, cx| {
        let mut options = app.prepare_session_start_options(Default::default());
        options.suppress_serial_prompt = true;
        options.request_id = Some(
            app.session
                .start
                .begin_saved_preparation(&connection, &options),
        );
        let tabs_before = app.session_strip_tabs(&None).len();
        app.continue_saved_connection_start(connection, options, cx);
        assert!(app.session.dialogs.serial_connect_prompt().is_none());
        let (_, failed) = app
            .session
            .start
            .failed_entries()
            .next()
            .expect("a readable failure instead of a dialog");
        assert!(failed.error.contains("Board"), "{}", failed.error);
        assert_eq!(app.session.start.pending_count(), 0);
        // Preparation already reserved a tab; the failure must not add another.
        assert_eq!(app.session_strip_tabs(&None).len(), tabs_before);
    });
}

/// A second request while the dialog is open must not stack a second prompt or
/// silently drop the first.
#[test]
fn a_second_prompt_request_keeps_the_open_dialog() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-reentrant");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.apply_serial_prompt_port("COM5".into(), cx);
            assert!(!app.continue_saved_connection_start(
                serial("COM8", 9600, serde_json::json!({"port": true})),
                Default::default(),
                cx,
            ));
            let prompt = app
                .session
                .dialogs
                .serial_connect_prompt()
                .expect("the first prompt survives");
            assert_eq!(prompt.port_name, "COM5");
            let ConnectionType::Serial { port_name, .. } = &prompt.connection.config else {
                panic!("serial connection expected");
            };
            assert_eq!(port_name, "COM7");
            assert!(window.has_active_nya_dialog(cx));
        });
    });
}

/// "Custom" is a way to reach the free-text field, not a device: the saved
/// device has to survive behind it so it can still be edited from.
#[test]
fn choosing_custom_reveals_the_field_without_inventing_a_device() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-custom");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_serial_prompt_port(SERIAL_PROMPT_CUSTOM_PORT.into(), cx);
            let prompt = app
                .session
                .dialogs
                .serial_connect_prompt()
                .expect("the dialog stays open");
            assert!(prompt.custom_port);
            assert_eq!(prompt.port_name, "COM7", "the field starts from the hint");

            // Typing is the answer while custom is chosen.
            app.apply_serial_prompt_port("/dev/ttyACM0".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            let (_, pending) = app.session.start.pending_entries().next().unwrap();
            let Some(SessionLaunchConfig::Serial(config)) = pending.launch_config.as_ref() else {
                panic!("serial launch config expected");
            };
            assert_eq!(config.port_name, "/dev/ttyACM0");
        });
    });
}

/// The tab is named after the device it opens. A connection saved from `COM7`
/// and reopened on `COM9` must not sit in a tab that still says `COM7 @ 115200`.
#[test]
fn the_tab_is_titled_after_the_device_the_dialog_answered_with() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-tab-title");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    // A connection whose name is still the default the device gave it.
    let mut connection = serial("COM7", 115200, serde_json::json!({"port": true}));
    connection.name = "COM7 @ 115200".to_string();
    let mut cx = open_dialog(&mut cx, &app, connection, Default::default());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_serial_prompt_port("COM9".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            let (_, pending) = app.session.start.pending_entries().next().unwrap();
            assert_eq!(pending.connection_name, "COM9 @ 115200");
            let Some(SessionLaunchConfig::Serial(config)) = pending.launch_config.as_ref() else {
                panic!("serial launch config expected");
            };
            assert_eq!(config.name, "COM9 @ 115200");
            assert_eq!(
                app.session.start_pending_display_name().as_deref(),
                Some("COM9 @ 115200")
            );
        });
    });
}

/// A name the user chose is the tab's name, whatever device answers the dialog.
#[test]
fn a_named_connection_keeps_its_tab_title_across_devices() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-keeps-name");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_serial_prompt_port("COM9".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            let (_, pending) = app.session.start.pending_entries().next().unwrap();
            assert_eq!(pending.connection_name, "Board");
            let Some(SessionLaunchConfig::Serial(config)) = pending.launch_config.as_ref() else {
                panic!("serial launch config expected");
            };
            assert_eq!(config.port_name, "COM9");
        });
    });
}

/// A connection saved without a device carries the editor's bare "Serial" —
/// that is not a name the user chose, so the tab still follows the device.
#[test]
fn a_deviceless_serial_connection_is_titled_after_the_answered_device() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-fallback-name");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut connection = serial("", 115200, serde_json::json!({"port": true}));
    connection.name = crate::models::SERIAL_FALLBACK_NAME.to_string();
    let mut cx = open_dialog(&mut cx, &app, connection, Default::default());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_serial_prompt_port("COM9".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            let (_, pending) = app.session.start.pending_entries().next().unwrap();
            assert_eq!(pending.connection_name, "COM9 @ 115200");
            let Some(SessionLaunchConfig::Serial(config)) = pending.launch_config.as_ref() else {
                panic!("serial launch config expected");
            };
            assert_eq!(config.name, "COM9 @ 115200");
            assert_eq!(config.port_name, "COM9");
        });
    });
}

/// Picking a listed device again is a decision, so the field goes away.
#[test]
fn picking_a_listed_device_leaves_the_custom_field_behind() {
    let root = TestConfigDir::new("nyaterm-serial-prompt-custom-back");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM7", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.select_serial_prompt_port(SERIAL_PROMPT_CUSTOM_PORT.into(), cx);
            app.select_serial_prompt_port("COM5".into(), cx);
            let prompt = app.session.dialogs.serial_connect_prompt().unwrap();
            assert!(!prompt.custom_port);
            assert_eq!(prompt.port_name, "COM5");
        });
    });
}

/// A saved connection that asks at connect, and a session that opened a
/// different device and rate than the saved hint: what the session answered is
/// the one a reconnect has to reuse.
fn prepare_serial_reconnect(app: &mut NyaTermApp) {
    let connection = serial(
        "COM7",
        115200,
        serde_json::json!({"port": true, "baud_rate": true}),
    );
    let metadata = app.session.metadata_mut("existing").unwrap();
    metadata.launch_config = SessionLaunchConfig::Serial(nyaterm_transport::SerialSessionConfig {
        port_name: "COM9".into(),
        baud_rate: 57600,
        ..Default::default()
    });
    metadata.source_connection_id = Some(connection.id.clone());
    app.connection_state
        .replace_loaded(vec![connection], vec![]);
    // The probe finds COM9 alive, so the reconnect stays a direct one.
    app.test_serial_ports = Some(vec![nyaterm_transport::SerialPortInfo {
        port_name: "COM9".into(),
        ..Default::default()
    }]);
    app.session
        .set_custom_name("existing".into(), "My board".into());
    app.session.set_tab_color("existing", Some(0x123456));
    app.session.set_tab_locked("existing", true);
}

/// A board that came back under another name cannot be reopened blindly: the
/// reconnect probes the enumeration and, finding the old device gone, asks
/// where it went, prefilled with what the session actually had open.
#[test]
fn a_reconnect_whose_device_vanished_asks_where_the_board_went() {
    let root = TestConfigDir::new("nyaterm-serial-reconnect-device-gone");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            // The session's COM9 is gone; only COM7 is enumerated now.
            app.test_serial_ports = Some(vec![nyaterm_transport::SerialPortInfo {
                port_name: "COM7".into(),
                ..Default::default()
            }]);
            app.reconnect_session("existing".into(), window, cx);
            // No direct attempt: the prompt stands in for it.
            assert_eq!(app.session.start.pending_count(), 0);
            let state = app.session.dialogs.serial_connect_prompt().unwrap();
            // Prefilled with the session's own answer, not the saved hint.
            assert_eq!(state.port_name, "COM9");
            assert_eq!(state.baud_rate, "57600");
            // COM9 is not enumerated, so the free-text field is on screen.
            assert!(state.custom_port);
            assert_eq!(state.continuation.reconnect_session_id(), Some("existing"));

            // The board came back on COM7.
            app.select_serial_prompt_port("COM7".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
        })
    });
    cx.run_until_parked();
    app.read_with(&cx, |app, _| {
        assert!(app.session.dialogs.serial_connect_prompt().is_none());
        assert_eq!(app.session.start.pending_count(), 1);
        let pending = app.session.start.pending_entries().next().unwrap().1;
        let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
            panic!("expected serial");
        };
        assert_eq!(config.port_name, "COM7");
        // The rate the session had open, not the saved hint's 115200.
        assert_eq!(config.baud_rate, 57600);
        assert_eq!(pending.reconnect_session_id.as_deref(), Some("existing"));
        assert_eq!(pending.custom_name.as_deref(), Some("My board"));
        // The saved connection keeps its own hint untouched.
        let ConnectionType::Serial { baud_rate, .. } =
            &app.connection_state.connections()[0].config
        else {
            panic!("expected serial");
        };
        assert_eq!(*baud_rate, 115200);
    });
}

/// The device a session opened is the only answer a reconnect needs, so a board
/// that came back after a replug returns on Enter rather than behind a dialog.
#[test]
fn reconnecting_a_serial_session_reuses_the_device_it_opened() {
    let root = TestConfigDir::new("nyaterm-serial-reconnect-reuses-device");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            app.reconnect_session("existing".into(), window, cx);
            assert!(!window.has_active_nya_dialog(cx));
            assert!(app.session.dialogs.serial_connect_prompt().is_none());
            assert!(app.session.metadata("existing").unwrap().disconnected);
            assert_eq!(app.session.start.pending_count(), 1);
            let pending = app.session.start.pending_entries().next().unwrap().1;
            let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
                panic!("expected serial");
            };
            // The session's answer, not the saved hint (COM7 @ 115200).
            assert_eq!(config.port_name, "COM9");
            assert_eq!(config.baud_rate, 57600);
            assert_eq!(pending.reconnect_session_id.as_deref(), Some("existing"));
            assert_eq!(pending.custom_name.as_deref(), Some("My board"));
            assert_eq!(pending.tab_color, Some(0x123456));
            assert!(pending.locked);
            // The saved connection keeps its hint: the selection is for this
            // start only.
            let ConnectionType::Serial {
                port_name,
                baud_rate,
                ..
            } = &app.connection_state.connections()[0].config
            else {
                panic!("expected serial");
            };
            assert_eq!(port_name, "COM7");
            assert_eq!(*baud_rate, 115200);
        })
    });
}

/// When a serial transport dies (unplugged board, dead driver) it now reports
/// the exit, so the tab shows the disconnect banner and Enter brings the board
/// back on the device it opened.
#[test]
fn a_serial_session_that_exits_offers_enter_to_reconnect() {
    let root = TestConfigDir::new("nyaterm-serial-exited-reconnect");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            app.session.extend_pending_events([SessionEvent::Exited {
                session_id: "existing".into(),
                reason: "serial read failed: device gone".into(),
            }]);
            app.drain_session_events(cx);
            let output = app.terminal.session_output("existing").unwrap();
            assert_eq!(output.matches("[Session disconnected]").count(), 1);
            assert_eq!(output.matches("[Press Enter to reconnect]").count(), 1);
            assert!(app.session.metadata("existing").unwrap().disconnected);

            // Enter on the disconnected tab is this exact call.
            app.reconnect_session("existing".into(), window, cx);
            assert_eq!(app.session.start.pending_count(), 1);
            let pending = app.session.start.pending_entries().next().unwrap().1;
            let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
                panic!("expected serial");
            };
            assert_eq!(config.port_name, "COM9");
            assert_eq!(config.baud_rate, 57600);
            assert_eq!(pending.reconnect_session_id.as_deref(), Some("existing"));
        })
    });
}

/// A device another tab already holds stays refused: reconnecting must not
/// close a working session to collide with a port it cannot open.
#[test]
fn reconnecting_onto_a_device_another_session_holds_is_refused() {
    let root = TestConfigDir::new("nyaterm-serial-reconnect-port-in-use");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            let mut other = app.session.metadata("existing").unwrap().clone();
            other.launch_config =
                SessionLaunchConfig::Serial(nyaterm_transport::SerialSessionConfig {
                    port_name: "COM9".into(),
                    ..Default::default()
                });
            app.session.register_session_metadata("other", other);
            app.reconnect_session("existing".into(), window, cx);
            assert!(!app.session.metadata("existing").unwrap().disconnected);
            assert_eq!(app.session.start.pending_count(), 0);
            assert_eq!(
                app.shell.status(),
                rust_i18n::t!("dialog.serialPortInUse").as_ref()
            );
        })
    });
}

#[test]
/// Serial consoles say nothing until they are spoken to, so a successful
/// reconnect sends one Enter to bring the prompt straight back.
#[cfg(unix)]
#[test]
fn a_successful_serial_reconnect_elicits_the_prompt_with_one_enter() {
    use std::ffi::CStr;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::time::Duration;

    let mut master = -1;
    let mut slave = -1;
    let mut name = [0 as libc::c_char; 128];
    // openpty writes two owned descriptors and a NUL-terminated slave path.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                name.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let _master = unsafe { OwnedFd::from_raw_fd(master) };
    let _slave = unsafe { OwnedFd::from_raw_fd(slave) };
    let path = unsafe { CStr::from_ptr(name.as_ptr()) }
        .to_str()
        .unwrap()
        .to_string();
    let root = TestConfigDir::new("nyaterm-serial-reconnect-enter");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    let mut backend_id = String::new();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let config = nyaterm_transport::SerialSessionConfig {
                port_name: path.clone(),
                // serialport uses zero to skip IOSSIOSPEED, which macOS PTYs
                // reject with ENOTTY. The fixture has no physical baud rate.
                #[cfg(target_os = "macos")]
                baud_rate: 0,
                ..Default::default()
            };
            backend_id = app
                .session
                .manager()
                .create_serial_session(config.clone())
                .unwrap()
                .id;
            let mut metadata = app.session.metadata("existing").unwrap().clone();
            metadata.launch_config = SessionLaunchConfig::Serial(config);
            app.session.register_session_metadata(&backend_id, metadata);
            app.session.select_active_session(&backend_id);
            app.terminal
                .seed_session_view(backend_id.clone(), "device> ".into(), "UTF-8");
            app.schedule_serial_reconnect_enter(backend_id.clone(), cx);
        })
    });
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();

    // One Enter, nothing else: the device answers with its prompt.
    unsafe {
        libc::fcntl(
            master,
            libc::F_SETFL,
            libc::fcntl(master, libc::F_GETFL) | libc::O_NONBLOCK,
        );
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16];
    while !bytes.contains(&b'\r') {
        let read = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read > 0 {
            bytes.extend_from_slice(&buffer[..read as usize]);
            continue;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no enter reached the device"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(bytes, vec![b'\r']);
}

#[test]
fn serial_reconnect_rechecks_a_deleted_or_changed_saved_connection_before_closing() {
    for deleted in [true, false] {
        let root = TestConfigDir::new("nyaterm-serial-reconnect-stale-source");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let mut cx = hosted(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                prepare_serial_reconnect(app);
                let connections = if deleted {
                    vec![]
                } else {
                    vec![
                        serde_json::from_value(serde_json::json!({
                            "id":"board", "name":"Changed", "type":"telnet", "host":"127.0.0.1"
                        }))
                        .unwrap(),
                    ]
                };
                app.connection_state.replace_loaded(connections, vec![]);
                app.reconnect_session("existing".into(), window, cx);
                assert!(!app.session.metadata("existing").unwrap().disconnected);
                assert_eq!(app.session.start.pending_count(), 0);
                assert!(!app.session.session_is_busy("existing"));
            })
        });
    }
}

#[test]
fn concurrent_serial_starts_reserve_the_port_before_the_worker_finishes() {
    let root = TestConfigDir::new("nyaterm-serial-concurrent-start");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("COM9", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let config = SessionLaunchConfig::Serial(nyaterm_transport::SerialSessionConfig {
                port_name: "COM9".into(),
                ..Default::default()
            });
            app.begin_background_session_start(
                "First".into(),
                config.clone(),
                None,
                Default::default(),
                Default::default(),
                cx,
            );
            assert_eq!(app.session.start.pending_count(), 1);
            app.select_serial_prompt_port(SERIAL_PROMPT_CUSTOM_PORT.into(), cx);
            app.apply_serial_prompt_port(" COM9 ".into(), cx);
            assert!(!app.submit_serial_connect_prompt(window, cx));
            assert_eq!(
                app.session.dialogs.serial_connect_prompt().unwrap().error,
                Some("dialog.serialPortInUse")
            );
            app.begin_background_session_start(
                "Second".into(),
                config,
                None,
                Default::default(),
                Default::default(),
                cx,
            );
            assert_eq!(app.session.start.pending_count(), 1);
            let id = app
                .session
                .start
                .pending_entries()
                .next()
                .unwrap()
                .0
                .clone();
            app.session.start.cancel_pending(&id, "cancelled".into());
            assert!(!app.serial_port_in_use("COM9"));
            assert!(app.submit_serial_connect_prompt(window, cx));
            assert_eq!(app.session.start.pending_count(), 1);
        })
    });
}

#[test]
fn serial_discovery_seeds_empty_fields_without_overwriting_user_input() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let root = TestConfigDir::new("nyaterm-serial-discovery-input");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let (release_tx, release_rx) = mpsc::channel();
    // Reserve discovery before opening the dialog so it never enumerates the
    // host's devices, and release the fixture only after the empty prompt opens.
    app.update(&mut cx, |app, cx| {
        app.request_connection_serial_ports_refresh_with_loader(
            move || {
                release_rx
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|error| error.to_string())?;
                Ok(vec![nyaterm_transport::SerialPortInfo {
                    port_name: "COM9".into(),
                    ..Default::default()
                }])
            },
            cx,
        );
    });
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    app.read_with(&cx, |app, _| {
        assert!(app.connection_state.serial_ports_loading());
        assert!(
            app.session
                .dialogs
                .serial_connect_prompt()
                .unwrap()
                .port_name
                .is_empty()
        );
    });
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.read_with(&cx, |app, _| app.connection_state.serial_ports_loading()) {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "serial discovery did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(
                app.session
                    .dialogs
                    .serial_connect_prompt()
                    .unwrap()
                    .port_name,
                "COM9"
            );
            app.apply_serial_prompt_port("/dev/custom-board".into(), cx);
            app.connection_state
                .replace_serial_ports(vec![nyaterm_transport::SerialPortInfo {
                    port_name: "COM10".into(),
                    ..Default::default()
                }]);
            app.seed_serial_connect_prompt_device(cx);
            assert_eq!(
                app.session
                    .dialogs
                    .serial_connect_prompt()
                    .unwrap()
                    .port_name,
                "/dev/custom-board"
            );
            app.apply_serial_prompt_port("".into(), cx);
            app.seed_serial_connect_prompt_device(cx);
            assert!(
                app.session
                    .dialogs
                    .serial_connect_prompt()
                    .unwrap()
                    .port_name
                    .is_empty()
            );
        })
    });
}

#[test]
fn slow_serial_discovery_allows_editing_and_cancel_without_duplicate_enumeration() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    for cancel in [false, true] {
        let root = TestConfigDir::new("nyaterm-serial-slow-discovery");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let (release_tx, release_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        app.update(&mut cx, |app, cx| {
            let calls = calls.clone();
            app.request_connection_serial_ports_refresh_with_loader(
                move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|error| error.to_string())?;
                    Ok(vec![nyaterm_transport::SerialPortInfo {
                        port_name: "COM9".into(),
                        ..Default::default()
                    }])
                },
                cx,
            );
        });
        let mut cx = open_dialog(
            &mut cx,
            &app,
            serial("", 115200, serde_json::json!({"port": true})),
            Default::default(),
        );
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        cx.update(|window, cx| {
            assert!(window.has_active_nya_dialog(cx));
            app.update(cx, |app, cx| {
                assert!(app.connection_state.serial_ports_loading());
                app.apply_serial_prompt_port("/dev/custom-board".into(), cx);
                let calls = calls.clone();
                app.request_connection_serial_ports_refresh_with_loader(
                    move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![])
                    },
                    cx,
                );
            });
        });
        if cancel {
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
        }
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if app.read_with(&cx, |app, _| !app.connection_state.serial_ports_loading()) {
                break;
            }
            assert!(Instant::now() < deadline, "serial discovery did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cx.update(|window, cx| {
            assert_eq!(window.has_active_nya_dialog(cx), !cancel);
            app.read_with(cx, |app, _| {
                assert_eq!(app.connection_state.serial_ports()[0].port_name, "COM9");
                if cancel {
                    assert!(app.session.dialogs.serial_connect_prompt().is_none());
                } else {
                    assert_eq!(
                        app.session
                            .dialogs
                            .serial_connect_prompt()
                            .unwrap()
                            .port_name,
                        "/dev/custom-board"
                    );
                }
            });
        });
    }
}

#[test]
fn reconnecting_a_closed_serial_session_leaves_nothing_pending() {
    let root = TestConfigDir::new("nyaterm-serial-reconnect-closed-session");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut cx = hosted(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            prepare_serial_reconnect(app);
            app.session.remove_session_catalog("existing");
            app.reconnect_session("existing".into(), window, cx);
            assert!(!app.session.has_session("existing"));
            assert_eq!(app.session.start.pending_count(), 0);
            assert!(app.session.dialogs.serial_connect_prompt().is_none());
        })
    });
}

#[test]
fn failed_serial_discovery_keeps_the_manual_device_input_usable() {
    use std::time::{Duration, Instant};

    let root = TestConfigDir::new("nyaterm-serial-discovery-failure");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    app.update(&mut cx, |app, cx| {
        app.request_connection_serial_ports_refresh_with_loader(
            || Err("enumeration failed".into()),
            cx,
        );
    });
    let mut cx = open_dialog(
        &mut cx,
        &app,
        serial("", 115200, serde_json::json!({"port": true})),
        Default::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.read_with(&cx, |app, _| app.connection_state.serial_ports_loading()) {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "serial discovery did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(
                app.connection_state.serial_ports_error(),
                Some("enumeration failed")
            );
            app.apply_serial_prompt_port("/dev/custom-board".into(), cx);
            assert!(app.submit_serial_connect_prompt(window, cx));
            assert_eq!(app.session.start.pending_count(), 1);
        })
    });
}

#[test]
fn serial_failed_retry_keeps_the_selected_device_rate_and_resolved_framing() {
    for saved_port in ["COM7", ""] {
        let root = TestConfigDir::new("nyaterm-serial-resolved-retry");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let mut connection = serial(
            saved_port,
            115200,
            serde_json::json!({"port": true, "baud_rate": true}),
        );
        let ConnectionType::Serial {
            data_bits, parity, ..
        } = &mut connection.config
        else {
            panic!("expected serial");
        };
        *data_bits = 7;
        *parity = "even".into();
        app.update(&mut cx, |app, _| {
            app.connection_state
                .replace_loaded(vec![connection.clone()], vec![])
        });
        let mut cx = open_dialog(
            &mut cx,
            &app,
            connection,
            SavedConnectionStartOptions {
                custom_name: Some("My board".into()),
                tab_color: Some(0x123456),
                locked: true,
                ..Default::default()
            },
        );
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.apply_serial_prompt_port("/nyaterm-missing-selected-device".into(), cx);
                app.apply_serial_prompt_baud_rate("57600".into(), cx);
                assert!(app.submit_serial_connect_prompt(window, cx));
                let request_id = app
                    .session
                    .start
                    .pending_entries()
                    .next()
                    .unwrap()
                    .0
                    .clone();
                app.session.simulate_start_failure(&request_id);
                let failed = app.session.start.failed(&request_id).unwrap();
                let Some(SessionLaunchConfig::Serial(config)) = &failed.pending.launch_config
                else {
                    panic!("expected serial");
                };
                assert_eq!(config.port_name, "/nyaterm-missing-selected-device");
                assert_eq!(config.baud_rate, 57600);
                let resolved_config = config.clone();
                app.retry_failed_session_start(request_id.clone(), window, cx);
                assert!(app.session.dialogs.serial_connect_prompt().is_none());
                assert_eq!(app.session.start.pending_count(), 1);
                let (new_id, pending) = app.session.start.pending_entries().next().unwrap();
                assert_ne!(new_id, &request_id);
                let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
                    panic!("expected serial");
                };
                assert_eq!(config, &resolved_config);
                assert_eq!(config.port_name, "/nyaterm-missing-selected-device");
                assert_eq!(config.baud_rate, 57600);
                assert_eq!(config.data_bits, 7);
                assert_eq!(config.parity, "even");
                assert_eq!(pending.custom_name.as_deref(), Some("My board"));
                assert_eq!(pending.tab_color, Some(0x123456));
                assert!(pending.locked);
                let ConnectionType::Serial {
                    port_name,
                    baud_rate,
                    ..
                } = &app.connection_state.connections()[0].config
                else {
                    panic!("expected serial");
                };
                assert_eq!(port_name, saved_port);
                assert_eq!(*baud_rate, 115200);
            })
        });
    }
}

#[test]
fn unattended_serial_starts_reject_every_deferred_field_even_with_saved_hints() {
    for prompt in [
        serde_json::json!({"port": true}),
        serde_json::json!({"baud_rate": true}),
        serde_json::json!({"port": true, "baud_rate": true}),
    ] {
        let root = TestConfigDir::new("nyaterm-serial-unattended-hints");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let connection = serial("COM7", 115200, prompt);
        app.update(&mut cx, |app, cx| {
            let mut options = app.prepare_session_start_options(SavedConnectionStartOptions {
                suppress_serial_prompt: true,
                ..Default::default()
            });
            let id = app
                .session
                .start
                .begin_saved_preparation(&connection, &options);
            options.request_id = Some(id.clone());
            let submitted = app.blocking_jobs.metrics().submitted;
            assert!(!app.continue_saved_connection_start(connection, options, cx));
            assert!(app.session.dialogs.serial_connect_prompt().is_none());
            assert_eq!(app.session.start.pending_count(), 0);
            assert!(
                app.session
                    .start
                    .failed(&id)
                    .unwrap()
                    .error
                    .contains("Board")
            );
            assert_eq!(app.blocking_jobs.metrics().submitted, submitted);
        });
    }
}

#[test]
fn an_explicit_serial_selection_allows_an_unattended_start_without_using_hints() {
    let root = TestConfigDir::new("nyaterm-serial-explicit-unattended");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    app.update(&mut cx, |app, cx| {
        let connection = serial(
            "COM7",
            115200,
            serde_json::json!({"port": true, "baud_rate": true}),
        );
        assert!(app.continue_saved_connection_start(
            connection,
            SavedConnectionStartOptions {
                suppress_serial_prompt: true,
                serial_selection: Some(crate::features::session::SerialConnectSelection {
                    port_name: "/nyaterm-missing-selected-device".into(),
                    baud_rate: 57600,
                }),
                ..Default::default()
            },
            cx
        ));
        assert!(app.session.dialogs.serial_connect_prompt().is_none());
        let pending = app.session.start.pending_entries().next().unwrap().1;
        let Some(SessionLaunchConfig::Serial(config)) = &pending.launch_config else {
            panic!("expected serial");
        };
        assert_eq!(config.port_name, "/nyaterm-missing-selected-device");
        assert_eq!(config.baud_rate, 57600);
    });
}
