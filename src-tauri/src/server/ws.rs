//! WebSocket event bridge (`GET /api/ws`).
//!
//! Forwards the app's global Tauri event broadcasts to connected browsers.
//! Desktop emits are global broadcasts (no `emit_to` in `core/`), so a single
//! multiplexed WebSocket reproduces the exact event semantics the frontend
//! already relies on: every client receives every event and filters locally,
//! exactly like multiple desktop windows do.
//!
//! Per-session events (`terminal-output-{id}`, `session-error-{id}`, ...)
//! are dynamic: a reconciler watches `sessions-changed` and attaches
//! listeners for new session ids. Output that fires before the listener is
//! attached is not lost — the attach/ack flow-control protocol replays the
//! recent output backlog when the renderer attaches.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::State as AxumState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde_json::Value;
use tokio::sync::broadcast;

use super::rpc::ServerState;
use crate::core::SessionManager;

/// Payload channel type shared with `ServerState.events`.
pub(crate) type EventTx = broadcast::Sender<(String, Value)>;

/// Global (non per-session) events emitted by the backend that the frontend
/// listens for. Frontend→frontend events (e.g. `proxy-saved`) are handled by
/// the web shim's local event bus and never reach the backend.
const GLOBAL_EVENTS: &[&str] = &[
    "sessions-changed",
    "connections-changed",
    "command-history-changed",
    "settings-changed",
    "credentials-changed",
    "transfer-event",
    "otp-request",
    "ssh-auth-request",
    "ssh-agent-auth-pending",
    "ssh-agent-auth-failed",
    "ssh-agent-auth-resolved",
    "host-key-verify",
    "host-key-verify-resolved",
    "security-prompt-resolved",
    "docker-sudo-password-request",
    "session-command-accepted",
    "recording-status-changed",
    "quick-commands-changed",
    "cloud-sync-status-changed",
    "cloud-sync-history-changed",
    "cloud-sync-conflict",
    "mcp-status-changed",
    "mcp-approval-request",
    "mcp-session-open-request",
    "mcp-session-open-cancel",
    "app-lock-state-changed",
    "app-user-activity",
    "external-open-available",
    "notes-changed",
    "auto-upload-decision",
];

/// Per-session event name prefixes; the session id is appended.
const SESSION_EVENT_PREFIXES: &[&str] = &[
    "terminal-output-",
    "session-error-",
    "session-closed-",
    "cwd-changed-",
    "focus-terminal-",
    "ai-capture-",
    "zmodem-event-",
];

pub(crate) fn start_event_bridge(app: &tauri::AppHandle) -> EventTx {
    use tauri::Listener;

    let (tx, _) = broadcast::channel(8192);

    for name in GLOBAL_EVENTS {
        attach_listener(app, &tx, name);
    }

    // Reconcile per-session listeners whenever the session set changes.
    {
        let listener_app = app.clone();
        let closure_app = listener_app.clone();
        let listener_tx = tx.clone();
        listener_app.listen_any("sessions-changed", move |_event| {
            let app = closure_app.clone();
            let tx = listener_tx.clone();
            tauri::async_runtime::spawn(async move {
                reconcile_session_listeners(&app, &tx).await;
            });
        });
    }

    let initial_app = app.clone();
    let initial_tx = tx.clone();
    tauri::async_runtime::spawn(async move {
        reconcile_session_listeners(&initial_app, &initial_tx).await;
    });

    tx
}

fn attach_listener(app: &tauri::AppHandle, tx: &EventTx, name: &str) {
    use tauri::Listener;

    let tx = tx.clone();
    let name = name.to_string();
    app.listen_any(name.clone(), move |event: tauri::Event| {
        let payload = event.payload().to_owned();
        let payload: Value = serde_json::from_str(&payload).unwrap_or(Value::String(payload));
        let _ = tx.send((name.clone(), payload));
    });
}

async fn reconcile_session_listeners(app: &tauri::AppHandle, tx: &EventTx) {
    use tauri::Manager;

    let sessions =
        match crate::cmd::session::list_sessions(app.state::<Arc<SessionManager>>()).await {
            Ok(sessions) => sessions,
            Err(error) => {
                tracing::warn!(%error, "event bridge failed to list sessions for reconciliation");
                return;
            }
        };

    for session in sessions {
        if is_session_listeners_attached(&session.id) {
            continue;
        }
        for prefix in SESSION_EVENT_PREFIXES {
            attach_listener(app, tx, &format!("{prefix}{}", session.id));
        }
        mark_session_listeners_attached(&session.id);
    }
}

fn session_listeners_registry() -> &'static Mutex<HashSet<String>> {
    static REGISTRY: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_session_listeners_attached(session_id: &str) -> bool {
    session_listeners_registry()
        .lock()
        .map(|registry| registry.contains(session_id))
        .unwrap_or(true)
}

fn mark_session_listeners_attached(session_id: &str) {
    if let Ok(mut registry) = session_listeners_registry().lock() {
        registry.insert(session_id.to_string());
    }
}

pub(crate) async fn ws_endpoint(
    AxumState(state): AxumState<ServerState>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let rx = state.events.subscribe();
    upgrade.on_upgrade(move |socket| ws_loop(socket, rx))
}

async fn ws_loop(mut socket: WebSocket, mut rx: broadcast::Receiver<(String, Value)>) {
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok((name, payload)) => {
                    let frame = serde_json::to_string(&serde_json::json!({
                        "event": name,
                        "payload": payload,
                    }))
                    .unwrap_or_default();
                    if socket.send(Message::text(frame)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "web client lagged behind event stream");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                None => break,
                Some(Ok(Message::Ping(data))) => {
                    let _ = socket.send(Message::Pong(data)).await;
                }
                Some(Ok(Message::Close(_))) => break,
                // Text/binary/pong frames from the client are ignored: the
                // event bridge is server-push only.
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
}
