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
    let conn_auth = conn
        .auth
        .as_ref()
        .ok_or_else(|| AppError::Auth("No auth config for SSH connection".to_string()))?;

    match conn_auth.mode.as_str() {
        "password" => {
            let pw_id = conn_auth
                .password_id
                .as_deref()
                .ok_or_else(|| AppError::Auth("No password for this connection".to_string()))?;
            let pw_entry = crate::config::load_password_by_id(app, pw_id)?;
            let password = pw_entry
                .password
                .ok_or_else(|| AppError::Auth("No stored password".to_string()))?;
            Ok(SshAuth::Password { password })
        }
        "key" => {
            let key_id = conn_auth
                .key_id
                .as_deref()
                .ok_or_else(|| AppError::Auth("No SSH key for this connection".to_string()))?;
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
