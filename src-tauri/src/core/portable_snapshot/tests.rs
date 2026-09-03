#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::{
        PORTABLE_SNAPSHOT_SCHEMA_VERSION, PortableAppSettings, PortableSnapshot,
        PortableSnapshotKind, PortableUiSettings, SNAPSHOT_ZIP_PAYLOAD_NAME,
        calculate_payload_hash, calculate_v3_raw_payload_hash, encode_portable_snapshot,
        encode_portable_snapshot_redb, encode_v3_raw_snapshot_redb_for_test,
        normalize_backup_sessions_for_platform, preserve_device_local_sessions,
        strip_device_local_sessions, AgentEndpointTargetPlatform,
    };
    use crate::config::{self, ActivityBarLayout, AppSettings};
    use crate::error::AppError;
    use std::collections::BTreeMap;
    use std::io::Write;

    #[test]
    fn backup_sessions_drop_incompatible_agent_endpoints_for_unix() {
        let mut sessions: config::SessionsConfig = serde_json::from_value(serde_json::json!({
            "connections": [{
                "id": "agent-1",
                "name": "Agent",
                "type": "ssh",
                "host": "example.com",
                "auth": { "mode": "agent" },
                "auth_agent_endpoint": { "type": "pageant" },
                "agent_forwarding_config": {
                    "enabled": true,
                    "sources": {
                        "external_agent": true,
                        "external_agent_endpoints": [
                            { "type": "pageant" },
                            { "type": "unix_socket", "path": "/tmp/agent.sock" }
                        ],
                        "stored_keys": false
                    },
                    "policy": { "mode": "all" }
                }
            }]
        }))
        .expect("sessions");

        assert!(normalize_backup_sessions_for_platform(
            &mut sessions,
            AgentEndpointTargetPlatform::Unix
        )
        .expect("normalize backup sessions"));

        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } = &sessions.connections[0].config
        else {
            panic!("expected SSH connection");
        };
        assert_eq!(auth_agent_endpoint, &Some(config::SshAgentEndpoint::Auto));
        let forwarding = agent_forwarding_config.as_ref().expect("forwarding config");
        assert_eq!(forwarding.sources.external_agent_endpoints.len(), 1);
        assert!(matches!(
            forwarding.sources.external_agent_endpoints[0],
            config::SshAgentEndpoint::UnixSocket { .. }
        ));
        assert!(forwarding.enabled);
    }

    #[test]
    fn backup_sessions_drop_incompatible_agent_endpoints_for_windows() {
        let mut sessions: config::SessionsConfig = serde_json::from_value(serde_json::json!({
            "connections": [{
                "id": "agent-1",
                "name": "Agent",
                "type": "ssh",
                "host": "example.com",
                "auth": { "mode": "password" },
                "auth_agent_endpoint": { "type": "unix_socket", "path": "/tmp/agent.sock" },
                "agent_forwarding_config": {
                    "enabled": true,
                    "sources": {
                        "external_agent": true,
                        "external_agent_endpoints": [
                            { "type": "unix_socket", "path": "/tmp/agent.sock" },
                            { "type": "pageant" }
                        ],
                        "stored_keys": false
                    },
                    "policy": { "mode": "all" }
                }
            }]
        }))
        .expect("sessions");

        assert!(normalize_backup_sessions_for_platform(
            &mut sessions,
            AgentEndpointTargetPlatform::Windows
        )
        .expect("normalize backup sessions"));

        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } = &sessions.connections[0].config
        else {
            panic!("expected SSH connection");
        };
        assert!(auth_agent_endpoint.is_none());
        let forwarding = agent_forwarding_config.as_ref().expect("forwarding config");
        assert_eq!(forwarding.sources.external_agent_endpoints, vec![config::SshAgentEndpoint::Pageant]);
        assert!(forwarding.enabled);
    }

    #[test]
    fn backup_rejects_malformed_endpoint_before_cross_platform_filtering() {
        let mut sessions: config::SessionsConfig = serde_json::from_value(serde_json::json!({
            "connections": [{
                "id": "agent-1",
                "name": "Agent",
                "type": "ssh",
                "host": "example.com",
                "agent_forwarding_config": {
                    "enabled": true,
                    "sources": {
                        "external_agent": true,
                        "external_agent_endpoints": [
                            { "type": "unix_socket", "path": "bad\u{0000}.sock" },
                            { "type": "pageant" }
                        ],
                        "stored_keys": false
                    },
                    "policy": { "mode": "all" }
                }
            }]
        }))
        .expect("sessions");

        assert!(normalize_backup_sessions_for_platform(
            &mut sessions,
            AgentEndpointTargetPlatform::Windows
        )
        .is_err());
    }

    #[test]
    fn portable_settings_strip_master_password_and_preserve_device_ui_state_on_apply() {
        let mut current = AppSettings::default();
        current.security.master_password = Some("encrypted-master".to_string());
        current.ui.panel_open_mode = "docked".to_string();
        current.ui.left_width = 444.0;
        current.ui.active_left_panel = Some("fileExplorer".to_string());
        current.ui.quick_cmd_selected_category = "local-category".to_string();
        current.ai.active_profile_id = "local-profile".to_string();
        current.ai.provider_profiles[0].api_key = Some("local-key".to_string());

        let mut updated =
            PortableAppSettings::from_app_settings(&current, &PortableSnapshotKind::Backup);
        updated.general.startup_restore = false;
        updated.ui.language = Some("zh-CN".to_string());
        updated.ui.panel_open_mode = "floating".to_string();
        updated.ui.saved_connections_sort_mode = "name-asc".to_string();
        updated.ai.active_profile_id = "synced-profile".to_string();
        updated.ai.provider_profiles[0].api_key = Some("synced-key".to_string());

        let merged = updated.apply_to(current.clone(), &PortableSnapshotKind::Backup);
        assert_eq!(
            merged.security.master_password,
            current.security.master_password
        );
        assert_eq!(merged.ui.left_width, current.ui.left_width);
        assert_eq!(merged.ui.active_left_panel, current.ui.active_left_panel);
        assert_eq!(
            merged.ui.quick_cmd_selected_category,
            current.ui.quick_cmd_selected_category
        );
        assert_eq!(merged.ui.language.as_deref(), Some("zh-CN"));
        assert_eq!(merged.ui.panel_open_mode, "floating");
        assert_eq!(merged.ui.saved_connections_sort_mode, "name-asc");
        assert_eq!(merged.ai.active_profile_id, "synced-profile");
        assert_eq!(
            merged.ai.provider_profiles[0].api_key.as_deref(),
            Some("synced-key")
        );
    }

    #[test]
    fn portable_settings_migrate_legacy_screen_lock_modes() {
        let portable = sample_portable_settings();
        let mut raw = serde_json::to_value(portable).expect("portable settings should serialize");
        let security = raw
            .get_mut("security")
            .and_then(serde_json::Value::as_object_mut)
            .expect("security settings should be an object");
        security.remove("enable_startup_lock");
        security.remove("enable_idle_lock");
        security.insert("enable_screen_lock".to_string(), serde_json::json!(true));

        let decoded: PortableAppSettings =
            serde_json::from_value(raw).expect("legacy portable settings should deserialize");
        let merged = decoded.apply_to(AppSettings::default(), &PortableSnapshotKind::Backup);

        assert!(merged.security.enable_startup_lock);
        assert!(merged.security.enable_idle_lock);
    }

    #[test]
    fn sync_portable_settings_strip_device_local_paths() {
        let mut current = AppSettings::default();
        current.appearance.background_image_path = Some("D:\\background.png".to_string());
        current.appearance.background_image_fit = "tile".to_string();
        current.appearance.background_image_opacity = 0.8;
        current.transfer.download_path = "D:\\downloads".to_string();
        current.transfer.default_editor = "D:\\tools\\editor.exe".to_string();
        current.transfer.recording_path = "D:\\recordings".to_string();

        let portable =
            PortableAppSettings::from_app_settings(&current, &PortableSnapshotKind::Sync);

        assert!(portable.appearance.background_image_path.is_none());
        assert_eq!(
            portable.appearance.background_image_fit,
            config::AppearanceSettings::default().background_image_fit
        );
        assert_eq!(
            portable.appearance.background_image_opacity,
            config::AppearanceSettings::default().background_image_opacity
        );
        assert!(portable.transfer.download_path.is_empty());
        assert!(portable.transfer.default_editor.is_empty());
        assert!(portable.transfer.recording_path.is_empty());
    }

    #[test]
    fn backup_portable_settings_preserve_device_local_paths() {
        let mut current = AppSettings::default();
        current.appearance.background_image_path = Some("D:\\background.png".to_string());
        current.appearance.background_image_fit = "tile".to_string();
        current.appearance.background_image_opacity = 0.8;
        current.transfer.download_path = "D:\\downloads".to_string();
        current.transfer.default_editor = "D:\\tools\\editor.exe".to_string();
        current.transfer.recording_path = "D:\\recordings".to_string();

        let portable =
            PortableAppSettings::from_app_settings(&current, &PortableSnapshotKind::Backup);

        assert_eq!(
            portable.appearance.background_image_path.as_deref(),
            Some("D:\\background.png")
        );
        assert_eq!(portable.appearance.background_image_fit, "tile");
        assert_eq!(portable.appearance.background_image_opacity, 0.8);
        assert_eq!(portable.transfer.download_path, "D:\\downloads");
        assert_eq!(portable.transfer.default_editor, "D:\\tools\\editor.exe");
        assert_eq!(portable.transfer.recording_path, "D:\\recordings");
    }

    #[test]
    fn applying_sync_portable_settings_preserves_current_device_paths() {
        let mut current = AppSettings::default();
        current.appearance.background_image_path = Some("/Users/me/local.png".to_string());
        current.appearance.background_image_fit = "contain".to_string();
        current.appearance.background_image_opacity = 0.4;
        current.transfer.download_path = "/Users/me/Downloads".to_string();
        current.transfer.default_editor = "/Applications/Editor.app".to_string();
        current.transfer.recording_path = "/Users/me/Recordings".to_string();

        let mut incoming = AppSettings::default();
        incoming.appearance.background_image_path = Some("D:\\background.png".to_string());
        incoming.appearance.background_image_fit = "tile".to_string();
        incoming.appearance.background_image_opacity = 0.9;
        incoming.transfer.download_path = "D:\\downloads".to_string();
        incoming.transfer.default_editor = "D:\\tools\\editor.exe".to_string();
        incoming.transfer.recording_path = "D:\\recordings".to_string();

        let portable =
            PortableAppSettings::from_app_settings(&incoming, &PortableSnapshotKind::Backup);
        let merged = portable.apply_to(current, &PortableSnapshotKind::Sync);

        assert_eq!(
            merged.appearance.background_image_path.as_deref(),
            Some("/Users/me/local.png")
        );
        assert_eq!(merged.appearance.background_image_fit, "contain");
        assert_eq!(merged.appearance.background_image_opacity, 0.4);
        assert_eq!(merged.transfer.download_path, "/Users/me/Downloads");
        assert_eq!(merged.transfer.default_editor, "/Applications/Editor.app");
        assert_eq!(merged.transfer.recording_path, "/Users/me/Recordings");
    }

    #[test]
    fn sync_sessions_strip_serial_but_keep_local_terminal_fields() {
        let mut sessions = sample_sessions_with_device_local_connections();

        strip_device_local_sessions(&mut sessions);

        let local = sessions
            .connections
            .iter()
            .find(|connection| connection.id == "local-1")
            .expect("local connection");
        let config::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } = &local.config
        else {
            panic!("expected local terminal");
        };
        assert_eq!(
            shell_path,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
        assert_eq!(shell_args, "-NoLogo");
        assert_eq!(working_dir.as_deref(), Some("C:\\Users\\me"));
        assert!(matches!(
            &local.config,
            config::ConnectionType::LocalTerminal {
                dynamic_tab_title: true,
                ..
            }
        ));

        let serial = sessions
            .connections
            .iter()
            .find(|connection| connection.id == "serial-1")
            .expect("serial connection");
        let config::ConnectionType::Serial { port_name, .. } = &serial.config else {
            panic!("expected serial");
        };
        assert!(port_name.is_empty());
    }

    #[test]
    fn applying_sync_sessions_uses_incoming_local_terminal_fields() {
        let mut current = sample_sessions_with_device_local_connections();
        let mut incoming = sample_sessions_with_device_local_connections();
        let config::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } = &mut current.connections[0].config
        else {
            panic!("expected local terminal");
        };
        *shell_path = "powershell.exe".to_string();
        *shell_args = "-NoExit".to_string();
        *working_dir = Some("C:\\Users\\current".to_string());

        preserve_device_local_sessions(&mut incoming, &current);

        let local = incoming
            .connections
            .iter()
            .find(|connection| connection.id == "local-1")
            .expect("local connection");
        let config::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } = &local.config
        else {
            panic!("expected local terminal");
        };
        assert_eq!(
            shell_path,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
        assert_eq!(shell_args, "-NoLogo");
        assert_eq!(working_dir.as_deref(), Some("C:\\Users\\me"));

        let config::ConnectionType::Serial { port_name, .. } = &mut incoming.connections[1].config
        else {
            panic!("expected serial");
        };
        port_name.clear();
        preserve_device_local_sessions(&mut incoming, &current);

        let serial = incoming
            .connections
            .iter()
            .find(|connection| connection.id == "serial-1")
            .expect("serial connection");
        let config::ConnectionType::Serial { port_name, .. } = &serial.config else {
            panic!("expected serial");
        };
        assert_eq!(port_name, "COM3");
    }

    #[test]
    fn applying_sync_sessions_preserves_local_terminal_for_legacy_empty_snapshot() {
        let current = sample_sessions_with_device_local_connections();
        let mut incoming = sample_sessions_with_device_local_connections();
        let config::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } = &mut incoming.connections[0].config
        else {
            panic!("expected local terminal");
        };
        shell_path.clear();
        shell_args.clear();
        *working_dir = None;

        preserve_device_local_sessions(&mut incoming, &current);

        let config::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } = &incoming.connections[0].config
        else {
            panic!("expected local terminal");
        };
        assert_eq!(
            shell_path,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
        assert_eq!(shell_args, "-NoLogo");
        assert_eq!(working_dir.as_deref(), Some("C:\\Users\\me"));
    }

    #[test]
    fn sync_sessions_strip_and_preserve_ssh_agent_device_fields() {
        let mut current = sample_sessions_with_asset_metadata();
        let mut incoming = sample_sessions_with_asset_metadata();

        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            legacy_agent_forwarding,
            agent_forwarding_config,
            ..
        } = &mut current.connections[0].config
        else {
            panic!("expected SSH connection");
        };
        *auth_agent_endpoint = Some(config::SshAgentEndpoint::Pageant);
        *legacy_agent_forwarding = None;
        *agent_forwarding_config = Some(config::SshAgentForwardingConfig {
            enabled: true,
            sources: config::SshAgentForwardingSources {
                external_agent: true,
                external_agent_endpoints: vec![config::SshAgentEndpoint::Pageant],
                stored_keys: false,
            },
            policy: config::SshAgentForwardingPolicy::All,
        });

        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            legacy_agent_forwarding,
            agent_forwarding_config,
            ..
        } = &mut incoming.connections[0].config
        else {
            panic!("expected SSH connection");
        };
        *auth_agent_endpoint = Some(config::SshAgentEndpoint::UnixSocket {
            path: "/Users/me/.ssh/agent.sock".to_string(),
        });
        *legacy_agent_forwarding = None;
        *agent_forwarding_config = Some(config::SshAgentForwardingConfig::default());

        strip_device_local_sessions(&mut incoming);
        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            legacy_agent_forwarding,
            agent_forwarding_config,
            ..
        } = &incoming.connections[0].config
        else {
            panic!("expected SSH connection");
        };
        assert!(auth_agent_endpoint.is_none());
        assert!(legacy_agent_forwarding.is_none());
        assert!(agent_forwarding_config.is_none());

        preserve_device_local_sessions(&mut incoming, &current);
        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            legacy_agent_forwarding,
            agent_forwarding_config,
            ..
        } = &incoming.connections[0].config
        else {
            panic!("expected SSH connection");
        };
        assert_eq!(auth_agent_endpoint, &Some(config::SshAgentEndpoint::Pageant));
        assert!(legacy_agent_forwarding.is_none());
        assert_eq!(
            agent_forwarding_config,
            &Some(config::SshAgentForwardingConfig {
                enabled: true,
                sources: config::SshAgentForwardingSources {
                    external_agent: true,
                    external_agent_endpoints: vec![config::SshAgentEndpoint::Pageant],
                    stored_keys: false,
                },
                policy: config::SshAgentForwardingPolicy::All,
            })
        );
    }

    fn sample_portable_settings() -> PortableAppSettings {
        PortableAppSettings {
            general: config::GeneralSettings::default(),
            appearance: config::AppearanceSettings::default(),
            proxy: config::ProxySettings::default(),
            search: config::SearchSettings::default(),
            translation: config::TranslationSettings::default(),
            security: config::SecuritySettings::default(),
            terminal: config::TerminalSettings::default(),
            interaction: config::InteractionSettings::default(),
            transfer: config::TransferSettings::default(),
            diagnostics: config::DiagnosticsSettings::default(),
            ai: config::AiSettings::default(),
            ui: PortableUiSettings {
                language: Some("en".to_string()),
                panel_open_mode: "docked".to_string(),
                header_status_visible: true,
                show_remote_stats: false,
                remote_stats_interval: 3,
                show_gpu_monitor: false,
                gpu_monitor_interval: 3,
                show_ascend_npu_monitor: false,
                ascend_npu_monitor_interval: 3,
                show_process_manager: false,
                process_manager_interval: 5,
                show_docker_manager: false,
                docker_manager_interval: 10,
                saved_connections_sort_mode: "default".to_string(),
                asset_sort_key: None,
                asset_sort_direction: None,
                activity_bar_layout: ActivityBarLayout::default(),
                serial_send_clear_after_send: true,
            },
        }
    }

    fn sample_snapshot() -> PortableSnapshot {
        let mut snapshot = PortableSnapshot {
            schema_version: PORTABLE_SNAPSHOT_SCHEMA_VERSION,
            snapshot_kind: PortableSnapshotKind::Sync,
            revision_id: "rev".to_string(),
            device_id: "dev".to_string(),
            created_at_ms: 1,
            payload_hash: String::new(),
            app_version: "1.0.0".to_string(),
            settings: sample_portable_settings(),
            sessions: config::SessionsConfig::default(),
            keys: config::KeysConfig::default(),
            passwords: config::PasswordsConfig::default(),
            credentials: config::CredentialsConfig::default(),
            otp: config::OtpConfig::default(),
            proxies: Vec::new(),
            proxy_groups: Vec::new(),
            tunnels: Vec::new(),
            tunnel_groups: Vec::new(),
            quick_commands: config::QuickCommandsConfig::default(),
            history: Vec::new(),
            master_key_token: Some("wrapped".to_string()),
            known_hosts: "example.com ssh-ed25519 AAAA\n".to_string(),
            notes: config::NotesSnapshot::default(),
        };
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");
        snapshot
    }

    fn sample_sessions_with_device_local_connections() -> config::SessionsConfig {
        config::SessionsConfig {
            groups: Vec::new(),
            connections: vec![
                config::SavedConnection {
                    id: "local-1".to_string(),
                    name: "Local PowerShell".to_string(),
                    config: config::ConnectionType::LocalTerminal {
                        shell_path:
                            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
                                .to_string(),
                        shell_args: "-NoLogo".to_string(),
                        working_dir: Some("C:\\Users\\me".to_string()),
                        ai_execution_profile: config::AiExecutionProfile::Auto,
                        encoding: String::new(),
                        dynamic_tab_title: true,
                    },
                    group_id: None,
                    description: None,
                    sort_order: 0,
                    icon: None,
                    icon_auto_detect: None,
                    auth: None,
                    network: None,
                    post_login: None,
                    recording: None,
                    ssh_algorithms: None,
                    ssh_profile: Default::default(),
                    terminal_type: None,
                    sftp: config::SftpSettings::default(),
                    asset: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                    last_used_at_ms: None,
                },
                config::SavedConnection {
                    id: "serial-1".to_string(),
                    name: "Serial Console".to_string(),
                    config: config::ConnectionType::Serial {
                        port_name: "COM3".to_string(),
                        baud_rate: 115_200,
                        data_bits: 8,
                        parity: "none".to_string(),
                        stop_bits: "1".to_string(),
                        ai_execution_profile: config::AiExecutionProfile::Auto,
                        backspace_mode: "del".to_string(),
                        encoding: String::new(),
                    },
                    group_id: None,
                    description: None,
                    sort_order: 1,
                    icon: None,
                    icon_auto_detect: None,
                    auth: None,
                    network: None,
                    post_login: None,
                    recording: None,
                    ssh_algorithms: None,
                    ssh_profile: Default::default(),
                    terminal_type: None,
                    sftp: config::SftpSettings::default(),
                    asset: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                    last_used_at_ms: None,
                },
            ],
            custom_icons: Vec::new(),
        }
    }

    fn sample_sessions_with_asset_metadata() -> config::SessionsConfig {
        config::SessionsConfig {
            groups: Vec::new(),
            connections: vec![config::SavedConnection {
                id: "asset-1".to_string(),
                name: "Asset Host".to_string(),
                config: config::ConnectionType::Ssh {
                    host: "10.0.0.2".to_string(),
                    port: 22,
                    username: "root".to_string(),
                    backspace_mode: "del".to_string(),
                    x11_forwarding: false,
                    auth_agent_endpoint: None,
                    legacy_agent_forwarding: None,
                    agent_forwarding_config: None,
                    encoding: String::new(),
                    dynamic_tab_title: true,
                },
                group_id: None,
                description: None,
                sort_order: 0,
                icon: None,
                icon_auto_detect: None,
                auth: None,
                network: None,
                post_login: None,
                recording: None,
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: config::SftpSettings::default(),
                asset: Some(config::AssetMetadata {
                    device_type: Some(config::AssetDeviceType::Physical),
                    os_name: Some("Ubuntu".to_string()),
                    os_version: Some("24.04".to_string()),
                    architecture: Some("x86_64".to_string()),
                    kernel_version: Some("6.8.0".to_string()),
                    hostname: Some("gpu-node-01".to_string()),
                    cpu_model: Some("AMD EPYC 9654".to_string()),
                    cpu_sockets: Some(2),
                    cpu_cores: Some(192),
                    cpu_threads: Some(384),
                    memory_bytes: Some(1_099_511_627_776),
                    accelerators: Some(vec![config::AssetAccelerator {
                        r#type: config::AssetAcceleratorType::Gpu,
                        vendor: Some("NVIDIA".to_string()),
                        model: Some("H100".to_string()),
                        count: Some(8),
                        memory_bytes: Some(85_899_345_920),
                    }]),
                    disks: Some(Vec::new()),
                    tags: Some(vec!["training".to_string(), "production".to_string()]),
                    notes: Some("Static asset metadata".to_string()),
                    updated_at: Some("2026-08-03T12:00:00.000Z".to_string()),
                }),
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            }],
            custom_icons: Vec::new(),
        }
    }

    fn assert_asset_metadata_preserved(sessions: &config::SessionsConfig) {
        assert!(matches!(
            &sessions.connections[0].config,
            config::ConnectionType::Ssh {
                dynamic_tab_title: true,
                ..
            }
        ));
        let asset = sessions.connections[0].asset.as_ref().expect("asset");
        assert_eq!(asset.device_type, Some(config::AssetDeviceType::Physical));
        assert_eq!(asset.hostname.as_deref(), Some("gpu-node-01"));
        assert_eq!(asset.cpu_threads, Some(384));
        assert_eq!(
            asset.updated_at.as_deref(),
            Some("2026-08-03T12:00:00.000Z")
        );
        assert_eq!(
            asset.accelerators
                .as_ref()
                .and_then(|items| items.first())
                .map(|item| &item.r#type),
            Some(&config::AssetAcceleratorType::Gpu)
        );
        assert_eq!(asset.disks.as_ref(), Some(&Vec::new()));
    }

    #[test]
    fn portable_snapshot_hash_changes_when_entity_changes() {
        let left = sample_snapshot();
        let mut right = sample_snapshot();
        right.master_key_token = Some("different".to_string());
        right.payload_hash = calculate_payload_hash(&right).expect("right hash");

        assert_ne!(left.payload_hash, right.payload_hash);
    }

    #[test]
    fn portable_snapshot_hash_changes_when_note_markdown_changes() {
        let left = sample_snapshot();
        let mut right = sample_snapshot();
        right.notes.notes.push(config::NoteDocument {
            id: "note-1".to_string(),
            parent_id: None,
            title: "Note".to_string(),
            markdown: "first".to_string(),
            sort_order: 0,
            revision: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
        });
        right.payload_hash = calculate_payload_hash(&right).expect("right hash");

        assert_ne!(left.payload_hash, right.payload_hash);
    }

    #[test]
    fn portable_snapshot_zip_roundtrip() {
        let snapshot = sample_snapshot();

        let encoded = encode_portable_snapshot(&snapshot).expect("encode snapshot");
        let decoded = super::decode_portable_snapshot(&encoded).expect("decode snapshot");

        assert_eq!(decoded.revision_id, snapshot.revision_id);
        assert_eq!(decoded.payload_hash, snapshot.payload_hash);
        assert_eq!(decoded.master_key_token, snapshot.master_key_token);
        assert_eq!(decoded.known_hosts, snapshot.known_hosts);
        assert_eq!(decoded.notes, snapshot.notes);
    }

    #[test]
    fn portable_backup_snapshot_zip_preserves_asset_metadata() {
        let mut snapshot = sample_snapshot();
        snapshot.snapshot_kind = PortableSnapshotKind::Backup;
        snapshot.sessions = sample_sessions_with_asset_metadata();
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");

        let encoded = encode_portable_snapshot(&snapshot).expect("encode snapshot");
        let decoded = super::decode_portable_snapshot(&encoded).expect("decode snapshot");

        assert_asset_metadata_preserved(&decoded.sessions);
    }

    #[test]
    fn portable_settings_deserializes_legacy_shape_without_ai() {
        let settings = sample_portable_settings();
        let mut raw = serde_json::to_value(&settings).expect("settings json");
        raw.as_object_mut().expect("settings object").remove("ai");

        let decoded: PortableAppSettings =
            serde_json::from_value(raw).expect("legacy settings decode");

        assert_eq!(
            decoded.ai.schema_version,
            config::AiSettings::default().schema_version
        );
        assert_eq!(
            decoded.ai.active_profile_id,
            config::AiSettings::default().active_profile_id
        );
    }

    #[test]
    fn corrupt_portable_snapshot_redb_returns_error() {
        let error = super::decode_portable_snapshot(b"not a redb file")
            .expect_err("corrupt snapshot should fail");

        assert!(matches!(error, AppError::Storage(_)));
    }

    fn sample_snapshot_with_quick_command() -> PortableSnapshot {
        let mut snapshot = sample_snapshot();
        snapshot.quick_commands.commands.push(config::QuickCommand {
            id: "cmd-1".to_string(),
            label: "List".to_string(),
            command: "ls".to_string(),
            category_id: None,
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: false,
            execution_mode: "execute".to_string(),
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
        });
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");
        snapshot
    }

    fn v121_quick_command_entities(snapshot: &PortableSnapshot) -> BTreeMap<String, String> {
        let mut quick_commands =
            serde_json::to_value(&snapshot.quick_commands).expect("quick commands json");
        quick_commands["commands"][0]
            .as_object_mut()
            .expect("quick command object")
            .remove("sort_order");

        let mut entities = snapshot_entities(snapshot);
        entities.insert(
            "quick_commands".to_string(),
            serde_json::to_string(&quick_commands).expect("quick commands raw"),
        );
        entities
    }

    fn snapshot_entities(snapshot: &PortableSnapshot) -> BTreeMap<String, String> {
        let mut entities = BTreeMap::new();
        entities.insert(
            "settings".to_string(),
            serde_json::to_string(&snapshot.settings).expect("settings raw"),
        );
        entities.insert(
            "sessions".to_string(),
            serde_json::to_string(&snapshot.sessions).expect("sessions raw"),
        );
        entities.insert(
            "keys".to_string(),
            serde_json::to_string(&snapshot.keys).expect("keys raw"),
        );
        entities.insert(
            "passwords".to_string(),
            serde_json::to_string(&snapshot.passwords).expect("passwords raw"),
        );
        entities.insert(
            "credentials".to_string(),
            serde_json::to_string(&snapshot.credentials).expect("credentials raw"),
        );
        entities.insert(
            "otp".to_string(),
            serde_json::to_string(&snapshot.otp).expect("otp raw"),
        );
        entities.insert(
            "proxies".to_string(),
            serde_json::to_string(&snapshot.proxies).expect("proxies raw"),
        );
        entities.insert(
            "proxy_groups".to_string(),
            serde_json::to_string(&snapshot.proxy_groups).expect("proxy groups raw"),
        );
        entities.insert(
            "tunnels".to_string(),
            serde_json::to_string(&snapshot.tunnels).expect("tunnels raw"),
        );
        entities.insert(
            "tunnel_groups".to_string(),
            serde_json::to_string(&snapshot.tunnel_groups).expect("tunnel groups raw"),
        );
        entities.insert(
            "quick_commands".to_string(),
            serde_json::to_string(&snapshot.quick_commands).expect("quick commands raw"),
        );
        entities.insert(
            "history".to_string(),
            serde_json::to_string(&snapshot.history).expect("history raw"),
        );
        entities.insert(
            "master_key_token".to_string(),
            serde_json::to_string(&snapshot.master_key_token).expect("master key raw"),
        );
        entities.insert(
            "known_hosts".to_string(),
            serde_json::to_string(&snapshot.known_hosts).expect("known hosts raw"),
        );
        entities.insert(
            "notes".to_string(),
            serde_json::to_string(&snapshot.notes).expect("notes raw"),
        );
        entities
    }

    #[test]
    fn truncated_zip_portable_snapshot_returns_error() {
        let error = super::decode_portable_snapshot(b"PK\x03\x04truncated")
            .expect_err("truncated zip snapshot should fail");

        assert!(matches!(error, AppError::Storage(_)));
    }

    #[test]
    fn portable_snapshot_legacy_redb_roundtrip() {
        let snapshot = sample_snapshot();

        let encoded = encode_portable_snapshot_redb(&snapshot).expect("encode legacy snapshot");
        let decoded = super::decode_portable_snapshot(&encoded).expect("decode legacy snapshot");

        assert_eq!(decoded.revision_id, snapshot.revision_id);
        assert_eq!(decoded.payload_hash, snapshot.payload_hash);
        assert_eq!(decoded.master_key_token, snapshot.master_key_token);
        assert_eq!(decoded.known_hosts, snapshot.known_hosts);
    }

    #[test]
    fn portable_snapshot_decode_result_preserves_v121_quick_command_source_hash() {
        let snapshot = sample_snapshot_with_quick_command();
        let entities = v121_quick_command_entities(&snapshot);
        let source_hash = calculate_v3_raw_payload_hash(&entities).expect("source hash");
        assert_ne!(source_hash, snapshot.payload_hash);
        let encoded =
            encode_v3_raw_snapshot_redb_for_test(&snapshot, &entities, source_hash.clone());

        let decoded = super::decode_portable_snapshot_with_source_hash(&encoded)
            .expect("decode v1.2.1 quick command shape");

        assert_eq!(decoded.source_payload_hash, source_hash);
        assert_eq!(decoded.snapshot.payload_hash, snapshot.payload_hash);
        assert_ne!(decoded.snapshot.payload_hash, decoded.source_payload_hash);
        assert_eq!(decoded.snapshot.quick_commands.commands[0].sort_order, None);
    }

    #[test]
    fn portable_snapshot_decode_result_source_hash_matches_current_snapshot_hash() {
        let snapshot = sample_snapshot_with_quick_command();
        let encoded = encode_portable_snapshot(&snapshot).expect("encode snapshot");

        let decoded =
            super::decode_portable_snapshot_with_source_hash(&encoded).expect("decode snapshot");

        assert_eq!(decoded.source_payload_hash, snapshot.payload_hash);
        assert_eq!(decoded.snapshot.payload_hash, snapshot.payload_hash);
    }

    #[test]
    fn portable_sync_snapshot_redb_preserves_asset_metadata() {
        let mut snapshot = sample_snapshot();
        snapshot.snapshot_kind = PortableSnapshotKind::Sync;
        snapshot.sessions = sample_sessions_with_asset_metadata();
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");

        let encoded = encode_portable_snapshot_redb(&snapshot).expect("encode legacy snapshot");
        let decoded = super::decode_portable_snapshot(&encoded).expect("decode snapshot");

        assert_asset_metadata_preserved(&decoded.sessions);
    }

    #[test]
    fn portable_snapshot_v3_accepts_older_entity_shape_before_normalizing_hash() {
        let snapshot = sample_snapshot();
        let mut settings = serde_json::to_value(&snapshot.settings).expect("settings json");
        settings["appearance"]
            .as_object_mut()
            .expect("appearance object")
            .remove("panel_multi_open");
        settings["ui"]
            .as_object_mut()
            .expect("ui object")
            .remove("panel_open_mode");

        let mut entities = snapshot_entities(&snapshot);
        entities.remove("proxy_groups");
        entities.remove("tunnel_groups");
        entities.remove("notes");
        entities.insert(
            "settings".to_string(),
            serde_json::to_string(&settings).expect("settings raw"),
        );

        let legacy_hash = calculate_v3_raw_payload_hash(&entities).expect("legacy hash");
        assert_ne!(legacy_hash, snapshot.payload_hash);

        let encoded =
            encode_v3_raw_snapshot_redb_for_test(&snapshot, &entities, legacy_hash.clone());
        let decoded = super::decode_portable_snapshot_with_source_hash(&encoded)
            .expect("decode legacy v3 shape");

        assert_eq!(decoded.source_payload_hash, legacy_hash);
        assert_eq!(decoded.snapshot.revision_id, snapshot.revision_id);
        assert!(!decoded.snapshot.settings.appearance.panel_multi_open);
        assert_eq!(decoded.snapshot.settings.ui.panel_open_mode, "docked");
        assert!(decoded.snapshot.notes.folders.is_empty());
        assert!(decoded.snapshot.notes.notes.is_empty());
        assert_eq!(decoded.snapshot.payload_hash, snapshot.payload_hash);
        assert_ne!(decoded.snapshot.payload_hash, legacy_hash);
    }

    #[test]
    fn portable_snapshot_zip_rejects_oversized_payload() {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(SNAPSHOT_ZIP_PAYLOAD_NAME, options)
            .expect("start payload");

        let chunk = vec![0u8; 1024 * 1024];
        for _ in 0..=50 {
            zip.write_all(&chunk).expect("write payload");
        }
        let bytes = zip.finish().expect("finish zip").into_inner();

        let error =
            super::decode_compressed_snapshot_payload(&bytes).expect_err("oversized payload");
        assert!(
            error
                .to_string()
                .contains("decompressed snapshot payload exceeds maximum allowed size"),
            "{error}"
        );
    }

    #[test]
    fn portable_snapshot_zip_reduces_history_heavy_payload_size() {
        let mut snapshot = sample_snapshot();
        snapshot.snapshot_kind = PortableSnapshotKind::Backup;
        snapshot.history = (0..5_000)
            .map(|index| crate::core::history::HistoryEntry {
                command: format!("kubectl get pods --namespace production-{index:04} --watch"),
                last_used_at_ms: 1_700_000_000_000 + index,
                use_count: 1,
            })
            .collect();
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");

        let legacy = encode_portable_snapshot_redb(&snapshot).expect("encode legacy snapshot");
        let compressed = encode_portable_snapshot(&snapshot).expect("encode compressed snapshot");
        let reduction = 100.0 - ((compressed.len() as f64 / legacy.len() as f64) * 100.0);

        println!(
            "portable snapshot size: legacy_redb={} compressed_zip={} reduction={reduction:.1}%",
            legacy.len(),
            compressed.len(),
        );
        assert!(
            compressed.len() < legacy.len(),
            "compressed snapshot should be smaller than legacy redb"
        );
    }

}
