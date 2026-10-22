use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::{Action, ActionType, NotificationExt};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use super::host::McpManager;
use crate::cmd::app::AppLockState;
use crate::config::RiskLevel;
use crate::error::{AppError, AppResult};

pub const APPROVAL_EVENT: &str = "mcp-approval-request";
pub const APPROVAL_RESOLVED_EVENT: &str = "mcp-approval-resolved";
pub const NOTIFICATION_ACTION_TYPE_ID: &str = "mcp-approval";

const SUMMARY_CHARS: usize = 180;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationAction {
    Focus,
    Resolve(ApprovalDecision),
    Ignore,
}

fn classify_notification_action(action_id: &str) -> NotificationAction {
    match action_id {
        "open" | "tap" => NotificationAction::Focus,
        "allow_once" => NotificationAction::Resolve(ApprovalDecision::AllowOnce),
        "deny" => NotificationAction::Resolve(ApprovalDecision::Deny),
        _ => NotificationAction::Ignore,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationContext {
    request_id: String,
    owner_window_label: String,
}

fn notification_context(extra: &HashMap<String, Value>) -> Option<NotificationContext> {
    let request_id = extra.get("requestId")?.as_str()?.to_string();
    let owner_window_label = extra.get("ownerWindowLabel")?.as_str()?.to_string();
    if request_id.is_empty() || owner_window_label.is_empty() {
        return None;
    }
    Some(NotificationContext {
        request_id,
        owner_window_label,
    })
}

fn should_notify(system_notification: bool, focused: bool, minimized: bool, visible: bool) -> bool {
    system_notification && (minimized || !visible || !focused)
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalResolvedEvent<'a> {
    request_id: &'a str,
    decision: &'a str,
}

struct PendingApproval {
    connection_id: String,
    owner_window_label: String,
    notification_id: Option<i32>,
    responder: oneshot::Sender<ApprovalDecision>,
}

struct ResolvedNotice {
    owner_window_label: String,
    notification_id: Option<i32>,
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
                owner_window_label: owner_window_label.to_string(),
                notification_id: None,
                responder: tx,
            },
        );
        if window.emit(APPROVAL_EVENT, &event).is_err() {
            self.pending.lock().await.remove(&event.request_id);
            return Err(AppError::Config(
                "Failed to display the MCP approval request.".into(),
            ));
        }
        if let Some(notification_id) =
            surface_notification(app, &window, owner_window_label, &event)
        {
            if let Some(pending) = self.pending.lock().await.get_mut(&event.request_id) {
                pending.notification_id = Some(notification_id);
            }
        }
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(AppError::Cancelled("MCP approval was cancelled.".into())),
            result = rx => result.map_err(|_| AppError::Cancelled("MCP approval was cancelled.".into())),
        };
        self.pending.lock().await.remove(&event.request_id);
        result
    }

    pub async fn respond(
        &self,
        app: Option<&tauri::AppHandle>,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> AppResult<()> {
        let notice = self.take_pending(request_id, decision).await?;
        let Some(app) = app else {
            return Ok(());
        };
        dismiss_notification(app, notice.notification_id);
        if let Some(window) = app.get_webview_window(&notice.owner_window_label) {
            let _ = window.emit(
                APPROVAL_RESOLVED_EVENT,
                ApprovalResolvedEvent {
                    request_id,
                    decision: decision.as_str(),
                },
            );
        }
        Ok(())
    }

    async fn take_pending(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> AppResult<ResolvedNotice> {
        let pending = self
            .pending
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| {
                AppError::Config("The MCP approval request is no longer pending.".into())
            })?;
        let notice = ResolvedNotice {
            owner_window_label: pending.owner_window_label,
            notification_id: pending.notification_id,
        };
        pending
            .responder
            .send(decision)
            .map_err(|_| AppError::Cancelled("The MCP approval request was cancelled.".into()))?;
        Ok(notice)
    }

    pub async fn cancel_connection(&self, app: Option<&tauri::AppHandle>, connection_id: &str) {
        let removed = {
            let mut pending = self.pending.lock().await;
            let ids = pending
                .extract_if(|_, item| item.connection_id == connection_id)
                .map(|(_, item)| item.notification_id)
                .collect::<Vec<_>>();
            ids
        };
        dismiss_all(app, removed);
    }

    pub async fn cancel_all(&self, app: Option<&tauri::AppHandle>) {
        let removed = std::mem::take(&mut *self.pending.lock().await);
        dismiss_all(
            app,
            removed
                .into_values()
                .map(|item| item.notification_id)
                .collect(),
        );
    }
}

pub fn install_approval_notifications(app: &tauri::AppHandle) {
    register_action_types(app);
    let app_handle = app.clone();
    if let Err(error) = app.notification().on_action(move |performed| {
        let app_handle = app_handle.clone();
        let action_id = performed.action_id().to_string();
        let extra = performed
            .notification()
            .map(|notification| notification.extra().clone());
        tauri::async_runtime::spawn(async move {
            handle_notification_action(&app_handle, &action_id, extra.as_ref()).await;
        });
    }) {
        tracing::warn!("Failed to listen for MCP approval notifications: {error}");
    }
}

fn register_action_types(app: &tauri::AppHandle) {
    let copy = notice_copy(&interface_language(app));
    if let Err(error) = app.notification().register_action_types(vec![
        ActionType::builder(NOTIFICATION_ACTION_TYPE_ID)
            .actions(vec![
                Action::builder("open", copy.open).build(),
                Action::builder("allow_once", copy.allow_once).build(),
                Action::builder("deny", copy.deny).build(),
            ])
            .build(),
    ]) {
        tracing::warn!("Failed to register MCP approval notification actions: {error}");
    }
}

async fn handle_notification_action(
    app: &tauri::AppHandle,
    action_id: &str,
    extra: Option<&HashMap<String, Value>>,
) {
    let Some(extra) = extra else {
        return;
    };
    let Some(context) = notification_context(extra) else {
        return;
    };
    match classify_notification_action(action_id) {
        NotificationAction::Focus => {
            if let Some(window) = app.get_webview_window(&context.owner_window_label) {
                crate::app::focus_window(&window);
            }
        }
        NotificationAction::Resolve(decision) => {
            let Some(manager) = app.try_state::<std::sync::Arc<McpManager>>() else {
                return;
            };
            if let Err(error) = manager
                .respond_approval(&context.request_id, decision)
                .await
            {
                tracing::debug!("Ignored MCP approval notification action: {error}");
            }
        }
        NotificationAction::Ignore => {}
    }
}

fn surface_notification(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    owner_window_label: &str,
    event: &ApprovalRequestEvent,
) -> Option<i32> {
    let settings = crate::config::load_app_settings(app).ok();
    let system_notification = settings
        .as_ref()
        .is_none_or(|settings| settings.ai.external_mcp.system_notification);
    let needs_attention = should_notify(
        system_notification,
        window.is_focused().unwrap_or(false),
        window.is_minimized().unwrap_or(false),
        window.is_visible().unwrap_or(true),
    );
    if !needs_attention {
        return None;
    }
    if let Err(error) = window.request_user_attention(Some(tauri::UserAttentionType::Critical)) {
        tracing::warn!("Failed to request attention for MCP approval: {error}");
    }
    let language = settings
        .as_ref()
        .and_then(|settings| settings.ui.language.as_deref())
        .unwrap_or("en");
    let id = next_notification_id();
    register_action_types(app);
    let copy = notice_copy(language);
    let shown = app
        .notification()
        .builder()
        .id(id)
        .title(copy.title)
        .body(notification_body(&copy, event))
        .action_type_id(NOTIFICATION_ACTION_TYPE_ID)
        .extra("requestId", event.request_id.clone())
        .extra("ownerWindowLabel", owner_window_label.to_string())
        .show();
    match shown {
        Ok(()) => Some(id),
        Err(error) => {
            tracing::warn!("Failed to show MCP approval notification: {error}");
            None
        }
    }
}

fn dismiss_all(app: Option<&tauri::AppHandle>, ids: Vec<Option<i32>>) {
    let Some(app) = app else {
        return;
    };
    for id in ids {
        dismiss_notification(app, id);
    }
}

fn dismiss_notification(app: &tauri::AppHandle, id: Option<i32>) {
    let Some(id) = id else {
        return;
    };
    if let Err(error) = app.notification().remove_active(vec![id]) {
        tracing::debug!("Failed to close MCP approval notification {id}: {error}");
    }
}

fn next_notification_id() -> i32 {
    static NEXT: AtomicI32 = AtomicI32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn interface_language(app: &tauri::AppHandle) -> String {
    crate::config::load_app_settings(app)
        .ok()
        .and_then(|settings| settings.ui.language)
        .unwrap_or_else(|| "en".to_string())
}

fn notification_body(copy: &NoticeCopy, event: &ApprovalRequestEvent) -> String {
    let target = event
        .connection_name
        .as_deref()
        .or(event.connection_id.as_deref())
        .or(event.session_name.as_deref())
        .or(event.session_id.as_deref())
        .unwrap_or("-");
    let mut body = format!(
        "{}: {}\n{}: {}\n{}: {}\n{}: {}",
        copy.client,
        event.client,
        copy.capability,
        event.capability,
        copy.target,
        target,
        copy.risk,
        risk_label(&event.risk),
    );
    let summary = truncate_summary(&event.parameter_summary);
    if !summary.is_empty() {
        body.push('\n');
        body.push_str(&summary);
    }
    body
}

fn risk_label(risk: &RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

fn truncate_summary(summary: &str) -> String {
    let mut truncated = summary.chars().take(SUMMARY_CHARS).collect::<String>();
    if summary.chars().count() > SUMMARY_CHARS {
        truncated.push('…');
    }
    truncated
}

struct NoticeCopy {
    title: &'static str,
    client: &'static str,
    capability: &'static str,
    target: &'static str,
    risk: &'static str,
    open: &'static str,
    allow_once: &'static str,
    deny: &'static str,
}

fn notice_copy(language: &str) -> NoticeCopy {
    let normalized = language.trim().replace('_', "-").to_ascii_lowercase();
    if normalized == "zh-tw"
        || normalized == "zh-hant"
        || normalized == "zh-hk"
        || normalized == "zh-mo"
        || normalized.starts_with("zh-hant-")
    {
        NoticeCopy {
            title: "外部 MCP 核准",
            client: "用戶端",
            capability: "功能",
            target: "目標",
            risk: "風險",
            open: "查看",
            allow_once: "允許一次",
            deny: "拒絕",
        }
    } else if normalized == "zh" || normalized.starts_with("zh-") {
        NoticeCopy {
            title: "外部 MCP 审批",
            client: "客户端",
            capability: "能力",
            target: "目标",
            risk: "风险",
            open: "查看",
            allow_once: "允许一次",
            deny: "拒绝",
        }
    } else if normalized == "ko" || normalized.starts_with("ko-") {
        NoticeCopy {
            title: "외부 MCP 승인",
            client: "클라이언트",
            capability: "기능",
            target: "대상",
            risk: "위험",
            open: "보기",
            allow_once: "한 번 허용",
            deny: "거부",
        }
    } else if normalized == "ru" || normalized.starts_with("ru-") {
        NoticeCopy {
            title: "Запрос разрешения от внешнего клиента MCP",
            client: "Клиент",
            capability: "Возможности",
            target: "Цель",
            risk: "Риск",
            open: "Открыть",
            allow_once: "Разрешить один раз",
            deny: "Отклонить",
        }
    } else {
        NoticeCopy {
            title: "External MCP approval",
            client: "Client",
            capability: "Capability",
            target: "Target",
            risk: "Risk",
            open: "View",
            allow_once: "Allow once",
            deny: "Deny",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(summary: &str) -> ApprovalRequestEvent {
        ApprovalRequestEvent {
            request_id: "req-1".into(),
            client: "Cursor".into(),
            capability: "terminal.execute".into(),
            session_id: None,
            session_name: Some("shell".into()),
            connection_id: None,
            connection_name: Some("prod".into()),
            parameter_summary: summary.into(),
            risk: RiskLevel::High,
        }
    }

    #[test]
    fn notification_actions_map_to_one_decision_or_focus() {
        assert_eq!(
            classify_notification_action("allow_once"),
            NotificationAction::Resolve(ApprovalDecision::AllowOnce)
        );
        assert_eq!(
            classify_notification_action("deny"),
            NotificationAction::Resolve(ApprovalDecision::Deny)
        );
        assert_eq!(
            classify_notification_action("open"),
            NotificationAction::Focus
        );
        assert_eq!(
            classify_notification_action("tap"),
            NotificationAction::Focus
        );
        assert_eq!(
            classify_notification_action("allow_session"),
            NotificationAction::Ignore
        );
    }

    #[test]
    fn notification_context_requires_request_and_owner() {
        let mut extra = HashMap::new();
        extra.insert("requestId".into(), Value::String("req-1".into()));
        extra.insert("ownerWindowLabel".into(), Value::String("main".into()));
        let context = notification_context(&extra).expect("context");
        assert_eq!(context.request_id, "req-1");
        assert_eq!(context.owner_window_label, "main");

        extra.insert("requestId".into(), Value::String(String::new()));
        assert!(notification_context(&extra).is_none());
        extra.remove("requestId");
        assert!(notification_context(&extra).is_none());
    }

    #[test]
    fn notification_payload_keeps_the_request_context() {
        let performed: tauri_plugin_notification::ActionPerformed =
            serde_json::from_value(serde_json::json!({
                "actionId": "deny",
                "notification": {
                    "id": 7,
                    "title": "External MCP approval",
                    "body": "Client: Cursor",
                    "extra": {
                        "requestId": "req-9",
                        "ownerWindowLabel": "main-2"
                    }
                }
            }))
            .expect("action payload");
        assert_eq!(
            classify_notification_action(performed.action_id()),
            NotificationAction::Resolve(ApprovalDecision::Deny)
        );
        let context = notification_context(performed.notification().expect("notice").extra())
            .expect("context");
        assert_eq!(context.request_id, "req-9");
        assert_eq!(context.owner_window_label, "main-2");
    }

    #[test]
    fn notification_is_limited_to_unfocused_windows_when_enabled() {
        assert!(!should_notify(true, true, false, true));
        assert!(should_notify(true, false, false, true));
        assert!(should_notify(true, true, true, true));
        assert!(should_notify(true, true, false, false));
        assert!(!should_notify(false, false, true, false));
    }

    #[test]
    fn notification_body_uses_the_interface_language_and_truncates_the_summary() {
        let body = notification_body(&notice_copy("zh-CN"), &event("rm -rf /tmp/demo"));
        assert!(
            body.starts_with("客户端: Cursor\n能力: terminal.execute\n目标: prod\n风险: high\n")
        );
        assert!(body.ends_with("rm -rf /tmp/demo"));

        let long = "令".repeat(SUMMARY_CHARS + 5);
        let truncated = notification_body(&notice_copy("en"), &event(&long));
        let summary = truncated.lines().next_back().expect("summary");
        assert_eq!(summary.chars().count(), SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));
        assert_eq!(notice_copy("zh-Hant-HK").open, "查看");
        assert_eq!(notice_copy("ko-KR").deny, "거부");
        assert_eq!(notice_copy("ru").allow_once, "Разрешить один раз");
        assert_eq!(notice_copy("fr").title, "External MCP approval");
    }

    #[tokio::test]
    async fn a_notification_decision_resolves_once_and_open_leaves_it_pending() {
        let manager = McpApprovalManager::default();
        let (tx, rx) = oneshot::channel();
        manager.pending.lock().await.insert(
            "req-1".into(),
            PendingApproval {
                connection_id: "connection".into(),
                owner_window_label: "main".into(),
                notification_id: Some(4),
                responder: tx,
            },
        );

        assert_eq!(
            classify_notification_action("open"),
            NotificationAction::Focus
        );
        assert!(manager.pending.lock().await.contains_key("req-1"));

        let NotificationAction::Resolve(decision) = classify_notification_action("allow_once")
        else {
            panic!("allow once resolves");
        };
        let notice = manager
            .take_pending("req-1", decision)
            .await
            .expect("taken");
        assert_eq!(notice.notification_id, Some(4));
        assert_eq!(rx.await.expect("decision"), ApprovalDecision::AllowOnce);
        assert!(manager.take_pending("req-1", decision).await.is_err());
        assert!(
            manager
                .take_pending("missing", ApprovalDecision::Deny)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cancelling_a_request_drops_it_without_a_decision() {
        let manager = McpApprovalManager::default();
        let (tx, rx) = oneshot::channel();
        manager.pending.lock().await.insert(
            "req-1".into(),
            PendingApproval {
                connection_id: "connection".into(),
                owner_window_label: "main".into(),
                notification_id: None,
                responder: tx,
            },
        );
        manager.cancel_connection(None, "connection").await;
        assert!(rx.await.is_err());
        assert!(
            manager
                .take_pending("req-1", ApprovalDecision::Deny)
                .await
                .is_err()
        );
    }
}
