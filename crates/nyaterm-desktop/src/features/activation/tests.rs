use nyaterm_core::{ConnectionType, ExternalConnectionRequest, TelnetAutoLoginConfig};

use super::external_request_matches_connection;

fn ssh_connection(username: &str) -> ConnectionType {
    ConnectionType::Ssh {
        host: "example.COM".to_string(),
        port: 22,
        username: username.to_string(),
        backspace_mode: String::new(),
        ai_execution_profile: Default::default(),
        x11_forwarding: false,
        auth_agent_endpoint: None,
        agent_forwarding_config: None,
        legacy_agent_forwarding: None,
        encoding: String::new(),
        dynamic_tab_title: false,
    }
}

#[test]
fn explicit_ssh_username_disambiguates_saved_connections() {
    let request = ExternalConnectionRequest::Ssh {
        host: "EXAMPLE.com".to_string(),
        port: 22,
        username: Some("deploy".to_string()),
        password: None,
        runtime_mode: None,
    };
    assert!(external_request_matches_connection(
        &request,
        &ssh_connection("deploy")
    ));
    assert!(!external_request_matches_connection(
        &request,
        &ssh_connection("root")
    ));
}

#[test]
fn telnet_match_requires_protocol_host_and_port() {
    let request = ExternalConnectionRequest::Telnet {
        host: "example.com".to_string(),
        port: 23,
    };
    let connection = ConnectionType::Telnet {
        host: "EXAMPLE.COM".to_string(),
        port: 23,
        username: String::new(),
        ai_execution_profile: Default::default(),
        backspace_mode: String::new(),
        raw_tcp_cli: false,
        enter_mode: String::new(),
        local_echo: false,
        local_line_edit: false,
        force_character_at_a_time: false,
        send_naws: true,
        send_sga: true,
        keep_alive_interval_secs:
            nyaterm_core::models::connection::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS,
        auto_login: TelnetAutoLoginConfig::default(),
        encoding: String::new(),
    };
    assert!(external_request_matches_connection(&request, &connection));
}

struct DialogHost;
impl gpui::Render for DialogHost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::Styled as _;
        gpui::div().size_full()
    }
}

#[test]
fn external_queue_waits_for_restoration_unlock_and_dialogs_and_cancel_keeps_the_next_item() {
    use super::state::ExternalDialog;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{ActivationRequest, SavedConnection};
    use nyaterm_ui::NyaDialogWindowExt as _;

    let root = TestConfigDir::new("nyaterm-external-queue");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        let saved = |id| -> SavedConnection { serde_json::from_value(serde_json::json!({"id":id, "name":id, "type":"ssh", "host":"example.com", "username":"root"})).unwrap() };
        app.connection_state.replace_loaded(vec![saved("first"), saved("second")], Vec::new());
        let request = ActivationRequest::from_os_args([1;16], [std::ffi::OsString::from("ssh://example.com"), std::ffi::OsString::from("ssh://example.com?mode=terminal")]);
        app.handle_activation(request.parse(), cx);
        assert_eq!(app.activation.pending.len(), 2);
        app.shell.mark_terminal_windows_restore_pending();
    });
    let fixture = cx.new(|_| DialogHost);
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.pump_external_activation(window, cx));
            assert_eq!(app.activation.pending.len(), 2);
            app.session.mark_restore_complete();
            assert!(app.pump_external_activation(window, cx));
            assert_eq!(app.activation.pending.len(), 2);
            app.shell.complete_terminal_windows_restore();
            app.security.activate_screen_lock(String::new());
            assert!(app.pump_external_activation(window, cx));
            assert_eq!(app.activation.pending.len(), 2);
            app.security.deactivate_screen_lock();
            window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Other operation"));
            assert!(app.pump_external_activation(window, cx));
            assert_eq!(app.activation.pending.len(), 2);
            window.close_nya_dialog(cx);
            assert!(app.pump_external_activation(window, cx));
            assert!(matches!(
                app.activation.dialog,
                Some(ExternalDialog::Choose { .. })
            ));
            assert_eq!(app.activation.pending.len(), 1);
            app.pump_external_activation(window, cx);
            assert!(window.has_active_nya_dialog(cx));
        })
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(app.activation.dialog.is_none());
            assert_eq!(app.activation.pending.len(), 1);
            // A completed/failed start leaves no busy reservation. The next item
            // must advance without requiring a new activation to wake it.
            app.activation.waiting_for_start = true;
            app.pump_external_activation(window, cx);
            assert!(matches!(
                app.activation.dialog,
                Some(ExternalDialog::Choose { .. })
            ));
            assert!(app.activation.pending.is_empty());
            app.cancel_external_activations();
            assert!(app.activation.is_empty());
        })
    });
}

#[test]
fn one_time_password_bypasses_saved_credentials_and_never_enters_restore_documents() {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::TestAppContext;
    use nyaterm_core::models::ssh_runtime::SshRuntimeMode;
    use nyaterm_core::{AiExecutionProfile, SecretString};
    use nyaterm_transport::SshSessionConfig;
    let request = ExternalConnectionRequest::Ssh {
        host: "example.com".into(),
        port: 22,
        username: Some("root".into()),
        password: Some(SecretString::new("one-time-never-save")),
        runtime_mode: Some(SshRuntimeMode::Sftp),
    };
    assert!(!external_request_matches_connection(
        &request,
        &ssh_connection("root")
    ));
    let root = TestConfigDir::new("nyaterm-external-restore");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    app.update(&mut cx, |app, _| {
        let config = SshSessionConfig {
            host: "example.com".into(),
            username: "root".into(),
            password: Some("one-time-never-save".into()),
            runtime_mode: SshRuntimeMode::Sftp,
            ..Default::default()
        };
        assert!(!format!("{config:?}").contains("one-time-never-save"));
        app.session.register_session_metadata(
            "temporary",
            SessionRuntimeMetadata {
                ssh_config: Some(config.clone()),
                ssh_multiplex_key: None,
                source_connection_id: None,
                ai_execution_profile: AiExecutionProfile::Disabled,
                launch_config: SessionLaunchConfig::Ssh(Box::new(config)),
                disconnected: false,
            },
        );
        let tabs = app.serialize_open_tabs();
        let serialized = serde_json::to_string(&tabs).unwrap();
        assert!(!serialized.contains("one-time-never-save"));
        assert!(
            tabs.iter()
                .any(|tab| tab.ssh_runtime_mode.as_deref() == Some("sftp"))
        );
    });
}

#[test]
fn split_restore_serialization_preserves_each_sessions_runtime_mode() {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{
        SessionLaunchConfig, SessionRuntimeMetadata, WorkspacePaneNode, WorkspaceSplitDirection,
    };
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::TestAppContext;
    use nyaterm_core::AiExecutionProfile;
    use nyaterm_core::models::ssh_runtime::SshRuntimeMode;
    use nyaterm_core::models::workspace::RestorableOpenTab;
    use nyaterm_transport::SshSessionConfig;

    let root = TestConfigDir::new("nyaterm-external-split-restore");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "first");
    app.update(&mut cx, |app, _| {
        for (id, mode) in [
            ("first", SshRuntimeMode::Terminal),
            ("second", SshRuntimeMode::Sftp),
        ] {
            let config = SshSessionConfig {
                runtime_mode: mode,
                ..Default::default()
            };
            app.session.register_session_metadata(
                id,
                SessionRuntimeMetadata {
                    ssh_config: Some(config.clone()),
                    ssh_multiplex_key: None,
                    source_connection_id: Some(format!("saved-{id}")),
                    ai_execution_profile: AiExecutionProfile::Disabled,
                    launch_config: SessionLaunchConfig::Ssh(Box::new(config)),
                    disconnected: false,
                },
            );
        }
        app.shell.insert_workspace_pane_root(
            "first".into(),
            WorkspacePaneNode::Split {
                id: "split".into(),
                direction: WorkspaceSplitDirection::Vertical,
                ratio_percent: 50,
                first: Box::new(WorkspacePaneNode::leaf("first")),
                second: Box::new(WorkspacePaneNode::leaf("second")),
            },
        );
        let tabs = app.serialize_open_tabs();
        let restored: Vec<RestorableOpenTab> =
            serde_json::from_str(&serde_json::to_string(&tabs).unwrap()).unwrap();
        assert_eq!(restored.len(), 1);
        let sessions = restored[0].expanded_sessions();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].ssh_runtime_mode.as_deref(), Some("terminal"));
        assert_eq!(sessions[1].ssh_runtime_mode.as_deref(), Some("sftp"));
    });
}
