//! SSH authentication: key, password, keyboard-interactive and OTP prompts.
//!
//! Split out of `lib.rs` by domain. The method order, prompt classification
//! heuristics and auto-fill rules are unchanged; this only moves the code.

use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::keys::PrivateKeyWithHashAlg;
use russh::{MethodKind, client};

use super::{
    SshAgentPrompt, SshAgentPromptAction, SshAgentPromptPhase, SshAgentPromptRequest,
    SshClientHandler, SshCredentialPrompt, SshCredentialPromptKind, SshCredentialPromptReason,
    SshKeyAuthConfig, SshKeyboardInteractivePrompt, SshKeyboardInteractiveRequest,
    SshSessionConfig,
};
use crate::ssh_agent::connect_agent_client_with_environment_until;
use crate::{ShellEnvironmentCache, ssh_agent::AGENT_CONNECTION_TIMEOUT};

/// Marks a user-selected retry that requires a fresh SSH transport.
#[derive(Debug)]
pub(super) struct SshAgentRetry;

impl std::fmt::Display for SshAgentRetry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SSH Agent authentication retry requested")
    }
}

impl std::error::Error for SshAgentRetry {}

pub(super) fn is_agent_retry(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SshAgentRetry>().is_some()
}

struct AgentPromptFinishGuard(Arc<dyn SshAgentPromptRequest>);

impl Drop for AgentPromptFinishGuard {
    fn drop(&mut self) {
        // A cancelled transport future must resolve the UI request as well.
        self.0.finish();
    }
}

pub(super) async fn authenticate_ssh(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    agent_attempt: u32,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<()> {
    if config.agent_auth {
        return authenticate_ssh_agent(handle, config, agent_attempt, shell_environment).await;
    }
    if let Some(key_auth) = config.key_auth.as_ref() {
        return authenticate_ssh_key(handle, config, key_auth).await;
    }

    if let Some(password) = config
        .password
        .as_deref()
        .filter(|password| !password.is_empty())
    {
        let auth_result = authenticate_password(handle, config, password).await?;
        if auth_result.success() {
            return Ok(());
        }
        if try_keyboard_interactive_after_auth_result(handle, config, &auth_result).await? {
            return Ok(());
        }
        return authenticate_password_with_prompt(
            handle,
            config,
            SshCredentialPromptReason::PasswordRejected,
        )
        .await;
    } else if config.allow_none_auth {
        let auth_result = tokio::time::timeout(
            Duration::from_secs(30),
            handle.authenticate_none(config.username.clone()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH none authentication timed out"))??;
        if auth_result.success() {
            return Ok(());
        }
        if try_keyboard_interactive_after_auth_result(handle, config, &auth_result).await? {
            return Ok(());
        }
        anyhow::bail!("SSH none authentication rejected by server");
    } else {
        authenticate_password_with_prompt(
            handle,
            config,
            SshCredentialPromptReason::MissingPassword,
        )
        .await
    }
}

async fn authenticate_ssh_agent(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    attempt: u32,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<()> {
    let Some(provider) = config.agent_prompt_provider.as_ref() else {
        return authenticate_ssh_agent_once(handle, config, shell_environment)
            .await
            .map_err(|failure| failure.error);
    };

    let pending_prompt = SshAgentPrompt {
        host: config.host.clone(),
        port: config.port,
        username: config.username.clone(),
        connection_name: config.name.clone(),
        phase: SshAgentPromptPhase::Sign,
        attempt,
        message: String::new(),
    };
    let request = provider
        .begin_request(&pending_prompt)
        .map_err(|error| anyhow::anyhow!("SSH Agent prompt failed: {error}"))?;

    let Some(request) = request else {
        match authenticate_ssh_agent_once(handle, config, shell_environment.clone()).await {
            Ok(()) => return Ok(()),
            Err(failure) => {
                let action = provider
                    .request_action(&SshAgentPrompt {
                        host: config.host.clone(),
                        port: config.port,
                        username: config.username.clone(),
                        connection_name: config.name.clone(),
                        phase: failure.phase,
                        attempt,
                        message: failure.error.to_string(),
                    })
                    .map_err(|error| anyhow::anyhow!("SSH Agent prompt failed: {error}"))?;
                match action {
                    SshAgentPromptAction::Retry => return Err(SshAgentRetry.into()),
                    SshAgentPromptAction::Cancel => {
                        anyhow::bail!("SSH Agent authentication was cancelled")
                    }
                }
            }
        }
    };

    let _prompt_guard = AgentPromptFinishGuard(Arc::clone(&request));
    let waiter = Arc::clone(&request);
    let mut action_task = tokio::task::spawn_blocking(move || waiter.wait_action());
    tokio::select! {
        result = authenticate_ssh_agent_once(handle, config, shell_environment.clone()) => {
            match result {
                Ok(()) => {
                    request.finish();
                    let _ = action_task.await;
                    Ok(())
                }
                Err(failure) => {
                    let failed_prompt = SshAgentPrompt {
                        host: config.host.clone(),
                        port: config.port,
                        username: config.username.clone(),
                        connection_name: config.name.clone(),
                        phase: failure.phase,
                        attempt,
                        message: failure.error.to_string(),
                    };
                    if let Err(error) = request.mark_failed(&failed_prompt) {
                        request.finish();
                        return Err(anyhow::anyhow!("SSH Agent prompt failed: {error}"));
                    }
                    let action = match action_task.await {
                        Ok(Ok(action)) => action,
                        Ok(Err(error)) => {
                            request.finish();
                            return Err(anyhow::anyhow!("SSH Agent prompt failed: {error}"));
                        }
                        Err(error) => {
                            request.finish();
                            return Err(anyhow::anyhow!("SSH Agent prompt task failed: {error}"));
                        }
                    };
                    request.finish();
                    match action {
                        SshAgentPromptAction::Retry => Err(SshAgentRetry.into()),
                        SshAgentPromptAction::Cancel => {
                            anyhow::bail!("SSH Agent authentication was cancelled")
                        }
                    }
                }
            }
        }
        action = &mut action_task => {
            request.finish();
            let action = match action {
                Ok(result) => result
                    .map_err(|error| anyhow::anyhow!("SSH Agent prompt failed: {error}"))?,
                Err(error) => {
                    return Err(anyhow::anyhow!("SSH Agent prompt task failed: {error}"));
                }
            };
            match action {
                SshAgentPromptAction::Retry => Err(SshAgentRetry.into()),
                SshAgentPromptAction::Cancel => {
                    anyhow::bail!("SSH Agent authentication was cancelled")
                }
            }
        }
    }
}

struct SshAgentFailure {
    phase: SshAgentPromptPhase,
    error: anyhow::Error,
}

async fn authenticate_ssh_agent_once(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Result<(), SshAgentFailure> {
    let deadline = Instant::now() + AGENT_CONNECTION_TIMEOUT;
    let mut agent = tokio::time::timeout(
        AGENT_CONNECTION_TIMEOUT,
        connect_agent_client_with_environment_until(
            &config.agent_endpoint,
            Some(shell_environment),
            deadline,
        ),
    )
    .await
    .map_err(|_| SshAgentFailure {
        phase: SshAgentPromptPhase::Connect,
        error: anyhow::anyhow!("SSH Agent connection timed out"),
    })?
    .map_err(|error| SshAgentFailure {
        phase: SshAgentPromptPhase::Connect,
        error,
    })?;
    let identities = tokio::time::timeout(Duration::from_secs(5), agent.request_identities())
        .await
        .map_err(|_| SshAgentFailure {
            phase: SshAgentPromptPhase::ListIdentities,
            error: anyhow::anyhow!("SSH Agent identity request timed out"),
        })?
        .map_err(|error| SshAgentFailure {
            phase: SshAgentPromptPhase::ListIdentities,
            error: error.into(),
        })?;
    if identities.is_empty() {
        return Err(SshAgentFailure {
            phase: SshAgentPromptPhase::ListIdentities,
            error: anyhow::anyhow!("SSH Agent has no identities"),
        });
    }
    for identity in identities {
        let key = identity.public_key().into_owned();
        let hash_alg =
            tokio::time::timeout(Duration::from_secs(30), handle.best_supported_rsa_hash())
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .flatten();
        let result = tokio::time::timeout(
            Duration::from_secs(60),
            handle.authenticate_publickey_with(config.username.clone(), key, hash_alg, &mut agent),
        )
        .await
        .map_err(|_| SshAgentFailure {
            phase: SshAgentPromptPhase::Sign,
            error: anyhow::anyhow!(
                "SSH Agent signing timed out after 60 seconds. Confirm the hardware key, enter its PIN, or approve the request in your SSH Agent, then retry."
            ),
        })?
        .map_err(|error| SshAgentFailure {
            phase: SshAgentPromptPhase::Sign,
            error: if matches!(
                &error,
                russh::AgentAuthError::Key(russh::keys::Error::AgentFailure)
            ) {
                anyhow::anyhow!(
                    "SSH Agent rejected the signing request. Confirm the hardware key, PIN, or SSH Agent approval, then retry."
                )
            } else {
                anyhow::anyhow!("SSH Agent signing failed: {error}")
            },
        })?;
        if result.success() {
            return Ok(());
        }
    }
    Err(SshAgentFailure {
        phase: SshAgentPromptPhase::Sign,
        error: anyhow::anyhow!("SSH Agent identities were rejected by the server"),
    })
}

async fn authenticate_ssh_key(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    key_auth: &SshKeyAuthConfig,
) -> anyhow::Result<()> {
    let key = decode_ssh_key_with_prompt(config, key_auth)?;
    let hash_alg = tokio::time::timeout(Duration::from_secs(30), handle.best_supported_rsa_hash())
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .flatten();
    let cert = key_auth
        .cert_data
        .as_deref()
        .map(russh::keys::Certificate::from_openssh)
        .transpose()
        .map_err(|error| anyhow::anyhow!("failed to decode OpenSSH certificate: {error}"))?;

    let auth_result = if let Some(cert) = cert {
        tokio::time::timeout(
            Duration::from_secs(30),
            handle.authenticate_openssh_cert(config.username.clone(), Arc::new(key), cert),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH certificate authentication timed out"))??
    } else {
        tokio::time::timeout(
            Duration::from_secs(30),
            handle.authenticate_publickey(
                config.username.clone(),
                PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH public-key authentication timed out"))??
    };

    if auth_result.success()
        || try_keyboard_interactive_after_auth_result(handle, config, &auth_result).await?
    {
        Ok(())
    } else {
        anyhow::bail!("SSH public-key authentication rejected by server")
    }
}

async fn authenticate_password(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    password: &str,
) -> anyhow::Result<client::AuthResult> {
    tokio::time::timeout(
        Duration::from_secs(30),
        handle.authenticate_password(config.username.clone(), password.to_string()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SSH password authentication timed out"))?
    .map_err(anyhow::Error::from)
}

async fn authenticate_password_with_prompt(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    reason: SshCredentialPromptReason,
) -> anyhow::Result<()> {
    for attempt in 1..=3 {
        let Some(password) = request_runtime_secret(
            config,
            SshCredentialPromptKind::Password,
            reason,
            attempt,
            None,
            false,
        )?
        else {
            anyhow::bail!("SSH password prompt was cancelled");
        };
        if password.is_empty() {
            continue;
        }
        let auth_result = authenticate_password(handle, config, &password).await?;
        if auth_result.success() {
            return Ok(());
        }
        if try_keyboard_interactive_after_auth_result(handle, config, &auth_result).await? {
            return Ok(());
        }
    }
    anyhow::bail!("SSH password authentication rejected by server")
}

const MAX_KEYBOARD_INTERACTIVE_RESTARTS: u32 = 8;

async fn try_keyboard_interactive_after_auth_result(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
    auth_result: &client::AuthResult,
) -> anyhow::Result<bool> {
    match auth_result {
        client::AuthResult::Success => Ok(true),
        client::AuthResult::Failure {
            remaining_methods,
            partial_success: _,
        } if remaining_methods.contains(&MethodKind::KeyboardInteractive) => {
            finish_keyboard_interactive(handle, config).await?;
            Ok(true)
        }
        client::AuthResult::Failure { .. } => Ok(false),
    }
}

async fn finish_keyboard_interactive(
    handle: &mut client::Handle<SshClientHandler>,
    config: &SshSessionConfig,
) -> anyhow::Result<()> {
    let mut step = tokio::time::timeout(
        Duration::from_secs(30),
        handle.authenticate_keyboard_interactive_start(config.username.clone(), None),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SSH keyboard-interactive authentication timed out"))??;
    let mut round = 0_u32;
    let mut restart_count = 0_u32;

    loop {
        match step {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
            client::KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                if partial_success
                    && remaining_methods.contains(&MethodKind::KeyboardInteractive)
                    && restart_count < MAX_KEYBOARD_INTERACTIVE_RESTARTS
                {
                    restart_count = restart_count.saturating_add(1);
                    step = tokio::time::timeout(
                        Duration::from_secs(30),
                        handle
                            .authenticate_keyboard_interactive_start(config.username.clone(), None),
                    )
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "SSH keyboard-interactive restart timed out after partial success"
                        )
                    })??;
                    continue;
                }

                anyhow::bail!(
                    "SSH keyboard-interactive authentication rejected by server (remaining methods: {:?}, partial success: {})",
                    remaining_methods,
                    partial_success
                );
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                round = round.saturating_add(1);
                let prompt_count = prompts.len();
                let responses = if prompts.is_empty() {
                    Vec::new()
                } else if let Some(password) = config
                    .password
                    .as_deref()
                    .filter(|_| should_auto_fill_password_prompts(&prompts))
                {
                    vec![password.to_string()]
                } else if config.auto_fill_otp && should_auto_fill_otp_prompts(&prompts) {
                    match request_otp_response(config, prompt_count)? {
                        Some(responses) => responses,
                        None => request_keyboard_interactive_responses(
                            config,
                            &name,
                            &instructions,
                            prompts,
                            round,
                        )?,
                    }
                } else {
                    request_keyboard_interactive_responses(
                        config,
                        &name,
                        &instructions,
                        prompts,
                        round,
                    )?
                };

                step = tokio::time::timeout(
                    Duration::from_secs(30),
                    handle.authenticate_keyboard_interactive_respond(responses),
                )
                .await
                .map_err(|_| anyhow::anyhow!("SSH keyboard-interactive response timed out"))??;
            }
        }
    }
}

fn request_keyboard_interactive_responses(
    config: &SshSessionConfig,
    name: &str,
    instructions: &str,
    prompts: Vec<client::Prompt>,
    round: u32,
) -> anyhow::Result<Vec<String>> {
    let prompt_count = prompts.len();
    let Some(provider) = config.credential_provider.as_ref() else {
        anyhow::bail!("SSH runtime credential prompt is unavailable");
    };
    let request = SshKeyboardInteractiveRequest {
        host: config.host.clone(),
        port: config.port,
        username: config.username.clone(),
        connection_name: config.name.clone(),
        name: name.to_string(),
        instructions: instructions.to_string(),
        round,
        prompts: prompts
            .into_iter()
            .map(|prompt| SshKeyboardInteractivePrompt {
                prompt: prompt.prompt,
                echo: prompt.echo,
            })
            .collect(),
        otp_id: config.otp_id.clone(),
    };
    let Some(responses) = provider
        .request_keyboard_interactive(&request)
        .map_err(|error| anyhow::anyhow!("SSH keyboard-interactive prompt failed: {error}"))?
    else {
        anyhow::bail!("SSH keyboard-interactive prompt was cancelled");
    };
    if responses.len() != prompt_count {
        anyhow::bail!(
            "SSH keyboard-interactive prompt returned {} responses for {prompt_count} prompts",
            responses.len()
        );
    }
    Ok(responses)
}

fn request_otp_response(
    config: &SshSessionConfig,
    prompt_count: usize,
) -> anyhow::Result<Option<Vec<String>>> {
    let Some(otp_id) = config.otp_id.as_deref().filter(|otp_id| !otp_id.is_empty()) else {
        return Ok(None);
    };
    let Some(provider) = config.otp_provider.as_ref() else {
        return Ok(None);
    };
    let Some(code) = provider
        .request_otp_code(otp_id)
        .map_err(|error| anyhow::anyhow!("SSH OTP auto-fill failed: {error}"))?
    else {
        return Ok(None);
    };
    Ok(Some(vec![code; prompt_count]))
}

pub(super) fn format_keyboard_interactive_prompt(
    name: &str,
    instructions: &str,
    prompt: &str,
    index: usize,
    prompt_count: usize,
) -> String {
    let mut parts = Vec::new();
    if !name.trim().is_empty() {
        parts.push(name.trim().to_string());
    }
    if !instructions.trim().is_empty() {
        parts.push(instructions.trim().to_string());
    }
    if !prompt.trim().is_empty() {
        parts.push(prompt.trim().to_string());
    } else if prompt_count > 1 {
        parts.push(format!("Response {} of {}", index + 1, prompt_count));
    } else {
        parts.push("Response".to_string());
    }
    parts.join("\n")
}

fn should_auto_fill_password_prompts(prompts: &[client::Prompt]) -> bool {
    prompts.len() == 1
        && !prompts[0].echo
        && is_password_keyboard_interactive_prompt(&prompts[0].prompt)
}

fn should_auto_fill_otp_prompts(prompts: &[client::Prompt]) -> bool {
    prompts.len() == 1 && is_otp_keyboard_interactive_prompt(&prompts[0].prompt)
}

fn is_otp_keyboard_interactive_prompt(prompt: &str) -> bool {
    let normalized = prompt.to_lowercase();
    let selection_markers = [
        "select",
        "choose",
        "choice",
        "option",
        "method",
        "delivery",
        "send to",
        "send via",
        "push",
        "sms/email",
        "sms or email",
        "email or sms",
        "选择",
        "请选择",
        "选项",
        "方式",
        "方法",
        "发送到",
        "发送至",
    ];
    if selection_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return false;
    }

    [
        "otp",
        "totp",
        "hotp",
        "2fa",
        "mfa",
        "one-time",
        "one time",
        "verification code",
        "authentication code",
        "auth code",
        "authenticator",
        "passcode",
        "token",
        "验证码",
        "校验码",
        "动态码",
        "动态密码",
        "动态口令",
        "一次性",
        "令牌",
        "双因素",
        "二次",
        "两步",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn is_password_keyboard_interactive_prompt(prompt: &str) -> bool {
    let normalized = prompt.to_lowercase();
    let additional_factor_markers = [
        "otp",
        "totp",
        "hotp",
        "2fa",
        "mfa",
        "one-time",
        "one time",
        "verification",
        "authentication code",
        "auth code",
        "authenticator",
        "passcode",
        "token",
        "code",
        "验证码",
        "校验码",
        "动态码",
        "动态密码",
        "动态口令",
        "一次性",
        "令牌",
        "双因素",
        "二次",
        "两步",
    ];
    if additional_factor_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return false;
    }

    ["password", "passphrase", "密码", "口令"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn decode_ssh_key_with_prompt(
    config: &SshSessionConfig,
    key_auth: &SshKeyAuthConfig,
) -> anyhow::Result<russh::keys::PrivateKey> {
    match russh::keys::decode_secret_key(&key_auth.key_data, key_auth.passphrase.as_deref()) {
        Ok(key) => return Ok(key),
        Err(error) if config.credential_provider.is_none() => {
            anyhow::bail!("failed to decode SSH private key: {error}");
        }
        Err(_) => {}
    }

    for attempt in 1..=3 {
        let Some(passphrase) = request_runtime_secret(
            config,
            SshCredentialPromptKind::KeyPassphrase,
            SshCredentialPromptReason::KeyPassphraseRequired,
            attempt,
            None,
            false,
        )?
        else {
            anyhow::bail!("SSH key passphrase prompt was cancelled");
        };
        match russh::keys::decode_secret_key(&key_auth.key_data, Some(&passphrase)) {
            Ok(key) => return Ok(key),
            Err(error) if attempt == 3 => {
                anyhow::bail!("failed to decode SSH private key: {error}");
            }
            Err(_) => {}
        }
    }

    anyhow::bail!("failed to decode SSH private key")
}

fn request_runtime_secret(
    config: &SshSessionConfig,
    kind: SshCredentialPromptKind,
    reason: SshCredentialPromptReason,
    attempt: u32,
    prompt_text: Option<String>,
    echo: bool,
) -> anyhow::Result<Option<String>> {
    let Some(provider) = config.credential_provider.as_ref() else {
        anyhow::bail!("SSH runtime credential prompt is unavailable");
    };
    provider
        .request_secret(&SshCredentialPrompt {
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            connection_name: config.name.clone(),
            kind,
            reason,
            attempt,
            prompt_text,
            echo,
        })
        .map_err(|error| anyhow::anyhow!("SSH runtime credential prompt failed: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use russh::client;

    use super::{
        SshCredentialPrompt, SshKeyboardInteractivePrompt, SshKeyboardInteractiveRequest,
        SshSessionConfig, request_keyboard_interactive_responses, should_auto_fill_otp_prompts,
        should_auto_fill_password_prompts,
    };
    use crate::SshCredentialProvider;

    #[derive(Default)]
    struct BatchKeyboardInteractiveProvider {
        requests: Mutex<Vec<SshKeyboardInteractiveRequest>>,
    }

    impl SshCredentialProvider for BatchKeyboardInteractiveProvider {
        fn request_secret(&self, _prompt: &SshCredentialPrompt) -> Result<Option<String>, String> {
            panic!("batch keyboard-interactive should not fall back to request_secret")
        }

        fn request_keyboard_interactive(
            &self,
            request: &SshKeyboardInteractiveRequest,
        ) -> Result<Option<Vec<String>>, String> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(Some(vec!["alice".to_string(), "123456".to_string()]))
        }
    }

    #[derive(Default)]
    struct LegacyCredentialProvider {
        prompts: Mutex<Vec<SshCredentialPrompt>>,
    }

    impl SshCredentialProvider for LegacyCredentialProvider {
        fn request_secret(&self, prompt: &SshCredentialPrompt) -> Result<Option<String>, String> {
            self.prompts.lock().unwrap().push(prompt.clone());
            Ok(Some("response".to_string()))
        }
    }

    struct ShortKeyboardInteractiveProvider;

    impl SshCredentialProvider for ShortKeyboardInteractiveProvider {
        fn request_secret(&self, _prompt: &SshCredentialPrompt) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn request_keyboard_interactive(
            &self,
            _request: &SshKeyboardInteractiveRequest,
        ) -> Result<Option<Vec<String>>, String> {
            Ok(Some(vec!["only-one".to_string()]))
        }
    }

    #[test]
    fn keyboard_interactive_prompt_classification_is_conservative() {
        let password_prompts = vec![client::Prompt {
            prompt: "Password: ".to_string(),
            echo: false,
        }];
        assert!(should_auto_fill_password_prompts(&password_prompts));
        assert!(!should_auto_fill_otp_prompts(&password_prompts));

        let otp_prompts = vec![client::Prompt {
            prompt: "Verification code: ".to_string(),
            echo: false,
        }];
        assert!(should_auto_fill_otp_prompts(&otp_prompts));
        assert!(!should_auto_fill_password_prompts(&otp_prompts));

        let selection_prompts = vec![client::Prompt {
            prompt: "Choose MFA method: ".to_string(),
            echo: true,
        }];
        assert!(!should_auto_fill_otp_prompts(&selection_prompts));
        assert!(!should_auto_fill_password_prompts(&selection_prompts));
    }

    #[test]
    fn keyboard_interactive_requests_are_delivered_as_one_challenge() {
        let provider = Arc::new(BatchKeyboardInteractiveProvider::default());
        let config = SshSessionConfig {
            name: "Production".to_string(),
            host: "host.example.com".to_string(),
            port: 2222,
            username: "root".to_string(),
            otp_id: Some("otp-1".to_string()),
            credential_provider: Some(provider.clone()),
            ..SshSessionConfig::default()
        };
        let responses = request_keyboard_interactive_responses(
            &config,
            "Login verification",
            "Complete both fields",
            vec![
                client::Prompt {
                    prompt: "Account:".to_string(),
                    echo: true,
                },
                client::Prompt {
                    prompt: "Code:".to_string(),
                    echo: false,
                },
            ],
            2,
        )
        .expect("keyboard-interactive responses");

        assert_eq!(responses, ["alice", "123456"]);
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.connection_name, "Production");
        assert_eq!(request.name, "Login verification");
        assert_eq!(request.instructions, "Complete both fields");
        assert_eq!(request.round, 2);
        assert_eq!(request.otp_id.as_deref(), Some("otp-1"));
        assert_eq!(request.prompts.len(), 2);
        assert!(request.prompts[0].echo);
        assert!(!request.prompts[1].echo);
    }

    #[test]
    fn keyboard_interactive_default_provider_keeps_single_prompt_compatibility() {
        let provider = LegacyCredentialProvider::default();
        let request = SshKeyboardInteractiveRequest {
            host: "host.example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            connection_name: "Production".to_string(),
            name: "Verification".to_string(),
            instructions: "Use both factors".to_string(),
            round: 3,
            prompts: vec![
                SshKeyboardInteractivePrompt {
                    prompt: "Password:".to_string(),
                    echo: false,
                },
                SshKeyboardInteractivePrompt {
                    prompt: "Code:".to_string(),
                    echo: false,
                },
            ],
            otp_id: None,
        };

        let responses = provider
            .request_keyboard_interactive(&request)
            .expect("fallback responses")
            .expect("not cancelled");

        assert_eq!(responses, ["response", "response"]);
        let prompts = provider.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].attempt, 3);
        assert_eq!(
            prompts[0].prompt_text.as_deref(),
            Some("Verification\nUse both factors\nPassword:")
        );
    }

    #[test]
    fn keyboard_interactive_rejects_response_count_mismatches() {
        let config = SshSessionConfig {
            credential_provider: Some(Arc::new(ShortKeyboardInteractiveProvider)),
            ..SshSessionConfig::default()
        };
        let error = request_keyboard_interactive_responses(
            &config,
            "Verification",
            "",
            vec![
                client::Prompt {
                    prompt: "First:".to_string(),
                    echo: true,
                },
                client::Prompt {
                    prompt: "Second:".to_string(),
                    echo: false,
                },
            ],
            1,
        )
        .expect_err("response count mismatch");

        assert!(error.to_string().contains("1 responses for 2 prompts"));
    }
}
