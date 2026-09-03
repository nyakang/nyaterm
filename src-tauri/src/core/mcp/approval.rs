use std::collections::HashMap;

use serde::Serialize;
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use crate::cmd::app::AppLockState;
use crate::config::RiskLevel;
use crate::error::{AppError, AppResult};

pub const APPROVAL_EVENT: &str = "mcp-approval-request";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Deny,
    AllowOnce,
    AllowSession,
}

impl ApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::AllowOnce => "allow_once",
            Self::AllowSession => "allow_session",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestEvent {
    pub request_id: String,
    pub client: String,
    pub capability: String,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub parameter_summary: String,
    pub risk: RiskLevel,
}

struct PendingApproval {
    connection_id: String,
    responder: oneshot::Sender<ApprovalDecision>,
}

#[derive(Default)]
pub struct McpApprovalManager {
    pending: Mutex<HashMap<String, PendingApproval>>,
}

impl McpApprovalManager {
    pub async fn request(
        &self,
        app: &tauri::AppHandle,
        owner_window_label: &str,
        connection_id: &str,
        event: ApprovalRequestEvent,
        cancellation: &CancellationToken,
    ) -> AppResult<ApprovalDecision> {
        if app
            .try_state::<AppLockState>()
            .is_some_and(|state| state.is_locked())
        {
            return Err(AppError::Config(
                "MCP approval is unavailable while NyaTerm is locked.".into(),
            ));
        }
        let Some(window) = app.get_webview_window(owner_window_label) else {
            return Err(AppError::Config(
                "The session owner window is unavailable for approval.".into(),
            ));
        };
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(
            event.request_id.clone(),
            PendingApproval {
                connection_id: connection_id.to_string(),
                responder: tx,
            },
        );
        if window.emit(APPROVAL_EVENT, &event).is_err() {
            self.pending.lock().await.remove(&event.request_id);
            return Err(AppError::Config(
                "Failed to display the MCP approval request.".into(),
            ));
        }
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(AppError::Cancelled("MCP approval was cancelled.".into())),
            result = rx => result.map_err(|_| AppError::Cancelled("MCP approval was cancelled.".into())),
        };
        self.pending.lock().await.remove(&event.request_id);
        result
    }

    pub async fn respond(&self, request_id: &str, decision: ApprovalDecision) -> AppResult<()> {
        let pending = self
            .pending
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| {
                AppError::Config("The MCP approval request is no longer pending.".into())
            })?;
        pending
            .responder
            .send(decision)
            .map_err(|_| AppError::Cancelled("The MCP approval request was cancelled.".into()))
    }

    pub async fn cancel_connection(&self, connection_id: &str) {
        self.pending
            .lock()
            .await
            .retain(|_, pending| pending.connection_id != connection_id);
    }

    pub async fn cancel_all(&self) {
        self.pending.lock().await.clear();
    }
}
