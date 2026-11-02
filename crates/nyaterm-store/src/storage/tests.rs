use aes_gcm::{Aes256Gcm, Key, KeyInit, aead::Aead};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_core::{
    AccountAuthError, AiExecutionProfile, AssetAccelerator, AssetAcceleratorType, AssetDeviceType,
    AssetMetadata, CloudSyncSettings, CloudSyncState, CommandHistoryEntry, ConnectionAuth,
    ConnectionType, ExistingFileBehavior, InternalEditorDisplay, MainWindowBounds, MainWindowState,
    OtpEntry, PortableSnapshotKind, RecordingMode, RecordingRotationPolicy, SavedCredential,
    SearchEngineConfig, SshKey, TerminalRightClickAction, export_quick_commands_json,
};

#[test]
fn theme_commits_are_atomic_and_preserve_unrelated_settings_and_unknown_fields() {
    use crate::storage::ThemeCommitOutcome;
    use nyaterm_core::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};
    let dir = unique_temp_dir("theme-only-commit");
    let store = ConnectionStore::open(&dir).unwrap();
    let mut theme = nyaterm_core::themes::builtins()[0].copy_as_custom();
    theme
        .extra
        .insert("future".into(), serde_json::json!({"version": 99}));
    let id = theme.id.clone();
    store
        .save_settings_value(&serde_json::json!({
            "appearance": {"font_size": 19, "future": 42, "custom_themes": [{"unsupported": true}]},
            "future": {"keep": true}
        }))
        .unwrap();
    let base = ThemeSettings::from(&store.load_app_settings_summary().unwrap());
    let mutation = ThemeMutation::Save {
        theme: Box::new(theme),
        target: Some(ThemeApplyTarget::BothFollowUi),
    };
    let ThemeCommitOutcome::Committed(commit) =
        store.commit_theme_mutation(&base, &mutation).unwrap()
    else {
        panic!("unexpected conflict");
    };
    let raw = store.load_settings_value().unwrap();
    assert_eq!(raw["appearance"]["font_size"], 19);
    assert_eq!(raw["appearance"]["future"], 42);
    assert_eq!(raw["future"], serde_json::json!({"keep": true}));
    assert_eq!(
        raw["appearance"]["custom_themes"][0],
        serde_json::json!({"unsupported": true})
    );
    assert_eq!(
        raw["appearance"]["custom_themes"][1]["future"],
        serde_json::json!({"version": 99})
    );
    assert_eq!(raw["appearance"]["theme"], id);
    assert!(raw["appearance"]["terminal_theme"].is_null());
    assert!(matches!(
        store.commit_theme_mutation(&base, &mutation).unwrap(),
        ThemeCommitOutcome::Conflict
    ));
    assert_eq!(store.load_settings_value().unwrap(), raw);
    let mut invalid = commit.settings.catalog.resolve(&id).unwrap();
    invalid.colors.primary = "invalid".into();
    assert!(
        store
            .commit_theme_mutation(
                &commit.settings,
                &ThemeMutation::Save {
                    theme: Box::new(invalid),
                    target: Some(ThemeApplyTarget::Ui)
                }
            )
            .is_err()
    );
    assert_eq!(store.load_settings_value().unwrap(), raw);
    let ThemeCommitOutcome::Committed(deleted) = store
        .commit_theme_mutation(&commit.settings, &ThemeMutation::Delete { id })
        .unwrap()
    else {
        panic!("unexpected conflict");
    };
    assert_eq!(deleted.settings.ui, "github-dark");
    assert!(deleted.settings.catalog.available().is_empty());
    assert_eq!(
        store.load_settings_value().unwrap()["appearance"]["custom_themes"],
        serde_json::json!([{"unsupported": true}])
    );
}

#[test]
fn applying_themes_preserves_an_unsupported_catalog_and_concurrent_unrelated_edits() {
    use crate::storage::ThemeCommitOutcome;
    use nyaterm_core::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};
    let dir = unique_temp_dir("theme-only-future");
    let store = ConnectionStore::open(&dir).unwrap();
    let future = serde_json::json!({"format": 99});
    store
        .save_settings_value(&serde_json::json!({"appearance": {"custom_themes": future}}))
        .unwrap();
    let base = ThemeSettings::from(&store.load_app_settings_summary().unwrap());
    let mut raw = store.load_settings_value().unwrap();
    raw["appearance"]["font_size"] = serde_json::json!(25);
    store.save_settings_value(&raw).unwrap();
    assert!(matches!(
        store
            .commit_theme_mutation(
                &base,
                &ThemeMutation::Apply {
                    id: "dracula".into(),
                    target: ThemeApplyTarget::Ui
                }
            )
            .unwrap(),
        ThemeCommitOutcome::Committed(_)
    ));
    let raw = store.load_settings_value().unwrap();
    assert_eq!(raw["appearance"]["custom_themes"], future);
    assert_eq!(raw["appearance"]["font_size"], 25);
}

#[test]
fn custom_themes_preserve_tauri_records_and_unknown_data_when_saving_appearance() {
    let dir = unique_temp_dir("custom-themes-compatibility");
    let store = ConnectionStore::open(&dir).expect("store");
    let theme = nyaterm_core::themes::builtins()[0].copy_as_custom();
    let mut raw = serde_json::to_value(&theme).unwrap();
    raw["future"] = serde_json::json!({"keep":true});
    raw["colors"]["terminal"]["future"] = serde_json::json!("keep");
    raw["colors"]["terminal"]
        .as_object_mut()
        .unwrap()
        .remove("foregroundIntense");
    let unsupported = serde_json::json!({"id":"future-theme", "format":99});
    store
        .save_settings_value(&serde_json::json!({"appearance":{
            "theme":theme.id, "custom_themes":[raw,unsupported], "future":42
        }}))
        .unwrap();
    let mut summary = store.load_app_settings_summary().unwrap();
    assert!(summary.custom_themes.resolve(&theme.id).is_some());
    summary.ui_font_size = 18;
    store.save_appearance_settings(&summary).unwrap();
    let mut edited = summary.custom_themes.resolve(&theme.id).unwrap();
    edited.colors.primary = "#123456".into();
    summary.custom_themes.upsert(edited).unwrap();
    store.save_appearance_settings(&summary).unwrap();
    let result = store.load_settings_value().unwrap();
    assert_eq!(
        result["appearance"]["custom_themes"][0]["future"],
        serde_json::json!({"keep":true})
    );
    assert_eq!(
        result["appearance"]["custom_themes"][0]["colors"]["terminal"]["future"],
        "keep"
    );
    assert_eq!(result["appearance"]["custom_themes"][1], unsupported);
    assert_eq!(result["appearance"]["future"], 42);
    let before = result.clone();
    summary.custom_themes.0[0]["colors"]["primary"] = serde_json::json!("invalid");
    assert!(store.save_appearance_settings(&summary).is_err());
    assert_eq!(store.load_settings_value().unwrap(), before);
    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn unsupported_catalog_shape_survives_unrelated_appearance_edits() {
    let dir = unique_temp_dir("future-theme-catalog");
    let store = ConnectionStore::open(&dir).unwrap();
    let future = serde_json::json!({"version":2,"records":[]});
    store
        .save_settings_value(&serde_json::json!({"appearance":{"custom_themes":future}}))
        .unwrap();
    let mut summary = store.load_app_settings_summary().unwrap();
    summary.ui_font_size = 19;
    store.save_appearance_settings(&summary).unwrap();
    assert_eq!(
        store.load_settings_value().unwrap()["appearance"]["custom_themes"],
        future
    );
    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn settings_migrate_right_click_and_lock_keys_without_losing_unknown_fields() {
    let dir = unique_temp_dir("settings-interaction-lock-migration");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_value(&serde_json::json!({
            "interaction": { "right_click_paste": true, "future_option": { "keep": true } },
            "security": { "enable_screen_lock": true, "future_option": 42 },
            "transfer": { "future_option": "retain" }
        }))
        .expect("seed legacy settings");

    let legacy = store.load_app_settings_summary().expect("load legacy");
    assert_eq!(
        legacy.interaction_terminal_right_click_action,
        TerminalRightClickAction::Paste
    );
    assert!(legacy.enable_startup_lock);
    assert!(legacy.enable_idle_lock);
    assert_eq!(
        legacy.transfer_internal_editor_display,
        InternalEditorDisplay::Window
    );

    let mut raw = store.load_settings_value().expect("raw");
    raw["interaction"]["terminal_right_click_action"] = serde_json::json!("none");
    raw["security"]["enable_startup_lock"] = serde_json::json!(false);
    raw["security"]["enable_idle_lock"] = serde_json::json!(true);
    store
        .save_settings_value(&raw)
        .expect("seed mixed settings");
    let mixed = store.load_app_settings_summary().expect("load mixed");
    assert_eq!(
        mixed.interaction_terminal_right_click_action,
        TerminalRightClickAction::None
    );
    assert!(!mixed.enable_startup_lock);
    assert!(mixed.enable_idle_lock);

    let mut updated = mixed;
    updated.interaction_terminal_right_click_action = TerminalRightClickAction::Menu;
    updated.interaction_mouse_events_require_alt = true;
    updated.enable_startup_lock = true;
    updated.enable_idle_lock = false;
    updated.transfer_internal_editor_display = InternalEditorDisplay::Workspace;
    store
        .save_interaction_settings(&updated)
        .expect("save interaction");
    store
        .save_screen_lock_settings(&updated)
        .expect("save locks");
    let saved = store
        .save_transfer_settings(&updated)
        .expect("save transfer");
    assert_eq!(
        saved.interaction_terminal_right_click_action,
        TerminalRightClickAction::Menu
    );
    assert!(saved.interaction_mouse_events_require_alt);
    assert!(saved.enable_startup_lock);
    assert!(!saved.enable_idle_lock);
    assert_eq!(
        saved.transfer_internal_editor_display,
        InternalEditorDisplay::Workspace
    );

    let raw = store.load_settings_value().expect("raw saved");
    assert_eq!(raw["interaction"]["right_click_paste"], false);
    assert_eq!(raw["security"]["enable_screen_lock"], true);
    assert_eq!(raw["interaction"]["future_option"]["keep"], true);
    assert_eq!(raw["security"]["future_option"], 42);
    assert_eq!(raw["transfer"]["future_option"], "retain");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn new_settings_survive_portable_snapshot_and_native_backup() {
    let source_dir = unique_temp_dir("settings-new-source");
    let portable_dir = unique_temp_dir("settings-new-portable");
    let native_dir = unique_temp_dir("settings-new-native");
    let backup_dir = unique_temp_dir("settings-new-backup");
    let backup_path = backup_dir.join("settings.redb");
    let source = ConnectionStore::open(&source_dir).expect("source");
    source
        .save_settings_value(&serde_json::json!({
            "interaction": {
                "terminal_right_click_action": "none",
                "right_click_paste": true,
                "mouse_events_require_alt": true,
                "future_option": "keep"
            },
            "security": {
                "enable_startup_lock": false,
                "enable_idle_lock": true,
                "enable_screen_lock": false,
                "idle_lock_minutes": 15
            },
            "transfer": { "internal_editor_display": "workspace" }
        }))
        .expect("seed");
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "device", "test")
        .expect("build portable snapshot");
    snapshot.recalculate_hash().expect("hash snapshot");
    let target = ConnectionStore::open(&portable_dir).expect("portable target");
    target
        .apply_raw_portable_snapshot(&snapshot)
        .expect("import portable");
    assert_new_settings_loaded(&target);
    drop(target);
    drop(source);

    ConnectionStore::export_config_database(&source_dir, None, &backup_path)
        .expect("export native backup");
    ConnectionStore::import_config_database(&native_dir, None, &backup_path)
        .expect("import native backup");
    let native = ConnectionStore::open(&native_dir).expect("native target");
    assert_new_settings_loaded(&native);
    drop(native);
    for dir in [source_dir, portable_dir, native_dir, backup_dir] {
        std::fs::remove_dir_all(dir).ok();
    }
}

fn assert_new_settings_loaded(store: &ConnectionStore) {
    let settings = store.load_app_settings_summary().expect("load settings");
    assert_eq!(
        settings.interaction_terminal_right_click_action,
        TerminalRightClickAction::None
    );
    assert!(!settings.interaction_right_click_paste);
    assert!(settings.interaction_mouse_events_require_alt);
    assert!(!settings.enable_startup_lock);
    assert!(settings.enable_idle_lock);
    assert_eq!(
        settings.transfer_internal_editor_display,
        InternalEditorDisplay::Workspace
    );
    assert_eq!(
        store.load_settings_value().expect("raw")["interaction"]["future_option"],
        "keep"
    );
}
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
fn proxy_ciphertext_round_trips_and_keeps_released_backup_password_contract() {
    let dir = TestTempDir::new("nyaterm-proxy-encryption-contract");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store
        .save_master_password(Some("fixture-master-password"))
        .unwrap();
    let key = entity_key(super::PROXY_PREFIX, "proxy");
    let txn = store.db.begin_write().unwrap();
    write_json_in_txn(&txn, super::PROXIES_TABLE, &key, &serde_json::json!({"id":"proxy","name":"Proxy","password":"legacy-fixture-password","future":{"keep":true}})).unwrap();
    txn.commit().unwrap();
    let legacy = store.list_proxies().unwrap();
    assert_eq!(
        legacy[0].password.as_deref(),
        Some("legacy-fixture-password")
    );
    store.replace_proxies(&legacy).unwrap();
    let raw: serde_json::Value = store
        .read_json_table(super::PROXIES_TABLE, &key)
        .unwrap()
        .unwrap();
    assert_ne!(raw["password"], "legacy-fixture-password");
    assert_eq!(
        store.list_proxies().unwrap()[0].password.as_deref(),
        Some("legacy-fixture-password")
    );
    let backup = store
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Backup, "device", "test")
        .unwrap();
    let proxies: serde_json::Value = serde_json::from_str(&backup.entities["proxies"]).unwrap();
    assert_eq!(proxies[0]["password"], "legacy-fixture-password");
    let mut corrupted = raw;
    corrupted["password"] = serde_json::json!(format!("{}invalid", super::PROXY_SECRET_PREFIX));
    let txn = store.db.begin_write().unwrap();
    write_json_in_txn(&txn, super::PROXIES_TABLE, &key, &corrupted).unwrap();
    txn.commit().unwrap();
    assert!(store.list_proxies().is_err());
}

#[test]
fn accounts_preserve_masking_legacy_references_and_portable_snapshot_contracts() {
    use nyaterm_core::models::credentials::{ConnectionPasswordSource, SavedPassword};
    let source_dir = unique_temp_dir("account-portable-source");
    let target_dir = unique_temp_dir("account-portable-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    source
        .save_password(SavedPassword {
            sort_order: 0,
            id: "account".into(),
            name: "Shared account".into(),
            username: "shared-user".into(),
            password: Some("synthetic-secret".into()),
            has_password: false,
        })
        .unwrap();
    let connection: SavedConnection = serde_json::from_value(serde_json::json!({
        "id":"account-connection", "name":"SSH", "type":"ssh", "host":"test.invalid",
        "auth":{"mode":"password", "account_id":"account", "password_id":"legacy-id", "password_source":"account", "future_auth":true}
    })).unwrap();
    source.save_connection(&connection).unwrap();
    let masked = source.list_passwords().unwrap();
    assert_eq!(masked[0].username, "shared-user");
    assert!(masked[0].password.is_none());
    assert!(masked[0].has_password);
    let mut snapshot = source
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Backup, "device", "test")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    let encoded = crate::encode_raw_portable_snapshot(&snapshot).unwrap();
    let decoded = crate::decode_raw_portable_snapshot(&encoded).unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    target.apply_raw_portable_snapshot(&decoded).unwrap();
    let auth = target
        .load_sessions()
        .unwrap()
        .connections
        .remove(0)
        .auth
        .unwrap();
    assert_eq!(auth.password_id.as_deref(), Some("legacy-id"));
    assert_eq!(auth.account_id.as_deref(), Some("account"));
    assert_eq!(
        auth.password_source,
        Some(ConnectionPasswordSource::Account)
    );
    let loaded = target.load_account_for_auth(&auth).unwrap().unwrap();
    assert_eq!(loaded.username, "shared-user");
    assert_eq!(loaded.password.as_deref(), Some("synthetic-secret"));
    target.delete_password("account").unwrap();
    assert!(target.load_account_for_auth(&auth).unwrap().is_none());
    let retained = target
        .load_sessions()
        .unwrap()
        .connections
        .remove(0)
        .auth
        .unwrap();
    assert_eq!(retained.account_id.as_deref(), Some("account"));
    assert_eq!(retained.password_id.as_deref(), Some("legacy-id"));
    drop(target);
    drop(source);
    std::fs::remove_dir_all(target_dir).unwrap();
    std::fs::remove_dir_all(source_dir).unwrap();
}

#[test]
fn connection_source_reads_account_username_without_decrypting_corrupt_password() {
    use nyaterm_core::models::credentials::{ConnectionPasswordSource, SavedPassword};
    let dir = unique_temp_dir("account-metadata-only");
    let store = ConnectionStore::open(&dir).unwrap();
    let txn = store.db.begin_write().unwrap();
    write_json_in_txn(
        &txn,
        CREDENTIALS_TABLE,
        &entity_key(super::PASSWORD_PREFIX, "metadata"),
        &SavedPassword {
            sort_order: 0,
            id: "metadata".into(),
            name: "Metadata".into(),
            username: "metadata-user".into(),
            password: Some("unsupported-encrypted-payload".into()),
            has_password: false,
        },
    )
    .unwrap();
    txn.commit().unwrap();
    let auth = ConnectionAuth {
        mode: "password".into(),
        account_id: Some("metadata".into()),
        password_source: Some(ConnectionPasswordSource::Connection),
        ..ConnectionAuth::default()
    };
    let loaded = store.load_account_for_auth(&auth).unwrap().unwrap();
    assert_eq!(loaded.username, "metadata-user");
    assert!(loaded.password.is_none());
    assert_eq!(
        auth.resolve_account_auth("fallback", Some(&loaded))
            .unwrap()
            .1,
        None
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_account_records_default_username_and_do_not_write_it_when_empty() {
    let dir = unique_temp_dir("account-legacy-default");
    let store = ConnectionStore::open(&dir).unwrap();
    let txn = store.db.begin_write().unwrap();
    write_json_in_txn(
        &txn,
        CREDENTIALS_TABLE,
        &entity_key(super::PASSWORD_PREFIX, "legacy"),
        &serde_json::json!({
            "id":"legacy", "name":"Legacy", "password":null
        }),
    )
    .unwrap();
    txn.commit().unwrap();
    let entry = store.load_password_by_id("legacy").unwrap().unwrap();
    assert!(entry.username.is_empty());
    store.save_password(entry).unwrap();
    let entry = store.load_password_by_id("legacy").unwrap().unwrap();
    assert!(
        serde_json::to_value(entry)
            .unwrap()
            .get("username")
            .is_none()
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tags_dual_write_preserve_asset_facts_and_survive_portable_snapshot_and_clear() {
    let source_dir = unique_temp_dir("tags-portable-source");
    let target_dir = unique_temp_dir("tags-portable-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    let txn = source.db.begin_write().unwrap();
    write_json_in_txn(
        &txn,
        CONNECTIONS_TABLE,
        &entity_key("connections/", "tagged"),
        &serde_json::json!({
            "id":"tagged", "name":"Tagged", "type":"ssh", "host":"test.invalid",
            "asset":{"tags":["legacy"], "os_name":"Linux", "future_asset":true}
        }),
    )
    .unwrap();
    txn.commit().unwrap();
    let mut connection = source.get_connection("tagged").unwrap().unwrap();
    assert_eq!(connection.tags, ["legacy"]);
    connection.tags = vec!["production".into(), "gpu".into()];
    source.save_connection(&connection).unwrap();
    let raw: serde_json::Value = {
        let txn = source.db.begin_read().unwrap();
        let table = txn.open_table(CONNECTIONS_TABLE).unwrap();
        let record = table
            .get(entity_key("connections/", "tagged").as_str())
            .unwrap()
            .unwrap();
        deserialize_json(record.value()).unwrap()
    };
    assert_eq!(raw["asset"]["tags"], raw["tags"]);
    assert_eq!(raw["asset"]["future_asset"], true);
    let mut snapshot = source
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Backup, "device", "test")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    target.apply_raw_portable_snapshot(&snapshot).unwrap();
    let mut connection = target.get_connection("tagged").unwrap().unwrap();
    assert_eq!(connection.tags, ["production", "gpu"]);
    assert_eq!(
        connection.asset.as_ref().unwrap().os_name.as_deref(),
        Some("Linux")
    );
    connection.tags.clear();
    target.save_connection(&connection).unwrap();
    target.mark_connection_used("tagged").unwrap();
    target
        .merge_connection_asset_from_monitoring(
            "tagged",
            AssetMetadata {
                hostname: Some("node".into()),
                ..AssetMetadata::default()
            },
        )
        .unwrap();
    assert!(
        target
            .get_connection("tagged")
            .unwrap()
            .unwrap()
            .tags
            .is_empty()
    );
    drop(target);
    drop(source);
    std::fs::remove_dir_all(target_dir).unwrap();
    std::fs::remove_dir_all(source_dir).unwrap();
}

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
            tags: Vec::new(),
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
            .is_some_and(|auth| !auth.has_password)
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn exports_and_imports_native_redb_backup() {
    let source_dir = unique_temp_dir("backup-source");
    let target_dir = unique_temp_dir("backup-target");
    let backup_dir = unique_temp_dir("backup-output");
    let backup_path = backup_dir.join("nyaterm.redb");
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
            tags: Vec::new(),
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
    let snapshot_dir = unique_temp_dir("portable-output");
    let snapshot_path = snapshot_dir.join("nyaterm.nya");

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
                tags: Vec::new(),
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
                sort_order: 0,
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
        serde_json::json!(["network", "gpuMonitor", "ascendNpuMonitor"])
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
fn sync_snapshot_preserves_local_secrets_history_device_settings_and_unknown_fields() {
    let source_dir = unique_temp_dir("sync-projection-source");
    let target_dir = unique_temp_dir("sync-projection-target");
    let source = ConnectionStore::open(&source_dir).expect("source");
    let target = ConnectionStore::open(&target_dir).expect("target");
    source
        .save_settings_value(&serde_json::json!({
            "general":{"startup_behavior":"restore"},
            "security":{"master_password":"source-secret"},
            "cloud_sync":{"provider":"webdav","password":"source-cloud-secret"},
            "ui":{"language":"en-US","left_width":410},
            "future_source":{"value":1}
        }))
        .expect("source settings");
    target
        .save_settings_value(&serde_json::json!({
            "general":{"startup_behavior":"empty"},
            "security":{"master_password":"local-secret"},
            "cloud_sync":{"provider":"s3","password":"local-cloud-secret"},
            "ui":{"language":"zh-CN","left_width":260},
            "future_local":{"value":2}
        }))
        .expect("target settings");
    source
        .append_command_history("remote-command")
        .expect("source history");
    target
        .append_command_history("local-command")
        .expect("target history");
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "2.0.0")
        .expect("build sync snapshot");
    snapshot.recalculate_hash().expect("hash snapshot");
    let exported_settings: serde_json::Value =
        serde_json::from_str(&snapshot.entities["settings"]).expect("settings entity");
    assert!(exported_settings.get("cloud_sync").is_none());
    assert!(
        exported_settings["security"]
            .get("master_password")
            .is_none()
    );
    assert!(exported_settings["ui"].get("left_width").is_none());
    assert_eq!(snapshot.entities["history"], "[]");

    target
        .apply_raw_portable_snapshot(&snapshot)
        .expect("apply sync");
    let restored = target.load_settings_value().expect("restored settings");
    assert_eq!(restored["general"]["startup_behavior"], "restore");
    assert_eq!(restored["ui"]["language"], "en-US");
    assert_eq!(restored["ui"]["left_width"], 260);
    assert_eq!(restored["security"]["master_password"], "local-secret");
    assert_eq!(restored["cloud_sync"]["password"], "local-cloud-secret");
    assert_eq!(restored["future_local"]["value"], 2);
    assert_eq!(restored["future_source"]["value"], 1);
    assert_eq!(target.list_command_history(10).expect("history").len(), 1);
    assert_eq!(
        target.list_command_history(10).expect("history")[0].command,
        "local-command"
    );
}

#[test]
fn encrypted_portable_snapshot_requires_master_password() {
    let source_dir = unique_temp_dir("portable-encrypted-source");
    let target_dir = unique_temp_dir("portable-encrypted-target");
    let wrong_target_dir = unique_temp_dir("portable-encrypted-wrong-target");
    let snapshot_dir = unique_temp_dir("portable-encrypted-output");
    let snapshot_path = snapshot_dir.join("nyaterm.nya");

    let source_store = ConnectionStore::open(&source_dir).expect("source store");
    source_store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![SavedConnection {
                extensions: Default::default(),
                tags: Vec::new(),
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
    assert!(
        source_store
            .export_encrypted_portable_snapshot_from_open_store(&snapshot_path, "dev", "1", "",)
            .is_err()
    );

    source_store
        .export_encrypted_portable_snapshot_from_open_store(&snapshot_path, "dev", "1", "secret")
        .expect("export encrypted snapshot from open store");
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
                tags: Vec::new(),
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

    let target_store = ConnectionStore::open(&target_dir).expect("open target store");
    let import = target_store
        .import_encrypted_portable_snapshot_into_open_store(&snapshot_path, "secret")
        .expect("import encrypted snapshot into open store");
    let imported = target_store.load_sessions().expect("load imported");
    assert_eq!(imported.connections[0].name, "Encrypted Snapshot");
    let safety_backup_path = import
        .safety_backup_path
        .expect("encrypted safety backup path");
    assert_eq!(
        safety_backup_path
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("nya")
    );
    let safety_snapshot = crate::decode_encrypted_raw_portable_snapshot(
        &std::fs::read(&safety_backup_path).expect("read safety backup"),
        "secret",
    )
    .expect("decode encrypted safety backup");
    let safety_sessions: SessionsConfig = serde_json::from_str(
        safety_snapshot
            .entities
            .get("sessions")
            .expect("safety sessions entity"),
    )
    .expect("decode safety sessions");
    assert!(safety_sessions.connections.is_empty());

    drop(source_store);
    drop(target_store);

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(wrong_target_dir).ok();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::remove_dir_all(parent).ok();
    }
}

#[test]
fn legacy_tauri_snapshot_reencrypts_settings_and_rewraps_master_key() {
    const SNAPSHOT_PASSWORD: &str = "legacy-snapshot-password";
    const AI_SECRET: &str = "synthetic-ai-secret";
    const CLOUD_SECRET: &str = "synthetic-cloud-secret";
    const VAULT_SECRET: &str = "synthetic-vault-secret";

    let source_dir = unique_temp_dir("legacy-tauri-portable-source");
    let snapshot_dir = unique_temp_dir("legacy-tauri-portable-output");
    let snapshot_path = snapshot_dir.join("legacy.nya");
    let source = ConnectionStore::open(&source_dir).expect("source store");
    source
        .save_master_password(Some(SNAPSHOT_PASSWORD))
        .expect("set source master password");

    let mut ai = AiSettings::default();
    ai.provider_profiles
        .first_mut()
        .expect("default AI profile")
        .api_key = Some(AI_SECRET.into());
    source
        .save_ai_settings(ai)
        .expect("save source AI settings");

    let mut cloud = CloudSyncSettings::default();
    cloud.webdav.password = Some(CLOUD_SECRET.into());
    source
        .save_cloud_sync_settings(cloud)
        .expect("save source cloud settings");
    source
        .save_password(nyaterm_core::SavedPassword {
            sort_order: 0,
            id: "legacy-account".to_string(),
            name: "Legacy account".to_string(),
            username: "legacy-user".to_string(),
            password: Some(VAULT_SECRET.into()),
            has_password: false,
        })
        .expect("save source vault secret");

    let source_master_key = source
        .load_master_key_token()
        .expect("load source master key")
        .expect("source master key exists");
    let mut snapshot = source
        .build_raw_portable_snapshot(
            nyaterm_core::PortableSnapshotKind::Backup,
            "legacy-device",
            "1.2.10",
        )
        .expect("build source snapshot");
    let mut settings: serde_json::Value =
        serde_json::from_str(snapshot.entities.get("settings").expect("settings entity"))
            .expect("decode settings entity");
    settings["ai"] = serde_json::to_value(source.load_ai_settings().expect("load plaintext AI"))
        .expect("encode plaintext AI");
    settings["cloud_sync"] = serde_json::to_value(
        source
            .load_cloud_sync_settings()
            .expect("load plaintext cloud settings"),
    )
    .expect("encode plaintext cloud settings");
    settings["ai"]["future_ai_field"] = serde_json::json!("preserved");
    settings["cloud_sync"]["future_cloud_field"] = serde_json::json!("preserved");
    snapshot.entities.insert(
        "settings".to_string(),
        serde_json::to_string(&settings).expect("encode legacy settings"),
    );
    snapshot.recalculate_hash().expect("hash legacy snapshot");
    let encoded = crate::encode_encrypted_raw_portable_snapshot(&snapshot, SNAPSHOT_PASSWORD)
        .expect("encrypt legacy snapshot");
    std::fs::create_dir_all(snapshot_path.parent().expect("snapshot parent"))
        .expect("create snapshot parent");
    std::fs::write(&snapshot_path, encoded).expect("write legacy snapshot");

    for target_password in [None, Some("different-target-password")] {
        let target_dir = unique_temp_dir(if target_password.is_some() {
            "legacy-tauri-portable-password-target"
        } else {
            "legacy-tauri-portable-fallback-target"
        });
        let target = ConnectionStore::open(&target_dir).expect("target store");
        if let Some(target_password) = target_password {
            target
                .save_master_password(Some(target_password))
                .expect("set target master password");
        }

        target
            .import_encrypted_portable_snapshot_into_open_store(&snapshot_path, SNAPSHOT_PASSWORD)
            .expect("import legacy snapshot");

        let loaded_ai = target
            .load_ai_settings()
            .expect("load imported AI settings");
        assert_eq!(
            loaded_ai.provider_profiles[0].api_key.as_deref(),
            Some(AI_SECRET)
        );
        let loaded_cloud = target
            .load_cloud_sync_settings()
            .expect("load imported cloud settings");
        assert_eq!(loaded_cloud.webdav.password.as_deref(), Some(CLOUD_SECRET));
        let loaded_password = target
            .load_decrypted_password_by_id("legacy-account")
            .expect("load imported account")
            .expect("imported account exists");
        assert_eq!(loaded_password.password.as_deref(), Some(VAULT_SECRET));

        let stored_settings = target.load_settings_value().expect("raw imported settings");
        let stored_settings_text =
            serde_json::to_string(&stored_settings).expect("encode raw imported settings");
        assert!(!stored_settings_text.contains(AI_SECRET));
        assert!(!stored_settings_text.contains(CLOUD_SECRET));
        assert_eq!(stored_settings["ai"]["future_ai_field"], "preserved");
        assert_eq!(
            stored_settings["cloud_sync"]["future_cloud_field"],
            "preserved"
        );
        assert_eq!(
            target
                .load_app_settings_summary()
                .expect("target settings summary")
                .has_master_password,
            target_password.is_some()
        );
        assert_ne!(
            target
                .load_master_key_token()
                .expect("load target master key")
                .expect("target master key exists"),
            source_master_key
        );

        drop(target);
        std::fs::remove_dir_all(target_dir).ok();
    }

    drop(source);
    std::fs::remove_dir_all(source_dir).ok();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::remove_dir_all(parent).ok();
    }
}

#[test]
fn legacy_tauri_cloud_pull_rewraps_vault_key_and_preserves_local_master_password() {
    let source_dir = unique_temp_dir("legacy-cloud-source");
    let target_dir = unique_temp_dir("legacy-cloud-target");
    let source = ConnectionStore::open(&source_dir).expect("source");
    source
        .save_master_password(Some("shared-cloud-password"))
        .expect("source master password");
    source
        .save_password(nyaterm_core::SavedPassword {
            sort_order: 0,
            id: "legacy-secret".into(),
            name: "Legacy".into(),
            username: "user".into(),
            password: Some("vault-value".into()),
            has_password: false,
        })
        .expect("source password");
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "1.2.10")
        .expect("legacy sync snapshot");
    snapshot.recalculate_hash().expect("hash");

    let target = ConnectionStore::open(&target_dir).expect("target");
    target
        .save_master_password(Some("different-local-password"))
        .expect("target master password");
    target
        .apply_cloud_sync_snapshot(&target_dir, &snapshot, "shared-cloud-password")
        .expect("apply legacy cloud snapshot");
    let imported = target
        .load_decrypted_password_by_id("legacy-secret")
        .expect("decrypt imported password")
        .expect("imported password");
    assert_eq!(imported.password.as_deref(), Some("vault-value"));
    assert!(
        target
            .verify_master_password("different-local-password")
            .expect("verify local password")
    );
}

#[test]
fn current_gpui_cloud_pull_rewraps_vault_key_for_local_master_password() {
    let source_dir = unique_temp_dir("gpui-cloud-source");
    let target_dir = unique_temp_dir("gpui-cloud-target");
    let source = ConnectionStore::open(&source_dir).expect("source");
    source
        .save_master_password(Some("source-password"))
        .expect("source master password");
    source
        .save_password(nyaterm_core::SavedPassword {
            sort_order: 0,
            id: "shared-account".into(),
            name: "Shared".into(),
            username: "user".into(),
            password: Some("shared-value".into()),
            has_password: false,
        })
        .expect("source password");
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "2.0.0")
        .expect("GPUI sync snapshot");
    snapshot.recalculate_hash().expect("hash");

    let target = ConnectionStore::open(&target_dir).expect("target");
    target
        .save_master_password(Some("target-password"))
        .expect("target master password");
    target
        .apply_cloud_sync_snapshot(&target_dir, &snapshot, "source-password")
        .expect("apply GPUI cloud snapshot");
    assert_eq!(
        target
            .load_decrypted_password_by_id("shared-account")
            .expect("decrypt imported password")
            .expect("imported password")
            .password
            .as_deref(),
        Some("shared-value")
    );
    assert!(
        target
            .verify_master_password("target-password")
            .expect("verify target password")
    );
}

#[test]
fn cloud_pull_reencrypts_device_secrets_across_restart_and_second_sync() {
    use crate::runtime::{LoadBootstrap, StoreRequest};

    for version in ["2.0.0-preview.5", "1.2.10"] {
        for legacy in [false, true] {
            for target_password in ["shared-password", "device-password"] {
                let source_dir = unique_temp_dir("rekey-cloud-source");
                let target_dir = unique_temp_dir("rekey-cloud-target");
                let source = ConnectionStore::open(&source_dir).unwrap();
                source
                    .save_master_password(Some("shared-password"))
                    .unwrap();
                source
                    .save_password(nyaterm_core::SavedPassword {
                        id: "synced-account".into(),
                        password: Some("synced-value".into()),
                        name: "Synced account".into(),
                        username: "user".into(),
                        sort_order: 0,
                        has_password: false,
                    })
                    .unwrap();
                let mut snapshot = source
                    .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", version)
                    .unwrap();
                snapshot.recalculate_hash().unwrap();
                let wire =
                    crate::encode_encrypted_raw_portable_snapshot(&snapshot, "shared-password")
                        .unwrap();
                let snapshot =
                    crate::decode_encrypted_raw_portable_snapshot(&wire, "shared-password")
                        .unwrap();
                let target = ConnectionStore::open(&target_dir).unwrap();
                target.save_master_password(Some(target_password)).unwrap();
                let mut local = CloudSyncSettings {
                    enabled: true,
                    provider: "webdav".into(),
                    device_name: "device-B".into(),
                    ..CloudSyncSettings::default()
                };
                local.webdav.password = Some("local-webdav".into());
                local.s3.access_key_id = Some("local-s3-id".into());
                local.s3.secret_access_key = Some("local-s3-key".into());
                local.s3.session_token = Some("local-s3-token".into());
                local.google_drive.access_token = Some("local-google-access".into());
                local.google_drive.refresh_token = Some("local-google-refresh".into());
                local.google_drive.client_secret = Some("local-google-client".into());
                local.onedrive.access_token = Some("local-ms-access".into());
                local.onedrive.refresh_token = Some("local-ms-refresh".into());
                local.onedrive.client_secret = Some("local-ms-client".into());
                local.aliyun_drive.access_token = Some("local-aliyun-access".into());
                local.aliyun_drive.refresh_token = Some("local-aliyun-refresh".into());
                local.aliyun_drive.client_secret = Some("local-aliyun-client".into());
                local.gitee_snippet.access_token = Some("local-gitee".into());
                local.github_gist.access_token = Some("local-github".into());
                target.save_cloud_sync_settings(local.clone()).unwrap();
                let old_key = target.load_master_key_token().unwrap().unwrap();
                let mut raw = target.load_settings_value().unwrap();
                raw["cloud_sync"]["future_policy"] = serde_json::json!({"retain":true});
                raw["cloud_sync"]["webdav"]["future_option"] = serde_json::json!(42);
                let old_ciphertext = raw["cloud_sync"]["webdav"]["password"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                if legacy {
                    target
                        .save_settings_doc_value(SETTINGS_CLOUD_SYNC, &raw["cloud_sync"])
                        .unwrap();
                    raw.as_object_mut().unwrap().remove("cloud_sync");
                }
                target.save_settings_value(&raw).unwrap();

                target
                    .apply_cloud_sync_snapshot(&target_dir, &snapshot, "shared-password")
                    .unwrap();
                assert_eq!(target.load_cloud_sync_settings().unwrap(), local);
                assert_eq!(
                    LoadBootstrap.execute(&target).unwrap().cloud_sync_settings,
                    local
                );
                let new_key = target.load_master_key_token().unwrap().unwrap();
                let saved = target.load_settings_value().unwrap();
                let new_ciphertext = saved["cloud_sync"]["webdav"]["password"].as_str().unwrap();
                assert_ne!(old_key, new_key);
                assert_ne!(old_ciphertext, new_ciphertext);
                assert_ne!(new_ciphertext, "local-webdav");
                assert!(
                    target
                        .credential_crypto()
                        .unwrap()
                        .decrypt_secret(&old_key, new_ciphertext)
                        .is_err()
                );
                assert_eq!(saved["cloud_sync"]["future_policy"]["retain"], true);
                assert_eq!(saved["cloud_sync"]["webdav"]["future_option"], 42);
                if legacy {
                    let stored: CloudSyncSettings = target
                        .read_json_table(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
                        .unwrap()
                        .unwrap();
                    let mut decrypted = stored;
                    super::cloud_sync::decrypt_cloud_sync_settings_secrets(
                        &mut decrypted,
                        &target.credential_crypto().unwrap(),
                        Some(&new_key),
                    )
                    .unwrap();
                    assert_eq!(decrypted, local);
                }
                drop(target);

                let target = ConnectionStore::open(&target_dir).unwrap();
                assert_eq!(
                    LoadBootstrap.execute(&target).unwrap().cloud_sync_settings,
                    local
                );
                target.save_cloud_sync_settings(local.clone()).unwrap();
                target
                    .apply_cloud_sync_snapshot(&target_dir, &snapshot, "shared-password")
                    .unwrap();
                assert_eq!(
                    target
                        .load_decrypted_password_by_id("synced-account")
                        .unwrap()
                        .unwrap()
                        .password
                        .as_deref(),
                    Some("synced-value")
                );
                assert!(target.verify_master_password(target_password).unwrap());
                drop(target);
                let restarted = ConnectionStore::open(&target_dir).unwrap();
                assert_eq!(
                    LoadBootstrap
                        .execute(&restarted)
                        .unwrap()
                        .cloud_sync_settings,
                    local
                );
            }
        }
    }
}

#[test]
fn cloud_pull_rejects_invalid_local_secrets_without_overwriting_database() {
    for legacy in [false, true] {
        let source_dir = unique_temp_dir("reject-rekey-source");
        let target_dir = unique_temp_dir("reject-rekey-target");
        let source = ConnectionStore::open(&source_dir).unwrap();
        source
            .save_master_password(Some("shared-password"))
            .unwrap();
        source
            .get_or_create_master_key_token(&source.credential_crypto().unwrap())
            .unwrap();
        let mut snapshot = source
            .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "2.0.0")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let target = ConnectionStore::open(&target_dir).unwrap();
        target
            .save_master_password(Some("shared-password"))
            .unwrap();
        let mut local = CloudSyncSettings::default();
        local.webdav.password = Some("local-webdav".into());
        target.save_cloud_sync_settings(local).unwrap();
        let mut raw = target.load_settings_value().unwrap();
        raw["cloud_sync"]["webdav"]["password"] = serde_json::json!("corrupted-ciphertext");
        if legacy {
            target
                .save_settings_doc_value(SETTINGS_CLOUD_SYNC, &raw["cloud_sync"])
                .unwrap();
            raw.as_object_mut().unwrap().remove("cloud_sync");
        }
        target.save_settings_value(&raw).unwrap();
        let before = target
            .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "target", "2.0.0")
            .unwrap();
        let legacy_before = target
            .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
            .unwrap();
        assert!(
            target
                .apply_cloud_sync_snapshot(&target_dir, &snapshot, "wrong-password")
                .is_err()
        );
        assert!(
            target
                .apply_cloud_sync_snapshot(&target_dir, &snapshot, "shared-password")
                .is_err()
        );
        let after = target
            .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "target", "2.0.0")
            .unwrap();
        assert_eq!(before.entities, after.entities);
        assert_eq!(
            legacy_before,
            target
                .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
                .unwrap()
        );
    }
}

#[test]
fn cloud_pull_store_worker_flushes_and_reopens_with_local_provider_secrets() {
    use crate::runtime::{FlushBarrier, LoadBootstrap, StoreConfig, StoreRuntime};
    use nyaterm_core::{CloudLocalStore, LocalCloudSyncOptions};

    let source_dir = unique_temp_dir("worker-cloud-source");
    let target_dir = unique_temp_dir("worker-cloud-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    source
        .save_master_password(Some("shared-password"))
        .unwrap();
    source
        .get_or_create_master_key_token(&source.credential_crypto().unwrap())
        .unwrap();
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "2.0.0")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    target
        .save_master_password(Some("shared-password"))
        .unwrap();
    let mut local = CloudSyncSettings::default();
    local.webdav.password = Some("local-webdav".into());
    target.save_cloud_sync_settings(local.clone()).unwrap();
    drop(target);
    let options = LocalCloudSyncOptions {
        device_name: None,
        config_dir: target_dir.to_path_buf(),
        portable_key_path: None,
        remote_dir: source_dir.to_path_buf(),
        remote_root: "nyaterm".into(),
        device_id: "target".into(),
        app_version: "2.0.0".into(),
        master_password: "shared-password".into(),
        enabled: true,
    };
    for _ in 0..2 {
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: target_dir.to_path_buf(),
            portable_key_path: None,
        })
        .unwrap();
        let client = runtime.blocking_client();
        client.apply_sync_snapshot(&options, &snapshot).unwrap();
        assert_eq!(
            client
                .request(0, LoadBootstrap)
                .unwrap()
                .outcome
                .unwrap()
                .cloud_sync_settings,
            local
        );
        runtime.begin_shutdown();
        client
            .request_shutdown(0, FlushBarrier)
            .unwrap()
            .outcome
            .unwrap();
        drop(client);
        drop(runtime);
        // Dropping the runtime closes its channel; the detached worker releases
        // redb asynchronously after receiving that closure.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let reopened = loop {
            match ConnectionStore::open(&target_dir) {
                Ok(store) => break store,
                Err(StorageError::Open {
                    source: redb::DatabaseError::DatabaseAlreadyOpen,
                    ..
                }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("cannot reopen the store after worker shutdown: {error}"),
            }
        };
        assert_eq!(reopened.load_cloud_sync_settings().unwrap(), local);
    }
    let restarted = ConnectionStore::open(&target_dir).unwrap();
    assert_eq!(restarted.load_cloud_sync_settings().unwrap(), local);
}

#[test]
fn cloud_pull_reencrypts_secrets_retained_from_partial_settings() {
    use crate::runtime::{LoadBootstrap, StoreRequest};

    let source_dir = unique_temp_dir("partial-rekey-source");
    let target_dir = unique_temp_dir("partial-rekey-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    source
        .save_master_password(Some("shared-password"))
        .unwrap();
    source
        .get_or_create_master_key_token(&source.credential_crypto().unwrap())
        .unwrap();
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "2.0.0")
        .unwrap();
    let mut settings: serde_json::Value =
        serde_json::from_str(&snapshot.entities["settings"]).unwrap();
    settings.as_object_mut().unwrap().remove("translation");
    assert!(settings.get("ai").is_none());
    snapshot
        .entities
        .insert("settings".into(), serde_json::to_string(&settings).unwrap());
    snapshot.recalculate_hash().unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    target
        .save_master_password(Some("shared-password"))
        .unwrap();
    let mut ai = target.load_ai_settings().unwrap();
    ai.proxy.password = Some("retained-ai-password".into());
    let ai = target.save_ai_settings(ai).unwrap();
    let mut translation = target.load_translation_settings().unwrap();
    translation.deepl_api_key = "retained-deepl-key".into();
    let translation = target.save_translation_settings(translation).unwrap();
    let mut raw = target.load_settings_value().unwrap();
    raw["ai"]["future_option"] = serde_json::json!(42);
    raw["translation"]["future_option"] = serde_json::json!(43);
    target.save_settings_value(&raw).unwrap();
    target
        .apply_cloud_sync_snapshot(&target_dir, &snapshot, "shared-password")
        .unwrap();
    assert_eq!(target.load_ai_settings().unwrap(), ai);
    assert_eq!(target.load_translation_settings().unwrap(), translation);
    let saved = target.load_settings_value().unwrap();
    assert_eq!(saved["ai"]["future_option"], 42);
    assert_eq!(saved["translation"]["future_option"], 43);
    drop(target);
    let restarted = ConnectionStore::open(&target_dir).unwrap();
    let bootstrap = LoadBootstrap.execute(&restarted).unwrap();
    assert_eq!(bootstrap.ai_settings, ai);
    assert_eq!(bootstrap.translation_settings, translation);
}

#[test]
fn cloud_pull_without_source_key_keeps_local_key_for_inline_passwords() {
    let source_dir = unique_temp_dir("keyless-cloud-source");
    let target_dir = unique_temp_dir("keyless-cloud-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "source", "2.0.0")
        .unwrap();
    let mut connection = ssh_connection_for_asset("keyless-inline-password");
    connection.auth = Some(ConnectionAuth {
        password: Some("imported-inline".into()),
        mode: "password".into(),
        ..ConnectionAuth::default()
    });
    let sessions = SessionsConfig {
        connections: vec![connection],
        ..SessionsConfig::default()
    };
    snapshot
        .entities
        .insert("sessions".into(), serde_json::to_string(&sessions).unwrap());
    snapshot.recalculate_hash().unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    let mut local = CloudSyncSettings::default();
    local.webdav.password = Some("local-webdav".into());
    target.save_cloud_sync_settings(local.clone()).unwrap();
    let key = target.load_master_key_token().unwrap();
    target
        .apply_cloud_sync_snapshot(&target_dir, &snapshot, "shared-password")
        .unwrap();
    assert_eq!(target.load_master_key_token().unwrap(), key);
    assert_eq!(target.load_cloud_sync_settings().unwrap(), local);
    assert_eq!(
        target.load_sessions().unwrap().connections[0]
            .auth
            .as_ref()
            .unwrap()
            .password
            .as_deref(),
        Some("imported-inline")
    );
}

#[test]
fn encrypted_backup_import_reencrypts_retained_standalone_cloud_settings() {
    let source_dir = unique_temp_dir("backup-rekey-source");
    let target_dir = unique_temp_dir("backup-rekey-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    source
        .save_master_password(Some("source-password"))
        .unwrap();
    let mut imported = CloudSyncSettings::default();
    imported.webdav.password = Some("imported-webdav".into());
    source.save_cloud_sync_settings(imported.clone()).unwrap();
    let backup_path = source_dir.join("encrypted.nya");
    source
        .export_encrypted_portable_snapshot_from_open_store(
            &backup_path,
            "source",
            "2.0.0",
            "source-password",
        )
        .unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    target
        .save_master_password(Some("target-password"))
        .unwrap();
    let mut local = CloudSyncSettings::default();
    local.webdav.password = Some("local-webdav".into());
    target.save_cloud_sync_settings(local.clone()).unwrap();
    let mut raw = target.load_settings_value().unwrap();
    raw["cloud_sync"]["future_option"] = serde_json::json!(42);
    target
        .save_settings_doc_value(SETTINGS_CLOUD_SYNC, &raw["cloud_sync"])
        .unwrap();
    raw.as_object_mut().unwrap().remove("cloud_sync");
    target.save_settings_value(&raw).unwrap();
    target
        .import_encrypted_portable_snapshot_into_open_store(&backup_path, "source-password")
        .unwrap();
    assert_eq!(target.load_cloud_sync_settings().unwrap(), imported);
    let retained: serde_json::Value = target
        .read_json_table(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
        .unwrap()
        .unwrap();
    assert_eq!(retained["future_option"], 42);
    let mut legacy: CloudSyncSettings = serde_json::from_value(retained).unwrap();
    super::cloud_sync::decrypt_cloud_sync_settings_secrets(
        &mut legacy,
        &target.credential_crypto().unwrap(),
        target.load_master_key_token().unwrap().as_deref(),
    )
    .unwrap();
    assert_eq!(legacy, local);
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
                tags: Vec::new(),
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
        tags: Vec::new(),
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
        tags: Vec::new(),
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
                tags: Vec::new(),
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
        tags: Vec::new(),
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
        sort_order: 0,
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
            sort_order: 0,
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
            sort_order: 0,
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
fn known_hosts_management_lists_deletes_and_clears_structured_entries() {
    let dir = unique_temp_dir("known-hosts-management");
    let store = ConnectionStore::open(&dir).expect("store");
    let key = "AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti";
    store
        .replace_known_hosts_export(&format!(
            "# preserved raw line\n@cert-authority host-b,host-a ssh-ed25519 {key} comment\nhost-c ssh-ed25519 {key}\ninvalid ssh-ed25519 AQID\n",
        ))
        .expect("seed known hosts");

    let entries = store.list_known_hosts().expect("list known hosts");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].host_identifier, "host-b");
    assert_eq!(entries[0].host_patterns, ["host-b", "host-a"]);
    assert_eq!(entries[0].marker.as_deref(), Some("@cert-authority"));
    assert_eq!(
        entries[0].fingerprint,
        "SHA256:UCUiLr7Pjs9wFFJMDByLgc3NrtdU344OgUM45wZPcIQ"
    );

    store
        .delete_known_host(&entries[0].id)
        .expect("delete known host");
    let rendered = store
        .render_known_hosts_export()
        .expect("render after delete");
    assert!(rendered.contains("# preserved raw line"));
    assert!(rendered.contains("invalid ssh-ed25519 AQID"));
    assert!(!rendered.contains("host-b,host-a"));
    assert!(rendered.contains("host-c"));

    assert!(store.delete_known_host("known_hosts/raw/invalid").is_err());
    assert!(store.delete_known_host("rdp_known_hosts/invalid").is_err());
    let hashed = "|1|nNMSH1CuL4w6FneDFn3ONf5paeg=|q8MlMsHsBk6GOpNwYqhnCeXKlRk=";
    store
        .upsert_known_host(&format!("@revoked {hashed} ssh-ed25519 {key}"))
        .expect("hashed revoked host");
    let entries = store.list_known_hosts().expect("list hashed hosts");
    let entry = entries
        .iter()
        .find(|entry| entry.host_identifier == hashed)
        .expect("hashed entry");
    assert_eq!(entry.host_patterns, [hashed]);
    assert_eq!(entry.marker.as_deref(), Some("@revoked"));
    store
        .upsert_rdp_known_host(
            "rdp.example",
            3389,
            "sha256:rdp",
            super::RdpCertificateMetadata::default(),
        )
        .expect("rdp certificate");
    store.clear_known_hosts().expect("clear known hosts");
    assert!(store.list_known_hosts().expect("list cleared").is_empty());
    assert_eq!(
        store.render_known_hosts_export().expect("render cleared"),
        ""
    );
    assert_eq!(
        store
            .check_rdp_known_host("rdp.example", 3389, "sha256:rdp")
            .expect("rdp unchanged"),
        RdpKnownHostCheck::Match
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
    summary.bold_default_foreground = true;
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
    assert!(saved.bold_default_foreground);
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
    assert_eq!(
        raw["appearance"]["bold_default_foreground"],
        serde_json::json!(true)
    );
    summary.bold_default_foreground = false;
    store.save_appearance_settings(&summary).expect("clear");
    let raw = store.load_settings_value().expect("raw cleared");
    assert!(raw["appearance"].get("bold_default_foreground").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn window_material_defaults_ignore_legacy_preferences_and_preserve_unknown_fields() {
    use nyaterm_core::models::window_material::WindowMaterial;

    let dir = unique_temp_dir("window-material-compatibility");
    let store = ConnectionStore::open(&dir).expect("store");
    let legacy = serde_json::json!({"appearance": {
        "window_transparency": "acrylic", "window_transparency_tint": 0.3,
        "window_transparency_blur": true, "background_opacity": 0.45,
        "future_field": {"enabled": true}
    }});
    store.save_settings_value(&legacy).expect("legacy document");
    let mut summary = store.load_app_settings_summary().expect("read legacy");
    assert_eq!(summary.window_material, WindowMaterial::Opaque);
    assert_eq!(summary.window_background_opacity, 20);
    assert_eq!(summary.background_content_opacity, 45);
    summary.window_material = WindowMaterial::Blurred;
    summary.window_background_opacity = 0;
    let saved = store
        .save_appearance_settings(&summary)
        .expect("save material");
    assert_eq!(saved.window_material, WindowMaterial::Blurred);
    assert_eq!(saved.window_background_opacity, 0);
    let raw = store.load_settings_value().expect("raw document");
    for key in [
        "window_transparency",
        "window_transparency_tint",
        "window_transparency_blur",
        "future_field",
        "background_opacity",
    ] {
        assert_eq!(raw["appearance"][key], legacy["appearance"][key]);
    }
    let mut future = raw;
    future["appearance"]["window_material"] = serde_json::json!("future_backdrop");
    future["appearance"]["window_background_opacity"] = serde_json::json!(150);
    store.save_settings_value(&future).expect("future document");
    let summary = store.load_app_settings_summary().expect("future material");
    assert_eq!(
        summary.window_material,
        WindowMaterial::Unknown("future_backdrop".into())
    );
    assert_eq!(summary.window_background_opacity, 100);
    store
        .save_appearance_settings(&summary)
        .expect("unrelated save");
    assert_eq!(
        store.load_settings_value().unwrap()["appearance"]["window_material"],
        "future_backdrop"
    );
}

#[test]
fn native_window_material_is_restored_from_backup_and_preserved_during_sync() {
    use nyaterm_core::models::window_material::WindowMaterial;
    use nyaterm_core::portable_snapshot::PortableSnapshotKind;
    let dir = unique_temp_dir("native-material-snapshots");
    let store = ConnectionStore::open(&dir).unwrap();
    let mut settings = store.load_app_settings_summary().unwrap();
    settings.window_material = WindowMaterial::Blurred;
    settings.window_background_opacity = 35;
    store.save_appearance_settings(&settings).unwrap();
    let mut backup = store
        .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "device", "test")
        .unwrap();
    backup.recalculate_hash().unwrap();
    let mut sync = store
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "device", "test")
        .unwrap();
    sync.recalculate_hash().unwrap();
    let synced: serde_json::Value = serde_json::from_str(&sync.entities["settings"]).unwrap();
    assert!(synced["appearance"].get("window_material").is_none());
    assert!(
        synced["appearance"]
            .get("window_background_opacity")
            .is_none()
    );
    settings.window_material = WindowMaterial::Transparent;
    settings.window_background_opacity = 75;
    store.save_appearance_settings(&settings).unwrap();
    store.apply_raw_portable_snapshot(&sync).unwrap();
    let local = store.load_app_settings_summary().unwrap();
    assert_eq!(local.window_material, WindowMaterial::Transparent);
    assert_eq!(local.window_background_opacity, 75);
    store.apply_raw_portable_snapshot(&backup).unwrap();
    let restored = store.load_app_settings_summary().unwrap();
    assert_eq!(restored.window_material, WindowMaterial::Blurred);
    assert_eq!(restored.window_background_opacity, 35);
}

#[test]
fn window_decorations_default_and_roundtrip_without_discarding_future_data() {
    use nyaterm_core::models::window_decoration::WindowDecoration;

    let dir = unique_temp_dir("window-decorations-compatibility");
    let store = ConnectionStore::open(&dir).unwrap();
    let legacy = serde_json::json!({"appearance": {
        "theme": "github-dark", "future_field": {"enabled": true}
    }});
    store.save_settings_value(&legacy).unwrap();
    let mut settings = store.load_app_settings_summary().unwrap();
    assert_eq!(settings.window_decorations, WindowDecoration::Client);
    for decoration in [
        WindowDecoration::Server,
        WindowDecoration::Client,
        WindowDecoration::Unknown("future_decoration".into()),
    ] {
        settings.window_decorations = decoration.clone();
        let saved = store.save_appearance_settings(&settings).unwrap();
        assert_eq!(saved.window_decorations, decoration);
        let raw = store.load_settings_value().unwrap();
        assert_eq!(raw["appearance"]["window_decorations"], decoration.as_str());
        assert_eq!(
            raw["appearance"]["future_field"],
            legacy["appearance"]["future_field"]
        );
        settings.theme = "github-light".into();
        assert_eq!(
            store
                .save_appearance_settings(&settings)
                .unwrap()
                .window_decorations,
            decoration
        );
    }
}

#[test]
fn window_decorations_restore_from_backups_and_remain_local_during_sync() {
    use nyaterm_core::models::window_decoration::WindowDecoration;
    use nyaterm_core::portable_snapshot::PortableSnapshotKind;

    let dir = unique_temp_dir("window-decorations-snapshots");
    let store = ConnectionStore::open(&dir).unwrap();
    let mut settings = store.load_app_settings_summary().unwrap();
    settings.window_decorations = WindowDecoration::Server;
    store.save_appearance_settings(&settings).unwrap();
    let mut backup = store
        .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "device", "test")
        .unwrap();
    backup.recalculate_hash().unwrap();
    let mut sync = store
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "device", "test")
        .unwrap();
    sync.recalculate_hash().unwrap();
    let mut synced: serde_json::Value = serde_json::from_str(&sync.entities["settings"]).unwrap();
    assert!(synced["appearance"].get("window_decorations").is_none());
    // Even an incoming document carrying another device's value cannot replace it.
    synced["appearance"]["window_decorations"] = serde_json::json!("server");
    sync.entities.insert("settings".into(), synced.to_string());
    sync.recalculate_hash().unwrap();
    settings.window_decorations = WindowDecoration::Client;
    store.save_appearance_settings(&settings).unwrap();
    store.apply_raw_portable_snapshot(&sync).unwrap();
    assert_eq!(
        store
            .load_app_settings_summary()
            .unwrap()
            .window_decorations,
        WindowDecoration::Client
    );
    store.apply_raw_portable_snapshot(&backup).unwrap();
    assert_eq!(
        store
            .load_app_settings_summary()
            .unwrap()
            .window_decorations,
        WindowDecoration::Server
    );
}

#[test]
fn malformed_native_window_settings_are_rejected_without_overwriting_user_data() {
    let dir = unique_temp_dir("native-material-malformed");
    let store = ConnectionStore::open(&dir).unwrap();
    let settings = store.load_app_settings_summary().unwrap();
    for appearance in [
        serde_json::json!({"window_decorations": {"future": "format"}}),
        serde_json::json!({"window_decorations": null}),
        serde_json::json!({"window_decorations": true}),
        serde_json::json!({"window_material": {"future": "format"}}),
        serde_json::json!({"window_background_opacity": "invalid"}),
        serde_json::json!({"window_background_opacity": 0.2}),
    ] {
        let raw = serde_json::json!({"appearance": appearance});
        store.save_settings_value(&raw).unwrap();
        assert!(store.load_app_settings_summary().is_err());
        assert!(store.save_appearance_settings(&settings).is_err());
        assert_eq!(store.load_settings_value().unwrap(), raw);
    }
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
    summary.ui_serial_send_clear_after_send = true;

    let saved = store.save_ui_layout_settings(&summary).expect("save");
    assert_eq!(saved.ui_quick_cmd_height, 312);
    assert!(!saved.ui_quick_cmd_visible);
    assert_eq!(saved.ui_serial_send_height, 284);
    assert!(saved.ui_serial_send_visible);
    assert!(saved.ui_serial_send_clear_after_send);
    let raw = store.load_settings_value().expect("raw");
    assert_eq!(raw["ui"]["quick_cmd_height"], serde_json::json!(312));
    assert_eq!(raw["ui"]["show_quick_cmd_bar"], serde_json::json!(false));
    assert_eq!(raw["ui"]["serial_send_height"], serde_json::json!(284));
    assert_eq!(raw["ui"]["show_serial_send_panel"], serde_json::json!(true));
    assert_eq!(
        raw["ui"]["serial_send_clear_after_send"],
        serde_json::json!(true)
    );

    summary.ui_quick_cmd_height = 0;
    summary.ui_serial_send_height = 999;
    let clamped = store
        .save_ui_layout_settings(&summary)
        .expect("save clamped");
    assert_eq!(clamped.ui_quick_cmd_height, 36);
    assert_eq!(clamped.ui_serial_send_height, 520);
    summary.ui_serial_send_height = 60;
    assert_eq!(
        store
            .save_ui_layout_settings(&summary)
            .unwrap()
            .ui_serial_send_height,
        120
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn command_send_preferences_load_legacy_defaults_and_preserve_unknown_settings() {
    let dir = unique_temp_dir("settings-command-send-compatibility");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_value(&serde_json::json!({
            "ui": {"serial_send_height": 60, "future_option": {"enabled": true}}
        }))
        .unwrap();
    let mut summary = store.load_app_settings_summary().unwrap();
    assert_eq!(summary.ui_serial_send_height, 120);
    assert!(!summary.ui_serial_send_clear_after_send);
    store
        .save_settings_value(&serde_json::json!({
            "ui": {"serial_send_clear_after_send": true, "future_option": {"enabled": true}}
        }))
        .unwrap();
    summary = store.load_app_settings_summary().unwrap();
    assert!(summary.ui_serial_send_clear_after_send);
    summary.ui_serial_send_clear_after_send = false;
    store.save_ui_layout_settings(&summary).unwrap();
    let raw = store.load_settings_value().unwrap();
    assert_eq!(
        raw["ui"]["future_option"],
        serde_json::json!({"enabled": true})
    );
    assert!(
        !store
            .load_app_settings_summary()
            .unwrap()
            .ui_serial_send_clear_after_send
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn mac_matcher_loads_legacy_defaults_and_round_trips_without_losing_unknown_options() {
    let dir = unique_temp_dir("mac-matcher-settings");
    let store = ConnectionStore::open(&dir).unwrap();
    for terminal in [
        serde_json::json!({}),
        serde_json::json!({"action_links_matchers": {"ipv4": false, "archive": false, "host_port": false}}),
    ] {
        store
            .save_settings_value(&serde_json::json!({"terminal": terminal}))
            .unwrap();
        let summary = store.load_app_settings_summary().unwrap();
        assert!(summary.terminal_action_links_matchers.mac);
        assert!(!summary.terminal_action_links_enabled);
    }
    store.save_settings_value(&serde_json::json!({"terminal": {
        "future_terminal_option": "keep",
        "action_links_enabled": true,
        "action_links_matchers": {"ipv4": false, "mac": false, "future_matcher": {"enabled": true}}
    }})).unwrap();
    let mut summary = store.load_app_settings_summary().unwrap();
    assert!(!summary.terminal_action_links_matchers.mac);
    assert!(!summary.terminal_action_links_matchers.ipv4);
    for mac in [true, false] {
        summary.terminal_action_links_matchers.mac = mac;
        let loaded = store.save_terminal_settings(&summary).unwrap();
        assert_eq!(loaded.terminal_action_links_matchers.mac, mac);
        assert!(loaded.terminal_action_links_enabled);
        assert!(!loaded.terminal_action_links_matchers.ipv4);
        let raw = store.load_settings_value().unwrap();
        assert_eq!(raw["terminal"]["action_links_matchers"]["mac"], mac);
        assert_eq!(
            raw["terminal"]["action_links_matchers"]["future_matcher"],
            serde_json::json!({"enabled": true})
        );
        assert_eq!(raw["terminal"]["future_terminal_option"], "keep");
    }
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
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
    assert_eq!(
        summary.ui_activity_bar_hidden_items,
        ["gpuMonitor", "ascendNpuMonitor"]
    );
    assert_eq!(summary.ui_panel_open_mode, "docked");
    assert!(!summary.ui_panel_multi_open);
    assert_eq!(
        summary.ui_activity_bar_left_bottom,
        ["syncBackupHistory", "plugins", "settings"]
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn plugin_layout_roundtrips_without_losing_legacy_or_unknown_settings() {
    let dir = unique_temp_dir("settings-plugin-layout-compatibility");
    let store = ConnectionStore::open(&dir).expect("store");
    let legacy = serde_json::json!({
        "ui": {
            "activity_bar_layout": {
                "left_bottom": ["futureEntry", "syncBackupHistory", "settings"],
                "right_top": ["savedConnections"],
                "hidden_items": ["futureEntry"],
                "futureLayoutOption": { "enabled": true }
            },
            "futureUiOption": "preserve"
        },
        "futureRootOption": [1, 2]
    });
    store
        .save_settings_value(&legacy)
        .expect("save legacy document");
    let mut summary = store
        .load_app_settings_summary()
        .expect("load legacy document");
    assert_eq!(
        summary.ui_activity_bar_left_bottom,
        ["futureEntry", "syncBackupHistory", "settings"]
    );
    assert_eq!(store.load_settings_value().expect("raw"), legacy);

    // The desktop can add and later move the entry using the existing layout contract.
    summary.ui_activity_bar_right_top.push("plugins".into());
    summary.ui_activity_bar_hidden_items.push("plugins".into());
    let saved = store
        .save_ui_layout_settings(&summary)
        .expect("save plugin placement");
    assert_eq!(
        saved.ui_activity_bar_left_bottom,
        summary.ui_activity_bar_left_bottom
    );
    assert_eq!(
        saved.ui_activity_bar_right_top,
        ["savedConnections", "plugins"]
    );
    assert_eq!(
        saved.ui_activity_bar_hidden_items,
        ["futureEntry", "gpuMonitor", "ascendNpuMonitor", "plugins"]
    );
    let raw = store.load_settings_value().expect("raw");
    assert_eq!(
        raw["ui"]["activity_bar_layout"]["futureLayoutOption"],
        legacy["ui"]["activity_bar_layout"]["futureLayoutOption"]
    );
    assert_eq!(raw["ui"]["futureUiOption"], legacy["ui"]["futureUiOption"]);
    assert_eq!(raw["futureRootOption"], legacy["futureRootOption"]);
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
fn retired_ime_setting_survives_loading_and_saving_interaction_settings() {
    for legacy_enabled in [false, true] {
        let dir = unique_temp_dir("retired-ime-setting");
        let store = ConnectionStore::open(&dir).unwrap();
        store
            .save_settings_value(&serde_json::json!({
                "interaction": {
                    "mac_ime_compatibility": legacy_enabled,
                    "future_setting": {"keep": true}
                }
            }))
            .unwrap();

        let mut settings = store.load_app_settings_summary().unwrap();
        assert_eq!(settings.interaction_mac_ime_compatibility, legacy_enabled);
        settings.interaction_alt_as_meta = true;
        let saved = store.save_interaction_settings(&settings).unwrap();
        assert_eq!(saved.interaction_mac_ime_compatibility, legacy_enabled);
        assert!(saved.interaction_alt_as_meta);
        let stored = store.load_settings_value().unwrap();
        assert_eq!(
            stored["interaction"]["mac_ime_compatibility"],
            legacy_enabled
        );
        assert_eq!(
            stored["interaction"]["future_setting"],
            serde_json::json!({"keep": true})
        );
    }
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
            "file_explorer_view_mode": "tree",
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
    assert_eq!(
        summary.ui_file_manager,
        nyaterm_core::FileManagerUiSettings::default()
    );
    assert_eq!(summary.ui_remote_stats_interval, 9);
    // Documents written before the logged-in users reading keep it hidden.
    assert!(!summary.ui_remote_stats_show_users);
    assert_eq!(summary.ui_gpu_monitor_interval, 7);
    assert_eq!(summary.ui_ascend_npu_monitor_interval, 8);
    assert_eq!(summary.ui_process_manager_interval, 11);
    assert_eq!(summary.ui_docker_manager_interval, 13);
    assert_eq!(summary.ui_quick_cmd_view_mode, "compact");
    assert_eq!(summary.ui_quick_cmd_sort_mode, "useCount");
    assert_eq!(summary.ui_quick_cmd_selected_category, "all");
    assert_eq!(
        summary.ui_file_explorer_view_mode,
        nyaterm_core::TransferBrowserViewMode::Tree
    );
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
    // The stored fixture predates the field, so it must default on.
    assert!(summary.interaction_credential_autofill_enabled);
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
    favorite_update.ui_file_explorer_view_mode = nyaterm_core::TransferBrowserViewMode::List;
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
        updated.ui_file_explorer_view_mode,
        nyaterm_core::TransferBrowserViewMode::List
    );
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
        json_path(&stored, &["ui", "file_explorer_view_mode"]).and_then(|value| value.as_str()),
        Some("list")
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

    let mut file_manager_update = summary.clone();
    file_manager_update.ui_file_manager.open = true;
    file_manager_update.ui_file_manager.split_percent = 40;
    file_manager_update.ui_file_manager.left.path = "/tmp/left".to_string();
    file_manager_update.ui_file_manager.right = nyaterm_core::FileManagerPaneRestore {
        source: nyaterm_core::FileManagerSourceRestore::Connection {
            id: "conn-1".to_string(),
        },
        path: "/var".to_string(),
        raw_path_token: None,
    };
    let updated = store
        .save_file_explorer_favorite_dirs(&file_manager_update)
        .expect("save file manager");
    assert!(updated.ui_file_manager.open);
    assert_eq!(updated.ui_file_manager.split_percent, 40);
    assert_eq!(updated.ui_file_manager.left.path, "/tmp/left");
    assert_eq!(
        updated.ui_file_manager.right,
        nyaterm_core::FileManagerPaneRestore {
            source: nyaterm_core::FileManagerSourceRestore::Connection {
                id: "conn-1".to_string(),
            },
            path: "/var".to_string(),
            raw_path_token: None,
        }
    );
    let stored = store.load_settings_value().expect("stored file manager");
    assert_eq!(
        json_path(&stored, &["ui", "file_manager", "open"]).and_then(|value| value.as_bool()),
        Some(true)
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
    terminal_update.ui_remote_stats_interval = 4;
    terminal_update.ui_remote_stats_show_users = true;
    terminal_update.ui_gpu_monitor_interval = 10;
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
    assert_eq!(saved_terminal.ui_remote_stats_interval, 4);
    assert!(saved_terminal.ui_remote_stats_show_users);
    assert_eq!(saved_terminal.ui_gpu_monitor_interval, 10);
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
        json_path(&stored, &["ui", "remote_stats_show_users"]).and_then(|value| value.as_bool()),
        Some(true)
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
    quick_command_update.ui_quick_cmd_selected_category = "category-1".to_string();
    let saved_quick_command_ui = store
        .save_quick_command_ui_settings(&quick_command_update)
        .expect("save quick command ui settings");
    assert_eq!(saved_quick_command_ui.ui_quick_cmd_view_mode, "list");
    assert_eq!(saved_quick_command_ui.ui_quick_cmd_sort_mode, "name");
    assert_eq!(
        saved_quick_command_ui.ui_quick_cmd_selected_category,
        "category-1"
    );
    assert_eq!(
        store
            .load_app_settings_summary()
            .expect("reload quick command settings")
            .ui_quick_cmd_selected_category,
        "category-1"
    );
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
    assert_eq!(
        json_path(&stored, &["ui", "quick_cmd_selected_category"]).and_then(|value| value.as_str()),
        Some("category-1")
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
    interaction_update.interaction_terminal_right_click_action = TerminalRightClickAction::Menu;
    interaction_update.interaction_command_suggestion_min_chars = 4;
    interaction_update.interaction_command_suggestion_max_chars = 120;
    interaction_update.interaction_duplicate_session_command_delay_ms = 2_500;
    interaction_update.interaction_allow_osc52_clipboard_write = false;
    interaction_update.interaction_terminal_zoom_enabled = true;
    interaction_update.interaction_credential_autofill_enabled = false;
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
    assert!(!saved_interaction.interaction_credential_autofill_enabled);
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
    recording_update.recording_include_input = true;
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
    assert!(saved_recording.recording_include_input);
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
    lock_update.enable_startup_lock = false;
    lock_update.enable_idle_lock = false;
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
        ssh_runtime_mode: None,
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
            ssh_runtime_mode: None,
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
fn open_tabs_preserve_legacy_and_tauri_ssh_modes_in_the_existing_document() {
    let dir = unique_temp_dir("open-tabs-ssh-modes");
    let store = ConnectionStore::open(&dir).unwrap();
    let tabs: Vec<nyaterm_core::models::RestorableOpenTab> = serde_json::from_value(serde_json::json!([
        {"title":"legacy", "session_type":"SSH", "connection_id":"saved"},
        {"title":"Tauri split", "session_type":"SSH", "ssh_runtime_mode":"standard", "root":{
            "kind":"split", "id":"split", "direction":"vertical", "ratio":0.5,
            "first":{"kind":"leaf", "id":"a", "session_type":"SSH", "connection_id":"a", "ssh_runtime_mode":"sftp"},
            "second":{"kind":"leaf", "id":"b", "session_type":"SSH", "connection_id":"b", "ssh_runtime_mode":"future-mode"}
        }}
    ])).unwrap();
    store.save_open_tabs(&tabs).unwrap();
    let restored = store.load_open_tabs().unwrap();
    assert_eq!(restored, tabs);
    assert!(restored[0].ssh_runtime_modes_supported());
    assert!(!restored[1].ssh_runtime_modes_supported());
    assert_eq!(
        restored[1].expanded_sessions()[1]
            .ssh_runtime_mode
            .as_deref(),
        Some("future-mode")
    );
    drop(store);
    let reopened = ConnectionStore::open(&dir).unwrap();
    assert_eq!(reopened.load_open_tabs().unwrap(), tabs);
    drop(reopened);
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
fn legacy_workspace_fields_migrate_and_new_manifest_dual_writes_recent_workspace() {
    let dir = unique_temp_dir("workspace-manifest-migration");
    let store = ConnectionStore::open(&dir).expect("store");
    let legacy_tab =
        nyaterm_core::RestorableOpenTab::with_leaf_root("legacy", "Local", None, None, None);
    store
        .save_open_tabs(std::slice::from_ref(&legacy_tab))
        .unwrap();

    let migrated = store.load_workspace_restore_manifest().unwrap();
    assert_eq!(migrated.workspaces.len(), 1);
    assert_eq!(
        migrated.workspaces[0].id,
        nyaterm_core::WorkspaceId::legacy()
    );
    assert_eq!(migrated.workspaces[0].sessions.open_tabs, vec![legacy_tab]);

    let first_id = migrated.workspaces[0].id;
    let second_id = nyaterm_core::WorkspaceId::new();
    let mut second = nyaterm_core::WorkspaceRestoreState::empty(second_id);
    let recent_tab = nyaterm_core::RestorableOpenTab::with_leaf_root(
        "recent",
        "SSH",
        Some("connection-2".to_string()),
        None,
        None,
    );
    second.sessions.open_tabs.push(recent_tab.clone());
    second.ui.left_panel_width = 333;
    second.ui.right_panel_width = 444;
    second.ui.transfer_panel_height = 275;
    second.ui.bottom_panel_height = 225;
    second.ui.serial_send_panel_height = 195;
    second.ui.bottom_panel_mode = "command_send".to_string();
    second.ui.panel_multi_open = true;
    second.ui.left_open_panels = vec!["fileExplorer".to_string(), "notes".to_string()];
    second
        .ui
        .panel_stack_sizes
        .insert("left:fileExplorer".to_string(), 600);
    let manifest = nyaterm_core::WorkspaceRestoreManifest {
        version: nyaterm_core::WORKSPACE_RESTORE_MANIFEST_VERSION,
        workspaces: vec![migrated.workspaces[0].clone(), second],
        most_recent_workspace_id: Some(second_id),
        extra: Default::default(),
    };
    store.save_workspace_restore_manifest(&manifest).unwrap();

    assert_eq!(store.load_open_tabs().unwrap(), vec![recent_tab]);
    let loaded = store.load_workspace_restore_manifest().unwrap();
    assert_eq!(loaded, manifest);
    assert_eq!(loaded.workspaces[0].id, first_id);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn device_window_manifest_roundtrips_and_projects_recent_window_to_legacy_key() {
    let dir = unique_temp_dir("device-window-manifest");
    let store = ConnectionStore::open(&dir).expect("store");
    let first_id = nyaterm_core::WorkspaceId::new();
    let second_id = nyaterm_core::WorkspaceId::new();
    let first = MainWindowState::new(
        None,
        MainWindowBounds {
            x: 0,
            y: 0,
            width: 900,
            height: 700,
        },
        false,
    );
    let second = MainWindowState::new(
        None,
        MainWindowBounds {
            x: 120,
            y: 80,
            width: 1280,
            height: 800,
        },
        true,
    );
    let manifest = nyaterm_core::DeviceWindowManifest {
        version: nyaterm_core::DEVICE_WINDOW_MANIFEST_VERSION,
        windows: vec![
            nyaterm_core::DeviceWindowState {
                workspace_id: first_id,
                window: first,
                extra: Default::default(),
            },
            nyaterm_core::DeviceWindowState {
                workspace_id: second_id,
                window: second.clone(),
                extra: Default::default(),
            },
        ],
        window_order: vec![first_id, second_id],
        most_recent_workspace_id: Some(second_id),
        extra: Default::default(),
    };
    store.save_device_window_manifest(&manifest).unwrap();

    assert_eq!(
        store.load_device_window_manifest(first_id).unwrap(),
        manifest
    );
    assert_eq!(store.load_main_window_state().unwrap(), Some(second));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn restore_manifests_rollback_every_document_on_failure() {
    use nyaterm_core::{
        DeviceWindowManifest, DeviceWindowState, WorkspaceId, WorkspaceRestoreManifest,
        WorkspaceRestoreState,
    };

    let dir = unique_temp_dir("restore-atomic-rollback");
    let store = ConnectionStore::open(&dir).unwrap();
    let first_id = WorkspaceId::new();
    let second_id = WorkspaceId::new();
    let first_window = MainWindowState::new(
        None,
        MainWindowBounds {
            x: 10,
            y: 20,
            width: 900,
            height: 700,
        },
        false,
    );
    let second_window = MainWindowState::new(
        None,
        MainWindowBounds {
            x: 40,
            y: 50,
            width: 1100,
            height: 800,
        },
        true,
    );
    let old_workspace = WorkspaceRestoreManifest::single(WorkspaceRestoreState::empty(first_id));
    let old_device = DeviceWindowManifest {
        version: nyaterm_core::DEVICE_WINDOW_MANIFEST_VERSION,
        windows: vec![DeviceWindowState {
            workspace_id: first_id,
            window: first_window.clone(),
            extra: Default::default(),
        }],
        window_order: vec![first_id],
        most_recent_workspace_id: Some(first_id),
        extra: Default::default(),
    };
    store
        .save_restore_manifests_atomically(&old_workspace, &old_device)
        .unwrap();
    let old_settings = store.load_settings_value().unwrap();
    let mut next_workspace = old_workspace.clone();
    next_workspace
        .workspaces
        .push(WorkspaceRestoreState::empty(second_id));
    next_workspace.most_recent_workspace_id = Some(second_id);
    let mut next_device = old_device.clone();
    next_device.windows.push(DeviceWindowState {
        workspace_id: second_id,
        window: second_window.clone(),
        extra: Default::default(),
    });
    next_device.window_order.push(second_id);
    next_device.most_recent_workspace_id = Some(second_id);

    for document in 1..=3 {
        assert!(
            store
                .save_restore_manifests_failing_after(&next_workspace, &next_device, document)
                .is_err()
        );
        assert_eq!(store.load_settings_value().unwrap(), old_settings);
        assert_eq!(
            store.load_workspace_restore_manifest().unwrap(),
            old_workspace
        );
        assert_eq!(
            store.load_device_window_manifest(first_id).unwrap(),
            old_device
        );
        assert_eq!(
            store.load_main_window_state().unwrap(),
            Some(first_window.clone())
        );
    }
    store
        .save_restore_manifests_atomically(&next_workspace, &next_device)
        .unwrap();
    assert_eq!(
        store.load_workspace_restore_manifest().unwrap(),
        next_workspace
    );
    assert_eq!(
        store.load_device_window_manifest(first_id).unwrap(),
        next_device
    );
    assert_eq!(store.load_main_window_state().unwrap(), Some(second_window));
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
fn fresh_portable_store_reopens_master_password_and_credential() {
    let dir = unique_temp_dir("fresh-portable-store");
    let key_path = dir.join("portable.key");
    let store = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()))
        .expect("open fresh portable store");

    let key_material = std::fs::read_to_string(&key_path).expect("read portable key");
    assert!(!key_material.trim().is_empty());
    assert_eq!(
        B64.decode(key_material.trim()).expect("base64 key").len(),
        32
    );
    store
        .save_master_password(Some("portable-master-password"))
        .expect("save master password");
    assert!(
        store
            .verify_master_password("portable-master-password")
            .unwrap()
    );
    store
        .save_credential(SavedCredential {
            id: "portable-credential".to_string(),
            sort_order: 0,
            name: "Portable login".to_string(),
            username: "portable-user".to_string(),
            password: Some("portable-secret".into()),
            username_prompt_regex: None,
            password_prompt_regex: None,
            enabled: true,
            has_password: false,
        })
        .expect("save portable credential");
    drop(store);

    let reopened = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path))
        .expect("reopen portable store");
    assert!(
        reopened
            .verify_master_password("portable-master-password")
            .unwrap()
    );
    let credential = reopened
        .load_decrypted_credential_by_id("portable-credential")
        .expect("decrypt reopened credential")
        .expect("portable credential");
    assert_eq!(credential.username, "portable-user");
    assert_eq!(credential.password.as_deref(), Some("portable-secret"));

    drop(reopened);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn existing_portable_key_is_preserved_byte_for_byte() {
    let dir = unique_temp_dir("existing-portable-key");
    std::fs::create_dir_all(&dir).expect("create portable dir");
    let key_path = dir.join("portable.key");
    let existing = b"existing-portable-key-material\r\n";
    std::fs::write(&key_path, existing).expect("seed portable key");

    let store = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()))
        .expect("open portable store");
    store
        .save_master_password(Some("existing-key-password"))
        .expect("save with existing key");
    drop(store);

    let store = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()))
        .expect("reopen with existing key");
    assert!(
        store
            .verify_master_password("existing-key-password")
            .unwrap()
    );
    super::create_portable_key(&key_path).expect("concurrent creator preserves existing key");
    assert_eq!(std::fs::read(&key_path).expect("read key"), existing);

    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn separate_fresh_portable_stores_get_distinct_keys() {
    let first_dir = unique_temp_dir("first-portable-copy");
    let second_dir = unique_temp_dir("second-portable-copy");
    let first_key = first_dir.join("portable.key");
    let second_key = second_dir.join("portable.key");
    let first = ConnectionStore::open_with_portable_key_path(&first_dir, Some(first_key.clone()))
        .expect("first portable store");
    let second =
        ConnectionStore::open_with_portable_key_path(&second_dir, Some(second_key.clone()))
            .expect("second portable store");

    assert_ne!(
        std::fs::read(first_key).unwrap(),
        std::fs::read(second_key).unwrap()
    );

    drop(first);
    drop(second);
    std::fs::remove_dir_all(first_dir).ok();
    std::fs::remove_dir_all(second_dir).ok();
}

#[test]
fn installed_store_does_not_create_portable_key() {
    let dir = unique_temp_dir("installed-store-no-portable-key");
    let key_path = dir.join("portable.key");
    let store = ConnectionStore::open(&dir).expect("open installed store");
    assert!(!key_path.exists());

    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn empty_existing_portable_key_is_rejected() {
    let dir = unique_temp_dir("empty-portable-key");
    std::fs::create_dir_all(&dir).expect("create portable dir");
    let key_path = dir.join("portable.key");
    std::fs::write(&key_path, b" \r\n\t").expect("seed empty portable key");

    let result = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()));
    assert!(matches!(
        result,
        Err(StorageError::Crypto(
            nyaterm_core::CredentialCryptoError::EmptyPortableKey(path)
        )) if path == key_path
    ));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn invalid_utf8_portable_key_is_not_replaced() {
    let dir = unique_temp_dir("invalid-portable-key");
    std::fs::create_dir_all(&dir).expect("create portable dir");
    let key_path = dir.join("portable.key");
    let invalid = [0xff, 0xfe, 0xfd];
    std::fs::write(&key_path, invalid).expect("seed invalid portable key");

    let result = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()));
    assert!(matches!(
        result,
        Err(StorageError::Crypto(
            nyaterm_core::CredentialCryptoError::ReadPortableKey { path, .. }
        )) if path == key_path
    ));
    assert_eq!(std::fs::read(&key_path).unwrap(), invalid);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn missing_portable_key_with_master_key_is_not_regenerated() {
    let dir = unique_temp_dir("missing-portable-master-key");
    let key_path = dir.join("portable.key");
    let store = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()))
        .expect("open portable store");
    store
        .save_credential(SavedCredential {
            id: "portable-credential".to_string(),
            sort_order: 0,
            name: "Portable login".to_string(),
            username: "portable-user".to_string(),
            password: Some("portable-secret".into()),
            username_prompt_regex: None,
            password_prompt_regex: None,
            enabled: true,
            has_password: false,
        })
        .expect("save credential");
    drop(store);
    std::fs::remove_file(&key_path).expect("remove portable key");

    let result = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()));
    assert!(matches!(
        result,
        Err(StorageError::MissingPortableKey { path }) if path == key_path
    ));
    assert!(!key_path.exists());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn missing_portable_key_with_master_password_is_not_regenerated() {
    let dir = unique_temp_dir("missing-portable-master-password");
    let key_path = dir.join("portable.key");
    let store = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()))
        .expect("open portable store");
    store
        .save_master_password(Some("portable-master-password"))
        .expect("save master password");
    drop(store);
    std::fs::remove_file(&key_path).expect("remove portable key");

    let result = ConnectionStore::open_with_portable_key_path(&dir, Some(key_path.clone()));
    assert!(matches!(
        result,
        Err(StorageError::MissingPortableKey { path }) if path == key_path
    ));
    assert!(!key_path.exists());
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
        ..CloudSyncState::default()
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
        ..CloudSyncState::default()
    };
    let legacy_content = serde_json::json!({
        "device_id": "legacy-device",
        "last_synced_payload_hash": "legacy-hash",
        "last_applied_remote_revision": "legacy-rev",
        "last_checked_at_ms": 30,
        "last_synced_at_ms": 40
    })
    .to_string();
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
fn cloud_sync_settings_read_tauri_embedded_config_before_stale_legacy_doc() {
    let dir = unique_temp_dir("cloud-sync-embedded");
    let store = ConnectionStore::open(&dir).expect("store");
    let crypto = store.credential_crypto().expect("crypto");
    let master_key = store
        .get_or_create_master_key_token(&crypto)
        .expect("master key");
    let password = crypto
        .encrypt_secret(&master_key, "synthetic-webdav-secret")
        .expect("encrypt");
    store
        .save_settings_doc_value(
            SETTINGS_CLOUD_SYNC,
            &serde_json::json!({
                "enabled": false
            }),
        )
        .expect("seed stale legacy settings");
    store
        .save_settings_value(&serde_json::json!({
            "cloud_sync": {
                "enabled": true,
                "provider": "webdav",
                "device_name": "tauri-device",
                "remote_root": "shared-root",
                "auto_pull_remote_changes": false,
                "webdav": { "endpoint": "https://dav.example.invalid/dav", "password": password }
            }
        }))
        .expect("seed Tauri settings");

    let loaded = store
        .load_cloud_sync_settings()
        .expect("load embedded settings");
    assert!(loaded.enabled);
    assert_eq!(loaded.device_name, "tauri-device");
    assert_eq!(loaded.remote_root, "shared-root");
    assert!(!loaded.auto_pull_remote_changes);
    assert_eq!(loaded.webdav.endpoint, "https://dav.example.invalid/dav");
    assert_eq!(
        loaded.webdav.password.as_deref(),
        Some("synthetic-webdav-secret")
    );
    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_settings_migrate_legacy_doc_on_save_without_losing_unknown_fields() {
    let dir = unique_temp_dir("cloud-sync-legacy-migration");
    let store = ConnectionStore::open(&dir).expect("store");
    let crypto = store.credential_crypto().expect("crypto");
    let master_key = store
        .get_or_create_master_key_token(&crypto)
        .expect("master key");
    let password = crypto
        .encrypt_secret(&master_key, "synthetic-legacy-secret")
        .expect("encrypt");
    let legacy = serde_json::json!({
        "enabled": true,
        "future_sync_policy": { "mode": "keep" },
        "webdav": { "password": password, "future_provider_option": true }
    });
    store
        .save_settings_doc_value(SETTINGS_CLOUD_SYNC, &legacy)
        .expect("seed legacy settings");
    store
        .save_settings_value(&serde_json::json!({
            "general": { "future_general_option": true }
        }))
        .expect("seed unrelated settings");

    let mut loaded = store
        .load_cloud_sync_settings()
        .expect("load legacy settings");
    assert!(loaded.enabled);
    assert_eq!(
        loaded.webdav.password.as_deref(),
        Some("synthetic-legacy-secret")
    );
    loaded.webdav.password = Some(nyaterm_core::MASKED_SECRET_VALUE.into());
    loaded.device_name = "gpui-device".to_string();
    let saved = store
        .save_cloud_sync_settings(loaded)
        .expect("migrate on save");
    let value = store.load_settings_value().expect("settings");
    assert_eq!(value["cloud_sync"]["device_name"], "gpui-device");
    assert_eq!(value["cloud_sync"]["future_sync_policy"]["mode"], "keep");
    assert_eq!(
        value["cloud_sync"]["webdav"]["future_provider_option"],
        true
    );
    assert_eq!(value["general"]["future_general_option"], true);
    assert_ne!(
        value["cloud_sync"]["webdav"]["password"].as_str(),
        Some("synthetic-legacy-secret")
    );
    assert_eq!(
        store
            .load_cloud_sync_settings()
            .expect("reload migrated settings"),
        saved
    );
    assert_eq!(
        store
            .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
            .expect("legacy doc"),
        Some(legacy)
    );
    drop(store);
    let reopened = ConnectionStore::open(&dir).expect("reopen store");
    assert_eq!(
        reopened
            .load_cloud_sync_settings()
            .expect("reopen migrated settings"),
        saved
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_settings_embedded_disable_wins_over_enabled_legacy_doc() {
    let dir = unique_temp_dir("cloud-sync-disabled-embedded");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_doc_value(SETTINGS_CLOUD_SYNC, &serde_json::json!({ "enabled": true }))
        .expect("seed legacy settings");
    store
        .save_settings_value(&serde_json::json!({ "cloud_sync": { "enabled": false } }))
        .expect("seed embedded settings");
    assert!(
        !store
            .load_cloud_sync_settings()
            .expect("load embedded disable")
            .enabled
    );
    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_settings_invalid_embedded_data_does_not_fall_back_or_overwrite() {
    let dir = unique_temp_dir("cloud-sync-corrupt-embedded");
    let store = ConnectionStore::open(&dir).expect("store");
    store
        .save_settings_doc_value(SETTINGS_CLOUD_SYNC, &serde_json::json!({ "enabled": true }))
        .expect("seed legacy settings");
    for cloud_sync in [
        serde_json::json!({ "enabled": "invalid" }),
        serde_json::json!({ "enabled": true, "webdav": { "password": "corrupt-token" } }),
        serde_json::Value::Null,
    ] {
        let value = serde_json::json!({ "cloud_sync": cloud_sync });
        store
            .save_settings_value(&value)
            .expect("seed invalid embedded settings");
        assert!(store.load_cloud_sync_settings().is_err());
        assert!(
            store
                .save_cloud_sync_settings(CloudSyncSettings::default())
                .is_err()
        );
        assert_eq!(
            store.load_settings_value().expect("unchanged settings"),
            value
        );
    }
    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_settings_encrypt_and_merge_masked_provider_secrets() {
    let dir = unique_temp_dir("cloud-sync-settings");
    let store = ConnectionStore::open(&dir).expect("store");
    store.save_settings_value(&serde_json::json!({
        "cloud_sync": { "future_sync_option": true, "webdav": { "future_provider_option": 42 } },
        "general": { "future_general_option": "keep" }
    })).expect("seed unknown embedded fields");
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

    let raw_settings = store.load_settings_value().expect("read raw settings");
    let raw: CloudSyncSettings =
        serde_json::from_value(raw_settings["cloud_sync"].clone()).expect("raw cloud settings");
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
    let raw_settings = store.load_settings_value().expect("read saved settings");
    assert_eq!(raw_settings["cloud_sync"]["future_sync_option"], true);
    assert_eq!(
        raw_settings["cloud_sync"]["webdav"]["future_provider_option"],
        42
    );
    assert_eq!(raw_settings["general"]["future_general_option"], "keep");

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ai_headers_without_api_key_encrypt_roundtrip_and_reject_invalid_updates() {
    let dir = unique_temp_dir("ai-header-only");
    let store = ConnectionStore::open(&dir).unwrap();
    let mut settings = AiSettings::default();
    settings.provider_credentials[0].extra_headers = vec![nyaterm_core::AiHttpHeader {
        id: "header-fixture".into(),
        name: "Authorization".into(),
        value: "Bearer header-only-fixture".into(),
    }];
    assert!(settings.provider_credentials[0].api_key.is_none());
    let saved = store.save_ai_settings(settings).unwrap();
    let raw = store
        .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_DEFAULT)
        .unwrap()
        .unwrap();
    assert!(!raw.to_string().contains("header-only-fixture"));
    assert!(store.load_master_key_token().unwrap().is_some());
    assert_eq!(store.load_ai_settings().unwrap(), saved);
    assert_eq!(
        store
            .save_ai_settings(nyaterm_core::mask_ai_settings(saved.clone()))
            .unwrap(),
        saved
    );

    let restore_dir = unique_temp_dir("ai-header-only-restore");
    let restore = ConnectionStore::open(&restore_dir).unwrap();
    for kind in [PortableSnapshotKind::Backup, PortableSnapshotKind::Sync] {
        let mut snapshot = store
            .build_raw_portable_snapshot(kind, "fixture", "2.0.0")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        assert!(!snapshot.entities["settings"].contains("header-only-fixture"));
        restore.apply_raw_portable_snapshot(&snapshot).unwrap();
        assert_eq!(restore.load_ai_settings().unwrap(), saved);
    }
    for (name, value) in [
        ("Host", "test"),
        ("X-Token", "line\r\nbreak"),
        ("X-Token", "中文"),
        ("X-Token", ""),
    ] {
        let mut next = saved.clone();
        next.provider_credentials[0].extra_headers[0].name = name.into();
        next.provider_credentials[0].extra_headers[0].value = value.into();
        assert!(store.save_ai_settings(next).is_err());
        assert_eq!(store.load_ai_settings().unwrap(), saved);
    }
    let mut duplicate = saved.clone();
    let mut header = duplicate.provider_credentials[0].extra_headers[0].clone();
    header.id = "other".into();
    header.name = "AUTHORIZATION".into();
    duplicate.provider_credentials[0].extra_headers.push(header);
    assert!(store.save_ai_settings(duplicate).is_err());
    assert_eq!(store.load_ai_settings().unwrap(), saved);

    store
        .save_master_password(Some("header-sync-password"))
        .unwrap();
    let mut snapshot = store
        .build_raw_portable_snapshot(PortableSnapshotKind::Sync, "fixture", "2.0.0")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    restore
        .save_master_password(Some("header-local-password"))
        .unwrap();
    assert!(
        restore
            .apply_cloud_sync_snapshot(&restore_dir, &snapshot, "wrong-fixture-password")
            .is_err()
    );
    restore
        .apply_cloud_sync_snapshot(&restore_dir, &snapshot, "header-sync-password")
        .unwrap();
    assert_eq!(restore.load_ai_settings().unwrap(), saved);

    let mut corrupted = store.load_settings_value().unwrap();
    corrupted["ai"]["provider_credentials"][0]["extra_headers"][0]["value"] =
        serde_json::json!("enc:v1:deliberately-corrupted-fixture");
    store.save_settings_value(&corrupted).unwrap();
    assert!(store.load_ai_settings().is_err());
    assert!(store.save_ai_settings(saved).is_err());
    assert_eq!(store.load_settings_value().unwrap(), corrupted);
}

#[test]
fn repaired_ai_header_ids_preserve_encrypted_values_and_unknown_extensions() {
    let dir = unique_temp_dir("ai-header-import-ids");
    let store = ConnectionStore::open(&dir).unwrap();
    let mut settings = AiSettings::default();
    for credential in settings.provider_credentials.iter_mut().take(2) {
        credential.extra_headers = vec![
            nyaterm_core::AiHttpHeader {
                id: "first".into(),
                name: "X-First".into(),
                value: "first-fixture".into(),
            },
            nyaterm_core::AiHttpHeader {
                id: "second".into(),
                name: "X-Second".into(),
                value: "second-fixture".into(),
            },
        ];
    }
    store.save_ai_settings(settings).unwrap();
    let mut imported = store.load_settings_value().unwrap();
    for credential in imported["ai"]["provider_credentials"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .take(2)
    {
        let headers = credential["extra_headers"].as_array_mut().unwrap();
        headers[0]["id"] = serde_json::json!("shared");
        headers[0]["future_header"] = serde_json::json!({"keep": "first-extension"});
        headers[1]["id"] = serde_json::json!("shared");
        headers[1]["future_header"] = serde_json::json!({"keep": "second-extension"});
    }
    store.save_settings_value(&imported).unwrap();
    let loaded = store.load_ai_settings().unwrap();
    let saved = store
        .save_ai_settings(nyaterm_core::mask_ai_settings(loaded.clone()))
        .unwrap();
    assert_eq!(saved, loaded);
    let raw = store.load_settings_value().unwrap();
    for credential in raw["ai"]["provider_credentials"]
        .as_array()
        .unwrap()
        .iter()
        .take(2)
    {
        assert_eq!(
            credential["extra_headers"][0]["future_header"]["keep"],
            "first-extension"
        );
        assert_eq!(
            credential["extra_headers"][1]["future_header"]["keep"],
            "second-extension"
        );
    }
    assert!(!raw.to_string().contains("first-fixture"));
    assert!(!raw.to_string().contains("second-fixture"));
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
    settings.proxy.password = Some("proxy-fixture-key".into());
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
        raw_ai["proxy"]["password"].as_str(),
        Some("proxy-fixture-key")
    );
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

    assert_eq!(loaded.proxy.password.as_deref(), Some("proxy-fixture-key"));
    let restore_dir = unique_temp_dir("ai-proxy-portable-roundtrip");
    let restore = ConnectionStore::open(&restore_dir).unwrap();
    for kind in [PortableSnapshotKind::Backup, PortableSnapshotKind::Sync] {
        let mut snapshot = store
            .build_raw_portable_snapshot(kind, "fixture", "2.0.0")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        assert!(!snapshot.entities["settings"].contains("proxy-fixture-key"));
        restore.apply_raw_portable_snapshot(&snapshot).unwrap();
        assert_eq!(restore.load_ai_settings().unwrap().proxy, loaded.proxy);
    }
    let mut masked_update = loaded.clone();
    masked_update.proxy.password = Some(nyaterm_core::MASKED_SECRET_VALUE.into());
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
    assert_eq!(merged.proxy.password.as_deref(), Some("proxy-fixture-key"));
    assert_eq!(store.load_ai_settings().expect("reload ai"), merged);
    let mut cleared = merged;
    cleared.proxy.password = Some("".into());
    assert!(
        store
            .save_ai_settings(cleared)
            .unwrap()
            .proxy
            .password
            .is_none()
    );

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
    assert_eq!(loaded.schema_version, 7);
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

    let mut loaded = store.load_ai_settings().expect("load settings");
    assert_eq!(loaded.schema_version, 7);
    assert_eq!(
        loaded.external_mcp.terminal_display_mode,
        nyaterm_core::McpTerminalDisplayMode::Inline
    );
    loaded.external_mcp.terminal_display_mode = nyaterm_core::McpTerminalDisplayMode::Silent;
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
    assert_eq!(ai["external_mcp"]["terminal_display_mode"], "silent");
    assert!(ai["external_mcp"].get("server_mode").is_none());
    assert!(ai["external_mcp"].get("idle_timeout_minutes").is_none());
    let reloaded = store.load_ai_settings().expect("reload settings");
    assert_eq!(
        reloaded.external_mcp.terminal_display_mode,
        nyaterm_core::McpTerminalDisplayMode::Silent
    );

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
                raw_send: None,
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
                raw_send: None,
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
fn quick_command_literal_mode_survives_storage_backup_and_sync_with_legacy_commands() {
    let dir = unique_temp_dir("quick-command-literal");
    let store = ConnectionStore::open(&dir).unwrap();
    let config: QuickCommandsConfig = serde_json::from_value(serde_json::json!({
        "commands": [
            {"id":"legacy", "label":"Legacy", "command":"pwd"},
            {"id":"null", "label":"Null", "command":"pwd", "raw_send":null},
            {"id":"literal", "label":"Literal", "command":"  {{变量}} C:\\temp\\p\n下一行  ", "raw_send":true}
        ]
    })).unwrap();
    // Seed the legacy document as written by older versions, before a typed save.
    store
        .save_settings_doc_value(
            SETTINGS_QUICK_COMMANDS,
            &serde_json::json!({
                "commands": [{"id":"old", "label":"Old", "command":"pwd"}]
            }),
        )
        .unwrap();
    assert_eq!(
        store.load_quick_commands().unwrap().commands[0].raw_send,
        None
    );
    store.save_quick_commands(config.clone()).unwrap();
    drop(store);
    let store = ConnectionStore::open(&dir).unwrap();
    assert_eq!(store.load_quick_commands().unwrap(), config);
    for kind in [
        nyaterm_core::portable_snapshot::PortableSnapshotKind::Backup,
        nyaterm_core::portable_snapshot::PortableSnapshotKind::Sync,
    ] {
        let mut snapshot = store
            .build_raw_portable_snapshot(kind, "quick-test", "2.0.1")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let encoded = crate::portable_codec::encode_raw_portable_snapshot(&snapshot).unwrap();
        let decoded = crate::portable_codec::decode_raw_portable_snapshot(&encoded).unwrap();
        store
            .save_quick_commands(QuickCommandsConfig::default())
            .unwrap();
        store.apply_raw_portable_snapshot(&decoded).unwrap();
        assert_eq!(store.load_quick_commands().unwrap(), config);
    }
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
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
            raw_send: None,
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
            tags: Vec::new(),
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

#[test]
fn command_history_merges_legacy_suggestion_rewrites_without_rewriting_raw_data() {
    let dir = unique_temp_dir("suggestion-history-compat");
    let store = ConnectionStore::open(&dir).expect("store");
    let legacy = [
        (
            "command_history/00000000000000000010|legacy-1",
            "\u{05}\u{15}ps -ef",
            2,
            10,
        ),
        (
            "command_history/00000000000000000020|legacy-2",
            "\u{05}\u{15}\u{05}\u{15}ps -ef",
            3,
            20,
        ),
    ];
    let txn = store.db.begin_write().expect("write transaction");
    for (key, command, use_count, last_used_at_ms) in legacy {
        let mut record = serde_json::json!({
            "command": command,
            "last_used_at_ms": last_used_at_ms,
            "use_count": use_count,
        });
        if key.ends_with("legacy-2") {
            record["future_field"] = serde_json::json!({ "keep": true });
        }
        write_json_in_txn(&txn, COMMAND_HISTORY_TABLE, key, &record).expect("write legacy history");
    }
    txn.commit().expect("commit legacy history");

    let raw_before_read = store
        .list_raw_by_prefix(COMMAND_HISTORY_TABLE, COMMAND_HISTORY_PREFIX)
        .expect("raw history");
    assert_eq!(raw_before_read.len(), 2);

    let history = store.list_command_history(1).expect("merged history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].command, "ps -ef");
    assert_eq!(history[0].use_count, 5);
    assert_eq!(history[0].last_used_at_ms, 20);
    assert_eq!(
        store
            .list_raw_by_prefix(COMMAND_HISTORY_TABLE, COMMAND_HISTORY_PREFIX)
            .expect("raw history after read"),
        raw_before_read
    );

    store
        .append_command_history("ps -ef")
        .expect("append canonical");
    let history = store
        .list_command_history(10)
        .expect("merged after first append");
    assert_eq!(history[0].use_count, 6);
    assert!(history[0].last_used_at_ms >= 20);
    assert_eq!(
        store
            .list_raw_by_prefix(COMMAND_HISTORY_TABLE, COMMAND_HISTORY_PREFIX)
            .expect("raw history after append")
            .len(),
        3
    );

    store
        .append_command_history("ps -ef")
        .expect("append again");
    let history = store.list_command_history(10).expect("merged after append");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].use_count, 7);

    store
        .delete_command_history("ps -ef")
        .expect("delete all variants");
    assert!(
        store
            .list_command_history(10)
            .expect("history after delete")
            .is_empty()
    );
    assert!(
        store
            .list_raw_by_prefix(COMMAND_HISTORY_TABLE, COMMAND_HISTORY_PREFIX)
            .expect("raw history after delete")
            .is_empty()
    );

    store
        .replace_command_history(&[
            CommandHistoryEntry {
                command: "\u{05}\u{15}ps -ef".to_string(),
                last_used_at_ms: 30,
                use_count: 2,
            },
            CommandHistoryEntry {
                command: "ps -ef".to_string(),
                last_used_at_ms: 40,
                use_count: 4,
            },
        ])
        .expect("import legacy backup history");
    let imported = store.list_command_history(10).expect("imported history");
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].command, "ps -ef");
    assert_eq!(imported[0].use_count, 6);
    assert_eq!(imported[0].last_used_at_ms, 40);
}

#[test]
fn legacy_panel_visibility_migrates_once_and_terminal_saves_preserve_legacy_keys() {
    let dir = unique_temp_dir("panel-visibility-migration");
    let legacy = serde_json::json!({"ui": {
        "show_notes_panel": false, "show_remote_stats": false,
        "show_gpu_monitor": true, "show_ascend_npu_monitor": false,
        "show_process_manager": false, "show_docker_manager": true,
        "activity_bar_layout": {"right_top": ["gpuMonitor", "notes"], "hidden_items": ["dockerManager"], "future": 17}
    }, "future": {"keep": true}});
    {
        let store = ConnectionStore::open(&dir).unwrap();
        store.save_settings_value(&legacy).unwrap();
    }
    {
        let store = ConnectionStore::open(&dir).unwrap();
        let raw = store.load_settings_value().unwrap();
        assert_eq!(
            raw["ui"]["activity_bar_layout"]["legacy_visibility_migrated"],
            true
        );
        assert_eq!(
            raw["ui"]["activity_bar_layout"]["hidden_items"],
            serde_json::json!([
                "dockerManager",
                "notes",
                "resourceMonitor",
                "ascendNpuMonitor",
                "processManager"
            ])
        );
        assert_eq!(
            raw["ui"]["activity_bar_layout"]["right_top"],
            legacy["ui"]["activity_bar_layout"]["right_top"]
        );
        let mut summary = store.load_app_settings_summary().unwrap();
        summary.ui_activity_bar_hidden_items.clear();
        store.save_ui_layout_settings(&summary).unwrap();
        summary.ui_gpu_monitor_interval = 19;
        store.save_terminal_settings(&summary).unwrap();
        let raw = store.load_settings_value().unwrap();
        assert_eq!(raw["ui"]["show_notes_panel"], false);
        assert_eq!(raw["ui"]["show_gpu_monitor"], true);
        assert_eq!(raw["ui"]["activity_bar_layout"]["future"], 17);
        assert_eq!(raw["future"], legacy["future"]);
    }
    let store = ConnectionStore::open(&dir).unwrap();
    assert!(
        store
            .load_app_settings_summary()
            .unwrap()
            .ui_activity_bar_hidden_items
            .is_empty()
    );
    assert_eq!(
        store
            .load_app_settings_summary()
            .unwrap()
            .ui_gpu_monitor_interval,
        19
    );
}

#[test]
fn old_backup_and_sync_visibility_are_migrated_before_merging_destination_marker() {
    for kind in [PortableSnapshotKind::Backup, PortableSnapshotKind::Sync] {
        let source_dir = unique_temp_dir("panel-visibility-source");
        let target_dir = unique_temp_dir("panel-visibility-target");
        let source = ConnectionStore::open(&source_dir).unwrap();
        source.save_settings_value(&serde_json::json!({"ui": {
            "show_remote_stats": false, "show_gpu_monitor": true,
            "activity_bar_layout": {"hidden_items": ["notes"], "right_top": ["gpuMonitor", "resourceMonitor"], "future": 23}
        }})).unwrap();
        let mut snapshot = source
            .build_raw_portable_snapshot(kind.clone(), "source", "legacy")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let target = ConnectionStore::open(&target_dir).unwrap();
        let mut current = target.load_app_settings_summary().unwrap();
        current.ui_left_panel_width = 313;
        target.save_ui_layout_settings(&current).unwrap();
        target.apply_raw_portable_snapshot(&snapshot).unwrap();
        let raw = target.load_settings_value().unwrap();
        assert_eq!(
            raw["ui"]["activity_bar_layout"]["hidden_items"],
            serde_json::json!(["notes", "resourceMonitor", "ascendNpuMonitor"])
        );
        assert_eq!(raw["ui"]["activity_bar_layout"]["future"], 23);
        assert_eq!(
            raw["ui"]["activity_bar_layout"]["legacy_visibility_migrated"],
            true
        );
        if kind == PortableSnapshotKind::Sync {
            assert_eq!(
                target
                    .load_app_settings_summary()
                    .unwrap()
                    .ui_left_panel_width,
                313
            );
        }
        let mut summary = target.load_app_settings_summary().unwrap();
        summary.ui_activity_bar_hidden_items.clear();
        target.save_ui_layout_settings(&summary).unwrap();
        let mut round_trip = target
            .build_raw_portable_snapshot(kind, "target", "new")
            .unwrap();
        round_trip.recalculate_hash().unwrap();
        source.apply_raw_portable_snapshot(&round_trip).unwrap();
        assert!(
            source
                .load_app_settings_summary()
                .unwrap()
                .ui_activity_bar_hidden_items
                .is_empty()
        );
    }
}

#[test]
fn invalid_visibility_import_and_open_leave_original_documents_intact() {
    let source_dir = unique_temp_dir("invalid-visibility-source");
    let target_dir = unique_temp_dir("invalid-visibility-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    let invalid =
        serde_json::json!({"ui": {"activity_bar_layout": {"hidden_items": [42]}}, "future": 9});
    source.save_settings_value(&invalid).unwrap();
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "source", "legacy")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    let before = target.load_settings_value().unwrap();
    assert!(target.apply_raw_portable_snapshot(&snapshot).is_err());
    assert_eq!(target.load_settings_value().unwrap(), before);
    assert!(source.migrate_legacy_panel_visibility().is_err());
    assert_eq!(source.load_settings_value().unwrap(), invalid);
    drop(source);
    assert!(ConnectionStore::open(&source_dir).is_err());
}

pub(super) fn unique_temp_dir(name: &str) -> TestTempDir {
    TestTempDir::new(&format!("nyaterm-core-{name}"))
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
        tags: Vec::new(),
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

pub(super) fn connection_password_record(
    store: &ConnectionStore,
    connection_id: &str,
) -> ConnectionPasswordRecord {
    let txn = store.db.begin_read().expect("read transaction");
    let table = txn
        .open_table(CREDENTIALS_TABLE)
        .expect("credentials table");
    let raw = table
        .get(entity_key(CONNECTION_PASSWORD_PREFIX, connection_id).as_str())
        .expect("read connection password")
        .expect("connection password record");
    deserialize_json(raw.value()).expect("deserialize connection password")
}

#[test]
fn connection_inline_password_is_encrypted_at_rest_and_roundtrips() {
    let dir = unique_temp_dir("connection-inline-password-roundtrip");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("inline-password");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some("hunter2".to_string().into()),
        has_password: false,
        ..ConnectionAuth::default()
    });

    store.save_connection(&connection).expect("save connection");

    let record = connection_password_record(&store, "inline-password");
    assert_ne!(record.password.expose_secret(), "hunter2");
    let crypto = store.credential_crypto().expect("credential crypto");
    let master_key = store
        .load_master_key_token()
        .expect("load master key")
        .expect("master key");
    assert_eq!(
        crypto
            .decrypt_secret(&master_key, record.password.expose_secret())
            .expect("decrypt stored password"),
        "hunter2"
    );

    let loaded = store
        .get_connection("inline-password")
        .expect("load connection")
        .expect("connection");
    let auth = loaded.auth.expect("auth");
    assert_eq!(auth.password.as_deref(), Some("hunter2"));
    assert!(!auth.has_password);
    let (_, password) = auth
        .resolve_account_auth("root", None)
        .expect("resolve inline password");
    assert_eq!(password.as_deref(), Some("hunter2"));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn connection_inline_password_whitespace_survives_storage_reopen_and_auth_resolution() {
    let dir = unique_temp_dir("connection-password-whitespace");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("password-whitespace");
    let password = " \tfixture-password\u{2003} ";
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some(password.into()),
        ..ConnectionAuth::default()
    });
    store.save_connection(&connection).expect("save connection");
    let record = connection_password_record(&store, &connection.id);
    assert_ne!(record.password.expose_secret(), password);
    drop(store);

    let store = ConnectionStore::open(&dir).expect("reopen store");
    let loaded = store
        .get_connection(&connection.id)
        .expect("load connection")
        .expect("connection");
    let auth = loaded.auth.expect("auth");
    assert!(!auth.has_password);
    let (username, resolved) = auth
        .resolve_account_auth("non-root", None)
        .expect("resolve inline password");
    assert_eq!(username, "non-root");
    assert_eq!(resolved.as_deref(), Some(password));

    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn save_group_and_connection_encrypts_inline_password() {
    let dir = unique_temp_dir("group-connection-inline-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let group = Group {
        id: "group-secret".to_string(),
        name: "Servers".to_string(),
        parent_id: None,
        sort_order: 0,
        created_at_ms: None,
        updated_at_ms: None,
    };
    let mut connection = ssh_connection_for_asset("group-inline-password");
    connection.group_id = Some(group.id.clone());
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some("group-secret".to_string().into()),
        ..ConnectionAuth::default()
    });

    store
        .save_group_and_connection(&group, &connection)
        .expect("save group and connection");

    let record = connection_password_record(&store, "group-inline-password");
    assert_ne!(record.password.expose_secret(), "group-secret");
    let loaded = store
        .get_connection("group-inline-password")
        .expect("load connection")
        .expect("connection");
    let auth = loaded.auth.expect("auth");
    assert_eq!(auth.password.as_deref(), Some("group-secret"));
    assert!(!auth.has_password);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn locked_connection_ciphertext_is_not_reencrypted_or_unlocked() {
    let dir = unique_temp_dir("locked-connection-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let locked_ciphertext = encrypt_for_test(b"locked-secret", &test_key(41));
    let mut connection = ssh_connection_for_asset("locked-inline-password");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some(locked_ciphertext.clone().into()),
        has_password: true,
        ..ConnectionAuth::default()
    });

    store
        .save_connection(&connection)
        .expect("save locked connection");
    assert_eq!(
        connection_password_record(&store, "locked-inline-password")
            .password
            .expose_secret(),
        locked_ciphertext
    );

    let loaded = store
        .get_connection("locked-inline-password")
        .expect("load connection")
        .expect("connection");
    let auth = loaded.auth.as_ref().expect("auth");
    assert!(auth.has_password);
    assert_eq!(auth.password.as_deref(), Some(locked_ciphertext.as_str()));
    assert_eq!(
        auth.resolve_account_auth("root", None),
        Err(AccountAuthError::LockedConnectionPassword)
    );

    store
        .save_connection(&loaded)
        .expect("resave locked connection");
    assert_eq!(
        connection_password_record(&store, "locked-inline-password")
            .password
            .expose_secret(),
        locked_ciphertext
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn preview_plaintext_connection_password_is_migrated_immediately() {
    let dir = unique_temp_dir("preview-plaintext-connection-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("preview-inline-password");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        ..ConnectionAuth::default()
    });
    store.save_connection(&connection).expect("save connection");
    let now = current_time_ms();
    let plaintext_record = ConnectionPasswordRecord {
        id: connection.id.clone(),
        connection_id: connection.id.clone(),
        password: "preview-secret!".to_string().into(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    {
        let txn = store.db.begin_write().expect("write transaction");
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(CONNECTION_PASSWORD_PREFIX, &connection.id),
            &plaintext_record,
        )
        .expect("write preview plaintext");
        txn.commit().expect("commit preview plaintext");
    }

    let loaded = store
        .get_connection(&connection.id)
        .expect("load connection")
        .expect("connection");
    let auth = loaded.auth.expect("auth");
    assert_eq!(auth.password.as_deref(), Some("preview-secret!"));
    assert!(!auth.has_password);

    let migrated = connection_password_record(&store, &connection.id);
    assert_ne!(migrated.password.expose_secret(), "preview-secret!");
    let crypto = store.credential_crypto().expect("credential crypto");
    let master_key = store
        .load_master_key_token()
        .expect("load master key")
        .expect("master key");
    assert_eq!(
        crypto
            .decrypt_secret(&master_key, migrated.password.expose_secret())
            .expect("decrypt migrated password"),
        "preview-secret!"
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn possible_ciphertext_with_failed_authentication_stays_locked() {
    let dir = unique_temp_dir("possible-ciphertext-stays-locked");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("ambiguous-inline-password");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        ..ConnectionAuth::default()
    });
    store.save_connection(&connection).expect("save connection");
    let possible_ciphertext = B64.encode([17_u8; 28]);
    let now = current_time_ms();
    let record = ConnectionPasswordRecord {
        id: connection.id.clone(),
        connection_id: connection.id.clone(),
        password: possible_ciphertext.clone().into(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    {
        let txn = store.db.begin_write().expect("write transaction");
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(CONNECTION_PASSWORD_PREFIX, &connection.id),
            &record,
        )
        .expect("write possible ciphertext");
        txn.commit().expect("commit possible ciphertext");
    }

    let auth = store
        .get_connection(&connection.id)
        .expect("load connection")
        .expect("connection")
        .auth
        .expect("auth");
    assert!(auth.has_password);
    assert_eq!(auth.password.as_deref(), Some(possible_ciphertext.as_str()));
    assert_eq!(
        auth.resolve_account_auth("root", None),
        Err(AccountAuthError::LockedConnectionPassword)
    );
    assert_eq!(
        connection_password_record(&store, &connection.id)
            .password
            .expose_secret(),
        possible_ciphertext
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn replace_sessions_encrypts_plaintext_and_preserves_locked_ciphertext() {
    let dir = unique_temp_dir("replace-inline-passwords");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut unlocked = ssh_connection_for_asset("replace-unlocked");
    unlocked.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some("replace-secret".to_string().into()),
        ..ConnectionAuth::default()
    });
    let locked_ciphertext = encrypt_for_test(b"locked-secret", &test_key(43));
    let mut locked = ssh_connection_for_asset("replace-locked");
    locked.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some(locked_ciphertext.clone().into()),
        has_password: true,
        ..ConnectionAuth::default()
    });
    let config = SessionsConfig {
        connections: vec![unlocked, locked],
        ..SessionsConfig::default()
    };

    store.replace_sessions(&config).expect("replace sessions");

    assert_ne!(
        connection_password_record(&store, "replace-unlocked")
            .password
            .expose_secret(),
        "replace-secret"
    );
    assert_eq!(
        connection_password_record(&store, "replace-locked")
            .password
            .expose_secret(),
        locked_ciphertext
    );
    let loaded = store.load_sessions().expect("load sessions");
    let unlocked_auth = loaded
        .connections
        .iter()
        .find(|connection| connection.id == "replace-unlocked")
        .and_then(|connection| connection.auth.as_ref())
        .expect("unlocked auth");
    assert_eq!(unlocked_auth.password.as_deref(), Some("replace-secret"));
    assert!(!unlocked_auth.has_password);
    let locked_auth = loaded
        .connections
        .iter()
        .find(|connection| connection.id == "replace-locked")
        .and_then(|connection| connection.auth.as_ref())
        .expect("locked auth");
    assert!(locked_auth.has_password);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn metadata_only_connection_saves_keep_inline_password_unlocked() {
    let dir = unique_temp_dir("connection-metadata-inline-password");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("metadata-inline-password");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some("metadata-secret".to_string().into()),
        ..ConnectionAuth::default()
    });
    store.save_connection(&connection).expect("initial save");

    let mut loaded = store
        .get_connection(&connection.id)
        .expect("load connection")
        .expect("connection");
    loaded.sort_order = 42;
    loaded.icon_auto_detect = Some(true);
    store.save_connection(&loaded).expect("save metadata");

    let reloaded = store
        .get_connection(&connection.id)
        .expect("reload connection")
        .expect("connection");
    let auth = reloaded.auth.expect("auth");
    assert_eq!(auth.password.as_deref(), Some("metadata-secret"));
    assert!(!auth.has_password);
    assert_eq!(reloaded.sort_order, 42);
    let record = connection_password_record(&store, &connection.id);
    assert_ne!(record.password.expose_secret(), "metadata-secret");
    let crypto = store.credential_crypto().expect("crypto");
    let token = store
        .load_master_key_token()
        .expect("load master key")
        .expect("master key");
    assert_eq!(
        crypto
            .decrypt_secret(&token, record.password.expose_secret())
            .expect("decrypt metadata save"),
        "metadata-secret"
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn portable_snapshot_restore_encrypts_connection_inline_password() {
    let source_dir = unique_temp_dir("portable-inline-password-source");
    let target_dir = unique_temp_dir("portable-inline-password-target");
    let source = ConnectionStore::open(&source_dir).expect("source store");
    let mut connection = ssh_connection_for_asset("portable-inline-password");
    connection.auth = Some(ConnectionAuth {
        mode: "password".to_string(),
        password: Some("portable-secret".to_string().into()),
        ..ConnectionAuth::default()
    });
    source.save_connection(&connection).expect("save source");
    let mut snapshot = source
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Backup, "device", "test")
        .expect("build snapshot");
    snapshot.recalculate_hash().expect("hash snapshot");

    let target = ConnectionStore::open(&target_dir).expect("target store");
    target
        .apply_raw_portable_snapshot(&snapshot)
        .expect("restore snapshot");

    let record = connection_password_record(&target, &connection.id);
    assert_ne!(record.password.expose_secret(), "portable-secret");
    let restored = target
        .get_connection(&connection.id)
        .expect("load restored connection")
        .expect("connection");
    let auth = restored.auth.expect("auth");
    assert_eq!(auth.password.as_deref(), Some("portable-secret"));
    assert!(!auth.has_password);

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
}

#[test]
fn merge_connection_asset_from_monitoring_creates_and_merges_atomically() {
    let dir = unique_temp_dir("asset-merge");
    let store = ConnectionStore::open(&dir).expect("store");
    let mut connection = ssh_connection_for_asset("asset-1");
    connection.tags = vec!["production".to_string()];
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
    assert!(!auth.has_password);
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
    let workspace_id = nyaterm_core::WorkspaceId::new();
    source
        .save_device_window_manifest(&nyaterm_core::DeviceWindowManifest {
            version: nyaterm_core::DEVICE_WINDOW_MANIFEST_VERSION,
            windows: vec![nyaterm_core::DeviceWindowState {
                workspace_id,
                window: MainWindowState::new(
                    None,
                    MainWindowBounds {
                        x: 30,
                        y: 40,
                        width: 800,
                        height: 600,
                    },
                    false,
                ),
                extra: Default::default(),
            }],
            window_order: vec![workspace_id],
            most_recent_workspace_id: Some(workspace_id),
            extra: Default::default(),
        })
        .expect("save device windows");
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

#[test]
fn tauri_ai_extensions_follow_account_and_model_ids_through_reorder_delete_and_backup() {
    let dir = unique_temp_dir("ai-tauri-fields");
    let restore_dir = unique_temp_dir("ai-tauri-restore");
    let backup = dir.join("tauri-ai.nya");
    let store = ConnectionStore::open(&dir).unwrap();
    let mut raw = default_settings_value();
    raw["ai"] = serde_json::json!({
        "provider_credentials": [
            {"id":"first","name":"First","provider_kind":"openai","api_protocol":"openai_compatible","icon_data_url":"data:image/png;base64,fixture","future_account":"first-extension"},
            {"id":"second","name":"Second","provider_kind":"openai","api_protocol":"anthropic","future_account":"second-extension"}
        ],
        "models": [
            {"id":"first:model","name":"model","credential_id":"first","provider_kind":"openai","supported_reasoning_efforts":["minimal","max","ultra"],"future_model":"first-extension"},
            {"id":"second:model","name":"model","credential_id":"second","provider_kind":"openai","supported_reasoning_efforts":[],"future_model":"second-extension"}
        ],"default_model_id":"first:model","future_ai":{"keep":true}
    });
    store.save_settings_value(&raw).unwrap();
    let mut settings = store.load_ai_settings().unwrap();
    settings.provider_credentials[0].api_key = Some("fixture-first".into());
    settings.provider_credentials[1].api_key = Some("fixture-second".into());
    let mut settings = store.save_ai_settings(settings).unwrap();
    settings.provider_credentials.reverse();
    settings.models.reverse();
    let saved = store
        .save_ai_settings(nyaterm_core::mask_ai_settings(settings))
        .unwrap();
    assert_eq!(
        saved.provider_credentials[0].api_key.as_deref(),
        Some("fixture-second")
    );
    assert_eq!(
        saved.provider_credentials[1].api_key.as_deref(),
        Some("fixture-first")
    );
    let raw = store.load_settings_value().unwrap();
    assert_eq!(
        raw["ai"]["provider_credentials"][0]["future_account"],
        "second-extension"
    );
    assert_eq!(
        raw["ai"]["provider_credentials"][1]["future_account"],
        "first-extension"
    );
    assert_eq!(raw["ai"]["models"][0]["future_model"], "second-extension");
    assert_eq!(raw["ai"]["models"][1]["future_model"], "first-extension");
    assert_ne!(
        raw["ai"]["provider_credentials"][0]["api_key"].as_str(),
        Some("fixture-second")
    );
    assert_eq!(
        raw["ai"]["provider_credentials"][1]["icon_data_url"],
        "data:image/png;base64,fixture"
    );
    assert_eq!(
        raw["ai"]["provider_credentials"][0]["api_protocol"],
        "anthropic"
    );
    assert_eq!(
        raw["ai"]["models"][1]["supported_reasoning_efforts"],
        serde_json::json!(["minimal", "max", "ultra"])
    );
    let sync_dir = unique_temp_dir("ai-tauri-sync");
    store
        .save_master_password(Some("fixture-sync-password"))
        .unwrap();
    let mut snapshot = store
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Sync, "fixture", "2.0.0")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    let synced = ConnectionStore::open(&sync_dir).unwrap();
    synced
        .save_master_password(Some("fixture-local-password"))
        .unwrap();
    assert!(
        synced
            .apply_cloud_sync_snapshot(&sync_dir, &snapshot, "fixture-wrong-password")
            .is_err()
    );
    synced
        .apply_cloud_sync_snapshot(&sync_dir, &snapshot, "fixture-sync-password")
        .unwrap();
    assert_eq!(synced.load_ai_settings().unwrap(), saved);
    assert_eq!(
        synced.load_settings_value().unwrap()["ai"]["models"][1]["future_model"],
        "first-extension"
    );
    drop(synced);
    std::fs::remove_dir_all(sync_dir).ok();
    store.save_master_password(None).unwrap();
    let mut updated = saved;
    updated
        .provider_credentials
        .retain(|item| item.id != "first");
    updated
        .models
        .retain(|item| item.credential_id.as_deref() != Some("first"));
    let saved = store.save_ai_settings(updated).unwrap();
    assert_eq!(saved.default_model_id.as_deref(), Some("second:model"));
    let raw = store.load_settings_value().unwrap();
    assert_eq!(
        raw["ai"]["provider_credentials"].as_array().unwrap().len(),
        1
    );
    assert_eq!(raw["ai"]["models"][0]["future_model"], "second-extension");
    assert_eq!(raw["ai"]["future_ai"]["keep"], true);
    drop(store);
    ConnectionStore::export_config_database(&dir, None, &backup).unwrap();
    ConnectionStore::import_config_database(&restore_dir, None, &backup).unwrap();
    let restored = ConnectionStore::open(&restore_dir).unwrap();
    assert_eq!(restored.load_ai_settings().unwrap(), saved);
    assert_eq!(
        restored.load_settings_value().unwrap()["ai"]["models"][0]["future_model"],
        "second-extension"
    );
    drop(restored);
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(restore_dir).ok();
}

#[test]
fn vnc_trust_cas_and_portable_roundtrip_preserve_tauri_records() {
    let source_dir = unique_temp_dir("vnc-trust-source");
    let target_dir = unique_temp_dir("vnc-trust-target");
    let source = ConnectionStore::open(&source_dir).unwrap();
    let target = ConnectionStore::open(&target_dir).unwrap();
    let first = format!("SHA256:{}", "a1".repeat(32));
    let next = format!("SHA256:{}", "b2".repeat(32));
    let checks = std::cell::Cell::new(0);
    assert!(
        !source
            .remember_vnc_known_host_if_current("cancelled", 5900, &first, None, &|| {
                checks.set(checks.get() + 1);
                checks.get() == 1
            })
            .unwrap()
    );
    assert!(
        source
            .load_vnc_known_host("cancelled", 5900)
            .unwrap()
            .is_none()
    );
    assert!(
        source
            .remember_vnc_known_host(" Example.COM ", 5900, &first, None)
            .unwrap()
    );
    assert!(
        !source
            .remember_vnc_known_host("example.com", 5900, &next, None)
            .unwrap()
    );
    assert!(
        source
            .remember_vnc_known_host("example.com", 5900, &next, Some(&first))
            .unwrap()
    );
    let record = source
        .load_vnc_known_host("EXAMPLE.COM", 5900)
        .unwrap()
        .unwrap();
    assert_eq!(record.sha256_fingerprint, next);
    for kind in [PortableSnapshotKind::Backup, PortableSnapshotKind::Sync] {
        let mut snapshot = source
            .build_raw_portable_snapshot(kind, "fixture", "2.0.0")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        target.apply_raw_portable_snapshot(&snapshot).unwrap();
        assert_eq!(
            target.load_vnc_known_host("example.com", 5900).unwrap(),
            Some(record.clone())
        );
        snapshot.entities.remove("vnc_known_hosts");
        snapshot.recalculate_hash().unwrap();
        target.apply_raw_portable_snapshot(&snapshot).unwrap();
        assert_eq!(
            target.load_vnc_known_host("example.com", 5900).unwrap(),
            Some(record.clone())
        );
        snapshot.entities.insert("vnc_known_hosts".into(), r#"[{"host":"x","port":5900,"sha256_fingerprint":"bad","created_at_ms":1,"updated_at_ms":1}]"#.into());
        snapshot.recalculate_hash().unwrap();
        assert!(target.apply_raw_portable_snapshot(&snapshot).is_err());
        assert_eq!(
            target.load_vnc_known_host("example.com", 5900).unwrap(),
            Some(record.clone())
        );
    }
}

#[test]
fn recording_input_opt_in_preserves_legacy_unknown_fields_sync_and_backup() {
    let source_dir = unique_temp_dir("recording-settings-source");
    let source = ConnectionStore::open(&source_dir).unwrap();
    source
        .save_settings_value(&serde_json::json!({
            "recording": { "path": "synthetic/logs", "future_option": { "retain": 7 } }
        }))
        .unwrap();
    let mut settings = source.load_app_settings_summary().unwrap();
    assert!(!settings.recording_include_input);
    settings.recording_include_input = true;
    source.save_recording_settings(&settings).unwrap();
    assert_eq!(
        source.load_settings_value().unwrap()["recording"]["future_option"]["retain"],
        7
    );
    for kind in [PortableSnapshotKind::Sync, PortableSnapshotKind::Backup] {
        let target_dir = unique_temp_dir("recording-settings-portable");
        let mut snapshot = source
            .build_raw_portable_snapshot(kind, "fixture", "2.0.0")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let target = ConnectionStore::open(&target_dir).unwrap();
        target.apply_raw_portable_snapshot(&snapshot).unwrap();
        assert!(
            target
                .load_app_settings_summary()
                .unwrap()
                .recording_include_input
        );
        assert_eq!(
            target.load_settings_value().unwrap()["recording"]["future_option"]["retain"],
            7
        );
        drop(target);
        std::fs::remove_dir_all(target_dir).ok();
    }
    drop(source);
    let backup_dir = unique_temp_dir("recording-settings-backup");
    let backup_path = backup_dir.join("recording.nya");
    let restore_dir = unique_temp_dir("recording-settings-restore");
    ConnectionStore::export_config_database(&source_dir, None, &backup_path).unwrap();
    ConnectionStore::import_config_database(&restore_dir, None, &backup_path).unwrap();
    let restored = ConnectionStore::open(&restore_dir).unwrap();
    assert!(
        restored
            .load_app_settings_summary()
            .unwrap()
            .recording_include_input
    );
    assert_eq!(
        restored.load_settings_value().unwrap()["recording"]["future_option"]["retain"],
        7
    );
    drop(restored);
    for dir in [source_dir, backup_dir, restore_dir] {
        std::fs::remove_dir_all(dir).ok();
    }
}

#[test]
fn telnet_keepalive_survives_storage_backup_and_sync_round_trips() {
    let dir = unique_temp_dir("telnet-keepalive");
    let store = ConnectionStore::open(&dir).unwrap();
    let config: SessionsConfig = serde_json::from_value(serde_json::json!({
        "groups": [], "connections": [
            {"id":"old", "name":"Old Telnet", "type":"telnet", "host":"old.invalid", "future_telnet":{"retained":true}},
            {"id":"disabled", "name":"Disabled", "type":"telnet", "host":"disabled.invalid", "keep_alive_interval_secs":0},
            {"id":"custom", "name":"Custom", "type":"telnet", "host":"custom.invalid", "keep_alive_interval_secs":45}
        ]
    })).unwrap();
    store.replace_sessions(&config).unwrap();
    let assert_connections = |store: &ConnectionStore| {
        let loaded = store.load_sessions().unwrap();
        for expected in &config.connections {
            let actual = loaded
                .connections
                .iter()
                .find(|connection| connection.id == expected.id)
                .unwrap();
            assert_eq!(actual.config, expected.config);
        }
        let old = loaded
            .connections
            .iter()
            .find(|connection| connection.id == "old")
            .unwrap();
        assert_eq!(
            serde_json::to_value(old).unwrap()["future_telnet"]["retained"],
            true
        );
    };
    assert_connections(&store);
    for kind in [
        nyaterm_core::portable_snapshot::PortableSnapshotKind::Backup,
        nyaterm_core::portable_snapshot::PortableSnapshotKind::Sync,
    ] {
        let mut snapshot = store
            .build_raw_portable_snapshot(kind, "telnet-test", "2.0.0")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let encoded = crate::portable_codec::encode_raw_portable_snapshot(&snapshot).unwrap();
        let decoded = crate::portable_codec::decode_raw_portable_snapshot(&encoded).unwrap();
        store.replace_sessions(&SessionsConfig::default()).unwrap();
        store.apply_raw_portable_snapshot(&decoded).unwrap();
        assert_connections(&store);
    }
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cloud_sync_rotated_credentials_survive_restart_and_reject_stale_accounts() {
    use nyaterm_core::cloud_sync::credentials::{
        CloudCredentialUpdate, CredentialUpdateResult, DriveProvider,
    };
    for provider in [
        DriveProvider::Google,
        DriveProvider::OneDrive,
        DriveProvider::Aliyun,
    ] {
        let dir = unique_temp_dir("cloud-token-rotation");
        let store = ConnectionStore::open(&dir).unwrap();
        let mut settings = CloudSyncSettings::default();
        match provider {
            DriveProvider::Google => {
                settings.google_drive.client_id = Some("client".into());
                settings.google_drive.refresh_token = Some("refresh-before".into());
            }
            DriveProvider::OneDrive => {
                settings.onedrive.client_id = Some("client".into());
                settings.onedrive.refresh_token = Some("refresh-before".into());
            }
            DriveProvider::Aliyun => {
                settings.aliyun_drive.client_id = Some("client".into());
                settings.aliyun_drive.refresh_token = Some("refresh-before".into());
            }
        }
        store.save_cloud_sync_settings(settings).unwrap();
        let mut raw = store.load_settings_value().unwrap();
        raw["cloud_sync"]["future_option"] = serde_json::json!({"keep": true});
        store.save_settings_value(&raw).unwrap();
        let mut update = CloudCredentialUpdate {
            provider,
            client_id: Some("client".into()),
            client_secret: None,
            expected_access_token: None,
            expected_refresh_token: Some("refresh-before".into()),
            access_token: "access-after".into(),
            refresh_token: Some("refresh-after".into()),
        };
        assert_eq!(
            store.update_cloud_credentials(&update).unwrap(),
            CredentialUpdateResult::Applied
        );
        assert_eq!(
            store.update_cloud_credentials(&update).unwrap(),
            CredentialUpdateResult::Superseded
        );
        // An access-only refresh must retain the last rotated refresh token.
        update.expected_access_token = Some("access-after".into());
        update.expected_refresh_token = Some("refresh-after".into());
        update.refresh_token = None;
        assert_eq!(
            store.update_cloud_credentials(&update).unwrap(),
            CredentialUpdateResult::Applied
        );
        let raw = store.load_settings_value().unwrap();
        assert_eq!(raw["cloud_sync"]["future_option"]["keep"], true);
        let text = serde_json::to_string(&raw).unwrap();
        assert!(!text.contains("access-after"));
        assert!(!text.contains("refresh-after"));
        drop(store);
        let reopened = ConnectionStore::open(&dir).unwrap();
        let settings = reopened.load_cloud_sync_settings().unwrap();
        let (access, refresh) = match provider {
            DriveProvider::Google => (
                settings.google_drive.access_token,
                settings.google_drive.refresh_token,
            ),
            DriveProvider::OneDrive => (
                settings.onedrive.access_token,
                settings.onedrive.refresh_token,
            ),
            DriveProvider::Aliyun => (
                settings.aliyun_drive.access_token,
                settings.aliyun_drive.refresh_token,
            ),
        };
        assert_eq!(access.as_deref(), Some("access-after"));
        assert_eq!(refresh.as_deref(), Some("refresh-after"));
        // Reauthorization may retain a refresh token but replace access credentials.
        let mut reauthorized = reopened.load_cloud_sync_settings().unwrap();
        match provider {
            DriveProvider::Google => {
                reauthorized.google_drive.access_token = Some("reauthorized".into())
            }
            DriveProvider::OneDrive => {
                reauthorized.onedrive.access_token = Some("reauthorized".into())
            }
            DriveProvider::Aliyun => {
                reauthorized.aliyun_drive.access_token = Some("reauthorized".into())
            }
        }
        reopened.save_cloud_sync_settings(reauthorized).unwrap();
        assert_eq!(
            reopened.update_cloud_credentials(&update).unwrap(),
            CredentialUpdateResult::Superseded
        );
        update.client_id = Some("replaced-account".into());
        assert_eq!(
            reopened.update_cloud_credentials(&update).unwrap(),
            CredentialUpdateResult::Superseded
        );
    }
}

#[test]
fn login_monitoring_setting_roundtrips_without_losing_unknown_fields() {
    let dir = unique_temp_dir("login-monitoring-settings");
    let store = ConnectionStore::open(&dir).unwrap();
    store.save_settings_value(&serde_json::json!({"ui":{"future_monitor_option":{"keep":true}},"future":{"version":99}})).unwrap();
    let mut settings = store.load_app_settings_summary().unwrap();
    assert!(!settings.ui_remote_stats_show_users);
    for enabled in [true, false] {
        settings.ui_remote_stats_show_users = enabled;
        store.save_terminal_settings(&settings).unwrap();
        assert_eq!(
            store
                .load_app_settings_summary()
                .unwrap()
                .ui_remote_stats_show_users,
            enabled
        );
        let raw = store.load_settings_value().unwrap();
        assert_eq!(raw["ui"]["future_monitor_option"]["keep"], true);
        assert_eq!(raw["future"]["version"], 99);
    }
}
