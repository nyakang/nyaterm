use std::sync::Arc;

use gpui::AppContext as _;

use nyaterm_core::test_support::TestTempDir;
use nyaterm_core::{SavedConnection, SavedPassword, SshKey};
use nyaterm_store::ConnectionStore;
use nyaterm_transport::connection_attempt::ConnectionAttempt;
use nyaterm_transport::ssh_authentication::{
    SshAuthenticationMaterial, SshAuthenticationMethod, SshAuthenticationRequest,
    SshAuthenticationStage,
};
use nyaterm_transport::{
    SshCredentialPrompt, SshCredentialPromptKind, SshCredentialPromptReason, SshKeyAuthConfig,
};
use serde_json::json;

use super::prepare_response;
use crate::features::session::runtime_auth::state::{
    RuntimeAuthenticationContext, RuntimeAuthenticationForm,
};

fn request() -> SshAuthenticationRequest {
    SshAuthenticationRequest {
        prompt: SshCredentialPrompt {
            host: "localhost".into(),
            port: 22,
            username: "actual-user".into(),
            connection_name: "fixture".into(),
            kind: SshCredentialPromptKind::Password,
            reason: SshCredentialPromptReason::MissingPassword,
            attempt: 1,
            prompt_text: None,
            echo: false,
        },
        available_methods: vec![SshAuthenticationMethod::Password],
        stage: Default::default(),
        accepted_response_ids: Vec::new(),
        key_response_id: None,
    }
}

fn store() -> (TestTempDir, ConnectionStore) {
    let dir = TestTempDir::new("nyaterm-desktop-runtime-auth");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let connection: SavedConnection = serde_json::from_value(json!({
        "id":"fixture", "name":"fixture", "type":"ssh", "host":"localhost",
        "username":"actual-user", "auth":{"mode":"password"}
    }))
    .unwrap();
    store.save_connection(&connection).unwrap();
    (dir, store)
}

#[test]
fn saved_account_uses_only_its_password_and_reference_is_written_only_after_success() {
    let (_dir, store) = store();
    let id = store
        .save_password(SavedPassword {
            id: String::new(),
            name: "account".into(),
            username: "other-user".into(),
            password: Some("saved secret".into()),
            sort_order: 0,
            has_password: false,
        })
        .unwrap();
    let context = Arc::new(RuntimeAuthenticationContext::new(
        Some(store.runtime_connection_snapshot("fixture").unwrap()),
        None,
        None,
        None,
        ConnectionAttempt::default(),
    ));
    let mut form = RuntimeAuthenticationForm::new(request(), context.clone());
    form.saved_password = true;
    form.password_id = Some(id.clone());
    form.remember = true;
    let (response, key, save) = prepare_response(&store, &form, "ignored".into()).unwrap();
    assert!(
        matches!(response.material, SshAuthenticationMaterial::Password(ref p) if p == "saved secret")
    );
    assert!(key.is_none());
    assert!(
        store
            .get_connection("fixture")
            .unwrap()
            .unwrap()
            .auth
            .unwrap()
            .account_id
            .is_none()
    );
    let response_id = response.response_id.unwrap();
    context
        .remember_selection(response_id.clone(), None, None, save)
        .unwrap();
    let saves = context.finish(Some(&[response_id]));
    assert_eq!(saves.len(), 1);
    assert!(context.finish(Some(&[])).is_empty());
    for save in saves {
        store.save_runtime_credential(save).unwrap();
    }
    let connection = store.get_connection("fixture").unwrap().unwrap();
    let serialized = serde_json::to_value(connection).unwrap();
    assert_eq!(serialized["username"], "actual-user");
    assert_eq!(serialized["auth"]["account_id"], id);
    assert_eq!(serialized["auth"]["username_source"], "connection");
}

#[test]
fn failed_or_cancelled_authentication_discards_prepared_secrets_without_a_write() {
    for cancel in [false, true] {
        let (_dir, store) = store();
        let context = Arc::new(RuntimeAuthenticationContext::new(
            Some(store.runtime_connection_snapshot("fixture").unwrap()),
            None,
            None,
            None,
            ConnectionAttempt::default(),
        ));
        let mut form = RuntimeAuthenticationForm::new(request(), context.clone());
        form.remember = true;
        let (response, _, save) = prepare_response(&store, &form, "failed secret".into()).unwrap();
        let id = response.response_id.unwrap();
        context
            .remember_selection(id.clone(), None, None, save)
            .unwrap();
        if cancel {
            context.attempt.cancel();
        }
        let ids = [id];
        assert!(
            context
                .finish(if cancel { Some(&ids) } else { None })
                .is_empty()
        );
        assert!(
            store
                .get_connection("fixture")
                .unwrap()
                .unwrap()
                .auth
                .unwrap()
                .password
                .is_none()
        );
    }
}

#[test]
fn unavailable_selection_keeps_the_form_and_does_not_prepare_a_save() {
    let (_dir, store) = store();
    let mut form = RuntimeAuthenticationForm::new(request(), Arc::default());
    form.saved_password = true;
    form.password_id = Some("deleted-account".into());
    assert!(prepare_response(&store, &form, "".into()).is_err());
    assert_eq!(form.password_id.as_deref(), Some("deleted-account"));
    assert!(!form.remember);
}

#[test]
fn additional_password_never_prepares_a_save_or_changes_key_or_agent_mode() {
    for mode in ["key", "agent"] {
        for saved_password in [false, true] {
            let (_dir, store) = store();
            let mut connection = store.get_connection("fixture").unwrap().unwrap();
            connection.auth.as_mut().unwrap().mode = mode.into();
            connection.auth.as_mut().unwrap().key_id = Some("original-key".into());
            store.save_connection(&connection).unwrap();
            let original =
                serde_json::to_value(store.get_connection("fixture").unwrap().unwrap()).unwrap();
            let account_id = store
                .save_password(SavedPassword {
                    id: String::new(),
                    name: "account".into(),
                    username: "other-user".into(),
                    password: Some("account secret".into()),
                    sort_order: 0,
                    has_password: false,
                })
                .unwrap();
            let context = Arc::new(RuntimeAuthenticationContext::new(
                Some(store.runtime_connection_snapshot("fixture").unwrap()),
                None,
                None,
                None,
                ConnectionAttempt::default(),
            ));
            let mut additional = request();
            additional.stage = SshAuthenticationStage::AdditionalFactor;
            let mut form = RuntimeAuthenticationForm::new(additional, context.clone());
            form.remember = true;
            form.saved_password = saved_password;
            form.password_id = Some(account_id);
            assert!(!form.can_remember());
            let (response, _, save) =
                prepare_response(&store, &form, "session secret".into()).unwrap();
            assert!(save.is_none());
            let id = response.response_id.unwrap();
            context
                .remember_selection(id.clone(), None, None, save)
                .unwrap();
            assert!(context.finish(Some(&[id])).is_empty());
            assert_eq!(
                serde_json::to_value(store.get_connection("fixture").unwrap().unwrap()).unwrap(),
                original
            );
        }
    }
}

#[test]
fn accepted_passphrase_survives_password_retries_and_is_saved_only_after_full_success() {
    for succeeds in [false, true] {
        let (_dir, store) = store();
        let key_id = store
            .save_ssh_key(SshKey {
                id: String::new(),
                name: "key".into(),
                sort_order: 0,
                key: Some("fixture-key-material".into()),
                cert: None,
                passphrase: None,
                key_file_path: None,
                cert_file_path: None,
                has_key_data: false,
                has_cert_data: false,
            })
            .unwrap();
        let mut connection = store.get_connection("fixture").unwrap().unwrap();
        connection.auth.as_mut().unwrap().mode = "key".into();
        connection.auth.as_mut().unwrap().key_id = Some(key_id.clone());
        store.save_connection(&connection).unwrap();
        let original =
            serde_json::to_value(store.get_connection("fixture").unwrap().unwrap()).unwrap();
        let context = Arc::new(RuntimeAuthenticationContext::new(
            Some(store.runtime_connection_snapshot("fixture").unwrap()),
            None,
            None,
            None,
            ConnectionAttempt::default(),
        ));
        context
            .remember_selection(
                "key-response".into(),
                Some(key_id.clone()),
                Some(SshKeyAuthConfig {
                    key_data: "fixture-key-material".into(),
                    cert_data: None,
                    passphrase: None,
                }),
                None,
            )
            .unwrap();
        let mut unlock = request();
        unlock.prompt.kind = SshCredentialPromptKind::KeyPassphrase;
        unlock.key_response_id = Some("key-response".into());
        let mut form = RuntimeAuthenticationForm::new(unlock, context.clone());
        form.remember = true;
        let (response, _, save) =
            prepare_response(&store, &form, "unlocked phrase".into()).unwrap();
        assert!(save.is_some());
        let phrase_id = response.response_id.unwrap();
        context
            .remember_selection(phrase_id.clone(), None, None, save)
            .unwrap();
        let accepted = vec!["key-response".into(), phrase_id];
        for password in ["wrong password", "correct password"] {
            context.begin_request(None, &accepted);
            let mut additional = request();
            additional.stage = SshAuthenticationStage::AdditionalFactor;
            additional.accepted_response_ids = accepted.clone();
            let form = RuntimeAuthenticationForm::new(additional, context.clone());
            let (response, _, save) = prepare_response(&store, &form, password.into()).unwrap();
            assert!(save.is_none());
            context
                .remember_selection(response.response_id.unwrap(), None, None, save)
                .unwrap();
            assert!(
                store
                    .load_decrypted_ssh_key_by_id(&key_id)
                    .unwrap()
                    .unwrap()
                    .passphrase
                    .is_none()
            );
        }
        let saves = context.finish(succeeds.then_some(accepted.as_slice()));
        assert_eq!(saves.len(), usize::from(succeeds));
        for save in saves {
            store.save_runtime_credential(save).unwrap();
        }
        assert_eq!(
            store
                .load_decrypted_ssh_key_by_id(&key_id)
                .unwrap()
                .unwrap()
                .passphrase
                .as_deref(),
            succeeds.then_some("unlocked phrase")
        );
        assert_eq!(
            serde_json::to_value(store.get_connection("fixture").unwrap().unwrap()).unwrap(),
            original
        );
        assert!(context.finish(Some(&accepted)).is_empty());
    }
}

#[gpui::test]
fn authentication_uses_and_remembers_credentials_without_unlocking_plaintext(
    cx: &mut gpui::TestAppContext,
) {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use nyaterm_store::StoreDomain;

    use crate::features::session::CredentialPromptState;
    use crate::features::session::runtime_auth::state::CredentialResponseSender;
    use crate::features::test_support::{
        app_with_visible_local_session, session_focus_test_window,
    };

    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-auth-without-plaintext-unlock");
    let app = app_with_visible_local_session(cx, root.path(), "terminal");
    let (snapshot, account_id) = app.update(cx, |app, _| {
        let mut summary = app.settings.summary().clone();
        summary.has_master_password = true;
        app.settings.replace_summary(summary);
        app.security.lock_secrets();
        app.store_blocking_client()
            .request_mutation_fn(StoreDomain::Security, |store| {
                let connection: SavedConnection = serde_json::from_value(json!({
                    "id":"fixture", "name":"fixture", "type":"ssh", "host":"localhost",
                    "username":"actual-user", "auth":{"mode":"password"}
                }))?;
                store.save_connection(&connection)?;
                let account_id = store.save_password(SavedPassword {
                    id: String::new(),
                    name: "account".into(),
                    username: "actual-user".into(),
                    password: Some("saved secret".into()),
                    sort_order: 0,
                    has_password: false,
                })?;
                Ok((store.runtime_connection_snapshot("fixture")?, account_id))
            })
            .unwrap()
    });
    let cx = session_focus_test_window(cx, &app);
    for (saved_password, remember) in [(false, false), (false, true), (true, false), (true, true)] {
        let context = Arc::new(RuntimeAuthenticationContext::new(
            Some(snapshot.clone()),
            None,
            None,
            None,
            ConnectionAttempt::default(),
        ));
        let mut form = RuntimeAuthenticationForm::new(request(), context.clone());
        form.saved_password = saved_password;
        form.password_id = saved_password.then(|| account_id.clone());
        form.remember = remember;
        let (sender, submitted) = mpsc::channel();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(app.security_secrets_locked());
                app.session
                    .prompts
                    .activate_credential(CredentialPromptState {
                        id: "without-unlock".into(),
                        prompt: form.request.prompt.clone(),
                        response_tx: CredentialResponseSender::Authentication(sender),
                        authentication: Some(form),
                        value: if saved_password { "" } else { "entered secret" }.into(),
                    });
                app.submit_runtime_authentication(window, cx);
                assert!(!app.security.unlock_prompt_open());
            });
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let response = loop {
            cx.run_until_parked();
            if let Ok(response) = submitted.try_recv() {
                break response.unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "authentication was not submitted"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        let expected = if saved_password {
            "saved secret"
        } else {
            "entered secret"
        };
        assert!(
            matches!(response.material, SshAuthenticationMaterial::Password(ref p) if p == expected)
        );
        let saves = context.finish(Some(&[response.response_id.unwrap()]));
        assert_eq!(saves.len(), usize::from(remember));
        app.read_with(cx, |app, _| {
            assert!(app.session.prompt_active_credential().is_none());
            assert!(app.security_secrets_locked());
            assert!(!app.security.unlock_prompt_open());
        });
    }
}

#[gpui::test]
fn credential_save_failure_keeps_the_session_and_reports_a_warning(cx: &mut gpui::TestAppContext) {
    use std::time::{Duration, Instant};

    use nyaterm_core::models::ssh_runtime_auth::SshCredentialSaveIntent;
    use nyaterm_store::StoreDomain;

    use crate::features::test_support::app_with_visible_local_session;

    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-auth-save-failure");
    let app = app_with_visible_local_session(cx, root.path(), "connected");
    cx.update_entity(&app, |app, cx| {
        let client = app.store_blocking_client();
        let save = client
            .request_mutation_fn(StoreDomain::Security, |store| {
                let connection: SavedConnection = serde_json::from_value(json!({
                    "id":"fixture", "name":"fixture", "type":"ssh", "host":"localhost",
                    "username":"actual-user", "auth":{"mode":"password"}
                }))?;
                store.save_connection(&connection)?;
                let save = store.prepare_runtime_credential_save(
                    Some(store.runtime_connection_snapshot("fixture")?),
                    "actual-user".into(),
                    SshCredentialSaveIntent::ConnectionPassword,
                    Some("authenticated secret".into()),
                )?;
                store.delete_connection("fixture")?;
                Ok(save)
            })
            .unwrap();
        app.save_authenticated_credentials(
            "successful-request".into(),
            vec![save],
            ConnectionAttempt::default(),
            cx,
        );
    });
    let warning = rust_i18n::t!("sshAuth.saveFailed").to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if cx.read(|cx| app.read(cx).shell.status() == warning) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "credential save did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    cx.read(|cx| {
        let app = app.read(cx);
        assert!(!app.session.metadata("connected").unwrap().disconnected);
        assert_eq!(app.visible_terminal_session_ids(), ["connected"]);
    });
}
