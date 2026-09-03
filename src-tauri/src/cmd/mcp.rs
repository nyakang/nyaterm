use std::sync::Arc;
use tauri::{Emitter, Manager};

use crate::core::mcp::{ApprovalDecision, McpClientConfigs, McpManager, McpRuntimeStatus};
use crate::error::{AppError, AppResult};

#[tauri::command]
pub async fn get_external_mcp_status(
    manager: tauri::State<'_, Arc<McpManager>>,
) -> Result<McpRuntimeStatus, String> {
    Ok(manager.status().await)
}

#[tauri::command]
pub async fn notify_mcp_session_restore_complete(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<McpManager>>,
    owner_window_label: String,
) -> AppResult<McpRuntimeStatus> {
    validate_owner_window(&app, &owner_window_label)?;
    manager
        .inner()
        .session_restore_complete(&owner_window_label)
        .await
}

#[tauri::command]
pub async fn set_external_mcp_enabled(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<McpManager>>,
    enabled: bool,
    owner_window_label: String,
) -> AppResult<McpRuntimeStatus> {
    validate_owner_window(&app, &owner_window_label)?;
    let mut settings = crate::config::load_app_settings(&app)?.ai.external_mcp;
    settings.enabled = enabled;
    if enabled {
        let status = manager
            .inner()
            .configure_external(settings, &owner_window_label)
            .await?;
        if let Err(error) = crate::storage::update_settings_doc(
            crate::storage::SettingsDocKey::AppSettings,
            |stored: &mut crate::config::AppSettings| {
                stored.ai.external_mcp.enabled = true;
                Ok(())
            },
        ) {
            let _ = manager.disable_external(false).await;
            return Err(error);
        }
        let _ = app.emit("settings-changed", ());
        Ok(status)
    } else {
        if let Err(error) = manager.disable_external(true).await {
            settings.enabled = true;
            let _ = manager
                .inner()
                .configure_external(settings, &owner_window_label)
                .await;
            return Err(error);
        }
        Ok(manager.status().await)
    }
}

#[tauri::command]
pub async fn respond_external_mcp_approval(
    manager: tauri::State<'_, Arc<McpManager>>,
    request_id: String,
    decision: String,
) -> AppResult<()> {
    let decision = match decision.as_str() {
        "deny" => ApprovalDecision::Deny,
        "allow_once" => ApprovalDecision::AllowOnce,
        "allow_session" => ApprovalDecision::AllowSession,
        _ => return Err(AppError::Config("Invalid MCP approval decision.".into())),
    };
    manager.respond_approval(&request_id, decision).await
}

#[tauri::command]
pub async fn report_mcp_active_session(
    window: tauri::WebviewWindow,
    manager: tauri::State<'_, Arc<McpManager>>,
    session_id: Option<String>,
) -> AppResult<()> {
    if !crate::window_state::is_main_window_label(window.label()) {
        return Err(AppError::Config(
            "Only a NyaTerm main window can report its active MCP session.".into(),
        ));
    }
    manager.set_active_session(window.label(), session_id).await
}

#[tauri::command]
pub async fn respond_mcp_session_open(
    window: tauri::WebviewWindow,
    manager: tauri::State<'_, Arc<McpManager>>,
    request_id: String,
    session_id: Option<String>,
    error: Option<String>,
) -> AppResult<()> {
    if !crate::window_state::is_main_window_label(window.label()) {
        return Err(AppError::Config(
            "Only a NyaTerm main window can complete an MCP session-open request.".into(),
        ));
    }
    manager
        .respond_session_open(window.label(), &request_id, session_id, error)
        .await
}

#[tauri::command]
pub fn get_external_mcp_client_configs(
    manager: tauri::State<'_, Arc<McpManager>>,
) -> AppResult<McpClientConfigs> {
    manager.client_configs()
}

pub fn validate_owner_window(app: &tauri::AppHandle, label: &str) -> AppResult<()> {
    if !crate::window_state::is_main_window_label(label) || app.get_webview_window(label).is_none()
    {
        return Err(AppError::Config(
            "A valid owner main-window label is required for External MCP.".into(),
        ));
    }
    Ok(())
}
