use std::fs;
use std::path::{Path, PathBuf};

use crate::ConnectionType;
use crate::session_import::{
    SessionImportError, SessionImportOptions, SessionImportWarningKind, prepare_session_import,
    prepare_session_import_with_options, prepare_windterm_preview,
};
use crate::test_support::TestTempDir;

use super::{
    WindtermAccountAction, WindtermAccountDecision, WindtermOneKeyConfigState,
    WindtermUnlinkedReason, parse_windterm_content, prepare_windterm_import_with_decisions,
    resolve_key_path,
};

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
                ..SessionImportOptions::default()
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
            ..SessionImportOptions::default()
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
            ..SessionImportOptions::default()
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
            ..SessionImportOptions::default()
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
fn one_keys_config_lookup_is_case_insensitive() {
    let dir = TestTempDir::new("nyaterm-windterm-onekeys-case");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/OneKeys.config"),
        serde_json::json!([{
            "name": "case.example",
            "type": "SshOneKey",
            "uuid": "one-key-case",
            "auth": EMPTY_MASTER,
        }])
        .to_string(),
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([{
            "session.protocol": "SSH", "session.target": "10.11.0.20",
            "session.oneKey": "one-key-case"
        }])
        .to_string(),
    )
    .unwrap();

    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.one_key_state, WindtermOneKeyConfigState::Readable);
    assert_eq!(preview.accounts.len(), 1);
    assert!(preview.missing_one_keys.is_empty());
}

#[test]
fn one_key_config_builds_accounts_and_relinks_sessions_by_uuid() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        serde_json::json!([{
            "name": "172.20.0.21",
            "type": "SshOneKey",
            "uuid": "one-key-1",
            "auth": EMPTY_MASTER,
            "expect.password": r#"{"text.matchRegex":true,"text.pattern":"password:"}"#,
            "expect.username": r#"{"text.matchRegex":true,"text.pattern":"login:"}"#,
        }])
        .to_string(),
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([
            {"session.protocol":"SSH","session.target":"10.11.0.15","session.oneKey":"one-key-1"},
            {"session.protocol":"SSH","session.target":"root@10.11.0.16","session.oneKey":"one-key-1"},
            {"session.protocol":"SSH","session.target":"inline.example",
             "session.autoLogin": r#"{"PasswordEnabled":true,"Password":"inline"}"#},
        ])
        .to_string(),
    )
    .unwrap();

    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.one_key_state, WindtermOneKeyConfigState::Readable);
    assert_eq!(preview.accounts.len(), 1);
    assert!(preview.missing_one_keys.is_empty());
    let account = &preview.accounts[0];
    assert_eq!(account.name, "172.20.0.21");
    assert_eq!(account.username, "deploy");
    assert!(account.password_available);
    assert!(account.password_readable);
    assert_eq!(account.session_count, 2);

    assert_eq!(preview.prepared.passwords.len(), 1);
    assert_eq!(preview.prepared.passwords[0].id, account.id);
    assert_eq!(
        preview.prepared.passwords[0].password.as_deref(),
        Some(" secret ")
    );

    for connection in &preview.prepared.connections[..2] {
        let auth = connection.auth.as_ref().unwrap();
        assert_eq!(auth.account_id.as_deref(), Some(account.id.as_str()));
        assert_eq!(auth.mode, "password");
        assert!(auth.password.is_none());
        assert!(
            matches!(&connection.config, ConnectionType::Ssh { username, .. } if username == "deploy")
        );
    }
    assert_eq!(
        preview.prepared.connections[2]
            .auth
            .as_ref()
            .unwrap()
            .password
            .as_deref(),
        Some("inline")
    );
    assert!(!format!("{:?}", preview.prepared).contains("secret"));
}

#[test]
fn unreadable_one_key_auth_still_creates_account_for_later_password_edit() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-unreadable");
    fs::create_dir_all(dir.join("terminal")).unwrap();
    let path = dir.join("terminal/user.sessions");
    fs::write(
        &path,
        r#"[{"session.protocol":"SSH","session.target":"10.11.0.15","session.oneKey":"broken"}]"#,
    )
    .unwrap();
    fs::write(
        dir.join("terminal/onekeys.config"),
        serde_json::json!([{
            "name": "jump-host", "type": "SshOneKey", "uuid": "broken", "auth": "not-decryptable",
        }])
        .to_string(),
    )
    .unwrap();

    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.accounts.len(), 1);
    assert_eq!(preview.unreadable_account_count, 1);
    assert!(!preview.accounts[0].password_available);
    assert!(!preview.accounts[0].password_readable);
    assert_eq!(preview.accounts[0].session_count, 1);
    assert_eq!(preview.prepared.passwords.len(), 1);
    assert!(preview.prepared.passwords[0].password.is_none());
    assert_eq!(
        preview.prepared.connections[0]
            .auth
            .as_ref()
            .unwrap()
            .account_id
            .as_deref(),
        Some(preview.accounts[0].id.as_str())
    );
}

#[test]
fn duplicate_one_key_names_are_suffixed_and_unmatched_references_are_counted() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-conflicts");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        serde_json::json!([
            {"name":"same", "type":"SshOneKey", "uuid":"a", "auth": EMPTY_MASTER},
            {"name":"same", "type":"SshOneKey", "uuid":"b", "auth": EMPTY_MASTER},
            {"name":"ignored", "type":"TelnetOneKey", "uuid":"c", "auth": EMPTY_MASTER},
        ])
        .to_string(),
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([
            {"session.protocol":"SSH","session.target":"one","session.oneKey":"a"},
            {"session.protocol":"SSH","session.target":"two","session.oneKey":"missing"},
        ])
        .to_string(),
    )
    .unwrap();

    let preview = prepare_windterm_preview(&path, None).unwrap();
    // Only the referenced, supported definition becomes an account candidate.
    assert_eq!(preview.accounts.len(), 1);
    assert_eq!(preview.duplicate_account_name_count, 1);
    assert_eq!(preview.accounts[0].name, "same");
    assert_eq!(preview.missing_one_keys.len(), 1);
    assert_eq!(preview.missing_one_keys[0].session_name, "two");
    assert_eq!(preview.missing_one_keys[0].one_key_uuid, "missing");
    assert_eq!(preview.unsupported_one_key_count, 1);
}

#[test]
fn missing_one_key_config_keeps_legacy_single_step_import() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-missing");
    let path = profile(&dir, false, EMPTY_MASTER);
    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.one_key_state, WindtermOneKeyConfigState::Absent);
    assert!(preview.accounts.is_empty());
    assert!(preview.prepared.passwords.is_empty());
    assert_eq!(preview.prepared.connections.len(), 1);
    assert_eq!(
        preview.prepared.connections[0]
            .auth
            .as_ref()
            .unwrap()
            .password
            .as_deref(),
        Some(" secret ")
    );
}

#[test]
fn corrupt_one_key_config_degrades_without_leaking_content() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-corrupt");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        "[{\"auth\":\"private-value\"",
    )
    .unwrap();
    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.one_key_state, WindtermOneKeyConfigState::Unreadable);
    assert!(preview.accounts.is_empty());
    assert_eq!(preview.prepared.connections.len(), 1);
    assert!(!format!("{preview:?}").contains("private-value"));
}

#[test]
fn missing_one_key_references_are_reported_for_none_partial_and_all_cases() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-report");
    let path = profile(&dir, false, EMPTY_MASTER);
    let one_key_path = dir.join("terminal/onekeys.config");
    let sessions = serde_json::json!([
        {"session.protocol":"SSH", "session.target":"one", "session.label":"one", "session.oneKey":"a"},
        {"session.protocol":"SSH", "session.target":"two", "session.label":"two", "session.oneKey":"b"},
        {"session.protocol":"SSH", "session.target":"three", "session.label":"plain"},
    ]);
    fs::write(&path, sessions.to_string()).unwrap();
    let cases = [
        (
            Some(serde_json::json!([
                {"name":"a", "type":"SshOneKey", "uuid":"a", "auth": EMPTY_MASTER},
                {"name":"b", "type":"SshOneKey", "uuid":"b", "auth": EMPTY_MASTER},
            ])),
            Vec::<&str>::new(),
        ),
        (
            Some(serde_json::json!([
                {"name":"a", "type":"SshOneKey", "uuid":"a", "auth": EMPTY_MASTER},
            ])),
            vec!["two"],
        ),
        (
            Some(serde_json::json!([
                {"name":"c", "type":"SshOneKey", "uuid":"c", "auth": EMPTY_MASTER},
            ])),
            vec!["one", "two"],
        ),
        (None, vec!["one", "two"]),
    ];
    for (config, expected_names) in cases {
        match config {
            Some(config) => fs::write(&one_key_path, config.to_string()).unwrap(),
            None => {
                if one_key_path.exists() {
                    fs::remove_file(&one_key_path).unwrap();
                }
            }
        }
        let preview = prepare_windterm_preview(&path, None).unwrap();
        let names = preview
            .missing_one_keys
            .iter()
            .map(|missing| missing.session_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, expected_names);
        let report = preview.report();
        assert_eq!(report.missing_one_keys.len(), expected_names.len());
        assert_eq!(report.imported_sessions, 3);
    }
}

#[test]
fn unsupported_one_key_types_are_counted_and_sessions_stay_unlinked() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-unsupported");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        serde_json::json!([{"name":"telnet", "type":"TelnetOneKey", "uuid":"c"}]).to_string(),
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([
            {"session.protocol":"SSH", "session.target":"telnet-host", "session.label":"telnet-host", "session.oneKey":"c"},
        ])
        .to_string(),
    )
    .unwrap();

    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.unsupported_one_key_count, 1);
    assert!(preview.accounts.is_empty());
    assert!(preview.missing_one_keys.is_empty());
    assert_eq!(preview.unlinked_sessions.len(), 1);
    assert_eq!(
        preview.unlinked_sessions[0].reason,
        WindtermUnlinkedReason::UnsupportedType
    );
    assert_eq!(
        preview.prepared.connections[0].auth.as_ref().unwrap().mode,
        "none"
    );
}

#[test]
fn invalid_and_duplicate_one_key_entries_are_counted_and_name_falls_back_to_uuid() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-invalid");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        serde_json::json!([
            {"name":"no-uuid", "type":"SshOneKey"},
            {"name":"first", "type":"SshOneKey", "uuid":"dup", "auth": EMPTY_MASTER},
            {"name":"second", "type":"SshOneKey", "uuid":"dup", "auth": EMPTY_MASTER},
            {"name":"", "type":"SshOneKey", "uuid":"nameless"},
        ])
        .to_string(),
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([
            {"session.protocol":"SSH", "session.target":"one", "session.oneKey":"dup"},
            {"session.protocol":"SSH", "session.target":"two", "session.oneKey":"nameless"},
        ])
        .to_string(),
    )
    .unwrap();

    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.invalid_one_key_count, 1);
    assert_eq!(preview.duplicate_one_key_uuid_count, 1);
    assert_eq!(preview.accounts.len(), 2);
    assert_eq!(preview.accounts[0].name, "first");
    assert_eq!(preview.accounts[1].name, "nameless");
}

#[test]
fn account_decisions_create_reuse_and_skip_rebuild_connections() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-decisions");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        serde_json::json!([{"name":"jump", "type":"SshOneKey", "uuid":"a", "auth": EMPTY_MASTER}])
            .to_string(),
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([
            {"session.protocol":"SSH", "session.target":"fallback@host", "session.label":"host-a",
             "session.oneKey":"a",
             "session.autoLogin": r#"{"PasswordEnabled":true,"Password":"fallback","session.user":"inline"}"#},
        ])
        .to_string(),
    )
    .unwrap();

    let create = prepare_windterm_import_with_decisions(
        &path,
        None,
        &[WindtermAccountDecision {
            one_key_uuid: "a".into(),
            action: WindtermAccountAction::Create {
                username: "custom".into(),
                password: Some("new-secret".into()),
            },
        }],
    )
    .unwrap();
    assert_eq!(create.prepared.passwords.len(), 1);
    assert_eq!(create.prepared.passwords[0].username, "custom");
    assert_eq!(
        create.prepared.passwords[0].password.as_deref(),
        Some("new-secret")
    );
    let connection = &create.prepared.connections[0];
    assert_eq!(
        connection.auth.as_ref().unwrap().account_id.as_deref(),
        Some(create.prepared.passwords[0].id.as_str())
    );
    assert!(
        matches!(&connection.config, ConnectionType::Ssh { username, .. } if username == "custom")
    );
    assert_eq!(create.report.imported_sessions, 1);
    assert!(create.report.missing_one_keys.is_empty());
    assert!(!format!("{create:?}").contains("new-secret"));

    let reuse = prepare_windterm_import_with_decisions(
        &path,
        None,
        &[WindtermAccountDecision {
            one_key_uuid: "a".into(),
            action: WindtermAccountAction::Reuse {
                account_id: "existing-id".into(),
                username: "existing-user".into(),
            },
        }],
    )
    .unwrap();
    assert!(reuse.prepared.passwords.is_empty());
    let connection = &reuse.prepared.connections[0];
    assert_eq!(
        connection.auth.as_ref().unwrap().account_id.as_deref(),
        Some("existing-id")
    );
    assert!(
        matches!(&connection.config, ConnectionType::Ssh { username, .. } if username == "existing-user")
    );

    let skip = prepare_windterm_import_with_decisions(
        &path,
        None,
        &[WindtermAccountDecision {
            one_key_uuid: "a".into(),
            action: WindtermAccountAction::Skip,
        }],
    )
    .unwrap();
    assert!(skip.prepared.passwords.is_empty());
    let connection = &skip.prepared.connections[0];
    assert_eq!(
        connection.auth.as_ref().unwrap().password.as_deref(),
        Some("fallback")
    );
    assert!(
        matches!(&connection.config, ConnectionType::Ssh { username, .. } if username == "inline")
    );
    assert_eq!(skip.report.unlinked_sessions.len(), 1);
    assert_eq!(
        skip.report.unlinked_sessions[0].reason,
        WindtermUnlinkedReason::SkippedByUser
    );
    assert!(skip.report.missing_one_keys.is_empty());
}

#[test]
fn report_keeps_skipped_session_and_warning_counts_consistent() {
    let content = serde_json::json!([
        null,
        {"session.protocol":"Telnet", "session.target":"telnet"},
        {"session.protocol":"SSH", "session.target":"bad-port", "session.port":0},
        {"session.protocol":"SSH", "session.target":"readable", "session.autoLogin": r#"{"PasswordEnabled":true,"Password":"p"}"#},
    ])
    .to_string();
    let parsed = parse_windterm_content(&content).unwrap();
    assert_eq!(parsed.connections.len(), 1);
    assert_eq!(parsed.incomplete_credentials_count(), 0);

    let dir = TestTempDir::new("nyaterm-windterm-report-counts");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(&path, content).unwrap();
    let preview = prepare_windterm_preview(&path, None).unwrap();
    let report = preview.report();
    assert_eq!(report.imported_sessions, 1);
    assert_eq!(report.skipped_sessions, 3);
    assert_eq!(report.credential_warning_sessions, 0);
    assert_eq!(report.one_key_state, WindtermOneKeyConfigState::Absent);
}

#[test]
fn all_unusable_sessions_return_a_clear_error_instead_of_silent_partial_import() {
    let content = serde_json::json!([
        null,
        {"session.protocol":"Telnet", "session.target":"telnet"},
    ])
    .to_string();
    assert!(parse_windterm_content(&content).is_err());
}

#[test]
fn unreadable_one_key_config_keeps_sessions_and_reports_missing_references() {
    let dir = TestTempDir::new("nyaterm-windterm-onekey-unreadable-config");
    let path = profile(&dir, false, EMPTY_MASTER);
    fs::write(
        dir.join("terminal/onekeys.config"),
        "[{\"auth\":\"private-value\"",
    )
    .unwrap();
    fs::write(
        &path,
        serde_json::json!([
            {"session.protocol":"SSH", "session.target":"host", "session.label":"host", "session.oneKey":"a"},
        ])
        .to_string(),
    )
    .unwrap();
    let preview = prepare_windterm_preview(&path, None).unwrap();
    assert_eq!(preview.one_key_state, WindtermOneKeyConfigState::Unreadable);
    assert_eq!(preview.prepared.connections.len(), 1);
    assert_eq!(preview.missing_one_keys.len(), 1);
    assert_eq!(preview.missing_one_keys[0].session_name, "host");
    assert!(!format!("{preview:?}").contains("private-value"));
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

#[test]
fn one_key_auth_preserves_alias_priority_tolerance_and_zeroizing_nested_strings() {
    let crypto = super::ProfileCrypto::Unavailable(SessionImportWarningKind::MissingProfileConfig);
    for (raw, user, password) in [
        (
            r#"{"username":" first ","user":"second","password":" primary ","Password":"secondary"}"#,
            Some("first"),
            Some(" primary "),
        ),
        (
            r#"{"username":42,"user":"fallback","password":false,"pass":"fallback-secret"}"#,
            Some("fallback"),
            Some("fallback-secret"),
        ),
        (
            r#"{"username":" ","user":"blocked","password":"","secret":"blocked"}"#,
            None,
            None,
        ),
        (
            r#"{"session.user":"login","secret":"\u0061\"escaped"}"#,
            Some("login"),
            Some("a\"escaped"),
        ),
        (
            r#"{"username":"old","username":"last","password":"old","password":12,"pass":"last"}"#,
            Some("last"),
            Some("last"),
        ),
    ] {
        for payload in [raw.to_owned(), serde_json::to_string(raw).unwrap()] {
            let (actual_user, actual_password, readable) =
                super::parse_one_key_auth(Some(&payload), &crypto);
            assert!(readable);
            assert_eq!(actual_user.as_deref(), user);
            assert_eq!(actual_password.as_deref(), password);
            if let Some(secret) = actual_password {
                assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
            }
        }
    }
    for invalid in ["not ciphertext", "{broken", "AAAA"] {
        let (user, password, readable) = super::parse_one_key_auth(Some(invalid), &crypto);
        assert!(!readable);
        assert!(user.is_none());
        assert!(password.is_none());
    }
}

#[test]
fn encrypted_one_key_auth_handles_nested_json_wrong_password_and_corruption() {
    use base64::Engine;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    let crypto = super::derive_crypto("fingerprint-1", "master");
    let payload =
        serde_json::to_string(r#"{"userName":"deploy","Password":"sensitive-value"}"#).unwrap();
    let mut buffer = zeroize::Zeroizing::new(vec![0u8; payload.len() + 16]);
    buffer[..payload.len()].copy_from_slice(payload.as_bytes());
    let ciphertext =
        cbc::Encryptor::<aes::Aes256>::new_from_slices(&crypto.0[..32], &crypto.0[32..])
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, payload.len())
            .unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(ciphertext);
    let ready = super::ProfileCrypto::Ready(crypto);
    let (user, password, readable) = super::parse_one_key_auth(Some(&encoded), &ready);
    assert!(readable);
    assert_eq!(user.as_deref(), Some("deploy"));
    assert_eq!(password.as_deref(), Some("sensitive-value"));
    let wrong = super::ProfileCrypto::Ready(super::derive_crypto("fingerprint-1", "wrong"));
    assert!(!super::parse_one_key_auth(Some(&encoded), &wrong).2);
    assert!(!super::parse_one_key_auth(Some(&encoded[..encoded.len() - 4]), &ready).2);
}
