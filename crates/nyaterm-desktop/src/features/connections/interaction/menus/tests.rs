use gpui::TestAppContext;
use nyaterm_core::SavedConnection;
use nyaterm_core::models::ssh_runtime::SshRuntimeMode;

use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn connection(kind: &str, enabled: bool) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": "saved", "name": "Saved SSH", "type": kind,
        "host": "127.0.0.1", "username": "fixture",
        "sftp": { "enabled": enabled },
    }))
    .unwrap()
}

#[test]
fn opening_file_manager_rechecks_deleted_changed_and_disabled_connections_before_starting() {
    let root = TestConfigDir::new("nyaterm-saved-file-manager-revalidation");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let cx = session_focus_test_window(&mut cx, &app);
    for (replacement, error) in [
        (None, "savedConnections.fileManagerConnectionUnavailable"),
        (
            Some(connection("local_terminal", true)),
            "savedConnections.fileManagerRequiresSsh",
        ),
        (
            Some(connection("ssh", false)),
            "savedConnections.fileManagerSftpDisabled",
        ),
    ] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.connection_state
                    .replace_loaded(vec![connection("ssh", true)], Vec::new());
                // Simulate a configuration change while the action is available.
                app.connection_state
                    .replace_loaded(replacement.into_iter().collect(), Vec::new());
                app.open_saved_connection_file_manager_by_id("saved", window, cx);
                assert_eq!(app.shell.status(), rust_i18n::t!(error));
                assert!(!app.session.start_has_pending());
                assert_eq!(app.session.session_ids().collect::<Vec<_>>(), ["existing"]);
                assert_eq!(app.session.active_id(), Some("existing"));
            });
        });
    }
}

#[test]
fn opening_file_manager_uses_latest_saved_configuration_and_starts_independent_sftp_sessions() {
    let root = TestConfigDir::new("nyaterm-saved-file-manager-start");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let cx = session_focus_test_window(&mut cx, &app);
    // Keep any background connection attempt confined to a local fixture.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.connection_state
                .replace_loaded(vec![connection("ssh", true)], Vec::new());
            let latest: SavedConnection = serde_json::from_value(serde_json::json!({
                "id": "saved", "name": "Latest SSH", "type": "ssh",
                "host": "127.0.0.1", "port": listener.local_addr().unwrap().port(),
                "username": "fixture", "post_login": { "enabled": true, "command": "echo test" },
            }))
            .unwrap();
            app.connection_state
                .replace_loaded(vec![latest.clone()], Vec::new());
            for _ in 0..2 {
                app.open_saved_connection_file_manager_by_id("saved", window, cx);
            }
            let request_ids: Vec<_> = app
                .session
                .start_pending_entries()
                .map(|(id, pending)| {
                    assert_eq!(
                        pending.requested_ssh_runtime_mode,
                        Some(SshRuntimeMode::Sftp)
                    );
                    assert_eq!(pending.source_connection_id.as_deref(), Some("saved"));
                    assert_eq!(pending.connection_name, "Latest SSH");
                    assert!(pending.startup_command.is_none());
                    id.clone()
                })
                .collect();
            assert_eq!(request_ids.len(), 2);
            assert_eq!(app.session.session_ids().collect::<Vec<_>>(), ["existing"]);
            assert_eq!(
                serde_json::to_value(app.connection_state.connection_by_id("saved").unwrap())
                    .unwrap(),
                serde_json::to_value(latest).unwrap(),
            );
            for id in request_ids {
                app.close_pending_session_start(id, cx);
            }
            assert!(!app.session.start_has_pending());
        });
    });
}
