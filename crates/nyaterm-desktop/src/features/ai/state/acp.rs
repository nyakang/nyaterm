use nyaterm_agent_client::acp::permissions::AgentPermissionRequest;
use nyaterm_core::ai::external_agents::{AcpPermissionView, profile_id};

use super::AiFeatureState;

impl AiFeatureState {
    pub(in crate::features) fn add_acp_approval(
        &mut self,
        job_id: u64,
        approval: AgentPermissionRequest,
    ) -> bool {
        self.chat
            .acp_approvals
            .retain(AgentPermissionRequest::is_pending);
        if job_id != self.chat.job_id
            || !self.chat.pending
            || profile_id(&self.chat.agent_kind) != Some(approval.view().profile_id.as_str())
            || !approval.is_pending()
            || self.chat.acp_approvals.len() >= 32
        {
            approval.respond(false);
            return false;
        }
        self.chat.acp_approvals.push(approval);
        true
    }

    pub(in crate::features) fn acp_approval_views(&self) -> Vec<AcpPermissionView> {
        self.chat
            .acp_approvals
            .iter()
            .filter(|request| request.is_pending())
            .map(|request| request.view().clone())
            .collect()
    }

    pub(in crate::features) fn respond_to_acp_approval(&mut self, id: &str, allow: bool) {
        let Some(index) = self
            .chat
            .acp_approvals
            .iter()
            .position(|request| request.view().id == id)
        else {
            return;
        };
        let request = self.chat.acp_approvals.remove(index);
        request.respond(
            allow
                && self.chat.pending
                && self
                    .chat
                    .cancel
                    .as_ref()
                    .is_some_and(|cancel| !cancel.load(std::sync::atomic::Ordering::Acquire))
                && profile_id(&self.chat.agent_kind) == Some(request.view().profile_id.as_str()),
        );
    }

    pub(super) fn clear_acp_approvals(&mut self) {
        for request in self.chat.acp_approvals.drain(..) {
            request.respond(false);
        }
    }
}
