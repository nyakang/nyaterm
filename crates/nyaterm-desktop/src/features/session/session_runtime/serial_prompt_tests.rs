//! The connect-time serial dialog is a deferral, not a start: nothing may be
//! registered, focused, or left behind until the device and framing are known.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use nyaterm_core::{ConnectionType, SavedConnection};
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
