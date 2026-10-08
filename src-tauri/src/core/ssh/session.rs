use super::agent_broker::AgentBrokerFactory;
use super::auth::{SSH_AGENT_AUTH_RETRY, authenticate_handle, load_saved_ssh_config};
use super::client::{
    RemoteForwardOpen, SshConfig, SshConnectionHandles, SshDiagnosticContext, SshDiagnosticStage,
    SshHandle, SshHandler, SshRawHandle, SshStartupCommand, build_client_config,
    connect_via_stream, connect_with_proxy,
};
use super::io::{open_shell_channel, sftp_only_ssh_lifecycle_loop, ssh_io_loop};
use crate::config::{
    AiExecutionProfile, SshAgentForwardingConfig, SshAgentForwardingPolicy, SshProfile,
    SshRuntimeMode, effective_cwd_follow_mode_for_runtime,
};
use crate::core::{
    DynamicTitleCapabilities, SessionHandle, SessionInfo, SessionManager, SessionReadyHook,
    SessionType, SharedCwd, session_command_channel,
};
use crate::error::{AppError, AppResult};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};

async fn create_authenticated_connection(
    app: &AppHandle,
    config: &SshConfig,
    enable_agent_forwarding: bool,
) -> AppResult<(
    SshHandle,
    Option<mpsc::UnboundedReceiver<super::x11_forwarding::X11ChannelOpen>>,
)> {
    create_authenticated_connection_with_notifications(
        app,
        config,
        None,
        None,
        enable_agent_forwarding,
        None,
    )
    .await
}

async fn create_session_connection_with_disconnect(
    app: &AppHandle,
    config: &SshConfig,
    enable_agent_forwarding: bool,
    diagnostics: Option<SshDiagnosticContext>,
) -> AppResult<(
    SshHandle,
    Option<mpsc::UnboundedReceiver<super::x11_forwarding::X11ChannelOpen>>,
    mpsc::UnboundedReceiver<String>,
)> {
    loop {
        let (disconnect_tx, disconnect_rx) = mpsc::unbounded_channel();
        match create_authenticated_connection_with_notifications(
            app,
            config,
            Some(disconnect_tx),
            None,
            enable_agent_forwarding,
            diagnostics.clone(),
        )
        .await
        {
            Err(error) if is_agent_auth_retry(&error) => continue,
            Ok((handle, x11_rx)) => return Ok((handle, x11_rx, disconnect_rx)),
            Err(error) => return Err(error),
        }
    }
}

async fn create_authenticated_connection_with_notifications(
    app: &AppHandle,
    config: &SshConfig,
    disconnect_tx: Option<mpsc::UnboundedSender<String>>,
    remote_forward_tx: Option<mpsc::UnboundedSender<RemoteForwardOpen>>,
    enable_agent_forwarding: bool,
    diagnostics: Option<SshDiagnosticContext>,
) -> AppResult<(
    SshHandle,
    Option<mpsc::UnboundedReceiver<super::x11_forwarding::X11ChannelOpen>>,
)> {
    let (x11_tx, x11_rx) = if config.x11_forwarding {
        let (tx, rx) = mpsc::unbounded_channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let (target_handle, jumps) = connect_authenticated_chain(
        app,
        config,
        x11_tx,
        disconnect_tx,
        remote_forward_tx,
        enable_agent_forwarding,
        diagnostics,
    )
    .await?;
    Ok((
        Arc::new(SshConnectionHandles::new(target_handle, jumps)),
        x11_rx,
    ))
}

async fn connect_authenticated_chain(
    app: &AppHandle,
    config: &SshConfig,
    x11_tx: Option<mpsc::UnboundedSender<super::x11_forwarding::X11ChannelOpen>>,
    disconnect_tx: Option<mpsc::UnboundedSender<String>>,
    remote_forward_tx: Option<mpsc::UnboundedSender<RemoteForwardOpen>>,
    enable_agent_forwarding: bool,
    diagnostics: Option<SshDiagnosticContext>,
) -> AppResult<(SshRawHandle, Vec<SshRawHandle>)> {
    connect_authenticated_chain_boxed(
        app,
        config,
        x11_tx,
        disconnect_tx,
        remote_forward_tx,
        enable_agent_forwarding,
        diagnostics,
    )
    .await
}

fn connect_authenticated_chain_boxed<'a>(
    app: &'a AppHandle,
    config: &'a SshConfig,
    x11_tx: Option<mpsc::UnboundedSender<super::x11_forwarding::X11ChannelOpen>>,
    disconnect_tx: Option<mpsc::UnboundedSender<String>>,
    remote_forward_tx: Option<mpsc::UnboundedSender<RemoteForwardOpen>>,
    enable_agent_forwarding: bool,
    diagnostics: Option<SshDiagnosticContext>,
) -> Pin<Box<dyn Future<Output = AppResult<(SshRawHandle, Vec<SshRawHandle>)>> + Send + 'a>> {
    Box::pin(async move {
        if let Some(jump_config) = config.proxy_jump.as_deref() {
            tracing::info!(
                jump_host = %jump_config.host,
                jump_port = jump_config.port,
                target_host = %config.host,
                target_port = config.port,
                "Creating SSH connection via ProxyJump"
            );

            let (jump_handle, mut jumps) =
                connect_authenticated_chain(app, jump_config, None, None, None, false, None)
                    .await?;
            let channel = {
                let jump = jump_handle.lock().await;
                jump.channel_open_direct_tcpip(&config.host, config.port.into(), "127.0.0.1", 0)
                    .await
                    .map_err(|error| {
                        AppError::Channel(format!("Failed to open ProxyJump channel: {}", error))
                    })?
            };
            tracing::info!(
                jump_host = %jump_config.host,
                jump_port = jump_config.port,
                target_host = %config.host,
                target_port = config.port,
                "ProxyJump direct-tcpip channel opened"
            );

            let mut target_handler = SshHandler::new(
                app.clone(),
                config.host.clone(),
                config.port,
                config.owner_window_label.clone(),
            );
            if let Some(tx) = x11_tx {
                target_handler = target_handler.with_x11_sender(tx);
            }
            if let Some(tx) = disconnect_tx {
                target_handler = target_handler.with_disconnect_sender(tx);
            }
            if let Some(tx) = remote_forward_tx {
                target_handler = target_handler.with_remote_forward_sender(tx);
            }
            let forwarding = effective_forwarding_config(config);
            if should_attach_agent_forwarding(enable_agent_forwarding, forwarding.enabled) {
                target_handler = attach_agent_forwarding(app, config, target_handler)?;
            }
            if let Some(diagnostics) = diagnostics.clone() {
                target_handler = target_handler.with_diagnostics(diagnostics);
            }
            let ssh_client_config = Arc::new(build_client_config(app, config)?);
            let mut target_handle =
                connect_via_stream(channel.into_stream(), ssh_client_config, target_handler)
                    .await?;
            authenticate_handle(
                &mut target_handle,
                config,
                app,
                "Authentication failed: invalid credentials",
                "Authentication failed: key rejected",
            )
            .await?;
            tracing::info!(
                host = %config.host,
                port = config.port,
                "SSH host authenticated via ProxyJump"
            );

            jumps.push(jump_handle);
            let target_handle: SshRawHandle = Arc::new(tokio::sync::Mutex::new(target_handle));
            return Ok((target_handle, jumps));
        }

        let mut handler = SshHandler::new(
            app.clone(),
            config.host.clone(),
            config.port,
            config.owner_window_label.clone(),
        );
        if let Some(tx) = x11_tx {
            handler = handler.with_x11_sender(tx);
        }
        if let Some(tx) = disconnect_tx {
            handler = handler.with_disconnect_sender(tx);
        }
        if let Some(tx) = remote_forward_tx {
            handler = handler.with_remote_forward_sender(tx);
        }
        let forwarding = effective_forwarding_config(config);
        if should_attach_agent_forwarding(enable_agent_forwarding, forwarding.enabled) {
            handler = attach_agent_forwarding(app, config, handler)?;
        }
        if let Some(diagnostics) = diagnostics.clone() {
            handler = handler.with_diagnostics(diagnostics);
        }
        let ssh_client_config = Arc::new(build_client_config(app, config)?);
        let mut handle = connect_with_proxy(config, ssh_client_config, handler).await?;
        authenticate_handle(
            &mut handle,
            config,
            app,
            "Authentication failed: invalid credentials",
            "Authentication failed: key rejected",
        )
        .await?;
        tracing::info!(
            host = %config.host,
            port = config.port,
            "SSH host authenticated"
        );

        let handle: SshRawHandle = Arc::new(tokio::sync::Mutex::new(handle));
        Ok((handle, Vec::new()))
    })
}

fn should_attach_agent_forwarding(global_enabled: bool, connection_enabled: bool) -> bool {
    global_enabled && connection_enabled
}

fn attach_agent_forwarding(
    app: &AppHandle,
    config: &SshConfig,
    handler: SshHandler,
) -> AppResult<SshHandler> {
    let forwarding = effective_forwarding_config(config);
    if is_raw_agent_forwarding_config(&forwarding) {
        return Ok(handler.with_agent_forwarding_endpoint(
            forwarding.sources.external_agent_endpoints[0].clone(),
        ));
    }

    let broker = Arc::new(AgentBrokerFactory::new(app, &forwarding)?);
    Ok(handler.with_agent_broker(broker))
}

fn is_raw_agent_forwarding_config(config: &SshAgentForwardingConfig) -> bool {
    config.sources.external_agent
        && config.sources.external_agent_endpoints.len() == 1
        && !config.sources.stored_keys
        && matches!(&config.policy, SshAgentForwardingPolicy::All)
}

fn effective_forwarding_config(config: &SshConfig) -> SshAgentForwardingConfig {
    config.agent_forwarding_config.clone()
}

fn is_agent_auth_retry(error: &AppError) -> bool {
    matches!(error, AppError::Auth(message) if message == SSH_AGENT_AUTH_RETRY)
}

fn set_owner_window_label(config: &mut SshConfig, owner_window_label: Option<String>) {
    config.owner_window_label = owner_window_label.clone();
    if let Some(proxy_jump) = config.proxy_jump.as_mut() {
        set_owner_window_label(proxy_jump, owner_window_label);
    }
}

pub(crate) struct SshForwardedStream {
    stream: russh::ChannelStream<russh::client::Msg>,
    _ssh_handle: SshHandle,
}

impl AsyncRead for SshForwardedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshForwardedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

pub(crate) async fn open_ssh_direct_tcpip_stream(
    app: &AppHandle,
    jump_connection_id: &str,
    target_host: &str,
    target_port: u16,
    owner_window_label: Option<String>,
) -> AppResult<SshForwardedStream> {
    let mut ssh_config = load_saved_ssh_config(app, jump_connection_id)?;
    set_owner_window_label(&mut ssh_config, owner_window_label);
    let (ssh_handle, _x11_rx) = loop {
        match create_authenticated_connection(app, &ssh_config, false).await {
            Err(error) if is_agent_auth_retry(&error) => continue,
            result => break result,
        }
    }?;

    let channel = {
        let handle = ssh_handle.target_handle();
        let handle = handle.lock().await;
        handle
            .channel_open_direct_tcpip(target_host, target_port.into(), "127.0.0.1", 0)
            .await
            .map_err(|error| {
                AppError::Channel(format!(
                    "Failed to open SSH ProxyJump direct-tcpip channel to {target_host}:{target_port}: {error}"
                ))
            })?
    };

    tracing::info!(
        jump_connection_id = %jump_connection_id,
        target_host = %target_host,
        target_port = target_port,
        "SSH direct-tcpip stream opened"
    );

    Ok(SshForwardedStream {
        stream: channel.into_stream(),
        _ssh_handle: ssh_handle,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SshRuntimeCapabilities {
    remote_file_browser_enabled: bool,
    remote_stats_enabled: bool,
    network_device_profile: bool,
}

fn resolve_runtime_capabilities(config: &SshConfig) -> SshRuntimeCapabilities {
    let network_device_profile = config.ssh_profile == SshProfile::NetworkDevice;
    let terminal_only = config.runtime_mode == SshRuntimeMode::Terminal;
    let sftp_only = config.runtime_mode == SshRuntimeMode::Sftp;
    SshRuntimeCapabilities {
        remote_file_browser_enabled: config.sftp.enabled
            && (sftp_only || (!network_device_profile && !terminal_only)),
        remote_stats_enabled: !network_device_profile && !terminal_only && !sftp_only,
        network_device_profile,
    }
}

async fn try_sftp_only_fallback<F, Fut>(
    session_id: &str,
    remote_file_browser_enabled: bool,
    shell_error: AppError,
    probe: F,
) -> AppResult<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = AppResult<()>>,
{
    if !remote_file_browser_enabled {
        return Err(shell_error);
    }

    match probe().await {
        Ok(()) => {
            tracing::info!(
                session_id = %session_id,
                shell_error = %shell_error,
                "SSH shell unavailable; continuing with SFTP-only session"
            );
            Ok(())
        }
        Err(probe_error) => {
            tracing::warn!(
                session_id = %session_id,
                shell_error = %shell_error,
                sftp_probe_error = %probe_error,
                "SFTP probe failed on current SSH transport"
            );
            Err(shell_error)
        }
    }
}

fn apply_sftp_only_runtime_policy(info: &mut SessionInfo) {
    info.ai_execution_profile = AiExecutionProfile::Disabled;
    info.injection_active = false;
    info.dynamic_title_capabilities = DynamicTitleCapabilities::default();
    info.remote_file_browser_enabled = true;
    info.remote_stats_enabled = false;
    info.ssh_runtime_mode = Some(SshRuntimeMode::Sftp);
}

fn validate_runtime_config(config: &SshConfig) -> AppResult<()> {
    if config.runtime_mode == SshRuntimeMode::Sftp && !config.sftp.enabled {
        return Err(AppError::Config(
            "SFTP is disabled for this SSH connection".to_string(),
        ));
    }
    Ok(())
}

/// Creates an authenticated SSH handle for a saved connection without opening a PTY/shell.
/// Used by tunnels to establish their own independent SSH connections.
#[allow(dead_code)]
pub async fn create_ssh_handle(app: &AppHandle, connection_id: &str) -> AppResult<SshHandle> {
    let ssh_config = load_saved_ssh_config(app, connection_id)?;
    let (handle, _x11_rx) = loop {
        match create_authenticated_connection(app, &ssh_config, false).await {
            Err(error) if is_agent_auth_retry(&error) => continue,
            result => break result,
        }
    }?;

    tracing::info!(
        host = %ssh_config.host,
        port = ssh_config.port,
        "Tunnel SSH handle created"
    );

    Ok(handle)
}

pub async fn create_ssh_handle_for_tunnel(
    app: &AppHandle,
    connection_id: &str,
    disconnect_tx: mpsc::UnboundedSender<String>,
    remote_forward_tx: Option<mpsc::UnboundedSender<RemoteForwardOpen>>,
) -> AppResult<SshHandle> {
    let ssh_config = load_saved_ssh_config(app, connection_id)?;
    let (handle, _x11_rx) = loop {
        match create_authenticated_connection_with_notifications(
            app,
            &ssh_config,
            Some(disconnect_tx.clone()),
            remote_forward_tx.clone(),
            false,
            None,
        )
        .await
        {
            Err(error) if is_agent_auth_retry(&error) => continue,
            result => break result,
        }
    }?;

    tracing::info!(
        host = %ssh_config.host,
        port = ssh_config.port,
        "Tunnel SSH handle created"
    );

    Ok(handle)
}

/// Connects via SSH, opens a PTY shell, and spawns the I/O loop.
pub async fn create_ssh_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: SshConfig,
    connection_id: Option<String>,
    owner_window_label: Option<String>,
    cancel_rx: Option<oneshot::Receiver<()>>,
    startup_command: Option<SshStartupCommand>,
    session_ready_hook: Option<SessionReadyHook>,
) -> AppResult<String> {
    if let Some(mut cancel_rx) = cancel_rx {
        return tokio::select! {
            result = create_ssh_session_inner(app, manager, config, connection_id, owner_window_label, startup_command, session_ready_hook) => result,
            _ = &mut cancel_rx => Err(AppError::Cancelled("Session creation cancelled".to_string())),
        };
    }

    create_ssh_session_inner(
        app,
        manager,
        config,
        connection_id,
        owner_window_label,
        startup_command,
        session_ready_hook,
    )
    .await
}

async fn create_ssh_session_inner(
    app: AppHandle,
    manager: Arc<SessionManager>,
    mut config: SshConfig,
    connection_id: Option<String>,
    owner_window_label: Option<String>,
    startup_command: Option<SshStartupCommand>,
    session_ready_hook: Option<SessionReadyHook>,
) -> AppResult<String> {
    set_owner_window_label(&mut config, owner_window_label.clone());
    tracing::info!(
        host = %config.host,
        port = config.port,
        user = %config.username,
        "Creating SSH session"
    );

    let session_id = uuid::Uuid::new_v4().to_string();
    let diagnostics = SshDiagnosticContext::new(Some(session_id.clone()));
    let (cmd_tx, cmd_rx) = session_command_channel(session_id.clone());
    let explicit_sftp = config.runtime_mode == SshRuntimeMode::Sftp;

    validate_runtime_config(&config)?;

    let x11_config = if config.x11_forwarding && !explicit_sftp {
        Some(super::x11_forwarding::prepare_x11_forwarding(&config.x11_display).await)
    } else {
        None
    };
    let (ssh_connection, x11_rx, disconnect_rx) = create_session_connection_with_disconnect(
        &app,
        &config,
        !explicit_sftp,
        Some(diagnostics.clone()),
    )
    .await?;
    diagnostics.set_stage(SshDiagnosticStage::Authenticated);
    let capabilities = resolve_runtime_capabilities(&config);
    let effective_cwd_follow_mode = effective_cwd_follow_mode_for_runtime(
        &config.sftp,
        &config.ssh_profile,
        &config.runtime_mode,
    );
    tracing::info!(
        session_id = %session_id,
        host = %config.host,
        port = config.port,
        ssh_profile = ?config.ssh_profile,
        runtime_mode = ?config.runtime_mode,
        terminal_type = %config.terminal_type.as_str(),
        sftp_enabled = config.sftp.enabled,
        cwd_follow_mode = ?config.sftp.cwd_follow_mode,
        effective_cwd_follow_mode = ?effective_cwd_follow_mode,
        remote_file_browser_enabled = capabilities.remote_file_browser_enabled,
        remote_stats_enabled = capabilities.remote_stats_enabled,
        shell_detection_timeout_ms = config.sftp.shell_detection_timeout_ms,
        sftp_compatibility_mode = config.sftp.compatibility_mode,
        "SSH session initialization starting"
    );
    let mut preinitialized_remote_fs = None;
    let (shell, sftp_only) = if explicit_sftp {
        if config.sftp.compatibility_mode {
            preinitialized_remote_fs = Some(
                crate::core::sftp::create_compatibility_remote_fs(
                    &app,
                    ssh_connection.clone(),
                    &config,
                )
                .await?,
            );
        } else {
            crate::core::sftp::probe_sftp_subsystem(&ssh_connection).await?;
        }
        tracing::info!(session_id = %session_id, "Explicit SFTP-only runtime established");
        (None, true)
    } else {
        let handle_mtx = ssh_connection.target_handle();
        let mut handle = handle_mtx.lock().await;
        let forwarding_enabled =
            should_attach_agent_forwarding(true, effective_forwarding_config(&config).enabled);
        let shell_result = open_shell_channel(
            &mut handle,
            &session_id,
            x11_config.as_ref().map(|cfg| cfg.fake_cookie_hex.as_str()),
            forwarding_enabled,
            config.terminal_type.as_str(),
            capabilities.remote_file_browser_enabled,
            capabilities.network_device_profile,
            effective_cwd_follow_mode,
            config.sftp.shell_detection_timeout_ms,
            Some(diagnostics.clone()),
        )
        .await;
        drop(handle);

        match shell_result {
            Ok(shell) => (Some(shell), false),
            Err(shell_error) => {
                if config.sftp.compatibility_mode {
                    if !capabilities.remote_file_browser_enabled {
                        return Err(shell_error);
                    }
                    match crate::core::sftp::create_compatibility_remote_fs(
                        &app,
                        ssh_connection.clone(),
                        &config,
                    )
                    .await
                    {
                        Ok(remote_fs) => {
                            tracing::info!(
                                session_id = %session_id,
                                shell_error = %shell_error,
                                "SSH shell unavailable; continuing with compatibility SFTP-only session"
                            );
                            preinitialized_remote_fs = Some(remote_fs);
                        }
                        Err(probe_error) => {
                            tracing::warn!(
                                session_id = %session_id,
                                shell_error = %shell_error,
                                sftp_probe_error = %probe_error,
                                "Compatibility SFTP initialization failed on current SSH transport"
                            );
                            return Err(shell_error);
                        }
                    }
                } else {
                    try_sftp_only_fallback(
                        &session_id,
                        capabilities.remote_file_browser_enabled,
                        shell_error,
                        || crate::core::sftp::probe_sftp_subsystem(&ssh_connection),
                    )
                    .await?;
                }
                config.runtime_mode = SshRuntimeMode::Sftp;
                (None, true)
            }
        }
    };
    debug_assert_eq!(sftp_only, shell.is_none());
    let injection_active = shell
        .as_ref()
        .is_some_and(|(_, injection_script, _, _, _)| injection_script.is_some());

    if !sftp_only {
        if let (Some(rx), Some(x11_config)) = (x11_rx, x11_config) {
            super::x11_forwarding::spawn_x11_forwarder(
                app.clone(),
                session_id.clone(),
                rx,
                x11_config,
            );
        }
    }

    let mut session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::SSH,
        started_at: crate::core::now_session_started_at(),
        connection_id: connection_id.clone(),
        connected: true,
        owner_window_label,
        ai_execution_profile: AiExecutionProfile::Posix,
        injection_active,
        cwd_tracking_active: false,
        dynamic_title_capabilities: DynamicTitleCapabilities::new(config.dynamic_tab_title, None),
        remote_file_browser_enabled: capabilities.remote_file_browser_enabled,
        remote_stats_enabled: capabilities.remote_stats_enabled,
        ssh_profile: Some(config.ssh_profile.clone()),
        ssh_runtime_mode: Some(config.runtime_mode),
    };
    if sftp_only {
        apply_sftp_only_runtime_policy(&mut session_info);
    }

    if preinitialized_remote_fs.is_none()
        && config.sftp.compatibility_mode
        && session_info.remote_file_browser_enabled
    {
        preinitialized_remote_fs = Some(crate::core::sftp::create_auto_remote_fs(
            &app,
            ssh_connection.clone(),
            &config,
            false,
            true,
        ));
    }

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(Default::default()));
    let ssh_config_arc: Arc<dyn std::any::Any + Send + Sync> = Arc::new(config.clone());
    let ssh_handle_arc: Arc<dyn std::any::Any + Send + Sync> = ssh_connection.clone();
    let session_handle = SessionHandle {
        info: session_info.clone(),
        cmd_tx: cmd_tx.clone(),
        startup_input_barrier: None,
        ssh_config: Some(ssh_config_arc),
        ssh_handle: Some(ssh_handle_arc),
        cwd: cwd.clone(),
        remote_fs: preinitialized_remote_fs,
    };
    manager.add_session(session_handle).await;
    tracing::info!(session_id = %session_id, "SSH session registered");
    if !sftp_only {
        if let Some(hook) = session_ready_hook.as_ref() {
            hook(&session_info);
        }
    }

    if let Some(ref conn_id) = connection_id {
        if let Some(tunnel_mgr) = app.try_state::<Arc<super::TunnelManager>>() {
            let tunnel_manager = tunnel_mgr.inner().clone();
            let connection_id = conn_id.clone();
            let app_handle = app.clone();
            tokio::spawn(async move {
                tunnel_manager
                    .auto_open_for_connection(&app_handle, &connection_id)
                    .await;
            });
        }
    }

    let io_session_id = session_id.clone();
    match shell {
        Some((channel, injection_script, ready_marker, detected_shell, initial_notice)) => {
            let output_control_tx = cmd_tx;
            let post_login = config.post_login.clone();
            let backspace_mode = config.backspace_mode.clone();
            let encoding = config.encoding.clone();
            tokio::spawn(async move {
                ssh_io_loop(
                    app,
                    io_session_id,
                    manager,
                    channel,
                    ssh_connection,
                    cmd_rx,
                    output_control_tx,
                    cwd,
                    connection_id,
                    injection_script,
                    ready_marker,
                    detected_shell,
                    post_login,
                    startup_command,
                    backspace_mode,
                    initial_notice,
                    encoding,
                    Some(diagnostics),
                )
                .await;
            });
        }
        None => {
            tokio::spawn(async move {
                sftp_only_ssh_lifecycle_loop(
                    app,
                    io_session_id,
                    manager,
                    ssh_connection,
                    cmd_rx,
                    disconnect_rx,
                    connection_id,
                )
                .await;
            });
        }
    }
    Ok(session_id)
}

/// Opens a new PTY shell channel on an existing authenticated SSH connection.
pub async fn create_multiplexed_ssh_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    source_session_id: &str,
    startup_command: Option<SshStartupCommand>,
    session_ready_hook: Option<SessionReadyHook>,
) -> AppResult<String> {
    let (config, ssh_connection, owner_window_label, shared_remote_fs) = {
        let sessions = manager.sessions.lock().await;
        let source = sessions.get(source_session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", source_session_id))
        })?;

        if source.info.session_type != SessionType::SSH {
            return Err(AppError::Config(
                "Source session is not an SSH session".to_string(),
            ));
        }
        if source.info.ssh_runtime_mode == Some(SshRuntimeMode::Sftp) {
            return Err(AppError::Config(
                "SFTP-only sessions cannot open multiplexed shell sessions".to_string(),
            ));
        }

        let config = source
            .ssh_config
            .as_ref()
            .and_then(|cfg| cfg.downcast_ref::<SshConfig>())
            .cloned()
            .ok_or_else(|| AppError::Config("Failed to get SSH config".to_string()))?;

        let ssh_connection = source
            .ssh_handle
            .as_ref()
            .ok_or_else(|| AppError::Config("Source session has no SSH handle".to_string()))?
            .clone()
            .downcast::<SshConnectionHandles>()
            .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?;

        let shared_remote_fs = if config.sftp.compatibility_mode {
            source.remote_fs.clone()
        } else {
            None
        };

        (
            config,
            ssh_connection,
            source.info.owner_window_label.clone(),
            shared_remote_fs,
        )
    };

    tracing::info!(
        source_session_id,
        host = %config.host,
        port = config.port,
        user = %config.username,
        "Creating multiplexed SSH session"
    );

    let session_id = uuid::Uuid::new_v4().to_string();
    let diagnostics = SshDiagnosticContext::new(Some(session_id.clone()));
    diagnostics.set_stage(SshDiagnosticStage::Authenticated);
    let capabilities = resolve_runtime_capabilities(&config);
    let effective_cwd_follow_mode = effective_cwd_follow_mode_for_runtime(
        &config.sftp,
        &config.ssh_profile,
        &config.runtime_mode,
    );
    tracing::info!(
        session_id = %session_id,
        source_session_id,
        host = %config.host,
        port = config.port,
        ssh_profile = ?config.ssh_profile,
        runtime_mode = ?config.runtime_mode,
        terminal_type = %config.terminal_type.as_str(),
        sftp_enabled = config.sftp.enabled,
        cwd_follow_mode = ?config.sftp.cwd_follow_mode,
        effective_cwd_follow_mode = ?effective_cwd_follow_mode,
        remote_file_browser_enabled = capabilities.remote_file_browser_enabled,
        remote_stats_enabled = capabilities.remote_stats_enabled,
        shell_detection_timeout_ms = config.sftp.shell_detection_timeout_ms,
        "SSH session initialization starting"
    );
    let (cmd_tx, cmd_rx) = session_command_channel(session_id.clone());

    if config.x11_forwarding {
        let connection_id = config.connection_id.clone().ok_or_else(|| {
            AppError::Config("X11 forwarding requires a saved SSH connection".to_string())
        })?;
        return create_ssh_session(
            app,
            manager,
            config,
            Some(connection_id),
            owner_window_label,
            None,
            startup_command,
            session_ready_hook,
        )
        .await;
    }

    let handle_mtx = ssh_connection.target_handle();
    let mut handle = handle_mtx.lock().await;
    let forwarding_enabled =
        should_attach_agent_forwarding(true, effective_forwarding_config(&config).enabled);
    let (channel, injection_script, ready_marker, detected_shell, initial_notice) =
        open_shell_channel(
            &mut handle,
            &session_id,
            None,
            forwarding_enabled,
            config.terminal_type.as_str(),
            capabilities.remote_file_browser_enabled,
            capabilities.network_device_profile,
            effective_cwd_follow_mode,
            config.sftp.shell_detection_timeout_ms,
            Some(diagnostics.clone()),
        )
        .await?;
    drop(handle);
    let injection_active = injection_script.is_some();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::SSH,
        started_at: crate::core::now_session_started_at(),
        connection_id: config.connection_id.clone(),
        connected: true,
        owner_window_label,
        ai_execution_profile: AiExecutionProfile::Posix,
        injection_active,
        cwd_tracking_active: false,
        dynamic_title_capabilities: DynamicTitleCapabilities::new(config.dynamic_tab_title, None),
        remote_file_browser_enabled: capabilities.remote_file_browser_enabled,
        remote_stats_enabled: capabilities.remote_stats_enabled,
        ssh_profile: Some(config.ssh_profile.clone()),
        ssh_runtime_mode: Some(config.runtime_mode),
    };

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(Default::default()));
    let ssh_config_arc: Arc<dyn std::any::Any + Send + Sync> = Arc::new(config.clone());
    let ssh_handle_arc: Arc<dyn std::any::Any + Send + Sync> = ssh_connection.clone();
    let output_control_tx = cmd_tx.clone();

    let session_handle = SessionHandle {
        info: session_info.clone(),
        cmd_tx,
        startup_input_barrier: None,
        ssh_config: Some(ssh_config_arc),
        ssh_handle: Some(ssh_handle_arc),
        cwd: cwd.clone(),
        remote_fs: shared_remote_fs,
    };
    manager.add_session(session_handle).await;
    tracing::info!(
        session_id = %session_id,
        source_session_id,
        "Multiplexed SSH session registered"
    );
    if let Some(hook) = session_ready_hook.as_ref() {
        hook(&session_info);
    }

    let io_session_id = session_id.clone();
    let io_manager = manager.clone();
    let io_handle = ssh_connection.clone();
    let io_connection_id = config.connection_id.clone();
    let post_login = config.post_login.clone();
    let startup_command = startup_command.clone();
    let backspace_mode = config.backspace_mode.clone();
    let encoding = config.encoding.clone();
    tokio::spawn(async move {
        ssh_io_loop(
            app,
            io_session_id,
            io_manager,
            channel,
            io_handle,
            cmd_rx,
            output_control_tx,
            cwd,
            io_connection_id,
            injection_script,
            ready_marker,
            detected_shell,
            post_login,
            startup_command,
            backspace_mode,
            initial_notice,
            encoding,
            Some(diagnostics),
        )
        .await;
    });
    Ok(session_id)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_sftp_only_runtime_policy, is_agent_auth_retry, is_raw_agent_forwarding_config,
        resolve_runtime_capabilities, should_attach_agent_forwarding, try_sftp_only_fallback,
        validate_runtime_config,
    };
    use crate::config::{
        SftpCwdFollowMode, SftpSettings, SshAgentEndpoint, SshAgentForwardingConfig,
        SshAgentForwardingPolicy, SshAgentForwardingSources, SshProfile, SshRuntimeMode,
        SshTerminalType,
    };
    use crate::core::ssh::client::{SshAuth, SshConfig};
    use crate::core::{
        DynamicTitleCapabilities, SessionHandle, SessionInfo, SessionManager, SessionType,
        session_command_channel,
    };
    use crate::error::AppError;
    use russh::{Channel, ChannelId, ChannelMsg, ChannelOpenFailure, Disconnect, client, server};
    use russh_sftp::protocol::{File, FileAttributes, Handle, Name, Status, StatusCode, Version};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;
    use tokio::time::{Duration, timeout};

    #[test]
    fn agent_forwarding_requires_both_global_and_connection_flags() {
        assert!(!should_attach_agent_forwarding(false, false));
        assert!(!should_attach_agent_forwarding(false, true));
        assert!(!should_attach_agent_forwarding(true, false));
        assert!(should_attach_agent_forwarding(true, true));
    }

    #[test]
    fn raw_agent_forwarding_is_limited_to_the_legacy_compatible_shape() {
        let raw = SshAgentForwardingConfig {
            enabled: true,
            sources: SshAgentForwardingSources {
                external_agent: true,
                external_agent_endpoints: vec![SshAgentEndpoint::Auto],
                stored_keys: false,
            },
            policy: SshAgentForwardingPolicy::All,
        };
        assert!(is_raw_agent_forwarding_config(&raw));

        let mut multiple = raw.clone();
        multiple
            .sources
            .external_agent_endpoints
            .push(SshAgentEndpoint::Auto);
        assert!(!is_raw_agent_forwarding_config(&multiple));

        let mut allowlist = raw;
        allowlist.policy = SshAgentForwardingPolicy::Allowlist {
            fingerprints: vec!["SHA256:example".to_string()],
        };
        assert!(!is_raw_agent_forwarding_config(&allowlist));
    }

    #[test]
    fn agent_retry_error_is_the_only_error_reconstructed() {
        assert!(is_agent_auth_retry(&AppError::Auth(
            super::SSH_AGENT_AUTH_RETRY.to_string()
        )));
        assert!(!is_agent_auth_retry(&AppError::Auth(
            "other-auth-error".to_string()
        )));
        assert!(!is_agent_auth_retry(&AppError::Cancelled(
            super::SSH_AGENT_AUTH_RETRY.to_string()
        )));
    }

    fn test_config(profile: SshProfile) -> SshConfig {
        SshConfig {
            connection_id: None,
            owner_window_label: None,
            name: "test".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SshAuth::None,
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            x11_display: String::new(),
            auth_agent_endpoint: Some(SshAgentEndpoint::Auto),
            agent_forwarding_config: crate::config::SshAgentForwardingConfig::default(),
            proxy: None,
            proxy_jump: None,
            post_login: None,
            ssh_algorithms: None,
            ssh_profile: profile,
            runtime_mode: SshRuntimeMode::Standard,
            terminal_type: SshTerminalType::default(),
            sftp: SftpSettings::default(),
            encoding: "UTF-8".to_string(),
            dynamic_tab_title: false,
        }
    }

    struct SftpOnlyTestClient;

    impl client::Handler for SftpOnlyTestClient {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            _server_public_key: &russh::keys::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    #[derive(Clone, Copy)]
    enum TestShellBehavior {
        Reject,
        AcceptWithoutExec,
    }

    fn assert_fake_server_session(result: Result<(), russh::Error>) {
        match result {
            Ok(()) => {}
            Err(russh::Error::IO(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(error) => panic!("fake SSH server session: {error:?}"),
        }
    }

    struct SftpOnlyTestServer {
        channels: Arc<Mutex<HashMap<ChannelId, Channel<server::Msg>>>>,
        allow_sftp: bool,
        pty_requests: Arc<AtomicUsize>,
        shell_requests: Arc<AtomicUsize>,
        subsystem_requests: Arc<AtomicUsize>,
        shell_behavior: TestShellBehavior,
        single_session_channel: bool,
    }

    impl server::Handler for SftpOnlyTestServer {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            if self.single_session_channel && !self.channels.lock().await.is_empty() {
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            }
            self.channels.lock().await.insert(channel.id(), channel);
            reply.accept().await;
            Ok(())
        }

        async fn channel_close(
            &mut self,
            channel: ChannelId,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.channels.lock().await.remove(&channel);
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.pty_requests.fetch_add(1, Ordering::SeqCst);
            session.channel_success(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.shell_requests.fetch_add(1, Ordering::SeqCst);
            match self.shell_behavior {
                TestShellBehavior::Reject => session.channel_failure(channel)?,
                TestShellBehavior::AcceptWithoutExec => session.channel_success(channel)?,
            }
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            _data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_failure(channel)?;
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            channel_id: ChannelId,
            name: &str,
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            if name != "sftp" {
                session.channel_failure(channel_id)?;
                return Ok(());
            }

            self.subsystem_requests.fetch_add(1, Ordering::SeqCst);
            if !self.allow_sftp {
                session.channel_failure(channel_id)?;
                return Ok(());
            }

            let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
                session.channel_failure(channel_id)?;
                return Ok(());
            };
            session.channel_success(channel_id)?;
            russh_sftp::server::run(channel.into_stream(), TestSftpSession::default()).await;
            Ok(())
        }
    }

    #[derive(Default)]
    struct TestSftpSession {
        root_dir_read_done: bool,
    }

    impl russh_sftp::server::Handler for TestSftpSession {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn init(
            &mut self,
            _version: u32,
            _extensions: HashMap<String, String>,
        ) -> Result<Version, Self::Error> {
            Ok(Version::new())
        }

        async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".to_string(),
                language_tag: "en-US".to_string(),
            })
        }

        async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
            self.root_dir_read_done = false;
            Ok(Handle { id, handle: path })
        }

        async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
            if handle == "/" && !self.root_dir_read_done {
                self.root_dir_read_done = true;
                return Ok(Name {
                    id,
                    files: vec![File::new("upload.txt", FileAttributes::default())],
                });
            }
            Err(StatusCode::Eof)
        }

        async fn realpath(&mut self, id: u32, _path: String) -> Result<Name, Self::Error> {
            Ok(Name {
                id,
                files: vec![File::dummy("/home/sftp")],
            })
        }
    }

    async fn start_sftp_only_test_connection(
        allow_sftp: bool,
        shell_behavior: TestShellBehavior,
        single_session_channel: bool,
    ) -> (
        client::Handle<SftpOnlyTestClient>,
        tokio::task::JoinHandle<()>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
        let channels = Arc::new(Mutex::new(HashMap::new()));
        let pty_requests = Arc::new(AtomicUsize::new(0));
        let shell_requests = Arc::new(AtomicUsize::new(0));
        let subsystem_requests = Arc::new(AtomicUsize::new(0));
        let mut rng = russh::keys::key::safe_rng();
        let server_config = Arc::new(server::Config {
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            keys: vec![
                russh::keys::PrivateKey::random(&mut rng, russh::keys::Algorithm::Ed25519)
                    .expect("test server key"),
            ],
            ..server::Config::default()
        });
        let server_channels = channels.clone();
        let server_pty_requests = pty_requests.clone();
        let server_shell_requests = shell_requests.clone();
        let server_subsystem_requests = subsystem_requests.clone();
        let server_task = tokio::spawn(async move {
            let session = server::run_stream(
                server_config,
                server_stream,
                SftpOnlyTestServer {
                    channels: server_channels,
                    allow_sftp,
                    pty_requests: server_pty_requests,
                    shell_requests: server_shell_requests,
                    subsystem_requests: server_subsystem_requests,
                    shell_behavior,
                    single_session_channel,
                },
            )
            .await
            .expect("fake SSH server handshake");
            assert_fake_server_session(session.await);
        });

        let mut handle = client::connect_stream(
            Arc::new(client::Config::default()),
            client_stream,
            SftpOnlyTestClient,
        )
        .await
        .expect("fake SSH client handshake");
        assert!(
            handle
                .authenticate_none("test")
                .await
                .expect("none authentication")
                .success()
        );
        (
            handle,
            server_task,
            pty_requests,
            shell_requests,
            subsystem_requests,
        )
    }

    async fn open_rejected_shell(handle: &mut client::Handle<SftpOnlyTestClient>) -> AppError {
        super::open_shell_channel(
            handle,
            "sftp-only-integration-test",
            None,
            false,
            "xterm-256color",
            true,
            false,
            SftpCwdFollowMode::Off,
            100,
            None,
        )
        .await
        .expect_err("test server must reject the interactive shell")
    }

    async fn probe_real_sftp(
        handle: &client::Handle<SftpOnlyTestClient>,
        verify_file_browser_operations: bool,
    ) -> Result<(), AppError> {
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open SFTP channel: {error}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| {
                AppError::Channel(format!("Failed to request SFTP subsystem: {error}"))
            })?;

        match timeout(Duration::from_secs(1), channel.wait()).await {
            Ok(Some(ChannelMsg::Success)) => {}
            Ok(Some(ChannelMsg::Failure)) => {
                return Err(AppError::Channel(
                    "SFTP subsystem request rejected by server".to_string(),
                ));
            }
            Ok(Some(_)) => {
                return Err(AppError::Channel(
                    "Unexpected SSH response to SFTP subsystem request".to_string(),
                ));
            }
            Ok(None) => {
                return Err(AppError::Channel(
                    "SSH channel closed during SFTP subsystem request".to_string(),
                ));
            }
            Err(_) => {
                return Err(AppError::Channel(
                    "SFTP subsystem request timed out".to_string(),
                ));
            }
        }

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
        if verify_file_browser_operations {
            assert_eq!(sftp.canonicalize(".").await?, "/home/sftp");
            let entries: Vec<_> = sftp.read_dir("/").await?.collect();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].file_name(), "upload.txt");
        }
        sftp.close().await?;
        Ok(())
    }

    async fn stop_sftp_only_test_connection(
        handle: &client::Handle<SftpOnlyTestClient>,
        server_task: tokio::task::JoinHandle<()>,
    ) {
        let _ = handle
            .disconnect(Disconnect::ByApplication, "test complete", "")
            .await;
        timeout(Duration::from_secs(1), server_task)
            .await
            .expect("fake SSH server should stop")
            .expect("fake SSH server task");
    }

    #[test]
    fn network_device_runtime_capabilities_disable_linux_only_features() {
        let config = test_config(SshProfile::NetworkDevice);

        let capabilities = resolve_runtime_capabilities(&config);

        assert!(!capabilities.remote_file_browser_enabled);
        assert!(!capabilities.remote_stats_enabled);
        assert!(capabilities.network_device_profile);
    }

    #[test]
    fn standard_runtime_capabilities_preserve_sftp_file_browser_choice() {
        let mut enabled = test_config(SshProfile::Standard);
        enabled.sftp.enabled = true;
        let mut disabled = test_config(SshProfile::Standard);
        disabled.sftp.enabled = false;

        assert!(resolve_runtime_capabilities(&enabled).remote_file_browser_enabled);
        assert!(!resolve_runtime_capabilities(&disabled).remote_file_browser_enabled);
        assert!(resolve_runtime_capabilities(&enabled).remote_stats_enabled);
    }

    #[test]
    fn terminal_runtime_capabilities_disable_background_ssh_features() {
        let mut config = test_config(SshProfile::Standard);
        config.runtime_mode = SshRuntimeMode::Terminal;

        let capabilities = resolve_runtime_capabilities(&config);

        assert!(!capabilities.remote_file_browser_enabled);
        assert!(!capabilities.remote_stats_enabled);
        assert!(!capabilities.network_device_profile);
    }

    #[test]
    fn sftp_runtime_enables_file_browser_for_network_device_profiles() {
        let mut config = test_config(SshProfile::NetworkDevice);
        config.runtime_mode = SshRuntimeMode::Sftp;

        let capabilities = resolve_runtime_capabilities(&config);

        assert!(capabilities.remote_file_browser_enabled);
        assert!(!capabilities.remote_stats_enabled);
        assert!(capabilities.network_device_profile);
    }

    #[test]
    fn explicit_sftp_runtime_rejects_disabled_sftp_before_connecting() {
        let mut config = test_config(SshProfile::Standard);
        config.runtime_mode = SshRuntimeMode::Sftp;
        config.sftp.enabled = false;

        let error = validate_runtime_config(&config).expect_err("disabled SFTP must fail");

        assert!(error.to_string().contains("SFTP is disabled"));
    }

    #[tokio::test]
    async fn explicit_sftp_runtime_requests_only_the_sftp_subsystem() {
        let (handle, server_task, pty_requests, shell_requests, subsystem_requests) =
            start_sftp_only_test_connection(true, TestShellBehavior::Reject, false).await;

        probe_real_sftp(&handle, true)
            .await
            .expect("explicit SFTP probe must succeed");

        assert_eq!(pty_requests.load(Ordering::SeqCst), 0);
        assert_eq!(shell_requests.load(Ordering::SeqCst), 0);
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 1);
        stop_sftp_only_test_connection(&handle, server_task).await;
    }

    #[tokio::test]
    async fn explicit_sftp_runtime_fails_when_subsystem_is_rejected() {
        let (handle, server_task, pty_requests, shell_requests, subsystem_requests) =
            start_sftp_only_test_connection(false, TestShellBehavior::Reject, false).await;

        let error = probe_real_sftp(&handle, false)
            .await
            .expect_err("rejected SFTP subsystem must fail session preparation");

        assert!(error.to_string().contains("rejected"));
        assert_eq!(pty_requests.load(Ordering::SeqCst), 0);
        assert_eq!(shell_requests.load(Ordering::SeqCst), 0);
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 1);
        stop_sftp_only_test_connection(&handle, server_task).await;
    }

    #[tokio::test]
    async fn standard_runtime_does_not_probe_sftp_after_shell_success() {
        let (mut handle, server_task, pty_requests, shell_requests, subsystem_requests) =
            start_sftp_only_test_connection(true, TestShellBehavior::AcceptWithoutExec, false)
                .await;

        let (channel, ..) = super::open_shell_channel(
            &mut handle,
            "standard-shell-success-test",
            None,
            false,
            "xterm-256color",
            true,
            false,
            SftpCwdFollowMode::Off,
            50,
            None,
        )
        .await
        .expect("standard Shell path must remain successful");

        assert_eq!(pty_requests.load(Ordering::SeqCst), 1);
        assert_eq!(shell_requests.load(Ordering::SeqCst), 1);
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 0);
        let _ = channel.close().await;
        stop_sftp_only_test_connection(&handle, server_task).await;
    }

    #[tokio::test]
    async fn sftp_only_fallback_accepts_sftp_when_shell_is_rejected() {
        let (mut handle, server_task, _pty_requests, _shell_requests, subsystem_requests) =
            start_sftp_only_test_connection(true, TestShellBehavior::Reject, true).await;
        let shell_error = open_rejected_shell(&mut handle).await;

        let result = try_sftp_only_fallback("test-session", true, shell_error, || {
            probe_real_sftp(&handle, true)
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 1);

        let manager = SessionManager::new();
        let (cmd_tx, _cmd_rx) = session_command_channel("test-session".to_string());
        let mut info = SessionInfo {
            id: "test-session".to_string(),
            name: "sftp-only".to_string(),
            session_type: SessionType::SSH,
            started_at: String::new(),
            connection_id: None,
            connected: true,
            owner_window_label: None,
            ai_execution_profile: crate::config::AiExecutionProfile::Posix,
            injection_active: true,
            cwd_tracking_active: false,
            dynamic_title_capabilities: DynamicTitleCapabilities::default(),
            remote_file_browser_enabled: false,
            remote_stats_enabled: true,
            ssh_profile: Some(SshProfile::Standard),
            ssh_runtime_mode: Some(SshRuntimeMode::Standard),
        };
        apply_sftp_only_runtime_policy(&mut info);
        manager
            .add_session(SessionHandle {
                info,
                cmd_tx,
                startup_input_barrier: None,
                ssh_config: None,
                ssh_handle: None,
                cwd: Arc::new(Mutex::new(Default::default())),
                remote_fs: None,
            })
            .await;
        let registered = manager.list_sessions().await;
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].session_type, SessionType::SSH);
        assert!(registered[0].connected);
        assert!(registered[0].remote_file_browser_enabled);
        assert!(!registered[0].remote_stats_enabled);
        assert_eq!(
            registered[0].ai_execution_profile,
            crate::config::AiExecutionProfile::Disabled
        );
        assert_eq!(registered[0].ssh_runtime_mode, Some(SshRuntimeMode::Sftp));

        stop_sftp_only_test_connection(&handle, server_task).await;
    }

    #[tokio::test]
    async fn interactive_shell_is_preserved_when_shell_detection_exec_is_rejected() {
        let (
            mut shell_handle,
            shell_server_task,
            _pty_requests,
            _shell_requests,
            subsystem_requests,
        ) = start_sftp_only_test_connection(true, TestShellBehavior::AcceptWithoutExec, false)
            .await;

        let (channel, injection_script, _, detected_shell, _) = super::open_shell_channel(
            &mut shell_handle,
            "interactive-shell-detection-rejection-test",
            None,
            false,
            "xterm-256color",
            true,
            false,
            SftpCwdFollowMode::ShellIntegration,
            50,
            None,
        )
        .await
        .expect("server accepts the shell request");
        assert!(injection_script.is_none());
        assert!(detected_shell.is_none());
        probe_real_sftp(&shell_handle, true)
            .await
            .expect("SFTP must remain available alongside the accepted interactive shell");
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 1);

        let _ = channel.close().await;
        stop_sftp_only_test_connection(&shell_handle, shell_server_task).await;
    }

    #[tokio::test]
    async fn sftp_only_fallback_preserves_shell_error_when_sftp_probe_fails() {
        let (mut handle, server_task, _pty_requests, _shell_requests, subsystem_requests) =
            start_sftp_only_test_connection(false, TestShellBehavior::Reject, false).await;
        let shell_error = open_rejected_shell(&mut handle).await;
        let shell_error_text = shell_error.to_string();
        let error = try_sftp_only_fallback("test-session", true, shell_error, || {
            probe_real_sftp(&handle, false)
        })
        .await
        .expect_err("SFTP probe failure must preserve the shell error");

        assert_eq!(error.to_string(), shell_error_text);
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 1);
        stop_sftp_only_test_connection(&handle, server_task).await;
    }

    #[tokio::test]
    async fn sftp_only_fallback_skips_probe_when_file_browser_is_disabled() {
        let (mut handle, server_task, _pty_requests, _shell_requests, subsystem_requests) =
            start_sftp_only_test_connection(true, TestShellBehavior::Reject, false).await;
        let shell_error = open_rejected_shell(&mut handle).await;
        let shell_error_text = shell_error.to_string();

        let error = try_sftp_only_fallback("test-session", false, shell_error, || {
            probe_real_sftp(&handle, false)
        })
        .await
        .expect_err("disabled file browser must not enable SFTP-only fallback");

        assert_eq!(error.to_string(), shell_error_text);
        assert_eq!(subsystem_requests.load(Ordering::SeqCst), 0);
        stop_sftp_only_test_connection(&handle, server_task).await;
    }

    #[test]
    fn sftp_only_runtime_policy_disables_shell_capabilities() {
        let mut info = SessionInfo {
            id: "test-session".to_string(),
            name: "test".to_string(),
            session_type: SessionType::SSH,
            started_at: String::new(),
            connection_id: None,
            connected: true,
            owner_window_label: None,
            ai_execution_profile: crate::config::AiExecutionProfile::Posix,
            injection_active: true,
            cwd_tracking_active: false,
            dynamic_title_capabilities: DynamicTitleCapabilities::default(),
            remote_file_browser_enabled: false,
            remote_stats_enabled: true,
            ssh_profile: Some(SshProfile::Standard),
            ssh_runtime_mode: Some(SshRuntimeMode::Standard),
        };

        apply_sftp_only_runtime_policy(&mut info);

        assert_eq!(info.session_type, SessionType::SSH);
        assert!(info.connected);
        assert!(info.remote_file_browser_enabled);
        assert!(!info.remote_stats_enabled);
        assert!(!info.injection_active);
        assert_eq!(
            info.ai_execution_profile,
            crate::config::AiExecutionProfile::Disabled
        );
        assert_eq!(info.ssh_runtime_mode, Some(SshRuntimeMode::Sftp));
    }

    #[test]
    fn ssh_runtime_mode_defaults_to_standard() {
        let config: SshConfig = serde_json::from_value(serde_json::json!({
            "name": "test",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "auth": { "type": "none" }
        }))
        .expect("ssh config");

        assert_eq!(config.runtime_mode, SshRuntimeMode::Standard);
    }
}
