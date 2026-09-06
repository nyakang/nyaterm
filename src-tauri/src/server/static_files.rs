//! Static asset serving for the web frontend.
//!
//! Two sources, in order:
//! 1. `--dist` / `NYATERM_WEB_DIST` directory (recommended: this is the
//!    web-flavored frontend build with the Tauri API shims aliased in)
//! 2. The Tauri-embedded assets (the desktop build — usable, but the
//!    frontend will fall back to raw Tauri IPC and fail in a browser)

use std::path::PathBuf;

use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use super::rpc::ServerState;

pub(crate) async fn fallback(AxumState(state): AxumState<ServerState>, uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    let decoded = percent_decode_path(raw_path);
    let path = decoded.as_str();

    if path.is_empty() {
        return serve_index(&state).await;
    }
    // Never expose non-asset paths through the static fallback.
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    if let Some(dist) = state.dist_dir.clone()
        && let Some(response) = serve_from_dist(&dist, path).await
    {
        return response;
    }

    if let Some(response) = serve_embedded(&state, path).await {
        return response;
    }

    // SPA client-side routing: unknown extension-less paths get index.html.
    if !path.contains('.') {
        return serve_index(&state).await;
    }

    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn serve_index(state: &ServerState) -> Response {
    if let Some(dist) = state.dist_dir.clone() {
        let index = dist.join("index.html");
        if index.is_file() {
            if let Some(response) = serve_file(&index, false).await {
                return response;
            }
        }
    }
    if let Some(response) = serve_embedded(state, "index.html").await {
        return response;
    }
    (
        StatusCode::NOT_FOUND,
        "frontend assets not configured; start with NYATERM_WEB_DIST=<dist dir>",
    )
        .into_response()
}

async fn serve_from_dist(dist: &std::path::Path, path: &str) -> Option<Response> {
    if path.split('/').any(|segment| segment == "..") {
        return None;
    }
    let candidate: PathBuf = dist.join(path);
    if candidate.is_file() {
        serve_file(&candidate, false).await
    } else {
        None
    }
}

async fn serve_embedded(state: &ServerState, path: &str) -> Option<Response> {
    let asset = state.app.asset_resolver().get(path.to_string())?;
    let mime = asset.mime_type().to_string();
    let mut response = (StatusCode::OK, asset.bytes().to_vec()).into_response();
    if let Ok(value) = mime.parse() {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    Some(response)
}

async fn serve_file(path: &std::path::Path, attachment: bool) -> Option<Response> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let metadata = file.metadata().await.ok()?;
    let mut headers = HeaderMap::new();
    if let Ok(value) = super::files::mime_for_path(path).parse() {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = metadata.len().to_string().parse() {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    let _ = attachment;
    Some(
        (
            StatusCode::OK,
            headers,
            axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)),
        )
            .into_response(),
    )
}

/// Percent-decoding for asset paths (e.g. `my%20file.png`).
fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                match bytes.get(i + 1..i + 3).and_then(|hex| {
                    std::str::from_utf8(hex)
                        .ok()
                        .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                }) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}
