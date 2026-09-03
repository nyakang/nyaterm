use super::*;
use crate::config::{
    ConnectionAuth, ConnectionCustomIcon, ConnectionType, Group, SavedConnection, SessionsConfig,
    SftpSettings,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_config_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("nyaterm-redb-v3-{name}-{nanos}"))
}
fn test_storage(name: &str) -> (PathBuf, Storage) {
    let dir = unique_config_dir(name);
    fs::create_dir_all(&dir).expect("create temp dir");
    let storage = Storage::open(&dir).expect("open storage");
    (dir, storage)
}
fn sample_group(id: &str, sort_order: i32) -> Group {
    Group {
        id: id.to_string(),
        name: id.to_string(),
        parent_id: None,
        sort_order,
        created_at_ms: None,
        updated_at_ms: None,
    }
}
fn sample_connection(id: &str, group_id: Option<&str>, sort_order: i32) -> SavedConnection {
    SavedConnection {
        id: id.to_string(),
        name: id.to_string(),
        config: ConnectionType::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            legacy_agent_forwarding: None,
            agent_forwarding_config: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: group_id.map(str::to_string),
        description: None,
        sort_order,
        icon: None,
        icon_auto_detect: None,
        auth: Some(ConnectionAuth {
            mode: "password".to_string(),
            password_id: None,
            password: Some(format!("cipher-{id}")),
            key_id: None,
            otp_id: None,
            auto_fill_otp: false,
            has_password: false,
        }),
        network: None,
        post_login: None,
        recording: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: SftpSettings::default(),
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    }
}
fn sample_custom_icon(id: &str, data_url: &str) -> ConnectionCustomIcon {
    ConnectionCustomIcon {
        id: id.to_string(),
        name: id.to_string(),
        data_url: data_url.to_string(),
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}
#[test]
fn new_storage_initializes_schema_v3_without_json_files() {
    let (dir, storage) = test_storage("init");
    assert_eq!(storage.get_schema_version().expect("schema version"), 3);
    assert!(!dir.join("settings.json").exists());
    assert!(!dir.join("sessions.json").exists());
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn settings_roundtrip_uses_generic_json_bytes() {
    let (dir, storage) = test_storage("settings");
    let value = serde_json::json!({"theme": "dark"});
    storage
        .save_settings("settings/ui", &value)
        .expect("save settings");
    let loaded: serde_json::Value = storage
        .get_settings("settings/ui")
        .expect("get settings")
        .expect("settings exist");
    assert_eq!(loaded["theme"], "dark");
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn group_crud_roundtrip() {
    let (dir, storage) = test_storage("groups");
    let group = sample_group("group-a", 1);
    storage.save_group(&group).expect("save group");
    assert_eq!(storage.list_groups().expect("list groups").len(), 1);
    assert_eq!(
        storage
            .get_group("group-a")
            .expect("get group")
            .expect("group")
            .name,
        "group-a"
    );
    storage.delete_group("group-a").expect("delete group");
    assert!(storage.list_groups().expect("list groups").is_empty());
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn connection_crud_and_group_index_roundtrip() {
    let (dir, storage) = test_storage("connections");
    let mut one = sample_connection("one", Some("group-a"), 1);
    let two = sample_connection("two", Some("group-b"), 2);
    storage.save_connection(&one).expect("save one");
    storage.save_connection(&two).expect("save two");
    assert_eq!(storage.list_connections().expect("list").len(), 2);
    assert!(
        storage
            .get_connection("one")
            .expect("get one")
            .expect("one")
            .auth
            .and_then(|auth| auth.password)
            .is_none()
    );
    assert_eq!(
        storage
            .get_connection_with_secret("one")
            .expect("get one with secret")
            .expect("one")
            .auth
            .and_then(|auth| auth.password),
        Some("cipher-one".to_string())
    );
    storage
        .mark_connection_used("one")
        .expect("mark connection used");
    assert_eq!(
        storage
            .get_connection_with_secret("one")
            .expect("get one with secret after mark used")
            .expect("one")
            .auth
            .and_then(|auth| auth.password),
        Some("cipher-one".to_string())
    );
    let group_a = storage
        .list_connections_by_group(Some("group-a"))
        .expect("list group a");
    assert_eq!(
        group_a
            .iter()
            .map(|conn| conn.id.as_str())
            .collect::<Vec<_>>(),
        ["one"]
    );
    one.group_id = Some("group-b".to_string());
    one.auth.as_mut().expect("auth").password = Some("cipher-one".to_string());
    storage.save_connection(&one).expect("move one");
    assert!(
        storage
            .list_connections_by_group(Some("group-a"))
            .expect("list old group")
            .is_empty()
    );
    assert_eq!(
        storage
            .list_connections_by_group(Some("group-b"))
            .expect("list new group")
            .len(),
        2
    );
    storage.delete_connection("one").expect("delete one");
    assert_eq!(
        storage
            .list_connections_by_group(Some("group-b"))
            .expect("list group b")
            .iter()
            .map(|conn| conn.id.as_str())
            .collect::<Vec<_>>(),
        ["two"]
    );
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn history_appends_lists_and_deletes_by_timestamp() {
    let (dir, storage) = test_storage("history");
    storage
        .append_command_history(&crate::core::history::HistoryEntry {
            command: "ls".to_string(),
            last_used_at_ms: 10,
            use_count: 1,
        })
        .expect("append ls");
    storage
        .append_command_history(&crate::core::history::HistoryEntry {
            command: "pwd".to_string(),
            last_used_at_ms: 20,
            use_count: 1,
        })
        .expect("append pwd");
    let recent = storage
        .list_recent_command_history(10)
        .expect("recent history");
    assert_eq!(recent[0].command, "pwd");
    storage
        .delete_command_history_before(15)
        .expect("delete old history");
    let remaining = storage
        .list_recent_command_history(10)
        .expect("remaining history");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].command, "pwd");
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn v1_migration_splits_sessions_deletes_legacy_tables_and_keeps_external_backup() {
    let dir = unique_config_dir("migration");
    fs::create_dir_all(&dir).expect("create temp dir");
    let db_path = database_path(&dir);
    {
        let db = redb::Database::create(&db_path).expect("create legacy db");
        let txn = db.begin_write().expect("begin legacy write");
        {
            let mut json = txn
                .open_table(tables::JSON_DOCS_TABLE)
                .expect("legacy json table");
            let sessions = SessionsConfig {
                groups: vec![sample_group("group-a", 1)],
                connections: vec![sample_connection("conn-a", Some("group-a"), 1)],
                custom_icons: vec![],
            };
            json.insert(
                tables::LEGACY_JSON_SETTINGS,
                serde_json::json!({"general": {}}).to_string().as_str(),
            )
            .expect("write settings");
            json.insert(
                tables::LEGACY_JSON_SESSIONS,
                serde_json::to_string(&sessions)
                    .expect("serialize")
                    .as_str(),
            )
            .expect("write sessions");
        }
        txn.commit().expect("commit legacy");
    }
    let storage = Storage::open(&dir).expect("migrate storage");
    assert_eq!(storage.get_schema_version().expect("schema version"), 3);
    assert_eq!(storage.list_groups().expect("groups").len(), 1);
    assert_eq!(storage.list_connections().expect("connections").len(), 1);
    let compat = storage.load_sessions().expect("load sessions");
    assert_eq!(compat.groups[0].id, "group-a");
    assert_eq!(compat.connections[0].id, "conn-a");
    assert!(
        compat.connections[0]
            .auth
            .as_ref()
            .and_then(|auth| auth.password.as_deref())
            .is_some()
    );
    let legacy_docs =
        migration::read_legacy_docs(&storage.db, tables::JSON_DOCS_TABLE).expect("legacy docs");
    assert!(legacy_docs.is_empty());
    let backup_count = fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("nyaterm.redb.bak-v1-")
        })
        .count();
    assert_eq!(backup_count, 1);
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn replace_sessions_splits_entities() {
    let (dir, storage) = test_storage("sessions");
    let config = SessionsConfig {
        groups: vec![sample_group("group-a", 1)],
        connections: vec![sample_connection("conn-a", Some("group-a"), 1)],
        custom_icons: vec![sample_custom_icon(
            "custom-icon-a",
            "data:image/png;base64,AAAA",
        )],
    };
    storage.replace_sessions(&config).expect("save sessions");
    assert!(
        migration::read_legacy_docs(&storage.db, tables::JSON_DOCS_TABLE)
            .expect("legacy docs")
            .is_empty()
    );
    assert_eq!(storage.list_groups().expect("groups").len(), 1);
    assert_eq!(storage.list_connections().expect("connections").len(), 1);
    assert_eq!(
        storage
            .list_connection_custom_icons()
            .expect("custom icons")
            .len(),
        1
    );
    let loaded = storage.load_sessions().expect("load sessions");
    assert_eq!(loaded.custom_icons[0].id, "custom-icon-a");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn load_sessions_migrates_connection_data_url_icons_into_custom_icon_library() {
    let (dir, storage) = test_storage("custom-icon-migration");
    let data_url = "data:image/png;base64,AAAA";
    let mut connection = sample_connection("conn-a", None, 1);
    connection.icon = Some(data_url.to_string());
    storage
        .save_connection(&connection)
        .expect("save connection with data url icon");

    let loaded = storage.load_sessions().expect("load sessions");

    assert_eq!(loaded.custom_icons.len(), 1);
    assert_eq!(loaded.custom_icons[0].data_url, data_url);
    assert_eq!(
        storage
            .list_connection_custom_icons()
            .expect("stored custom icons")
            .len(),
        1
    );
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn known_hosts_repository_preserves_structured_marker_hashed_and_raw_lines() {
    let (dir, storage) = test_storage("known-hosts");
    storage
        .replace_known_hosts_export(
            "# comment\n@cert-authority *.example.com ssh-ed25519 AAAA ca\n|1|nNMSH1CuL4w6FneDFn3ONf5paeg=|q8MlMsHsBk6GOpNwYqhnCeXKlRk= ssh-rsa BBBB\n",
        )
        .expect("save known hosts");
    let rendered = storage
        .render_known_hosts_export()
        .expect("load known hosts");
    assert!(rendered.contains("# comment"));
    assert!(rendered.contains("@cert-authority *.example.com ssh-ed25519 AAAA ca"));
    assert!(
        rendered
            .contains("|1|nNMSH1CuL4w6FneDFn3ONf5paeg=|q8MlMsHsBk6GOpNwYqhnCeXKlRk= ssh-rsa BBBB")
    );
    let _ = fs::remove_dir_all(dir);
}
