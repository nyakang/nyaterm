use crate::core::SessionManager;
use crate::core::monitoring::stats::{
    RemoteStats, RemoteStatsSampler, build_stats_script, parse_stats_output,
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
    // Auto-icon detection and the monitor can request the same new session concurrently.
    // Serializing per session avoids duplicate static/disk probes without blocking other hosts.
    let lease = sampler.lock_session(&session_id).await;
    if !lease.is_current() {
        return Err(AppError::SessionNotFound(format!(
            "Session '{session_id}' was closed while waiting for remote stats"
        )));
    }

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

    let probe_plan = sampler.probe_plan(&session_id).await;
    let script = build_stats_script(probe_plan);

    let output = exec_ssh_session_command(
        state.inner(),
        &session_id,
        script.as_bytes(),
        Duration::from_secs(15),
    )
    .await?;
    let output = ensure_success(output, "Failed to fetch stats")?;
    let parsed = parse_stats_output(&output.stdout)?;

    Ok(sampler
        .complete_snapshot_with_lease(&session_id, &lease, parsed)
        .await)
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

/// 在 su/sudo su 写入终端前，静默准备目标用户的目录跟随配置。
#[tauri::command]
pub async fn prepare_terminal_cwd_tracking(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<bool> {
    let session = state.session_info(&session_id).await?;
    let Some(connection_id) = session.connection_id else {
        return Ok(false);
    };
    let settings = crate::config::load_app_settings(&app)?;
    if !settings
        .ui
        .file_explorer_auto_sync_cwd_connection_ids
        .contains(&connection_id)
    {
        return Ok(false);
    }

    crate::core::ssh::prepare_terminal_cwd_tracking_for_user_switch(
        state.inner().clone(),
        &session_id,
        &command,
    )
    .await
}
