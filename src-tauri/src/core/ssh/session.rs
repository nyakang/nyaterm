use super::agent_broker::AgentBrokerFactory;
use super::auth::{SSH_AGENT_AUTH_RETRY, authenticate_handle, load_saved_ssh_config};
use super::client::{
    RemoteForwardOpen, SshConfig, SshConnectionHandles, SshDiagnosticContext, SshDiagnosticStage,
    SshHandle, SshHandler, SshRawHandle, SshStartupCommand, build_client_config,
    connect_via_stream, connect_with_proxy,
};
use super::io::{open_shell_channel, ssh_io_loop};
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
    SshRuntimeCapabilities {
        remote_file_browser_enabled: config.sftp.enabled
            && !network_device_profile
            && !terminal_only,
        remote_stats_enabled: !network_device_profile && !terminal_only,
        network_device_profile,
    }
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

    let x11_config = if config.x11_forwarding {
        Some(super::x11_forwarding::prepare_x11_forwarding(&config.x11_display).await)
    } else {
        None
    };
    let (ssh_connection, x11_rx) = loop {
        match create_authenticated_connection_with_notifications(
            &app,
            &config,
            None,
            None,
            true,
            Some(diagnostics.clone()),
        )
        .await
        {
            Err(error) if is_agent_auth_retry(&error) => continue,
            result => break result,
        }
    }?;
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
        "SSH session initialization starting"
    );
    let handle_mtx = ssh_connection.target_handle();
    let mut handle = handle_mtx.lock().await;
    let forwarding_enabled =
        should_attach_agent_forwarding(true, effective_forwarding_config(&config).enabled);

    let (channel, injection_script, ready_marker, detected_shell, initial_notice) =
        open_shell_channel(
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
        .await?;
    drop(handle);
    let injection_active = injection_script.is_some();

    if let (Some(rx), Some(x11_config)) = (x11_rx, x11_config) {
        super::x11_forwarding::spawn_x11_forwarder(app.clone(), session_id.clone(), rx, x11_config);
    }

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::SSH,
        started_at: crate::core::now_session_started_at(),
        connection_id: connection_id.clone(),
        connected: true,
        owner_window_label,
        ai_execution_profile: AiExecutionProfile::Posix,
        injection_active,
        dynamic_title_capabilities: DynamicTitleCapabilities::new(config.dynamic_tab_title, None),
        remote_file_browser_enabled: capabilities.remote_file_browser_enabled,
        remote_stats_enabled: capabilities.remote_stats_enabled,
        ssh_profile: Some(config.ssh_profile.clone()),
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
        remote_fs: None,
    };
    manager.add_session(session_handle).await;
    tracing::info!(session_id = %session_id, "SSH session registered");
    if let Some(hook) = session_ready_hook.as_ref() {
        hook(&session_info);
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
    let io_manager = manager.clone();
    let io_handle = ssh_connection.clone();
    let io_connection_id = connection_id.clone();
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

/// Opens a new PTY shell channel on an existing authenticated SSH connection.
pub async fn create_multiplexed_ssh_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    source_session_id: &str,
    startup_command: Option<SshStartupCommand>,
    session_ready_hook: Option<SessionReadyHook>,
) -> AppResult<String> {
    let (config, ssh_connection, owner_window_label) = {
        let sessions = manager.sessions.lock().await;
        let source = sessions.get(source_session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", source_session_id))
        })?;

        if source.info.session_type != SessionType::SSH {
            return Err(AppError::Config(
                "Source session is not an SSH session".to_string(),
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

        (
            config,
            ssh_connection,
            source.info.owner_window_label.clone(),
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
        dynamic_title_capabilities: DynamicTitleCapabilities::new(config.dynamic_tab_title, None),
        remote_file_browser_enabled: capabilities.remote_file_browser_enabled,
        remote_stats_enabled: capabilities.remote_stats_enabled,
        ssh_profile: Some(config.ssh_profile.clone()),
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
        remote_fs: None,
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
        is_agent_auth_retry, is_raw_agent_forwarding_config, resolve_runtime_capabilities,
        should_attach_agent_forwarding,
    };
    use crate::config::{
        SftpSettings, SshAgentEndpoint, SshAgentForwardingConfig, SshAgentForwardingPolicy,
        SshAgentForwardingSources, SshProfile, SshRuntimeMode, SshTerminalType,
    };
    use crate::core::ssh::client::{SshAuth, SshConfig};
    use crate::error::AppError;

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
