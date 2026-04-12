use super::auth::{authenticate_handle, load_saved_ssh_config};
use super::client::{
    build_client_config, connect_via_stream, connect_with_proxy, SshConfig, SshConnectionHandles,
    SshHandle, SshHandler, SshRawHandle,
};
use super::io::{open_shell_channel, ssh_io_loop};
use crate::core::{
    SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, SharedCwd,
};
use crate::error::{AppError, AppResult};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

async fn create_authenticated_connection(
    app: &AppHandle,
    config: &SshConfig,
) -> AppResult<SshHandle> {
    let ssh_client_config = Arc::new(build_client_config(app));

    if let Some(jump_config) = config.proxy_jump.as_deref() {
        tracing::info!(
            jump_host = %jump_config.host,
            jump_port = jump_config.port,
            target_host = %config.host,
            target_port = config.port,
            "Creating SSH connection via ProxyJump"
        );

        let jump_handler = SshHandler::new(app.clone(), jump_config.host.clone(), jump_config.port);
        let mut jump_handle =
            connect_with_proxy(jump_config, ssh_client_config.clone(), jump_handler).await?;
        let jump_password_error = format!(
            "Authentication failed for jump host '{}': invalid credentials",
            jump_config.name
        );
        let jump_key_error = format!(
            "Authentication failed for jump host '{}': key rejected",
            jump_config.name
        );
        authenticate_handle(
            &mut jump_handle,
            jump_config,
            app,
            &jump_password_error,
            &jump_key_error,
        )
        .await?;

        let channel = jump_handle
            .channel_open_direct_tcpip(&config.host, config.port.into(), "127.0.0.1", 0)
            .await
            .map_err(|error| {
                AppError::Channel(format!("Failed to open ProxyJump channel: {}", error))
            })?;

        let target_handler = SshHandler::new(app.clone(), config.host.clone(), config.port);
        let mut target_handle =
            connect_via_stream(channel.into_stream(), ssh_client_config, target_handler).await?;
        authenticate_handle(
            &mut target_handle,
            config,
            app,
            "Authentication failed: invalid credentials",
            "Authentication failed: key rejected",
        )
        .await?;

        let target_handle: SshRawHandle = Arc::new(tokio::sync::Mutex::new(target_handle));
        let jump_handle: SshRawHandle = Arc::new(tokio::sync::Mutex::new(jump_handle));
        return Ok(Arc::new(SshConnectionHandles::new(
            target_handle,
            Some(jump_handle),
        )));
    }

    let handler = SshHandler::new(app.clone(), config.host.clone(), config.port);
    let mut handle = connect_with_proxy(config, ssh_client_config, handler).await?;
    authenticate_handle(
        &mut handle,
        config,
        app,
        "Authentication failed: invalid credentials",
        "Authentication failed: key rejected",
    )
    .await?;

    let handle: SshRawHandle = Arc::new(tokio::sync::Mutex::new(handle));
    Ok(Arc::new(SshConnectionHandles::new(handle, None)))
}

/// Creates an authenticated SSH handle for a saved connection without opening a PTY/shell.
/// Used by tunnels to establish their own independent SSH connections.
pub async fn create_ssh_handle(app: &AppHandle, connection_id: &str) -> AppResult<SshHandle> {
    let ssh_config = load_saved_ssh_config(app, connection_id)?;
    let handle = create_authenticated_connection(app, &ssh_config).await?;

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
) -> AppResult<String> {
    tracing::info!(
        host = %config.host,
        port = config.port,
        user = %config.username,
        "Creating SSH session"
    );

    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();

    let ssh_connection = create_authenticated_connection(&app, &config).await?;
    let handle_mtx = ssh_connection.target_handle();
    let mut handle = handle_mtx.lock().await;

    let (channel, injection_script, ready_marker) =
        open_shell_channel(&mut handle, &session_id).await?;
    drop(handle);
    let injection_active = injection_script.is_some();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::SSH,
        connected: true,
        injection_active,
    };

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(None));
    let ssh_config_arc: Arc<dyn std::any::Any + Send + Sync> = Arc::new(config.clone());
    let ssh_handle_arc: Arc<dyn std::any::Any + Send + Sync> = ssh_connection.clone();

    let session_handle = SessionHandle {
        info: session_info,
        cmd_tx,
        ssh_config: Some(ssh_config_arc),
        ssh_handle: Some(ssh_handle_arc),
        cwd: cwd.clone(),
    };
    manager.add_session(session_handle).await;

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
    tokio::spawn(async move {
        ssh_io_loop(
            app,
            io_session_id,
            io_manager,
            channel,
            io_handle,
            cmd_rx,
            cwd,
            io_connection_id,
            injection_script,
            ready_marker,
        )
        .await;
    });

    tracing::info!(session_id = %session_id, "SSH session created");
    Ok(session_id)
}
