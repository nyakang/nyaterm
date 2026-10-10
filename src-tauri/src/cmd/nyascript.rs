use std::sync::Arc;

use crate::core::{NyaScriptManager, NyaScriptRunStatus};
use crate::error::{AppError, AppResult};

#[tauri::command]
pub async fn start_nyascript(
    window: tauri::WebviewWindow,
    manager: tauri::State<'_, Arc<NyaScriptManager>>,
    source: String,
    current_session_id: Option<String>,
) -> AppResult<String> {
    if !cfg!(target_os = "windows") {
        return Err(AppError::Unsupported(
            "NyaScript is currently available only on Windows desktop.".into(),
        ));
    }
    if !crate::window_state::is_main_window_label(window.label()) {
        return Err(AppError::Config(
            "NyaScript must be started from a NyaTerm main window.".into(),
        ));
    }
    manager
        .inner()
        .start_run(window.label().to_string(), source, current_session_id)
        .await
}

#[tauri::command]
pub async fn cancel_nyascript(
    manager: tauri::State<'_, Arc<NyaScriptManager>>,
    run_id: String,
) -> AppResult<()> {
    manager.cancel_run(&run_id).await
}

#[tauri::command]
pub async fn get_nyascript_status(
    manager: tauri::State<'_, Arc<NyaScriptManager>>,
    run_id: String,
) -> AppResult<NyaScriptRunStatus> {
    manager.run_status(&run_id).await
}

#[tauri::command]
pub async fn respond_nyascript_session_open(
    window: tauri::WebviewWindow,
    manager: tauri::State<'_, Arc<NyaScriptManager>>,
    request_id: String,
    session_id: Option<String>,
    error: Option<String>,
) -> AppResult<()> {
    if !crate::window_state::is_main_window_label(window.label()) {
        return Err(AppError::Config(
            "Only a NyaTerm main window can complete a NyaScript session-open request.".into(),
        ));
    }
    manager
        .respond_session_open(window.label(), &request_id, session_id, error)
        .await
}
