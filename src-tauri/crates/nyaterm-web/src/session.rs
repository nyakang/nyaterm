use crate::{
    auth::Owner,
    error::{Result, WebError},
    state::State,
};
use axum::{
    Extension, Json,
    extract::{
        Path, State as ExtractState, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use nyaterm_core::{
    config,
    ssh::{
        algorithms, protocol,
        terminal::{self, Command, Output},
    },
    storage,
    utils::crypto,
};
use russh::{client, keys::PublicKeyBase64};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

pub struct Handler {
    state: Arc<State>,
    owner: String,
    host: String,
    port: u16,
    cancel: CancellationToken,
}
impl client::Handler for Handler {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let host_id = if self.port == 22 {
            self.host.clone()
        } else {
            format!("[{}]:{}", self.host, self.port)
        };
        let key_type = key.algorithm().to_string();
        let encoded = key.public_key_base64();
        let known = storage::check_known_host(&host_id, &key_type, &encoded)
            .map_err(|_| russh::Error::UnknownKey)?;
        if matches!(known, storage::KnownHostCheck::Match) {
            return Ok(true);
        }
        let reply = self
            .state
            .prompt(
                &self.owner,
                "host-key-verify",
                json!({
                    "host":self.host,"port":self.port,"keyType":key_type,
                    "fingerprint":key.fingerprint(russh::keys::HashAlg::Sha256).to_string(),
                    "isKeyChanged":matches!(known,storage::KnownHostCheck::HostSeen),
                }),
                &self.cancel,
            )
            .await
            .map_err(|_| russh::Error::UnknownKey)?;
        if reply.as_bool() != Some(true) {
            return Ok(false);
        }
        storage::replace_known_host_for_host(&host_id, &format!("{host_id} {key_type} {encoded}"))
            .map_err(|_| russh::Error::UnknownKey)?;
        Ok(true)
    }
}
pub struct WebSession {
    pub protocol: SessionProtocol,
    pub id: String,
    pub owner: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub connection_id: Option<String>,
    pub request_id: Option<String>,
    pub workspace_pane_id: Option<String>,
    pub cancel: CancellationToken,
    pub handle: Mutex<Option<client::Handle<Handler>>>,
    pub input: mpsc::Sender<Command>,
    pub output: Arc<Mutex<mpsc::Receiver<Output>>>,
    pub attached: AtomicBool,
    pub detached_at: StdMutex<Instant>,
    pub ready: AtomicBool,
    pub transfers: Arc<tokio::sync::Semaphore>,
    pub sftp: config::SftpSettings,
    pub stats: nyaterm_core::core::monitoring::stats::RemoteStatsSampler,
}
pub enum SessionProtocol {
    Ssh,
    Telnet,
    Vnc(Arc<crate::vnc::WebVnc>),
}
impl WebSession {
    pub fn info(&self) -> Value {
        let kind = match self.protocol {
            SessionProtocol::Ssh => "SSH",
            SessionProtocol::Telnet => "Telnet",
            SessionProtocol::Vnc(_) => "VNC",
        };
        let terminal = !matches!(self.protocol, SessionProtocol::Vnc(_));
        let sftp = matches!(self.protocol, SessionProtocol::Ssh)
            && self.sftp.enabled
            && self.ready.load(Ordering::Acquire);
        let connected = match &self.protocol {
            SessionProtocol::Vnc(web) => *web.state.lock().unwrap() == "active",
            _ => self.ready.load(Ordering::Acquire),
        } && !self.cancel.is_cancelled();
        let stats = matches!(self.protocol, SessionProtocol::Ssh) && connected;
        json!({"id":self.id,"name":self.name,"session_type":kind,"host":self.host,"port":self.port,"username":self.username,"connection_id":self.connection_id,"owner_window_label":"main","workspace_pane_id":self.workspace_pane_id,"attached":self.attached.load(Ordering::Acquire),"ready":self.ready.load(Ordering::Acquire),"connected":connected,"terminal_available":terminal,"shell_available":terminal,"sftp_available":sftp,"remote_file_browser_enabled":sftp,"remote_stats_enabled":stats,"injection_active":false,"runtime_mode":"standard","ssh_runtime_mode":"standard","dynamic_title_enabled":false,"dynamic_title_integration_active":false})
    }
}

pub(crate) struct SessionMetadata {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub name: String,
    pub connection_id: Option<String>,
    pub sftp: Option<config::SftpSettings>,
}
pub(crate) async fn register(
    state: &Arc<State>,
    owner: &str,
    args: &Value,
    protocol: SessionProtocol,
    metadata: SessionMetadata,
    cancel: CancellationToken,
) -> Result<(
    Arc<WebSession>,
    mpsc::Receiver<Command>,
    mpsc::Sender<Output>,
)> {
    state.login(owner).await?;
    crate::network::validate_target(&metadata.host, metadata.port)?;
    let (input, commands) = mpsc::channel(128);
    let (sender, output) = mpsc::channel(terminal::OUTPUT_CAPACITY);
    let session = Arc::new(WebSession {
        protocol,
        id: uuid::Uuid::new_v4().to_string(),
        owner: owner.into(),
        name: metadata.name,
        host: metadata.host,
        port: metadata.port,
        username: metadata.username,
        connection_id: metadata.connection_id,
        request_id: args["createRequestId"].as_str().map(String::from),
        workspace_pane_id: args["recordingScopeId"].as_str().map(String::from),
        cancel,
        handle: Mutex::new(None),
        input,
        output: Arc::new(Mutex::new(output)),
        attached: AtomicBool::new(false),
        detached_at: StdMutex::new(Instant::now()),
        ready: AtomicBool::new(false),
        transfers: Arc::new(tokio::sync::Semaphore::new(4)),
        sftp: metadata.sftp.unwrap_or(config::SftpSettings {
            enabled: false,
            ..Default::default()
        }),
        stats: Default::default(),
    });
    let mut registry = state.sessions.lock().await;
    if registry.len() >= 128 || registry.values().filter(|s| s.owner == owner).count() >= 16 {
        return Err(WebError::bad("Too many sessions"));
    }
    registry.insert(session.id.clone(), session.clone());
    Ok((session, commands, sender))
}
pub(crate) fn connection(args: &Value, kind: &str) -> Result<(config::SavedConnection, bool)> {
    if let Some(id) = args["connectionId"].as_str() {
        return Ok((config::load_connection_by_id(&(), id)?, true));
    }
    let mut value = if args["config"].is_object() {
        args["config"].clone()
    } else {
        args.clone()
    };
    value["type"] = json!(kind);
    value["id"] = json!("");
    if !value["name"].is_string() {
        value["name"] = json!(kind);
    }
    Ok((serde_json::from_value(value)?, false))
}

pub(crate) struct Target {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    name: String,
    mode: String,
    secret: Option<zeroize::Zeroizing<String>>,
    key: Option<String>,
    passphrase: Option<zeroize::Zeroizing<String>>,
    preferences: Option<config::SshAlgorithmPreferences>,
    connection_id: Option<String>,
    pub(crate) network: Option<config::ConnectionNetwork>,
    terminal_type: config::SshTerminalType,
    sftp: config::SftpSettings,
}
fn require_utf8(encoding: &str) -> Result<()> {
    let effective = if encoding.is_empty() || encoding.eq_ignore_ascii_case("global") {
        config::load_app_settings(&())?.interaction.default_encoding
    } else {
        encoding.to_owned()
    };
    if !matches!(effective.to_lowercase().as_str(), "utf-8" | "utf8") {
        return Err(WebError::bad("Web SSH requires UTF-8 terminal encoding"));
    }
    Ok(())
}
fn validate_sftp(settings: &config::SftpSettings) -> Result<()> {
    if settings.enabled {
        if settings.compatibility_mode {
            return Err(WebError::unsupported());
        }
        if !settings.filename_encoding.is_empty() {
            require_utf8(&settings.filename_encoding)?;
        }
    }
    Ok(())
}
pub(crate) fn target(args: &Value) -> Result<Target> {
    target_for(args, false)
}
pub(crate) fn jump_target(id: &str) -> Result<Target> {
    target_for(&json!({"connectionId": id}), true)
}
fn target_for(args: &Value, transport_only: bool) -> Result<Target> {
    if let Some(id) = args["connectionId"].as_str() {
        let conn = config::load_connection_by_id(&(), id)?;
        if conn.ssh_transport != config::SshTransport::Ssh {
            return Err(WebError::unsupported());
        }
        let config::ConnectionType::Ssh {
            host,
            port,
            username,
            x11_forwarding,
            agent_forwarding_config,
            encoding,
            ..
        } = &conn.config
        else {
            return Err(WebError::unsupported());
        };
        if !transport_only
            && (*x11_forwarding || agent_forwarding_config.as_ref().is_some_and(|c| c.enabled))
        {
            return Err(WebError::unsupported());
        }
        if !transport_only {
            require_utf8(encoding)?;
            validate_sftp(&conn.sftp)?;
        }
        if !transport_only
            && (conn.post_login.as_ref().is_some_and(|p| p.enabled)
                || conn.ssh_profile != config::SshProfile::Standard)
        {
            return Err(WebError::unsupported());
        }
        let terminal_type =
            config::resolve_ssh_terminal_type(&conn.ssh_profile, conn.terminal_type.as_ref());
        let account = conn
            .auth
            .as_ref()
            .map(|a| {
                config::load_saved_account(&(), a.account_id.as_deref(), a.password_id.as_deref())
            })
            .transpose()?
            .flatten();
        let username = config::resolve_account_username(account.as_ref(), username);
        let mut secret = None;
        let mut key = None;
        let mut passphrase = None;
        let mode = conn
            .auth
            .as_ref()
            .map(|a| a.mode.clone())
            .unwrap_or_else(|| "password".into());
        if let Some(auth) = &conn.auth {
            secret = match &auth.password {
                Some(ciphertext) => Some(crypto::decrypt(ciphertext)?),
                None if auth.password_source.as_deref() != Some("connection") => {
                    config::decrypt_account_password(account.as_ref())?
                }
                _ => None,
            };
            if let Some(key_id) = &auth.key_id {
                let saved = config::load_key_by_id(&(), key_id)?;
                if saved.cert.is_some() {
                    return Err(WebError::unsupported());
                }
                key = saved.key.as_deref().map(crypto::decrypt).transpose()?;
                passphrase = saved.passphrase;
            }
        }
        Ok(Target {
            host: host.clone(),
            port: *port,
            username,
            name: conn.name,
            mode,
            secret: secret.map(zeroize::Zeroizing::new),
            key,
            passphrase: passphrase.map(zeroize::Zeroizing::new),
            preferences: conn.ssh_algorithms,
            network: conn.network.clone(),
            connection_id: Some(id.into()),
            terminal_type,
            sftp: conn.sftp,
        })
    } else {
        let value = &args["config"];
        if !value["proxy"].is_null()
            || !value["proxy_jump"].is_null()
            || value["x11_forwarding"].as_bool() == Some(true)
        {
            return Err(WebError::unsupported());
        }
        if value["agent_forwarding_config"]["enabled"].as_bool() == Some(true)
            || !value["auth"]["cert_data"].is_null()
        {
            return Err(WebError::unsupported());
        }
        require_utf8(value["encoding"].as_str().unwrap_or(""))?;
        let sftp: config::SftpSettings = value
            .get("sftp")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or_default();
        validate_sftp(&sftp)?;
        if value["ssh_profile"]
            .as_str()
            .is_some_and(|p| p != "standard")
        {
            return Err(WebError::unsupported());
        }
        let terminal_type = value
            .get("terminal_type")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or(config::SshTerminalType::Xterm256Color);
        let port = match value["port"].as_u64() {
            Some(p) => u16::try_from(p)
                .ok()
                .filter(|p| *p > 0)
                .ok_or(WebError::bad("Invalid SSH port"))?,
            None if value["port"].is_null() => 22,
            _ => return Err(WebError::bad("Invalid SSH port")),
        };
        Ok(Target {
            host: value["host"]
                .as_str()
                .ok_or(WebError::bad("SSH host required"))?
                .into(),
            port,
            username: value["username"].as_str().unwrap_or("root").into(),
            name: value["name"].as_str().unwrap_or("SSH").into(),
            mode: value["auth"]["type"].as_str().unwrap_or("password").into(),
            secret: value["auth"]["password"]
                .as_str()
                .map(|v| zeroize::Zeroizing::new(v.into())),
            key: value["auth"]["key_data"].as_str().map(String::from),
            passphrase: value["auth"]["passphrase"]
                .as_str()
                .map(|v| zeroize::Zeroizing::new(v.into())),
            preferences: value
                .get("ssh_algorithms")
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?,
            network: value
                .get("network")
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?,
            connection_id: None,
            terminal_type,
            sftp,
        })
    }
}
pub async fn create(state: Arc<State>, owner: &str, args: Value) -> Result<String> {
    match args["type"].as_str().unwrap_or("ssh") {
        "telnet" => return crate::telnet::create(state, owner, args).await,
        "vnc" => return crate::vnc::create(state, owner, args).await,
        "ssh" => {}
        _ => return Err(WebError::unsupported()),
    }
    if args["startupCommand"].as_object().is_some()
        || args["runtimeMode"]
            .as_str()
            .is_some_and(|m| m != "standard")
        || args["config"]["post_login"]["enabled"].as_bool() == Some(true)
    {
        return Err(WebError::unsupported());
    }
    let target = target(&args)?;
    if !matches!(target.mode.as_str(), "none" | "password" | "key") {
        return Err(WebError::unsupported());
    }
    if target.host.is_empty()
        || target.host.len() > 253
        || target
            .host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        || target.port == 0
        || target.username.is_empty()
        || target.username.len() > 256
        || target.username.contains('\0')
    {
        return Err(WebError::bad("Invalid SSH target"));
    }
    let login = state.login(owner).await?;
    let metadata = SessionMetadata {
        host: target.host.clone(),
        port: target.port,
        username: target.username.clone(),
        name: target.name.clone(),
        connection_id: target.connection_id.clone(),
        sftp: Some(target.sftp.clone()),
    };
    let (session, commands, sender) = register(
        &state,
        owner,
        &args,
        SessionProtocol::Ssh,
        metadata,
        login.cancel.child_token(),
    )
    .await?;
    let id = session.id.clone();
    crate::observability::spawn(async move {
        let result = tokio::select! {
            _ = session.cancel.cancelled() => Err(WebError::bad("Session creation cancelled")),
            result = tokio::time::timeout(Duration::from_secs(120), connect(&state,&session,target)) => result.unwrap_or(Err(WebError::bad("SSH connection timed out"))),
        };
        match result {
            Ok(channel) => {
                tracing::info!(event="session.connected", session_id=%session.id, "SSH session connected");
                session.ready.store(true, Ordering::Release);
                state
                    .event(&session.owner, "sessions-changed", Value::Null)
                    .await;
                terminal::run(channel, commands, sender, session.cancel.clone()).await;
            }
            Err(error) => {
                tracing::warn!(event="session.connect_failed", session_id=%session.id, reason=crate::observability::error_reason(&error.1), "SSH connection failed");
                state
                    .event(
                        &session.owner,
                        &format!("connection-error-{}", session.id),
                        json!(error.1),
                    )
                    .await;
                let _ = sender.try_send(Output::Failure(error.1));
            }
        }
        finish(&state, &session).await;
    });
    Ok(id)
}
pub(crate) async fn finish(state: &State, session: &WebSession) {
    tracing::info!(event="session.closed", session_id=%session.id, "Web session closed");
    session.cancel.cancel();
    // Let cancelled transfers remove their temporary files while SSH is still alive.
    let _transfers = tokio::time::timeout(
        Duration::from_secs(6),
        session.transfers.clone().acquire_many_owned(4),
    )
    .await;
    if let Some(handle) = session.handle.lock().await.take() {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            handle.disconnect(russh::Disconnect::ByApplication, "", ""),
        )
        .await;
    }
    state.sessions.lock().await.remove(&session.id);
    if !session.attached.load(Ordering::Acquire) {
        state
            .event(
                &session.owner,
                &format!("session-closed-{}", session.id),
                Value::Null,
            )
            .await;
    }
    state
        .event(&session.owner, "sessions-changed", Value::Null)
        .await;
}
async fn connect(
    state: &Arc<State>,
    session: &Arc<WebSession>,
    target: Target,
) -> Result<russh::Channel<client::Msg>> {
    let stream = crate::network::open(
        state.clone(),
        session.owner.clone(),
        session.cancel.clone(),
        target.host.clone(),
        target.port,
        target.network.clone(),
    )
    .await?;
    let terminal = target.terminal_type.as_str().to_owned();
    let connection_id = target.connection_id.clone();
    let mut handle = authenticate(
        state,
        &session.owner,
        &session.cancel,
        target,
        stream.stream,
    )
    .await?;
    let channel = protocol::open_shell_with_terminal(&mut handle, 80, 24, &terminal).await?;
    *session.handle.lock().await = Some(handle);
    if let Some(id) = connection_id {
        storage::mark_connection_used(&id)?;
    }
    Ok(channel)
}
pub(crate) async fn authenticate(
    state: &Arc<State>,
    owner: &str,
    cancel: &CancellationToken,
    mut target: Target,
    stream: nyaterm_core::network::BoxedTransportStream,
) -> Result<client::Handle<Handler>> {
    if !matches!(target.mode.as_str(), "none" | "password" | "key") {
        return Err(WebError::bad("Unsupported SSH authentication mode"));
    }
    let client_config = client::Config {
        preferred: algorithms::resolve_preferred_algorithms(target.preferences.as_ref())?,
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    };
    let handler = Handler {
        state: state.clone(),
        owner: owner.into(),
        host: target.host.clone(),
        port: target.port,
        cancel: cancel.clone(),
    };
    let mut handle = client::connect_stream(Arc::new(client_config), stream, handler).await?;
    let mut authenticated = false;
    if target.mode == "none" {
        authenticated = handle.authenticate_none(&target.username).await?.success();
    }
    if target.mode == "key" {
        let key_data = zeroize::Zeroizing::new(
            target
                .key
                .take()
                .ok_or(WebError::bad("Private key required"))?,
        );
        let mut decoded = russh::keys::decode_secret_key(
            &key_data,
            target.passphrase.as_deref().map(|p| p.as_str()),
        );
        if decoded.is_err() && target.passphrase.is_none() {
            let response=state.prompt(owner,"ssh-auth-request",json!({"connectionId":target.connection_id,"connectionName":target.name,"host":target.host,"port":target.port,"username":target.username,"reason":"key_passphrase_required","promptKind":"passphrase","availableMethods":["publickey"],"currentAuthMode":"key","attempt":1,"canSave":false}),cancel).await?;
            target.passphrase = response["secret"]
                .as_str()
                .map(|s| zeroize::Zeroizing::new(s.into()));
            decoded = russh::keys::decode_secret_key(
                &key_data,
                target.passphrase.as_deref().map(|p| p.as_str()),
            );
        }
        let key = decoded.map_err(|_| WebError::bad("Private key could not be unlocked"))?;
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        authenticated = handle
            .authenticate_publickey(
                &target.username,
                russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await?
            .success();
    }
    if !authenticated && target.mode != "key" {
        for attempt in 1..=3 {
            if target.secret.is_none() {
                let response = state.prompt(owner,"ssh-auth-request",json!({
                    "connectionId":target.connection_id,"connectionName":target.name,"host":target.host,"port":target.port,"username":target.username,
                    "reason":if attempt == 1 {"missing_password"} else {"password_rejected"},"promptKind":"password","availableMethods":["password"],"currentAuthMode":"password","attempt":attempt,"canSave":false,"accountId":null,
                }),cancel).await?;
                target.secret = response["secret"]
                    .as_str()
                    .map(|s| zeroize::Zeroizing::new(s.into()));
            }
            let Some(secret) = &target.secret else {
                return Err(WebError::bad("SSH authentication cancelled"));
            };
            let result = protocol::password(&mut handle, &target.username, secret).await?;
            if result.success() {
                authenticated = true;
                break;
            }
            if let russh::client::AuthResult::Failure {
                remaining_methods, ..
            } = result
            {
                if remaining_methods.contains(&russh::MethodKind::KeyboardInteractive) {
                    authenticated =
                        keyboard_auth(state, owner, cancel, &mut handle, &target).await?;
                    break;
                }
            }
            target.secret = None;
        }
    }
    if !authenticated && target.mode == "key" {
        authenticated = keyboard_auth(state, owner, cancel, &mut handle, &target).await?;
    }
    if !authenticated {
        return Err(WebError::bad("SSH authentication failed"));
    }
    Ok(handle)
}
async fn keyboard_auth(
    state: &Arc<State>,
    owner: &str,
    cancel: &CancellationToken,
    handle: &mut client::Handle<Handler>,
    target: &Target,
) -> Result<bool> {
    let mut step = protocol::keyboard_start(handle, &target.username).await?;
    for round in 1..=8 {
        match step {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(true),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            client::KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                if prompts.len() > 16 {
                    return Err(WebError::bad("Too many authentication prompts"));
                }
                let response=state.prompt(owner,"otp-request",json!({"connectionName":target.name,"name":name,"instructions":instructions,"round":round,"prompts":prompts.iter().map(|p|json!({"prompt":p.prompt,"echo":p.echo})).collect::<Vec<_>>(),"otpEntryId":null}),cancel).await?;
                let responses: Vec<String> = serde_json::from_value(response)?;
                if responses.len() != prompts.len() {
                    return Err(WebError::bad("Invalid authentication response"));
                }
                step = protocol::keyboard_respond(handle, responses).await?;
            }
        }
    }
    Err(WebError::bad("Too many authentication rounds"))
}
pub async fn create_route(
    ExtractState(state): ExtractState<Arc<State>>,
    Extension(owner): Extension<Owner>,
    Json(args): Json<Value>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"session_id":create(state,&owner.0,args).await?}),
    ))
}
pub async fn ws_route(
    ExtractState(state): ExtractState<Arc<State>>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let session = state.session(&owner.0, &id).await?;
    if matches!(session.protocol, SessionProtocol::Vnc(_)) {
        return Err(WebError::bad("This session has no terminal"));
    }
    let receiver = session.output.clone().try_lock_owned().map_err(|_| {
        WebError(
            axum::http::StatusCode::CONFLICT,
            "Terminal already attached".into(),
        )
    })?;
    Ok(upgrade
        .max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .max_write_buffer_size(256 * 1024)
         .on_upgrade({
            let span = tracing::Span::current();
            let guard = crate::observability::StreamLogGuard::new("terminal.disconnected", Some(session.id.clone()));
            move |socket| async move {
                let _guard = guard;
                tracing::info!(event="terminal.attached", session_id=%session.id, "Web stream attached");
                socket_loop(session, receiver, socket).await;
            }.instrument(span)
        }))
}
struct Attachment(Arc<WebSession>);
impl Drop for Attachment {
    fn drop(&mut self) {
        *self.0.detached_at.lock().unwrap() = Instant::now();
        self.0.attached.store(false, Ordering::Release);
    }
}
async fn socket_loop(
    session: Arc<WebSession>,
    mut output: OwnedMutexGuard<mpsc::Receiver<Output>>,
    mut socket: WebSocket,
) {
    session.attached.store(true, Ordering::Release);
    let _attachment = Attachment(session.clone());
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last_peer = Instant::now();
    loop {
        tokio::select! {
            // Read control frames and heartbeats even when output is always ready.
            // Keep buffered terminal output ahead of backend cancellation so the
            // final bytes are delivered before the closed frame.
            biased;
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    last_peer = Instant::now();
                    let Ok(value) = serde_json::from_str::<Value>(&text) else { break; };
                    let command = match value["type"].as_str() {
                        Some("input") => value["data"].as_str().map(|s| Command::Input(s.as_bytes().to_vec())),
                        Some("resize") => value["cols"].as_u64().zip(value["rows"].as_u64()).filter(|(c,r)| (1..=1000).contains(c)&&(1..=1000).contains(r)).map(|(c,r)| Command::Resize(c as u32,r as u32)),
                        Some("close") => { session.cancel.cancel(); break; },
                        _ => None,
                    };
                    let Some(command) = command else { break; };
                    if session.input.try_send(command).is_err() { break; }
                },
                Some(Ok(Message::Binary(bytes))) => { last_peer=Instant::now(); if session.input.try_send(Command::RawInput(bytes.to_vec())).is_err() { break; } },
                Some(Ok(Message::Pong(_))) => last_peer=Instant::now(),
                Some(Ok(Message::Ping(data))) => { if !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Pong(data))).await,Ok(Ok(()))) { break; } },
                _ => break,
            },
            _ = heartbeat.tick() => {
                if last_peer.elapsed() > Duration::from_secs(45) { break; }
                if !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Ping(Vec::new().into()))).await,Ok(Ok(()))) { break; }
            },
            next = output.recv() => {
                let final_frame = !matches!(&next, Some(Output::Data(_)));
                let message = match next {
                    Some(Output::Data(data)) => Message::Binary(data.into()),
                    Some(Output::Error) => Message::Text(json!({"type":"error","error":if matches!(session.protocol, SessionProtocol::Telnet) { "Telnet connection failed" } else { "SSH connection failed" }}).to_string().into()),
                    Some(Output::Failure(error)) => Message::Text(json!({"type":"error","error":error}).to_string().into()),
                    _ => Message::Text(json!({"type":"closed"}).to_string().into()),
                };
                // A failed attachment leaves the remote alive for its reconnect lease.
                if !matches!(tokio::time::timeout(Duration::from_secs(10),socket.send(message)).await,Ok(Ok(()))) { break; }
                if final_frame { break; }
            },
            _ = session.cancel.cancelled() => { let _ = tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Text(json!({"type":"closed"}).to_string().into()))).await; break; },
        }
    }
    // Complete the WebSocket close handshake before dropping the TCP stream.
    // Dropping with an unread pong/input can reset the connection on Windows,
    // discarding the last terminal output even though send() already succeeded.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        if socket.send(Message::Close(None)).await.is_ok() {
            while let Some(Ok(message)) = socket.recv().await {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        }
    })
    .await;
}
