use std::sync::Arc;

use nyaterm_transport::connection_attempt::ConnectionAttempt;
use nyaterm_transport::ssh_authentication::{SshAuthenticationMethod, SshAuthenticationRequest};
use nyaterm_transport::{SshCredentialPrompt, SshCredentialPromptKind, SshCredentialPromptReason};

use super::{AuthMethod, RuntimeAuthenticationContext, RuntimeAuthenticationForm};

fn form(methods: Vec<SshAuthenticationMethod>) -> RuntimeAuthenticationForm {
    RuntimeAuthenticationForm::new(
        SshAuthenticationRequest {
            prompt: SshCredentialPrompt {
                host: "localhost".into(),
                port: 22,
                username: "test".into(),
                connection_name: "test".into(),
                kind: SshCredentialPromptKind::Password,
                reason: SshCredentialPromptReason::MissingPassword,
                attempt: 1,
                prompt_text: None,
                echo: false,
            },
            available_methods: methods,
            stage: Default::default(),
            accepted_response_ids: Vec::new(),
            key_response_id: None,
        },
        Arc::new(RuntimeAuthenticationContext::default()),
    )
}

#[test]
fn manual_password_is_default_and_remember_is_off() {
    let form = form(vec![
        SshAuthenticationMethod::Password,
        SshAuthenticationMethod::Publickey,
    ]);
    assert_eq!(form.method, AuthMethod::Password);
    assert!(!form.saved_password);
    assert!(!form.remember);
    assert!(!form.can_submit(""));
    assert!(form.can_submit("secret"));
    assert!(
        !form.can_remember(),
        "temporary password has no persistence target"
    );
}

#[test]
fn source_changes_clear_save_intent_and_require_a_selection() {
    let mut form = form(vec![
        SshAuthenticationMethod::Password,
        SshAuthenticationMethod::Publickey,
    ]);
    form.remember = true;
    form.change_source(AuthMethod::Key, false);
    assert!(!form.remember);
    assert!(!form.can_submit("unused-password"));
    form.key_id = Some("key-1".into());
    assert!(form.can_submit(""));
    form.change_source(AuthMethod::Password, true);
    assert!(!form.can_submit("unused-password"));
    form.password_id = Some("account-1".into());
    assert!(form.can_submit(""));
    form.busy = true;
    assert!(!form.can_submit(""));
}

#[test]
fn publickey_only_request_defaults_to_key() {
    let form = form(vec![SshAuthenticationMethod::Publickey]);
    assert_eq!(form.method, AuthMethod::Key);
    assert!(!form.uses_secret_input());
}

#[test]
fn cancelled_attempt_rejects_late_selections() {
    let context = RuntimeAuthenticationContext::default();
    context.attempt.cancel();
    assert!(
        context
            .remember_selection("late".into(), Some("key".into()), None, None)
            .is_err()
    );
    assert!(context.finish(Some(&["late".into()])).is_empty());
}

#[test]
fn retry_discards_failed_selections_and_passphrase_retains_only_its_key() {
    let context = RuntimeAuthenticationContext::default();
    context
        .remember_selection("failed-password".into(), None, None, None)
        .unwrap();
    context.begin_request(None, &[]);
    assert!(context.selections.lock().unwrap().entries.is_empty());
    context
        .remember_selection("key-response".into(), Some("key".into()), None, None)
        .unwrap();
    context
        .remember_selection("failed-passphrase".into(), None, None, None)
        .unwrap();
    context.begin_request(Some("key-response"), &[]);
    assert_eq!(context.selections.lock().unwrap().entries.len(), 1);
    assert_eq!(
        context.selected_key(Some("key-response")).as_deref(),
        Some("key")
    );
    context.finish(None);
    assert!(context.selections.lock().unwrap().entries.is_empty());
}

#[test]
fn completed_authentication_rejects_late_responses_even_before_attempt_cancellation() {
    let context = RuntimeAuthenticationContext::default();
    context.finish(Some(&[]));
    assert!(!context.attempt.is_cancelled());
    assert!(
        context
            .remember_selection("late-response".into(), None, None, None)
            .is_err()
    );
    assert!(context.selections.lock().unwrap().entries.is_empty());
}

#[test]
fn each_attempt_and_jump_target_has_its_own_selection_state() {
    let first = RuntimeAuthenticationContext::new(
        None,
        None,
        Some("jump-key".into()),
        None,
        ConnectionAttempt::default(),
    );
    let second = first.for_attempt(ConnectionAttempt::default());
    first
        .remember_selection("first".into(), Some("selected".into()), None, None)
        .unwrap();
    assert_eq!(
        first.selected_key(Some("first")).as_deref(),
        Some("selected")
    );
    assert!(second.selected_key(Some("first")).is_none());
    assert_eq!(second.selected_key(None).as_deref(), Some("jump-key"));
    first.attempt.cancel();
    assert!(!second.attempt.is_cancelled());
}

#[test]
fn additional_requests_keep_completed_factors_and_discard_only_failed_candidates() {
    let context = RuntimeAuthenticationContext::default();
    for id in ["key", "passphrase", "wrong-password"] {
        context
            .remember_selection(id.into(), None, None, None)
            .unwrap();
    }
    let accepted = vec!["key".into(), "passphrase".into()];
    context.begin_request(None, &accepted);
    let entries = context.selections.lock().unwrap();
    assert_eq!(entries.entries.len(), 2);
    assert!(entries.entries.contains_key("key"));
    assert!(entries.entries.contains_key("passphrase"));
    drop(entries);
    context.finish(None);
    assert!(context.selections.lock().unwrap().entries.is_empty());
}
