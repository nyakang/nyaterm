use std::path::PathBuf;

use aes_gcm::{Aes256Gcm, Key, KeyInit, aead::Aead};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use nyaterm_core::{
    AiExecutionProfile, AssetAccelerator, AssetAcceleratorType, AssetDeviceType, AssetMetadata,
    CloudSyncSettings, CloudSyncState, CommandHistoryEntry, ConnectionAuth, ConnectionType,
    ExistingFileBehavior, MainWindowBounds, MainWindowState, OtpEntry, RecordingMode,
    RecordingRotationPolicy, SavedCredential, SearchEngineConfig, SshKey,
    export_quick_commands_json,
};
use redb::{Database, ReadableDatabase};
use sha2::{Digest, Sha256};

use super::{
    AiSettings, COMMAND_HISTORY_PREFIX, COMMAND_HISTORY_TABLE, CONNECTION_PASSWORD_PREFIX,
    CONNECTIONS_TABLE, CREDENTIALS_TABLE, ConnectionPasswordRecord, ConnectionStore, DATABASE_FILE,
    Group, KNOWN_HOSTS_TABLE, KnownHostCheck, LEGACY_TEXT_CLOUD_SYNC_STATE,
    LEGACY_TEXT_KNOWN_HOSTS, LEGACY_TEXT_REMOTE_FILE_BACKEND_CACHE, META_MASTER_KEY,
    META_PORTABLE_SOURCE_PAYLOAD_HASH, META_PORTABLE_SOURCE_SCHEMA_VERSION, META_TABLE,
    OTP_ACCOUNTS_TABLE, OTP_PREFIX, PORTABLE_OPAQUE_ENTITIES_TABLE, QuickCommand,
    QuickCommandCategory, QuickCommandsConfig, RDP_KNOWN_HOST_PREFIX, RDP_KNOWN_HOSTS_TABLE,
    RdpKnownHostCheck, SETTINGS_CLOUD_SYNC, SETTINGS_DEFAULT, SETTINGS_MAIN_WINDOW_STATE,
    SETTINGS_QUICK_COMMANDS, SETTINGS_TABLE, SSH_KEY_FILE_IMPORT_MAX_BYTES, SSH_KEY_PREFIX,
    SavedConnection, SessionsConfig, StorageError, TEXT_DOCS_TABLE, TUNNELS_TABLE, TunnelConfig,
    TunnelGroup, current_time_ms, default_settings_value, deserialize_json, entity_key, json_path,
    set_nested_json_value, stable_id, write_json_in_txn,
};

#[test]
fn mark_connection_used_persists_legacy_agent_forwarding_migration() {
    let dir = unique_temp_dir("mark-used-agent-migration");
    let store = ConnectionStore::open(&dir).expect("store");
    let legacy = serde_json::json!({
        "id": "legacy-agent",
        "name": "Legacy Agent",
        "type": "ssh",
        "host": "example.com",
        "port": 22,
        "username": "root",
        "auth": { "mode": "agent" },
        "agent_endpoint": { "type": "unix_socket", "path": "/tmp/legacy-agent.sock" },
        "agent_forwarding": true
    });
    let txn = store.db.begin_write().expect("write transaction");
    write_json_in_txn(
        &txn,
        CONNECTIONS_TABLE,
        &entity_key("connections/", "legacy-agent"),
        &legacy,
    )
    .expect("write legacy connection");
    txn.commit().expect("commit legacy connection");

    store
        .mark_connection_used("legacy-agent")
        .expect("mark connection used");
    let raw: serde_json::Value = {
        let txn = store.db.begin_read().expect("read transaction");
        let table = txn.open_table(CONNECTIONS_TABLE).expect("connections");
        let value = table
            .get(entity_key("connections/", "legacy-agent").as_str())
            .expect("read legacy connection")
            .expect("legacy connection exists");
        deserialize_json(value.value()).expect("decode migrated connection")
    };
    assert!(raw.get("agent_forwarding").is_none());
    assert_eq!(raw["agent_forwarding_config"]["enabled"], true);
    assert_eq!(
        raw["agent_forwarding_config"]["sources"]["stored_keys"],
        false
    );
    assert_eq!(raw["agent_forwarding_config"]["policy"]["mode"], "all");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn round_trips_sessions_in_redb_compatible_tables() {
    let dir = unique_temp_dir("round-trip");
    let store = ConnectionStore::open(&dir).expect("store");
    let config = SessionsConfig {
        custom_icons: Vec::new(),
        groups: vec![Group {
            id: "group-1".to_string(),
            name: "Servers".to_string(),
            parent_id: None,
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        }],
        connections: vec![SavedConnection {
            extensions: Default::default(),
            id: "conn-1".to_string(),
            name: "Production".to_string(),
            config: ConnectionType::Ssh {
                host: "10.0.0.8".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: Some("group-1".to_string()),
            description: Some("Primary".to_string()),
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "password".to_string(),
                password: Some("secret".to_string().into()),
                ..Default::default()
            }),
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            recording: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }],
    };

    store.replace_sessions(&config).expect("replace");
    let loaded = store.load_sessions().expect("load");

    assert_eq!(loaded.groups.len(), 1);
    assert_eq!(loaded.connections.len(), 1);
    assert_eq!(loaded.connections[0].endpoint(), "root@10.0.0.8:22");
    assert_eq!(
        loaded.connections[0]
            .auth
            .as_ref()
            .and_then(|auth| auth.password.as_deref()),
        Some("secret")
    );
    assert!(
        loaded.connections[0]
            .auth
            .as_ref()
            .is_some_and(|auth| auth.has_password)
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn exports_and_imports_native_redb_backup() {
    let source_dir = unique_temp_dir("backup-source");
    let target_dir = unique_temp_dir("backup-target");
    let backup_path = unique_temp_dir("backup-output").join("nyaterm.redb");
    let source_store = ConnectionStore::open(&source_dir).expect("source store");
    let config = SessionsConfig {
        custom_icons: Vec::new(),
        groups: vec![Group {
            id: "ops".to_string(),
            name: "Ops".to_string(),
            parent_id: None,
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        }],
        connections: vec![SavedConnection {
            extensions: Default::default(),
            id: "local-1".to_string(),
            name: "Shell".to_string(),
            config: ConnectionType::LocalTerminal {
                shell_path: "/bin/sh".to_string(),
                shell_args: String::new(),
                working_dir: Some("/tmp".to_string()),
                ai_execution_profile: AiExecutionProfile::Auto,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: Some("ops".to_string()),
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            recording: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }],
    };
    source_store.replace_sessions(&config).expect("seed source");
    drop(source_store);

    let export = ConnectionStore::export_config_database(&source_dir, None, &backup_path)
        .expect("export backup");
    assert_eq!(export.backup_path, backup_path);
    assert!(export.bytes > 0);

    let target_store = ConnectionStore::open(&target_dir).expect("target store");
    target_store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: Vec::new(),
        })
        .expect("seed target");
    drop(target_store);

    let import = ConnectionStore::import_config_database(&target_dir, None, &backup_path)
        .expect("import backup");
    assert!(import.safety_backup_path.is_some());
    let loaded = ConnectionStore::open(&target_dir)
        .expect("open imported")
        .load_sessions()
        .expect("load imported");
    assert_eq!(loaded.groups.len(), 1);
    assert_eq!(loaded.connections.len(), 1);
    assert_eq!(loaded.connections[0].name, "Shell");

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    if let Some(parent) = backup_path.parent() {
        std::fs::remove_dir_all(parent).ok();
    }
}

#[test]
fn exports_and_imports_portable_snapshot() {
    let source_dir = unique_temp_dir("portable-source");
    let target_dir = unique_temp_dir("portable-target");
    let snapshot_path = unique_temp_dir("portable-output").join("nyaterm.nya");

    let source_store = ConnectionStore::open(&source_dir).expect("source store");
    source_store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: vec![Group {
                id: "group-1".to_string(),
                name: "Servers".to_string(),
                parent_id: None,
                sort_order: 0,
                created_at_ms: None,
                updated_at_ms: None,
            }],
            connections: vec![SavedConnection {
                extensions: Default::default(),
                id: "conn-1".to_string(),
                name: "Production".to_string(),
                config: ConnectionType::Ssh {
                    host: "10.0.0.8".to_string(),
                    port: 22,
                    username: "deploy".to_string(),
                    backspace_mode: "del".to_string(),
                    ai_execution_profile: AiExecutionProfile::Auto,
                    x11_forwarding: true,
                    auth_agent_endpoint: None,
                    agent_forwarding_config: None,
                    legacy_agent_forwarding: None,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_id: Some("group-1".to_string()),
                description: Some("Primary".to_string()),
                sort_order: 0,
                icon: None,
                icon_auto_detect: None,
                auth: Some(ConnectionAuth {
                    mode: "password".to_string(),
                    password: Some("session-secret".to_string().into()),
                    ..Default::default()
                }),
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: Default::default(),
                network: None,
                post_login: None,
                recording: None,
                asset: None,
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            }],
        })
        .expect("seed sessions");
    source_store
        .replace_tunnels(&[TunnelConfig {
            id: "tun-1".to_string(),
            name: "DB".to_string(),
            tunnel_type: "local".to_string(),
            connection_id: Some("conn-1".to_string()),
            listen_port: 15432,
            target_host: "127.0.0.1".to_string(),
            target_port: 5432,
            is_open: false,
            auto_open: true,
            bind_localhost: true,
            group_id: Some("tg-1".to_string()),
        }])
        .expect("seed tunnels");
    source_store
        .replace_tunnel_groups(&[TunnelGroup {
            id: "tg-1".to_string(),
            name: "Databases".to_string(),
            sort_order: 2,
        }])
        .expect("seed tunnel groups");
    source_store
        .replace_known_hosts_export("example.com ssh-ed25519 AAAA\n")
        .expect("seed known hosts");
    source_store
        .save_settings_value(&serde_json::json!({
            "appearance": {
                "theme": "github-light",
                "font_family": "Berkeley Mono",
                "font_size": 14,
                "panel_multi_open": true
            },
            "ui": {
                "activity_bar_layout": {
                    "left_top": ["fileExplorer", "notes", "network"],
                    "hidden_items": ["network"]
                },
                "panel_open_mode": "floating",
                "panel_multi_open": true,
                "show_notes_panel": true
            },
            "security": {
                "master_password": "source-local-secret",
                "host_key_policy": "strict"
            },
            "transfer": {
                "duplicate_strategy": "rename"
            }
        }))
        .expect("seed settings");
    source_store
        .save_credential(SavedCredential {
            id: "credential-1".to_string(),
            sort_order: 7,
            name: "Production login".to_string(),
            username: "deploy".to_string(),
            password: Some("credential-secret".to_string().into()),
            username_prompt_regex: Some("(?i)login:".to_string()),
            password_prompt_regex: Some("(?i)password:".to_string()),
            enabled: true,
            has_password: false,
        })
        .expect("seed credential");
    {
        let txn = source_store.db.begin_write().expect("txn");
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(SSH_KEY_PREFIX, "key-1"),
            &SshKey {
                id: "key-1".to_string(),
                name: "Deploy".to_string(),
                key: Some("encrypted-key".to_string().into()),
                cert: None,
                passphrase: None,
                key_file_path: None,
                cert_file_path: None,
                has_key_data: false,
                has_cert_data: false,
            },
        )
        .expect("write key");
        txn.commit().expect("commit");
    }
    drop(source_store);

    let target_store = ConnectionStore::open(&target_dir).expect("target store");
    target_store
        .save_settings_value(&serde_json::json!({
            "security": {
                "master_password": "target-local-secret",
                "host_key_policy": "prompt"
            }
        }))
        .expect("seed target settings");
    drop(target_store);

    let export =
        ConnectionStore::export_portable_snapshot(&source_dir, None, &snapshot_path, "dev", "1")
            .expect("export snapshot");
    assert_eq!(export.backup_path, snapshot_path);
    assert!(snapshot_path.exists());

    let import = ConnectionStore::import_portable_snapshot(&target_dir, None, &snapshot_path)
        .expect("import snapshot");
    assert!(import.safety_backup_path.is_some());

    let imported = ConnectionStore::open(&target_dir).expect("imported store");
    let sessions = imported.load_sessions().expect("load sessions");
    assert_eq!(sessions.connections.len(), 1);
    assert_eq!(sessions.connections[0].endpoint(), "deploy@10.0.0.8:22");
    assert_eq!(
        sessions.connections[0]
            .auth
            .as_ref()
            .and_then(|auth| auth.password.as_deref()),
        Some("session-secret")
    );
    assert_eq!(imported.list_ssh_keys().expect("keys")[0].name, "Deploy");
    let credentials = imported.list_credentials().expect("credentials");
    assert_eq!(credentials[0].sort_order, 7);
    let credential = imported
        .load_decrypted_credential_by_id("credential-1")
        .expect("load credential")
        .expect("credential");
    assert_eq!(credential.password.as_deref(), Some("credential-secret"));
    assert_eq!(imported.list_tunnels().expect("tunnels")[0].name, "DB");
    assert_eq!(
        imported.list_tunnel_groups().expect("tunnel groups")[0].name,
        "Databases"
    );
    assert_eq!(
        imported.render_known_hosts_export().expect("known hosts"),
        "example.com ssh-ed25519 AAAA\n"
    );
    let settings = imported.load_settings_value().expect("settings");
    assert_eq!(
        json_path(&settings, &["appearance", "theme"]).and_then(serde_json::Value::as_str),
        Some("github-light")
    );
    assert_eq!(settings["ui"]["panel_open_mode"], "floating");
    assert_eq!(settings["ui"]["panel_multi_open"], true);
    assert_eq!(settings["appearance"]["panel_multi_open"], true);
    assert_eq!(
        settings["ui"]["activity_bar_layout"]["left_top"],
        serde_json::json!(["fileExplorer", "notes", "network"])
    );
    assert_eq!(
        settings["ui"]["activity_bar_layout"]["hidden_items"],
        serde_json::json!(["network"])
    );
    assert_eq!(settings["ui"]["show_notes_panel"], true);
    assert_eq!(
        json_path(&settings, &["security", "master_password"]).and_then(serde_json::Value::as_str),
        Some("target-local-secret")
    );

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::remove_dir_all(parent).ok();
    }
}

#[test]
fn encrypted_portable_snapshot_requires_master_password() {
    let source_dir = unique_temp_dir("portable-encrypted-source");
    let target_dir = unique_temp_dir("portable-encrypted-target");
    let wrong_target_dir = unique_temp_dir("portable-encrypted-wrong-target");
    let snapshot_path = unique_temp_dir("portable-encrypted-output").join("nyaterm.nya");

    let source_store = ConnectionStore::open(&source_dir).expect("source store");
    source_store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![SavedConnection {
                extensions: Default::default(),
                id: "conn-1".to_string(),
                name: "Encrypted Snapshot".to_string(),
                config: ConnectionType::LocalTerminal {
                    shell_path: "bash".to_string(),
                    shell_args: String::new(),
                    working_dir: None,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_id: None,
                description: None,
                sort_order: 0,
                icon: None,
                icon_auto_detect: None,
                auth: None,
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: Default::default(),
                network: None,
                post_login: None,
                recording: None,
                asset: None,
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            }],
        })
        .expect("seed source");
    drop(source_store);

    assert!(
        ConnectionStore::export_encrypted_portable_snapshot(
            &source_dir,
            None,
            &snapshot_path,
            "dev",
            "1",
            "",
        )
        .is_err()
    );

    ConnectionStore::export_encrypted_portable_snapshot(
        &source_dir,
        None,
        &snapshot_path,
        "dev",
        "1",
        "secret",
    )
    .expect("export encrypted snapshot");
    assert!(
        crate::decode_raw_portable_snapshot(
            &std::fs::read(&snapshot_path).expect("read encrypted snapshot")
        )
        .is_err()
    );

    let wrong_target = ConnectionStore::open(&wrong_target_dir).expect("wrong target");
    wrong_target
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![SavedConnection {
                extensions: Default::default(),
                id: "keep".to_string(),
                name: "Keep".to_string(),
                config: ConnectionType::LocalTerminal {
                    shell_path: "zsh".to_string(),
                    shell_args: String::new(),
                    working_dir: None,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_id: None,
                description: None,
                sort_order: 0,
                icon: None,
                icon_auto_detect: None,
                auth: None,
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: Default::default(),
                network: None,
                post_login: None,
                recording: None,
                asset: None,
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            }],
        })
        .expect("seed wrong target");
    drop(wrong_target);
    assert!(
        ConnectionStore::import_encrypted_portable_snapshot(
            &wrong_target_dir,
            None,
            &snapshot_path,
            "wrong",
        )
        .is_err()
    );
    let preserved = ConnectionStore::open(&wrong_target_dir)
        .expect("open wrong target")
        .load_sessions()
        .expect("load preserved");
    assert_eq!(preserved.connections[0].name, "Keep");

    ConnectionStore::import_encrypted_portable_snapshot(
        &target_dir,
        None,
        &snapshot_path,
        "secret",
    )
    .expect("import encrypted snapshot");
    let imported = ConnectionStore::open(&target_dir)
        .expect("open imported")
        .load_sessions()
        .expect("load imported");
    assert_eq!(imported.connections[0].name, "Encrypted Snapshot");

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(wrong_target_dir).ok();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::remove_dir_all(parent).ok();
    }
}

#[test]
fn rejects_invalid_backup_without_replacing_current_database() {
    let target_dir = unique_temp_dir("backup-reject-target");
    let invalid_dir = unique_temp_dir("backup-reject-invalid");
    std::fs::create_dir_all(&invalid_dir).expect("invalid dir");
    let invalid_path = invalid_dir.join("not-redb.redb");
    std::fs::write(&invalid_path, b"not a redb database").expect("write invalid");
    let store = ConnectionStore::open(&target_dir).expect("target store");
    store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![SavedConnection {
                extensions: Default::default(),
                id: "keep".to_string(),
                name: "Keep".to_string(),
                config: ConnectionType::LocalTerminal {
                    shell_path: String::new(),
                    shell_args: String::new(),
                    working_dir: None,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_id: None,
                description: None,
                sort_order: 0,
                icon: None,
                icon_auto_detect: None,
                auth: None,
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: Default::default(),
                network: None,
                post_login: None,
                recording: None,
                asset: None,
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            }],
        })
        .expect("seed target");
    drop(store);

    assert!(ConnectionStore::import_config_database(&target_dir, None, &invalid_path).is_err());
    let loaded = ConnectionStore::open(&target_dir)
        .expect("open target")
        .load_sessions()
        .expect("load target");
    assert_eq!(loaded.connections.len(), 1);
    assert_eq!(loaded.connections[0].name, "Keep");

    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(invalid_dir).ok();
}

#[test]
fn load_tunnels_reads_legacy_tunnel_table() {
    let dir = unique_temp_dir("tunnels");
    let store = ConnectionStore::open(&dir).expect("store");
    let tunnel = TunnelConfig {
        id: "tunnel-1".to_string(),
        name: "Local Web".to_string(),
        tunnel_type: "local".to_string(),
        connection_id: Some("conn-1".to_string()),
        listen_port: 8080,
        target_host: "127.0.0.1".to_string(),
        target_port: 80,
        is_open: true,
        auto_open: true,
        bind_localhost: true,
        group_id: None,
    };
    let txn = store.db.begin_write().expect("txn");
    write_json_in_txn(&txn, TUNNELS_TABLE, "tunnels/tunnel-1", &tunnel).expect("write tunnel");
    txn.commit().expect("commit");

    let loaded = store.list_tunnels().expect("tunnels");
    assert_eq!(loaded, vec![tunnel]);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_and_delete_connection_updates_store() {
    let dir = unique_temp_dir("delete");
    let store = ConnectionStore::open(&dir).expect("store");
    let connection = SavedConnection {
        extensions: Default::default(),
        id: "local-1".to_string(),
        name: "Local".to_string(),
        config: ConnectionType::LocalTerminal {
            shell_path: "bash".to_string(),
            shell_args: String::new(),
            working_dir: None,
            ai_execution_profile: Default::default(),
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: None,
        description: None,
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        network: None,
        post_login: None,
        recording: None,
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    };

    store.save_connection(&connection).expect("save");
    assert!(store.get_connection("local-1").expect("get").is_some());
    store.delete_connection("local-1").expect("delete");
    assert!(store.get_connection("local-1").expect("missing").is_none());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_group_and_connection_persists_both_records() {
    let dir = unique_temp_dir("group-and-connection");
    let store = ConnectionStore::open(&dir).expect("store");
    let group = Group {
        id: "group-1".to_string(),
        name: "Servers".to_string(),
        parent_id: None,
        sort_order: 0,
        created_at_ms: None,
        updated_at_ms: None,
    };
    let connection = SavedConnection {
        extensions: Default::default(),
        id: "local-grouped".to_string(),
        name: "Local".to_string(),
        config: ConnectionType::LocalTerminal {
            shell_path: "bash".to_string(),
            shell_args: String::new(),
            working_dir: None,
            ai_execution_profile: Default::default(),
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: Some(group.id.clone()),
        description: None,
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        network: None,
        post_login: None,
        recording: None,
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    };

    store
        .save_group_and_connection(&group, &connection)
        .expect("save group and connection");

    assert_eq!(store.list_groups().expect("groups")[0].id, group.id);
    assert_eq!(
        store
            .get_connection(&connection.id)
            .expect("connection")
            .expect("saved connection")
            .group_id
            .as_deref(),
        Some("group-1")
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn deleting_group_removes_descendants_and_grouped_connections() {
    let dir = unique_temp_dir("delete-group-tree");
    let store = ConnectionStore::open(&dir).expect("store");
    let root = Group {
        id: "root".to_string(),
        name: "Root".to_string(),
        parent_id: None,
        sort_order: 0,
        created_at_ms: None,
        updated_at_ms: None,
    };
    let child = Group {
        id: "child".to_string(),
        name: "Child".to_string(),
        parent_id: Some(root.id.clone()),
        sort_order: 0,
        created_at_ms: None,
        updated_at_ms: None,
    };
    for group in [&root, &child] {
        store.save_group(group).expect("save group");
    }
    for (id, group_id) in [
        ("root-connection", root.id.clone()),
        ("child-connection", child.id.clone()),
    ] {
        store
            .save_connection(&SavedConnection {
                extensions: Default::default(),
                id: id.to_string(),
                name: id.to_string(),
                config: ConnectionType::LocalTerminal {
                    shell_path: "bash".to_string(),
                    shell_args: String::new(),
                    working_dir: None,
                    ai_execution_profile: Default::default(),
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_id: Some(group_id),
                description: None,
                sort_order: 0,
                icon: None,
                icon_auto_detect: None,
                auth: None,
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: Default::default(),
                network: None,
                post_login: None,
                recording: None,
                asset: None,
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            })
            .expect("save connection");
    }

    store.delete_group(&root.id).expect("delete group tree");

    assert!(store.list_groups().expect("groups").is_empty());
    assert!(store.list_connections().expect("connections").is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn load_sessions_decrypts_legacy_connection_password_record() {
    let dir = unique_temp_dir("decrypt-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let connection = SavedConnection {
        extensions: Default::default(),
        id: "ssh-1".to_string(),
        name: "SSH".to_string(),
        config: ConnectionType::Ssh {
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            ai_execution_profile: AiExecutionProfile::Auto,
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: None,
        description: None,
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: Some(ConnectionAuth {
            mode: "password".to_string(),
            ..Default::default()
        }),
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        network: None,
        post_login: None,
        recording: None,
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    };
    store.save_connection(&connection).expect("save connection");

    let master_key = test_key(3);
    let master_key_token = encrypt_for_test(master_key.as_slice(), &home_wrapping_key());
    let encrypted_password = encrypt_for_test(b"legacy-secret", &master_key);
    let now = current_time_ms();
    let record = ConnectionPasswordRecord {
        id: "ssh-1".to_string(),
        connection_id: "ssh-1".to_string(),
        password: encrypted_password.clone().into(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    {
        let txn = store.db.begin_write().expect("txn");
        txn.open_table(META_TABLE)
            .expect("meta")
            .insert(META_MASTER_KEY, master_key_token.as_str())
            .expect("insert master");
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(CONNECTION_PASSWORD_PREFIX, "ssh-1"),
            &record,
        )
        .expect("write credential");
        txn.commit().expect("commit");
    }

    let loaded = store.load_sessions().expect("load sessions");
    let auth = loaded.connections[0].auth.as_ref().expect("auth");
    assert_eq!(auth.password.as_deref(), Some("legacy-secret"));
    assert!(!auth.has_password);

    store.mark_connection_used("ssh-1").expect("mark used");
    let stored_record: ConnectionPasswordRecord = {
        let txn = store.db.begin_read().expect("txn");
        let table = txn.open_table(CREDENTIALS_TABLE).expect("credentials");
        let raw = table
            .get(entity_key(CONNECTION_PASSWORD_PREFIX, "ssh-1").as_str())
            .expect("get")
            .expect("record");
        deserialize_json(raw.value()).expect("deserialize")
    };
    assert_eq!(stored_record.password, encrypted_password);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn load_decrypted_ssh_key_reads_legacy_key_store() {
    let dir = unique_temp_dir("decrypt-ssh-key");
    let store = ConnectionStore::open(&dir).expect("store");
    let master_key = test_key(7);
    let master_key_token = encrypt_for_test(master_key.as_slice(), &home_wrapping_key());
    let key = SshKey {
        id: "key-1".to_string(),
        name: "Deploy Key".to_string(),
        key: Some(encrypt_for_test(b"-----BEGIN PRIVATE KEY-----", &master_key).into()),
        cert: Some(encrypt_for_test(b"ssh-ed25519-cert-v01@openssh.com AAAA", &master_key).into()),
        passphrase: Some(encrypt_for_test(b"passphrase", &master_key).into()),
        key_file_path: None,
        cert_file_path: None,
        has_key_data: false,
        has_cert_data: false,
    };
    {
        let txn = store.db.begin_write().expect("txn");
        txn.open_table(META_TABLE)
            .expect("meta")
            .insert(META_MASTER_KEY, master_key_token.as_str())
            .expect("insert master");
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(SSH_KEY_PREFIX, "key-1"),
            &key,
        )
        .expect("write key");
        txn.commit().expect("commit");
    }

    let listed = store.list_ssh_keys().expect("list keys");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].has_key_data);
    assert!(listed[0].has_cert_data);

    let decrypted = store
        .load_decrypted_ssh_key_by_id("key-1")
        .expect("decrypt key")
        .expect("key");
    assert_eq!(decrypted.name, "Deploy Key");
    assert_eq!(
        decrypted.key_data.as_deref(),
        Some("-----BEGIN PRIVATE KEY-----")
    );
    assert_eq!(
        decrypted.cert_data.as_deref(),
        Some("ssh-ed25519-cert-v01@openssh.com AAAA")
    );
    assert_eq!(decrypted.passphrase.as_deref(), Some("passphrase"));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_ssh_key_rejects_oversized_key_file_import() {
    let dir = unique_temp_dir("ssh-key-large-key-file");
    std::fs::create_dir_all(&dir).expect("dir");
    let key_path = dir.join("too-large.key");
    let file = std::fs::File::create(&key_path).expect("create key file");
    file.set_len(SSH_KEY_FILE_IMPORT_MAX_BYTES + 1)
        .expect("grow key file");

    let store = ConnectionStore::open(&dir).expect("store");
    let error = store
        .save_ssh_key(SshKey {
            id: "key-1".to_string(),
            name: "Large Key".to_string(),
            key: None,
            cert: None,
            passphrase: None,
            key_file_path: Some(key_path.display().to_string()),
            cert_file_path: None,
            has_key_data: false,
            has_cert_data: false,
        })
        .expect_err("large key import should fail");

    assert!(error.to_string().contains("key material file is too large"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_ssh_key_rejects_oversized_cert_file_import() {
    let dir = unique_temp_dir("ssh-key-large-cert-file");
    std::fs::create_dir_all(&dir).expect("dir");
    let cert_path = dir.join("too-large-cert.pub");
    let file = std::fs::File::create(&cert_path).expect("create cert file");
    file.set_len(SSH_KEY_FILE_IMPORT_MAX_BYTES + 1)
        .expect("grow cert file");

    let store = ConnectionStore::open(&dir).expect("store");
    let error = store
        .save_ssh_key(SshKey {
            id: "key-1".to_string(),
            name: "Large Cert".to_string(),
            key: Some("-----BEGIN PRIVATE KEY-----\nsmall\n".to_string().into()),
            cert: None,
            passphrase: None,
            key_file_path: None,
            cert_file_path: Some(cert_path.display().to_string()),
            has_key_data: false,
            has_cert_data: false,
        })
        .expect_err("large cert import should fail");

    assert!(error.to_string().contains("certificate file is too large"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn load_decrypted_otp_entry_reads_legacy_otp_store() {
    let dir = unique_temp_dir("decrypt-otp");
    let store = ConnectionStore::open(&dir).expect("store");
    let master_key = test_key(8);
    let master_key_token = encrypt_for_test(master_key.as_slice(), &home_wrapping_key());
    let entry = OtpEntry {
        id: "otp-1".to_string(),
        otp_type: "totp".to_string(),
        issuer: "Example".to_string(),
        username: "deploy".to_string(),
        secret: Some(encrypt_for_test(b"JBSWY3DPEHPK3PXP", &master_key).into()),
        algorithm: "SHA1".to_string(),
        digits: 6,
        period: 30,
        counter: 0,
        has_secret: false,
    };
    {
        let txn = store.db.begin_write().expect("txn");
        txn.open_table(META_TABLE)
            .expect("meta")
            .insert(META_MASTER_KEY, master_key_token.as_str())
            .expect("insert master");
        write_json_in_txn(
            &txn,
            OTP_ACCOUNTS_TABLE,
            &entity_key(OTP_PREFIX, "otp-1"),
            &entry,
        )
        .expect("write otp");
        txn.commit().expect("commit");
    }

    let listed = store.list_otp_entries().expect("list");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].has_secret);
    let decrypted = store
        .load_decrypted_otp_entry_by_id("otp-1")
        .expect("decrypt")
        .expect("entry");
    assert_eq!(decrypted.secret.as_deref(), Some("JBSWY3DPEHPK3PXP"));
    assert_eq!(decrypted.period, 30);
    store.increment_otp_counter("otp-1").expect("increment");
    let incremented = store
        .load_otp_entry_by_id("otp-1")
        .expect("load incremented")
        .expect("entry");
    assert_eq!(incremented.counter, 1);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn legacy_saved_credential_without_sort_order_defaults_to_zero() {
    let credential: SavedCredential = serde_json::from_value(serde_json::json!({
        "id": "legacy-credential",
        "name": "Legacy",
        "username": "deploy",
        "enabled": true
    }))
    .expect("legacy credential");

    assert_eq!(credential.sort_order, 0);
}

#[test]
fn credential_order_appends_and_reorders_without_exposing_passwords() {
    let dir = unique_temp_dir("credential-order");
    let store = ConnectionStore::open(&dir).expect("store");
    let save = |id: &str, name: &str, sort_order: i32| {
        store
            .save_credential(SavedCredential {
                id: id.to_string(),
                sort_order,
                name: name.to_string(),
                username: format!("{name}-user"),
                password: Some(format!("{name}-secret").into()),
                username_prompt_regex: None,
                password_prompt_regex: None,
                enabled: true,
                has_password: false,
            })
            .expect("save credential")
    };

    save("first", "First", 4);
    save("second", "Second", 8);
    let appended_id = save("", "Appended", 0);

    let listed = store.list_credentials().expect("list credentials");
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second", appended_id.as_str()]
    );
    assert_eq!(listed[2].sort_order, 9);
    assert!(listed.iter().all(|entry| entry.password.is_none()));
    assert!(listed.iter().all(|entry| entry.has_password));

    store
        .reorder_credentials(&[
            (appended_id.clone(), 0),
            ("first".to_string(), 1),
            ("second".to_string(), 2),
        ])
        .expect("reorder credentials");
    let reordered = store.list_credentials().expect("list reordered");
    assert_eq!(
        reordered
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec![appended_id.as_str(), "first", "second"]
    );
    let decrypted = store
        .load_decrypted_credential_by_id("first")
        .expect("decrypt credential")
        .expect("credential");
    assert_eq!(decrypted.password.as_deref(), Some("First-secret"));
    assert_eq!(decrypted.sort_order, 1);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn known_hosts_repository_preserves_structured_hashed_and_raw_lines() {
    let dir = unique_temp_dir("known-hosts");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .replace_known_hosts_export(
            "# comment\n@cert-authority *.example.com ssh-ed25519 AAAA ca\n|1|nNMSH1CuL4w6FneDFn3ONf5paeg=|q8MlMsHsBk6GOpNwYqhnCeXKlRk= ssh-rsa BBBB\n",
        )
        .expect("save known hosts");

    let rendered = store.render_known_hosts_export().expect("render");
    assert!(rendered.contains("# comment"));
    assert!(rendered.contains("@cert-authority *.example.com ssh-ed25519 AAAA ca"));
    assert!(
        rendered
            .contains("|1|nNMSH1CuL4w6FneDFn3ONf5paeg=|q8MlMsHsBk6GOpNwYqhnCeXKlRk= ssh-rsa BBBB")
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn known_hosts_check_distinguishes_match_changed_and_unknown() {
    let dir = unique_temp_dir("known-hosts-check");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .upsert_known_host("example.com ssh-ed25519 AAAA")
        .expect("known host");

    assert_eq!(
        store
            .check_known_host("example.com", "ssh-ed25519", "AAAA")
            .expect("match"),
        KnownHostCheck::Match
    );
    assert_eq!(
        store
            .check_known_host("example.com", "ssh-ed25519", "BBBB")
            .expect("changed"),
        KnownHostCheck::HostSeen
    );
    assert_eq!(
        store
            .check_known_host("other.example.com", "ssh-ed25519", "AAAA")
            .expect("unknown"),
        KnownHostCheck::UnknownHost
    );

    store
        .replace_known_host_for_host("example.com", "example.com ssh-ed25519 CCCC")
        .expect("replace");
    assert_eq!(
        store
            .check_known_host("example.com", "ssh-ed25519", "CCCC")
            .expect("replaced"),
        KnownHostCheck::Match
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn rdp_known_hosts_check_distinguishes_match_changed_and_unknown() {
    let dir = unique_temp_dir("rdp-known-hosts");
    let store = ConnectionStore::open(&dir).expect("store");

    assert_eq!(
        store
            .check_rdp_known_host("Windows.EXAMPLE.com", 3389, "sha256:a")
            .expect("unknown rdp host"),
        RdpKnownHostCheck::UnknownHost
    );

    store
        .upsert_rdp_known_host(
            "windows.example.com",
            3389,
            "sha256:a",
            super::RdpCertificateMetadata {
                subject: Some("CN=windows.example.com".to_string()),
                issuer: Some("CN=lab-ca".to_string()),
                valid_from: None,
                valid_to: None,
            },
        )
        .expect("save rdp host");

    assert_eq!(
        store
            .check_rdp_known_host("WINDOWS.example.com", 3389, "SHA256:A")
            .expect("matching rdp host"),
        RdpKnownHostCheck::Match
    );
    assert_eq!(
        store
            .check_rdp_known_host("windows.example.com", 3389, "sha256:b")
            .expect("changed rdp host"),
        RdpKnownHostCheck::Changed {
            remembered_fingerprint: "sha256:a".to_string(),
        }
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn imports_legacy_text_doc_known_hosts() {
    let dir = unique_temp_dir("legacy-known-hosts");
    std::fs::create_dir_all(&dir).expect("temp dir");
    {
        let db = Database::create(dir.join(DATABASE_FILE)).expect("db");
        let txn = db.begin_write().expect("txn");
        txn.open_table(TEXT_DOCS_TABLE)
            .expect("text docs")
            .insert(LEGACY_TEXT_KNOWN_HOSTS, "legacy.example.com ssh-rsa AAAA")
            .expect("insert");
        txn.commit().expect("commit");
    }

    let store = ConnectionStore::open(&dir).expect("store");
    assert_eq!(
        store
            .check_known_host("legacy.example.com", "ssh-rsa", "AAAA")
            .expect("legacy"),
        KnownHostCheck::Match
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn remote_file_backend_cache_round_trips_and_preserves_unknown_fields() {
    let dir = unique_temp_dir("remote-file-backend-cache");
    let store = ConnectionStore::open(&dir).expect("store");
    let legacy = serde_json::json!({
        "schema_hint": "keep-me",
        "entries": {
            "example.com:22:alice": {
                "last_working_backend": "scp_enhanced",
                "sftp_unavailable": true,
                "last_failure_reason": "subsystem unavailable",
                "updated_at": 10,
                "future_field": 42
            }
        }
    });
    {
        let txn = store.db.begin_write().expect("txn");
        txn.open_table(TEXT_DOCS_TABLE)
            .expect("text docs")
            .insert(
                LEGACY_TEXT_REMOTE_FILE_BACKEND_CACHE,
                serde_json::to_string(&legacy).expect("json").as_str(),
            )
            .expect("insert legacy cache");
        txn.commit().expect("commit");
    }

    let loaded = store.load_remote_file_backend_cache().expect("load legacy");
    assert_eq!(
        loaded
            .entries
            .get("example.com:22:alice")
            .expect("entry")
            .extra
            .get("future_field"),
        Some(&serde_json::json!(42))
    );
    store
        .update_remote_file_backend_cache_entry("example.com:22:alice", "sftp", false, None)
        .expect("update cache");
    let updated = store.load_remote_file_backend_cache().expect("reload");
    assert_eq!(
        updated.extra.get("schema_hint"),
        Some(&serde_json::json!("keep-me"))
    );
    let entry = updated
        .entries
        .get("example.com:22:alice")
        .expect("updated entry");
    assert_eq!(entry.last_working_backend, "sftp");
    assert_eq!(
        entry.extra.get("future_field"),
        Some(&serde_json::json!(42))
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn corrupt_remote_file_backend_cache_is_not_overwritten() {
    let dir = unique_temp_dir("corrupt-remote-file-backend-cache");
    let store = ConnectionStore::open(&dir).expect("store");
    {
        let txn = store.db.begin_write().expect("txn");
        txn.open_table(SETTINGS_TABLE)
            .expect("settings")
            .insert(
                super::SETTINGS_REMOTE_FILE_BACKEND_CACHE,
                b"not-json".as_slice(),
            )
            .expect("insert corrupt cache");
        txn.commit().expect("commit");
    }
    assert!(store.load_remote_file_backend_cache().is_err());
    assert!(
        store
            .update_remote_file_backend_cache_entry("host:22:user", "sftp", false, None)
            .is_err()
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_appearance_theme_and_contrast_roundtrip() {
    let dir = unique_temp_dir("settings-appearance-extra");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");
    summary.theme = "dracula".to_string();
    summary.terminal_theme = Some("nord".to_string());
    summary.minimum_contrast_ratio = "4.5".to_string();
    summary.background_image_opacity = 0;
    summary.background_content_opacity = 0;
    summary.ui_font_family = "Segoe UI, Inter".to_string();
    summary.terminal_font_family = "JetBrains Mono, monospace".to_string();
    summary.ui_font_size = 18;
    summary.terminal_font_weight = 500;
    summary.terminal_font_weight_bold = 800;
    let saved = store.save_appearance_settings(&summary).expect("save");
    assert_eq!(saved.theme, "dracula");
    assert_eq!(saved.terminal_theme.as_deref(), Some("nord"));
    assert_eq!(saved.minimum_contrast_ratio, "4.5");
    assert_eq!(saved.background_image_opacity, 0);
    assert_eq!(saved.background_content_opacity, 0);
    assert_eq!(saved.ui_font_family, "Segoe UI, Inter");
    assert_eq!(saved.terminal_font_family, "JetBrains Mono, monospace");
    assert_eq!(saved.ui_font_size, 18);
    assert_eq!(saved.terminal_font_weight, 500);
    assert_eq!(saved.terminal_font_weight_bold, 800);
    let raw = store.load_settings_value().expect("raw");
    assert_eq!(
        raw["appearance"]["terminal_theme"],
        serde_json::Value::String("nord".into())
    );
    assert_eq!(
        raw["appearance"]["minimum_contrast_ratio"],
        serde_json::json!(4.5)
    );
    assert_eq!(
        raw["appearance"]["ui_font_family"],
        serde_json::Value::String("Segoe UI, Inter".into())
    );
    assert_eq!(
        raw["appearance"]["font_family"],
        serde_json::Value::String("JetBrains Mono, monospace".into())
    );
    assert_eq!(
        raw["appearance"]["background_image_opacity"],
        serde_json::json!(0.0)
    );
    assert_eq!(
        raw["appearance"]["background_opacity"],
        serde_json::json!(0.0)
    );
    assert_eq!(raw["appearance"]["ui_font_size"], serde_json::json!(18));
    assert_eq!(raw["appearance"]["font_weight"], serde_json::json!(500));
    assert_eq!(
        raw["appearance"]["font_weight_bold"],
        serde_json::json!(800)
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_empty_search_engine_list_roundtrip() {
    let dir = unique_temp_dir("settings-empty-search-engines");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");
    assert!(!summary.search_custom_engines.is_empty());

    summary.search_custom_engines.clear();
    let saved = store.save_terminal_settings(&summary).expect("save");
    assert!(saved.search_custom_engines.is_empty());

    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["search"]["custom_engines"], serde_json::json!([]));

    summary.search_custom_engines = vec![SearchEngineConfig {
        name: String::new(),
        url_template: String::new(),
        icon: None,
        show_in_menu: true,
    }];
    let saved = store
        .save_terminal_settings(&summary)
        .expect("save blank engine");
    assert_eq!(saved.search_custom_engines.len(), 1);
    assert!(saved.search_custom_engines[0].name.is_empty());
    assert!(saved.search_custom_engines[0].url_template.is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_general_and_diagnostics_settings_roundtrip() {
    let dir = unique_temp_dir("settings-general-diag");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");
    summary.startup_restore = true;
    summary.startup_restore_window_layout = false;
    summary.minimize_to_tray = true;
    summary.confirm_on_close = false;
    summary.language = "zh-CN".to_string();
    let saved = store.save_general_settings(&summary).expect("save general");
    assert!(saved.startup_restore);
    assert!(!saved.startup_restore_window_layout);
    assert!(saved.minimize_to_tray);
    assert!(!saved.confirm_on_close);
    assert_eq!(saved.language, "zh-CN");

    summary = saved;
    summary.diagnostics_level = "debug".to_string();
    summary.diagnostics_retention_days = 14;
    let saved = store
        .save_diagnostics_settings(&summary)
        .expect("save diagnostics");
    assert_eq!(saved.diagnostics_level, "debug");
    assert_eq!(saved.diagnostics_retention_days, 14);

    let raw = store.load_settings_value().expect("raw");
    assert_eq!(
        raw["general"]["minimize_to_tray"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        raw["ui"]["language"],
        serde_json::Value::String("zh-CN".into())
    );
    assert_eq!(
        raw["diagnostics"]["level"],
        serde_json::Value::String("debug".into())
    );
    assert_eq!(
        raw["diagnostics"]["retention_days"],
        serde_json::Value::from(14)
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_ui_layout_bottom_panel_state_roundtrip_and_clamp() {
    let dir = unique_temp_dir("settings-bottom-panel-heights");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");
    summary.ui_quick_cmd_height = 312;
    summary.ui_quick_cmd_visible = false;
    summary.ui_serial_send_height = 284;
    summary.ui_serial_send_visible = true;

    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert_eq!(saved.ui_quick_cmd_height, 312);
    assert!(!saved.ui_quick_cmd_visible);
    assert_eq!(saved.ui_serial_send_height, 284);
    assert!(saved.ui_serial_send_visible);
    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["ui"]["quick_cmd_height"], serde_json::json!(312));
    assert_eq!(raw["ui"]["show_quick_cmd_bar"], serde_json::json!(false));
    assert_eq!(raw["ui"]["serial_send_height"], serde_json::json!(284));
    assert_eq!(raw["ui"]["show_serial_send_panel"], serde_json::json!(true));

    summary.ui_quick_cmd_height = 0;
    summary.ui_serial_send_height = 999;
    let clamped = store
        .save_ui_layout_settings(&summary)
        .expect("save clamped");
    assert_eq!(clamped.ui_quick_cmd_height, 36);
    assert_eq!(clamped.ui_serial_send_height, 520);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_ui_layout_preserves_explicit_empty_activity_zone() {
    let dir = unique_temp_dir("settings-empty-activity-zone");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");
    let moved = std::mem::take(&mut summary.ui_activity_bar_left_top);
    summary.ui_activity_bar_right_top.extend(moved);
    summary.ui_saved_connections_sort_mode = "name-desc".to_string();

    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert!(saved.ui_activity_bar_left_top.is_empty());
    assert_eq!(saved.ui_saved_connections_sort_mode, "name-desc");
    assert!(
        saved
            .ui_activity_bar_right_top
            .iter()
            .any(|id| id == "fileExplorer")
    );
    let raw = store.load_settings_value().expect("raw");
    assert_eq!(
        raw["ui"]["activity_bar_layout"]["left_top"],
        serde_json::json!([])
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn activity_bar_hidden_items_and_panel_open_mode_roundtrip() {
    let dir = unique_temp_dir("settings-hidden-items-panel-mode");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");
    summary.ui_activity_bar_hidden_items =
        vec!["gpuMonitor".to_string(), "dockerManager".to_string()];
    summary.ui_panel_open_mode = "floating".to_string();
    summary.ui_panel_multi_open = true;

    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert_eq!(
        saved.ui_activity_bar_hidden_items,
        vec!["gpuMonitor".to_string(), "dockerManager".to_string()]
    );
    assert_eq!(saved.ui_panel_open_mode, "floating");
    assert!(saved.ui_panel_multi_open);

    let raw = store.load_settings_value().expect("raw");
    assert_eq!(
        raw["ui"]["activity_bar_layout"]["hidden_items"],
        serde_json::json!(["gpuMonitor", "dockerManager"])
    );
    assert_eq!(raw["ui"]["panel_open_mode"], "floating");
    assert_eq!(raw["ui"]["panel_multi_open"], true);
    assert_eq!(raw["appearance"]["panel_multi_open"], true);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn legacy_settings_default_hidden_items_and_docked_panel_mode() {
    let dir = unique_temp_dir("settings-hidden-items-legacy-defaults");
    let store = ConnectionStore::open(&dir).expect("store");

    let summary = store.load_app_settings_summary().expect("load");
    assert!(summary.ui_activity_bar_hidden_items.is_empty());
    assert_eq!(summary.ui_panel_open_mode, "docked");
    assert!(!summary.ui_panel_multi_open);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn legacy_panel_multi_open_boolean_stays_independent_from_docked_mode() {
    let dir = unique_temp_dir("settings-legacy-panel-multi-open");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_value(&serde_json::json!({
            "ui": {
                "panel_multi_open": true,
                "future_ui_option": { "keep": true }
            }
        }))
        .expect("seed legacy settings");

    let summary = store.load_app_settings_summary().expect("load");
    assert_eq!(summary.ui_panel_open_mode, "docked");
    assert!(summary.ui_panel_multi_open);

    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert_eq!(saved.ui_panel_open_mode, "docked");
    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["ui"]["panel_open_mode"], "docked");
    assert_eq!(raw["ui"]["future_ui_option"]["keep"], true);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn panel_open_mode_normalizes_unknown_without_changing_multi_open() {
    let dir = unique_temp_dir("settings-panel-mode-normalize");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_value(&serde_json::json!({
            "appearance": { "panel_multi_open": true },
            "ui": { "panel_open_mode": "weird" }
        }))
        .expect("seed settings");

    let summary = store.load_app_settings_summary().expect("load");
    assert_eq!(summary.ui_panel_open_mode, "docked");
    assert!(summary.ui_panel_multi_open);

    let mut summary = summary;
    summary.ui_panel_open_mode = "FLOATING".to_string();
    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert_eq!(saved.ui_panel_open_mode, "floating");
    assert!(saved.ui_panel_multi_open);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn legacy_settings_default_header_status_to_visible_session() {
    let dir = unique_temp_dir("settings-header-status-legacy-defaults");
    let store = ConnectionStore::open(&dir).expect("store");

    let summary = store.load_app_settings_summary().expect("load");
    assert_eq!(summary.ui_header_status_mode, "session");
    assert!(summary.ui_header_status_visible);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn header_status_settings_roundtrip_and_preserve_unknown_ui_fields() {
    let dir = unique_temp_dir("settings-header-status-roundtrip");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_value(&serde_json::json!({
            "ui": {
                "header_status_mode": "host",
                "header_status_visible": false,
                "future_header_option": { "keep": true }
            }
        }))
        .expect("seed settings");

    let mut summary = store.load_app_settings_summary().expect("load");
    assert_eq!(summary.ui_header_status_mode, "host");
    assert!(!summary.ui_header_status_visible);

    summary.ui_header_status_mode = "resources".to_string();
    summary.ui_header_status_visible = true;
    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert_eq!(saved.ui_header_status_mode, "resources");
    assert!(saved.ui_header_status_visible);

    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["ui"]["header_status_mode"], "resources");
    assert_eq!(raw["ui"]["header_status_visible"], true);
    assert_eq!(raw["ui"]["future_header_option"]["keep"], true);

    summary.ui_header_status_mode = "unsupported".to_string();
    let normalized = store
        .save_ui_layout_settings(&summary)
        .expect("save normalized");
    assert_eq!(normalized.ui_header_status_mode, "session");

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn app_settings_summary_reads_and_updates_host_key_policy() {
    let dir = unique_temp_dir("settings-summary");
    let store = ConnectionStore::open(&dir).expect("store");
    let initial = serde_json::json!({
        "general": {
            "startup_restore": true,
            "minimize_to_tray": true,
            "confirm_on_close": false,
            "custom_general": "keep"
        },
        "appearance": {
            "theme": "catppuccin",
            "font_family": "Iosevka",
            "font_size": 14.0
        },
        "translation": {
            "target_language": "ja"
        },
        "security": {
            "host_key_policy": "strict",
            "enable_screen_lock": true,
            "idle_lock_minutes": 12,
            "master_password": "encrypted"
        },
        "transfer": {
            "download_path": "/tmp/downloads",
            "ask_save_location": true,
            "duplicate_strategy": "rename",
            "editor_type": "internal",
            "internal_editor_font_size": 17,
            "default_editor": "code",
            "download_threads": 5,
            "upload_threads": 4,
            "max_transfer_retries": 6,
            "transfer_buffer_size": 64,
            "default_file_permissions": "664",
            "preserve_timestamps": false,
            "resume_broken_transfer": false
        },
        "recording": {
            "base_path": "/tmp/nyaterm-recordings",
            "auto_start": true,
            "default_mode": "raw",
            "path_template": "{session}/{yyyy}-{MM}-{dd}.raw.log",
            "include_session_metadata": false,
            "rotation": {
                "type": "size",
                "max_bytes": 2097152
            },
            "existing_file_behavior": "append",
            "include_binary_transfer_payloads": true,
            "include_io_labels": false,
            "include_timestamps": false,
            "memory_limit_bytes": 1048576
        },
        "terminal": {
            "x11_display": "localhost:1",
            "scrollback_lines": 8000,
            "keep_alive_mode": "strict",
            "keep_alive_interval": 45,
            "timestamp_format": "YYYY-MM-DD HH:mm:ss",
            "hardware_acceleration": false,
            "show_workspace_padding": true,
            "show_line_numbers": true,
            "show_timestamps": true,
            "show_timestamp_milliseconds": true,
            "show_multi_line_paste_dialog": false,
            "paste_image_as_path": false,
            "reconnect_restore_cwd": true,
            "low_latency_mode": true
        },
        "ui": {
            "language": "zh-CN",
            "show_remote_stats": false,
            "remote_stats_interval": 9,
            "show_gpu_monitor": true,
            "gpu_monitor_interval": 7,
            "show_ascend_npu_monitor": true,
            "ascend_npu_monitor_interval": 8,
            "show_process_manager": false,
            "process_manager_interval": 11,
            "show_docker_manager": false,
            "docker_manager_interval": 13,
            "quick_cmd_view_mode": "compact",
            "quick_cmd_sort_mode": "useCount",
            "file_explorer_show_hidden_files": false,
            "file_explorer_auto_sync_cwd_connection_ids": ["conn-1", "conn-1", " ", "conn-2"],
            "file_explorer_favorite_dirs_by_connection_id": {
                "conn-1": ["/var", "/var", " ", "/opt", "/srv", "/tmp", "/home", "/etc", "/usr", "/bin", "/sbin", "/lib", "/mnt"],
                "conn-empty": [],
                "conn-invalid": false
            }
        },
        "interaction": {
            "copy_on_select": true,
            "right_click_paste": true,
            "command_suggestions_enabled": false,
            "command_suggestion_min_chars": 3,
            "command_suggestion_max_chars": 80,
            "word_separators": " .,:",
            "duplicate_session_command_delay_ms": 1500,
            "allow_osc52_clipboard_write": true,
            "terminal_zoom_enabled": false,
            "alt_as_meta": true,
            "mac_ime_compatibility": false,
            "tab_double_click_action": "duplicate_session",
            "tab_middle_click_action": "close_tab",
            "tab_right_click_action": "copy_tab_name",
            "default_encoding": "GBK"
        },
        "diagnostics": {
            "level": "debug",
            "retention_days": 3
        },
        "keybindings": {
            "terminal.find": "ctrl+f",
            "ignored_non_string": false
        },
        "unrelated": {
            "preserve": true
        }
    });
    store.save_settings_value(&initial).expect("seed settings");

    let summary = store.load_app_settings_summary().expect("summary");
    assert_eq!(summary.theme, "catppuccin");
    assert_eq!(summary.language, "zh-CN");
    assert_eq!(summary.terminal_font_family, "Iosevka");
    assert_eq!(summary.terminal_font_size, 14);
    assert_eq!(summary.x11_display, "localhost:1");
    assert_eq!(summary.terminal_scrollback_lines, 8000);
    assert_eq!(summary.terminal_keep_alive_mode, "strict");
    assert_eq!(summary.terminal_keep_alive_interval, 45);
    assert_eq!(summary.terminal_timestamp_format, "YYYY-MM-DD HH:mm:ss");
    assert!(!summary.terminal_hardware_acceleration);
    assert!(summary.terminal_show_workspace_padding);
    assert!(summary.terminal_show_line_numbers);
    assert!(summary.terminal_show_timestamps);
    assert!(!summary.terminal_show_multi_line_paste_dialog);
    assert!(!summary.terminal_paste_image_as_path);
    assert!(summary.terminal_reconnect_restore_cwd);
    assert!(summary.terminal_low_latency_mode);
    // Legacy settings omit the new field and must keep zebra stripes enabled.
    assert!(summary.terminal_zebra_stripes_enabled);
    assert!(!summary.ui_show_remote_stats);
    assert_eq!(summary.ui_remote_stats_interval, 9);
    assert!(summary.ui_show_gpu_monitor);
    assert_eq!(summary.ui_gpu_monitor_interval, 7);
    assert!(summary.ui_show_ascend_npu_monitor);
    assert_eq!(summary.ui_ascend_npu_monitor_interval, 8);
    assert!(!summary.ui_show_process_manager);
    assert_eq!(summary.ui_process_manager_interval, 11);
    assert!(!summary.ui_show_docker_manager);
    assert_eq!(summary.ui_docker_manager_interval, 13);
    assert_eq!(summary.ui_quick_cmd_view_mode, "compact");
    assert_eq!(summary.ui_quick_cmd_sort_mode, "useCount");
    assert!(!summary.ui_file_explorer_show_hidden_files);
    assert_eq!(
        summary.ui_file_explorer_auto_sync_cwd_connection_ids,
        vec!["conn-1".to_string(), "conn-2".to_string()]
    );
    assert_eq!(
        summary
            .ui_file_explorer_favorite_dirs_by_connection_id
            .get("conn-1"),
        Some(&vec![
            "/var".to_string(),
            "/opt".to_string(),
            "/srv".to_string(),
            "/tmp".to_string(),
            "/home".to_string(),
            "/etc".to_string(),
            "/usr".to_string(),
            "/bin".to_string(),
            "/sbin".to_string(),
            "/lib".to_string(),
            "/mnt".to_string(),
        ])
    );
    assert!(
        !summary
            .ui_file_explorer_favorite_dirs_by_connection_id
            .contains_key("conn-empty")
    );
    assert!(summary.interaction_copy_on_select);
    assert!(summary.interaction_right_click_paste);
    assert!(summary.interaction_allow_osc52_clipboard_write);
    assert!(!summary.interaction_terminal_zoom_enabled);
    assert!(!summary.interaction_command_suggestions_enabled);
    assert_eq!(summary.interaction_command_suggestion_min_chars, 3);
    assert_eq!(summary.interaction_command_suggestion_max_chars, 80);
    assert_eq!(summary.interaction_word_separators, " .,:");
    assert_eq!(summary.interaction_duplicate_session_command_delay_ms, 1500);
    assert!(summary.interaction_alt_as_meta);
    assert!(!summary.interaction_mac_ime_compatibility);
    assert_eq!(
        summary.interaction_tab_double_click_action,
        "duplicate_session"
    );
    assert_eq!(summary.interaction_tab_middle_click_action, "close_tab");
    assert_eq!(summary.interaction_tab_right_click_action, "copy_tab_name");
    assert_eq!(summary.interaction_default_encoding, "GBK");
    assert_eq!(summary.host_key_policy, "strict");
    assert_eq!(summary.transfer_download_path, "/tmp/downloads");
    assert!(summary.transfer_ask_save_location);
    assert_eq!(summary.transfer_duplicate_strategy, "rename");
    assert_eq!(summary.transfer_editor_type, "internal");
    assert_eq!(summary.transfer_internal_editor_font_size, 17);
    assert_eq!(summary.transfer_default_editor, "code");
    assert_eq!(summary.transfer_download_threads, 5);
    assert_eq!(summary.transfer_upload_threads, 4);
    assert_eq!(summary.transfer_max_retries, 6);
    assert_eq!(summary.transfer_buffer_size, 64);
    assert_eq!(summary.transfer_default_file_permissions, "664");
    assert!(!summary.transfer_preserve_timestamps);
    assert!(!summary.transfer_resume_broken_transfer);
    assert_eq!(summary.recording_path, "/tmp/nyaterm-recordings");
    assert!(summary.recording_auto_start);
    assert_eq!(summary.recording_default_mode, RecordingMode::Raw);
    assert_eq!(
        summary.recording_path_template,
        "{session}/{yyyy}-{MM}-{dd}.raw.log"
    );
    assert!(!summary.recording_include_session_metadata);
    assert_eq!(
        summary.recording_rotation,
        RecordingRotationPolicy::Size { max_bytes: 2097152 }
    );
    assert_eq!(
        summary.recording_existing_file_behavior,
        ExistingFileBehavior::Append
    );
    assert!(summary.recording_include_binary_transfer_payloads);
    assert!(!summary.recording_include_io_labels);
    assert!(!summary.recording_include_timestamps);
    assert_eq!(summary.recording_memory_limit_bytes, 1048576);
    assert_eq!(summary.diagnostics_level, "debug");
    assert_eq!(summary.diagnostics_retention_days, 3);
    assert!(summary.startup_restore);
    assert!(summary.minimize_to_tray);
    assert!(!summary.confirm_on_close);
    assert!(summary.enable_screen_lock);
    assert_eq!(summary.idle_lock_minutes, 12);
    assert!(summary.has_master_password);
    assert_eq!(
        summary.keybindings.get("terminal.find").map(String::as_str),
        Some("ctrl+f")
    );
    assert!(!summary.keybindings.contains_key("ignored_non_string"));

    let updated = store.save_host_key_policy("accept").expect("save policy");
    assert_eq!(updated.host_key_policy, "accept");
    let stored = store.load_settings_value().expect("stored settings");
    assert_eq!(
        json_path(&stored, &["general", "custom_general"]).and_then(|value| value.as_str()),
        Some("keep")
    );
    assert_eq!(
        json_path(&stored, &["unrelated", "preserve"]).and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        json_path(&stored, &["security", "host_key_policy"]).and_then(|value| value.as_str()),
        Some("accept")
    );
    assert_eq!(
        json_path(&stored, &["keybindings", "terminal.find"]).and_then(|value| value.as_str()),
        Some("ctrl+f")
    );

    let mut terminal_update = summary.clone();
    terminal_update.terminal_show_line_numbers = false;
    terminal_update.terminal_reconnect_restore_cwd = false;
    store
        .save_terminal_settings(&terminal_update)
        .expect("save terminal settings");
    let stored = store
        .load_settings_value()
        .expect("stored terminal settings");
    assert_eq!(
        json_path(&stored, &["terminal", "show_timestamp_milliseconds"])
            .and_then(|value| value.as_bool()),
        Some(true),
        "the retired legacy key must remain byte-semantically untouched"
    );
    assert_eq!(
        json_path(&stored, &["terminal", "reconnect_restore_cwd"])
            .and_then(|value| value.as_bool()),
        Some(false)
    );

    let mut transfer_update = summary.clone();
    transfer_update.transfer_download_path = "/var/tmp/downloads".to_string();
    transfer_update.transfer_ask_save_location = false;
    transfer_update.transfer_duplicate_strategy = "overwrite".to_string();
    transfer_update.transfer_editor_type = "external".to_string();
    transfer_update.transfer_internal_editor_font_size = 19;
    transfer_update.transfer_default_editor = "gedit".to_string();
    transfer_update.transfer_download_threads = 2;
    transfer_update.transfer_upload_threads = 6;
    transfer_update.transfer_max_retries = 3;
    transfer_update.transfer_buffer_size = 128;
    transfer_update.transfer_default_file_permissions = "755".to_string();
    transfer_update.transfer_preserve_timestamps = true;
    transfer_update.transfer_resume_broken_transfer = true;
    let updated = store
        .save_transfer_settings(&transfer_update)
        .expect("save transfer settings");
    assert_eq!(updated.transfer_download_path, "/var/tmp/downloads");
    assert!(!updated.transfer_ask_save_location);
    assert_eq!(updated.transfer_duplicate_strategy, "overwrite");
    assert_eq!(updated.transfer_editor_type, "external");
    assert_eq!(updated.transfer_internal_editor_font_size, 19);
    assert_eq!(updated.transfer_default_editor, "gedit");
    assert_eq!(updated.transfer_download_threads, 2);
    assert_eq!(updated.transfer_upload_threads, 6);
    assert_eq!(updated.transfer_max_retries, 3);
    assert_eq!(updated.transfer_buffer_size, 128);
    assert_eq!(updated.transfer_default_file_permissions, "755");
    assert!(updated.transfer_preserve_timestamps);
    assert!(updated.transfer_resume_broken_transfer);
    let stored = store
        .load_settings_value()
        .expect("stored transfer settings");
    assert_eq!(
        json_path(&stored, &["transfer", "download_path"]).and_then(|value| value.as_str()),
        Some("/var/tmp/downloads")
    );
    assert_eq!(
        json_path(&stored, &["transfer", "ask_save_location"]).and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "duplicate_strategy"]).and_then(|value| value.as_str()),
        Some("overwrite")
    );
    assert_eq!(
        json_path(&stored, &["transfer", "editor_type"]).and_then(|value| value.as_str()),
        Some("external")
    );
    assert_eq!(
        json_path(&stored, &["transfer", "internal_editor_font_size"])
            .and_then(|value| value.as_u64()),
        Some(19)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "default_editor"]).and_then(|value| value.as_str()),
        Some("gedit")
    );
    assert_eq!(
        json_path(&stored, &["transfer", "download_threads"]).and_then(|value| value.as_u64()),
        Some(2)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "upload_threads"]).and_then(|value| value.as_u64()),
        Some(6)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "max_transfer_retries"]).and_then(|value| value.as_u64()),
        Some(3)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "transfer_buffer_size"]).and_then(|value| value.as_u64()),
        Some(128)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "default_file_permissions"])
            .and_then(|value| value.as_str()),
        Some("755")
    );
    assert_eq!(
        json_path(&stored, &["transfer", "preserve_timestamps"]).and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        json_path(&stored, &["transfer", "resume_broken_transfer"])
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        json_path(&stored, &["unrelated", "preserve"]).and_then(|value| value.as_bool()),
        Some(true)
    );

    let mut favorite_update = summary.clone();
    favorite_update.ui_file_explorer_show_hidden_files = true;
    favorite_update.ui_file_explorer_auto_sync_cwd_connection_ids = vec![
        "conn-3".to_string(),
        "conn-3".to_string(),
        " ".to_string(),
        "conn-1".to_string(),
    ];
    favorite_update
        .ui_file_explorer_favorite_dirs_by_connection_id
        .insert(
            "conn-2".to_string(),
            vec![
                "/data".to_string(),
                "/data".to_string(),
                " ".to_string(),
                "/logs".to_string(),
            ],
        );
    let updated = store
        .save_file_explorer_favorite_dirs(&favorite_update)
        .expect("save favorites");
    assert_eq!(
        updated.ui_file_explorer_auto_sync_cwd_connection_ids,
        vec!["conn-3".to_string(), "conn-1".to_string()]
    );
    assert!(updated.ui_file_explorer_show_hidden_files);
    assert_eq!(
        updated
            .ui_file_explorer_favorite_dirs_by_connection_id
            .get("conn-2"),
        Some(&vec!["/data".to_string(), "/logs".to_string()])
    );
    let stored = store.load_settings_value().expect("stored favorites");
    assert_eq!(
        json_path(&stored, &["ui", "file_explorer_show_hidden_files"])
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        json_path(
            &stored,
            &["ui", "file_explorer_auto_sync_cwd_connection_ids"]
        )
        .and_then(|value| value.as_array())
        .map(|values| values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()),
        Some(vec!["conn-3", "conn-1"])
    );
    assert_eq!(
        json_path(
            &stored,
            &[
                "ui",
                "file_explorer_favorite_dirs_by_connection_id",
                "conn-2"
            ]
        )
        .and_then(|value| value.as_array())
        .map(|values| values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()),
        Some(vec!["/data", "/logs"])
    );

    let mut next_keybindings = summary.keybindings.clone();
    next_keybindings.insert("view.openSettings".to_string(), "ctrl+.".to_string());
    next_keybindings.insert(
        "terminal.copy".to_string(),
        "ctrl+shift+c,meta+shift+c".to_string(),
    );
    next_keybindings.insert("plugin.futureAction".to_string(), " Win+Q ".to_string());
    next_keybindings.insert("blank".to_string(), " ".to_string());
    let updated = store
        .save_keybindings(&next_keybindings)
        .expect("save keybindings");
    assert_eq!(
        updated
            .keybindings
            .get("view.openSettings")
            .map(String::as_str),
        Some("ctrl+.")
    );
    assert_eq!(
        updated.keybindings.get("blank").map(String::as_str),
        Some(" ")
    );
    assert_eq!(
        updated.keybindings.get("terminal.copy").map(String::as_str),
        Some("ctrl+shift+c,meta+shift+c")
    );
    assert_eq!(
        updated
            .keybindings
            .get("plugin.futureAction")
            .map(String::as_str),
        Some(" Win+Q ")
    );

    let mut terminal_update = updated.clone();
    terminal_update.terminal_scrollback_lines = 12_000;
    terminal_update.terminal_keep_alive_mode = "disabled".to_string();
    terminal_update.terminal_keep_alive_interval = 20;
    terminal_update.terminal_timestamp_format = "[HH:mm:ss.SSS]".to_string();
    terminal_update.terminal_show_multi_line_paste_dialog = true;
    terminal_update.terminal_low_latency_mode = true;
    terminal_update.terminal_zebra_stripes_enabled = false;
    terminal_update.ui_show_remote_stats = true;
    terminal_update.ui_remote_stats_interval = 4;
    terminal_update.ui_show_gpu_monitor = true;
    terminal_update.ui_gpu_monitor_interval = 10;
    terminal_update.ui_show_ascend_npu_monitor = true;
    terminal_update.ui_ascend_npu_monitor_interval = 12;
    let saved_terminal = store
        .save_terminal_settings(&terminal_update)
        .expect("save terminal settings");
    assert_eq!(saved_terminal.terminal_scrollback_lines, 12_000);
    assert_eq!(saved_terminal.terminal_keep_alive_mode, "disabled");
    assert_eq!(saved_terminal.terminal_keep_alive_interval, 20);
    assert_eq!(saved_terminal.terminal_timestamp_format, "[HH:mm:ss.SSS]");
    assert!(saved_terminal.terminal_show_multi_line_paste_dialog);
    assert!(saved_terminal.terminal_low_latency_mode);
    assert!(!saved_terminal.terminal_zebra_stripes_enabled);
    assert!(saved_terminal.ui_show_remote_stats);
    assert_eq!(saved_terminal.ui_remote_stats_interval, 4);
    assert!(saved_terminal.ui_show_gpu_monitor);
    assert_eq!(saved_terminal.ui_gpu_monitor_interval, 10);
    assert!(saved_terminal.ui_show_ascend_npu_monitor);
    assert_eq!(saved_terminal.ui_ascend_npu_monitor_interval, 12);
    let stored = store
        .load_settings_value()
        .expect("stored terminal settings");
    assert_eq!(
        json_path(&stored, &["terminal", "scrollback_lines"]).and_then(|value| value.as_u64()),
        Some(12_000)
    );
    assert_eq!(
        json_path(&stored, &["ui", "remote_stats_interval"]).and_then(|value| value.as_u64()),
        Some(4)
    );
    assert_eq!(
        json_path(&stored, &["terminal", "keep_alive_mode"]).and_then(|value| value.as_str()),
        Some("disabled")
    );
    assert_eq!(
        json_path(&stored, &["terminal", "timestamp_format"]).and_then(|value| value.as_str()),
        Some("[HH:mm:ss.SSS]")
    );
    assert_eq!(
        json_path(&stored, &["ui", "gpu_monitor_interval"]).and_then(|value| value.as_u64()),
        Some(10)
    );
    assert_eq!(
        json_path(&stored, &["terminal", "low_latency_mode"]).and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        json_path(&stored, &["terminal", "zebra_stripes_enabled"])
            .and_then(|value| value.as_bool()),
        Some(false)
    );

    let mut quick_command_update = saved_terminal.clone();
    quick_command_update.ui_quick_cmd_view_mode = "list".to_string();
    quick_command_update.ui_quick_cmd_sort_mode = "name".to_string();
    let saved_quick_command_ui = store
        .save_quick_command_ui_settings(&quick_command_update)
        .expect("save quick command ui settings");
    assert_eq!(saved_quick_command_ui.ui_quick_cmd_view_mode, "list");
    assert_eq!(saved_quick_command_ui.ui_quick_cmd_sort_mode, "name");
    let stored = store
        .load_settings_value()
        .expect("stored quick command ui settings");
    assert_eq!(
        json_path(&stored, &["ui", "quick_cmd_view_mode"]).and_then(|value| value.as_str()),
        Some("list")
    );
    assert_eq!(
        json_path(&stored, &["ui", "quick_cmd_sort_mode"]).and_then(|value| value.as_str()),
        Some("name")
    );

    let mut ui_layout_update = saved_quick_command_ui.clone();
    ui_layout_update.ui_saved_connections_expanded_group_ids =
        vec!["group-a".to_string(), "group-b".to_string()];
    let saved_ui_layout = store
        .save_ui_layout_settings(&ui_layout_update)
        .expect("save saved connection group expansion");
    assert_eq!(
        saved_ui_layout.ui_saved_connections_expanded_group_ids,
        vec!["group-a".to_string(), "group-b".to_string()]
    );
    let stored = store
        .load_settings_value()
        .expect("stored saved connection group expansion");
    assert_eq!(
        json_path(&stored, &["ui", "saved_connections_expanded_group_ids"])
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(2)
    );

    let mut interaction_update = saved_ui_layout.clone();
    interaction_update.interaction_right_click_paste = false;
    interaction_update.interaction_command_suggestion_min_chars = 4;
    interaction_update.interaction_command_suggestion_max_chars = 120;
    interaction_update.interaction_duplicate_session_command_delay_ms = 2_500;
    interaction_update.interaction_allow_osc52_clipboard_write = false;
    interaction_update.interaction_terminal_zoom_enabled = true;
    interaction_update.interaction_alt_as_meta = false;
    interaction_update.interaction_tab_double_click_action = "reconnect_session".to_string();
    interaction_update.interaction_default_encoding = "utf-8".to_string();
    let saved_interaction = store
        .save_interaction_settings(&interaction_update)
        .expect("save interaction settings");
    assert!(!saved_interaction.interaction_right_click_paste);
    assert_eq!(
        saved_interaction.interaction_command_suggestion_min_chars,
        4
    );
    assert_eq!(
        saved_interaction.interaction_command_suggestion_max_chars,
        120
    );
    assert_eq!(
        saved_interaction.interaction_duplicate_session_command_delay_ms,
        2_500
    );
    assert!(!saved_interaction.interaction_allow_osc52_clipboard_write);
    assert!(saved_interaction.interaction_terminal_zoom_enabled);
    assert!(!saved_interaction.interaction_alt_as_meta);
    assert_eq!(
        saved_interaction.interaction_tab_double_click_action,
        "reconnect_session"
    );
    assert_eq!(saved_interaction.interaction_default_encoding, "UTF-8");

    let normalized = store.save_host_key_policy("wild").expect("normalize");
    assert_eq!(normalized.host_key_policy, "prompt");

    let mut recording_update = normalized.clone();
    recording_update.recording_path = "/var/log/nyaterm".to_string();
    recording_update.recording_auto_start = false;
    recording_update.recording_default_mode = RecordingMode::Raw;
    recording_update.recording_path_template = "{host}/{session}.raw.log".to_string();
    recording_update.recording_include_session_metadata = false;
    recording_update.recording_rotation = RecordingRotationPolicy::Daily;
    recording_update.recording_existing_file_behavior = ExistingFileBehavior::Overwrite;
    recording_update.recording_include_binary_transfer_payloads = true;
    recording_update.recording_include_io_labels = true;
    recording_update.recording_include_timestamps = true;
    recording_update.recording_memory_limit_bytes = 2 * 1024 * 1024;
    let saved_recording = store
        .save_recording_settings(&recording_update)
        .expect("save recording settings");
    assert_eq!(saved_recording.recording_path, "/var/log/nyaterm");
    assert!(!saved_recording.recording_auto_start);
    assert_eq!(saved_recording.recording_default_mode, RecordingMode::Raw);
    assert_eq!(
        saved_recording.recording_path_template,
        "{host}/{session}.raw.log"
    );
    assert!(!saved_recording.recording_include_session_metadata);
    assert_eq!(
        saved_recording.recording_rotation,
        RecordingRotationPolicy::Daily
    );
    assert_eq!(
        saved_recording.recording_existing_file_behavior,
        ExistingFileBehavior::Overwrite
    );
    assert!(saved_recording.recording_include_binary_transfer_payloads);
    assert!(saved_recording.recording_include_io_labels);
    assert!(saved_recording.recording_include_timestamps);
    assert_eq!(
        saved_recording.recording_memory_limit_bytes,
        2 * 1024 * 1024
    );
    let stored = store
        .load_settings_value()
        .expect("stored recording settings");
    assert_eq!(
        json_path(&stored, &["recording", "base_path"]).and_then(|value| value.as_str()),
        Some("/var/log/nyaterm")
    );
    assert_eq!(
        json_path(&stored, &["recording", "default_mode"]).and_then(|value| value.as_str()),
        Some("raw")
    );
    assert_eq!(
        json_path(&stored, &["recording", "existing_file_behavior"])
            .and_then(|value| value.as_str()),
        Some("overwrite")
    );
    assert_eq!(
        json_path(&stored, &["transfer", "recording_auto_start"]),
        None
    );

    let mut lock_update = saved_recording.clone();
    lock_update.enable_screen_lock = false;
    lock_update.idle_lock_minutes = 30;
    let saved_lock = store
        .save_screen_lock_settings(&lock_update)
        .expect("save screen lock settings");
    assert!(!saved_lock.enable_screen_lock);
    assert_eq!(saved_lock.idle_lock_minutes, 30);
    assert!(saved_lock.has_master_password);
    let stored = store.load_settings_value().expect("stored settings");
    assert_eq!(
        json_path(&stored, &["security", "master_password"]).and_then(|value| value.as_str()),
        Some("encrypted")
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn workspace_pane_layout_roundtrip() {
    let dir = unique_temp_dir("workspace-pane-layout");
    let store = ConnectionStore::open(&dir).expect("store");
    let layout = nyaterm_core::models::RestorableWorkspacePaneNode::Split {
        id: "split-1".to_string(),
        direction: "vertical".to_string(),
        ratio: 0.4,
        first: Box::new(nyaterm_core::models::RestorableWorkspacePaneNode::Leaf { tab_index: 0 }),
        second: Box::new(nyaterm_core::models::RestorableWorkspacePaneNode::Leaf { tab_index: 1 }),
    };
    store
        .save_workspace_pane_layout(Some(&layout))
        .expect("save");
    let loaded = store
        .load_workspace_pane_layout()
        .expect("load")
        .expect("some");
    assert_eq!(loaded, layout);
    store.save_workspace_pane_layout(None).expect("clear");
    assert!(store.load_workspace_pane_layout().expect("load").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn open_tab_pane_root_expands_and_maps_layout() {
    let tab = nyaterm_core::models::RestorableOpenTab {
        title: "split".to_string(),
        session_type: "SSH".to_string(),
        connection_id: Some("c1".to_string()),
        custom_name: Some("g".to_string()),
        tab_color: Some("#ff0000".to_string()),
        locked: true,
        active_pane_id: None,
        root: Some(nyaterm_core::models::RestorablePaneNode::Split {
            id: "s".to_string(),
            direction: "horizontal".to_string(),
            ratio: 0.4,
            first: Box::new(nyaterm_core::models::RestorablePaneNode::leaf_session(
                "a",
                "SSH",
                Some("c1".to_string()),
            )),
            second: Box::new(nyaterm_core::models::RestorablePaneNode::leaf_session(
                "b", "Local", None,
            )),
        }),
    };
    let sessions = tab.expanded_sessions();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].title, "a");
    assert_eq!(sessions[1].session_type, "Local");
    assert!(sessions.iter().all(|session| session.locked));
    let layout = tab.workspace_pane_layout_from_root(3).expect("layout");
    match layout {
        nyaterm_core::models::RestorableWorkspacePaneNode::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } => {
            assert_eq!(direction, "horizontal");
            assert!((ratio - 0.4).abs() < f64::EPSILON);
            assert_eq!(
                *first,
                nyaterm_core::models::RestorableWorkspacePaneNode::Leaf { tab_index: 3 }
            );
            assert_eq!(
                *second,
                nyaterm_core::models::RestorableWorkspacePaneNode::Leaf { tab_index: 4 }
            );
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn open_tabs_roundtrip() {
    let dir = unique_temp_dir("open-tabs");
    let store = ConnectionStore::open(&dir).expect("store");
    let tabs = vec![
        nyaterm_core::models::RestorableOpenTab::with_leaf_root(
            "Local",
            "Local",
            None,
            Some("dev".to_string()),
            Some("#22c55e".to_string()),
        ),
        nyaterm_core::models::RestorableOpenTab {
            title: "prod".to_string(),
            session_type: "SSH".to_string(),
            connection_id: Some("conn-1".to_string()),
            custom_name: None,
            tab_color: None,
            locked: true,
            active_pane_id: None,
            root: Some(nyaterm_core::models::RestorablePaneNode::Split {
                id: "split-1".to_string(),
                direction: "vertical".to_string(),
                ratio: 0.5,
                first: Box::new(nyaterm_core::models::RestorablePaneNode::leaf_session(
                    "prod-a",
                    "SSH",
                    Some("conn-1".to_string()),
                )),
                second: Box::new(nyaterm_core::models::RestorablePaneNode::leaf_session(
                    "prod-b",
                    "SSH",
                    Some("conn-1".to_string()),
                )),
            }),
        },
    ];
    store.save_open_tabs(&tabs).expect("save");
    let loaded = store.load_open_tabs().expect("load");
    assert_eq!(loaded, tabs);
    store.save_open_tabs(&[]).expect("clear");
    assert!(store.load_open_tabs().expect("load empty").is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn terminal_window_layout_roundtrip() {
    let dir = unique_temp_dir("terminal-window-layout");
    let store = ConnectionStore::open(&dir).expect("store");
    let layout = nyaterm_core::models::RestorableTerminalWindowNode::Split {
        direction: "vertical".to_string(),
        ratio: 0.45,
        first: Box::new(nyaterm_core::models::RestorableTerminalWindowNode::Leaf {
            tab_indexes: vec![0, 1],
            active_tab_index: Some(0),
        }),
        second: Box::new(nyaterm_core::models::RestorableTerminalWindowNode::Leaf {
            tab_indexes: vec![2],
            active_tab_index: Some(2),
        }),
    };
    store
        .save_terminal_window_layout(Some(&layout))
        .expect("save layout");
    let loaded = store
        .load_terminal_window_layout()
        .expect("load layout")
        .expect("some layout");
    assert_eq!(loaded, layout);
    store.save_terminal_window_layout(None).expect("clear");
    assert!(store.load_terminal_window_layout().expect("load").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn verifies_encrypted_master_password_from_settings() {
    let dir = unique_temp_dir("verify-master-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let token = encrypt_for_test(b"swordfish", &home_wrapping_key());
    store
        .save_settings_value(&serde_json::json!({
            "security": {
                "master_password": token
            }
        }))
        .expect("seed settings");

    assert!(
        store
            .verify_master_password("swordfish")
            .expect("verify correct")
    );
    assert!(
        !store
            .verify_master_password("wrong")
            .expect("verify incorrect")
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn changing_master_password_preserves_encrypted_cloud_secrets() {
    let dir = unique_temp_dir("change-master-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut cloud = CloudSyncSettings::default();
    cloud.webdav.password = Some("cloud-secret".to_string().into());
    store
        .save_cloud_sync_settings(cloud)
        .expect("save cloud secret");

    let summary = store
        .save_master_password(Some("first-password"))
        .expect("set password");
    assert!(summary.has_master_password);
    assert!(
        store
            .verify_master_password("first-password")
            .expect("verify")
    );
    assert_eq!(
        store
            .load_cloud_sync_settings()
            .expect("load after setting password")
            .webdav
            .password
            .as_deref(),
        Some("cloud-secret")
    );

    store
        .save_master_password(Some("second-password"))
        .expect("change password");
    assert!(
        store
            .verify_master_password("second-password")
            .expect("verify")
    );
    assert_eq!(
        store
            .load_cloud_sync_settings()
            .expect("load after changing password")
            .webdav
            .password
            .as_deref(),
        Some("cloud-secret")
    );

    let summary = store.save_master_password(None).expect("remove password");
    assert!(!summary.has_master_password);
    assert_eq!(
        store
            .load_cloud_sync_settings()
            .expect("load after removing password")
            .webdav
            .password
            .as_deref(),
        Some("cloud-secret")
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn missing_master_password_verifies_without_prompt_secret() {
    let dir = unique_temp_dir("verify-empty-master-password");
    let store = ConnectionStore::open(&dir).expect("store");
    assert!(
        store
            .verify_master_password("")
            .expect("verify without master password")
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn keyword_highlights_round_trip_and_import_merge() {
    let dir = unique_temp_dir("keyword-highlights");
    let store = ConnectionStore::open(&dir).expect("store");
    let initial = serde_json::json!({
        "general": {
            "custom_general": "keep"
        },
        "terminal": {
            "keyword_highlights_enabled": true,
            "keyword_highlights_across_wrapped_lines": true,
            "keyword_highlights": [
                {
                    "id": "panic",
                    "name": "Panic",
                    "patterns": ["panic", "ERROR"],
                    "color_dark": "#ff6b6b",
                    "color_light": "#b00020",
                    "enabled": true
                },
                {
                    "id": "invalid-empty-pattern",
                    "name": "Ignored",
                    "patterns": [""]
                },
                {
                    "id": "invalid-name",
                    "name": "   ",
                    "patterns": ["warn"]
                }
            ]
        }
    });
    store.save_settings_value(&initial).expect("seed settings");

    let loaded = store.load_keyword_highlights().expect("load highlights");
    assert!(loaded.enabled);
    assert!(loaded.across_wrapped_lines);
    // Blank-name + patterns becomes "Untitled rule"; blank-pattern named draft is kept.
    assert_eq!(loaded.rules.len(), 3);
    assert_eq!(loaded.rules[0].id, "panic");
    assert_eq!(loaded.rules[0].patterns, vec!["panic", "ERROR"]);
    assert_eq!(loaded.rules[1].id, "invalid-empty-pattern");
    assert_eq!(loaded.rules[2].name, "Untitled rule");

    let object_import = r##"{
        "keyword_highlights": [
            {
                "id": "panic",
                "name": "Panic Updated",
                "patterns": ["panic:"],
                "color_dark": "#ffd166",
                "color_light": "#8a5a00",
                "enabled": false
            },
            {
                "name": "Deploy",
                "patterns": ["deploy"]
            }
        ]
    }"##;
    let (saved, result) = store
        .import_keyword_highlights_json(object_import)
        .expect("object import");
    assert_eq!(result.imported_rules, 1);
    assert_eq!(result.updated_rules, 1);
    assert_eq!(result.total_rules, 4);
    assert!(saved.enabled);
    assert!(saved.across_wrapped_lines);

    let panic_rule = saved
        .rules
        .iter()
        .find(|rule| rule.id == "panic")
        .expect("panic rule");
    assert_eq!(panic_rule.name, "Panic Updated");
    assert!(!panic_rule.enabled);

    let deploy_rule = saved
        .rules
        .iter()
        .find(|rule| rule.name == "Deploy")
        .expect("deploy rule");
    assert!(deploy_rule.id.starts_with("highlight-"));
    assert_eq!(deploy_rule.color_dark, "#79c0ff");
    assert_eq!(deploy_rule.color_light, "#0969da");

    let array_import = r##"[
        {
            "id": "panic",
            "name": "Panic Final",
            "patterns": ["fatal"],
            "color_dark": "#fca5a5",
            "color_light": "#991b1b",
            "enabled": true
        }
    ]"##;
    let (saved, result) = store
        .import_keyword_highlights_json(array_import)
        .expect("array import");
    assert_eq!(result.imported_rules, 0);
    assert_eq!(result.updated_rules, 1);
    assert_eq!(result.total_rules, 4);
    assert_eq!(
        saved
            .rules
            .iter()
            .find(|rule| rule.id == "panic")
            .map(|rule| rule.patterns.as_slice()),
        Some(&["fatal".to_string()][..])
    );

    let stored = store.load_settings_value().expect("stored settings");
    assert_eq!(
        json_path(&stored, &["general", "custom_general"]).and_then(|value| value.as_str()),
        Some("keep")
    );

    let invalid = store.import_keyword_highlights_json(r#"[{"name":"","patterns":[""]}]"#);
    assert!(matches!(invalid, Err(StorageError::InvalidData(_))));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_state_round_trips_and_reads_legacy_doc() {
    let dir = unique_temp_dir("cloud-sync-state");
    let store = ConnectionStore::open(&dir).expect("store");
    let state = CloudSyncState {
        device_id: "device-a".to_string(),
        last_synced_payload_hash: Some("hash-a".to_string()),
        last_applied_remote_revision: Some("rev-a".to_string()),
        last_checked_at_ms: Some(10),
        last_synced_at_ms: Some(20),
    };
    store
        .save_cloud_sync_state(&state)
        .expect("save cloud state");
    let loaded = store.load_cloud_sync_state().expect("load cloud state");
    assert_eq!(loaded, state);

    let legacy_dir = unique_temp_dir("cloud-sync-state-legacy");
    let legacy_store = ConnectionStore::open(&legacy_dir).expect("legacy store");
    let legacy = CloudSyncState {
        device_id: "legacy-device".to_string(),
        last_synced_payload_hash: Some("legacy-hash".to_string()),
        last_applied_remote_revision: Some("legacy-rev".to_string()),
        last_checked_at_ms: Some(30),
        last_synced_at_ms: Some(40),
    };
    let legacy_content = serde_json::to_string(&legacy).expect("legacy json");
    let txn = legacy_store.db.begin_write().expect("legacy txn");
    txn.open_table(TEXT_DOCS_TABLE)
        .expect("text docs")
        .insert(LEGACY_TEXT_CLOUD_SYNC_STATE, legacy_content.as_str())
        .expect("insert legacy state");
    txn.commit().expect("commit legacy state");
    let loaded_legacy = legacy_store
        .load_cloud_sync_state()
        .expect("load legacy cloud state");
    assert_eq!(loaded_legacy, legacy);

    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(legacy_dir).ok();
}

#[test]
fn translation_settings_read_legacy_plaintext_and_encrypt_on_save() {
    let dir = unique_temp_dir("translation-settings");
    let store = ConnectionStore::open(&dir).expect("store");
    let initial = serde_json::json!({
        "translation": {
            "target_language": "ja",
            "deepl_api_key": "deepl-secret",
            "baidu_app_id": "baidu-id",
            "baidu_app_key": "baidu-secret",
            "ali_app_id": "ali-id",
            "ali_app_key": "ali-secret",
            "youdao_app_id": "youdao-id",
            "youdao_app_key": "youdao-secret"
        },
        "general": {
            "custom_general": "keep"
        }
    });
    store.save_settings_value(&initial).expect("seed settings");

    let loaded = store
        .load_translation_settings()
        .expect("load translation settings");
    assert_eq!(loaded.target_language, "ja");
    assert_eq!(loaded.deepl_api_key, "deepl-secret");
    assert_eq!(loaded.baidu_app_key, "baidu-secret");
    assert_eq!(loaded.ali_app_key, "ali-secret");
    assert_eq!(loaded.youdao_app_key, "youdao-secret");

    let mut update = loaded.clone();
    update.deepl_api_key = nyaterm_core::MASKED_SECRET_VALUE.to_string().into();
    update.baidu_app_key.expose_secret_mut().clear();
    update.ali_app_key = "ali-replacement".to_string().into();
    let saved = store
        .save_translation_settings(update)
        .expect("save translation settings");
    assert_eq!(saved.deepl_api_key, "deepl-secret");
    assert_eq!(saved.baidu_app_key, "");
    assert_eq!(saved.ali_app_key, "ali-replacement");
    assert_eq!(saved.youdao_app_key, "youdao-secret");
    assert!(store.load_master_key_token().expect("master key").is_some());

    let raw = store
        .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_DEFAULT)
        .expect("read raw settings")
        .expect("raw settings");
    let raw_translation = raw.get("translation").expect("translation");
    assert_ne!(
        raw_translation["deepl_api_key"].as_str(),
        Some("deepl-secret")
    );
    assert_eq!(raw_translation["baidu_app_key"].as_str(), Some(""));
    assert_ne!(
        raw_translation["ali_app_key"].as_str(),
        Some("ali-replacement")
    );
    assert_ne!(
        raw_translation["youdao_app_key"].as_str(),
        Some("youdao-secret")
    );
    assert_eq!(
        json_path(&raw, &["general", "custom_general"]).and_then(|value| value.as_str()),
        Some("keep")
    );
    assert_eq!(
        store
            .load_translation_settings()
            .expect("reload translation settings"),
        saved
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_settings_encrypt_and_merge_masked_provider_secrets() {
    let dir = unique_temp_dir("cloud-sync-settings");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut settings = CloudSyncSettings {
        enabled: true,
        provider: "github_gist".to_string(),
        ..CloudSyncSettings::default()
    };
    settings.webdav.password = Some("webdav-secret".to_string().into());
    settings.s3.secret_access_key = Some("s3-secret".to_string().into());
    settings.google_drive.access_token = Some("google-access".to_string().into());
    settings.github_gist.access_token = Some("github-token".to_string().into());

    let saved = store
        .save_cloud_sync_settings(settings.clone())
        .expect("save cloud settings");
    assert_eq!(saved, settings);
    assert!(store.load_master_key_token().expect("master key").is_some());

    let raw = store
        .read_json_table::<CloudSyncSettings>(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
        .expect("read raw")
        .expect("raw cloud settings");
    assert_ne!(raw.webdav.password.as_deref(), Some("webdav-secret"));
    assert_ne!(raw.s3.secret_access_key.as_deref(), Some("s3-secret"));
    assert_ne!(
        raw.google_drive.access_token.as_deref(),
        Some("google-access")
    );
    assert_ne!(
        raw.github_gist.access_token.as_deref(),
        Some("github-token")
    );

    let loaded = store
        .load_cloud_sync_settings()
        .expect("load cloud settings");
    assert_eq!(loaded, settings);

    let mut masked_update = loaded.clone();
    masked_update.webdav.password = Some(nyaterm_core::MASKED_SECRET_VALUE.to_string().into());
    masked_update.s3.secret_access_key = Some(String::new().into());
    masked_update.github_gist.access_token = Some("replacement-token".to_string().into());
    let merged = store
        .save_cloud_sync_settings(masked_update)
        .expect("save masked cloud settings");
    assert_eq!(merged.webdav.password.as_deref(), Some("webdav-secret"));
    assert_eq!(merged.s3.secret_access_key, None);
    assert_eq!(
        merged.github_gist.access_token.as_deref(),
        Some("replacement-token")
    );

    let reloaded = store
        .load_cloud_sync_settings()
        .expect("reload cloud settings");
    assert_eq!(reloaded, merged);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ai_settings_encrypt_and_merge_masked_provider_secrets() {
    let dir = unique_temp_dir("ai-settings");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut settings = AiSettings {
        enabled: true,
        default_mode: nyaterm_core::AiMode::Agent,
        ..AiSettings::default()
    };
    settings.provider_profiles[0].enabled = true;
    settings.provider_profiles[0].api_key = Some("profile-key".to_string().into());
    settings.provider_credentials[0].enabled = true;
    settings.provider_credentials[0].api_key = Some("credential-key".to_string().into());

    let saved = store.save_ai_settings(settings.clone()).expect("save ai");
    assert_eq!(saved.default_mode, nyaterm_core::AiMode::Agent);
    assert_eq!(
        saved.provider_profiles[0].api_key.as_deref(),
        Some("profile-key")
    );
    assert_eq!(
        saved.provider_credentials[0].api_key.as_deref(),
        Some("credential-key")
    );
    assert_eq!(
        saved.default_model_id.as_deref(),
        Some("openai:gpt-4o-mini")
    );
    assert!(!saved.models.is_empty());
    assert!(store.load_master_key_token().expect("master key").is_some());

    let raw = store
        .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_DEFAULT)
        .expect("read raw settings")
        .expect("raw settings");
    let raw_ai = raw.get("ai").expect("ai field");
    assert_ne!(
        raw_ai["provider_profiles"][0]["api_key"].as_str(),
        Some("profile-key")
    );
    assert_ne!(
        raw_ai["provider_credentials"][0]["api_key"].as_str(),
        Some("credential-key")
    );

    let loaded = store.load_ai_settings().expect("load ai");
    assert_eq!(loaded, saved);

    let mut masked_update = loaded.clone();
    masked_update.provider_profiles[0].api_key =
        Some(nyaterm_core::MASKED_SECRET_VALUE.to_string().into());
    masked_update.provider_credentials[0].api_key = Some("replacement-key".to_string().into());
    let merged = store
        .save_ai_settings(masked_update)
        .expect("save masked ai");
    assert_eq!(
        merged.provider_profiles[0].api_key.as_deref(),
        Some("profile-key")
    );
    assert_eq!(
        merged.provider_credentials[0].api_key.as_deref(),
        Some("replacement-key")
    );
    assert_eq!(store.load_ai_settings().expect("reload ai"), merged);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ai_settings_loads_and_normalizes_legacy_embedded_settings() {
    let dir = unique_temp_dir("ai-settings-legacy");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut raw = default_settings_value();
    set_nested_json_value(
        &mut raw,
        &["ai"],
        serde_json::json!({
            "schema_version": 2,
            "enabled": true,
            "active_profile_id": "ollama",
            "provider_profiles": [{
                "id": "ollama",
                "name": "Ollama",
                "provider_kind": "ollama",
                "model": "llama3",
                "base_url": "http://localhost:11434/v1/",
                "enabled": true
            }],
            "models": [],
            "provider_credentials": []
        }),
    );
    store.save_settings_value(&raw).expect("save raw settings");

    let loaded = store.load_ai_settings().expect("load ai");
    assert_eq!(loaded.schema_version, 6);
    assert_eq!(loaded.active_profile_id, "ollama");
    assert_eq!(loaded.default_model_id.as_deref(), Some("ollama:llama3"));
    assert!(!loaded.provider_credentials.is_empty());
    assert!(!loaded.terminal_ai_actions.is_empty());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ai_settings_v6_preserves_unknown_fields_and_drops_explicitly_deprecated_fields() {
    let dir = unique_temp_dir("ai-settings-v6-unknown");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut raw = default_settings_value();
    set_nested_json_value(
        &mut raw,
        &["ai"],
        serde_json::json!({
            "schema_version": 5,
            "enabled": true,
            "future_top_level": { "flag": true },
            "active_profile_id": "ollama",
            "provider_profiles": [{
                "id": "ollama",
                "name": "Ollama",
                "provider_kind": "ollama",
                "model": "llama3",
                "base_url": "http://localhost:11434/v1/",
                "enabled": true,
                "future_profile_field": "preserved"
            }],
            "provider_credentials": [{
                "id": "ollama",
                "name": "Ollama",
                "provider_kind": "ollama",
                "base_url": "http://localhost:11434/v1/",
                "enabled": true,
                "future_credential_field": 42
            }],
            "models": [{
                "id": "ollama:llama3",
                "name": "llama3",
                "provider_kind": "ollama",
                "enabled": true,
                "source": "rust-genai",
                "future_model_field": [1, 2, 3]
            }],
            "external_mcp": {
                "enabled": true,
                "permission_mode": "confirm",
                "session_scope": "current_window",
                "server_mode": "temporary",
                "idle_timeout_minutes": 10,
                "future_mcp_field": "preserved"
            }
        }),
    );
    store.save_settings_value(&raw).expect("seed settings");

    let loaded = store.load_ai_settings().expect("load settings");
    assert_eq!(loaded.schema_version, 6);
    assert_eq!(
        loaded.provider_profiles[0].base_url.as_deref(),
        Some("http://localhost:11434/")
    );
    assert_eq!(
        loaded.provider_credentials[0].base_url.as_deref(),
        Some("http://localhost:11434/")
    );
    store
        .save_ai_settings(loaded)
        .expect("save normalized settings");

    let saved = store
        .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_DEFAULT)
        .expect("read raw settings")
        .expect("raw settings");
    let ai = &saved["ai"];
    assert_eq!(ai["future_top_level"]["flag"], true);
    assert_eq!(
        ai["provider_profiles"][0]["future_profile_field"],
        "preserved"
    );
    assert_eq!(ai["provider_credentials"][0]["future_credential_field"], 42);
    assert_eq!(
        ai["models"][0]["future_model_field"],
        serde_json::json!([1, 2, 3])
    );
    assert_eq!(ai["external_mcp"]["future_mcp_field"], "preserved");
    assert!(ai["external_mcp"].get("server_mode").is_none());
    assert!(ai["external_mcp"].get("idle_timeout_minutes").is_none());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn quick_commands_round_trip_and_upsert_preserves_created_use_count() {
    let dir = unique_temp_dir("quick-commands");
    let store = ConnectionStore::open(&dir).expect("store");

    assert_eq!(
        store.load_quick_commands().expect("empty quick commands"),
        QuickCommandsConfig::default()
    );

    let inserted = store
        .upsert_quick_command(
            QuickCommand {
                id: "cmd-1".to_string(),
                label: "List".to_string(),
                command: "ls -la".to_string(),
                category_id: Some("cat-shell".to_string()),
                description: Some("List files".to_string()),
                color_tag: Some("blue".to_string()),
                icon_tag: Some("terminal".to_string()),
                pinned: Some(false),
                execution_mode: Some("append".to_string()),
                source: Some("ai".to_string()),
                risk_level: Some(nyaterm_core::RiskLevel::Low),
                updated_at: None,
                created_at: Some(111),
                use_count: Some(7),
                sort_order: Some(3),
            },
            Some(QuickCommandCategory {
                id: "cat-shell".to_string(),
                name: "Shell".to_string(),
                parent_id: None,
                sort_order: 0,
            }),
        )
        .expect("insert quick command");
    assert_eq!(inserted.categories.len(), 1);
    assert_eq!(inserted.commands[0].created_at, Some(111));
    assert_eq!(inserted.commands[0].use_count, Some(7));
    assert!(inserted.commands[0].updated_at.is_some());

    let updated = store
        .upsert_quick_command(
            QuickCommand {
                id: "cmd-1".to_string(),
                label: "List all".to_string(),
                command: "ls -lah".to_string(),
                category_id: Some("cat-shell".to_string()),
                description: None,
                color_tag: Some("green".to_string()),
                icon_tag: Some("terminal".to_string()),
                pinned: Some(true),
                execution_mode: Some("append".to_string()),
                source: Some("manual".to_string()),
                risk_level: Some(nyaterm_core::RiskLevel::Medium),
                updated_at: None,
                created_at: Some(999),
                use_count: Some(99),
                sort_order: Some(3),
            },
            Some(QuickCommandCategory {
                id: "cat-shell".to_string(),
                name: "Duplicate Shell".to_string(),
                parent_id: None,
                sort_order: 0,
            }),
        )
        .expect("update quick command");
    assert_eq!(updated.categories.len(), 1);
    assert_eq!(updated.commands.len(), 1);
    assert_eq!(updated.commands[0].label, "List all");
    assert_eq!(updated.commands[0].command, "ls -lah");
    assert_eq!(updated.commands[0].created_at, Some(111));
    assert_eq!(updated.commands[0].use_count, Some(7));

    store
        .increment_quick_command_use_count("cmd-1")
        .expect("increment quick command");
    let loaded = store.load_quick_commands().expect("load quick commands");
    assert_eq!(loaded.commands[0].use_count, Some(8));

    let raw = store
        .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_QUICK_COMMANDS)
        .expect("read raw quick commands")
        .expect("quick command doc");
    assert_eq!(raw["categories"][0]["id"], "cat-shell");
    assert_eq!(raw["commands"][0]["category_id"], "cat-shell");
    assert_eq!(raw["commands"][0]["execution_mode"], "append");
    assert_eq!(raw["commands"][0]["source"], "manual");
    assert!(raw["commands"][0].get("created_at").is_some());
    assert!(raw["commands"][0].get("use_count").is_some());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn quick_commands_export_matches_tauri_json_shape() {
    let raw = export_quick_commands_json(QuickCommandsConfig {
        categories: vec![QuickCommandCategory {
            id: "cat-shell".to_string(),
            name: "Shell".to_string(),
            parent_id: None,
            sort_order: 0,
        }],
        commands: vec![QuickCommand {
            id: "cmd-1".to_string(),
            label: "List".to_string(),
            command: "ls -la".to_string(),
            category_id: Some("cat-shell".to_string()),
            description: Some("List files".to_string()),
            color_tag: Some("blue".to_string()),
            icon_tag: Some("terminal".to_string()),
            pinned: Some(true),
            execution_mode: Some("append".to_string()),
            source: Some("manual".to_string()),
            risk_level: Some(nyaterm_core::RiskLevel::Low),
            updated_at: Some(222),
            created_at: Some(111),
            use_count: Some(7),
            sort_order: Some(3),
        }],
    })
    .expect("export serializes");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("export json parses");

    assert_eq!(value["categories"][0]["id"], "cat-shell");
    assert_eq!(value["categories"][0]["sort_order"], 0);
    assert!(value["categories"][0].get("parent_id").is_none());
    assert_eq!(value["commands"][0]["id"], "cmd-1");
    assert_eq!(value["commands"][0]["pinned"], true);
    assert_eq!(value["commands"][0]["execution_mode"], "append");
    assert_eq!(value["commands"][0]["risk_level"], "low");
    assert_eq!(value["commands"][0]["sort_order"], 3);
    assert!(value["commands"][0].get("created_at").is_none());
    assert!(value["commands"][0].get("use_count").is_none());
}

/// Endpoint the sync/backup snapshot carries, valid on the running platform.
///
/// `normalize_backup_agent_settings` rewrites endpoints that
/// `ssh_agent_endpoint_supported_on_current_platform` rejects, so the fixture has to
/// pick per-platform values. The two helpers must stay distinguishable: the test
/// asserts the device-local value survives a sync apply while this one is restored
/// from a backup.
fn synced_agent_endpoint() -> nyaterm_core::SshAgentEndpoint {
    #[cfg(windows)]
    return nyaterm_core::SshAgentEndpoint::WindowsOpenSsh;
    #[cfg(not(windows))]
    return nyaterm_core::SshAgentEndpoint::UnixSocket {
        path: "/run/user/1000/agent.sock".to_string(),
    };
}

/// Endpoint standing in for a device-local setting that must never be synced away.
fn device_local_agent_endpoint() -> nyaterm_core::SshAgentEndpoint {
    #[cfg(windows)]
    return nyaterm_core::SshAgentEndpoint::Pageant;
    #[cfg(not(windows))]
    return nyaterm_core::SshAgentEndpoint::UnixSocket {
        path: "/tmp/device-local-agent.sock".to_string(),
    };
}

#[test]
fn sync_snapshot_strips_device_local_ssh_agent_settings() {
    let dir = unique_temp_dir("sync-agent-settings");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut sessions = SessionsConfig {
        custom_icons: Vec::new(),
        groups: Vec::new(),
        connections: vec![SavedConnection {
            extensions: Default::default(),
            id: "agent-sync".to_string(),
            name: "Agent Sync".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "agent".to_string(),
                ..Default::default()
            }),
            network: None,
            post_login: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }],
    };
    let ConnectionType::Ssh {
        auth_agent_endpoint,
        agent_forwarding_config,
        ..
    } = &mut sessions.connections[0].config
    else {
        panic!("SSH expected");
    };
    *auth_agent_endpoint = Some(synced_agent_endpoint());
    *agent_forwarding_config = Some(nyaterm_core::SshAgentForwardingConfig {
        enabled: true,
        sources: nyaterm_core::SshAgentForwardingSources {
            external_agent: true,
            external_agent_endpoints: vec![synced_agent_endpoint()],
            stored_keys: false,
        },
        policy: nyaterm_core::SshAgentForwardingPolicy::All,
    });
    store.replace_sessions(&sessions).expect("save sessions");

    let mut snapshot = store
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Sync, "device", "test")
        .expect("build sync snapshot");
    snapshot.recalculate_hash().expect("hash sync snapshot");
    let portable: SessionsConfig =
        serde_json::from_str(snapshot.entities.get("sessions").expect("sessions entity"))
            .expect("decode sessions");
    let ConnectionType::Ssh {
        auth_agent_endpoint,
        agent_forwarding_config,
        ..
    } = &portable.connections[0].config
    else {
        panic!("SSH expected");
    };
    assert_eq!(
        auth_agent_endpoint,
        &Some(nyaterm_core::SshAgentEndpoint::Auto)
    );
    assert!(agent_forwarding_config.is_none());

    let mut backup = store
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Backup, "device", "test")
        .expect("build backup snapshot");
    backup.recalculate_hash().expect("hash backup snapshot");
    let ConnectionType::Ssh {
        auth_agent_endpoint,
        agent_forwarding_config,
        ..
    } = &mut sessions.connections[0].config
    else {
        panic!("SSH expected");
    };
    *auth_agent_endpoint = Some(device_local_agent_endpoint());
    *agent_forwarding_config = Some(nyaterm_core::SshAgentForwardingConfig::default());
    store
        .replace_sessions(&sessions)
        .expect("replace local Agent settings");

    store
        .apply_raw_portable_snapshot(&snapshot)
        .expect("apply sync snapshot");
    let synced = store.get_connection("agent-sync").unwrap().unwrap();
    let ConnectionType::Ssh {
        auth_agent_endpoint,
        agent_forwarding_config,
        ..
    } = synced.config
    else {
        panic!("SSH expected");
    };
    assert_eq!(auth_agent_endpoint, Some(device_local_agent_endpoint()));
    assert_eq!(
        agent_forwarding_config,
        Some(nyaterm_core::SshAgentForwardingConfig::default())
    );

    store
        .apply_raw_portable_snapshot(&backup)
        .expect("apply backup snapshot");
    let restored = store.get_connection("agent-sync").unwrap().unwrap();
    let ConnectionType::Ssh {
        auth_agent_endpoint,
        agent_forwarding_config,
        ..
    } = restored.config
    else {
        panic!("SSH expected");
    };
    assert_eq!(auth_agent_endpoint, Some(synced_agent_endpoint()));
    assert!(agent_forwarding_config.is_some_and(|config| config.enabled));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn command_history_uses_legacy_table_and_normalizes_entries() {
    let dir = unique_temp_dir("command-history");
    let store = ConnectionStore::open(&dir).expect("store");

    store
        .append_command_history(" user@host:~$ ls -la ")
        .expect("append ls");
    store
        .append_command_history("ls -la")
        .expect("append ls again");
    store
        .append_command_history("PS C:\\Users\\me> git status")
        .expect("append powershell");
    store.append_command_history("   ").expect("ignore blank");

    let history = store.list_command_history(10).expect("list history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].command, "git status");
    assert_eq!(history[1].command, "ls -la");
    assert_eq!(history[1].use_count, 2);

    let raw = store
        .list_raw_by_prefix(COMMAND_HISTORY_TABLE, COMMAND_HISTORY_PREFIX)
        .expect("raw history");
    assert_eq!(raw.len(), 2);
    assert!(
        raw.iter()
            .all(|(key, _)| key.starts_with(COMMAND_HISTORY_PREFIX))
    );
    assert!(raw.iter().all(|(key, _)| key.contains('|')));

    store
        .delete_command_history("root@server:/tmp# ls -la")
        .expect("delete normalized");
    let remaining = store.list_command_history(10).expect("remaining history");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].command, "git status");

    store
        .replace_command_history(&[
            CommandHistoryEntry {
                command: "[prod] $ uptime".to_string(),
                last_used_at_ms: 10,
                use_count: 1,
            },
            CommandHistoryEntry {
                command: "uptime".to_string(),
                last_used_at_ms: 20,
                use_count: 3,
            },
            CommandHistoryEntry {
                command: "pwd".to_string(),
                last_used_at_ms: 30,
                use_count: 1,
            },
        ])
        .expect("replace history");
    let replaced = store.list_command_history(10).expect("replaced history");
    assert_eq!(
        replaced
            .iter()
            .map(|entry| entry.command.as_str())
            .collect::<Vec<_>>(),
        ["pwd", "uptime"]
    );
    assert_eq!(replaced[1].use_count, 4);

    std::fs::remove_dir_all(dir).ok();
}

pub(super) fn unique_temp_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nyaterm-core-{name}-{}-{n}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn encrypt_for_test(plaintext: &[u8], key: &Key<Aes256Gcm>) -> String {
    let cipher = Aes256Gcm::new(key);
    let nonce = aes_gcm::Nonce::from([9_u8; 12]);
    let ciphertext = cipher.encrypt(&nonce, plaintext).expect("encrypt");
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    B64.encode(combined)
}

#[allow(deprecated)]
fn test_key(seed: u8) -> Key<Aes256Gcm> {
    *Key::<Aes256Gcm>::from_slice(&[seed; 32])
}

#[allow(deprecated)]
fn home_wrapping_key() -> Key<Aes256Gcm> {
    let mut hasher = Sha256::new();
    hasher.update(b"nyaterm-key-wrap-v1:");
    hasher.update(dirs::home_dir().expect("home").to_string_lossy().as_bytes());
    *Key::<Aes256Gcm>::from_slice(&hasher.finalize())
}

#[test]
fn portable_snapshot_preserves_notes_and_unknown_entities_after_apply() {
    let dir = unique_temp_dir("portable-opaque-entities");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut incoming = store
        .build_raw_portable_snapshot(
            nyaterm_core::PortableSnapshotKind::Backup,
            "tauri-device",
            "tauri-test",
        )
        .expect("build source snapshot");
    let notes = r#"{"folders":[],"notes":[{"id":"note-1","parent_id":null,"title":"Imported","markdown":"preserve me","sort_order":0,"revision":1,"created_at_ms":1,"updated_at_ms":2}]}"#;
    let future = r#"{"schema":7,"payload":{"future":true}}"#;
    incoming
        .entities
        .insert("notes".to_string(), notes.to_string());
    incoming
        .entities
        .insert("future_entity".to_string(), future.to_string());
    incoming.recalculate_hash().expect("source hash");
    let source_hash = incoming.meta.payload_hash.clone();

    store
        .apply_raw_portable_snapshot(&incoming)
        .expect("apply source snapshot");

    assert!(
        store
            .read_string_table(PORTABLE_OPAQUE_ENTITIES_TABLE, "notes")
            .expect("read opaque notes")
            .is_none()
    );
    assert_eq!(
        store.load_notes_snapshot().expect("load typed notes").notes[0].markdown,
        "preserve me"
    );
    assert_eq!(
        store
            .read_string_table(META_TABLE, META_PORTABLE_SOURCE_PAYLOAD_HASH)
            .expect("read source hash")
            .as_deref(),
        Some(source_hash.as_str())
    );
    assert_eq!(
        store
            .read_string_table(META_TABLE, META_PORTABLE_SOURCE_SCHEMA_VERSION)
            .expect("read source schema")
            .as_deref(),
        Some("3")
    );

    let mut rebuilt = store
        .build_raw_portable_snapshot(
            nyaterm_core::PortableSnapshotKind::Backup,
            "gpui-device",
            "gpui-test",
        )
        .expect("rebuild snapshot");
    assert_eq!(
        rebuilt.entities.get("notes").map(String::as_str),
        Some(notes)
    );
    assert_eq!(
        rebuilt.entities.get("future_entity").map(String::as_str),
        Some(future)
    );
    rebuilt.recalculate_hash().expect("rebuilt hash");
    nyaterm_core::portable_snapshot::validate_raw_snapshot(&rebuilt)
        .expect("rebuilt snapshot is valid");

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn portable_snapshot_rejects_tampered_unknown_entity_before_writing() {
    let dir = unique_temp_dir("portable-opaque-entity-integrity");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut snapshot = store
        .build_raw_portable_snapshot(
            nyaterm_core::PortableSnapshotKind::Backup,
            "source-device",
            "source-version",
        )
        .expect("build snapshot");
    let original = r#"{"schema":7,"payload":{"future":true}}"#;
    snapshot
        .entities
        .insert("future_entity".to_string(), original.to_string());
    snapshot.recalculate_hash().expect("hash snapshot");
    store
        .apply_raw_portable_snapshot(&snapshot)
        .expect("apply valid snapshot");

    snapshot.entities.insert(
        "future_entity".to_string(),
        r#"{"schema":7,"payload":{"future":false}}"#.to_string(),
    );
    let error = store
        .apply_raw_portable_snapshot(&snapshot)
        .expect_err("tampered opaque entity must be rejected");
    assert!(matches!(
        error,
        StorageError::PortableSnapshot(nyaterm_core::PortableSnapshotError::EntitiesHashMismatch)
    ));
    assert_eq!(
        store
            .read_string_table(PORTABLE_OPAQUE_ENTITIES_TABLE, "future_entity")
            .expect("read preserved entity")
            .as_deref(),
        Some(original)
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn legacy_rdp_known_hosts_migrate_to_the_dedicated_table_idempotently() {
    let dir = unique_temp_dir("rdp-known-host-migration");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let host = "legacy.example.com";
    let port = 3389;
    let key = format!(
        "{RDP_KNOWN_HOST_PREFIX}{}",
        stable_id(&format!("{host}:{port}"))
    );
    let legacy_record = serde_json::json!({
        "host": host,
        "port": port,
        "sha256_fingerprint": "sha256:legacy",
        "subject": "CN=legacy.example.com",
        "issuer": "CN=legacy-ca",
        "valid_from": null,
        "valid_to": null,
        "created_at_ms": 10,
        "updated_at_ms": 20
    });
    {
        let db = Database::create(dir.join(DATABASE_FILE)).expect("db");
        let txn = db.begin_write().expect("txn");
        write_json_in_txn(&txn, KNOWN_HOSTS_TABLE, &key, &legacy_record)
            .expect("seed legacy rdp record");
        txn.commit().expect("commit");
    }

    let store = ConnectionStore::open(&dir).expect("store");
    assert_eq!(
        store
            .check_rdp_known_host(host, port, "SHA256:LEGACY")
            .expect("migrated match"),
        RdpKnownHostCheck::Match
    );
    let migrated: Option<serde_json::Value> = store
        .read_json_table(RDP_KNOWN_HOSTS_TABLE, &key)
        .expect("read dedicated record");
    assert_eq!(
        migrated
            .as_ref()
            .and_then(|record| record.get("created_at_ms"))
            .and_then(serde_json::Value::as_u64),
        Some(10)
    );
    let second = store
        .migrate_legacy_rdp_known_hosts()
        .expect("repeat migration");
    assert_eq!(second.migrated, 0);
    assert_eq!(second.already_present, 1);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn dedicated_rdp_records_win_conflicts_and_do_not_break_ssh_replacement() {
    let dir = unique_temp_dir("rdp-known-host-conflict");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let host = "windows.example.com";
    let port = 3389;
    let key = format!(
        "{RDP_KNOWN_HOST_PREFIX}{}",
        stable_id(&format!("{host}:{port}"))
    );
    let legacy_record = serde_json::json!({
        "host": host,
        "port": port,
        "sha256_fingerprint": "sha256:legacy",
        "created_at_ms": 1,
        "updated_at_ms": 1
    });
    let dedicated_record = serde_json::json!({
        "host": host,
        "port": port,
        "sha256_fingerprint": "sha256:dedicated",
        "created_at_ms": 2,
        "updated_at_ms": 3
    });
    {
        let db = Database::create(dir.join(DATABASE_FILE)).expect("db");
        let txn = db.begin_write().expect("txn");
        write_json_in_txn(&txn, KNOWN_HOSTS_TABLE, &key, &legacy_record)
            .expect("seed legacy record");
        write_json_in_txn(&txn, RDP_KNOWN_HOSTS_TABLE, &key, &dedicated_record)
            .expect("seed dedicated record");
        txn.commit().expect("commit");
    }

    let store = ConnectionStore::open(&dir).expect("store");
    assert_eq!(
        store
            .check_rdp_known_host(host, port, "SHA256:DEDICATED")
            .expect("dedicated match"),
        RdpKnownHostCheck::Match
    );
    store
        .upsert_known_host("ssh.example.com ssh-ed25519 AAAA")
        .expect("seed ssh host");
    store
        .replace_known_host_for_host("ssh.example.com", "ssh.example.com ssh-ed25519 BBBB")
        .expect("replace ssh host without parsing legacy rdp json");
    assert_eq!(
        store
            .check_known_host("ssh.example.com", "ssh-ed25519", "BBBB")
            .expect("ssh replacement"),
        KnownHostCheck::Match
    );

    std::fs::remove_dir_all(dir).ok();
}

fn ssh_connection_for_asset(id: &str) -> SavedConnection {
    SavedConnection {
        extensions: Default::default(),
        id: id.to_string(),
        name: "Asset Host".to_string(),
        config: ConnectionType::Ssh {
            host: "10.0.0.2".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            ai_execution_profile: AiExecutionProfile::Auto,
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
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
        sftp: Default::default(),
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    }
}

#[test]
fn merge_connection_asset_from_monitoring_creates_and_merges_atomically() {
    let dir = unique_temp_dir("asset-merge");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("asset-1");
    connection.asset = Some(AssetMetadata {
        device_type: Some(AssetDeviceType::Cloud),
        cpu_threads: Some(16),
        tags: Some(vec!["production".to_string()]),
        notes: Some("maintained by operator".to_string()),
        ..AssetMetadata::default()
    });
    store.save_connection(&connection).expect("save connection");

    // First patch establishes monitoring fields without replacing operator-maintained facts.
    let created = store
        .merge_connection_asset_from_monitoring(
            "asset-1",
            AssetMetadata {
                hostname: Some("node-1".to_string()),
                cpu_cores: Some(8),
                accelerators: Some(vec![AssetAccelerator {
                    r#type: AssetAcceleratorType::Gpu,
                    vendor: Some("NVIDIA".to_string()),
                    model: Some("A100".to_string()),
                    count: Some(1),
                    memory_bytes: None,
                }]),
                ..AssetMetadata::default()
            },
        )
        .expect("merge asset");
    assert!(created, "first merge changes the asset");

    let asset = store
        .get_connection("asset-1")
        .expect("get")
        .expect("connection")
        .asset
        .expect("asset");
    assert_eq!(asset.hostname.as_deref(), Some("node-1"));
    assert_eq!(asset.cpu_cores, Some(8));
    assert_eq!(asset.device_type, Some(AssetDeviceType::Cloud));
    assert_eq!(asset.cpu_threads, Some(16));
    assert_eq!(
        asset.tags.as_deref(),
        Some(["production".to_string()].as_slice())
    );
    assert_eq!(asset.notes.as_deref(), Some("maintained by operator"));

    // Second patch updates memory and replaces GPU entries, keeping hostname.
    let changed = store
        .merge_connection_asset_from_monitoring(
            "asset-1",
            AssetMetadata {
                memory_bytes: Some(1024),
                accelerators: Some(vec![AssetAccelerator {
                    r#type: AssetAcceleratorType::Gpu,
                    vendor: Some("NVIDIA".to_string()),
                    model: Some("H100".to_string()),
                    count: Some(2),
                    memory_bytes: None,
                }]),
                ..AssetMetadata::default()
            },
        )
        .expect("merge asset again");
    assert!(changed);

    let asset = store
        .get_connection("asset-1")
        .expect("get")
        .expect("connection")
        .asset
        .expect("asset");
    assert_eq!(asset.hostname.as_deref(), Some("node-1"));
    assert_eq!(asset.cpu_cores, Some(8));
    assert_eq!(asset.memory_bytes, Some(1024));
    let accelerators = asset.accelerators.expect("accelerators");
    assert_eq!(accelerators.len(), 1);
    assert_eq!(accelerators[0].model.as_deref(), Some("H100"));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn merge_connection_asset_returns_false_when_unchanged_or_missing() {
    let dir = unique_temp_dir("asset-merge-noop");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_connection(&ssh_connection_for_asset("asset-1"))
        .expect("save connection");

    // Missing connection is a no-op, not an error (background monitoring).
    assert!(
        !store
            .merge_connection_asset_from_monitoring(
                "missing",
                AssetMetadata {
                    hostname: Some("x".to_string()),
                    ..AssetMetadata::default()
                },
            )
            .expect("missing connection is Ok(false)")
    );

    store
        .merge_connection_asset_from_monitoring(
            "asset-1",
            AssetMetadata {
                hostname: Some("node-1".to_string()),
                ..AssetMetadata::default()
            },
        )
        .expect("first merge");

    // An identical patch does not report a change.
    assert!(
        !store
            .merge_connection_asset_from_monitoring(
                "asset-1",
                AssetMetadata {
                    hostname: Some("node-1".to_string()),
                    ..AssetMetadata::default()
                },
            )
            .expect("idempotent merge")
    );

    // An empty patch never changes anything.
    assert!(
        !store
            .merge_connection_asset_from_monitoring("asset-1", AssetMetadata::default())
            .expect("empty patch")
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn merge_connection_asset_preserves_inline_password() {
    let dir = unique_temp_dir("asset-merge-password");
    let store = ConnectionStore::open(&dir).expect("store");

    let mut connection = ssh_connection_for_asset("asset-secret");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some("hunter2".to_string().into()),
        ..Default::default()
    });
    store
        .save_connection(&connection)
        .expect("save with password");

    store
        .merge_connection_asset_from_monitoring(
            "asset-secret",
            AssetMetadata {
                hostname: Some("node-1".to_string()),
                ..AssetMetadata::default()
            },
        )
        .expect("merge asset");

    // The stored inline password must survive an asset-only update.
    let reloaded = store
        .get_connection("asset-secret")
        .expect("get")
        .expect("connection");
    let auth = reloaded.auth.expect("auth");
    assert_eq!(auth.password.as_deref(), Some("hunter2"));
    assert_eq!(
        reloaded.asset.expect("asset").hostname.as_deref(),
        Some("node-1")
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_ui_layout_settings_roundtrips_start_workspace_and_asset_sort() {
    let dir = unique_temp_dir("ui-asset-sort");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut summary = store.load_app_settings_summary().expect("load");

    summary.ui_start_workspace_mode = "assets".to_string();
    summary.ui_asset_sort_key = Some("hostname".to_string());
    summary.ui_asset_sort_direction = Some("desc".to_string());
    let saved = store.save_ui_layout_settings(&summary).expect("save ui");
    assert_eq!(saved.ui_start_workspace_mode, "assets");
    assert_eq!(saved.ui_asset_sort_key.as_deref(), Some("hostname"));
    assert_eq!(saved.ui_asset_sort_direction.as_deref(), Some("desc"));

    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["ui"]["start_workspace_mode"], "assets");
    assert_eq!(raw["ui"]["asset_sort_key"], "hostname");
    assert_eq!(raw["ui"]["asset_sort_direction"], "desc");

    let reloaded = store.load_app_settings_summary().expect("reload");
    assert_eq!(reloaded.ui_start_workspace_mode, "assets");
    assert_eq!(reloaded.ui_asset_sort_key.as_deref(), Some("hostname"));
    assert_eq!(reloaded.ui_asset_sort_direction.as_deref(), Some("desc"));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ui_settings_normalize_and_default_for_legacy_and_invalid_values() {
    let dir = unique_temp_dir("ui-asset-sort-legacy");
    let store = ConnectionStore::open(&dir).expect("store");

    // Legacy document without the new keys falls back to defaults.
    let legacy = store.load_app_settings_summary().expect("load legacy");
    assert_eq!(legacy.ui_start_workspace_mode, "workbench");
    assert!(legacy.ui_asset_sort_key.is_none());
    assert!(legacy.ui_asset_sort_direction.is_none());

    // Invalid values normalize: unknown workspace mode -> workbench, unknown
    // direction -> None (serialized as JSON null).
    let mut summary = legacy;
    summary.ui_start_workspace_mode = "not-a-mode".to_string();
    summary.ui_asset_sort_direction = Some("sideways".to_string());
    summary.ui_asset_sort_key = Some("   ".to_string());
    let saved = store
        .save_ui_layout_settings(&summary)
        .expect("save invalid");
    assert_eq!(saved.ui_start_workspace_mode, "workbench");
    assert!(saved.ui_asset_sort_direction.is_none());
    assert!(saved.ui_asset_sort_key.is_none());

    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["ui"]["start_workspace_mode"], "workbench");
    assert_eq!(raw["ui"]["asset_sort_direction"], serde_json::Value::Null);
    assert_eq!(raw["ui"]["asset_sort_key"], serde_json::Value::Null);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn connection_asset_survives_redb_replace_sessions_roundtrip() {
    let dir = unique_temp_dir("asset-redb-roundtrip");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("asset-rt");
    connection.asset = Some(AssetMetadata {
        hostname: Some("gpu-node".to_string()),
        cpu_cores: Some(64),
        accelerators: Some(vec![AssetAccelerator {
            r#type: AssetAcceleratorType::Npu,
            vendor: Some("Huawei".to_string()),
            model: Some("910B".to_string()),
            count: Some(4),
            memory_bytes: None,
        }]),
        ..AssetMetadata::default()
    });
    let config = SessionsConfig {
        custom_icons: Vec::new(),
        groups: Vec::new(),
        connections: vec![connection],
    };
    store.replace_sessions(&config).expect("replace");

    let loaded = store
        .get_connection("asset-rt")
        .expect("get")
        .expect("connection")
        .asset
        .expect("asset");
    assert_eq!(loaded.hostname.as_deref(), Some("gpu-node"));
    assert_eq!(loaded.cpu_cores, Some(64));
    let accelerators = loaded.accelerators.expect("accelerators");
    assert_eq!(accelerators[0].r#type, AssetAcceleratorType::Npu);
    assert_eq!(accelerators[0].model.as_deref(), Some("910B"));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn main_window_state_is_missing_until_saved_and_round_trips_independently() {
    let dir = unique_temp_dir("main-window-state-round-trip");
    let store = ConnectionStore::open(&dir).expect("store");
    assert_eq!(store.load_main_window_state().expect("load missing"), None);

    let state = MainWindowState::new(
        Some(uuid::Uuid::parse_str("12345678-1234-5678-1234-567812345678").expect("uuid")),
        MainWindowBounds {
            x: -1280,
            y: 40,
            width: 1200,
            height: 760,
        },
        true,
    );
    store
        .save_main_window_state(&state)
        .expect("save window state");

    let mut settings = store.load_app_settings_summary().expect("load settings");
    settings.theme = "github-light".to_string();
    store
        .save_appearance_settings(&settings)
        .expect("save settings");
    drop(store);

    let reopened = ConnectionStore::open(&dir).expect("reopen store");
    assert_eq!(
        reopened
            .load_main_window_state()
            .expect("load window state"),
        Some(state)
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn main_window_state_rejects_corrupted_and_unsupported_documents() {
    let dir = unique_temp_dir("main-window-state-invalid");
    let store = ConnectionStore::open(&dir).expect("store");

    let txn = store.db.begin_write().expect("write transaction");
    write_json_in_txn(
        &txn,
        SETTINGS_TABLE,
        SETTINGS_MAIN_WINDOW_STATE,
        &serde_json::json!({
            "version": 99,
            "restore_bounds": {"x": 0, "y": 0, "width": 1280, "height": 800},
            "maximized": false
        }),
    )
    .expect("write unsupported state");
    txn.commit().expect("commit unsupported state");
    assert!(matches!(
        store.load_main_window_state(),
        Err(StorageError::InvalidData(_))
    ));

    let txn = store.db.begin_write().expect("write transaction");
    write_json_in_txn(
        &txn,
        SETTINGS_TABLE,
        SETTINGS_MAIN_WINDOW_STATE,
        &serde_json::json!({
            "version": 1,
            "restore_bounds": {"x": 0, "y": 0, "width": 0, "height": -1},
            "maximized": false
        }),
    )
    .expect("write invalid state");
    txn.commit().expect("commit invalid state");
    assert!(matches!(
        store.load_main_window_state(),
        Err(StorageError::InvalidData(_))
    ));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn portable_snapshot_excludes_and_preserves_device_local_main_window_state() {
    let source_dir = unique_temp_dir("window-state-portable-source");
    let source = ConnectionStore::open(&source_dir).expect("source store");
    source
        .save_main_window_state(&MainWindowState::new(
            None,
            MainWindowBounds {
                x: 2100,
                y: 100,
                width: 1200,
                height: 800,
            },
            true,
        ))
        .expect("save source window state");
    let mut snapshot = source
        .build_raw_portable_snapshot(
            nyaterm_core::PortableSnapshotKind::Backup,
            "source-device",
            "test",
        )
        .expect("build portable snapshot");
    snapshot.recalculate_hash().expect("hash portable snapshot");
    assert!(!snapshot.entities.keys().any(|key| key.contains("window")));

    let target_dir = unique_temp_dir("window-state-portable-target");
    let target = ConnectionStore::open(&target_dir).expect("target store");
    let target_state = MainWindowState::new(
        None,
        MainWindowBounds {
            x: 40,
            y: 60,
            width: 1000,
            height: 700,
        },
        false,
    );
    target
        .save_main_window_state(&target_state)
        .expect("save target window state");
    target
        .apply_raw_portable_snapshot(&snapshot)
        .expect("apply portable snapshot");

    assert_eq!(
        target.load_main_window_state().expect("load target state"),
        Some(target_state)
    );
    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
}

#[test]
fn shared_custom_icons_and_pipeline_survive_sessions_replacement() {
    let dir = unique_temp_dir("custom-icons-pipeline");
    let store = ConnectionStore::open(&dir).expect("store");
    let config: SessionsConfig = serde_json::from_value(serde_json::json!({
        "groups": [],
        "connections": [{"id":"compat", "name":"Compat", "type":"ssh", "host":"example.com",
            "icon":"custom-icon-fixture", "future":{"opaque":true}, "auth":{"mode":"password","future_auth":7}, "sftp":{"pipeline_depth":32,"future_option":{"enabled":true}}}],
        "custom_icons": [{"id":"custom-icon-fixture","name":"Fixture","data_url":"data:image/png;base64,AA==", "created_at_ms":1,"updated_at_ms":2}]
    })).expect("legacy config");
    store.replace_sessions(&config).expect("replace");
    let mut loaded = store.load_sessions().expect("load");
    assert_eq!(loaded.custom_icons, config.custom_icons);
    loaded.connections[0].name = "Renamed".to_string();
    store.save_connection(&loaded.connections[0]).expect("edit");
    let loaded = store.load_sessions().expect("reload");
    assert_eq!(loaded.connections[0].sftp.pipeline_depth, Some(32));
    assert_eq!(
        loaded.connections[0].sftp.extra["future_option"]["enabled"],
        true
    );
    let bytes = serde_json::to_vec(&loaded).expect("export");
    let exported: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(exported["connections"][0]["future"]["opaque"], true);
    assert_eq!(exported["connections"][0]["auth"]["future_auth"], 7);
    let restored: SessionsConfig = serde_json::from_slice(&bytes).expect("import");
    store.replace_sessions(&restored).expect("restore");
    assert_eq!(
        store.load_sessions().expect("restored").custom_icons,
        config.custom_icons
    );
    let mut snapshot = store
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Sync, "fixture", "2.0.0")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    store.apply_raw_portable_snapshot(&snapshot).unwrap();
    let synced = serde_json::to_value(store.load_sessions().unwrap()).unwrap();
    assert_eq!(synced["connections"][0]["future"]["opaque"], true);
    assert_eq!(synced["custom_icons"][0]["id"], "custom-icon-fixture");
    store
        .delete_connection_custom_icon("custom-icon-fixture")
        .expect("delete");
    assert!(
        store
            .list_connection_custom_icons()
            .expect("list")
            .is_empty()
    );
    drop(store);
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn deleting_migrated_shared_icon_clears_legacy_references_without_recreating_it() {
    let dir = unique_temp_dir("legacy-icon-delete");
    let store = ConnectionStore::open(&dir).unwrap();
    let connection: SavedConnection = serde_json::from_value(serde_json::json!({
        "id":"legacy-icon", "name":"Legacy", "type":"ssh", "host":"example.com",
        "icon":"data:image/png;base64,AA==", "future":{"retained":true}
    }))
    .unwrap();
    store.save_connection(&connection).unwrap();
    let loaded = store.load_sessions().unwrap();
    assert_eq!(loaded.custom_icons.len(), 1);
    assert_eq!(
        loaded.connections[0].icon.as_deref(),
        Some(loaded.custom_icons[0].id.as_str())
    );
    store
        .delete_connection_custom_icon(&loaded.custom_icons[0].id)
        .unwrap();
    let loaded = store.load_sessions().unwrap();
    assert!(loaded.custom_icons.is_empty());
    assert!(loaded.connections[0].icon.is_none());
    assert_eq!(
        serde_json::to_value(&loaded.connections[0]).unwrap()["future"]["retained"],
        true
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn editing_keywords_preserves_both_values_of_the_retired_soft_wrap_setting() {
    let dir = unique_temp_dir("keyword-wrap-compatibility");
    let store = ConnectionStore::open(&dir).expect("store");
    for legacy_value in [false, true] {
        store.save_settings_value(&serde_json::json!({
            "terminal": {"keyword_highlights_across_wrapped_lines": legacy_value, "future_keyword_option": "retain"}
        })).expect("legacy settings");
        let mut config = store.load_keyword_highlights().expect("load");
        config.enabled = true;
        let saved = store.save_keyword_highlights(&config).expect("save");
        assert_eq!(saved.across_wrapped_lines, legacy_value);
        let reloaded = store.load_settings_value().expect("reload");
        assert_eq!(
            reloaded["terminal"]["keyword_highlights_across_wrapped_lines"],
            legacy_value
        );
        assert_eq!(reloaded["terminal"]["future_keyword_option"], "retain");
    }
    drop(store);
    std::fs::remove_dir_all(dir).expect("cleanup");
}
