use crate::core::ai::{
    self, AiAuditLog, AiChatRequest, AiMessage, AiSession, AiStreamStart, AppendAiAuditRequest,
    CommandRiskRequest, CommandRiskResponse,
};
use crate::core::SessionManager;
use crate::error::AppResult;
use std::sync::Arc;

#[tauri::command]
pub fn start_ai_chat_stream(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    request: AiChatRequest,
) -> AppResult<AiStreamStart> {
    ai::start_chat_stream(app, state.inner().clone(), request)
}

#[tauri::command]
pub async fn list_ai_model_names(app: tauri::AppHandle) -> AppResult<Vec<ai::AiModelDiscovery>> {
    ai::list_model_names(&app).await
}

#[tauri::command]
pub fn cancel_ai_chat_stream(stream_id: String) -> AppResult<()> {
    ai::cancel_chat_stream(stream_id)
}

#[tauri::command]
pub fn check_command_risk(request: CommandRiskRequest) -> AppResult<CommandRiskResponse> {
    Ok(ai::check_command_risk(request))
}

#[tauri::command]
pub fn get_ai_sessions(app: tauri::AppHandle) -> AppResult<Vec<AiSession>> {
    ai::get_ai_sessions(&app)
}

#[tauri::command]
pub fn get_ai_messages(
    app: tauri::AppHandle,
    session_id: String,
) -> AppResult<Vec<AiMessage>> {
    ai::get_ai_messages(&app, session_id)
}

#[tauri::command]
pub fn clear_ai_history(app: tauri::AppHandle) -> AppResult<()> {
    ai::clear_ai_history(&app)
}

#[tauri::command]
pub fn append_ai_audit(
    app: tauri::AppHandle,
    request: AppendAiAuditRequest,
) -> AppResult<AiAuditLog> {
    ai::append_ai_audit(&app, request)
}

#[tauri::command]
pub fn get_ai_audit_logs(
    app: tauri::AppHandle,
    limit: Option<usize>,
) -> AppResult<Vec<AiAuditLog>> {
    ai::get_ai_audit_logs(&app, limit)
}
