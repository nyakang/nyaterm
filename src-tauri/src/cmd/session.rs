use crate::config;
use crate::core::ssh::{self, HostKeyVerifyManager, PendingAuthManager};
use crate::core::{
    self, QuickCommandsStore, RecordingManager, SessionCommand, SessionInfo, SessionManager,
};
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use crate::utils::fuzzy::{FuzzyResult, fuzzy_search_items};
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
pub async fn create_ssh_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: String,
) -> AppResult<String> {
    let ssh_config = ssh::load_saved_ssh_config(&app, &connection_id)?;

    let session_id = ssh::create_ssh_session(
        app,
        state.inner().clone(),
        ssh_config,
        Some(connection_id.clone()),
        Some(window.label().to_string()),
    )
    .await?;
    if let Err(error) = crate::storage::mark_connection_used(&connection_id) {
        tracing::warn!(connection_id, %error, "Failed to mark connection as recently used");
    }
    Ok(session_id)
}

#[tauri::command]
pub async fn create_local_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: Option<String>,
) -> AppResult<String> {
    let config = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        match conn.config {
            config::ConnectionType::LocalTerminal {
                shell_path,
                shell_args,
                working_dir,
                ..
            } => Some(core::LocalSessionConfig {
                shell_path,
                shell_args,
                working_dir,
                name: conn.name,
            }),
            _ => None,
        }
    } else {
        None
    };
    let session_id = core::create_local_session(
        app,
        state.inner().clone(),
        config,
        Some(window.label().to_string()),
    )
    .await?;
    if let Some(connection_id) = connection_id {
        if let Err(error) = crate::storage::mark_connection_used(&connection_id) {
            tracing::warn!(connection_id, %error, "Failed to mark connection as recently used");
        }
    }
    Ok(session_id)
}

#[tauri::command]
pub async fn create_telnet_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    connection_id: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    name: Option<String>,
) -> AppResult<String> {
    let (h, p, n, bs_mode) = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        match conn.config {
            config::ConnectionType::Telnet {
                host: ref ch,
                port: cp,
                backspace_mode,
                ..
            } => (ch.clone(), cp, conn.name.clone(), backspace_mode),
            _ => {
                return Err(AppError::Config(
                    "Connection is not a Telnet connection".to_string(),
                ));
            }
        }
    } else {
        (
            host.ok_or_else(|| AppError::Config("host is required".to_string()))?,
            port.unwrap_or(23),
            name.unwrap_or_else(|| "Telnet".to_string()),
            "del".to_string(),
        )
    };
    let marked_connection_id = connection_id.clone();
    let session_id =
        core::create_telnet_session(
            app,
            state.inner().clone(),
            h,
            p,
            connection_id,
            n,
            bs_mode,
            Some(window.label().to_string()),
        )
        .await?;
    if let Some(connection_id) = marked_connection_id {
        if let Err(error) = crate::storage::mark_connection_used(&connection_id) {
            tracing::warn!(connection_id, %error, "Failed to mark connection as recently used");
        }
    }
    Ok(session_id)
}

#[tauri::command]
pub async fn create_serial_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
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
                backspace_mode,
                ..
            } => core::SerialConfig {
                port_name,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
                name: conn.name,
                backspace_mode,
            },
            _ => {
                return Err(AppError::Config(
                    "Connection is not a Serial connection".to_string(),
                ));
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
            backspace_mode: "ctrl_h".to_string(),
        }
    };
    let marked_connection_id = connection_id.clone();
    let session_id =
        core::create_serial_session(
            app,
            state.inner().clone(),
            cfg,
            connection_id,
            Some(window.label().to_string()),
        )
        .await?;
    if let Some(connection_id) = marked_connection_id {
        if let Err(error) = crate::storage::mark_connection_used(&connection_id) {
            tracing::warn!(connection_id, %error, "Failed to mark connection as recently used");
        }
    }
    Ok(session_id)
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
pub async fn zmodem_accept_download(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    save_dir: String,
) -> AppResult<()> {
    state
        .send_command(
            &session_id,
            SessionCommand::ZmodemAcceptDownload {
                save_dir: std::path::PathBuf::from(save_dir),
            },
        )
        .await
}

#[tauri::command]
pub async fn zmodem_accept_upload(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    file_paths: Vec<String>,
) -> AppResult<()> {
    state
        .send_command(
            &session_id,
            SessionCommand::ZmodemAcceptUpload {
                files: file_paths
                    .into_iter()
                    .map(std::path::PathBuf::from)
                    .collect(),
            },
        )
        .await
}

#[tauri::command]
pub async fn zmodem_cancel(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::ZmodemCancel)
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

    observability::log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "session.lifecycle".to_string(),
        event: "session.close_requested".to_string(),
        message: "Closing session".to_string(),
        ids: Some(serde_json::json!({ "session_id": session_id.clone() })),
        data: None,
        error: None,
        client_timestamp: None,
    });

    let res = match state.send_command(&session_id, SessionCommand::Close).await {
        Err(AppError::SessionNotFound(_)) => Ok(()),
        other => other,
    };

    // Concurrently tidy up any downloaded/watcher temporary files stored in the OS temp directory
    tauri::async_runtime::spawn(async move {
        if let Ok(temp_dir) = app.path().temp_dir() {
            let session_temp_dir = temp_dir.join("nyaterm").join(&session_id_clone);
            if session_temp_dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&session_temp_dir).await {
                    observability::log_event(StructuredLog {
                        level: StructuredLogLevel::Warn,
                        domain: "session.lifecycle".to_string(),
                        event: "session.temp_cleanup_failed".to_string(),
                        message: "Failed to clean up session temp directory".to_string(),
                        ids: Some(serde_json::json!({ "session_id": session_id_clone })),
                        data: Some(serde_json::json!({
                            "temp_dir": session_temp_dir,
                        })),
                        error: Some(serde_json::json!({ "message": e.to_string() })),
                        client_timestamp: None,
                    });
                } else {
                    observability::log_event(StructuredLog {
                        level: StructuredLogLevel::Info,
                        domain: "session.lifecycle".to_string(),
                        event: "session.temp_cleanup_succeeded".to_string(),
                        message: "Cleaned up session temp directory".to_string(),
                        ids: Some(serde_json::json!({ "session_id": session_id_clone })),
                        data: Some(serde_json::json!({
                            "temp_dir": session_temp_dir,
                        })),
                        error: None,
                        client_timestamp: None,
                    });
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
pub async fn register_command_submission(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<()> {
    state
        .register_command_submission(&session_id, command)
        .await;
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
    min_command_length: Option<usize>,
    max_command_length: Option<usize>,
) -> AppResult<Vec<FuzzyResult>> {
    Ok(state
        .fuzzy_search(&pattern, limit, min_command_length, max_command_length)
        .await)
}

#[tauri::command]
pub async fn fuzzy_search_commands(
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
    pattern: String,
    limit: usize,
) -> AppResult<Vec<FuzzyResult>> {
    let cfg = state.snapshot();
    let items: Vec<(&str, &str)> = cfg
        .commands
        .iter()
        .map(|c| (c.label.as_str(), c.command.as_str()))
        .collect();
    Ok(fuzzy_search_items(
        &items,
        &pattern,
        "quickCommand",
        limit,
        None,
        None,
    ))
}

#[tauri::command]
pub async fn start_recording(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
    file_path: String,
    include_io_labels: bool,
    include_timestamps: bool,
) -> AppResult<()> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        mgr.start(
            &session_id,
            &file_path,
            include_io_labels,
            include_timestamps,
        )
    })
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
pub async fn save_session_transcript(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
    file_path: String,
    include_io_labels: bool,
    include_timestamps: bool,
) -> AppResult<String> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        mgr.save_transcript(
            &session_id,
            &file_path,
            include_io_labels,
            include_timestamps,
        )
    })
    .await
    .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn list_recording_sessions(
    state: tauri::State<'_, Arc<RecordingManager>>,
) -> AppResult<Vec<String>> {
    Ok(state.list_recording_sessions())
}

#[tauri::command]
pub async fn set_recording_memory_limit(
    state: tauri::State<'_, Arc<RecordingManager>>,
    max_bytes: usize,
) -> AppResult<()> {
    state.set_memory_limit(max_bytes);
    Ok(())
}

#[tauri::command]
pub async fn submit_otp_response(
    state: tauri::State<'_, Arc<PendingAuthManager>>,
    request_id: String,
    responses: Vec<String>,
) -> AppResult<()> {
    if state.respond(&request_id, Some(responses)).await {
        observability::log_event(StructuredLog {
            level: StructuredLogLevel::Info,
            domain: "security.flow".to_string(),
            event: "otp.response_received".to_string(),
            message: "Received OTP response from frontend".to_string(),
            ids: Some(serde_json::json!({ "request_id": request_id })),
            data: None,
            error: None,
            client_timestamp: None,
        });
        Ok(())
    } else {
        observability::log_event(StructuredLog {
            level: StructuredLogLevel::Warn,
            domain: "security.flow".to_string(),
            event: "otp.response_rejected".to_string(),
            message: "Rejected OTP response for missing request".to_string(),
            ids: Some(serde_json::json!({ "request_id": request_id.clone() })),
            data: None,
            error: None,
            client_timestamp: None,
        });
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
    let cancelled = state.respond(&request_id, None).await;
    observability::log_event(StructuredLog {
        level: if cancelled {
            StructuredLogLevel::Info
        } else {
            StructuredLogLevel::Warn
        },
        domain: "security.flow".to_string(),
        event: if cancelled {
            "otp.request_cancelled".to_string()
        } else {
            "otp.request_cancel_missing".to_string()
        },
        message: if cancelled {
            "Cancelled OTP request".to_string()
        } else {
            "OTP request was already missing when cancellation arrived".to_string()
        },
        ids: Some(serde_json::json!({ "request_id": request_id })),
        data: None,
        error: None,
        client_timestamp: None,
    });
    Ok(())
}

#[tauri::command]
pub async fn respond_host_key_verify(
    state: tauri::State<'_, Arc<HostKeyVerifyManager>>,
    request_id: String,
    accepted: bool,
) -> AppResult<()> {
    let resolved = state.respond(&request_id, accepted).await;
    observability::log_event(StructuredLog {
        level: if resolved {
            StructuredLogLevel::Info
        } else {
            StructuredLogLevel::Warn
        },
        domain: "security.flow".to_string(),
        event: if accepted {
            "host_key.accepted".to_string()
        } else {
            "host_key.rejected".to_string()
        },
        message: if resolved {
            format!(
                "Host key verification response received (accepted={})",
                accepted
            )
        } else {
            "Host key verification response for missing request".to_string()
        },
        ids: Some(serde_json::json!({ "request_id": request_id })),
        data: None,
        error: None,
        client_timestamp: None,
    });
    if resolved {
        Ok(())
    } else {
        Err(AppError::Auth(format!(
            "No pending host key verification with id '{}'",
            request_id
        )))
    }
}
