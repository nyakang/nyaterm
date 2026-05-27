use super::client::{SshAuth, SshConfig, SshHandler};
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use russh::MethodKind;
use russh::client::{self, KeyboardInteractiveAuthResponse};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, oneshot};

/// Manages pending keyboard-interactive auth requests awaiting user input from the frontend.
pub struct PendingAuthManager {
    pending: Mutex<HashMap<String, oneshot::Sender<Option<Vec<String>>>>>,
}

impl PendingAuthManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, request_id: String) -> oneshot::Receiver<Option<Vec<String>>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);
        rx
    }

    pub async fn respond(&self, request_id: &str, responses: Option<Vec<String>>) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            tx.send(responses).is_ok()
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct OtpPrompt {
    prompt: String,
    echo: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OtpRequestPayload {
    request_id: String,
    connection_name: String,
    prompts: Vec<OtpPrompt>,
    otp_entry_id: Option<String>,
}

fn build_ids(connection_id: Option<&str>, request_id: Option<&str>) -> Option<Value> {
    let mut ids = Map::new();
    if let Some(connection_id) = connection_id {
        ids.insert(
            "connection_id".to_string(),
            Value::String(connection_id.to_string()),
        );
    }
    if let Some(request_id) = request_id {
        ids.insert(
            "request_id".to_string(),
            Value::String(request_id.to_string()),
        );
    }
    if ids.is_empty() {
        None
    } else {
        Some(Value::Object(ids))
    }
}

fn log_structured(
    level: StructuredLogLevel,
    domain: &str,
    event: &str,
    message: &str,
    connection_id: Option<&str>,
    request_id: Option<&str>,
    data: Option<Value>,
    error: Option<Value>,
) {
    observability::log_event(StructuredLog {
        level,
        domain: domain.to_string(),
        event: event.to_string(),
        message: message.to_string(),
        ids: build_ids(connection_id, request_id),
        data,
        error,
        client_timestamp: None,
    });
}

pub(crate) fn load_saved_ssh_config(app: &AppHandle, connection_id: &str) -> AppResult<SshConfig> {
    let conn = crate::config::load_connection_by_id(app, connection_id)?;
    resolve_saved_ssh_config(app, &conn, Some(connection_id.to_string()), true)
}

fn resolve_saved_ssh_config(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
    connection_id: Option<String>,
    include_proxy_jump: bool,
) -> AppResult<SshConfig> {
    let proxy = resolve_proxy(app, conn)?;
    let (host, port, username) = resolve_ssh_target(conn)?;
    let auth = resolve_auth(app, conn)?;
    let proxy_jump = if include_proxy_jump {
        resolve_proxy_jump(app, conn)?
    } else {
        None
    };

    Ok(SshConfig {
        connection_id,
        name: conn.name.clone(),
        host,
        port,
        username,
        auth,
        proxy,
        proxy_jump,
    })
}

fn resolve_ssh_target(conn: &crate::config::SavedConnection) -> AppResult<(String, u16, String)> {
    match &conn.config {
        crate::config::ConnectionType::Ssh {
            host,
            port,
            username,
        } => Ok((host.clone(), *port, username.clone())),
        _ => Err(AppError::Auth(
            "Connection is not an SSH connection".to_string(),
        )),
    }
}

fn resolve_auth(app: &AppHandle, conn: &crate::config::SavedConnection) -> AppResult<SshAuth> {
    let Some(conn_auth) = conn.auth.as_ref() else {
        return Ok(SshAuth::None);
    };

    match conn_auth.mode.as_str() {
        "none" => Ok(SshAuth::None),
        "password" => {
            if let Some(ref ciphertext) = conn_auth.password {
                let password = crate::utils::crypto::decrypt(ciphertext).map_err(|e| {
                    AppError::Auth(format!("Failed to decrypt inline password: {e}"))
                })?;
                return Ok(SshAuth::Password { password });
            }

            let Some(pw_id) = conn_auth.password_id.as_deref() else {
                return Ok(SshAuth::None);
            };
            let pw_entry = crate::config::load_password_by_id(app, pw_id)?;
            let password = pw_entry
                .password
                .ok_or_else(|| AppError::Auth("No stored password".to_string()))?;
            Ok(SshAuth::Password { password })
        }
        "key" => {
            let Some(key_id) = conn_auth.key_id.as_deref() else {
                return Ok(SshAuth::None);
            };
            let ssh_key = crate::config::load_key_by_id(app, key_id)?;
            let key_data = crate::config::decrypt_key_pem(&ssh_key)?
                .ok_or_else(|| AppError::Auth("No key data stored".to_string()))?;
            Ok(SshAuth::Key {
                key_data,
                passphrase: ssh_key.passphrase,
            })
        }
        other => Err(AppError::Auth(format!("Unknown auth type: {}", other))),
    }
}

fn resolve_proxy_jump(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
) -> AppResult<Option<Box<SshConfig>>> {
    let proxy_jump_id = conn
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref());

    let Some(proxy_jump_id) = proxy_jump_id else {
        return Ok(None);
    };

    let jump_conn = crate::config::load_connection_by_id(app, proxy_jump_id)?;
    if !matches!(jump_conn.config, crate::config::ConnectionType::Ssh { .. }) {
        return Err(AppError::Config(
            "Only SSH connections can be used as jump hosts".to_string(),
        ));
    }
    if jump_conn
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref())
        .is_some()
    {
        return Err(AppError::Config(
            "Jump hosts cannot use another jump host".to_string(),
        ));
    }

    Ok(Some(Box::new(resolve_saved_ssh_config(
        app,
        &jump_conn,
        Some(proxy_jump_id.to_string()),
        false,
    )?)))
}

fn resolve_proxy(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
) -> AppResult<Option<crate::config::ProxySettings>> {
    let proxy_id = conn.network.as_ref().and_then(|n| n.proxy_id.as_deref());

    let Some(proxy_id) = proxy_id else {
        return Ok(None);
    };

    let proxy_cfg = crate::config::load_proxy_by_id(app, proxy_id)?
        .ok_or_else(|| AppError::Config(format!("Proxy '{}' not found", proxy_id)))?;
    let password = proxy_cfg
        .password
        .as_ref()
        .and_then(|ciphertext| crate::utils::crypto::decrypt(ciphertext).ok());

    Ok(Some(crate::config::ProxySettings {
        enabled: true,
        protocol: proxy_cfg.protocol,
        host: proxy_cfg.host,
        port: proxy_cfg.port,
        username: proxy_cfg.username,
        password,
    }))
}

pub(super) async fn authenticate_handle(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    password_error: &str,
    key_error: &str,
) -> AppResult<()> {
    let otp_info = config
        .connection_id
        .as_deref()
        .and_then(|connection_id| resolve_otp_info(app, connection_id));

    match &config.auth {
        SshAuth::None => {
            log_structured(
                StructuredLogLevel::Info,
                "ssh.auth",
                "auth.start",
                "Starting SSH authentication",
                config.connection_id.as_deref(),
                None,
                Some(json!({
                    "host": config.host,
                    "port": config.port,
                    "username": config.username,
                    "auth_mode": "none",
                })),
                None,
            );

            let authenticated = handle
                .authenticate_none(&config.username)
                .await
                .map_err(|error| AppError::Auth(format!("None auth failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                config.connection_id.as_deref(),
                &config.name,
                app,
                "Authentication failed: none auth rejected",
                Some(KeyboardInteractiveMode::AdditionalFactor),
                otp_info.as_ref(),
            )
            .await?;
        }
        SshAuth::Password { password } => {
            log_structured(
                StructuredLogLevel::Info,
                "ssh.auth",
                "auth.start",
                "Starting SSH authentication",
                config.connection_id.as_deref(),
                None,
                Some(json!({
                    "host": config.host,
                    "port": config.port,
                    "username": config.username,
                    "auth_mode": "password",
                })),
                None,
            );

            let authenticated = handle
                .authenticate_password(&config.username, password)
                .await
                .map_err(|error| AppError::Auth(format!("Authentication failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                config.connection_id.as_deref(),
                &config.name,
                app,
                password_error,
                Some(KeyboardInteractiveMode::PasswordFallback { password }),
                otp_info.as_ref(),
            )
            .await?;
        }
        SshAuth::Key {
            key_data,
            passphrase,
        } => {
            let key = russh::keys::decode_secret_key(key_data, passphrase.as_deref())?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();

            log_structured(
                StructuredLogLevel::Info,
                "ssh.auth",
                "auth.start",
                "Starting SSH authentication",
                config.connection_id.as_deref(),
                None,
                Some(json!({
                    "host": config.host,
                    "port": config.port,
                    "username": config.username,
                    "auth_mode": "publickey",
                    "key_algorithm": key.algorithm().to_string(),
                    "rsa_hash": format!("{hash_alg:?}"),
                })),
                None,
            );

            let authenticated = handle
                .authenticate_publickey(
                    &config.username,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(|error| AppError::Auth(format!("Key auth failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                config.connection_id.as_deref(),
                &config.name,
                app,
                key_error,
                None,
                otp_info.as_ref(),
            )
            .await?;
        }
    }

    log_structured(
        StructuredLogLevel::Info,
        "ssh.auth",
        "auth.succeeded",
        "SSH authentication succeeded",
        config.connection_id.as_deref(),
        None,
        Some(json!({
            "host": config.host,
            "port": config.port,
            "username": config.username,
        })),
        None,
    );

    Ok(())
}

struct OtpAutoFillInfo {
    otp_id: String,
    auto_fill: bool,
}

#[derive(Debug, Clone, Copy)]
enum KeyboardInteractiveMode<'a> {
    AdditionalFactor,
    PasswordFallback { password: &'a str },
}

impl<'a> KeyboardInteractiveMode<'a> {
    fn label(self) -> &'static str {
        match self {
            Self::AdditionalFactor => "additional-factor",
            Self::PasswordFallback { .. } => "password-fallback",
        }
    }

    fn password(self) -> Option<&'a str> {
        match self {
            Self::AdditionalFactor => None,
            Self::PasswordFallback { password } => Some(password),
        }
    }
}

fn resolve_otp_info(app: &AppHandle, connection_id: &str) -> Option<OtpAutoFillInfo> {
    let conn = crate::config::load_connection_by_id(app, connection_id).ok()?;
    let auth = conn.auth.as_ref()?;
    let otp_id = auth.otp_id.clone()?;
    Some(OtpAutoFillInfo {
        otp_id,
        auto_fill: auth.auto_fill_otp,
    })
}

/// Runs the keyboard-interactive auth state machine, emitting `otp-request` events
/// to the frontend for each `InfoRequest` that contains prompts, and automatically
/// responding with an empty array for empty `InfoRequest`s.
///
/// When `otp_info` is present with `auto_fill == true`, the OTP code is generated
/// automatically and used as the response without prompting the user.
async fn finish_keyboard_interactive(
    handle: &mut client::Handle<SshHandler>,
    username: &str,
    connection_id: Option<&str>,
    connection_name: &str,
    app: &AppHandle,
    mode: KeyboardInteractiveMode<'_>,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    let pending_mgr = app
        .try_state::<Arc<PendingAuthManager>>()
        .ok_or_else(|| AppError::Auth("PendingAuthManager not available".to_string()))?;
    let pending_mgr = pending_mgr.inner().clone();

    log_structured(
        StructuredLogLevel::Info,
        "ssh.auth",
        "keyboard_interactive.start",
        "Starting keyboard-interactive authentication",
        connection_id,
        None,
        Some(json!({
            "username": username,
            "mode": mode.label(),
        })),
        None,
    );

    let mut step = handle
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(|error| AppError::Auth(format!("Keyboard-interactive start failed: {}", error)))?;

    loop {
        match step {
            KeyboardInteractiveAuthResponse::Success => {
                log_structured(
                    StructuredLogLevel::Info,
                    "ssh.auth",
                    "keyboard_interactive.succeeded",
                    "Keyboard-interactive authentication succeeded",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "mode": mode.label(),
                    })),
                    None,
                );
                return Ok(());
            }
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                log_structured(
                    StructuredLogLevel::Warn,
                    "ssh.auth",
                    "keyboard_interactive.failed",
                    "Keyboard-interactive authentication failed",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "mode": mode.label(),
                        "remaining_methods": format!("{remaining_methods:?}"),
                        "partial_success": partial_success,
                    })),
                    None,
                );
                return Err(AppError::Auth(
                    "Keyboard-interactive authentication failed".to_string(),
                ));
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name: _,
                instructions: _,
                prompts,
            } => {
                let hidden_prompts = prompts.iter().filter(|prompt| !prompt.echo).count();
                log_structured(
                    StructuredLogLevel::Debug,
                    "ssh.auth",
                    "keyboard_interactive.prompts_received",
                    "Received keyboard-interactive prompts",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "mode": mode.label(),
                        "prompt_count": prompts.len(),
                        "hidden_prompts": hidden_prompts,
                    })),
                    None,
                );

                let responses = if prompts.is_empty() {
                    Vec::new()
                } else if let Some(password) = mode
                    .password()
                    .filter(|_| should_auto_fill_password_prompts(&prompts))
                {
                    log_structured(
                        StructuredLogLevel::Info,
                        "security.flow",
                        "keyboard_interactive.password_autofill",
                        "Auto-filling password for keyboard-interactive auth",
                        connection_id,
                        None,
                        Some(json!({
                            "username": username,
                            "prompt_count": prompts.len(),
                        })),
                        None,
                    );
                    vec![password.to_string()]
                } else if let Some(info) = otp_info.filter(|i| i.auto_fill) {
                    log_structured(
                        StructuredLogLevel::Info,
                        "security.flow",
                        "otp.auto_fill",
                        "Auto-filling OTP for keyboard-interactive auth",
                        connection_id,
                        None,
                        Some(json!({
                            "username": username,
                            "otp_entry_id": info.otp_id,
                            "prompt_count": prompts.len(),
                        })),
                        None,
                    );
                    let result = crate::cmd::otp::generate_otp_for_entry(app, &info.otp_id)?;
                    prompts.iter().map(|_| result.code.clone()).collect()
                } else {
                    let request_id = uuid::Uuid::new_v4().to_string();
                    let rx = pending_mgr.register(request_id.clone()).await;

                    let payload = OtpRequestPayload {
                        request_id: request_id.clone(),
                        connection_name: connection_name.to_string(),
                        prompts: prompts
                            .iter()
                            .map(|prompt| OtpPrompt {
                                prompt: prompt.prompt.clone(),
                                echo: prompt.echo,
                            })
                            .collect(),
                        otp_entry_id: otp_info.map(|i| i.otp_id.clone()),
                    };
                    log_structured(
                        StructuredLogLevel::Info,
                        "security.flow",
                        "otp.requested",
                        "Forwarding keyboard-interactive prompts to frontend",
                        connection_id,
                        Some(&request_id),
                        Some(json!({
                            "username": username,
                            "prompt_count": payload.prompts.len(),
                            "otp_entry_id": payload.otp_entry_id,
                        })),
                        None,
                    );
                    let _ = app.emit("otp-request", &payload);

                    match rx.await {
                        Ok(Some(responses)) => responses,
                        Ok(None) => {
                            return Err(AppError::Auth(
                                "2FA authentication cancelled by user".to_string(),
                            ));
                        }
                        Err(_) => {
                            return Err(AppError::Auth(
                                "2FA authentication request dropped".to_string(),
                            ));
                        }
                    }
                };

                step = handle
                    .authenticate_keyboard_interactive_respond(responses)
                    .await
                    .map_err(|error| {
                        AppError::Auth(format!("Keyboard-interactive respond failed: {}", error))
                    })?;
            }
        }
    }
}

/// After primary auth returns `Failure`, continue with keyboard-interactive when
/// the server advertises it and either partial success or an explicit fallback applies.
async fn try_keyboard_interactive_after_partial(
    handle: &mut client::Handle<SshHandler>,
    auth_result: &client::AuthResult,
    username: &str,
    connection_id: Option<&str>,
    connection_name: &str,
    app: &AppHandle,
    fallback_error: &str,
    keyboard_interactive_fallback: Option<KeyboardInteractiveMode<'_>>,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    match auth_result {
        client::AuthResult::Success => Ok(()),
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            let keyboard_interactive_available =
                remaining_methods.contains(&MethodKind::KeyboardInteractive);
            let can_retry_keyboard_interactive =
                keyboard_interactive_available && keyboard_interactive_fallback.is_some();

            if *partial_success && keyboard_interactive_available {
                log_structured(
                    StructuredLogLevel::Info,
                    "ssh.auth",
                    "auth.partial_success",
                    "Primary auth partial success, continuing with keyboard-interactive",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "remaining_methods": format!("{remaining_methods:?}"),
                    })),
                    None,
                );
                finish_keyboard_interactive(
                    handle,
                    username,
                    connection_id,
                    connection_name,
                    app,
                    KeyboardInteractiveMode::AdditionalFactor,
                    otp_info,
                )
                .await
            } else if can_retry_keyboard_interactive {
                log_structured(
                    StructuredLogLevel::Info,
                    "ssh.auth",
                    "auth.keyboard_interactive_fallback",
                    "Primary auth rejected, retrying with keyboard-interactive",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "remaining_methods": format!("{remaining_methods:?}"),
                    })),
                    None,
                );
                let Some(mode) = keyboard_interactive_fallback else {
                    return Err(AppError::Auth(fallback_error.to_string()));
                };
                finish_keyboard_interactive(
                    handle,
                    username,
                    connection_id,
                    connection_name,
                    app,
                    mode,
                    otp_info,
                )
                .await
            } else {
                log_structured(
                    StructuredLogLevel::Warn,
                    "ssh.auth",
                    "auth.failed",
                    "SSH authentication failed without usable keyboard-interactive fallback",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "remaining_methods": format!("{remaining_methods:?}"),
                        "partial_success": partial_success,
                    })),
                    Some(json!({
                        "message": fallback_error,
                    })),
                );
                Err(AppError::Auth(fallback_error.to_string()))
            }
        }
    }
}

fn should_auto_fill_password_prompts(prompts: &[client::Prompt]) -> bool {
    prompts.len() == 1 && !prompts[0].echo
}

#[cfg(test)]
mod tests {
    use super::{KeyboardInteractiveMode, should_auto_fill_password_prompts};
    use russh::client::Prompt;

    #[test]
    fn auto_fills_single_hidden_keyboard_interactive_prompt() {
        let prompts = vec![Prompt {
            prompt: "Password: ".to_string(),
            echo: false,
        }];

        assert!(should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn does_not_auto_fill_multiple_keyboard_interactive_prompts() {
        let prompts = vec![
            Prompt {
                prompt: "Password: ".to_string(),
                echo: false,
            },
            Prompt {
                prompt: "Verification code: ".to_string(),
                echo: false,
            },
        ];

        assert!(!should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn does_not_auto_fill_echoed_prompt() {
        let prompts = vec![Prompt {
            prompt: "Username: ".to_string(),
            echo: true,
        }];

        assert!(!should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn additional_factor_mode_never_exposes_password_fallback() {
        assert!(
            KeyboardInteractiveMode::AdditionalFactor
                .password()
                .is_none()
        );
    }
}
