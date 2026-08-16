use crate::core::SessionManager;
use crate::core::sftp::duplicate::DuplicateChoice;
use crate::core::sftp::{self, TransferDuplicateManager};
use crate::error::{AppError, AppResult};
use std::sync::Arc;

#[tauri::command]
pub async fn get_home_dir(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<String> {
    sftp::get_home_dir(state.inner().clone(), &session_id).await
}

#[tauri::command]
pub async fn list_remote_dir(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    raw_path_token: Option<String>,
) -> AppResult<Vec<sftp::FileEntry>> {
    sftp::list_remote_dir(
        state.inner().clone(),
        &session_id,
        &path,
        raw_path_token.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn list_remote_child_directories(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    raw_path_token: Option<String>,
    show_hidden_files: bool,
) -> AppResult<Vec<sftp::DirectoryChild>> {
    sftp::list_remote_child_directories(
        state.inner().clone(),
        &session_id,
        &path,
        raw_path_token.as_deref(),
        show_hidden_files,
    )
    .await
}

#[tauri::command]
pub async fn delete_remote_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    raw_path_token: Option<String>,
) -> AppResult<()> {
    sftp::delete_remote_file(
        state.inner().clone(),
        &session_id,
        &path,
        raw_path_token.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn rename_remote_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    old_path: String,
    new_path: String,
    old_raw_path_token: Option<String>,
    new_raw_path_token: Option<String>,
) -> AppResult<()> {
    sftp::rename_remote_file(
        state.inner().clone(),
        &session_id,
        &old_path,
        &new_path,
        old_raw_path_token.as_deref(),
        new_raw_path_token.as_deref(),
    )
    .await
}

#[tauri::command]
pub fn sanitize_download_file_name(name: String) -> String {
    sftp::sanitize_download_file_name(&name)
}

#[tauri::command]
pub async fn download_remote_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: Option<String>,
) -> AppResult<()> {
    sftp::download_remote_file(
        app,
        state.inner().clone(),
        &session_id,
        &remote_path,
        &local_path,
        transfer_id,
    )
    .await
}

#[tauri::command]
pub async fn upload_local_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: Option<String>,
    duplicate_strategy_override: Option<String>,
) -> AppResult<()> {
    sftp::upload_local_file(
        app,
        state.inner().clone(),
        &session_id,
        &local_path,
        &remote_path,
        transfer_id,
        duplicate_strategy_override,
    )
    .await
}

#[tauri::command]
pub async fn get_file_properties(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    raw_path_token: Option<String>,
) -> AppResult<sftp::FileProperties> {
    sftp::get_file_properties(
        state.inner().clone(),
        &session_id,
        &path,
        raw_path_token.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn read_remote_file_text(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    max_bytes: u64,
) -> AppResult<sftp::RemoteTextFile> {
    sftp::read_remote_file_text(state.inner().clone(), &session_id, &path, max_bytes).await
}

#[tauri::command]
pub async fn read_remote_file_bytes(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    max_bytes: u64,
) -> AppResult<sftp::RemoteBinaryFile> {
    sftp::read_remote_file_bytes(state.inner().clone(), &session_id, &path, max_bytes).await
}

#[tauri::command]
pub async fn write_remote_file_text(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    content: String,
    expected_mtime: Option<u64>,
    expected_size: Option<u64>,
    force: Option<bool>,
) -> AppResult<sftp::WriteRemoteTextResult> {
    sftp::write_remote_file_text(
        state.inner().clone(),
        &session_id,
        &path,
        &content,
        expected_mtime,
        expected_size,
        force.unwrap_or(false),
    )
    .await
}

#[tauri::command]
pub async fn create_remote_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    mode: Option<String>,
) -> AppResult<()> {
    sftp::create_remote_file(state.inner().clone(), &session_id, &path, mode).await
}

#[tauri::command]
pub async fn create_remote_dir(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    mode: Option<String>,
) -> AppResult<()> {
    sftp::create_remote_dir(state.inner().clone(), &session_id, &path, mode).await
}

#[tauri::command]
pub async fn create_remote_symlink(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    link_path: String,
    target_path: String,
) -> AppResult<()> {
    sftp::create_remote_symlink(state.inner().clone(), &session_id, &link_path, &target_path).await
}

#[tauri::command]
pub async fn chmod_remote_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    mode: String,
) -> AppResult<()> {
    sftp::chmod_remote_file(state.inner().clone(), &session_id, &path, &mode).await
}

#[tauri::command]
pub async fn update_remote_file_attributes(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    raw_path_token: Option<String>,
    update: sftp::RemoteFileAttributeUpdate,
) -> AppResult<()> {
    sftp::update_remote_file_attributes(
        state.inner().clone(),
        &session_id,
        &path,
        raw_path_token.as_deref(),
        update,
    )
    .await
}

#[tauri::command]
pub async fn download_remote_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: Option<String>,
) -> AppResult<()> {
    sftp::download_remote_directory(
        app,
        state.inner().clone(),
        &session_id,
        &remote_path,
        &local_path,
        transfer_id,
    )
    .await
}

#[tauri::command]
pub async fn upload_local_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: Option<String>,
    duplicate_strategy_override: Option<String>,
) -> AppResult<()> {
    sftp::upload_local_directory(
        app,
        state.inner().clone(),
        &session_id,
        &local_path,
        &remote_path,
        transfer_id,
        duplicate_strategy_override,
    )
    .await
}

#[tauri::command]
pub async fn copy_file_entry(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    request: sftp::CopyFileEntryRequest,
) -> AppResult<()> {
    sftp::copy_file_entry(app, state.inner().clone(), request).await
}

#[tauri::command]
pub async fn move_remote_entries(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    source_session_id: String,
    target_session_id: String,
    target_dir: String,
    entries: Vec<sftp::RemoteMoveEntry>,
    duplicate_strategy: Option<String>,
) -> AppResult<()> {
    sftp::move_remote_entries(
        app,
        state.inner().clone(),
        &source_session_id,
        &target_session_id,
        &target_dir,
        entries,
        duplicate_strategy,
    )
    .await
}

#[tauri::command]
pub async fn pause_transfer(app: tauri::AppHandle, transfer_id: String) -> AppResult<()> {
    sftp::pause_transfer(app, &transfer_id).await
}

#[tauri::command]
pub async fn resume_transfer(app: tauri::AppHandle, transfer_id: String) -> AppResult<()> {
    sftp::resume_transfer(app, &transfer_id).await
}

#[tauri::command]
pub async fn cancel_transfer(app: tauri::AppHandle, transfer_id: String) -> AppResult<()> {
    sftp::cancel_transfer(app, &transfer_id).await
}

#[tauri::command]
pub async fn respond_transfer_duplicate(
    state: tauri::State<'_, Arc<TransferDuplicateManager>>,
    request_id: String,
    action: String,
) -> AppResult<()> {
    let choice = DuplicateChoice::from_action(&action)
        .ok_or_else(|| AppError::Config(format!("Invalid duplicate action: {action}")))?;
    if state.respond(&request_id, choice).await {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "No pending duplicate prompt with id '{request_id}'"
        )))
    }
}
