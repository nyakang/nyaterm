use crate::core::SessionManager;
use crate::core::monitoring::stats::{
    RemoteStats, RemoteStatsSampler, SYSINFO_SCRIPT, parse_stats_output,
};
use crate::core::remote_exec::{ensure_success, exec_ssh_session_command};
use crate::error::{AppError, AppResult};
use std::sync::Arc;
use std::time::Duration;

#[tauri::command]
pub async fn get_remote_stats(
    state: tauri::State<'_, Arc<SessionManager>>,
    sampler: tauri::State<'_, Arc<RemoteStatsSampler>>,
    session_id: String,
) -> AppResult<RemoteStats> {
    let remote_stats_enabled = {
        let sessions = state.sessions.lock().await;
        let session = sessions.get(&session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", session_id))
        })?;
        session.info.remote_stats_enabled
    };
    if !remote_stats_enabled {
        return Err(AppError::Config(
            "Remote stats are disabled for this session profile".to_string(),
        ));
    }

    let output = exec_ssh_session_command(
        state.inner(),
        &session_id,
        SYSINFO_SCRIPT.as_bytes(),
        Duration::from_secs(15),
    )
    .await?;
    let output = ensure_success(output, "Failed to fetch stats")?;
    let parsed = parse_stats_output(&output.stdout)?;

    Ok(sampler.complete_snapshot(&session_id, parsed).await)
}

#[tauri::command]
pub async fn get_terminal_cwd(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<String> {
    let cwd_arc = {
        let sessions = state.sessions.lock().await;
        let session = sessions.get(&session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", session_id))
        })?;
        session.cwd.clone()
    };

    let cached = cwd_arc.lock().await;
    if let Some(cwd) = cached.operational_path.as_ref() {
        return Ok(cwd.clone());
    }

    Err(AppError::Config(
        "Working directory is not available for this session. Terminal path sync is only available when the backend receives directory updates from the session.".to_string(),
    ))
}

#[tauri::command]
pub async fn try_get_terminal_cwd(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<Option<String>> {
    let cwd_arc = {
        let sessions = state.sessions.lock().await;
        let session = sessions.get(&session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", session_id))
        })?;
        session.cwd.clone()
    };

    Ok(cwd_arc.lock().await.operational_path.clone())
}
