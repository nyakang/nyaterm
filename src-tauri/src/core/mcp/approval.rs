use std::collections::HashMap;
use std::sync::Arc;

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
    #[cfg(any(windows, test))]
    owner_window_label: String,
    #[cfg(any(windows, test))]
    cancellation: CancellationToken,
    responder: oneshot::Sender<ApprovalDecision>,
}

#[derive(Default)]
pub struct McpApprovalManager {
    pending: Mutex<HashMap<String, PendingApproval>>,
}

impl McpApprovalManager {
    pub async fn request(
        self: &Arc<Self>,
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
                #[cfg(any(windows, test))]
                owner_window_label: owner_window_label.to_string(),
                #[cfg(any(windows, test))]
                cancellation: cancellation.clone(),
                responder: tx,
            },
        );
        if window.emit(APPROVAL_EVENT, &event).is_err() {
            self.pending.lock().await.remove(&event.request_id);
            return Err(AppError::Config(
                "Failed to display the MCP approval request.".into(),
            ));
        }
        #[cfg(windows)]
        {
            let approvals = Arc::downgrade(self);
            let app = app.clone();
            let request_id = event.request_id.clone();
            super::notification::show(&window, move || {
                if let Some(approvals) = approvals.upgrade() {
                    let app = app.clone();
                    let request_id = request_id.clone();
                    tauri::async_runtime::spawn(async move {
                        approvals.focus_pending(&app, &request_id).await;
                    });
                }
            });
        }
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(AppError::Cancelled("MCP approval was cancelled.".into())),
            result = rx => result.map_err(|_| AppError::Cancelled("MCP approval was cancelled.".into())),
        };
        self.pending.lock().await.remove(&event.request_id);
        result
    }

    #[cfg(any(windows, test))]
    async fn pending_owner(&self, request_id: &str) -> Option<String> {
        self.pending
            .lock()
            .await
            .get(request_id)
            .filter(|pending| {
                !pending.responder.is_closed() && !pending.cancellation.is_cancelled()
            })
            .map(|pending| pending.owner_window_label.clone())
    }

    #[cfg(windows)]
    async fn focus_pending(&self, app: &tauri::AppHandle, request_id: &str) {
        let Some(owner) = self.pending_owner(request_id).await else {
            return;
        };
        if app
            .try_state::<AppLockState>()
            .is_some_and(|state| state.is_locked())
        {
            return;
        }
        if let Some(window) = app.get_webview_window(&owner) {
            crate::app::focus_window(&window);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn add_pending(
        manager: &McpApprovalManager,
        request_id: &str,
        owner: &str,
        connection_id: &str,
    ) -> (oneshot::Receiver<ApprovalDecision>, CancellationToken) {
        let (tx, rx) = oneshot::channel();
        let cancellation = CancellationToken::new();
        manager.pending.lock().await.insert(
            request_id.to_string(),
            PendingApproval {
                connection_id: connection_id.to_string(),
                owner_window_label: owner.to_string(),
                cancellation: cancellation.clone(),
                responder: tx,
            },
        );
        (rx, cancellation)
    }

    #[tokio::test]
    async fn mcp_notification_resolves_only_its_pending_owner() {
        let manager = McpApprovalManager::default();
        let (mut first, _) = add_pending(&manager, "first", "main", "connection-1").await;
        let (mut second, _) = add_pending(&manager, "second", "child", "connection-2").await;
        assert_eq!(
            manager.pending_owner("first").await.as_deref(),
            Some("main")
        );
        assert_eq!(
            manager.pending_owner("second").await.as_deref(),
            Some("child")
        );
        assert_eq!(manager.pending_owner("unknown").await, None);
        assert_eq!(first.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        assert_eq!(second.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    }

    #[tokio::test]
    async fn mcp_notification_cannot_resolve_completed_requests() {
        for decision in [
            ApprovalDecision::AllowOnce,
            ApprovalDecision::AllowSession,
            ApprovalDecision::Deny,
        ] {
            let manager = McpApprovalManager::default();
            let (rx, _) = add_pending(&manager, "request", "main", "connection").await;
            manager.respond("request", decision).await.unwrap();
            assert_eq!(rx.await.unwrap(), decision);
            assert_eq!(manager.pending_owner("request").await, None);
            assert!(manager.respond("request", decision).await.is_err());
        }
    }

    #[tokio::test]
    async fn mcp_notification_ignores_cancelled_and_abandoned_requests() {
        let manager = McpApprovalManager::default();
        let (mut rx, cancellation) = add_pending(&manager, "cancelled", "main", "connection").await;
        cancellation.cancel();
        assert_eq!(manager.pending_owner("cancelled").await, None);
        assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty));

        let (rx, _) = add_pending(&manager, "abandoned", "main", "connection").await;
        drop(rx);
        assert_eq!(manager.pending_owner("abandoned").await, None);
    }

    #[tokio::test]
    async fn mcp_notification_cannot_resolve_disconnected_requests() {
        let manager = McpApprovalManager::default();
        let (mut first, _) = add_pending(&manager, "first", "main", "connection-1").await;
        let (mut second, _) = add_pending(&manager, "second", "child", "connection-2").await;
        manager.cancel_connection("connection-1").await;
        assert_eq!(manager.pending_owner("first").await, None);
        assert_eq!(
            manager.pending_owner("second").await.as_deref(),
            Some("child")
        );
        assert_eq!(first.try_recv(), Err(oneshot::error::TryRecvError::Closed));
        assert_eq!(second.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        manager.cancel_all().await;
        assert_eq!(manager.pending_owner("second").await, None);
        assert_eq!(second.try_recv(), Err(oneshot::error::TryRecvError::Closed));
    }
}
