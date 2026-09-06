//! Web-mode file-transfer glue.
//!
//! Browser file dialogs cannot produce server-side filesystem paths, so the
//! frontend dialog shims produce marker paths instead:
//!
//! - `__web_upload__/<staged name>` — a file the browser previously uploaded
//!   via `POST /api/sftp/staging`
//! - `__web_downloads__/<file name>` — a download target chosen in the
//!   browser's save dialog
//!
//! The RPC handlers for `download_remote_file` / `upload_local_file` detect
//! these markers and resolve them against server-side staging/download
//! directories, keeping the existing frontend transfer flows (and progress
//! events) unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, Query as AxumQuery, State as AxumState};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tokio_util::io::ReaderStream;

use super::rpc::{HandlerOutput, ServerState};
use crate::core::SessionManager;

pub(crate) const WEB_UPLOAD_PREFIX: &str = "__web_upload__/";
pub(crate) const WEB_DOWNLOAD_PREFIX: &str = "__web_downloads__/";

pub(crate) fn staging_dir(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    app.state::<crate::runtime::AppRuntime>()
        .config_dir()
        .join("web-staging")
}

pub(crate) fn download_dir(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    app.state::<crate::runtime::AppRuntime>()
        .config_dir()
        .join("web-downloads")
}

/// Removes stale staging/download files left behind by interrupted transfers.
pub(crate) fn cleanup_old_files(dir: &Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if let Ok(modified) = metadata.modified()
            && modified.elapsed().map(|age| age > MAX_AGE).unwrap_or(false)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c == ' ');
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn validate_served_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err("invalid file name".to_string())
    }
}

/// Resolves a frontend marker path (`__web_upload__/...`) to a real server
/// path. Non-marker paths pass through untouched.
pub(crate) fn resolve_staged_path_for(
    app: &tauri::AppHandle,
    marker_path: &str,
) -> Result<String, String> {
    if let Some(staged) = marker_path.strip_prefix(WEB_UPLOAD_PREFIX) {
        validate_served_name(staged)?;
        let path = staging_dir(app).join(staged);
        if path.is_file() {
            Ok(path.to_string_lossy().to_string())
        } else {
            Err(format!("Staged upload not found: {staged}"))
        }
    } else {
        Ok(marker_path.to_string())
    }
}

/// Web-mode `download_remote_file`: transfers the remote file into the
/// server-side download directory and reports its served name so the browser
/// can fetch it from `GET /api/sftp/temp/{name}`.
pub(crate) async fn download_remote_file_web(
    app: tauri::AppHandle,
    session_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: Option<String>,
) -> HandlerOutput {
    use tauri::Manager;

    let Some(requested_name) = local_path.strip_prefix(WEB_DOWNLOAD_PREFIX) else {
        // Not a web marker — hand the path to the regular command unchanged.
        let state_app = app.clone();
        return super::rpc::ok(
            crate::cmd::sftp::download_remote_file(
                app,
                state_app.state::<Arc<SessionManager>>(),
                session_id,
                remote_path,
                local_path,
                transfer_id,
            )
            .await,
        );
    };

    let base_name = match Path::new(&remote_path).file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => requested_name.to_string(),
    };
    let served_name = format!(
        "{}_{}",
        uuid::Uuid::new_v4().simple(),
        sanitize_file_name(&base_name)
    );
    let temp_path = download_dir(&app).join(&served_name);

    let state_app = app.clone();
    let result = crate::cmd::sftp::download_remote_file(
        app,
        state_app.state::<Arc<SessionManager>>(),
        session_id,
        remote_path,
        temp_path.to_string_lossy().to_string(),
        transfer_id,
    )
    .await;

    match result {
        Ok(()) => Ok(json!({ "__webDownload": served_name })),
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(error.to_string())
        }
    }
}

/// Web-mode `upload_local_file`: resolves the staged upload produced by the
/// browser dialog shim and feeds it to the regular upload command.
pub(crate) async fn upload_local_file_web(
    app: tauri::AppHandle,
    session_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: Option<String>,
    duplicate_strategy_override: Option<String>,
) -> HandlerOutput {
    use tauri::Manager;

    let resolved = resolve_staged_path_for(&app, &local_path)?;
    let state_app = app.clone();
    super::rpc::ok(
        crate::cmd::sftp::upload_local_file(
            app,
            state_app.state::<Arc<SessionManager>>(),
            session_id,
            resolved,
            remote_path,
            transfer_id,
            duplicate_strategy_override,
        )
        .await,
    )
}

/// `POST /api/sftp/staging?name=<filename>` — raw-body upload used by the
/// browser dialog shim before a file can be handed to the regular upload
/// command. Returns the staged file name to embed in the marker path.
pub(crate) async fn stage_upload(
    AxumState(state): AxumState<ServerState>,
    AxumQuery(query): AxumQuery<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let original_name = query
        .get("name")
        .cloned()
        .unwrap_or_else(|| "upload.bin".to_string());
    let staged_name = format!(
        "{}_{}",
        uuid::Uuid::new_v4().simple(),
        sanitize_file_name(&original_name)
    );
    let target = state.staging_dir.join(&staged_name);
    match tokio::fs::write(&target, &body).await {
        Ok(()) => (StatusCode::OK, axum::Json(json!({ "name": staged_name }))).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to stage upload: {error}"),
        )
            .into_response(),
    }
}

/// `GET /api/sftp/staging/{name}` — re-fetch a staged file (used by flows
/// that open a staged file directly).
pub(crate) async fn fetch_staged(
    AxumState(state): AxumState<ServerState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Err(message) = validate_served_name(&name) {
        return (StatusCode::BAD_REQUEST, message).into_response();
    }
    serve_file(state.staging_dir.join(&name), None, false).await
}

/// `GET /api/sftp/temp/{name}` — stream a completed download to the browser.
pub(crate) async fn fetch_temp_download(
    AxumState(state): AxumState<ServerState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Err(message) = validate_served_name(&name) {
        return (StatusCode::BAD_REQUEST, message).into_response();
    }
    serve_file(state.download_dir.join(&name), Some(name), true).await
}

async fn serve_file(path: PathBuf, download_name: Option<String>, attachment: bool) -> Response {
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };
    let Ok(metadata) = file.metadata().await else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };

    let mime = mime_for_path(&path);
    let mut headers = HeaderMap::new();
    if let Ok(value) = mime.parse() {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let (true, Some(name)) = (attachment, download_name)
        && let Ok(value) = format!("attachment; filename=\"{name}\"").parse()
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = metadata.len().to_string().parse() {
        headers.insert(header::CONTENT_LENGTH, value);
    }

    (
        StatusCode::OK,
        headers,
        axum::body::Body::from_stream(ReaderStream::new(file)),
    )
        .into_response()
}

pub(crate) fn mime_for_path(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" | "log" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_file_name_strips_unsafe_characters() {
        assert_eq!(sanitize_file_name("report 2026.pdf"), "report 2026.pdf");
        // Path separators are replaced; a sanitized name is a single safe
        // component (leading dots are trimmed, ".." can never reappear at
        // the start, and no separator survives).
        let sanitized = sanitize_file_name("../../etc/passwd");
        assert_eq!(sanitized, "_.._etc_passwd");
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.starts_with(".."));
        assert_eq!(sanitize_file_name(""), "download");
    }

    #[test]
    fn validate_served_name_rejects_traversal() {
        assert!(validate_served_name("abc_123.bin").is_ok());
        assert!(validate_served_name("..").is_err());
        assert!(validate_served_name("a/b").is_err());
        assert!(validate_served_name("").is_err());
    }
}
