//! One-shot replies remain revocable while the protocol continues to run.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use nyaterm_core::ai::external_agents::AcpPermissionView;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct AgentPermissionRequest {
    view: AcpPermissionView,
    reply: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    stopped: Arc<AtomicBool>,
}

impl std::fmt::Debug for AgentPermissionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPermissionRequest")
            .finish_non_exhaustive()
    }
}

impl AgentPermissionRequest {
    #[cfg(feature = "test-support")]
    pub fn fixture(profile_id: &str) -> (Self, oneshot::Receiver<bool>) {
        Self::new(
            profile_id.into(),
            &json!({"title":"Fixture tool", "rawInput":{"fixture":true}}),
            true,
            Arc::new(AtomicBool::new(false)),
        )
    }
    pub(super) fn new(
        profile_id: String,
        tool: &Value,
        allow_once: bool,
        stopped: Arc<AtomicBool>,
    ) -> (Self, oneshot::Receiver<bool>) {
        let title = tool["title"].as_str().unwrap_or("Agent tool");
        let details = serde_json::to_string_pretty(&json!({
            "input": tool["rawInput"], "locations": tool["locations"],
            "content": tool["content"]
        }))
        .unwrap_or_default();
        // Incomplete review text must never acquire an Allow button.
        let complete = title.len() <= 1024 && details.len() <= 16384;
        let (reply, receive) = oneshot::channel();
        (
            Self {
                view: AcpPermissionView {
                    id: nyaterm_core::uuid(),
                    profile_id,
                    title: title.chars().take(1024).collect(),
                    details: details.chars().take(16384).collect(),
                    allow_once: allow_once && complete,
                },
                reply: Arc::new(Mutex::new(Some(reply))),
                stopped,
            },
            receive,
        )
    }

    pub fn view(&self) -> &AcpPermissionView {
        &self.view
    }

    pub fn is_pending(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self
                .reply
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|reply| !reply.is_canceled())
    }

    pub fn respond(&self, allow: bool) -> bool {
        let Some(reply) = self.reply.lock().unwrap().take() else {
            return false;
        };
        let allow = allow && self.view.allow_once && !self.stopped.load(Ordering::Acquire);
        reply.send(allow).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::acp::permissions::AgentPermissionRequest;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn approvals_are_single_use_revocable_and_redacted() {
        let stopped = Arc::new(AtomicBool::new(false));
        let (request, reply) = AgentPermissionRequest::new(
            "test".into(),
            &json!({"title":"fixture-secret", "rawInput":{"token":"fixture-secret"}}),
            true,
            stopped.clone(),
        );
        assert!(!format!("{request:?} {:?}", request.view()).contains("fixture-secret"));
        stopped.store(true, Ordering::Release);
        assert!(!request.is_pending());
        assert!(request.respond(true));
        assert!(!futures::executor::block_on(reply).unwrap());
        assert!(!request.respond(true));
    }

    #[test]
    fn oversized_review_text_cannot_be_approved() {
        let (request, reply) = AgentPermissionRequest::new(
            "test".into(),
            &json!({"rawInput":{"command":"x".repeat(17000)}}),
            true,
            Arc::new(AtomicBool::new(false)),
        );
        assert!(!request.view().allow_once);
        request.respond(true);
        assert!(!futures::executor::block_on(reply).unwrap());
    }
}
