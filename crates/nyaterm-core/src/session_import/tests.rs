use super::{ConnectionType, prepare_session_import, read_file_limited};
use super::{
    mobaxterm::parse_moba_entry,
    nyaterm_json::parse_nyaterm_json_content,
    windterm::{parse_windterm_content, parse_windterm_target},
    xshell::parse_xsh_content,
};

const SAMPLE_JSON: &str = r#"
{
  "version": 1,
  "passwords": [
    { "ref": "prod-root-password", "name": "Prod root password", "password": "replace-me" }
  ],
  "ssh_keys": [
    {
      "ref": "ops-ed25519",
      "name": "Ops ED25519",
      "private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----",
      "passphrase": "optional-passphrase"
    }
  ],
  "groups": [
    { "path": ["Production"] },
    { "path": ["Production", "Web"] },
    { "path": ["Lab"] }
  ],
  "sessions": [
    {
      "name": "Prod web direct password",
      "type": "ssh",
      "group_path": ["Production", "Web"],
      "host": "web-01.example.com",
      "port": 22,
      "username": "deploy",
      "auth": { "mode": "password", "password": "replace-me" }
    },
    {
      "name": "Prod db saved password",
      "type": "ssh",
      "group_path": ["Production", "Database"],
      "host": "db-01.example.com",
      "username": "root",
      "auth": { "mode": "password", "password_ref": "prod-root-password" }
    },
    {
      "name": "Bastion saved key",
      "type": "ssh",
      "group_path": ["Production"],
      "host": "bastion.example.com",
      "username": "ops",
      "auth": { "mode": "key", "key_ref": "ops-ed25519" }
    },
    {
      "name": "Lab router",
      "type": "telnet",
      "group_path": ["Lab"],
      "host": "192.168.10.1",
      "port": 23,
      "backspace_mode": "del"
    },
    {
      "name": "USB console",
      "type": "serial",
      "group_path": ["Lab"],
      "port_name": "COM3",
      "baud_rate": 115200,
      "data_bits": 8,
      "parity": "none",
      "stop_bits": "1",
      "backspace_mode": "ctrl_h"
    },
    {
      "name": "Local PowerShell",
      "type": "local_terminal",
      "shell_path": "pwsh.exe",
      "shell_args": "-NoLogo",
      "working_dir": "C:\\Users\\me"
    }
  ]
}
"#;

#[test]
fn xshell_session_parser_preserves_group_and_key_auth() {
    let session = parse_xsh_content(
        r#"
[CONNECTION]
Protocol=SSH
Host=web.example.com
Port=2222

[CONNECTION:AUTHENTICATION]
UserName=deploy
UserKey=C:\keys\deploy.key
"#,
        "Xshell/Sessions/Production/Web/prod.xsh",
    )
    .expect("parse Xshell session");

    assert_eq!(session.name, "prod");
    assert_eq!(session.host, "web.example.com");
    assert_eq!(session.port, 2222);
    assert_eq!(session.username, "deploy");
    assert_eq!(session.auth_type, "key");
    assert_eq!(
        session.group_path,
        Some(vec!["Production".to_string(), "Web".to_string()])
    );
}

#[test]
fn mobaxterm_session_parser_reads_ssh_bookmark_fields() {
    let group = Some(vec!["Production".to_string()]);
    let session = parse_moba_entry("Prod web", "#109#0%web.example.com%2200%deploy%", &group)
        .expect("parse MobaXterm session");

    assert_eq!(session.name, "Prod web");
    assert_eq!(session.host, "web.example.com");
    assert_eq!(session.port, 2200);
    assert_eq!(session.username, "deploy");
    assert_eq!(session.auth_type, "password");
    assert_eq!(session.group_path, group);
}

#[test]
fn windterm_import_splits_user_at_host_targets() {
    let sessions = parse_windterm_content(
        r#"
[
  {
    "session.protocol": "SSH",
    "session.target": "deploy@192.168.1.10",
    "session.label": "Prod web",
    "session.port": 2222
  }
]
"#,
    )
    .expect("parse windterm sessions");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "Prod web");
    assert_eq!(sessions[0].host, "192.168.1.10");
    assert_eq!(sessions[0].username, "deploy");
    assert_eq!(sessions[0].port, 2222);
}

#[test]
fn windterm_import_defaults_username_when_target_has_no_user() {
    let sessions = parse_windterm_content(
        r#"
[
  {
    "session.protocol": "SSH",
    "session.target": "192.168.1.10"
  }
]
"#,
    )
    .expect("parse windterm sessions");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].host, "192.168.1.10");
    assert_eq!(sessions[0].username, "root");
}

#[test]
fn windterm_target_rejects_empty_user_or_host_splits() {
    assert_eq!(
        parse_windterm_target("@192.168.1.10"),
        ("@192.168.1.10".to_string(), "root".to_string())
    );
    assert_eq!(
        parse_windterm_target("deploy@"),
        ("deploy@".to_string(), "root".to_string())
    );
}

#[test]
fn windterm_target_splits_on_last_at_symbol() {
    assert_eq!(
        parse_windterm_target("ops@team@example.com"),
        ("example.com".to_string(), "ops@team".to_string())
    );
}

#[test]
fn limited_reader_rejects_oversized_files() {
    let dir = std::env::temp_dir().join(format!(
        "nyaterm-session-import-limit-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create limit directory");
    let path = dir.join("large.json");
    std::fs::write(&path, b"12345").expect("write oversized file");

    let error = read_file_limited(path.to_str().expect("utf8 path"), "test import file", 4)
        .expect_err("oversized file should fail");

    assert!(error.to_string().contains("exceeds"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn nyaterm_json_sample_import_prepares_supported_shapes() {
    let prepared = parse_nyaterm_json_content(SAMPLE_JSON).expect("parse sample");

    assert_eq!(prepared.groups.len(), 3);
    assert_eq!(prepared.passwords.len(), 1);
    assert_eq!(prepared.ssh_keys.len(), 1);
    assert_eq!(prepared.connections.len(), 6);
    assert_eq!(
        prepared.passwords[0].password.as_deref(),
        Some("replace-me")
    );
    assert_eq!(
        prepared.ssh_keys[0].key.as_deref(),
        Some("-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----")
    );

    let direct_auth = prepared.connections[0].auth.as_ref().expect("direct auth");
    assert_eq!(direct_auth.mode, "password");
    assert!(direct_auth.password_id.is_none());
    assert_eq!(direct_auth.password.as_deref(), Some("replace-me"));

    let saved_password_auth = prepared.connections[1]
        .auth
        .as_ref()
        .expect("saved password auth");
    assert_eq!(saved_password_auth.mode, "password");
    assert!(saved_password_auth.password_id.is_some());
    assert!(saved_password_auth.password.is_none());

    let key_auth = prepared.connections[2].auth.as_ref().expect("key auth");
    assert_eq!(key_auth.mode, "key");
    assert!(key_auth.key_id.is_some());

    let local_config = &prepared.connections[5].config;
    assert!(matches!(
        local_config,
        ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            ..
        } if shell_path == "pwsh.exe" && shell_args == "-NoLogo"
    ));
}

#[test]
fn nyaterm_json_rejects_duplicate_password_refs() {
    let json = r#"
{
  "version": 1,
  "passwords": [
    { "ref": "dup", "name": "One", "password": "a" },
    { "ref": "dup", "name": "Two", "password": "b" }
  ],
  "sessions": []
}
"#;

    let error = parse_nyaterm_json_content(json).unwrap_err();
    assert!(error.to_string().contains("Duplicate password ref"));
}

#[test]
fn nyaterm_json_rejects_missing_password_refs() {
    let json = r#"
{
  "version": 1,
  "sessions": [
    {
      "name": "Missing password",
      "type": "ssh",
      "host": "example.com",
      "username": "root",
      "auth": { "mode": "password", "password_ref": "missing" }
    }
  ]
}
"#;

    let error = parse_nyaterm_json_content(json).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("password_ref 'missing' was not found")
    );
}

#[test]
fn nyaterm_json_rejects_invalid_ports() {
    let json = r#"
{
  "version": 1,
  "sessions": [
    {
      "name": "Bad port",
      "type": "ssh",
      "host": "example.com",
      "port": 0,
      "username": "root",
      "auth": { "mode": "none" }
    }
  ]
}
"#;

    let error = parse_nyaterm_json_content(json).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("port must be between 1 and 65535")
    );
}

#[test]
fn securecrt_import_prepares_xml_sessions_with_groups() {
    let dir = temp_import_dir("securecrt");
    let import_path = dir.join("sessions.xml");
    std::fs::write(
        &import_path,
        r#"
<?xml version="1.0" encoding="UTF-8"?>
<key name="Sessions">
  <key name="Production">
    <key name="Web">
      <key name="Prod web">
        <string name="Hostname">web.example.com</string>
        <dword name="Port">2200</dword>
        <string name="Username">deploy</string>
        <string name="Protocol Name">SSH2</string>
      </key>
    </key>
  </key>
</key>
"#,
    )
    .expect("write SecureCRT XML");
    let prepared = prepare_session_import(&import_path).expect("prepare SecureCRT");

    assert_eq!(prepared.connections.len(), 1);
    let connection = prepared
        .connections
        .iter()
        .find(|connection| connection.name == "Prod web")
        .expect("SecureCRT connection");
    assert!(matches!(
        &connection.config,
        ConnectionType::Ssh { host, port, username, .. }
            if host == "web.example.com" && *port == 2200 && username == "deploy"
    ));
    assert_eq!(
        connection.group_path.as_deref(),
        Some(["Production".to_string(), "Web".to_string()].as_slice())
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn finalshell_import_accepts_conn_directory() {
    let dir = temp_import_dir("finalshell");
    let conn_dir = dir.join("conn");
    let nested = conn_dir.join("prod");
    std::fs::create_dir_all(&nested).expect("create FinalShell conn dir");
    std::fs::write(
        nested.join("folder.json"),
        r#"{"id":"folder-prod","name":"Production","parent_id":"root","delete_time":0}"#,
    )
    .expect("write FinalShell folder");
    std::fs::write(
            nested.join("prod_connect_config.json"),
            r#"{"name":"Prod shell","host":"prod.example.com","port":2222,"user_name":"ops","parent_id":"folder-prod","conection_type":100,"description":"primary","delete_time":0}"#,
        )
        .expect("write FinalShell connection");
    let prepared = prepare_session_import(&conn_dir).expect("prepare FinalShell");

    assert_eq!(prepared.connections.len(), 1);
    let connection = prepared
        .connections
        .iter()
        .find(|connection| connection.name == "Prod shell")
        .expect("FinalShell connection");
    assert_eq!(connection.description.as_deref(), Some("primary"));
    assert_eq!(
        connection.group_path.as_deref(),
        Some(["Production".to_string()].as_slice())
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn electerm_json_imports_bookmarks_with_groups() {
    let dir = temp_import_dir("electerm");
    let import_path = dir.join("bookmarks.json");
    std::fs::write(
        &import_path,
        r#"
{
  "bookmarkGroups": [
    { "id": "root", "title": "Production", "bookmarkIds": ["web"], "bookmarkGroupIds": [] }
  ],
  "bookmarks": [
    {
      "id": "web",
      "title": "Web",
      "host": "web.example.com",
      "username": "deploy",
      "authType": "password",
      "port": 2200,
      "type": "ssh"
    }
  ]
}
"#,
    )
    .expect("write Electerm bookmarks");
    let prepared = prepare_session_import(&import_path).expect("prepare Electerm");

    assert_eq!(prepared.connections.len(), 1);
    let connection = prepared
        .connections
        .iter()
        .find(|connection| connection.name == "Web")
        .expect("Electerm connection");
    assert!(matches!(
        &connection.config,
        ConnectionType::Ssh { host, port, username, .. }
            if host == "web.example.com" && *port == 2200 && username == "deploy"
    ));
    assert_eq!(
        connection.auth.as_ref().map(|auth| auth.mode.as_str()),
        Some("password")
    );
    assert_eq!(
        connection.group_path.as_deref(),
        Some(["Production".to_string()].as_slice())
    );
    std::fs::remove_dir_all(dir).ok();
}

fn temp_import_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nyaterm-session-import-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create import directory");
    dir
}

#[test]
fn nyaterm_json_retains_pipeline_shared_icons_and_future_connection_fields() {
    let prepared = parse_nyaterm_json_content(r#"{"version":1,"custom_icons":[{"id":"custom-icon-test","name":"test","data_url":"data:image/png;base64,AA==","created_at_ms":1,"updated_at_ms":1}],"sessions":[{"name":"test","type":"ssh","host":"example.com","sftp":{"pipeline_depth":16},"icon":"custom-icon-test","future":{"enabled":true}}]}"#).unwrap();
    assert_eq!(prepared.custom_icons.len(), 1);
    let saved = prepared.connections[0].saved.as_ref().unwrap();
    assert_eq!(saved.sftp.pipeline_depth, Some(16));
    assert_eq!(
        serde_json::to_value(saved).unwrap()["future"]["enabled"],
        true
    );
}
