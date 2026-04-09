use crate::error::AppResult;
use crate::runtime::SessionManager;
use crate::ssh::sftp;
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
) -> AppResult<Vec<sftp::FileEntry>> {
    sftp::list_remote_dir(state.inner().clone(), &session_id, &path).await
}

#[tauri::command]
pub async fn delete_remote_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
) -> AppResult<()> {
    sftp::delete_remote_file(state.inner().clone(), &session_id, &path).await
}

#[tauri::command]
pub async fn rename_remote_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    old_path: String,
    new_path: String,
) -> AppResult<()> {
    sftp::rename_remote_file(state.inner().clone(), &session_id, &old_path, &new_path).await
}

#[tauri::command]
pub async fn download_remote_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    remote_path: String,
    local_path: String,
) -> AppResult<()> {
    sftp::download_remote_file(
        app,
        state.inner().clone(),
        &session_id,
        &remote_path,
        &local_path,
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
) -> AppResult<()> {
    sftp::upload_local_file(
        app,
        state.inner().clone(),
        &session_id,
        &local_path,
        &remote_path,
    )
    .await
}

#[tauri::command]
pub async fn get_file_properties(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
) -> AppResult<sftp::FileProperties> {
    sftp::get_file_properties(state.inner().clone(), &session_id, &path).await
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
pub async fn download_remote_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    remote_path: String,
    local_path: String,
) -> AppResult<()> {
    sftp::download_remote_directory(
        app,
        state.inner().clone(),
        &session_id,
        &remote_path,
        &local_path,
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
) -> AppResult<()> {
    sftp::upload_local_directory(
        app,
        state.inner().clone(),
        &session_id,
        &local_path,
        &remote_path,
    )
    .await
}
