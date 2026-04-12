use crate::error::{AppError, AppResult};
use russh::client;
use russh::keys::PublicKeyBase64;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

/// Connection parameters for SSH (host, port, user, auth method).
#[derive(Debug, Clone, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub connection_id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    #[serde(default)]
    pub proxy: Option<crate::config::ProxySettings>,
    #[serde(default)]
    pub proxy_jump: Option<Box<SshConfig>>,
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

pub(crate) type SshRawHandle = Arc<Mutex<client::Handle<SshHandler>>>;

pub struct SshConnectionHandles {
    target: SshRawHandle,
    jump: Option<SshRawHandle>,
}

impl SshConnectionHandles {
    pub fn new(target: SshRawHandle, jump: Option<SshRawHandle>) -> Self {
        Self { target, jump }
    }

    pub fn target_handle(&self) -> SshRawHandle {
        self.target.clone()
    }

    #[allow(dead_code)]
    pub fn jump_handle(&self) -> Option<SshRawHandle> {
        self.jump.clone()
    }
}

pub(crate) type SshHandle = Arc<SshConnectionHandles>;

/// russh client handler; performs TOFU known_hosts verification.
pub struct SshHandler {
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

        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            if let Err(error) = writeln!(file, "{}", host_entry) {
                let _ = self.app.emit(
                    "ssh-error",
                    format!("Failed to save known_hosts: {}", error),
                );
                return Ok(false);
            }
        }

        Ok(true)
    }
}

pub(super) fn build_client_config(app: &AppHandle) -> client::Config {
    let mut client_cfg = client::Config {
        window_size: 32 * 1024 * 1024,
        maximum_packet_size: 32 * 1024,
        nodelay: true,
        inactivity_timeout: None,
        keepalive_max: 3,
        ..Default::default()
    };

    if let Ok(app_settings) = crate::config::load_app_settings(app) {
        let interval = app_settings.terminal.keep_alive_interval;
        if interval > 0 {
            client_cfg.keepalive_interval = Some(std::time::Duration::from_secs(interval as u64));
        }
    }

    client_cfg
}

pub(super) async fn connect_with_proxy(
    config: &SshConfig,
    ssh_config: Arc<client::Config>,
    handler: SshHandler,
) -> AppResult<client::Handle<SshHandler>> {
    let target = (config.host.as_str(), config.port);
    let handle = if let Some(proxy) = config.proxy.clone().filter(|proxy| proxy.enabled) {
        let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
        match proxy.protocol.as_str() {
            "socks5" => {
                let stream = match (&proxy.username, &proxy.password) {
                    (Some(user), Some(pass)) => {
                        tokio_socks::tcp::Socks5Stream::connect_with_password(
                            proxy_addr.as_str(),
                            target,
                            user,
                            pass,
                        )
                        .await
                    }
                    _ => tokio_socks::tcp::Socks5Stream::connect(proxy_addr.as_str(), target).await,
                }
                .map_err(|error| {
                    AppError::Auth(format!("SOCKS5 proxy connection failed: {}", error))
                })?;
                client::connect_stream(ssh_config, stream.into_inner(), handler).await
            }
            "http" => {
                let mut stream =
                    tokio::net::TcpStream::connect(&proxy_addr)
                        .await
                        .map_err(|error| {
                            AppError::Auth(format!("HTTP proxy connection failed: {}", error))
                        })?;

                match (&proxy.username, &proxy.password) {
                    (Some(user), Some(pass)) => {
                        async_http_proxy::http_connect_tokio_with_basic_auth(
                            &mut stream,
                            &config.host,
                            config.port,
                            user,
                            pass,
                        )
                        .await
                    }
                    _ => {
                        async_http_proxy::http_connect_tokio(&mut stream, &config.host, config.port)
                            .await
                    }
                }
                .map_err(|error| AppError::Auth(format!("HTTP proxy tunnel failed: {}", error)))?;

                client::connect_stream(ssh_config, stream, handler).await
            }
            _ => client::connect(ssh_config, target, handler).await,
        }
    } else {
        client::connect(ssh_config, target, handler).await
    }
    .map_err(|error| AppError::Auth(format!("SSH connection failed: {}", error)))?;

    Ok(handle)
}

pub(super) async fn connect_via_stream<S>(
    stream: S,
    ssh_config: Arc<client::Config>,
    handler: SshHandler,
) -> AppResult<client::Handle<SshHandler>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    client::connect_stream(ssh_config, stream, handler)
        .await
        .map_err(|error| AppError::Auth(format!("SSH connection failed: {}", error)))
}
