use crate::ConnectionType;
use crate::session_import::mobaxterm::{
    MobaXtermCredentialDiagnosticKind, parse_stored_passwords, prepare_content,
};
use crate::session_import::{SessionImportError, SessionImportOptions, prepare_session_import};
use crate::test_support::TestTempDir;

const SECRET: &str = "Lw+8ew2leHRCfHlGFP0SMA==";

fn options() -> SessionImportOptions<'static> {
    SessionImportOptions {
        mobaxterm_master_password: Some("synthetic-master"),
        ..SessionImportOptions::default()
    }
}

#[test]
fn named_credentials_resolve_username_share_accounts_and_do_not_match_plain_usernames() {
    let source = format!(
        "[Sesspass]\n[Credentials]\nprod=deploy:{SECRET}\n[Bookmarks]\nA=#109#0%a.example%22%[prod]%\nB=#91#0%b.example%3389%[prod]%\nC=#109#0%c.example%22%deploy%\n"
    );
    let outcome = prepare_content(&source, &[], &options()).unwrap();
    assert_eq!(outcome.prepared.passwords.len(), 1);
    assert_eq!(outcome.report.linked_credentials, 1);
    assert_eq!(outcome.prepared.passwords[0].username, "deploy");
    let id = &outcome.prepared.passwords[0].id;
    for connection in &outcome.prepared.connections[..2] {
        assert_eq!(
            connection.auth.as_ref().unwrap().account_id.as_ref(),
            Some(id)
        );
        assert_eq!(
            connection.auth.as_ref().unwrap().password_id.as_ref(),
            Some(id)
        );
    }
    assert!(
        matches!(&outcome.prepared.connections[0].config, ConnectionType::Ssh { username, .. } if username == "deploy")
    );
    assert!(
        outcome.prepared.connections[2]
            .auth
            .as_ref()
            .unwrap()
            .account_id
            .is_none()
    );
}

#[test]
fn endpoint_passwords_isolate_protocol_port_username_and_ambiguous_portless_records() {
    let source = format!(
        "[Passwords]\nssh2222:deploy@server.example={SECRET}\ndeploy@bare.example={SECRET}\n[Bookmarks]\nA=#109#0%SERVER.example%2222%deploy%\nB=#109#0%server.example%22%deploy%\nC=#109#0%server.example%2222%other%\nD=#98#0%server.example%2222%deploy%\nE=#109#0%bare.example%22%deploy%\nF=#109#0%bare.example%2222%deploy%\n"
    );
    let outcome = prepare_content(&source, &[], &options()).unwrap();
    assert_eq!(outcome.prepared.passwords.len(), 1);
    assert!(
        outcome.prepared.connections[0]
            .auth
            .as_ref()
            .unwrap()
            .account_id
            .is_some()
    );
    assert!(
        outcome.prepared.connections[1..]
            .iter()
            .all(|connection| connection.auth.as_ref().unwrap().account_id.is_none())
    );
    assert!(outcome.report.diagnostics.iter().any(|item| item.kind
        == MobaXtermCredentialDiagnosticKind::AmbiguousMatch
        && item.count == 2));
}

#[test]
fn plaintext_file_wins_and_skip_encrypted_retains_plaintext_accounts() {
    let source = format!(
        "[Sesspass]\n[Credentials]\nprod=deploy:{SECRET}\n[Bookmarks]\nA=#109#0%a.example%22%[prod]%\n"
    );
    let plaintext = parse_stored_passwords("prod(manual-user) = synthetic-manual");
    for options in [
        options(),
        SessionImportOptions {
            mobaxterm_skip_encrypted_credentials: true,
            ..SessionImportOptions::default()
        },
    ] {
        let outcome = prepare_content(&source, &plaintext, &options).unwrap();
        assert_eq!(outcome.prepared.passwords.len(), 1);
        assert_eq!(
            outcome.prepared.passwords[0].password.as_deref(),
            Some("synthetic-manual")
        );
        assert_eq!(outcome.prepared.passwords[0].username, "manual-user");
    }
}

#[test]
fn skipped_named_password_retains_real_username_without_claiming_missing_reference() {
    let source = format!(
        "[Sesspass]\n[Credentials]\nprod=deploy:{SECRET}\n[Bookmarks]\nA=#109#0%a.example%22%[prod]%\nB=#109#0%b.example%22%[missing]%\n"
    );
    let outcome = prepare_content(
        &source,
        &[],
        &SessionImportOptions {
            mobaxterm_skip_encrypted_credentials: true,
            ..SessionImportOptions::default()
        },
    )
    .unwrap();
    assert!(outcome.prepared.passwords.is_empty());
    assert!(
        matches!(&outcome.prepared.connections[0].config, ConnectionType::Ssh { username, .. } if username == "deploy")
    );
    let auth = outcome.prepared.connections[0].auth.as_ref().unwrap();
    assert!(auth.account_id.is_none());
    assert!(auth.password_id.is_none());
    assert_eq!(outcome.prepared.incomplete_credentials_count(), 2);
    assert!(outcome.report.diagnostics.iter().any(|item| item.kind
        == MobaXtermCredentialDiagnosticKind::MissingReference
        && item.count == 1));
}

#[test]
fn failed_weak_password_retains_named_identity_and_duplicate_definitions_remain_ambiguous() {
    let source = "[Misc]\nSessionP=synthetic-session-p\n[Credentials]\nprod=deploy:TtIHsyiSS+EECsEITeEOeyIOCieEtstC\nprod=other:!!!\nbad=ops:!!!\n[Bookmarks]\nA=#109#0%a.example%22%[prod]%\nB=#109#0%b.example%22%[bad]%\n";
    let outcome = prepare_content(source, &[], &SessionImportOptions::default()).unwrap();
    assert!(outcome.prepared.passwords.is_empty());
    assert!(
        matches!(&outcome.prepared.connections[0].config, ConnectionType::Ssh { username, .. } if username == "prod")
    );
    assert!(
        matches!(&outcome.prepared.connections[1].config, ConnectionType::Ssh { username, .. } if username == "ops")
    );
    assert!(outcome.report.diagnostics.iter().any(|item| item.kind
        == MobaXtermCredentialDiagnosticKind::AmbiguousMatch
        && item.count == 1));
    assert!(
        outcome
            .report
            .diagnostics
            .iter()
            .all(|item| item.kind != MobaXtermCredentialDiagnosticKind::MissingReference)
    );
    assert_eq!(outcome.prepared.incomplete_credentials_count(), 2);
}

#[test]
fn master_challenge_and_failed_attempt_never_return_partially_decrypted_accounts() {
    let source = format!(
        "[Sesspass]\n[Credentials]\ngood=deploy:{SECRET}\nbad=ops:!!!\n[Bookmarks]\nA=#109#0%a.example%22%[good]%\n"
    );
    assert!(matches!(
        prepare_content(&source, &[], &SessionImportOptions::default()),
        Err(SessionImportError::MobaXtermMasterPasswordRequired)
    ));
    assert!(matches!(
        prepare_content(&source, &[], &options()),
        Err(SessionImportError::MobaXtermCredentialDecryptionFailed)
    ));
    let skipped = prepare_content(
        &source,
        &[],
        &SessionImportOptions {
            mobaxterm_skip_encrypted_credentials: true,
            ..SessionImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(skipped.prepared.connections.len(), 1);
    assert!(skipped.prepared.passwords.is_empty());
    assert_eq!(skipped.prepared.incomplete_credentials_count(), 1);
}

#[test]
fn weak_credentials_import_successes_and_report_damaged_entries() {
    let source = "[Misc]\nSessionP=synthetic-session-p\n[Credentials]\nprod=deploy:TtIHsyiSS+EECsEITeEOeyIOCieEtstC\nbad=other:!!!\n[Bookmarks]\nA=#109#0%a.example%22%[prod]%\nB=#109#0%b.example%22%[bad]%\n";
    let outcome = prepare_content(source, &[], &SessionImportOptions::default()).unwrap();
    assert_eq!(outcome.prepared.passwords.len(), 1);
    assert_eq!(outcome.prepared.incomplete_credentials_count(), 1);
    assert!(
        outcome
            .report
            .diagnostics
            .iter()
            .any(|item| item.kind == MobaXtermCredentialDiagnosticKind::Undecryptable)
    );
}

#[test]
fn v25_marker_requests_master_without_sesspass_and_imports_after_retry() {
    let source = "[Passwords]\ndeploy@a.example=_@QWERTYUIOPASDFGHJKITPTpQ1TPIVVjBQhlq_7cQ==\n[Bookmarks]\nA=#109#0%a.example%2222%deploy%\n";
    assert!(matches!(
        prepare_content(source, &[], &SessionImportOptions::default()),
        Err(SessionImportError::MobaXtermMasterPasswordRequired)
    ));
    let outcome = prepare_content(source, &[], &options()).unwrap();
    assert_eq!(
        outcome.prepared.passwords[0].password.as_deref(),
        Some("synthetic-import")
    );
}

#[test]
fn weak_connection_metadata_imports_duplicate_bookmarks_for_one_endpoint() {
    let source = "[Misc]\nMPSetAccount=synthetic-user\nMPSetComputer=synthetic-pc\n[Passwords]\nssh22:deploy@server.example=TsIO/yxDDrSSR/SITeSYeyIYRxeSs/sR\n[Bookmarks]\nA=#109#0%server.example%22%deploy%\nB=#109#0%server.example%22%deploy%\n";
    let outcome = prepare_content(source, &[], &SessionImportOptions::default()).unwrap();
    assert_eq!(outcome.prepared.passwords.len(), 1);
    assert_eq!(
        outcome.prepared.passwords[0].password.as_deref(),
        Some("synthetic-import")
    );
    assert_eq!(
        outcome.prepared.connections[0]
            .auth
            .as_ref()
            .unwrap()
            .account_id,
        outcome.prepared.connections[1]
            .auth
            .as_ref()
            .unwrap()
            .account_id
    );
    assert!(outcome.report.diagnostics.is_empty());
}

#[test]
fn missing_weak_metadata_requests_user_choice_and_unset_username_claims_nothing() {
    let source = "[Passwords]\nroot@a.example=TtIHsyiSS+EECsEITeEOeyIOCieEtstC\n[Bookmarks]\nA=#109#0%a.example%22%%\n";
    assert!(matches!(
        prepare_content(source, &[], &SessionImportOptions::default()),
        Err(SessionImportError::MobaXtermMasterPasswordRequired)
    ));
    let source =
        format!("[Passwords]\nroot@a.example={SECRET}\n[Bookmarks]\nA=#109#0%a.example%22%%\n");
    let outcome = prepare_content(&source, &[], &options()).unwrap();
    assert!(outcome.prepared.passwords.is_empty());
}

#[test]
fn ini_detection_registry_diagnostics_and_no_ciphertext_do_not_request_password() {
    let dir = TestTempDir::new("nyaterm-moba-ini-detection");
    std::fs::create_dir_all(dir.path()).unwrap();
    let path = dir.join("MobaXterm.ini");
    std::fs::write(
        &path,
        "[Misc]\nPasswordsInRegistry=1\n[Sesspass]\n[Bookmarks]\nA=#109#0%a.example%22%deploy%\n",
    )
    .unwrap();
    let outcome = crate::session_import::mobaxterm::prepare_mobaxterm_import(
        &path,
        SessionImportOptions::default(),
    )
    .unwrap();
    assert_eq!(
        outcome.report.diagnostics[0].kind,
        MobaXtermCredentialDiagnosticKind::RegistryOnly
    );
    assert_eq!(prepare_session_import(&path).unwrap().connections.len(), 1);
    std::fs::write(&path, "[Application]\nfoo=bar").unwrap();
    assert!(matches!(
        prepare_session_import(&path),
        Err(SessionImportError::Config(_))
    ));
    assert!(prepare_content("[Bookmarks]\nUnsupported=#999#0%x%22%u%", &[], &options()).is_err());
    let empty = prepare_content(
        "[Sesspass]\n[Passwords]\ndeploy@a.example=\n[Bookmarks]\nA=#109#0%a.example%22%deploy%",
        &[],
        &SessionImportOptions::default(),
    )
    .unwrap();
    assert!(empty.prepared.passwords.is_empty());
}

#[test]
fn serial_never_claims_encrypted_credentials_and_duplicate_names_are_ambiguous() {
    let source = format!(
        "[Credentials]\nprod=deploy:{SECRET}\nprod=other:{SECRET}\n[Bookmarks]\nA=#109#0%a.example%22%[prod]%\nprod=#131#0%115200%8%0%0%1%0%0%COM3\n"
    );
    let outcome = prepare_content(&source, &[], &options()).unwrap();
    assert!(outcome.prepared.passwords.is_empty());
    assert!(outcome.prepared.connections[1].auth.is_none());
    assert!(
        outcome
            .report
            .diagnostics
            .iter()
            .any(|item| item.kind == MobaXtermCredentialDiagnosticKind::AmbiguousMatch)
    );
}
