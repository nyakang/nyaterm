use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_core::{
    ConnectionRecordingSettings, RecordingMode as CoreRecordingMode, SavedConnection,
};
use nyaterm_recording::recording::RecordingMode;
use std::fs;
use std::time::{Duration, Instant};

fn fixture(cx: &mut TestAppContext, root: &TestTempDir) -> Entity<NyaTermApp> {
    cx.executor().allow_parking();
    let app = app_with_visible_local_session(cx, root.path(), "fixture");
    cx.update_entity(&app, |app, cx| {
        let mut settings = app.settings.summary().clone();
        settings.recording_auto_start = false;
        settings.recording_path = root.join("recordings").to_string_lossy().into_owned();
        settings.recording_path_template = "nested/{session_short_id}.log".into();
        settings.recording_default_mode = CoreRecordingMode::Raw;
        app.settings.replace_summary(settings);
        app.start_recording_event_drain(cx);
        app.terminal
            .seed_session_view("fixture".into(), "fixture$ ".into(), "UTF-8");
        let metadata = app.session.metadata("fixture").unwrap().clone();
        app.session.register_session_metadata("other", metadata);
        app.terminal
            .seed_session_view("other".into(), "other$ ".into(), "UTF-8");
    });
    app
}

fn wait_for(
    cx: &mut TestAppContext,
    app: &Entity<NyaTermApp>,
    condition: impl Fn(&NyaTermApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if cx.read(|cx| condition(app.read(cx))) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "recording operation did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn assert_terminal_unchanged(cx: &mut TestAppContext, app: &Entity<NyaTermApp>) {
    cx.read(|cx| {
        let app = app.read(cx);
        assert_eq!(app.terminal.session_output("fixture"), Some("fixture$ "));
        assert_eq!(app.terminal.session_output("other"), Some("other$ "));
    });
}

#[gpui::test]
fn manual_recording_uses_profile_without_picker_and_never_inserts_terminal_notices(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-manual-recording");
    let app = fixture(cx, &root);
    cx.update_entity(&app, |app, cx| {
        app.toggle_active_session_recording(cx);
        // A second click while the writer starts must not queue another operation.
        app.toggle_active_session_recording(cx);
        app.session.select_active_session("other");
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && app.recording.is_recording("fixture")
    });
    cx.read(|cx| {
        let status = app.read(cx).recording.status("fixture").unwrap();
        assert_eq!(status.mode, RecordingMode::Raw);
        assert_eq!(
            status.file_path.unwrap(),
            root.join("recordings/nested/fixture.raw.log")
        );
    });
    assert_terminal_unchanged(cx, &app);
    cx.update_entity(&app, |app, cx| {
        app.toggle_recording_for_session("fixture", cx)
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && !app.recording.is_recording("fixture")
    });
    assert_terminal_unchanged(cx, &app);
    cx.update_entity(&app, |app, cx| {
        app.save_session_transcript_for_session("fixture", cx)
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none()
    });
    assert_terminal_unchanged(cx, &app);
    assert!(fs::read_dir(root.join("recordings")).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("session-Local_session-")
    }));
}

#[gpui::test]
fn manual_recording_honors_connection_overrides_and_explicit_modes(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-manual-recording-overrides");
    let app = fixture(cx, &root);
    cx.update_entity(&app, |app, cx| {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "saved", "name": "synthetic", "type": "ssh", "host": "example.test"
        }))
        .unwrap();
        connection.recording = Some(ConnectionRecordingSettings {
            mode: Some(CoreRecordingMode::Transcript),
            path_template: Some("override/{session_short_id}.log".into()),
            ..Default::default()
        });
        app.connection_state
            .replace_loaded(vec![connection], vec![]);
        app.session
            .metadata_mut("fixture")
            .unwrap()
            .source_connection_id = Some("saved".into());
        app.toggle_recording_for_session("fixture", cx);
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && app.recording.is_recording("fixture")
    });
    cx.read(|cx| {
        let status = app.read(cx).recording.status("fixture").unwrap();
        assert_eq!(status.mode, RecordingMode::Transcript);
        assert_eq!(
            status.file_path.unwrap(),
            root.join("recordings/override/fixture.log")
        );
    });
    cx.update_entity(&app, |app, cx| {
        app.stop_recording_for_session("fixture", cx)
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && !app.recording.is_recording("fixture")
    });
    cx.update_entity(&app, |app, cx| {
        app.start_recording_for_session("fixture", RecordingMode::Raw, cx)
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && app.recording.is_recording("fixture")
    });
    cx.read(|cx| {
        assert_eq!(
            app.read(cx).recording.status("fixture").unwrap().mode,
            RecordingMode::Raw
        )
    });
    cx.update_entity(&app, |app, cx| {
        app.stop_recording_for_session("fixture", cx)
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && !app.recording.is_recording("fixture")
    });
}

#[gpui::test]
fn recording_failures_and_closed_sessions_release_busy_state(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-manual-recording-failure");
    let app = fixture(cx, &root);
    fs::write(root.join("blocked"), b"existing data").unwrap();
    cx.update_entity(&app, |app, cx| {
        let mut settings = app.settings.summary().clone();
        settings.recording_path = root.join("blocked").to_string_lossy().into_owned();
        app.settings.replace_summary(settings);
        app.toggle_recording_for_session("fixture", cx);
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none()
    });
    cx.read(|cx| {
        assert!(
            app.read(cx)
                .shell
                .status()
                .contains(rust_i18n::t!("recording.startFailed").as_ref())
        )
    });
    assert_eq!(fs::read(root.join("blocked")).unwrap(), b"existing data");
    cx.update_entity(&app, |app, cx| {
        app.save_session_transcript_for_session("fixture", cx);
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none()
    });
    cx.read(|cx| {
        assert!(
            app.read(cx)
                .shell
                .status()
                .contains(rust_i18n::t!("recording.saveFailed").as_ref())
        )
    });
    cx.update_entity(&app, |app, cx| {
        let mut settings = app.settings.summary().clone();
        settings.recording_path = root.join("recordings").to_string_lossy().into_owned();
        app.settings.replace_summary(settings);
        app.toggle_recording_for_session("fixture", cx);
        app.session.metadata_mut("fixture").unwrap().disconnected = true;
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none() && !app.recording.is_recording("fixture")
    });
    cx.read(|cx| {
        assert_eq!(
            app.read(cx).shell.status(),
            rust_i18n::t!("recording.startCancelled").as_ref()
        )
    });
    cx.update_entity(&app, |app, cx| {
        app.toggle_recording_for_session("missing", cx)
    });
    cx.read(|cx| assert!(app.read(cx).recording.busy_action("missing").is_none()));
    assert_terminal_unchanged(cx, &app);
}

#[gpui::test]
fn transcript_export_keeps_existing_destination_and_reserves_unique_name(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-manual-export-unique");
    let app = fixture(cx, &root);
    let destination = root.join("session.log");
    fs::write(&destination, b"existing data").unwrap();
    cx.update_entity(&app, |app, cx| {
        app.recording.write_output("fixture", "synthetic output\n");
        app.save_transcript_to_path(
            "fixture",
            destination.to_string_lossy().into_owned(),
            true,
            cx,
        );
        app.session.select_active_session("other");
    });
    wait_for(cx, &app, |app| {
        app.recording.busy_action("fixture").is_none()
    });
    assert_eq!(fs::read(destination).unwrap(), b"existing data");
    assert!(
        fs::read_to_string(root.join("session-1.log"))
            .unwrap()
            .contains("synthetic output")
    );
    assert_terminal_unchanged(cx, &app);
}
