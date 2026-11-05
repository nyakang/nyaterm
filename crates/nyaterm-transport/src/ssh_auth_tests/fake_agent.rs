//! Isolated Agent protocol fixtures; never connect to a user's real Agent.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use russh::keys::signature::Signer;
use russh::keys::ssh_encoding::Encode;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{Provider, Scenario, key, password, run_with_config};
use crate::ssh_authentication::{
    SshAuthenticationRequest, SshAuthenticationResponse, SshAuthenticationStage,
};
use crate::{
    ShellEnvironmentCache, SshAgentEndpoint, SshAgentPrompt, SshAgentPromptAction,
    SshAgentPromptProvider, SshAgentPromptRequest, SshCredentialPrompt, SshCredentialProvider,
    SshSessionConfig,
};

fn put_string(frame: &mut Vec<u8>, value: &[u8]) {
    frame.extend_from_slice(&(value.len() as u32).to_be_bytes());
    frame.extend_from_slice(value);
}

fn take_string<'a>(frame: &mut &'a [u8]) -> &'a [u8] {
    let length = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    let value = &frame[4..4 + length];
    *frame = &frame[4 + length..];
    value
}

async fn serve_agent<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    signatures: Arc<AtomicUsize>,
) {
    let key = key();
    let blob = key.public_key().to_bytes().unwrap();
    while let Ok(length) = stream.read_u32().await {
        assert!(length < 4096);
        let mut frame = vec![0; length as usize];
        stream.read_exact(&mut frame).await.unwrap();
        let response = match frame[0] {
            11 => {
                let mut response = vec![12];
                response.extend_from_slice(&1u32.to_be_bytes());
                put_string(&mut response, &blob);
                put_string(&mut response, b"auth-test");
                response
            }
            13 => {
                let mut payload = &frame[1..];
                assert_eq!(take_string(&mut payload), blob);
                let data = take_string(&mut payload);
                assert_eq!(payload, [0; 4]);
                signatures.fetch_add(1, Ordering::SeqCst);
                let signature = key.try_sign(data).unwrap();
                let mut encoded = Vec::new();
                signature.encode(&mut encoded).unwrap();
                let mut response = vec![14];
                put_string(&mut response, &encoded);
                response
            }
            other => panic!("unexpected Agent message {other}"),
        };
        stream.write_u32(response.len() as u32).await.unwrap();
        stream.write_all(&response).await.unwrap();
        stream.flush().await.unwrap();
    }
}

#[derive(Default)]
struct PromptSlot {
    finished: Mutex<bool>,
    changed: Condvar,
}

impl SshAgentPromptRequest for PromptSlot {
    fn wait_action(&self) -> Result<SshAgentPromptAction, String> {
        let mut finished = self.finished.lock().unwrap();
        while !*finished {
            finished = self.changed.wait(finished).unwrap();
        }
        Err("prompt finished".into())
    }

    fn mark_failed(&self, _: &SshAgentPrompt) -> Result<(), String> {
        panic!("fixture Agent must sign successfully")
    }

    fn finish(&self) {
        *self.finished.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

struct PromptProvider(Arc<PromptSlot>);

impl SshAgentPromptProvider for PromptProvider {
    fn request_action(&self, _: &SshAgentPrompt) -> Result<SshAgentPromptAction, String> {
        panic!("fixture Agent must not require retry")
    }

    fn begin_request(
        &self,
        _: &SshAgentPrompt,
    ) -> Result<Option<Arc<dyn SshAgentPromptRequest>>, String> {
        Ok(Some(self.0.clone()))
    }
}

struct PasswordProvider {
    slot: Arc<PromptSlot>,
    inner: Arc<Provider>,
}

impl SshCredentialProvider for PasswordProvider {
    fn request_secret(&self, _: &SshCredentialPrompt) -> Result<Option<String>, String> {
        panic!("expected structured additional password")
    }

    fn request_authentication(
        &self,
        request: &SshAuthenticationRequest,
    ) -> Result<Option<SshAuthenticationResponse>, String> {
        assert!(*self.slot.finished.lock().unwrap());
        assert_eq!(request.stage, SshAuthenticationStage::AdditionalFactor);
        self.inner.request_authentication(request)
    }

    fn authentication_finished(&self, responses: Option<&[String]>) {
        self.inner.authentication_finished(responses);
    }
}

#[tokio::test]
async fn agent_signs_once_and_dismisses_its_prompt_before_additional_password_retries() {
    for succeeds in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let signatures = Arc::new(AtomicUsize::new(0));
        let observed = signatures.clone();
        #[cfg(unix)]
        let (endpoint, environment, agent) = {
            let path = root.path().join("agent.sock");
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let agent = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                serve_agent(stream, observed).await;
            });
            (
                SshAgentEndpoint::UnixSocket {
                    path: path.to_string_lossy().into_owned(),
                },
                ShellEnvironmentCache::new(),
                agent,
            )
        };
        #[cfg(windows)]
        let (endpoint, environment, agent) = {
            let path = format!(r"\\.\pipe\nyaterm-auth-test-{}", uuid::Uuid::new_v4());
            let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(true)
                .create(&path)
                .unwrap();
            let agent = tokio::spawn(async move {
                pipe.connect().await.unwrap();
                serve_agent(pipe, observed).await;
            });
            let environment = ShellEnvironmentCache::with_values_for_test(
                root.path().join("unused-shell"),
                [(
                    "NYATERM_AUTH_TEST_AGENT".into(),
                    crate::EnvironmentValue::new(path),
                )]
                .into_iter()
                .collect(),
            );
            (
                SshAgentEndpoint::Environment {
                    variable: "NYATERM_AUTH_TEST_AGENT".into(),
                },
                environment,
                agent,
            )
        };
        let slot = Arc::new(PromptSlot::default());
        let provider = Arc::new(Provider {
            answers: Mutex::new(if succeeds {
                vec![password("wrong"), password("new")].into()
            } else {
                vec![password("wrong1"), password("wrong2"), password("wrong3")].into()
            }),
            ..Default::default()
        });
        let config = SshSessionConfig {
            agent_auth: true,
            agent_endpoint: endpoint,
            agent_prompt_provider: Some(Arc::new(PromptProvider(slot.clone()))),
            credential_provider: Some(Arc::new(PasswordProvider {
                slot,
                inner: provider.clone(),
            })),
            ..Default::default()
        };
        let (success, _, seen) = run_with_config(
            Scenario::KeyPassword,
            config,
            vec![],
            true,
            Some(environment),
        )
        .await;
        assert_eq!(success, succeeds);
        assert_eq!(signatures.load(Ordering::SeqCst), 1);
        assert_eq!(
            seen.iter().filter(|event| event.as_str() == "key").count(),
            1
        );
        assert_eq!(seen.len(), if succeeds { 3 } else { 4 });
        assert_eq!(
            *provider.completions.lock().unwrap(),
            [succeeds.then(Vec::new)]
        );
        agent.await.unwrap();
    }
}
