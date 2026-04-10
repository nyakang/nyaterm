use super::client::{SshAuth, SshConfig, SshHandler};
use crate::error::{AppError, AppResult};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::MethodKind;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, Mutex};

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

pub(crate) fn load_saved_ssh_config(app: &AppHandle, connection_id: &str) -> AppResult<SshConfig> {
    let conn = crate::config::load_connection_by_id(app, connection_id)?;
    let proxy = resolve_proxy(app, &conn)?;

    let auth = match conn.auth_type.as_str() {
        "password" => {
            let pw_id = conn
                .password_id
                .as_deref()
                .ok_or_else(|| AppError::Auth("No password for this connection".to_string()))?;
            let pw_entry = crate::config::load_password_by_id(app, pw_id)?;
            let password = pw_entry
                .password
                .ok_or_else(|| AppError::Auth("No stored password".to_string()))?;
            SshAuth::Password { password }
        }
        "key" => {
            let key_id = conn
                .key_id
                .as_deref()
                .ok_or_else(|| AppError::Auth("No SSH key for this connection".to_string()))?;
            let ssh_key = crate::config::load_key_by_id(app, key_id)?;
            let key_data = crate::config::decrypt_key_pem(&ssh_key)?
                .ok_or_else(|| AppError::Auth("No key data stored".to_string()))?;
            SshAuth::Key {
                key_data,
                passphrase: ssh_key.passphrase,
            }
        }
        other => return Err(AppError::Auth(format!("Unknown auth type: {}", other))),
    };

    Ok(SshConfig {
        name: conn.name,
        host: conn.host,
        port: conn.port,
        username: conn.username,
        auth,
        proxy,
    })
}

fn resolve_proxy(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
) -> AppResult<Option<crate::config::ProxySettings>> {
    let Some(proxy_id) = &conn.proxy_id else {
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
    authenticate_handle_with_otp(handle, config, app, password_error, key_error, None).await
}

pub(super) async fn authenticate_handle_with_otp(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    password_error: &str,
    key_error: &str,
    connection_id: Option<&str>,
) -> AppResult<()> {
    let otp_info = connection_id.and_then(|cid| resolve_otp_info(app, cid));

    match &config.auth {
        SshAuth::Password { password } => {
            let authenticated = handle
                .authenticate_password(&config.username, password)
                .await
                .map_err(|error| AppError::Auth(format!("Authentication failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                &config.name,
                app,
                password_error,
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
                &config.name,
                app,
                key_error,
                otp_info.as_ref(),
            )
            .await?;
        }
    }

    Ok(())
}

struct OtpAutoFillInfo {
    otp_id: String,
    auto_fill: bool,
}

fn resolve_otp_info(app: &AppHandle, connection_id: &str) -> Option<OtpAutoFillInfo> {
    let conn = crate::config::load_connection_by_id(app, connection_id).ok()?;
    let otp_id = conn.otp_id?;
    Some(OtpAutoFillInfo {
        otp_id,
        auto_fill: conn.auto_fill_otp,
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
    connection_name: &str,
    app: &AppHandle,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    let pending_mgr = app
        .try_state::<Arc<PendingAuthManager>>()
        .ok_or_else(|| AppError::Auth("PendingAuthManager not available".to_string()))?;
    let pending_mgr = pending_mgr.inner().clone();

    let mut step = handle
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(|error| AppError::Auth(format!("Keyboard-interactive start failed: {}", error)))?;

    loop {
        match step {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Err(AppError::Auth(
                    "Keyboard-interactive authentication failed".to_string(),
                ));
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name: _,
                instructions: _,
                prompts,
            } => {
                let responses = if prompts.is_empty() {
                    Vec::new()
                } else if let Some(info) = otp_info.filter(|i| i.auto_fill) {
                    tracing::info!("Auto-filling OTP for keyboard-interactive auth");
                    let result =
                        crate::cmd::otp::generate_otp_for_entry(app, &info.otp_id)?;
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

/// After primary auth returns `Failure`, check if `partial_success` is true and
/// keyboard-interactive is available. If so, run the keyboard-interactive flow.
async fn try_keyboard_interactive_after_partial(
    handle: &mut client::Handle<SshHandler>,
    auth_result: &client::AuthResult,
    username: &str,
    connection_name: &str,
    app: &AppHandle,
    fallback_error: &str,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    match auth_result {
        client::AuthResult::Success => Ok(()),
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            if *partial_success && remaining_methods.contains(&MethodKind::KeyboardInteractive) {
                tracing::info!(
                    "Primary auth partial success, continuing with keyboard-interactive"
                );
                finish_keyboard_interactive(handle, username, connection_name, app, otp_info).await
            } else {
                Err(AppError::Auth(fallback_error.to_string()))
            }
        }
    }
}
