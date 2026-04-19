use crate::config;
use crate::core::ssh::{self, PendingAuthManager};
use crate::core::{self, RecordingManager, SessionCommand, SessionInfo, SessionManager};
use crate::error::{AppError, AppResult};
use crate::utils::fuzzy::{fuzzy_search_items, FuzzyResult};
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
pub async fn create_ssh_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: String,
) -> AppResult<String> {
    let ssh_config = ssh::load_saved_ssh_config(&app, &connection_id)?;

    ssh::create_ssh_session(app, state.inner().clone(), ssh_config, Some(connection_id)).await
}

#[tauri::command]
pub async fn create_local_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: Option<String>,
) -> AppResult<String> {
    let config = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        match conn.config {
            config::ConnectionType::LocalTerminal {
                shell_path,
                working_dir,
            } => Some(core::LocalSessionConfig {
                shell_path,
                working_dir,
                name: conn.name,
            }),
            _ => None,
        }
    } else {
        None
    };
    core::create_local_session(app, state.inner().clone(), config).await
}

#[tauri::command]
pub async fn create_telnet_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    name: Option<String>,
) -> AppResult<String> {
    let (h, p, n) = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        match conn.config {
            config::ConnectionType::Telnet {
                host: ref ch,
                port: cp,
            } => (ch.clone(), cp, conn.name.clone()),
            _ => {
                return Err(AppError::Config(
                    "Connection is not a Telnet connection".to_string(),
                ))
            }
        }
    } else {
        (
            host.ok_or_else(|| AppError::Config("host is required".to_string()))?,
            port.unwrap_or(23),
            name.unwrap_or_else(|| "Telnet".to_string()),
        )
    };
    core::create_telnet_session(app, state.inner().clone(), h, p, connection_id, n).await
}

#[tauri::command]
pub async fn create_serial_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: Option<String>,
    port_name: Option<String>,
    baud_rate: Option<u32>,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<String>,
    name: Option<String>,
) -> AppResult<String> {
    let cfg = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        match conn.config {
            config::ConnectionType::Serial {
                port_name,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
            } => core::SerialConfig {
                port_name,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
                name: conn.name,
            },
            _ => {
                return Err(AppError::Config(
                    "Connection is not a Serial connection".to_string(),
                ))
            }
        }
    } else {
        core::SerialConfig {
            port_name: port_name
                .ok_or_else(|| AppError::Config("port_name is required".to_string()))?,
            baud_rate: baud_rate.unwrap_or(115_200),
            data_bits: data_bits.unwrap_or(8),
            parity: parity.unwrap_or_else(|| "none".to_string()),
            stop_bits: stop_bits.unwrap_or_else(|| "1".to_string()),
            name: name.unwrap_or_else(|| "Serial".to_string()),
        }
    };
    core::create_serial_session(app, state.inner().clone(), cfg, connection_id).await
}

#[tauri::command]
pub fn list_serial_ports() -> AppResult<Vec<String>> {
    core::list_serial_ports()
}

#[tauri::command]
pub async fn write_to_session(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    data: String,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::Write(data.into_bytes()))
        .await
}

#[tauri::command]
pub async fn resize_session(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::Resize { cols, rows })
        .await
}

#[tauri::command]
pub async fn attach_session(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::Attach)
        .await
}

#[tauri::command]
pub async fn close_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    let session_id_clone = session_id.clone();

    let res = match state.send_command(&session_id, SessionCommand::Close).await {
        Err(AppError::SessionNotFound(_)) => Ok(()),
        other => other,
    };

    // Concurrently tidy up any downloaded/watcher temporary files stored in the OS temp directory
    tauri::async_runtime::spawn(async move {
        if let Ok(temp_dir) = app.path().temp_dir() {
            let session_temp_dir = temp_dir.join("dragonfly").join(&session_id_clone);
            if session_temp_dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&session_temp_dir).await {
                    tracing::warn!(
                        "Failed to clean up temp directory {}: {}",
                        session_temp_dir.display(),
                        e
                    );
                } else {
                    tracing::info!(
                        "Successfully cleaned up temp directory for session: {}",
                        session_id_clone
                    );
                }
            }
        }
    });

    res
}

#[tauri::command]
pub async fn list_sessions(
    state: tauri::State<'_, Arc<SessionManager>>,
) -> AppResult<Vec<SessionInfo>> {
    Ok(state.list_sessions().await)
}

#[tauri::command]
pub async fn add_command_history(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<()> {
    state.add_command(&session_id, command).await;
    Ok(())
}

#[tauri::command]
pub async fn get_command_history(
    state: tauri::State<'_, Arc<SessionManager>>,
) -> AppResult<Vec<String>> {
    Ok(state.get_all_history().await)
}

#[tauri::command]
pub async fn fuzzy_search_history(
    state: tauri::State<'_, Arc<SessionManager>>,
    pattern: String,
    limit: usize,
) -> AppResult<Vec<FuzzyResult>> {
    Ok(state.fuzzy_search(&pattern, limit).await)
}

#[tauri::command]
pub async fn fuzzy_search_commands(
    app: tauri::AppHandle,
    pattern: String,
    limit: usize,
) -> AppResult<Vec<FuzzyResult>> {
    tokio::task::spawn_blocking(move || {
        let cfg = config::load_quick_commands(&app)?;
        let items: Vec<(&str, &str)> = cfg
            .commands
            .iter()
            .map(|c| (c.label.as_str(), c.command.as_str()))
            .collect();
        Ok(fuzzy_search_items(&items, &pattern, "quickCommand", limit))
    })
    .await
    .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn start_recording(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
    file_path: String,
) -> AppResult<()> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || mgr.start(&session_id, &file_path))
        .await
        .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn stop_recording(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
) -> AppResult<String> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || mgr.stop(&session_id))
        .await
        .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn is_recording(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
) -> AppResult<bool> {
    Ok(state.is_recording(&session_id))
}

#[tauri::command]
pub async fn submit_otp_response(
    state: tauri::State<'_, Arc<PendingAuthManager>>,
    request_id: String,
    responses: Vec<String>,
) -> AppResult<()> {
    if state.respond(&request_id, Some(responses)).await {
        Ok(())
    } else {
        Err(AppError::Auth(format!(
            "No pending OTP request with id '{}'",
            request_id
        )))
    }
}

#[tauri::command]
pub async fn cancel_otp_request(
    state: tauri::State<'_, Arc<PendingAuthManager>>,
    request_id: String,
) -> AppResult<()> {
    state.respond(&request_id, None).await;
    Ok(())
}
