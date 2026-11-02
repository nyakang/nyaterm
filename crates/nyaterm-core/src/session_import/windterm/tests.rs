use std::fs;
use std::path::{Path, PathBuf};

use crate::ConnectionType;
use crate::session_import::{
    SessionImportError, SessionImportOptions, SessionImportWarningKind, prepare_session_import,
    prepare_session_import_with_options,
};
use crate::test_support::TestTempDir;

use super::{parse_windterm_content, resolve_key_path};

// Independent vectors produced with Python hashlib.pbkdf2_hmac('sha3_512') and
// cryptography's AES-CBC + PKCS7, not this importer's encryption implementation.
const EMPTY_MASTER: &str = "Ee0O/U90mT5Qe9H8k5AHgZsxQ64E0f2GlGxMfQKGGanWtqa5i3TMxOMs5R7Tv/hAxURUCG/Y/dPjd+HRi+/g5nJ4ZjdNEDt5u7xzsCp99LE=";
const WITH_MASTER: &str = "xCSsthKpnyjEgvx1zOw2rU6noHJj5/EtRQj0+Z4incUd4D9Szbg+1/07c8Sf+IuiVDh7+GQ80spldkp3BHSui4RJyFZkZOaJYKg4gKwGqP0=";

fn profile(root: &TestTempDir, master: bool, auto_login: &str) -> PathBuf {
    fs::create_dir_all(root.join("terminal")).unwrap();
    fs::write(
        root.join("user.config"),
        serde_json::json!({
            "application.fingerprint": "fingerprint-1", "application.masterPassword": master,
        })
        .to_string(),
    )
    .unwrap();
    let path = root.join("terminal/user.sessions");
    fs::write(
        &path,
        serde_json::json!([{
            "session.protocol": "SSH", "session.target": "fallback@example.com",
            "session.autoLogin": auto_login,
        }])
        .to_string(),
    )
    .unwrap();
    path
}

#[test]
fn encrypted_auto_login_matches_independent_vectors_and_preserves_password_whitespace() {
    for (master, encrypted) in [(None, EMPTY_MASTER), (Some(" master-secret "), WITH_MASTER)] {
        let dir = TestTempDir::new("nyaterm-windterm-crypto");
        let path = profile(&dir, master.is_some(), encrypted);
        let prepared = prepare_session_import_with_options(
            &path,
            SessionImportOptions {
                windterm_master_password: master,
            },
        )
        .unwrap();
        assert!(prepared.warnings.is_empty());
        let connection = &prepared.connections[0];
        assert!(
            matches!(&connection.config, ConnectionType::Ssh { username, .. } if username == "deploy")
        );
        assert_eq!(
            connection.auth.as_ref().unwrap().password.as_deref(),
            Some(" secret ")
        );
        assert!(!format!("{prepared:?}").contains("secret"));
    }
}

#[test]
fn enabled_master_password_requires_a_typed_challenge_and_can_be_resubmitted() {
    let dir = TestTempDir::new("nyaterm-windterm-master");
    let path = profile(&dir, true, WITH_MASTER);
    assert!(matches!(
        prepare_session_import(&path),
        Err(SessionImportError::WindTermMasterPasswordRequired)
    ));
    let prepared = prepare_session_import_with_options(
        &path,
        SessionImportOptions {
            windterm_master_password: Some(" master-secret "),
        },
    )
    .unwrap();
    assert_eq!(prepared.connections.len(), 1);
    assert!(prepared.warnings.is_empty());
}

#[test]
fn disabled_master_password_always_uses_empty_password_derivation() {
    let dir = TestTempDir::new("nyaterm-windterm-disabled-master");
    let path = profile(&dir, false, EMPTY_MASTER);
    let prepared = prepare_session_import_with_options(
        &path,
        SessionImportOptions {
            windterm_master_password: Some("unused"),
        },
    )
    .unwrap();
    assert!(prepared.warnings.is_empty());
    assert_eq!(
        prepared.connections[0]
            .auth
            .as_ref()
            .unwrap()
            .password
            .as_deref(),
        Some(" secret ")
    );
}

#[test]
fn wrong_master_password_keeps_connection_with_redacted_warning() {
    let dir = TestTempDir::new("nyaterm-windterm-wrong-master");
    let path = profile(&dir, true, WITH_MASTER);
    let prepared = prepare_session_import_with_options(
        &path,
        SessionImportOptions {
            windterm_master_password: Some("wrong"),
        },
    )
    .unwrap();
    assert_eq!(prepared.connections.len(), 1);
    assert_eq!(prepared.incomplete_credentials_count(), 1);
    assert_eq!(
        prepared.warnings[0].kind,
        SessionImportWarningKind::UnreadableAutoLogin
    );
    assert_eq!(prepared.connections[0].auth.as_ref().unwrap().mode, "none");
}

#[test]
fn missing_or_corrupt_profile_config_keeps_connections() {
    let dir = TestTempDir::new("nyaterm-windterm-config");
    let path = profile(&dir, false, EMPTY_MASTER);
    for config in [None, Some("invalid"), Some("{}")] {
        if let Some(config) = config {
            fs::write(dir.join("user.config"), config).unwrap();
        } else {
            fs::remove_file(dir.join("user.config")).unwrap();
        }
        let prepared = prepare_session_import(&path).unwrap();
        assert_eq!(prepared.connections.len(), 1);
        assert_eq!(prepared.incomplete_credentials_count(), 1);
        assert_eq!(
            prepared.warnings[0].kind,
            if config.is_none() {
                SessionImportWarningKind::MissingProfileConfig
            } else {
                SessionImportWarningKind::UnreadableProfileConfig
            }
        );
    }
}

#[test]
fn plaintext_auto_login_preserves_metadata_and_password_takes_precedence_over_key() {
    let content = serde_json::json!([{
        "session.protocol": "ssh", "session.target": "fallback@example.com",
        "session.label": " Production ", "session.port": 2222,
        "session.group": " Prod > > Web ", "session.description": " Description ",
        "session.icon": "Ubuntu.svg", "ssh.x11": true,
        "ssh.identityFilePath.windows": "missing-key",
        "session.autoLogin": r#"{"PasswordEnabled":true,"Password":" value ","session.user":"deploy"}"#,
    }]).to_string();
    let prepared = parse_windterm_content(&content).unwrap();
    let connection = &prepared.connections[0];
    assert_eq!(connection.name, "Production");
    assert_eq!(connection.description.as_deref(), Some("Description"));
    assert_eq!(connection.icon.as_deref(), Some("Ubuntu.svg"));
    assert_eq!(connection.group_path.as_ref().unwrap(), &["Prod", "Web"]);
    assert!(
        matches!(&connection.config, ConnectionType::Ssh { port: 2222, username, x11_forwarding: true, .. } if username == "deploy")
    );
    assert_eq!(
        connection.auth.as_ref().unwrap().password.as_deref(),
        Some(" value ")
    );
    assert!(prepared.ssh_keys.is_empty());
    assert!(prepared.warnings.is_empty());
}

#[test]
fn unreadable_auto_login_falls_back_to_top_level_key_and_counts_each_session_once() {
    let dir = TestTempDir::new("nyaterm-windterm-mixed");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(dir.join("terminal/key"), "private-key-data").unwrap();
    let entries = serde_json::json!([
        {"session.protocol":"SSH", "session.target":"one", "session.autoLogin":"bad", "ssh.identityFilePath.windows":"key"},
        {"session.protocol":"SSH", "session.target":"two", "session.autoLogin":"bad", "ssh.identityFilePath.windows":"missing"},
        {"session.protocol":"SSH", "session.target":"three", "session.autoLogin": EMPTY_MASTER},
    ]);
    fs::write(&path, entries.to_string()).unwrap();
    let prepared = prepare_session_import(&path).unwrap();
    assert_eq!(prepared.connections.len(), 3);
    assert_eq!(prepared.warnings.len(), 3);
    assert_eq!(prepared.incomplete_credentials_count(), 2);
    assert_eq!(prepared.connections[0].auth.as_ref().unwrap().mode, "key");
    assert_eq!(prepared.connections[1].auth.as_ref().unwrap().mode, "none");
    assert_eq!(
        prepared.connections[2].auth.as_ref().unwrap().mode,
        "password"
    );
}

#[test]
fn key_import_falls_back_to_relative_path_and_reuses_keys_with_matching_passphrases() {
    let dir = TestTempDir::new("nyaterm-windterm-keys");
    let path = profile(&dir, false, "");
    fs::write(dir.join("terminal/key"), "private-key-data\n").unwrap();
    let auto = r#"{"PasswordEnabled":false,"Password":"ignored","Public Key":{"windows.path":"missing","windows.pass":" phrase "}}"#;
    fs::write(&path, serde_json::json!([
        {"session.protocol":"SSH","session.target":"one","session.autoLogin":auto,"ssh.identityFilePath.windows":"key"},
        {"session.protocol":"SSH","session.target":"two","session.autoLogin":auto,"ssh.identityFilePath.windows":"key"},
        {"session.protocol":"SSH","session.target":"three","ssh.identityFilePath.windows":"key"},
    ]).to_string()).unwrap();
    let prepared = prepare_session_import(&path).unwrap();
    assert!(prepared.warnings.is_empty());
    assert_eq!(prepared.ssh_keys.len(), 1);
    assert_eq!(
        prepared.ssh_keys[0].key.as_deref(),
        Some("private-key-data\n")
    );
    assert_eq!(prepared.ssh_keys[0].passphrase.as_deref(), Some(" phrase "));
    for connection in &prepared.connections {
        assert_eq!(
            connection.auth.as_ref().unwrap().key_id.as_ref(),
            Some(&prepared.ssh_keys[0].id)
        );
    }
}

#[test]
fn key_path_expands_home_placeholders_and_resolves_relative_to_sessions() {
    let home = dirs::home_dir().unwrap();
    for path in ["$(HomeDir)/key", "${HomeDir}/key", "~/key", "~\\key"] {
        assert_eq!(resolve_key_path(path, None), home.join("key"));
    }
    assert_eq!(resolve_key_path("~", None), home);
    assert_eq!(
        resolve_key_path("key", Some(Path::new("profile/terminal/user.sessions"))),
        PathBuf::from("profile/terminal/key")
    );
}

#[test]
fn oversized_profile_and_key_files_are_rejected_without_losing_connections() {
    let dir = TestTempDir::new("nyaterm-windterm-limits");
    let path = profile(&dir, false, EMPTY_MASTER);
    let file = fs::File::create(dir.join("user.config")).unwrap();
    file.set_len(super::super::SESSION_IMPORT_MAX_BYTES + 1)
        .unwrap();
    assert_eq!(
        prepare_session_import(&path).unwrap().warnings[0].kind,
        SessionImportWarningKind::UnreadableProfileConfig
    );
    let file = fs::File::create(dir.join("terminal/key")).unwrap();
    file.set_len(super::super::SESSION_IMPORT_MAX_BYTES + 1)
        .unwrap();
    fs::write(&path, r#"[{"session.protocol":"SSH","session.target":"host","ssh.identityFilePath.windows":"key"}]"#).unwrap();
    let prepared = prepare_session_import(&path).unwrap();
    assert_eq!(prepared.connections.len(), 1);
    assert_eq!(
        prepared.warnings[0].kind,
        SessionImportWarningKind::UnreadablePrivateKey
    );
}

#[test]
fn invalid_ports_and_non_ssh_sessions_are_skipped_without_truncation() {
    let mut entries =
        vec![serde_json::json!({"session.protocol":"Telnet", "session.target":"skip"})];
    for port in [
        serde_json::json!(0),
        serde_json::json!(65536),
        serde_json::json!(-1),
        serde_json::json!("22"),
    ] {
        entries.push(serde_json::json!({"session.protocol":"SSH", "session.target":"skip", "session.port":port}));
    }
    entries.push(serde_json::json!({"session.protocol":"SSH", "session.target":"keep"}));
    assert_eq!(
        parse_windterm_content(&serde_json::to_string(&entries).unwrap())
            .unwrap()
            .connections
            .len(),
        1
    );
}

#[test]
fn corrupt_plaintext_and_ciphertext_do_not_leak_values_in_errors() {
    for auto in [
        "{\"PasswordEnabled\":\"private-value\"}",
        "not-base64",
        "AA==",
    ] {
        let dir = TestTempDir::new("nyaterm-windterm-corrupt");
        let path = profile(&dir, false, auto);
        let prepared = prepare_session_import(&path).unwrap();
        assert_eq!(prepared.incomplete_credentials_count(), 1);
        assert!(!format!("{:?}", prepared.warnings).contains("private-value"));
    }
}

#[test]
fn unsupported_optional_fields_do_not_abort_other_sessions_or_lose_readable_passwords() {
    let prepared = parse_windterm_content(&serde_json::json!([
        null, 123,
        {"session.protocol":"Serial", "session.autoLogin": {"unsupported":"value"}},
        {"session.protocol":"SSH", "session.target":"broken.example", "session.autoLogin": []},
        {"session.protocol":"SSH", "session.target":"readable.example", "session.label": false, "ssh.x11": null,
         "session.autoLogin": r#"{"PasswordEnabled":true,"Password":"readable","Public Key":false}"#},
    ]).to_string()).unwrap();
    assert_eq!(prepared.connections.len(), 2);
    assert_eq!(prepared.incomplete_credentials_count(), 1);
    assert_eq!(
        prepared.warnings[0].kind,
        SessionImportWarningKind::UnreadableAutoLogin
    );
    assert_eq!(prepared.connections[1].name, "readable.example");
    assert_eq!(
        prepared.connections[1]
            .auth
            .as_ref()
            .unwrap()
            .password
            .as_deref(),
        Some("readable")
    );
}
