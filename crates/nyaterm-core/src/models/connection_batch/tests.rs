use super::{
    BatchField, ConnectionBatchEdit, ConnectionBatchSkipReason, RdpBatchFieldGroup,
    apply_connection_batch_edit, rdp_batch_field_skip_reason, validate_connection_batch_edit,
};
use crate::SecretString;
use crate::models::{
    AiExecutionProfile, ConnectionAuth, ConnectionExtensions, ConnectionNetwork,
    ConnectionPasswordSource, ConnectionPostLogin, ConnectionType, ConnectionUsernameSource,
    RecordingMode, SavedConnection, SerialFlowControl, SftpCwdFollowMode, SftpSettings,
    SshAgentEndpoint, SshAgentForwardingConfig, SshAgentForwardingPolicy, SshAlgorithmMode,
    SshAlgorithmPreferences, SshProfile, SshTerminalType,
};

fn ssh_connection() -> SavedConnection {
    SavedConnection {
        id: "ssh-1".to_string(),
        name: "SSH".to_string(),
        config: ConnectionType::Ssh {
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            ai_execution_profile: AiExecutionProfile::default(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: Some("group-a".to_string()),
        description: Some("original".to_string()),
        tags: vec!["prod".to_string()],
        tree_order: None,
        sort_order: 7,
        icon: Some("server".to_string()),
        icon_auto_detect: Some(true),
        auth: None,
        network: None,
        post_login: Some(ConnectionPostLogin {
            enabled: true,
            command: "uptime".to_string(),
            delay_ms: 1000,
        }),
        recording: None,
        ssh_algorithms: Some(SshAlgorithmPreferences {
            mode: SshAlgorithmMode::Secure,
            kex: vec!["curve25519-sha256".to_string()],
            ciphers: vec!["aes256-gcm".to_string()],
            macs: vec!["hmac-sha2-256".to_string()],
            host_keys: vec!["ssh-ed25519".to_string()],
        }),
        allow_short_rsa_host_keys: false,
        ssh_compatibility_mode: true,
        ssh_profile: SshProfile::NetworkDevice,
        terminal_type: Some(SshTerminalType::Vt100),
        sftp: SftpSettings {
            compatibility_mode: true,
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::RcFile,
            shell_detection_timeout_ms: 2500,
            filename_encoding: "utf-8".to_string(),
            pipeline_depth: Some(4),
            extra: Default::default(),
        },
        asset: None,
        created_at_ms: Some(1),
        updated_at_ms: Some(2),
        last_used_at_ms: Some(3),
        extensions: ConnectionExtensions::default(),
    }
}

fn local_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::LocalTerminal {
            shell_path: "/bin/bash".to_string(),
            shell_args: String::new(),
            working_dir: None,
            ai_execution_profile: AiExecutionProfile::default(),
            encoding: "utf-8".to_string(),
            dynamic_tab_title: false,
        },
        ..ssh_connection()
    }
}

fn rdp_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::Rdp {
            host: "10.0.0.2".to_string(),
            port: 3389,
            username: "admin".to_string(),
            domain: String::new(),
            backend: "builtin".to_string(),
            security: Default::default(),
            display: Default::default(),
            clipboard: Default::default(),
            reconnect: Default::default(),
        },
        ..ssh_connection()
    }
}

fn vnc_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::Vnc {
            username: String::new(),
            host: "10.0.0.3".to_string(),
            port: 5900,
            security: Default::default(),
            display: Default::default(),
            clipboard: Default::default(),
            reconnect: Default::default(),
            shared: true,
            view_only: false,
        },
        ..ssh_connection()
    }
}

fn telnet_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::Telnet {
            host: "10.0.0.4".to_string(),
            port: crate::models::default_telnet_port(),
            username: "guest".to_string(),
            ai_execution_profile: AiExecutionProfile::default(),
            backspace_mode: crate::models::default_backspace_mode_telnet(),
            raw_tcp_cli: false,
            enter_mode: crate::models::default_telnet_enter_mode(),
            local_echo: false,
            local_line_edit: false,
            force_character_at_a_time: false,
            send_naws: true,
            send_sga: true,
            keep_alive_interval_secs: 60,
            auto_login: Default::default(),
            encoding: String::new(),
        },
        ..ssh_connection()
    }
}

fn serial_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::Serial {
            port_name: "COM1".to_string(),
            baud_rate: 115_200,
            data_bits: 8,
            parity: crate::models::default_parity(),
            flow_control: SerialFlowControl::default(),
            stop_bits: crate::models::default_stop_bits(),
            connect_prompt: Default::default(),
            ai_execution_profile: AiExecutionProfile::default(),
            backspace_mode: crate::models::default_backspace_mode_serial(),
            encoding: String::new(),
        },
        ..ssh_connection()
    }
}

#[test]
fn unchanged_edit_leaves_connection_untouched() {
    let original = ssh_connection();
    let mut connection = original.clone();
    let edit = ConnectionBatchEdit::default();

    assert!(edit.is_empty());
    assert!(!edit.has_changes());
    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert!(!report.changed());
    assert_eq!(connection, original);
}

#[test]
fn basic_fields_only_write_selected_ones() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        description: BatchField::Set("managed".to_string()),
        port: BatchField::Set(2222),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert_eq!(report.applied_fields, 2);
    assert_eq!(connection.description.as_deref(), Some("managed"));
    assert_eq!(connection.icon.as_deref(), Some("server"));
    assert_eq!(connection.group_id.as_deref(), Some("group-a"));
    match connection.config {
        ConnectionType::Ssh { port, .. } => assert_eq!(port, 2222),
        _ => panic!("expected ssh"),
    }
}

#[test]
fn clear_restores_field_defaults() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        group_id: BatchField::Clear,
        description: BatchField::Clear,
        icon: BatchField::Clear,
        icon_auto_detect: BatchField::Clear,
        port: BatchField::Clear,
        username: BatchField::Clear,
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    assert!(connection.group_id.is_none());
    assert!(connection.description.is_none());
    assert!(connection.icon.is_none());
    assert!(connection.icon_auto_detect.is_none());
    match connection.config {
        ConnectionType::Ssh { port, username, .. } => {
            assert_eq!(port, 22);
            assert_eq!(username, "root");
        }
        _ => panic!("expected ssh"),
    }
}

#[test]
fn endpoint_fields_are_skipped_for_non_endpoint_types() {
    let original = local_connection();
    let mut connection = original.clone();
    let edit = ConnectionBatchEdit {
        port: BatchField::Set(1234),
        username: BatchField::Set("someone".to_string()),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert_eq!(report.applied_fields, 0);
    assert_eq!(report.skipped_fields, 2);
    assert_eq!(connection, original);
}

#[test]
fn sftp_partial_fields_keep_untouched_values_and_extra() {
    let mut connection = ssh_connection();
    connection.sftp.extra.insert(
        "future_sftp_field".to_string(),
        serde_json::Value::String("keep".to_string()),
    );
    let edit = ConnectionBatchEdit {
        sftp_enabled: BatchField::Set(false),
        sftp_pipeline_depth: BatchField::Clear,
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert_eq!(report.applied_fields, 2);
    assert!(!connection.sftp.enabled);
    assert_eq!(connection.sftp.pipeline_depth, None);
    assert_eq!(connection.sftp.shell_detection_timeout_ms, 2500);
    assert_eq!(connection.sftp.filename_encoding, "utf-8");
    assert_eq!(
        connection.sftp.extra.get("future_sftp_field"),
        Some(&serde_json::Value::String("keep".to_string()))
    );
}

#[test]
fn sftp_fields_are_skipped_for_non_ssh_connections() {
    let original = rdp_connection();
    let mut connection = original.clone();
    let edit = ConnectionBatchEdit {
        sftp_enabled: BatchField::Set(false),
        sftp_cwd_follow_mode: BatchField::Set(SftpCwdFollowMode::Off),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert_eq!(report.applied_fields, 0);
    assert_eq!(report.skipped_fields, 2);
    assert_eq!(connection, original);
}

#[test]
fn post_login_partial_writes_preserve_other_fields() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        post_login_command: BatchField::Set("echo hello".to_string()),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    let post_login = connection.post_login.expect("post login");
    assert!(post_login.enabled);
    assert_eq!(post_login.command, "echo hello");
    assert_eq!(post_login.delay_ms, 1000);
}

#[test]
fn post_login_clear_removes_the_object() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        post_login_enabled: BatchField::Clear,
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    assert!(connection.post_login.is_none());
}

#[test]
fn ssh_algorithm_fields_only_change_selected_lists() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        ssh_kex: BatchField::Set(vec!["sntrup761x25519-sha512".to_string()]),
        ssh_macs: BatchField::Clear,
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    let algorithms = connection.ssh_algorithms.expect("algorithms");
    assert_eq!(algorithms.mode, SshAlgorithmMode::Secure);
    assert_eq!(algorithms.kex, vec!["sntrup761x25519-sha512"]);
    assert!(algorithms.macs.is_empty());
    assert_eq!(algorithms.ciphers, vec!["aes256-gcm"]);
    assert_eq!(algorithms.host_keys, vec!["ssh-ed25519"]);
}

#[test]
fn security_fields_apply_only_to_matching_protocols() {
    let mut rdp = rdp_connection();
    let edit = ConnectionBatchEdit {
        rdp_use_nla: BatchField::Set(false),
        rdp_administrative_session: BatchField::Set(true),
        vnc_security_mode: BatchField::Set("vnc-password".to_string()),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut rdp, &edit);

    assert_eq!(report.applied_fields, 2);
    assert_eq!(report.skipped_fields, 1);
    match rdp.config {
        ConnectionType::Rdp { security, .. } => {
            assert!(!security.use_nla);
            assert!(security.administrative_session);
        }
        _ => panic!("expected rdp"),
    }

    let mut vnc = vnc_connection();
    apply_connection_batch_edit(&mut vnc, &edit);
    match vnc.config {
        ConnectionType::Vnc { security, .. } => assert_eq!(security.mode, "vnc-password"),
        _ => panic!("expected vnc"),
    }
}

#[test]
fn network_alias_keeps_existing_proxy_references() {
    let mut connection = ssh_connection();
    connection.network = Some(ConnectionNetwork {
        proxy_id: Some("proxy-1".to_string()),
        proxy_jump_id: None,
        host_key_alias: Some("old-alias".to_string()),
    });
    let edit = ConnectionBatchEdit {
        host_key_alias: BatchField::Set("new-alias".to_string()),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    let network = connection.network.expect("network");
    assert_eq!(network.host_key_alias.as_deref(), Some("new-alias"));
    assert_eq!(network.proxy_id.as_deref(), Some("proxy-1"));
}

#[test]
fn unknown_top_level_fields_survive_apply_and_serialize() {
    let mut value = serde_json::to_value(ssh_connection()).expect("serialize");
    value["future_connection_field"] = serde_json::Value::String("keep".to_string());
    let mut connection: SavedConnection = serde_json::from_value(value).expect("deserialize");
    let edit = ConnectionBatchEdit {
        description: BatchField::Set("updated".to_string()),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    let value = serde_json::to_value(&connection).expect("serialize");
    assert_eq!(value["future_connection_field"], "keep");
}

#[test]
fn tags_and_recording_fields_apply() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        tags: BatchField::Set(vec!["ops".to_string()]),
        recording_use_global: BatchField::Set(false),
        recording_auto_start: BatchField::Set(true),
        recording_mode: BatchField::Set(RecordingMode::Raw),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert_eq!(report.applied_fields, 4);
    assert_eq!(connection.tags, vec!["ops".to_string()]);
    let recording = connection.recording.expect("recording settings");
    assert_eq!(recording.auto_start, Some(true));
    assert_eq!(recording.mode, Some(RecordingMode::Raw));

    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        tags: BatchField::Clear,
        recording_use_global: BatchField::Set(true),
        recording_auto_start: BatchField::Set(false),
        ..Default::default()
    };
    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert!(connection.tags.is_empty());
    assert!(connection.recording.is_none());
    assert_eq!(report.applied_fields, 2);
    assert_eq!(report.skipped_fields, 1);
}

#[test]
fn password_fields_never_expose_plaintext() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        auth_mode: BatchField::Set("password".to_string()),
        password: BatchField::Set(SecretString::new("s3cret")),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);
    let auth = connection.auth.as_ref().expect("auth");
    assert_eq!(
        auth.password.as_ref().map(SecretString::expose_secret),
        Some("s3cret")
    );
    assert_eq!(
        auth.password_source,
        Some(ConnectionPasswordSource::Connection)
    );
    assert!(!auth.has_password);
    assert!(!format!("{edit:?}").contains("s3cret"));

    let edit = ConnectionBatchEdit {
        password: BatchField::Clear,
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    let auth = connection.auth.as_ref().expect("auth");
    assert!(auth.password.is_none());
    assert!(!auth.has_password);
    assert_eq!(
        auth.password_source,
        Some(ConnectionPasswordSource::Connection)
    );
}

#[test]
fn auth_references_and_agent_endpoint_apply() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        account_id: BatchField::Set("account-1".to_string()),
        key_id: BatchField::Set("key-1".to_string()),
        otp_id: BatchField::Set("otp-1".to_string()),
        auto_fill_otp: BatchField::Set(true),
        auth_agent_endpoint: BatchField::Set(SshAgentEndpoint::Environment {
            variable: "SSH_AUTH_SOCK".to_string(),
        }),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    let auth = connection.auth.as_ref().expect("auth");
    assert_eq!(auth.account_id.as_deref(), Some("account-1"));
    assert_eq!(auth.key_id.as_deref(), Some("key-1"));
    assert_eq!(auth.mode, ConnectionAuth::default().mode);
    assert_eq!(auth.otp_id.as_deref(), Some("otp-1"));
    assert!(auth.auto_fill_otp);
    match &connection.config {
        ConnectionType::Ssh {
            auth_agent_endpoint,
            ..
        } => assert_eq!(
            auth_agent_endpoint,
            &Some(SshAgentEndpoint::Environment {
                variable: "SSH_AUTH_SOCK".to_string(),
            })
        ),
        _ => panic!("expected ssh"),
    }
}

#[test]
fn agent_forwarding_policy_keeps_existing_fingerprints() {
    let mut connection = ssh_connection();
    if let ConnectionType::Ssh {
        agent_forwarding_config,
        ..
    } = &mut connection.config
    {
        *agent_forwarding_config = Some(SshAgentForwardingConfig {
            enabled: true,
            sources: Default::default(),
            policy: SshAgentForwardingPolicy::Allowlist {
                fingerprints: vec!["fp-1".to_string()],
            },
        });
    }
    let edit = ConnectionBatchEdit {
        agent_forwarding_external: BatchField::Set(true),
        agent_forwarding_stored_keys: BatchField::Set(false),
        agent_forwarding_policy: BatchField::Set(SshAgentForwardingPolicy::Allowlist {
            fingerprints: Vec::new(),
        }),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    match &connection.config {
        ConnectionType::Ssh {
            agent_forwarding_config,
            ..
        } => {
            let agent = agent_forwarding_config.as_ref().expect("agent config");
            assert!(agent.sources.external_agent);
            assert!(!agent.sources.stored_keys);
            assert_eq!(
                agent.policy,
                SshAgentForwardingPolicy::Allowlist {
                    fingerprints: vec!["fp-1".to_string()]
                }
            );
        }
        _ => panic!("expected ssh"),
    }
}

#[test]
fn allow_short_rsa_host_keys_applies_only_to_ssh() {
    let mut connection = ssh_connection();
    connection.allow_short_rsa_host_keys = false;
    let edit = ConnectionBatchEdit {
        allow_short_rsa_host_keys: BatchField::Set(true),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert!(connection.allow_short_rsa_host_keys);
    assert_eq!(report.applied_fields, 1);
    assert_eq!(report.skipped_fields, 0);

    let original = local_connection();
    let mut connection = original.clone();
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 0);
    assert_eq!(report.skipped_fields, 1);
    assert_eq!(connection, original);
}

#[test]
fn ssh_compatibility_mode_applies_only_to_ssh() {
    let mut connection = ssh_connection();
    connection.ssh_compatibility_mode = false;
    let edit = ConnectionBatchEdit {
        ssh_compatibility_mode: BatchField::Set(true),
        ..Default::default()
    };

    let report = apply_connection_batch_edit(&mut connection, &edit);

    assert!(connection.ssh_compatibility_mode);
    assert_eq!(report.applied_fields, 1);
    assert_eq!(report.skipped_fields, 0);

    let original = local_connection();
    let mut connection = original.clone();
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 0);
    assert_eq!(report.skipped_fields, 1);
    assert_eq!(connection, original);
}

#[test]
fn proxy_and_local_startup_fields_apply() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        proxy_id: BatchField::Set("proxy-1".to_string()),
        proxy_jump_id: BatchField::Set("jump-1".to_string()),
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    let network = connection.network.as_ref().expect("network");
    assert_eq!(network.proxy_id.as_deref(), Some("proxy-1"));
    assert_eq!(network.proxy_jump_id.as_deref(), Some("jump-1"));

    let mut connection = local_connection();
    let edit = ConnectionBatchEdit {
        shell_path: BatchField::Set("/bin/zsh".to_string()),
        shell_args: BatchField::Set("-l".to_string()),
        working_dir: BatchField::Set("   ".to_string()),
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    match &connection.config {
        ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } => {
            assert_eq!(shell_path, "/bin/zsh");
            assert_eq!(shell_args, "-l");
            assert!(working_dir.is_none());
        }
        _ => panic!("expected local terminal"),
    }
}

#[test]
fn telnet_compatibility_and_auto_login_apply() {
    let mut connection = telnet_connection();
    let edit = ConnectionBatchEdit {
        telnet_raw_tcp_cli: BatchField::Set(true),
        telnet_enter_mode: BatchField::Set("lf".to_string()),
        telnet_local_echo: BatchField::Set(true),
        telnet_local_line_edit: BatchField::Set(true),
        telnet_force_character_at_a_time: BatchField::Set(true),
        telnet_send_naws: BatchField::Set(false),
        telnet_send_sga: BatchField::Set(false),
        telnet_keep_alive_interval_secs: BatchField::Set(0),
        telnet_auto_login_enabled: BatchField::Set(true),
        telnet_auto_login_send_wake_enter: BatchField::Set(false),
        telnet_auto_login_timeout_ms: BatchField::Set(500),
        telnet_auto_login_max_retries: BatchField::Set(2),
        telnet_auto_login_username_prompt: BatchField::Set("login:".to_string()),
        telnet_auto_login_password_prompt: BatchField::Set("password:".to_string()),
        telnet_auto_login_success_prompt: BatchField::Set("welcome".to_string()),
        telnet_auto_login_failure_prompt: BatchField::Set("denied".to_string()),
        ..Default::default()
    };

    apply_connection_batch_edit(&mut connection, &edit);

    match &connection.config {
        ConnectionType::Telnet {
            raw_tcp_cli,
            enter_mode,
            local_echo,
            local_line_edit,
            force_character_at_a_time,
            send_naws,
            send_sga,
            keep_alive_interval_secs,
            auto_login,
            ..
        } => {
            assert!(*raw_tcp_cli);
            assert_eq!(enter_mode, "lf");
            assert!(*local_echo);
            assert!(*local_line_edit);
            assert!(*force_character_at_a_time);
            assert!(!*send_naws);
            assert!(!*send_sga);
            assert_eq!(*keep_alive_interval_secs, 0);
            assert!(auto_login.enabled);
            assert!(!auto_login.send_wake_enter);
            assert_eq!(auto_login.timeout_ms, 500);
            assert_eq!(auto_login.max_retries, 2);
            assert_eq!(auto_login.username_prompt_regex.as_deref(), Some("login:"));
            assert_eq!(
                auto_login.password_prompt_regex.as_deref(),
                Some("password:")
            );
            assert_eq!(auto_login.success_prompt_regex.as_deref(), Some("welcome"));
            assert_eq!(auto_login.failure_prompt_regex.as_deref(), Some("denied"));
        }
        _ => panic!("expected telnet"),
    }
}

#[test]
fn serial_and_vnc_fields_apply() {
    let mut connection = serial_connection();
    let edit = ConnectionBatchEdit {
        serial_port_name: BatchField::Set("COM9".to_string()),
        serial_baud_rate: BatchField::Set(9_600),
        serial_data_bits: BatchField::Set(7),
        serial_parity: BatchField::Set("even".to_string()),
        serial_stop_bits: BatchField::Set("2".to_string()),
        serial_flow_control: BatchField::Set(SerialFlowControl::Software),
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    match &connection.config {
        ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            flow_control,
            stop_bits,
            ..
        } => {
            assert_eq!(port_name, "COM9");
            assert_eq!(*baud_rate, 9_600);
            assert_eq!(*data_bits, 7);
            assert_eq!(parity, "even");
            assert_eq!(*flow_control, SerialFlowControl::Software);
            assert_eq!(stop_bits, "2");
        }
        _ => panic!("expected serial"),
    }

    let mut connection = vnc_connection();
    let edit = ConnectionBatchEdit {
        vnc_scale_mode: BatchField::Set("fit".to_string()),
        vnc_clipboard: BatchField::Set(false),
        vnc_shared: BatchField::Set(false),
        vnc_view_only: BatchField::Set(true),
        vnc_reconnect: BatchField::Set(true),
        vnc_reconnect_attempts: BatchField::Set(5),
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    match &connection.config {
        ConnectionType::Vnc {
            display,
            clipboard,
            reconnect,
            shared,
            view_only,
            ..
        } => {
            assert_eq!(display.scale_mode, "fit");
            assert!(!clipboard.enabled);
            assert!(!*shared);
            assert!(*view_only);
            assert!(reconnect.enabled);
            assert_eq!(reconnect.max_attempts, 5);
        }
        _ => panic!("expected vnc"),
    }
}

#[test]
fn rdp_display_and_reconnect_apply_but_mstsc_skips_them() {
    let mut connection = rdp_connection();
    let edit = ConnectionBatchEdit {
        rdp_domain: BatchField::Set("example".to_string()),
        rdp_backend: BatchField::Set("builtin".to_string()),
        rdp_display_mode: BatchField::Set("fixed".to_string()),
        rdp_display_width: BatchField::Set(1_600),
        rdp_display_height: BatchField::Set(900),
        rdp_display_wallpaper: BatchField::Set(false),
        rdp_display_themes: BatchField::Set(false),
        rdp_font_smoothing: BatchField::Set(true),
        rdp_desktop_composition: BatchField::Set(false),
        rdp_disable_full_window_drag: BatchField::Set(true),
        rdp_disable_menu_animations: BatchField::Set(true),
        rdp_disable_cursor_shadow: BatchField::Set(true),
        rdp_disable_cursor_blinking: BatchField::Set(true),
        rdp_auto_reconnect: BatchField::Set(false),
        rdp_reconnect_attempts: BatchField::Set(2),
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    match &connection.config {
        ConnectionType::Rdp {
            domain,
            backend,
            display,
            reconnect,
            ..
        } => {
            assert_eq!(domain, "example");
            assert_eq!(backend, "builtin");
            assert_eq!(display.mode, "fixed");
            assert_eq!(display.width, 1_600);
            assert_eq!(display.height, 900);
            assert!(!display.display_wallpaper);
            assert!(!display.display_themes);
            assert!(display.enable_font_smoothing);
            assert!(!display.enable_desktop_composition);
            assert!(display.disable_full_window_drag);
            assert!(display.disable_menu_animations);
            assert!(display.disable_cursor_shadow);
            assert!(display.disable_cursor_blinking);
            assert!(!reconnect.enabled);
            assert_eq!(reconnect.max_attempts, 2);
        }
        _ => panic!("expected rdp"),
    }

    let mut connection = rdp_connection();
    let edit = ConnectionBatchEdit {
        rdp_backend: BatchField::Set("mstsc".to_string()),
        rdp_display_mode: BatchField::Set("fixed".to_string()),
        rdp_use_nla: BatchField::Set(false),
        rdp_auto_reconnect: BatchField::Set(false),
        ..Default::default()
    };
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 1);
    assert_eq!(report.skipped_fields, 3);
    match &connection.config {
        ConnectionType::Rdp {
            backend,
            security,
            display,
            reconnect,
            ..
        } => {
            assert_eq!(backend, "mstsc");
            assert!(security.use_nla);
            assert_ne!(display.mode, "fixed");
            assert!(reconnect.enabled);
        }
        _ => panic!("expected rdp"),
    }
}

#[test]
fn validation_rejects_invalid_new_values() {
    let edit = ConnectionBatchEdit {
        rdp_display_width: BatchField::Set(0),
        ..Default::default()
    };
    assert!(validate_connection_batch_edit(&edit).is_err());

    let edit = ConnectionBatchEdit {
        serial_data_bits: BatchField::Set(9),
        ..Default::default()
    };
    assert!(validate_connection_batch_edit(&edit).is_err());

    let edit = ConnectionBatchEdit {
        telnet_auto_login_username_prompt: BatchField::Set("[".to_string()),
        ..Default::default()
    };
    assert!(validate_connection_batch_edit(&edit).is_err());
}

#[test]
fn validation_rejects_empty_edit_and_invalid_values() {
    assert!(validate_connection_batch_edit(&ConnectionBatchEdit::default()).is_err());

    let mut edit = ConnectionBatchEdit {
        port: BatchField::Set(0),
        ..Default::default()
    };
    assert!(validate_connection_batch_edit(&edit).is_err());

    edit.port = BatchField::Set(22);
    assert!(validate_connection_batch_edit(&edit).is_ok());

    let edit = ConnectionBatchEdit {
        sftp_shell_detection_timeout_ms: BatchField::Set(50),
        ..Default::default()
    };
    assert!(validate_connection_batch_edit(&edit).is_err());

    let edit = ConnectionBatchEdit {
        post_login_command: BatchField::Set("   ".to_string()),
        ..Default::default()
    };
    assert!(validate_connection_batch_edit(&edit).is_err());
}

#[test]
fn explicit_username_wins_over_account_and_clearing_account_removes_legacy_reference() {
    let mut connection = ssh_connection();
    connection.auth = Some(ConnectionAuth {
        password_id: Some("legacy".into()),
        password: Some(SecretString::new(" retained ")),
        ..Default::default()
    });
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            account_id: BatchField::Set("account".into()),
            username: BatchField::Set("connection-user".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        connection.auth.as_ref().unwrap().username_source,
        Some(ConnectionUsernameSource::Connection)
    );
    connection.auth.as_mut().unwrap().password_id = Some("legacy".into());
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            account_id: BatchField::Clear,
            ..Default::default()
        },
    );
    let auth = connection.auth.unwrap();
    assert!(auth.account_id.is_none() && auth.password_id.is_none());
    assert_eq!(
        auth.password_source,
        Some(ConnectionPasswordSource::Connection)
    );
    assert_eq!(auth.password.unwrap().expose_secret(), " retained ");
}

#[test]
fn boolean_clear_restores_true_defaults_and_never_converts_protocol() {
    let mut connection = vnc_connection();
    let kind = super::ConnectionBatchKind::from_config(&connection.config);
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            vnc_shared: BatchField::Set(false),
            vnc_clipboard: BatchField::Set(false),
            vnc_reconnect: BatchField::Set(false),
            ..Default::default()
        },
    );
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            vnc_shared: BatchField::Clear,
            vnc_clipboard: BatchField::Clear,
            vnc_reconnect: BatchField::Clear,
            ..Default::default()
        },
    );
    assert_eq!(
        super::ConnectionBatchKind::from_config(&connection.config),
        kind
    );
    match connection.config {
        ConnectionType::Vnc {
            shared,
            clipboard,
            reconnect,
            ..
        } => assert!(shared && clipboard.enabled && reconnect.enabled),
        _ => unreachable!(),
    }
}

#[test]
fn clear_restores_telnet_and_rdp_true_defaults() {
    let mut telnet = telnet_connection();
    if let ConnectionType::Telnet {
        send_naws,
        send_sga,
        auto_login,
        ..
    } = &mut telnet.config
    {
        *send_naws = false;
        *send_sga = false;
        auto_login.enabled = false;
        auto_login.send_wake_enter = false;
    }
    apply_connection_batch_edit(
        &mut telnet,
        &ConnectionBatchEdit {
            telnet_send_naws: BatchField::Clear,
            telnet_send_sga: BatchField::Clear,
            telnet_auto_login_enabled: BatchField::Clear,
            telnet_auto_login_send_wake_enter: BatchField::Clear,
            ..Default::default()
        },
    );
    if let ConnectionType::Telnet {
        send_naws,
        send_sga,
        auto_login,
        ..
    } = telnet.config
    {
        assert!(send_naws && send_sga && auto_login.enabled && auto_login.send_wake_enter);
    }
    let mut rdp = rdp_connection();
    apply_connection_batch_edit(
        &mut rdp,
        &ConnectionBatchEdit {
            rdp_display_wallpaper: BatchField::Clear,
            rdp_display_themes: BatchField::Clear,
            rdp_font_smoothing: BatchField::Clear,
            rdp_disable_full_window_drag: BatchField::Clear,
            rdp_disable_menu_animations: BatchField::Clear,
            rdp_auto_reconnect: BatchField::Clear,
            ..Default::default()
        },
    );
    if let ConnectionType::Rdp {
        display, reconnect, ..
    } = rdp.config
    {
        assert!(
            display.display_wallpaper
                && display.display_themes
                && display.enable_font_smoothing
                && display.disable_full_window_drag
                && display.disable_menu_animations
                && reconnect.enabled
        );
    }
}

#[test]
fn setting_account_defaults_to_account_username_without_overwriting_auth_mode() {
    let mut connection = ssh_connection();
    connection.auth = Some(ConnectionAuth {
        mode: "agent".into(),
        username_source: Some(ConnectionUsernameSource::Connection),
        password_source: Some(ConnectionPasswordSource::Connection),
        ..Default::default()
    });
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            account_id: BatchField::Set("account".into()),
            key_id: BatchField::Set("key".into()),
            ..Default::default()
        },
    );
    let auth = connection.auth.unwrap();
    assert_eq!(
        auth.username_source,
        Some(ConnectionUsernameSource::Account)
    );
    assert_eq!(
        auth.password_source,
        Some(ConnectionPasswordSource::Account)
    );
    assert_eq!(auth.mode, "agent");
}

#[test]
fn rdp_backend_and_display_dependencies_match_preview_and_preserve_skipped_fields() {
    for original_backend in ["builtin", "windows", "mstsc", "future-backend"] {
        for original_mode in ["fit-window", "fixed", "native"] {
            for backend_edit in [
                BatchField::Unchanged,
                BatchField::Set("builtin".into()),
                BatchField::Set("windows".into()),
                BatchField::Set("mstsc".into()),
                BatchField::Clear,
            ] {
                for mode_edit in [
                    BatchField::Unchanged,
                    BatchField::Set("fixed".into()),
                    BatchField::Set("fit-window".into()),
                    BatchField::Clear,
                ] {
                    let mut connection = rdp_connection();
                    if let ConnectionType::Rdp {
                        backend, display, ..
                    } = &mut connection.config
                    {
                        *backend = original_backend.into();
                        display.mode = original_mode.into();
                    }
                    let edit = ConnectionBatchEdit {
                        rdp_backend: backend_edit.clone(),
                        rdp_display_mode: mode_edit.clone(),
                        rdp_use_nla: BatchField::Set(false),
                        rdp_display_width: BatchField::Set(1600),
                        rdp_display_height: BatchField::Set(900),
                        ..Default::default()
                    };
                    let mstsc = match &backend_edit {
                        BatchField::Unchanged => original_backend == "mstsc",
                        BatchField::Set(value) => value == "mstsc",
                        BatchField::Clear => false,
                    };
                    let fixed = match &mode_edit {
                        BatchField::Unchanged => matches!(original_mode, "fixed" | "native"),
                        BatchField::Set(value) => value == "fixed",
                        BatchField::Clear => false,
                    };
                    assert_eq!(
                        rdp_batch_field_skip_reason(
                            &connection.config,
                            &edit,
                            RdpBatchFieldGroup::SecurityAndDisplay
                        ),
                        mstsc.then_some(ConnectionBatchSkipReason::MstscBackend)
                    );
                    assert_eq!(
                        rdp_batch_field_skip_reason(
                            &connection.config,
                            &edit,
                            RdpBatchFieldGroup::Dimensions
                        ),
                        if mstsc {
                            Some(ConnectionBatchSkipReason::MstscBackend)
                        } else if !fixed {
                            Some(ConnectionBatchSkipReason::NonFixedRdpDisplay)
                        } else {
                            None
                        }
                    );
                    let original = connection.clone();
                    let report = apply_connection_batch_edit(&mut connection, &edit);
                    let skipped = if mstsc {
                        3 + usize::from(!mode_edit.is_unchanged())
                    } else if !fixed {
                        2
                    } else {
                        0
                    };
                    assert_eq!(report.skipped_fields, skipped);
                    assert_eq!(
                        report.applied_fields + report.skipped_fields,
                        3 + usize::from(!backend_edit.is_unchanged())
                            + usize::from(!mode_edit.is_unchanged())
                    );
                    assert_eq!(
                        report
                            .skipped_reasons
                            .count(ConnectionBatchSkipReason::MstscBackend),
                        if mstsc { skipped } else { 0 }
                    );
                    assert_eq!(
                        report
                            .skipped_reasons
                            .count(ConnectionBatchSkipReason::NonFixedRdpDisplay),
                        if !mstsc && !fixed { 2 } else { 0 }
                    );
                    let ConnectionType::Rdp {
                        security, display, ..
                    } = &connection.config
                    else {
                        unreachable!()
                    };
                    let ConnectionType::Rdp {
                        security: original_security,
                        display: original_display,
                        ..
                    } = &original.config
                    else {
                        unreachable!()
                    };
                    assert_eq!(
                        security.use_nla,
                        if mstsc {
                            original_security.use_nla
                        } else {
                            false
                        }
                    );
                    assert_eq!(
                        (display.width, display.height),
                        if !mstsc && fixed {
                            (1600, 900)
                        } else {
                            (original_display.width, original_display.height)
                        }
                    );
                    if mode_edit.is_unchanged() || mstsc {
                        assert_eq!(display.mode, original_mode);
                    }
                }
            }
        }
    }
}

#[test]
fn skip_reasons_distinguish_protocol_from_global_recording_dependencies() {
    let mut connection = ssh_connection();
    let edit = ConnectionBatchEdit {
        rdp_use_nla: BatchField::Clear,
        rdp_display_width: BatchField::Set(1600),
        recording_use_global: BatchField::Set(true),
        recording_auto_start: BatchField::Set(true),
        recording_mode: BatchField::Set(RecordingMode::default()),
        ..Default::default()
    };
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 1);
    assert_eq!(report.skipped_fields, 4);
    assert_eq!(
        report
            .skipped_reasons
            .count(ConnectionBatchSkipReason::UnsupportedProtocol),
        2
    );
    assert_eq!(
        report
            .skipped_reasons
            .count(ConnectionBatchSkipReason::GlobalRecording),
        2
    );
    assert!(connection.recording.is_none());
}

#[test]
fn rdp_basis_counts_skipped_members_after_values_become_mixed() {
    use super::ConnectionBatchKind;
    use super::basis::{BatchValueBasis, rdp_field_basis};
    let mut first = rdp_connection();
    first.id = "first".into();
    let mut second = first.clone();
    second.id = "second".into();
    let mut external = first.clone();
    external.id = "external".into();
    if let ConnectionType::Rdp { backend, .. } = &mut external.config {
        *backend = "mstsc".into();
    }
    let connections = [first, second, external];
    let targets =
        ["first", "second", "external", "removed"].map(|id| (id.into(), ConnectionBatchKind::Rdp));
    let (basis, counts) = rdp_field_basis(
        &connections,
        &targets,
        RdpBatchFieldGroup::SecurityAndDisplay,
        &ConnectionBatchEdit::default(),
        |entry| entry.id.clone(),
    );
    assert_eq!(basis, BatchValueBasis::Mixed);
    assert_eq!(counts.total, 4);
    assert_eq!(counts.applicable, 2);
    assert_eq!(
        counts
            .skipped_reasons
            .count(ConnectionBatchSkipReason::UnsupportedProtocol),
        1
    );
    let edit = ConnectionBatchEdit {
        rdp_backend: BatchField::Set("builtin".into()),
        ..Default::default()
    };
    let (_, counts) = rdp_field_basis(
        &connections,
        &targets,
        RdpBatchFieldGroup::SecurityAndDisplay,
        &edit,
        |entry| entry.id.clone(),
    );
    assert_eq!(counts.applicable, 3);
}

#[test]
fn batch_kind_counts_keep_supported_order_and_ignore_missing_or_android_targets() {
    use super::ConnectionBatchKind;
    use super::basis::connection_kind_counts;
    let mut ssh = ssh_connection();
    ssh.id = "ssh".into();
    let mut rdp = rdp_connection();
    rdp.id = "rdp".into();
    let android: SavedConnection = serde_json::from_value(serde_json::json!({
        "id": "android", "name": "fixture", "type": "adb",
        "device": { "kind": "usb", "serial": "fixture" }
    }))
    .unwrap();
    let ids = ["rdp", "android", "missing", "ssh", "ssh"].map(str::to_string);
    assert_eq!(
        connection_kind_counts(&[rdp, android, ssh], &ids),
        vec![
            (ConnectionBatchKind::Ssh, 2),
            (ConnectionBatchKind::Rdp, 1),
            (ConnectionBatchKind::Adb, 1),
        ]
    );
}

#[test]
fn serial_batch_values_disable_only_the_corresponding_connect_prompt() {
    for (edit, expected_port, expected_baud) in [
        (
            ConnectionBatchEdit {
                serial_port_name: BatchField::Set("COM9".into()),
                ..Default::default()
            },
            false,
            true,
        ),
        (
            ConnectionBatchEdit {
                serial_baud_rate: BatchField::Set(57_600),
                ..Default::default()
            },
            true,
            false,
        ),
        (
            ConnectionBatchEdit {
                serial_baud_rate: BatchField::Clear,
                ..Default::default()
            },
            true,
            false,
        ),
        (
            ConnectionBatchEdit {
                serial_port_name: BatchField::Set("COM9".into()),
                serial_baud_rate: BatchField::Set(57_600),
                ..Default::default()
            },
            false,
            false,
        ),
        (
            ConnectionBatchEdit {
                serial_data_bits: BatchField::Set(7),
                ..Default::default()
            },
            true,
            true,
        ),
    ] {
        let mut connection = serial_connection();
        let ConnectionType::Serial { connect_prompt, .. } = &mut connection.config else {
            panic!("expected serial");
        };
        connect_prompt.port = true;
        connect_prompt.baud_rate = true;
        apply_connection_batch_edit(&mut connection, &edit);
        let ConnectionType::Serial { connect_prompt, .. } = &connection.config else {
            panic!("expected serial");
        };
        assert_eq!(connect_prompt.port, expected_port);
        assert_eq!(connect_prompt.baud_rate, expected_baud);
        let restored: SavedConnection =
            serde_json::from_value(serde_json::to_value(&connection).unwrap()).unwrap();
        let ConnectionType::Serial {
            connect_prompt: restored_prompt,
            ..
        } = restored.config
        else {
            panic!("expected serial");
        };
        assert_eq!(restored_prompt, *connect_prompt);
    }
}

fn adb_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::Adb {
            device: crate::models::android::AdbDeviceSelector::Usb {
                serial: "device-1".to_string(),
            },
            directory: crate::models::android::default_android_directory(),
            screen: crate::models::android::AndroidScreenSettings::default(),
        },
        ..ssh_connection()
    }
}

fn ftp_connection() -> SavedConnection {
    SavedConnection {
        config: ConnectionType::Ftp {
            host: "10.0.0.5".to_string(),
            port: 21,
            username: "anonymous".to_string(),
            initial_directory: String::new(),
            local_initial_directory: String::new(),
            passive_mode: "auto".to_string(),
            ftp_filename_encoding_mode: "auto".to_string(),
            ftp_filename_encoding: String::new(),
            ftp_ascii_mode: false,
            ftp_security_mode: "none".to_string(),
            ftp_tls_version: "auto".to_string(),
            connect_timeout_ms: 15_000,
        },
        ..ssh_connection()
    }
}

#[test]
fn adb_batch_fields_apply_and_clear_to_screen_defaults() {
    use crate::models::android::{AndroidScreenSettings, CaptureOrientation, ScreenQualityPreset};

    let mut connection = adb_connection();
    let edit = ConnectionBatchEdit {
        adb_directory: BatchField::Set("/sdcard/Download".to_string()),
        adb_screen_quality: BatchField::Set(ScreenQualityPreset::Sharp),
        adb_screen_power_on: BatchField::Set(true),
        adb_screen_orientation: BatchField::Set(CaptureOrientation::Degrees90),
        ..Default::default()
    };
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 4);
    let ConnectionType::Adb {
        directory, screen, ..
    } = &connection.config
    else {
        panic!("expected adb");
    };
    assert_eq!(directory, "/sdcard/Download");
    assert_eq!(
        *screen,
        AndroidScreenSettings {
            max_size: 1920,
            max_fps: 30,
            bit_rate: 12_000_000,
            power_on: true,
            capture_orientation: CaptureOrientation::Degrees90,
        }
    );

    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            adb_directory: BatchField::Clear,
            adb_screen_quality: BatchField::Clear,
            adb_screen_power_on: BatchField::Clear,
            adb_screen_orientation: BatchField::Clear,
            ..Default::default()
        },
    );
    let ConnectionType::Adb {
        directory, screen, ..
    } = &connection.config
    else {
        panic!("expected adb");
    };
    assert_eq!(
        directory,
        &crate::models::android::default_android_directory()
    );
    assert_eq!(*screen, AndroidScreenSettings::default());
}

#[test]
fn adb_screen_dimension_edits_stay_custom() {
    use crate::models::android::{AndroidScreenSettings, ScreenQualityPreset};

    let mut connection = adb_connection();
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            adb_screen_quality: BatchField::Set(ScreenQualityPreset::Smooth),
            adb_screen_max_size: BatchField::Set(800),
            ..Default::default()
        },
    );
    let ConnectionType::Adb { screen, .. } = &connection.config else {
        panic!("expected adb");
    };
    assert_eq!(screen.max_size, 800);
    assert_eq!(screen.max_fps, 60);
    assert_eq!(screen.bit_rate, 8_000_000);
    assert_ne!(*screen, {
        let mut preset = *screen;
        ScreenQualityPreset::Balanced.apply(&mut preset);
        preset
    });
    assert_eq!(
        AndroidScreenSettings::default().capture_orientation,
        screen.capture_orientation
    );
}

#[test]
fn ftp_batch_fields_apply_and_clear_to_defaults() {
    let mut connection = ftp_connection();
    let edit = ConnectionBatchEdit {
        ftp_initial_directory: BatchField::Set("/incoming".to_string()),
        ftp_local_initial_directory: BatchField::Set("C:/downloads".to_string()),
        ftp_passive_mode: BatchField::Set("epsv".to_string()),
        ftp_filename_encoding_mode: BatchField::Set("custom".to_string()),
        ftp_filename_encoding: BatchField::Set("GBK".to_string()),
        ftp_ascii_mode: BatchField::Set(true),
        ftp_security_mode: BatchField::Set("explicit".to_string()),
        ftp_tls_version: BatchField::Set("tls1.3".to_string()),
        ftp_connect_timeout_ms: BatchField::Set(30_000),
        ..Default::default()
    };
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 9);
    let ConnectionType::Ftp {
        initial_directory,
        local_initial_directory,
        passive_mode,
        ftp_filename_encoding_mode,
        ftp_filename_encoding,
        ftp_ascii_mode,
        ftp_security_mode,
        ftp_tls_version,
        connect_timeout_ms,
        ..
    } = &connection.config
    else {
        panic!("expected ftp");
    };
    assert_eq!(initial_directory, "/incoming");
    assert_eq!(local_initial_directory, "C:/downloads");
    assert_eq!(passive_mode, "epsv");
    assert_eq!(ftp_filename_encoding_mode, "custom");
    assert_eq!(ftp_filename_encoding, "GBK");
    assert!(*ftp_ascii_mode);
    assert_eq!(ftp_security_mode, "explicit");
    assert_eq!(ftp_tls_version, "tls1.3");
    assert_eq!(*connect_timeout_ms, 30_000);

    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            ftp_initial_directory: BatchField::Clear,
            ftp_local_initial_directory: BatchField::Clear,
            ftp_passive_mode: BatchField::Clear,
            ftp_filename_encoding_mode: BatchField::Clear,
            ftp_filename_encoding: BatchField::Clear,
            ftp_ascii_mode: BatchField::Clear,
            ftp_security_mode: BatchField::Clear,
            ftp_tls_version: BatchField::Clear,
            ftp_connect_timeout_ms: BatchField::Clear,
            ..Default::default()
        },
    );
    let ConnectionType::Ftp {
        initial_directory,
        local_initial_directory,
        passive_mode,
        ftp_filename_encoding_mode,
        ftp_filename_encoding,
        ftp_ascii_mode,
        ftp_security_mode,
        ftp_tls_version,
        connect_timeout_ms,
        ..
    } = &connection.config
    else {
        panic!("expected ftp");
    };
    assert!(initial_directory.is_empty());
    assert!(local_initial_directory.is_empty());
    assert_eq!(passive_mode, "auto");
    assert_eq!(ftp_filename_encoding_mode, "auto");
    assert!(ftp_filename_encoding.is_empty());
    assert!(!*ftp_ascii_mode);
    assert_eq!(ftp_security_mode, "none");
    assert_eq!(ftp_tls_version, "auto");
    assert_eq!(*connect_timeout_ms, 15_000);
}

#[test]
fn ftp_shares_endpoint_and_credentials_without_proxy() {
    let mut connection = ftp_connection();
    let report = apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            port: BatchField::Set(2121),
            username: BatchField::Set("file-user".to_string()),
            proxy_id: BatchField::Set("proxy-1".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(report.applied_fields, 2);
    assert_eq!(report.skipped_fields, 1);
    match &connection.config {
        ConnectionType::Ftp { port, username, .. } => {
            assert_eq!(*port, 2121);
            assert_eq!(username, "file-user");
        }
        _ => panic!("expected ftp"),
    }
    assert!(
        connection.network.is_none() || connection.network.as_ref().unwrap().proxy_id.is_none()
    );
}

#[test]
fn anonymous_ftp_clears_the_stored_password() {
    let mut connection = ftp_connection();
    connection.auth = Some(ConnectionAuth {
        password: Some(SecretString::new("stored")),
        has_password: true,
        password_id: Some("saved".to_string()),
        ..Default::default()
    });
    apply_connection_batch_edit(
        &mut connection,
        &ConnectionBatchEdit {
            auth_mode: BatchField::Set("none".to_string()),
            ..Default::default()
        },
    );
    let auth = connection.auth.as_ref().expect("auth");
    assert_eq!(auth.mode, "none");
    assert!(auth.password.is_none());
    assert!(auth.password_id.is_none());
    assert!(!auth.has_password);
}

#[test]
fn adb_and_ftp_fields_are_skipped_for_other_protocols() {
    let original = ssh_connection();
    let mut connection = original.clone();
    let edit = ConnectionBatchEdit {
        adb_directory: BatchField::Set("/sdcard/".to_string()),
        adb_screen_quality: BatchField::Clear,
        ftp_initial_directory: BatchField::Set("/incoming".to_string()),
        ftp_connect_timeout_ms: BatchField::Set(30_000),
        ..Default::default()
    };
    let report = apply_connection_batch_edit(&mut connection, &edit);
    assert_eq!(report.applied_fields, 0);
    assert_eq!(report.skipped_fields, 4);
    assert_eq!(connection, original);
}

#[test]
fn validation_rejects_invalid_adb_screen_and_ftp_timeout() {
    for edit in [
        ConnectionBatchEdit {
            adb_screen_max_size: BatchField::Set(64),
            ..Default::default()
        },
        ConnectionBatchEdit {
            adb_screen_max_fps: BatchField::Set(0),
            ..Default::default()
        },
        ConnectionBatchEdit {
            adb_screen_bit_rate: BatchField::Set(1),
            ..Default::default()
        },
        ConnectionBatchEdit {
            ftp_connect_timeout_ms: BatchField::Set(999),
            ..Default::default()
        },
        ConnectionBatchEdit {
            ftp_security_mode: BatchField::Set("  ".to_string()),
            ..Default::default()
        },
    ] {
        assert!(validate_connection_batch_edit(&edit).is_err());
    }
    assert!(
        validate_connection_batch_edit(&ConnectionBatchEdit {
            adb_screen_quality: BatchField::Set(crate::models::android::ScreenQualityPreset::Sharp),
            ftp_connect_timeout_ms: BatchField::Set(120_000),
            ..Default::default()
        })
        .is_ok()
    );
}

#[test]
fn clearing_the_serial_device_enables_prompt_without_changing_baud_prompt() {
    for device in [BatchField::Clear, BatchField::Set(String::new())] {
        let mut connection = serial_connection();
        apply_connection_batch_edit(
            &mut connection,
            &ConnectionBatchEdit {
                serial_port_name: device,
                ..Default::default()
            },
        );
        let ConnectionType::Serial {
            port_name,
            connect_prompt,
            ..
        } = connection.config
        else {
            panic!("expected serial");
        };
        assert!(port_name.is_empty());
        assert!(connect_prompt.port);
        assert!(!connect_prompt.baud_rate);
    }
}
