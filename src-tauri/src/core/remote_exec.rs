use crate::core::SessionManager;
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use russh::ChannelMsg;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct RemoteCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
}

pub async fn exec_ssh_session_command(
    manager: &Arc<SessionManager>,
    session_id: &str,
    command: &[u8],
    timeout: Duration,
) -> AppResult<RemoteCommandOutput> {
    // Use an independent SSH connection if possible, so that opening an exec
    // channel doesn't kill the shell session on servers that don't support
    // multiple channels (e.g. dropbear/Termux on port 8022).
    let ssh_handle = get_or_create_independent_handle(manager, session_id).await?;
    exec_ssh_command(&ssh_handle, command, timeout).await
}

/// Returns an independent SSH handle for the session, creating one if needed.
///
/// Falls back to the session's shared handle when a new connection cannot be
/// established (e.g. temporary sessions without a connection_id).
async fn get_or_create_independent_handle(
    manager: &Arc<SessionManager>,
    session_id: &str,
) -> AppResult<Arc<SshConnectionHandles>> {
    let (connection_id, shared_handle) = {
        let sessions = manager.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(format!("Session '{session_id}' not found")))?;

        let shared_handle = session
            .ssh_handle
            .as_ref()
            .ok_or_else(|| AppError::Config("Not an SSH session".to_string()))?
            .clone()
            .downcast::<SshConnectionHandles>()
            .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?;

        let connection_id = session
            .ssh_config
            .as_ref()
            .and_then(|cfg| cfg.downcast_ref::<crate::core::ssh::SshConfig>())
            .and_then(|cfg| cfg.connection_id.clone());

        (connection_id, shared_handle)
    };

    if let (Some(conn_id), Some(app)) = (&connection_id, manager.app()) {
        match crate::core::ssh::create_ssh_handle(&app, conn_id).await {
            Ok(handle) => {
                tracing::info!(
                    session_id,
                    "Created independent SSH connection for remote exec"
                );
                return Ok(handle);
            }
            Err(error) => {
                tracing::warn!(
                    session_id,
                    %error,
                    "Failed to create independent SSH connection for remote exec, \
                     falling back to shared connection"
                );
            }
        }
    }

    Ok(shared_handle)
}

async fn exec_ssh_command(
    ssh_handle: &Arc<SshConnectionHandles>,
    command: &[u8],
    timeout: Duration,
) -> AppResult<RemoteCommandOutput> {
    let handle_mtx = ssh_handle.target_handle();

    tokio::time::timeout(timeout, async {
        let mut channel = {
            let handle = handle_mtx.lock().await;
            handle
                .channel_open_session()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to open channel: {e}")))?
        };

        channel
            .exec(true, command)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to execute command: {e}")))?;

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_status = None;

        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { ref data }) => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                    stderr.push_str(&String::from_utf8_lossy(data));
                }
                Some(ChannelMsg::ExitStatus {
                    exit_status: status,
                }) => {
                    exit_status = Some(status);
                }
                Some(ChannelMsg::Eof) | None => break,
                _ => {}
            }
        }

        Ok::<RemoteCommandOutput, AppError>(RemoteCommandOutput {
            stdout,
            stderr,
            exit_status,
        })
    })
    .await
    .map_err(|_| AppError::Channel("Remote command timed out".to_string()))?
}

pub fn ensure_success(
    output: RemoteCommandOutput,
    context: &str,
) -> AppResult<RemoteCommandOutput> {
    if matches!(output.exit_status, Some(0) | None) {
        return Ok(output);
    }

    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "remote command failed"
    };

    Err(AppError::Channel(format!("{context}: {detail}")))
}

pub fn sh_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
