use super::auth::{authenticate_handle, authenticate_handle_with_otp, load_saved_ssh_config};
use super::client::{build_client_config, connect_with_proxy, SshConfig, SshHandle, SshHandler};
use super::io::{open_shell_channel, ssh_io_loop};
use crate::error::AppResult;
use crate::core::{
    SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, SharedCwd,
};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

/// Creates an authenticated SSH handle for a saved connection without opening a PTY/shell.
/// Used by tunnels to establish their own independent SSH connections.
pub async fn create_ssh_handle(app: &AppHandle, connection_id: &str) -> AppResult<SshHandle> {
    let ssh_config = load_saved_ssh_config(app, connection_id)?;
    let handler = SshHandler::new(app.clone(), ssh_config.host.clone(), ssh_config.port);
    let mut handle =
        connect_with_proxy(&ssh_config, Arc::new(build_client_config(app)), handler).await?;

    authenticate_handle(
        &mut handle,
        &ssh_config,
        app,
        "Invalid credentials",
        "Key authentication rejected",
    )
    .await?;

    tracing::info!(
        host = %ssh_config.host,
        port = ssh_config.port,
        "Tunnel SSH handle created"
    );

    Ok(Arc::new(tokio::sync::Mutex::new(handle)))
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

    let handler = SshHandler::new(app.clone(), config.host.clone(), config.port);
    let mut handle =
        connect_with_proxy(&config, Arc::new(build_client_config(&app)), handler).await?;

    authenticate_handle_with_otp(
        &mut handle,
        &config,
        &app,
        "Authentication failed: invalid credentials",
        "Authentication failed: key rejected",
        connection_id.as_deref(),
    )
    .await?;

    let (channel, injection_script, ready_marker) =
        open_shell_channel(&mut handle, &session_id).await?;
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
    let handle_mtx: SshHandle = Arc::new(tokio::sync::Mutex::new(handle));
    let ssh_handle_arc: Arc<dyn std::any::Any + Send + Sync> = handle_mtx.clone();

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
    let io_handle = handle_mtx.clone();
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
