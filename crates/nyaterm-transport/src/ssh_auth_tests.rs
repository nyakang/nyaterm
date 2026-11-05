//! Authentication regressions against a real local SSH peer.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{MethodKind, MethodSet, server};

use crate::ssh_authentication::{
    SshAuthenticationMaterial, SshAuthenticationMethod, SshAuthenticationRequest,
    SshAuthenticationResponse, SshAuthenticationStage,
};
use crate::{
    SshCredentialPrompt, SshCredentialPromptReason, SshCredentialProvider, SshHostKey,
    SshHostKeyDecision, SshHostKeyVerifier, SshKeyAuthConfig, SshKeyboardInteractiveRequest,
    SshSessionConfig,
};

#[derive(Clone, Copy)]
enum Scenario {
    Password,
    Reject,
    PasswordFallback,
    KeyOnly,
    SwitchKey,
    KeyOtp,
    KeyOtpReject,
    RejectOtp,
    RestartFactor,
    IndependentInteractive,
    InteractiveOnly,
    KeyPassword,
    KeyInteractivePassword,
    InteractivePassword,
    RepeatedPassword,
    AlternatingFactors,
    UnsupportedFactor,
    DisconnectPassword,
}

struct Peer {
    scenario: Scenario,
    seen: Arc<Mutex<Vec<String>>>,
    interactive_factor: u32,
    key_authenticated: bool,
}

fn reject(methods: &[MethodKind], partial_success: bool) -> server::Auth {
    server::Auth::Reject {
        proceed_with_methods: Some(MethodSet::from(methods)),
        partial_success,
    }
}

impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        self.seen.lock().unwrap().push("none".into());
        Ok(reject(
            if matches!(
                self.scenario,
                Scenario::InteractiveOnly | Scenario::InteractivePassword
            ) {
                &[MethodKind::KeyboardInteractive]
            } else if matches!(
                self.scenario,
                Scenario::KeyOnly
                    | Scenario::KeyOtp
                    | Scenario::KeyOtpReject
                    | Scenario::KeyPassword
                    | Scenario::KeyInteractivePassword
                    | Scenario::RepeatedPassword
                    | Scenario::AlternatingFactors
                    | Scenario::UnsupportedFactor
                    | Scenario::DisconnectPassword
            ) {
                &[MethodKind::PublicKey]
            } else {
                &[MethodKind::Password, MethodKind::PublicKey]
            },
            false,
        ))
    }
    async fn auth_password(
        &mut self,
        _: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("password:{password}"));
        if matches!(self.scenario, Scenario::DisconnectPassword) {
            return Err(russh::Error::Disconnect);
        }
        Ok(match self.scenario {
            Scenario::KeyPassword | Scenario::KeyInteractivePassword
                if self.key_authenticated && password == "new" =>
            {
                server::Auth::Accept
            }
            Scenario::InteractivePassword
                if self.interactive_factor > 0 && password == "second" =>
            {
                server::Auth::Accept
            }
            Scenario::RepeatedPassword => reject(&[MethodKind::Password], true),
            Scenario::AlternatingFactors => reject(&[MethodKind::KeyboardInteractive], true),
            Scenario::Password if password == "new" => server::Auth::Accept,
            Scenario::SwitchKey => reject(&[MethodKind::PublicKey], false),
            Scenario::PasswordFallback
            | Scenario::RejectOtp
            | Scenario::RestartFactor
            | Scenario::IndependentInteractive => reject(&[MethodKind::KeyboardInteractive], false),
            _ => reject(&[MethodKind::Password], false),
        })
    }
    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: russh::server::PublicKeyOffer<'_>,
    ) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }
    async fn auth_publickey(
        &mut self,
        _: &str,
        _: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<server::Auth, Self::Error> {
        self.seen.lock().unwrap().push("key".into());
        self.key_authenticated = true;
        match self.scenario {
            Scenario::KeyPassword
            | Scenario::RepeatedPassword
            | Scenario::AlternatingFactors
            | Scenario::DisconnectPassword => return Ok(reject(&[MethodKind::Password], true)),
            Scenario::KeyInteractivePassword => {
                return Ok(reject(&[MethodKind::KeyboardInteractive], true));
            }
            Scenario::UnsupportedFactor => return Ok(reject(&[], true)),
            _ => {}
        }
        Ok(
            if matches!(self.scenario, Scenario::KeyOtp | Scenario::KeyOtpReject) {
                reject(
                    &[MethodKind::Password, MethodKind::KeyboardInteractive],
                    true,
                )
            } else {
                server::Auth::Accept
            },
        )
    }
    async fn auth_keyboard_interactive(
        &mut self,
        _: &str,
        _: &str,
        response: Option<server::Response<'_>>,
    ) -> Result<server::Auth, Self::Error> {
        if let Some(mut response) = response {
            let value = response.next().unwrap();
            let value = String::from_utf8(value.to_vec()).unwrap();
            self.seen
                .lock()
                .unwrap()
                .push(format!("interactive:{value}"));
            if matches!(
                self.scenario,
                Scenario::KeyInteractivePassword | Scenario::AlternatingFactors
            ) && self.key_authenticated
                && value == "test-code"
            {
                return Ok(reject(&[MethodKind::Password], true));
            }
            if matches!(self.scenario, Scenario::InteractivePassword) && value == "new" {
                self.interactive_factor += 1;
                return Ok(reject(&[MethodKind::Password], true));
            }
            if matches!(self.scenario, Scenario::RestartFactor)
                && self.interactive_factor == 0
                && value == "new"
            {
                self.interactive_factor += 1;
                return Ok(reject(&[MethodKind::KeyboardInteractive], true));
            }
            if matches!(self.scenario, Scenario::KeyOtpReject) {
                return Ok(reject(&[MethodKind::KeyboardInteractive], false));
            }
            Ok(
                if matches!(
                    self.scenario,
                    Scenario::PasswordFallback | Scenario::InteractiveOnly
                ) && value == "new"
                    || matches!(self.scenario, Scenario::KeyOtp) && value == "test-code"
                    || matches!(
                        self.scenario,
                        Scenario::RestartFactor | Scenario::IndependentInteractive
                    ) && value == "test-code"
                {
                    server::Auth::Accept
                } else {
                    reject(&[MethodKind::Password], false)
                },
            )
        } else {
            Ok(server::Auth::Partial {
                name: "verification".into(),
                instructions: "".into(),
                prompts: vec![(
                    if matches!(
                        self.scenario,
                        Scenario::KeyOtp
                            | Scenario::KeyOtpReject
                            | Scenario::IndependentInteractive
                    ) {
                        "Verification code:".into()
                    } else {
                        "Password:".into()
                    },
                    false,
                )]
                .into(),
            })
        }
    }
}

#[derive(Default)]
struct Provider {
    answers: Mutex<VecDeque<SshAuthenticationMaterial>>,
    requests: Mutex<Vec<SshAuthenticationRequest>>,
    completions: Mutex<Vec<Option<Vec<String>>>>,
    cancel_on_request: Option<(usize, crate::connection_attempt::ConnectionAttempt)>,
}

impl SshCredentialProvider for Provider {
    fn request_secret(&self, _: &SshCredentialPrompt) -> Result<Option<String>, String> {
        panic!("expected structured authentication")
    }
    fn request_authentication(
        &self,
        request: &SshAuthenticationRequest,
    ) -> Result<Option<SshAuthenticationResponse>, String> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request.clone());
        if let Some((number, attempt)) = &self.cancel_on_request
            && requests.len() == *number
        {
            attempt.cancel();
        }
        Ok(self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .map(|material| SshAuthenticationResponse {
                material,
                response_id: Some(format!("response-{}", requests.len())),
            }))
    }
    fn request_keyboard_interactive(
        &self,
        _: &SshKeyboardInteractiveRequest,
    ) -> Result<Option<Vec<String>>, String> {
        Ok(Some(vec!["test-code".into()]))
    }
    fn authentication_finished(&self, ids: Option<&[String]>) {
        self.completions
            .lock()
            .unwrap()
            .push(ids.map(<[String]>::to_vec));
    }
}

struct AcceptKey;
impl SshHostKeyVerifier for AcceptKey {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

fn key() -> russh::keys::PrivateKey {
    russh::keys::PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&[17; 32])),
        "auth-test",
    )
    .unwrap()
}

fn key_material() -> SshAuthenticationMaterial {
    SshAuthenticationMaterial::Key(SshKeyAuthConfig {
        key_data: key()
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .to_string()
            .into(),
        cert_data: None,
        passphrase: None,
    })
}

async fn run(
    scenario: Scenario,
    initial: Option<&str>,
    answers: Vec<SshAuthenticationMaterial>,
) -> (bool, Arc<Provider>, Vec<String>) {
    run_with_config(
        scenario,
        SshSessionConfig {
            password: initial.map(Into::into),
            ..Default::default()
        },
        answers,
        true,
        None,
    )
    .await
}

async fn run_with_config(
    scenario: Scenario,
    mut config: SshSessionConfig,
    answers: Vec<SshAuthenticationMaterial>,
    provide_credentials: bool,
    environment: Option<Arc<crate::ShellEnvironmentCache>>,
) -> (bool, Arc<Provider>, Vec<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let peer = Peer {
        scenario,
        seen: seen.clone(),
        interactive_factor: 0,
        key_authenticated: false,
    };
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![key()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            max_auth_attempts: 30,
            methods: MethodSet::from(
                &[
                    MethodKind::Password,
                    MethodKind::PublicKey,
                    MethodKind::KeyboardInteractive,
                ][..],
            ),
            ..Default::default()
        });
        if let Ok(session) = server::run_stream(config, socket, peer).await {
            let _ = session.await;
        }
    });
    let provider = Arc::new(Provider {
        answers: Mutex::new(answers.into()),
        ..Default::default()
    });
    config.host = "127.0.0.1".into();
    config.port = port;
    config.username = "test".into();
    if provide_credentials && config.credential_provider.is_none() {
        config.credential_provider = Some(provider.clone());
    }
    config.host_key_verifier = Some(Arc::new(AcceptKey));
    let success = if let Some(environment) = environment {
        tokio::time::timeout(
            Duration::from_secs(15),
            crate::open_authenticated_ssh_handle_with_sender_registry(
                &config,
                None,
                None,
                environment,
            ),
        )
        .await
        .unwrap()
        .is_ok()
    } else {
        tokio::time::timeout(
            Duration::from_secs(15),
            tokio::task::spawn_blocking(move || crate::open_ssh_multiplex_handle(config).is_ok()),
        )
        .await
        .unwrap()
        .unwrap()
    };
    server.abort();
    let recorded = seen.lock().unwrap().clone();
    (success, provider, recorded)
}

fn password(value: &str) -> SshAuthenticationMaterial {
    SshAuthenticationMaterial::Password(value.into())
}

#[tokio::test]
async fn runtime_password_success_notifies_only_the_successful_response_once() {
    let (success, provider, seen) = run(Scenario::Password, None, vec![password("new")]).await;
    assert!(success);
    assert_eq!(seen, ["none", "password:new"]);
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into()])]
    );
}

#[tokio::test]
async fn configured_password_counts_towards_three_attempts_and_updates_reason() {
    let (success, provider, seen) = run(
        Scenario::Reject,
        Some("old"),
        vec![password("wrong1"), password("wrong2")],
    )
    .await;
    assert!(!success);
    assert_eq!(seen.len(), 3);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.prompt.attempt)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(
        requests
            .iter()
            .all(|r| r.prompt.reason == SshCredentialPromptReason::PasswordRejected)
    );
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn keyboard_interactive_password_fallback_uses_the_new_runtime_password() {
    let (success, _, seen) = run(Scenario::PasswordFallback, None, vec![password("new")]).await;
    assert!(success);
    assert!(seen.contains(&"interactive:new".into()));
}

#[tokio::test]
async fn interactive_only_server_does_not_receive_an_unsupported_password_submission() {
    let (success, provider, seen) =
        run(Scenario::InteractiveOnly, None, vec![password("new")]).await;
    assert!(success);
    assert_eq!(seen, ["none", "interactive:new"]);
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into()])]
    );
}

#[tokio::test]
async fn interactive_partial_success_stops_password_autofill_for_the_next_factor() {
    let (success, provider, seen) = run(Scenario::RestartFactor, None, vec![password("new")]).await;
    assert!(success);
    assert_eq!(
        seen,
        [
            "none",
            "password:new",
            "interactive:new",
            "interactive:test-code"
        ]
    );
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into()])]
    );
}

#[tokio::test]
async fn rejected_password_unused_by_successful_interactive_auth_is_not_save_eligible() {
    let (success, provider, _) = run(
        Scenario::IndependentInteractive,
        None,
        vec![password("wrong")],
    )
    .await;
    assert!(success);
    assert_eq!(*provider.completions.lock().unwrap(), [Some(vec![])]);
}

#[tokio::test]
async fn publickey_only_server_requests_a_key_before_any_password() {
    let (success, provider, seen) = run(Scenario::KeyOnly, None, vec![key_material()]).await;
    assert!(success);
    assert_eq!(seen, ["none", "key"]);
    assert_eq!(
        provider.requests.lock().unwrap()[0].prompt.reason,
        SshCredentialPromptReason::PublickeyRequired
    );
}

#[tokio::test]
async fn rejected_password_can_switch_to_key_on_the_same_connection() {
    let (success, _, seen) = run(
        Scenario::SwitchKey,
        None,
        vec![password("wrong"), key_material()],
    )
    .await;
    assert!(success);
    assert_eq!(seen, ["none", "password:wrong", "key"]);
}

#[tokio::test]
async fn partial_key_success_continues_otp_before_password_fallback() {
    let (success, provider, seen) = run(Scenario::KeyOtp, None, vec![key_material()]).await;
    assert!(success);
    assert_eq!(seen, ["none", "key", "interactive:test-code"]);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rejected_otp_retries_the_factor_without_rejecting_the_accepted_key_or_saving() {
    let (success, provider, seen) = run(Scenario::KeyOtpReject, None, vec![key_material()]).await;
    assert!(!success);
    assert_eq!(
        seen.iter().filter(|value| value.as_str() == "key").count(),
        1
    );
    assert_eq!(
        seen.iter()
            .filter(|value| value.starts_with("interactive:"))
            .count(),
        3
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn interactive_rejection_retries_and_never_reports_success() {
    let (success, provider, seen) = run(
        Scenario::RejectOtp,
        None,
        vec![password("one"), password("two"), password("three")],
    )
    .await;
    assert!(!success);
    assert_eq!(
        seen.iter().filter(|s| s.starts_with("password:")).count(),
        3
    );
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn cancelling_selection_finishes_without_success() {
    let (success, provider, _) = run(Scenario::Password, None, vec![]).await;
    assert!(!success);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn encrypted_runtime_key_prompts_for_passphrase_and_reports_both_responses() {
    let encrypted = key().encrypt(&mut rand::rng(), " key phrase ").unwrap();
    let material = SshAuthenticationMaterial::Key(SshKeyAuthConfig {
        key_data: encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .to_string()
            .into(),
        cert_data: None,
        passphrase: None,
    });
    let (success, provider, _) = run(
        Scenario::KeyOnly,
        None,
        vec![
            material,
            SshAuthenticationMaterial::KeyPassphrase("wrong".into()),
            SshAuthenticationMaterial::KeyPassphrase(" key phrase ".into()),
        ],
    )
    .await;
    assert!(success);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].key_response_id.as_deref(), Some("response-1"));
    assert_eq!(
        requests[2].prompt.reason,
        SshCredentialPromptReason::KeyPassphraseRejected
    );
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into(), "response-3".into()])]
    );
}

#[tokio::test]
async fn encrypted_key_unlock_stops_after_three_prompts_without_submitting_a_key() {
    let encrypted = key().encrypt(&mut rand::rng(), "phrase").unwrap();
    let (success, provider, seen) = run(
        Scenario::KeyOnly,
        None,
        vec![
            SshAuthenticationMaterial::Key(SshKeyAuthConfig {
                key_data: encrypted
                    .to_openssh(russh::keys::ssh_key::LineEnding::LF)
                    .unwrap()
                    .to_string()
                    .into(),
                cert_data: None,
                passphrase: None,
            }),
            SshAuthenticationMaterial::KeyPassphrase("wrong1".into()),
            SshAuthenticationMaterial::KeyPassphrase("wrong2".into()),
            SshAuthenticationMaterial::KeyPassphrase("wrong3".into()),
        ],
    )
    .await;
    assert!(!success);
    assert_eq!(seen, ["none"]);
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[test]
fn legacy_secret_provider_adapts_to_structured_password_requests() {
    struct Legacy;
    impl SshCredentialProvider for Legacy {
        fn request_secret(&self, _: &SshCredentialPrompt) -> Result<Option<String>, String> {
            Ok(Some("fixture".into()))
        }
    }
    let response = Legacy
        .request_authentication(&SshAuthenticationRequest {
            prompt: SshCredentialPrompt {
                host: "localhost".into(),
                port: 22,
                username: "test".into(),
                connection_name: "test".into(),
                kind: crate::SshCredentialPromptKind::Password,
                reason: SshCredentialPromptReason::MissingPassword,
                attempt: 1,
                prompt_text: None,
                echo: false,
            },
            available_methods: vec![crate::ssh_authentication::SshAuthenticationMethod::Password],
            stage: Default::default(),
            accepted_response_ids: Vec::new(),
            key_response_id: None,
        })
        .unwrap()
        .unwrap();
    assert!(
        matches!(response.material, SshAuthenticationMaterial::Password(ref value) if value == "fixture")
    );
    assert!(response.response_id.is_none());
}

fn configured_key() -> SshSessionConfig {
    let SshAuthenticationMaterial::Key(key_auth) = key_material() else {
        unreachable!()
    };
    SshSessionConfig {
        key_auth: Some(key_auth),
        ..Default::default()
    }
}

#[tokio::test]
async fn configured_key_continues_with_session_only_account_password() {
    let (success, provider, seen) = run_with_config(
        Scenario::KeyPassword,
        configured_key(),
        vec![password("new")],
        true,
        None,
    )
    .await;
    assert!(success);
    assert_eq!(seen, ["key", "password:new"]);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].stage, SshAuthenticationStage::AdditionalFactor);
    assert_eq!(
        requests[0].prompt.reason,
        SshCredentialPromptReason::AdditionalPasswordRequired
    );
    assert_eq!(
        requests[0].available_methods,
        [SshAuthenticationMethod::Password]
    );
    assert_eq!(*provider.completions.lock().unwrap(), [Some(vec![])]);
}

#[tokio::test]
async fn runtime_key_survives_wrong_password_without_resubmitting_publickey() {
    let (success, provider, seen) = run(
        Scenario::KeyPassword,
        None,
        vec![key_material(), password("wrong"), password("new")],
    )
    .await;
    assert!(success);
    assert_eq!(seen, ["none", "key", "password:wrong", "password:new"]);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests[1].prompt.attempt, 1);
    assert_eq!(requests[2].prompt.attempt, 2);
    assert_eq!(
        requests[2].prompt.reason,
        SshCredentialPromptReason::PasswordRejected
    );
    for request in &requests[1..] {
        assert_eq!(request.stage, SshAuthenticationStage::AdditionalFactor);
        assert_eq!(request.accepted_response_ids, ["response-1"]);
        assert!(request.key_response_id.is_none());
    }
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into()])]
    );
}

#[tokio::test]
async fn configured_password_is_not_reused_as_an_additional_factor() {
    let mut config = configured_key();
    config.password = Some("must-not-be-reused".into());
    let (success, provider, seen) = run_with_config(
        Scenario::KeyPassword,
        config,
        vec![password("new")],
        true,
        None,
    )
    .await;
    assert!(success);
    assert_eq!(seen, ["key", "password:new"]);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn additional_password_rejection_stops_after_three_submissions_without_saving() {
    let (success, provider, seen) = run_with_config(
        Scenario::KeyPassword,
        configured_key(),
        vec![
            password("wrong1"),
            password("wrong2"),
            password("wrong3"),
            password("new"),
        ],
        true,
        None,
    )
    .await;
    assert!(!success);
    assert_eq!(
        seen,
        [
            "key",
            "password:wrong1",
            "password:wrong2",
            "password:wrong3"
        ]
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn cancelled_additional_password_never_saves_the_accepted_key() {
    let (success, provider, seen) = run(Scenario::KeyPassword, None, vec![key_material()]).await;
    assert!(!success);
    assert_eq!(seen, ["none", "key"]);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn cancellation_during_password_prompt_prevents_submission_and_late_saving() {
    let mut config = configured_key();
    let provider = Arc::new(Provider {
        answers: Mutex::new(vec![password("new")].into()),
        cancel_on_request: Some((1, config.attempt.clone())),
        ..Default::default()
    });
    config.credential_provider = Some(provider.clone());
    let (success, _, seen) =
        run_with_config(Scenario::KeyPassword, config, vec![], true, None).await;
    assert!(!success);
    assert_eq!(seen, ["key"]);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn additional_factor_cannot_switch_back_to_publickey_or_accept_empty_password() {
    for material in [key_material(), password("")] {
        let (success, provider, seen) = run_with_config(
            Scenario::KeyPassword,
            configured_key(),
            vec![material],
            true,
            None,
        )
        .await;
        assert!(!success);
        assert_eq!(seen, ["key"]);
        assert_eq!(*provider.completions.lock().unwrap(), [None]);
    }
}

#[tokio::test]
async fn missing_additional_password_provider_fails_without_resubmitting_key() {
    let (success, provider, seen) =
        run_with_config(Scenario::KeyPassword, configured_key(), vec![], false, None).await;
    assert!(!success);
    assert_eq!(seen, ["key"]);
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn disconnect_during_additional_password_never_reports_success() {
    let (success, provider, seen) = run_with_config(
        Scenario::DisconnectPassword,
        configured_key(),
        vec![password("new")],
        true,
        None,
    )
    .await;
    assert!(!success);
    assert_eq!(seen, ["key", "password:new"]);
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn unsupported_additional_factor_does_not_request_or_save_credentials() {
    let (success, provider, seen) = run_with_config(
        Scenario::UnsupportedFactor,
        configured_key(),
        vec![password("new")],
        true,
        None,
    )
    .await;
    assert!(!success);
    assert_eq!(seen, ["key"]);
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(*provider.completions.lock().unwrap(), [None]);
}

#[tokio::test]
async fn interactive_factor_can_continue_to_a_distinct_account_password() {
    let (success, provider, seen) = run(
        Scenario::InteractivePassword,
        None,
        vec![password("new"), password("second")],
    )
    .await;
    assert!(success);
    assert_eq!(seen, ["none", "interactive:new", "password:second"]);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests[1].accepted_response_ids, ["response-1"]);
    assert_eq!(requests[1].stage, SshAuthenticationStage::AdditionalFactor);
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into()])]
    );
}

#[tokio::test]
async fn key_and_interactive_factors_can_continue_to_password_without_losing_key() {
    let (success, provider, seen) = run(
        Scenario::KeyInteractivePassword,
        None,
        vec![key_material(), password("new")],
    )
    .await;
    assert!(success);
    assert_eq!(
        seen,
        ["none", "key", "interactive:test-code", "password:new"]
    );
    assert_eq!(
        provider.requests.lock().unwrap()[1].accepted_response_ids,
        ["response-1"]
    );
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into()])]
    );
}

#[tokio::test]
async fn password_and_interactive_factors_share_the_eight_transition_limit() {
    for scenario in [Scenario::RepeatedPassword, Scenario::AlternatingFactors] {
        let (success, provider, seen) = run_with_config(
            scenario,
            configured_key(),
            (0..10).map(|_| password("new")).collect(),
            true,
            None,
        )
        .await;
        assert!(!success);
        assert_eq!(
            seen.iter().filter(|event| event.as_str() == "key").count(),
            1
        );
        assert_eq!(seen.len(), 9);
        assert_eq!(*provider.completions.lock().unwrap(), [None]);
    }
}

#[tokio::test]
async fn unlocked_key_passphrase_remains_eligible_after_additional_password_retries() {
    let encrypted = key().encrypt(&mut rand::rng(), "phrase").unwrap();
    let material = SshAuthenticationMaterial::Key(SshKeyAuthConfig {
        key_data: encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .to_string()
            .into(),
        cert_data: None,
        passphrase: None,
    });
    let (success, provider, seen) = run(
        Scenario::KeyPassword,
        None,
        vec![
            material,
            SshAuthenticationMaterial::KeyPassphrase("phrase".into()),
            password("wrong"),
            password("new"),
        ],
    )
    .await;
    assert!(success);
    assert_eq!(seen, ["none", "key", "password:wrong", "password:new"]);
    let requests = provider.requests.lock().unwrap();
    for request in &requests[2..] {
        assert_eq!(request.accepted_response_ids, ["response-1", "response-2"]);
    }
    assert_eq!(
        *provider.completions.lock().unwrap(),
        [Some(vec!["response-1".into(), "response-2".into()])]
    );
}

#[cfg(any(unix, windows))]
mod fake_agent;
