use super::transfer::{
    TRANSFER_CANCELLED_MESSAGE, TransferController, wait_for_transfer_cancelled,
};
use crate::core::SessionManager;
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, oneshot};

const DUPLICATE_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
// 隐藏队列不消耗回答时间，但窗口失联时也不能无限挂起。
const DUPLICATE_QUEUE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateChoice {
    Skip,
    Overwrite,
    Rename,
}

impl DuplicateChoice {
    pub fn from_action(action: &str) -> Option<Self> {
        match action {
            "skip" => Some(Self::Skip),
            "overwrite" => Some(Self::Overwrite),
            "rename" => Some(Self::Rename),
            _ => None,
        }
    }
}

/// Manages pending duplicate-file prompts awaiting user input from the frontend.
pub struct TransferDuplicateManager {
    pending: Mutex<HashMap<String, PendingDuplicatePrompt>>,
}

struct PendingDuplicatePrompt {
    reply: oneshot::Sender<DuplicateChoice>,
    displayed: Option<oneshot::Sender<()>>,
}

impl TransferDuplicateManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(
        &self,
        request_id: String,
    ) -> (oneshot::Receiver<DuplicateChoice>, oneshot::Receiver<()>) {
        let (reply, rx) = oneshot::channel();
        let (displayed, displayed_rx) = oneshot::channel();
        self.pending.lock().await.insert(
            request_id,
            PendingDuplicatePrompt {
                reply,
                displayed: Some(displayed),
            },
        );
        (rx, displayed_rx)
    }

    pub async fn mark_displayed(&self, request_id: &str) -> bool {
        let mut pending = self.pending.lock().await;
        let Some(prompt) = pending.get_mut(request_id) else {
            return false;
        };
        match prompt.displayed.take() {
            Some(tx) => tx.send(()).is_ok(),
            None => true,
        }
    }

    pub async fn respond(&self, request_id: &str, choice: DuplicateChoice) -> bool {
        if let Some(prompt) = self.pending.lock().await.remove(request_id) {
            prompt.reply.send(choice).is_ok()
        } else {
            false
        }
    }

    pub(super) async fn wait_for_response(
        &self,
        request_id: &str,
        mut reply: oneshot::Receiver<DuplicateChoice>,
        displayed: oneshot::Receiver<()>,
        controller: Option<&Arc<TransferController>>,
        answer_timeout: Duration,
        queue_timeout: Duration,
    ) -> AppResult<DuplicateChoice> {
        let response = async {
            tokio::select! {
                biased;
                choice = &mut reply => return choice.unwrap_or(DuplicateChoice::Skip),
                shown = displayed => if shown.is_err() { return DuplicateChoice::Skip; },
                _ = tokio::time::sleep(queue_timeout) => return DuplicateChoice::Skip,
            }
            match tokio::time::timeout(answer_timeout, reply).await {
                Ok(Ok(choice)) => choice,
                _ => DuplicateChoice::Skip,
            }
        };
        let cancelled = async {
            if let Some(controller) = controller {
                let _ = wait_for_transfer_cancelled(controller).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancelled => Err(AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.to_string())),
            choice = response => Ok(choice),
        };
        self.pending.lock().await.remove(request_id);
        result
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferDuplicateRequestPayload {
    pub request_id: String,
    pub session_id: String,
    pub remote_path: String,
    pub file_name: String,
    pub is_directory: bool,
    pub target_window_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_kind: Option<String>,
    pub requires_activation: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferDuplicateResolvedPayload {
    request_id: String,
    target_window_label: Option<String>,
}

pub async fn prompt_duplicate_choice(
    app: &AppHandle,
    session_manager: &SessionManager,
    session_id: &str,
    remote_path: &str,
    file_name: &str,
    is_directory: bool,
) -> AppResult<DuplicateChoice> {
    prompt_duplicate_choice_with_destination(
        app,
        session_manager,
        session_id,
        remote_path,
        file_name,
        is_directory,
        None,
        None,
    )
    .await
}

pub async fn prompt_duplicate_choice_with_destination(
    app: &AppHandle,
    session_manager: &SessionManager,
    session_id: &str,
    remote_path: &str,
    file_name: &str,
    is_directory: bool,
    destination_kind: Option<&str>,
    controller: Option<&Arc<TransferController>>,
) -> AppResult<DuplicateChoice> {
    let manager = app
        .try_state::<Arc<TransferDuplicateManager>>()
        .ok_or_else(|| AppError::Config("TransferDuplicateManager not available".to_string()))?;

    let target_window_label = session_manager
        .session_info(session_id)
        .await
        .ok()
        .and_then(|info| info.owner_window_label);

    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply, displayed) = manager.register(request_id.clone()).await;

    let emitted = app.emit(
        "transfer-duplicate-request",
        TransferDuplicateRequestPayload {
            request_id: request_id.clone(),
            session_id: session_id.to_string(),
            remote_path: remote_path.to_string(),
            file_name: file_name.to_string(),
            is_directory,
            target_window_label: target_window_label.clone(),
            destination_kind: destination_kind.map(|s| s.to_string()),
            requires_activation: true,
        },
    );

    if let Err(error) = emitted {
        manager.pending.lock().await.remove(&request_id);
        return Err(AppError::Channel(format!(
            "Failed to request duplicate choice: {error}"
        )));
    }

    let result = manager
        .wait_for_response(
            &request_id,
            reply,
            displayed,
            controller,
            DUPLICATE_PROMPT_TIMEOUT,
            DUPLICATE_QUEUE_TIMEOUT,
        )
        .await;
    let _ = app.emit(
        "transfer-duplicate-resolved",
        TransferDuplicateResolvedPayload {
            request_id,
            target_window_label,
        },
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_choice_accepts_rename_and_rejects_unknown_actions() {
        assert_eq!(
            DuplicateChoice::from_action("rename"),
            Some(DuplicateChoice::Rename)
        );
        assert_eq!(
            DuplicateChoice::from_action("skip"),
            Some(DuplicateChoice::Skip)
        );
        assert_eq!(
            DuplicateChoice::from_action("overwrite"),
            Some(DuplicateChoice::Overwrite)
        );
        assert_eq!(DuplicateChoice::from_action("unknown"), None);
    }

    #[tokio::test]
    async fn duplicate_manager_routes_rename_to_its_request_only() {
        let manager = TransferDuplicateManager::new();
        let (first, _) = manager.register("first".to_string()).await;
        let (second, _) = manager.register("second".to_string()).await;
        assert!(manager.respond("second", DuplicateChoice::Rename).await);
        assert_eq!(second.await.unwrap(), DuplicateChoice::Rename);
        assert!(manager.respond("first", DuplicateChoice::Skip).await);
        assert_eq!(first.await.unwrap(), DuplicateChoice::Skip);
        assert!(!manager.respond("second", DuplicateChoice::Overwrite).await);
        assert!(manager.pending.lock().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_answer_timeout_starts_only_after_display() {
        let manager = Arc::new(TransferDuplicateManager::new());
        let (reply, displayed) = manager.register("queued".to_string()).await;
        let waiting = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .wait_for_response(
                        "queued",
                        reply,
                        displayed,
                        None,
                        Duration::from_secs(120),
                        Duration::from_secs(600),
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(121)).await;
        assert!(!waiting.is_finished());
        assert!(manager.mark_displayed("queued").await);
        assert!(manager.mark_displayed("queued").await);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(119)).await;
        assert!(!waiting.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(waiting.await.unwrap().unwrap(), DuplicateChoice::Skip);
        assert!(manager.pending.lock().await.is_empty());
        assert!(!manager.mark_displayed("queued").await);
        assert!(!manager.respond("queued", DuplicateChoice::Rename).await);
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_queue_expiry_removes_an_unshown_request() {
        let manager = TransferDuplicateManager::new();
        let (reply, displayed) = manager.register("unshown".to_string()).await;
        assert_eq!(
            manager
                .wait_for_response(
                    "unshown",
                    reply,
                    displayed,
                    None,
                    Duration::from_secs(120),
                    Duration::from_secs(600)
                )
                .await
                .unwrap(),
            DuplicateChoice::Skip
        );
        assert!(manager.pending.lock().await.is_empty());
        assert!(!manager.respond("unshown", DuplicateChoice::Overwrite).await);
    }

    #[tokio::test]
    async fn duplicate_reply_before_activation_is_still_supported() {
        let manager = TransferDuplicateManager::new();
        let (reply, displayed) = manager.register("early".to_string()).await;
        assert!(manager.respond("early", DuplicateChoice::Rename).await);
        assert_eq!(
            manager
                .wait_for_response(
                    "early",
                    reply,
                    displayed,
                    None,
                    DUPLICATE_PROMPT_TIMEOUT,
                    DUPLICATE_QUEUE_TIMEOUT
                )
                .await
                .unwrap(),
            DuplicateChoice::Rename
        );
        assert!(manager.pending.lock().await.is_empty());
    }
}
