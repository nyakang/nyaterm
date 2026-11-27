use gpui::TestAppContext;
use nyaterm_remote_desktop::{RdpBackend, RdpSessionConfig};

use super::policy::{SessionToolbarAction, SessionToolbarContext, ToolbarCapabilities};
use crate::features::layout::title_bar::definition::{TitleMenuCommand, TitleMenuKind};
use crate::features::test_support::{app_with_visible_ssh_session, session_focus_test_window};
use crate::models::{HeaderStatusMode, SessionLaunchConfig};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn context(capabilities: ToolbarCapabilities, mode: HeaderStatusMode) -> SessionToolbarContext {
    SessionToolbarContext {
        session_id: Some("pane".into()),
        capabilities,
        preferred_mode: mode,
        visible: true,
        fullscreen_active: false,
    }
}

#[test]
fn temporary_host_loss_keeps_choices_but_falls_back_without_changing_preference() {
    for mode in HeaderStatusMode::ALL {
        let mut context = context(
            ToolbarCapabilities {
                host: true,
                accelerators: true,
                host_available: true,
                ..Default::default()
            },
            mode,
        );
        assert_eq!(context.effective_mode(), mode);
        context.capabilities.host_available = false;
        assert!(context.supports_mode(mode));
        assert_eq!(
            context.effective_mode(),
            if mode == HeaderStatusMode::DateTime {
                mode
            } else {
                HeaderStatusMode::Session
            }
        );
        assert_eq!(context.preferred_mode, mode);
        context.capabilities.host_available = true;
        assert_eq!(context.effective_mode(), mode);
        context.visible = false;
        assert!(
            HeaderStatusMode::ALL
                .into_iter()
                .all(|mode| !context.displays(mode))
        );
    }
}

#[test]
fn unsupported_modes_are_hidden_and_accelerators_can_be_restricted_separately() {
    let mut context = context(ToolbarCapabilities::default(), HeaderStatusMode::Gpu);
    assert_eq!(
        HeaderStatusMode::ALL
            .into_iter()
            .filter(|mode| context.supports_mode(*mode))
            .collect::<Vec<_>>(),
        vec![HeaderStatusMode::Session, HeaderStatusMode::DateTime]
    );
    context.capabilities.host = true;
    context.capabilities.host_available = true;
    assert!(context.mode_enabled(HeaderStatusMode::Resources));
    assert!(context.mode_enabled(HeaderStatusMode::Host));
    assert!(!context.supports_mode(HeaderStatusMode::Gpu));
    assert!(!context.supports_mode(HeaderStatusMode::Npu));
}

#[test]
fn narrow_layout_folds_actions_and_live_reconnect_never_gets_a_shortcut() {
    let mut context = context(
        ToolbarCapabilities {
            fullscreen: Some(true),
            reconnect: Some(false),
            ..Default::default()
        },
        HeaderStatusMode::Session,
    );
    assert!(context.shortcut_actions(1023.).is_empty());
    assert_eq!(
        context.shortcut_actions(1024.),
        vec![SessionToolbarAction::Fullscreen]
    );
    context.capabilities.reconnect = Some(true);
    assert_eq!(
        context.shortcut_actions(1200.),
        vec![SessionToolbarAction::Fullscreen]
    );
    context.capabilities.disconnected = true;
    assert_eq!(
        context.shortcut_actions(1200.),
        vec![
            SessionToolbarAction::Fullscreen,
            SessionToolbarAction::Reconnect
        ]
    );
}

fn rdp_config() -> RdpSessionConfig {
    RdpSessionConfig {
        relay: None,
        backend: RdpBackend::Builtin,
        windows_authentication: None,
        name: "RDP fixture".into(),
        host: "127.0.0.1".into(),
        port: 1,
        username: String::new(),
        domain: String::new(),
        password: None,
        use_nla: false,
        certificate_policy: Default::default(),
        administrative_session: false,
        display: Default::default(),
        clipboard: Default::default(),
        reconnect: Default::default(),
        redirection: Default::default(),
    }
}

#[test]
fn ssh_rdp_switch_restores_monitor_preference_and_demand() {
    let root = TestConfigDir::new("nyaterm-session-toolbar-switch");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, |app, _| {
        app.settings.set_header_status_mode("resources".into());
        let mut remote = app.session.metadata("ssh").unwrap().clone();
        remote.ssh_config = None;
        remote.launch_config = SessionLaunchConfig::Rdp(rdp_config());
        remote.disconnected = true;
        app.session.register_session_metadata("rdp", remote);
        app.remote_desktop.insert_disconnected("rdp".into());
        assert!(app.header_status_needs_stats());
        app.session.select_active_session("rdp");
        let context = app.session_toolbar_context();
        assert_eq!(context.effective_mode(), HeaderStatusMode::Session);
        assert_eq!(
            context.action_enabled(SessionToolbarAction::Reconnect),
            Some(true)
        );
        assert!(!app.header_status_needs_stats());
        assert!(!context.accepts_action("ssh", SessionToolbarAction::Reconnect));
        assert_eq!(app.settings.summary().ui_header_status_mode, "resources");
        app.session.select_active_session("ssh");
        assert!(app.header_status_needs_stats());
        assert_eq!(
            app.session_toolbar_context().effective_mode(),
            HeaderStatusMode::Resources
        );
        app.session.metadata_mut("ssh").unwrap().disconnected = true;
        assert!(!app.header_status_needs_stats());
        assert!(
            app.session_toolbar_context()
                .supports_mode(HeaderStatusMode::Resources)
        );
        app.session.metadata_mut("ssh").unwrap().disconnected = false;
        assert!(app.header_status_needs_stats());
        app.settings.set_header_status_visible(false);
        assert!(!app.header_status_needs_stats());
        app.settings.set_header_status_visible(true);
        assert!(app.header_status_needs_stats());
        assert_eq!(app.settings.summary().ui_header_status_mode, "resources");
    });
}

#[test]
fn ssh_compatibility_and_sftp_modes_remove_host_choices() {
    let root = TestConfigDir::new("nyaterm-session-toolbar-ssh-capabilities");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, |app, _| {
        app.settings.set_header_status_mode("gpu".into());
        assert!(app.header_status_needs_gpu());
        app.session
            .metadata_mut("ssh")
            .unwrap()
            .ssh_config
            .as_mut()
            .unwrap()
            .ssh_compatibility_mode = true;
        assert!(
            !app.session_toolbar_context()
                .supports_mode(HeaderStatusMode::Resources)
        );
        assert!(!app.header_status_needs_gpu());
        let config = app
            .session
            .metadata_mut("ssh")
            .unwrap()
            .ssh_config
            .as_mut()
            .unwrap();
        config.ssh_compatibility_mode = false;
        config.runtime_mode = nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp;
        assert!(
            !app.session_toolbar_context()
                .supports_mode(HeaderStatusMode::Gpu)
        );
        assert_eq!(app.settings.summary().ui_header_status_mode, "gpu");
    });
}

#[test]
fn dates_survive_context_changes_and_hidden_toolbar_has_no_clock_demand() {
    let root = TestConfigDir::new("nyaterm-session-toolbar-clock");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, |app, _| {
        app.settings.set_header_status_mode("datetime".into());
        assert!(app.header_status_clock_should_run());
        app.session.metadata_mut("ssh").unwrap().launch_config =
            SessionLaunchConfig::Rdp(rdp_config());
        assert!(app.header_status_clock_should_run());
        app.session.select_active_session("missing");
        assert!(app.header_status_clock_should_run());
        assert!(app.session_toolbar_context().session_id.is_none());
        app.settings.set_header_status_visible(false);
        assert!(!app.header_status_clock_should_run());
    });
}

#[test]
fn menu_and_dispatch_reject_old_targets_and_busy_sessions() {
    let root = TestConfigDir::new("nyaterm-session-toolbar-stale-action");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    let vcx = session_focus_test_window(&mut cx, &app);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let command = TitleMenuCommand::SessionToolbarAction {
                session_id: "ssh".into(),
                action: SessionToolbarAction::Reconnect,
            };
            assert!(app.title_menu_command_enabled(&command, cx));
            let other = app.session.metadata("ssh").unwrap().clone();
            app.session.register_session_metadata("other", other);
            app.session.select_active_session("other");
            assert!(!app.title_menu_command_enabled(&command, cx));
            app.execute_session_toolbar_action("ssh", SessionToolbarAction::Reconnect, window, cx);
            assert!(!app.session.start_reconnect_is_pending("ssh"));
            assert_eq!(app.session.active_id(), Some("other"));
            app.session.begin_reconnect_action("other".into());
            let context = app.session_toolbar_context();
            assert!(!context.accepts_action("other", SessionToolbarAction::Reconnect));
            let menu = app.session_toolbar_menu_definitions();
            assert!(menu.iter().any(|item| item.disabled
                && matches!(
                    &item.kind,
                    TitleMenuKind::Action(Some(TitleMenuCommand::SessionToolbarAction {
                        action: SessionToolbarAction::Reconnect,
                        ..
                    }))
                )));
            app.execute_session_toolbar_action(
                "other",
                SessionToolbarAction::Reconnect,
                window,
                cx,
            );
            assert!(!app.session.start_reconnect_is_pending("other"));
        });
    });
}

#[test]
fn show_command_restores_saved_preference_without_selecting_fallback() {
    let root = TestConfigDir::new("nyaterm-session-toolbar-show");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    let vcx = session_focus_test_window(&mut cx, &app);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            for value in [
                "session",
                "resources",
                "host",
                "datetime",
                "gpu",
                "npu",
                "future-mode",
            ] {
                app.settings.set_header_status_mode(value.into());
                app.settings.set_header_status_visible(false);
                assert!(app.title_menu_command_enabled(&TitleMenuCommand::ShowHeaderStatus, cx));
                app.execute_title_menu_command(&TitleMenuCommand::ShowHeaderStatus, window, cx);
                assert!(app.settings.summary().ui_header_status_visible);
                assert_eq!(app.settings.summary().ui_header_status_mode, value);
            }
        })
    });
}

#[test]
fn split_pane_focus_and_settings_tab_cannot_inherit_another_panes_actions() {
    use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};

    let root = TestConfigDir::new("nyaterm-session-toolbar-panes");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, |app, cx| {
        app.settings.set_header_status_mode("resources".into());
        let mut remote = app.session.metadata("ssh").unwrap().clone();
        remote.ssh_config = None;
        remote.launch_config = SessionLaunchConfig::Rdp(rdp_config());
        app.session.register_session_metadata("rdp", remote);
        app.remote_desktop.insert_disconnected("rdp".into());
        app.shell.insert_workspace_pane_root(
            "ssh".into(),
            WorkspacePaneNode::Split {
                id: "split".into(),
                direction: WorkspaceSplitDirection::Vertical,
                ratio_percent: 50,
                first: Box::new(WorkspacePaneNode::leaf("ssh")),
                second: Box::new(WorkspacePaneNode::leaf("rdp")),
            },
        );
        app.rebuild_session_tab_owners();
        app.activate_workspace_pane("rdp".into(), cx);
        assert_eq!(app.tab_root_for_session("rdp"), "ssh");
        let context = app.session_toolbar_context();
        assert_eq!(context.session_id.as_deref(), Some("rdp"));
        assert!(context.accepts_action("rdp", SessionToolbarAction::Reconnect));
        assert!(!context.accepts_action("ssh", SessionToolbarAction::Reconnect));
        assert!(!app.header_status_needs_stats());
        app.activate_workspace_pane("ssh".into(), cx);
        assert!(app.header_status_needs_stats());
        app.open_settings_workspace_tab(cx);
        assert!(app.session_toolbar_context().session_id.is_none());
        assert!(!app.header_status_needs_stats());
        assert_eq!(app.settings.summary().ui_header_status_mode, "resources");
    });
}

#[test]
fn active_pending_and_failed_start_never_target_the_previous_session() {
    use crate::features::session::PendingSessionStart;

    let root = TestConfigDir::new("nyaterm-session-toolbar-start-page");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh");
    app.update(&mut cx, |app, _| {
        app.settings.set_header_status_mode("resources".into());
        app.session.simulate_pending_start(
            "pending",
            PendingSessionStart {
                preparing: false,
                offer_serial_reconnect_selection: false,
                mcp_open_operation_id: None,
                requested_ssh_runtime_mode: None,
                attempt: Default::default(),
                connection_name: "New connection".into(),
                launch_config: None,
                requested_at: std::time::Instant::now(),
                kind: nyaterm_transport::SessionKind::Ssh,
                ai_execution_profile: Default::default(),
                custom_name: None,
                tab_color: None,
                locked: false,
                after_session_id: None,
                insert_index: None,
                seed_output: None,
                startup_command: None,
                multiplex_key: None,
                source_connection_id: None,
                workspace_split: None,
                tab_placement: None,
                reconnect_session_id: None,
            },
        );
        assert_eq!(app.session.active_id(), Some("ssh"));
        assert!(app.session_toolbar_context().session_id.is_none());
        assert!(!app.header_status_needs_stats());
        app.session.simulate_start_failure("pending");
        assert!(app.session.start_has_active_failed());
        assert!(app.session_toolbar_context().session_id.is_none());
        assert!(!app.header_status_needs_stats());
        app.session.clear_active_start_selection();
        assert!(app.header_status_needs_stats());
    });
}
