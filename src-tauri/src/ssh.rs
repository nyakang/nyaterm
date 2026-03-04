//! SSH session creation, TOFU known_hosts verification, and I/O loop.
//!
//! Uses russh for connection/auth and emits terminal output via Tauri events.

use crate::error::{AppError, AppResult};
use crate::session::{SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType};
use russh::client;
use russh::ChannelMsg;
use russh::keys::PublicKeyBase64;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

/// Connection parameters for SSH (host, port, user, auth method).
#[derive(Debug, Clone, Deserialize)]
pub struct SshConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
}

/// Authentication method: password or key (with optional passphrase).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum SshAuth {
    #[serde(rename = "password")]
    Password { password: String },
    #[serde(rename = "key")]
    Key {
        key_data: String,
        passphrase: Option<String>,
    },
}

/// russh client handler; performs TOFU known_hosts verification.
pub(crate) struct SshHandler {
    app: AppHandle,
    host: String,
    port: u16,
}

impl SshHandler {
    pub fn new(app: AppHandle, host: String, port: u16) -> Self {
        Self { app, host, port }
    }

    fn get_known_hosts_path(&self) -> Option<std::path::PathBuf> {
        self.app
            .path()
            .home_dir()
            .ok()
            .map(|h: std::path::PathBuf| h.join(".dragonfly").join("known_hosts"))
    }
}

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let path = match self.get_known_hosts_path() {
            Some(p) => p,
            None => return Ok(false),
        };

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let key_type = server_public_key.algorithm().to_string();
        let key_base64 = server_public_key.public_key_base64();
        let fingerprint = server_public_key.fingerprint(Default::default());

        let host_identifier = if self.port != 22 {
            format!("[{}]:{}", self.host, self.port)
        } else {
            self.host.clone()
        };

        let host_entry = format!("{} {} {}", host_identifier, key_type, key_base64);

        let content = std::fs::read_to_string(&path).unwrap_or_default();

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[0] == host_identifier {
                if parts[1] == key_type && parts[2] == key_base64 {
                    return Ok(true);
                }
                // Host key mismatch — potential MITM
                let _ = self.app.emit(
                    "ssh-error",
                    format!(
                        "SECURITY ALERT: Host key for {}:{} has changed! New fingerprint: {}",
                        self.host, self.port, fingerprint
                    ),
                );
                return Ok(false);
            }
        }

        // TOFU: trust on first use — add new host key
        use std::io::Write;
        if let Ok(mut file) =
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        {
            if let Err(e) = writeln!(file, "{}", host_entry) {
                let _ = self.app.emit(
                    "ssh-error",
                    format!("Failed to save known_hosts: {}", e),
                );
                return Ok(false);
            }
        }

        Ok(true)
    }
}

pub async fn connect_with_proxy(
    app: &AppHandle,
    config: &SshConfig,
    ssh_config: Arc<client::Config>,
    handler: SshHandler,
) -> AppResult<client::Handle<SshHandler>> {
    let mut proxy_settings = None;
    if let Ok(app_settings) = crate::config::load_app_settings(app) {
        if app_settings.proxy.enabled {
            proxy_settings = Some(app_settings.proxy);
        }
    }

    let target = (config.host.as_str(), config.port);
    let handle = if let Some(proxy) = proxy_settings {
        let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
        match proxy.protocol.as_str() {
            "socks5" => {
                let stream = tokio_socks::tcp::Socks5Stream::connect(proxy_addr.as_str(), target).await
                    .map_err(|e| AppError::Auth(format!("SOCKS5 proxy connection failed: {}", e)))?;
                client::connect_stream(ssh_config, stream.into_inner(), handler).await
            }
            "http" => {
                let mut stream = tokio::net::TcpStream::connect(&proxy_addr).await
                    .map_err(|e| AppError::Auth(format!("HTTP proxy connection failed: {}", e)))?;
                async_http_proxy::http_connect_tokio(&mut stream, &config.host, config.port).await
                    .map_err(|e| AppError::Auth(format!("HTTP proxy tunnel failed: {}", e)))?;
                client::connect_stream(ssh_config, stream, handler).await
            }
            _ => client::connect(ssh_config, target, handler).await,
        }
    } else {
        client::connect(ssh_config, target, handler).await
    }.map_err(|e| AppError::Auth(format!("SSH connection failed: {}", e)))?;

    Ok(handle)
}

/// Connects via SSH, opens a PTY shell, and spawns the I/O loop.
pub async fn create_ssh_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: SshConfig,
) -> AppResult<String> {
    tracing::info!(host = %config.host, port = config.port, user = %config.username, "Creating SSH session");
    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();

    let mut ssh_config_obj = client::Config {
        window_size: 32 * 1024 * 1024,
        maximum_packet_size: 32 * 1024,
        nodelay: true,
        ..Default::default()
    };
    if let Ok(app_settings) = crate::config::load_app_settings(&app) {
        let interval = app_settings.terminal.keep_alive_interval;
        if interval > 0 {
            ssh_config_obj.keepalive_interval = Some(std::time::Duration::from_secs(interval as u64));
        }
    }
    let ssh_config = Arc::new(ssh_config_obj);
    let handler = SshHandler::new(app.clone(), config.host.clone(), config.port);

    let mut handle = connect_with_proxy(&app, &config, ssh_config, handler).await?;

    match &config.auth {
        SshAuth::Password { password } => {
            let authenticated = handle
                .authenticate_password(&config.username, password)
                .await
                .map_err(|e| AppError::Auth(format!("Authentication failed: {}", e)))?;
            if !authenticated.success() {
                return Err(AppError::Auth(
                    "Authentication failed: invalid credentials".to_string(),
                ));
            }
        }
        SshAuth::Key {
            key_data,
            passphrase,
        } => {
            let key = russh::keys::decode_secret_key(key_data, passphrase.as_deref())?;
            let hash_alg = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
            let authenticated = handle
                .authenticate_publickey(&config.username, russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg))
                .await
                .map_err(|e| AppError::Auth(format!("Key authentication failed: {}", e)))?;
            if !authenticated.success() {
                return Err(AppError::Auth(
                    "Authentication failed: key rejected".to_string(),
                ));
            }
        }
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?;

    channel
        .request_pty(false, "xterm-256color", 80, 24, 0, 0, &[])
        .await
        .map_err(|e| AppError::Channel(format!("PTY request failed: {}", e)))?;

    channel
        .request_shell(false)
        .await
        .map_err(|e| AppError::Channel(format!("Shell request failed: {}", e)))?;

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::SSH,
        connected: true,
    };

    let ssh_config_arc: Arc<dyn std::any::Any + Send + Sync> = Arc::new(config.clone());
    let handle_arc = Arc::new(handle);
    let ssh_handle_arc: Arc<dyn std::any::Any + Send + Sync> = handle_arc.clone();

    let session_handle = SessionHandle {
        info: session_info,
        cmd_tx,
        ssh_config: Some(ssh_config_arc),
        ssh_handle: Some(ssh_handle_arc),
    };
    manager.add_session(session_handle).await;

    let sid = session_id.clone();
    let mgr = manager.clone();
    tokio::spawn(async move {
        ssh_io_loop(app, sid, mgr, channel, handle_arc, cmd_rx).await;
    });

    tracing::info!(session_id = %session_id, "SSH session created");
    Ok(session_id)
}

async fn ssh_io_loop(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut channel: russh::Channel<client::Msg>,
    _handle: Arc<client::Handle<SshHandler>>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let output_event = format!("terminal-output-{}", session_id);
    let closed_event = format!("session-closed-{}", session_id);

    let mut attached = false;
    let mut buffer: Vec<String> = Vec::new();

    loop {
        tokio::select! {
            biased;

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCommand::Attach) => {
                        attached = true;
                        for text in buffer.drain(..) {
                            let _ = app.emit(&output_event, &text);
                        }
                    }
                    Some(SessionCommand::Write(data)) => {
                        let _ = channel.data(&data[..]).await;
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        let _ = channel.window_change(cols, rows, 0, 0).await;
                    }
                    Some(SessionCommand::Close) | None => {
                        let _ = channel.close().await;
                        break;
                    }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { ref data }) => {
                        let text = String::from_utf8_lossy(data).to_string();
                        if attached {
                            let _ = app.emit(&output_event, &text);
                        } else {
                            buffer.push(text);
                        }
                    }
                    Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        let text = String::from_utf8_lossy(data).to_string();
                        if attached {
                            let _ = app.emit(&output_event, &text);
                        } else {
                            buffer.push(text);
                        }
                    }
                    Some(ChannelMsg::Eof) | None => break,
                    _ => {}
                }
            }
        }
    }

    manager.remove_session(&session_id).await;
    tracing::info!(session_id = %session_id, "SSH session closed");
    let _ = app.emit(&closed_event, ());
}
