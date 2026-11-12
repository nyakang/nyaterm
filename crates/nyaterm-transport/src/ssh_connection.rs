//! SSH authentication, connection routing, and channel ownership.
use crate::ssh_algorithms::resolve_preferred_algorithms;
use crate::ssh_auth::{authenticate_ssh, is_agent_retry};
use crate::x11::X11ChannelOpen;
use crate::{
    ShellEnvironmentCache, SshAgentForwardingConfig, SshAgentForwardingPolicy,
    SshAgentForwardingSources, SshHostKey, SshHostKeyDecision, SshSessionConfig, SshTunnelConfig,
    connection_attempt, ssh_agent, ssh_agent_broker,
};
use crate::{SshAgentStoredKeyProvider, SshHostKeyVerifier, SshProxyConfig};
use russh::keys::PublicKeyBase64;
use russh::{Disconnect, client};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshMultiplexInfo {
    pub name: String,
    pub host: String,
    pub host_key_alias: Option<String>,
    pub port: u16,
    pub username: String,
    pub proxy: Option<SshProxyConfig>,
    pub jump_count: usize,
}

#[derive(Clone)]
pub struct SshMultiplexHandle {
    pub(crate) inner: Arc<SshMultiplexInner>,
}

pub(crate) type SharedSshHandle = Arc<tokio::sync::Mutex<client::Handle<SshClientHandler>>>;

pub(crate) type ForwardedTcpIpRegistry = Arc<tokio::sync::Mutex<ForwardedTcpIpDispatch>>;

pub(crate) type X11Registry = Arc<tokio::sync::Mutex<Option<X11Registration>>>;

pub(crate) struct SshMultiplexInner {
    pub(crate) attempt: connection_attempt::ConnectionAttempt,
    pub(crate) runtime: Arc<tokio::runtime::Runtime>,
    pub(crate) target: SharedSshHandle,
    pub(crate) jumps: Vec<SharedSshHandle>,
    pub(crate) info: SshMultiplexInfo,
    /// The Agent handler is fixed when the transport is created, so multiplex
    /// handles with different forwarding policies must never be shared.
    pub(crate) agent_forwarding_config: Option<SshAgentForwardingConfig>,
    pub(crate) agent_stored_key_revision: Option<u64>,
    pub(crate) forwarded_tcpip: ForwardedTcpIpRegistry,
    pub(crate) x11: X11Registry,
    pub(crate) primary_session: Arc<PrimarySessionGate>,
    pub(crate) closed: AtomicBool,
}

#[derive(Default)]
pub(crate) struct ForwardedTcpIpDispatch {
    pub(crate) fallback: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
    pub(crate) by_listener:
        HashMap<(String, u32), tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
}

pub(crate) struct X11Registration {
    pub(crate) session_id: String,
    pub(crate) tx: tokio_mpsc::UnboundedSender<X11ChannelOpen>,
}

/// How long an auxiliary channel waits for the interactive shell to take the
/// connection's first session channel before opening anyway.
///
/// `sshd` hands the PAM login message (`pam_motd`, i.e. the whole
/// `/etc/update-motd.d` banner) to the first session channel that runs `do_exec`
/// and clears its buffer afterwards, so only one channel per connection can ever
/// print it. Before multiplexing the terminal owned its connection and always
/// won that slot; now Stats/Docker/GPU/SFTP share the connection and would
/// silently eat the banner if one of them opened a channel first.
pub(crate) const PRIMARY_SESSION_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Holds auxiliary channels back until the interactive shell has requested its
/// channel on a freshly authenticated connection.
pub(crate) struct PrimarySessionGate {
    pub(crate) claimed: AtomicBool,
    /// A zero-permit semaphore, used only for its closed state: closing releases
    /// every waiter, including the ones that start waiting afterwards.
    pub(crate) opened: tokio::sync::Semaphore,
}

/// Released once the interactive shell holds its channel -- or has given up --
/// which lets the queued auxiliary channels through.
pub(crate) struct PrimarySessionClaim {
    pub(crate) gate: Arc<PrimarySessionGate>,
}

pub(crate) struct ForwardedTcpIpChannel {
    pub(crate) channel: russh::Channel<client::Msg>,
    pub(crate) connected_address: String,
    pub(crate) connected_port: u32,
    pub(crate) originator_address: String,
    pub(crate) originator_port: u32,
}

pub(crate) type SshHandleChain = (
    client::Handle<SshClientHandler>,
    Vec<client::Handle<SshClientHandler>>,
);

pub(crate) struct SshClientHandler {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) host_key_alias: Option<String>,
    pub(crate) verifier: Option<Arc<dyn SshHostKeyVerifier>>,
    pub(crate) forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    pub(crate) x11: Option<X11Registry>,
    pub(crate) agent_forwarding_config: Option<SshAgentForwardingConfig>,
    pub(crate) agent_stored_key_provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    pub(crate) shell_environment: Arc<ShellEnvironmentCache>,
}

impl PrimarySessionGate {
    pub(crate) fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            opened: tokio::sync::Semaphore::new(0),
        }
    }

    /// Take the first-channel slot, or `None` if it is already spoken for.
    ///
    /// Only the connection's first interactive session can claim it; a second
    /// terminal on the same multiplex handle gets `None` and proceeds at once,
    /// which matches OpenSSH's own `ControlMaster` behaviour of printing the
    /// banner once per connection.
    pub(crate) fn claim(gate: &Arc<Self>) -> Option<PrimarySessionClaim> {
        gate.claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then(|| PrimarySessionClaim { gate: gate.clone() })
    }

    pub(crate) async fn wait(&self) {
        if !self.claimed.load(Ordering::SeqCst) {
            return;
        }
        // Permits are never added, so this only resolves once the claim is
        // dropped and closes the semaphore. The timeout keeps a wedged shell
        // from stalling every other feature on the connection forever.
        let _ = tokio::time::timeout(PRIMARY_SESSION_WAIT_TIMEOUT, self.opened.acquire()).await;
    }
}

impl Drop for PrimarySessionClaim {
    fn drop(&mut self) {
        self.gate.opened.close();
    }
}

impl std::fmt::Debug for SshMultiplexHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshMultiplexHandle")
            .field("info", &self.inner.info)
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl SshMultiplexHandle {
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
    /// Runtime for auxiliary tasks; the connection remains the authoritative owner.
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.inner.runtime.handle().clone()
    }

    pub fn shell_availability(&self) -> connection_attempt::ShellAvailability {
        self.inner.attempt.shell_availability()
    }

    pub fn info(&self) -> SshMultiplexInfo {
        self.inner.info.clone()
    }

    pub fn jump_count(&self) -> usize {
        self.inner.info.jump_count
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
            || self
                .inner
                .target
                .try_lock()
                .is_ok_and(|handle| handle.is_closed())
    }

    pub fn matches_config(&self, config: &SshSessionConfig) -> bool {
        self.inner.info.host == config.host
            && self.inner.info.host_key_alias.as_deref()
                == config
                    .host_key_alias
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            && self.inner.info.port == config.port
            && self.inner.info.username == config.username
            && self.inner.info.proxy == config.proxy
            && self.inner.agent_forwarding_config == effective_agent_forwarding_config(config)
            && self.inner.agent_stored_key_revision == current_agent_stored_key_revision(config)
    }

    pub fn ensure_matches_config(&self, config: &SshSessionConfig) -> anyhow::Result<()> {
        if self.matches_config(config) {
            return Ok(());
        }
        let info = &self.inner.info;
        anyhow::bail!(
            "SSH multiplex handle targets {}@{}:{}, but operation targets {}@{}:{}",
            info.username,
            info.host,
            info.port,
            config.username,
            config.host,
            config.port
        )
    }

    pub fn disconnect(&self) -> anyhow::Result<()> {
        if self.inner.closed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let target = self.inner.target.clone();
        let jumps = self.inner.jumps.clone();
        self.inner.runtime.block_on(async move {
            let _ = target
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "ssh multiplex closed", "en")
                .await;
            for jump in jumps {
                let _ = jump
                    .lock()
                    .await
                    .disconnect(Disconnect::ByApplication, "ssh multiplex closed", "en")
                    .await;
            }
            Ok(())
        })
    }

    /// The shared connection, for channels whose ordering does not matter.
    ///
    /// Only two callers qualify: the interactive terminal, which is the channel
    /// everything else is ordered behind, and `direct-tcpip` forwards, which
    /// never make the server run `do_exec`. Anything that will `exec` a command
    /// or start a subsystem must use [`Self::exec_target_handle`] instead.
    pub(crate) fn target_handle(&self) -> SharedSshHandle {
        self.inner.target.clone()
    }

    /// The shared connection, for a channel that will `exec` or start a
    /// subsystem, once the interactive shell has taken its own channel.
    ///
    /// `sshd` hands the PAM login banner to whichever session channel runs
    /// `do_exec` first and clears its buffer afterwards, so a Stats, Docker or
    /// SFTP channel that opens ahead of the terminal costs the user their whole
    /// MOTD. Waiting here is what keeps the terminal first.
    pub(crate) async fn exec_target_handle(&self) -> SharedSshHandle {
        self.inner.primary_session.wait().await;
        self.inner.target.clone()
    }

    pub(crate) fn forwarded_tcpip_registry(&self) -> ForwardedTcpIpRegistry {
        self.inner.forwarded_tcpip.clone()
    }

    /// Reserve the connection's first session channel for an interactive shell.
    pub(crate) fn claim_primary_session(&self) -> Option<PrimarySessionClaim> {
        PrimarySessionGate::claim(&self.inner.primary_session)
    }

    pub(crate) async fn register_x11_sender(
        &self,
        session_id: &str,
        tx: tokio_mpsc::UnboundedSender<X11ChannelOpen>,
    ) -> anyhow::Result<()> {
        if self.is_closed() {
            anyhow::bail!("SSH multiplex handle is closed");
        }
        register_x11_sender(&self.inner.x11, session_id, tx).await
    }

    pub(crate) async fn unregister_x11_sender(&self, session_id: &str) {
        unregister_x11_sender(&self.inner.x11, session_id).await;
    }

    pub fn block_on<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        if self.is_closed() {
            anyhow::bail!("SSH multiplex handle is closed");
        }
        self.inner.runtime.block_on(operation)
    }
}

pub(crate) async fn register_x11_sender(
    registry: &X11Registry,
    session_id: &str,
    tx: tokio_mpsc::UnboundedSender<X11ChannelOpen>,
) -> anyhow::Result<()> {
    let mut registration = registry.lock().await;
    if registration
        .as_ref()
        .is_some_and(|registration| registration.tx.is_closed())
    {
        *registration = None;
    }
    if let Some(registration) = registration.as_ref() {
        if registration.session_id == session_id {
            anyhow::bail!("X11 forwarding is already active for this multiplexed SSH session");
        }
        anyhow::bail!("X11 forwarding is already active for another multiplexed SSH session");
    }
    *registration = Some(X11Registration {
        session_id: session_id.to_string(),
        tx,
    });
    Ok(())
}

pub(crate) async fn unregister_x11_sender(registry: &X11Registry, session_id: &str) {
    let mut registration = registry.lock().await;
    if registration
        .as_ref()
        .is_some_and(|registration| registration.session_id == session_id)
    {
        *registration = None;
    }
}

pub(crate) fn forwarded_tcpip_sender_for(
    dispatch: &ForwardedTcpIpDispatch,
    connected_address: &str,
    connected_port: u32,
) -> Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>> {
    dispatch
        .by_listener
        .get(&(connected_address.to_string(), connected_port))
        .or(dispatch.fallback.as_ref())
        .cloned()
}

pub fn open_ssh_multiplex_handle(
    mut config: SshSessionConfig,
) -> anyhow::Result<SshMultiplexHandle> {
    config.apply_runtime_policy().map_err(anyhow::Error::msg)?;
    let agent_forwarding_config = effective_agent_forwarding_config(&config);
    let agent_stored_key_revision = current_agent_stored_key_revision(&config);
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("nyaterm-ssh-multiplex")
            .build()
            .map_err(|error| anyhow::anyhow!("failed to start SSH multiplex runtime: {error}"))?,
    );
    let forwarded_tcpip = Arc::new(tokio::sync::Mutex::new(ForwardedTcpIpDispatch::default()));
    let x11 = Arc::new(tokio::sync::Mutex::new(None));
    let shell_environment = ShellEnvironmentCache::global();
    let (target, jumps) = runtime.block_on(open_authenticated_ssh_handle_with_sender_registry(
        &config,
        Some(forwarded_tcpip.clone()),
        Some(x11.clone()),
        shell_environment,
    ))?;
    let info = SshMultiplexInfo {
        name: config.name,
        host: config.host,
        host_key_alias: config
            .host_key_alias
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        port: config.port,
        username: config.username,
        proxy: config.proxy,
        jump_count: jumps.len(),
    };
    Ok(SshMultiplexHandle {
        inner: Arc::new(SshMultiplexInner {
            attempt: config.attempt.clone(),
            runtime,
            target: Arc::new(tokio::sync::Mutex::new(target)),
            jumps: jumps
                .into_iter()
                .map(|jump| Arc::new(tokio::sync::Mutex::new(jump)))
                .collect(),
            info,
            agent_forwarding_config,
            agent_stored_key_revision,
            forwarded_tcpip,
            x11,
            primary_session: Arc::new(PrimarySessionGate::new()),
            closed: AtomicBool::new(false),
        }),
    })
}

impl std::fmt::Debug for SshTunnelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshTunnelConfig")
            .field("id", &self.id)
            .field("ssh_config", &self.ssh_config)
            .field("mode", &self.mode)
            .field("bind_host", &self.bind_host)
            .field("listen_port", &self.listen_port)
            .field("target_host", &self.target_host)
            .field("target_port", &self.target_port)
            .finish()
    }
}

pub(crate) fn ssh_client_config(
    config: &SshSessionConfig,
) -> anyhow::Result<Arc<russh::client::Config>> {
    let keepalive_interval = if config.keep_alive_interval_secs == 0
        || config.keep_alive_mode
            == nyaterm_core::terminal::connection_input::KeepaliveMode::Disabled
    {
        None
    } else {
        Some(Duration::from_secs(u64::from(
            config.keep_alive_interval_secs,
        )))
    };
    let preferred = resolve_preferred_algorithms(config.ssh_algorithms.as_ref())?;
    Ok(Arc::new(russh::client::Config {
        window_size: 32 * 1024 * 1024,
        maximum_packet_size: 32 * 1024,
        nodelay: true,
        inactivity_timeout: None,
        keepalive_interval,
        keepalive_max: 3,
        keepalive_mode: match config.keep_alive_mode {
            nyaterm_core::terminal::connection_input::KeepaliveMode::Strict => {
                russh::client::KeepaliveMode::Strict
            }
            _ => russh::client::KeepaliveMode::Compatible,
        },
        preferred,
        ..Default::default()
    }))
}

pub(crate) fn open_authenticated_ssh_handle(
    config: &SshSessionConfig,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_channel_senders(
        config,
        None,
        None,
        ShellEnvironmentCache::global(),
    )
}

pub(crate) fn open_authenticated_ssh_handle_with_forwarded_tx(
    config: &SshSessionConfig,
    forwarded_tcpip_tx: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_channel_senders(
        config,
        forwarded_tcpip_tx,
        None,
        ShellEnvironmentCache::global(),
    )
}

pub(crate) fn open_authenticated_ssh_handle_with_channel_senders(
    config: &SshSessionConfig,
    forwarded_tcpip_tx: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
    x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    let forwarded_tcpip = forwarded_tcpip_tx.map(|tx| {
        Arc::new(tokio::sync::Mutex::new(ForwardedTcpIpDispatch {
            fallback: Some(tx),
            by_listener: HashMap::new(),
        }))
    });
    let x11 = x11_tx.map(|tx| {
        Arc::new(tokio::sync::Mutex::new(Some(X11Registration {
            session_id: String::new(),
            tx,
        })))
    });
    open_authenticated_ssh_handle_with_sender_registry(
        config,
        forwarded_tcpip,
        x11,
        shell_environment,
    )
}

pub(crate) fn open_authenticated_ssh_handle_with_sender_registry(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11: Option<X11Registry>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    Box::pin(async move {
        config
            .attempt
            .until_cancelled(async {
                const MAX_AGENT_ATTEMPTS: u32 = 3;
                let mut agent_attempt = 1;
                loop {
                    match open_authenticated_ssh_handle_once(
                        config,
                        forwarded_tcpip.clone(),
                        x11.clone(),
                        Arc::clone(&shell_environment),
                        agent_attempt,
                    )
                    .await
                    {
                        Err(error)
                            if is_agent_retry(&error) && agent_attempt < MAX_AGENT_ATTEMPTS =>
                        {
                            agent_attempt += 1;
                        }
                        Err(error) if is_agent_retry(&error) => {
                            return Err(anyhow::anyhow!(
                                "SSH Agent authentication failed after {agent_attempt} attempts"
                            ));
                        }
                        result => return result,
                    }
                }
            })
            .await
            .map_err(anyhow::Error::msg)?
    })
}

pub(crate) async fn open_authenticated_ssh_handle_once(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11: Option<X11Registry>,
    shell_environment: Arc<ShellEnvironmentCache>,
    agent_attempt: u32,
) -> anyhow::Result<SshHandleChain> {
    if let Some(jump_config) = config.proxy_jump.as_deref() {
        let (jump_handle, mut jump_handles) = Box::pin(open_authenticated_ssh_handle_once(
            jump_config,
            None,
            None,
            Arc::clone(&shell_environment),
            agent_attempt,
        ))
        .await?;
        let direct_channel = tokio::time::timeout(
            Duration::from_secs(30),
            jump_handle.channel_open_direct_tcpip(&config.host, config.port.into(), "127.0.0.1", 0),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH ProxyJump direct-tcpip open timed out"))??;
        let mut handle = tokio::time::timeout(
            Duration::from_secs(30),
            client::connect_stream(
                ssh_client_config(config)?,
                direct_channel.into_stream(),
                SshClientHandler {
                    host: config.host.clone(),
                    port: config.port,
                    host_key_alias: config.host_key_alias.clone(),
                    verifier: config.host_key_verifier.clone(),
                    forwarded_tcpip: forwarded_tcpip.clone(),
                    x11: x11.clone(),
                    agent_forwarding_config: effective_agent_forwarding_config(config),
                    agent_stored_key_provider: config.agent_stored_key_provider.clone(),
                    shell_environment: Arc::clone(&shell_environment),
                },
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH ProxyJump target connection timed out"))??;
        authenticate_ssh(
            &mut handle,
            config,
            agent_attempt,
            Arc::clone(&shell_environment),
        )
        .await?;
        tracing::debug!(
            stage = "authentication",
            host = %config.host,
            port = config.port,
            profile = ?config.profile,
            via_jump = true,
            "SSH authentication completed"
        );
        jump_handles.push(jump_handle);
        return Ok((handle, jump_handles));
    }

    let mut handle = tokio::time::timeout(
        Duration::from_secs(30),
        connect_ssh_transport(
            config,
            forwarded_tcpip.clone(),
            x11.clone(),
            Arc::clone(&shell_environment),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SSH connection timed out"))??;

    authenticate_ssh(&mut handle, config, agent_attempt, shell_environment).await?;
    tracing::debug!(
        stage = "authentication",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        via_jump = false,
        "SSH authentication completed"
    );
    Ok((handle, Vec::new()))
}

pub(crate) async fn connect_ssh_transport(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11: Option<X11Registry>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<client::Handle<SshClientHandler>> {
    let handler = SshClientHandler {
        host: config.host.clone(),
        port: config.port,
        host_key_alias: config.host_key_alias.clone(),
        verifier: config.host_key_verifier.clone(),
        forwarded_tcpip,
        x11,
        agent_forwarding_config: effective_agent_forwarding_config(config),
        agent_stored_key_provider: config.agent_stored_key_provider.clone(),
        shell_environment,
    };
    let Some(proxy) = config.proxy.as_ref() else {
        return client::connect(
            ssh_client_config(config)?,
            (config.host.as_str(), config.port),
            handler,
        )
        .await
        .map_err(|error| anyhow::anyhow!("SSH connection failed: {error}"));
    };

    match proxy.protocol.as_str() {
        "socks5" => {
            let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
            let target = (config.host.as_str(), config.port);
            let stream = match (
                proxy.username.as_deref().filter(|value| !value.is_empty()),
                proxy.password.as_deref().filter(|value| !value.is_empty()),
            ) {
                (Some(username), Some(password)) => {
                    tokio_socks::tcp::Socks5Stream::connect_with_password(
                        proxy_addr.as_str(),
                        target,
                        username,
                        password,
                    )
                    .await
                }
                _ => tokio_socks::tcp::Socks5Stream::connect(proxy_addr.as_str(), target).await,
            }
            .map_err(|error| anyhow::anyhow!("SOCKS5 proxy connection failed: {error}"))?;
            client::connect_stream(ssh_client_config(config)?, stream.into_inner(), handler)
                .await
                .map_err(|error| anyhow::anyhow!("SSH connection via SOCKS5 proxy failed: {error}"))
        }
        "http" => {
            let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
            let mut stream = tokio::net::TcpStream::connect(&proxy_addr)
                .await
                .map_err(|error| anyhow::anyhow!("HTTP proxy connection failed: {error}"))?;
            crate::http_proxy::connect(
                &mut stream,
                &config.host,
                config.port,
                proxy
                    .username
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .zip(proxy.password.as_deref().filter(|value| !value.is_empty())),
            )
            .await
            .map_err(|error| anyhow::anyhow!("HTTP proxy tunnel failed: {error}"))?;
            client::connect_stream(ssh_client_config(config)?, stream, handler)
                .await
                .map_err(|error| anyhow::anyhow!("SSH connection via HTTP proxy failed: {error}"))
        }
        "proxycommand" => {
            let stream = open_proxy_command_stream(
                proxy.command.as_deref(),
                &config.host,
                config.port,
                &config.username,
            )
            .await?;
            client::connect_stream(ssh_client_config(config)?, stream, handler)
                .await
                .map_err(|error| anyhow::anyhow!("SSH connection via ProxyCommand failed: {error}"))
        }
        other => anyhow::bail!("unsupported SSH proxy protocol '{other}'"),
    }
}

pub(crate) use crate::proxy_command::open_proxy_command_stream;

pub(crate) fn expand_proxy_command(
    template: Option<&str>,
    host: &str,
    port: u16,
    username: &str,
) -> anyhow::Result<String> {
    let template = template.unwrap_or_default().trim();
    if template.is_empty() {
        anyhow::bail!("ProxyCommand is empty");
    }

    let quoted_host = local_shell_quote(host);
    let port = port.to_string();
    let quoted_port = local_shell_quote(&port);
    let quoted_username = local_shell_quote(username);

    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(&quoted_host),
            Some('p') => output.push_str(&quoted_port),
            Some('r') => output.push_str(&quoted_username),
            Some(other) => {
                output.push('%');
                output.push(other);
            }
            None => output.push('%'),
        }
    }

    Ok(output)
}

#[cfg(windows)]
pub(crate) fn system_shell_command(command: &str) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
pub(crate) fn system_shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
pub(crate) fn local_shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }

    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '@' | '%'))
    {
        return value.to_string();
    }

    let escaped = value.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(not(windows))]
pub(crate) fn local_shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // A host certificate replaces the host-key check rather than adding to it,
        // and known hosts records plain host keys only, so there is nothing to
        // compare a certificate against. Reject instead of falling back to the key
        // inside it: that key is not what the user was asked to trust. Unreachable
        // in practice, because the client never offers certificate host-key
        // algorithms (`Preferred::host_key_certificates` is left empty).
        let russh::keys::PublicKeyOrCertificate::PublicKey {
            key: server_public_key,
            ..
        } = server_public_key
        else {
            return Ok(false);
        };
        let Some(verifier) = &self.verifier else {
            return Ok(false);
        };
        let host_identifier = self
            .host_key_alias
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| ssh_host_identifier(&self.host, self.port));
        let host_key = SshHostKey {
            host: self.host.clone(),
            port: self.port,
            host_identifier,
            key_type: server_public_key.algorithm().to_string(),
            key_base64: server_public_key.public_key_base64(),
            fingerprint: server_public_key
                .fingerprint(Default::default())
                .to_string(),
        };
        match verifier.verify(&host_key) {
            Ok(SshHostKeyDecision::Accept) => Ok(true),
            Ok(SshHostKeyDecision::Reject(_)) | Err(_) => Ok(false),
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(registry) = self.forwarded_tcpip.as_ref() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let dispatch = registry.lock().await;
        let tx = forwarded_tcpip_sender_for(&dispatch, connected_address, connected_port);
        drop(dispatch);
        let Some(tx) = tx else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        if tx
            .send(ForwardedTcpIpChannel {
                channel,
                connected_address: connected_address.to_string(),
                connected_port,
                originator_address: originator_address.to_string(),
                originator_port,
            })
            .is_ok()
        {
            reply.accept().await;
        } else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<client::Msg>,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(registry) = self.x11.as_ref() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let mut registration = registry.lock().await;
        if registration
            .as_ref()
            .is_some_and(|registration| registration.tx.is_closed())
        {
            *registration = None;
        }
        let Some(tx) = registration
            .as_ref()
            .map(|registration| registration.tx.clone())
        else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        drop(registration);
        if tx
            .send(X11ChannelOpen {
                channel,
                originator_address: originator_address.to_string(),
                originator_port,
            })
            .is_ok()
        {
            reply.accept().await;
        } else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(config) = self.agent_forwarding_config.clone() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        if !config.enabled {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let Some(permit) = ssh_agent_broker::try_acquire_agent_channel_permit() else {
            reply
                .reject(russh::ChannelOpenFailure::ResourceShortage)
                .await;
            let _ = channel.close().await;
            return Ok(());
        };
        if is_raw_relay_compatible(&config) {
            let endpoint = config.sources.external_agent_endpoints[0].clone();
            let shell_environment = Arc::clone(&self.shell_environment);
            tokio::spawn(async move {
                let Ok(agent_stream) = ssh_agent::connect_agent_stream_with_environment(
                    &endpoint,
                    Some(shell_environment.clone()),
                )
                .await
                else {
                    reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                    let _ = channel.close().await;
                    return;
                };
                reply.accept().await;
                ssh_agent_broker::serve_raw_channel(channel.into_stream(), agent_stream, permit)
                    .await;
            });
            return Ok(());
        }
        let provider = self.agent_stored_key_provider.clone();
        let shell_environment = Arc::clone(&self.shell_environment);
        tokio::spawn(async move {
            reply.accept().await;
            ssh_agent_broker::serve_channel(
                channel.into_stream(),
                config,
                provider,
                shell_environment,
                permit,
            )
            .await;
        });
        Ok(())
    }
}

pub(crate) fn effective_agent_forwarding_config(
    config: &SshSessionConfig,
) -> Option<SshAgentForwardingConfig> {
    if config.runtime_mode == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp {
        return None;
    }
    config.agent_forwarding_config.clone().or_else(|| {
        config.agent_forwarding.then(|| SshAgentForwardingConfig {
            enabled: true,
            sources: SshAgentForwardingSources {
                external_agent: true,
                external_agent_endpoints: vec![config.agent_endpoint.clone()],
                stored_keys: false,
            },
            policy: SshAgentForwardingPolicy::All,
        })
    })
}

pub(crate) fn current_agent_stored_key_revision(config: &SshSessionConfig) -> Option<u64> {
    let forwarding = effective_agent_forwarding_config(config)?;
    if !forwarding.enabled || !forwarding.sources.stored_keys {
        return None;
    }
    config
        .agent_stored_key_provider
        .as_ref()
        .and_then(|provider| provider.revision().ok())
}

pub(crate) fn is_raw_relay_compatible(config: &SshAgentForwardingConfig) -> bool {
    config.sources.external_agent
        && config.sources.external_agent_endpoints.len() == 1
        && !config.sources.stored_keys
        && matches!(config.policy, SshAgentForwardingPolicy::All)
}

pub(crate) fn ssh_host_identifier(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}
