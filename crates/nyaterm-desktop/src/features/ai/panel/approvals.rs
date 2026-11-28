use gpui::{Context, IntoElement};

use super::texts::TextOwner;
use super::{AiPanel, AiPanelSnapshot};
use crate::features::capability_runtime::approvals::NativeCapabilityApproval;

impl AiPanel {
    pub(super) fn acp_approval_card(
        &self,
        snapshot: &AiPanelSnapshot,
        view: &nyaterm_core::ai::external_agents::AcpPermissionView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let owner = cx.weak_entity();
        let session_id = snapshot.current_ai_session_id.clone();
        let request_id = view.id.clone();
        let handler = std::rc::Rc::new(move |allow, cx: &mut gpui::App| {
            let _ = owner.update(cx, |panel, cx| {
                panel.with_app(cx, |app, cx| {
                    let Some(scope) = app.ai.scope_for_ai_session(&session_id) else {
                        return;
                    };
                    if scope != app.ai.active_scope_key() {
                        return;
                    }
                    app.ai.respond_to_acp_approval(&request_id, allow);
                    app.defer_ai_panel_snapshot_flush(cx);
                });
            });
        });
        nyaterm_ai_gpui::approvals::acp_approval_card(
            snapshot.chrome.palette,
            snapshot.agent_label.clone(),
            view.clone(),
            handler,
        )
    }

    pub(super) fn native_approval_card(
        &self,
        snapshot: &AiPanelSnapshot,
        view: &NativeCapabilityApproval,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let request = &view.approval;
        let target = request.target.as_ref().map(|_| {
            self.selectable_text(
                TextOwner::Approval(request.request_id.clone()),
                "target",
                false,
                cx,
            )
            .into_any_element()
        });
        let content = self
            .selectable_text(
                TextOwner::Approval(request.request_id.clone()),
                if view.command.is_some() {
                    "script"
                } else {
                    "parameters"
                },
                false,
                cx,
            )
            .into_any_element();
        let owner = cx.weak_entity();
        let run_id = view.run_id.clone();
        let ai_session_id = view.ai_session_id.clone();
        let request_id = request.request_id.clone();
        let handler = std::rc::Rc::new(move |decision, cx: &mut gpui::App| {
            let _ = owner.update(cx, |panel, cx| {
                panel.with_app(cx, |app, cx| {
                    app.respond_to_native_capability_approval(
                        &run_id,
                        &ai_session_id,
                        &request_id,
                        decision,
                        cx,
                    )
                });
            });
        });
        nyaterm_ai_gpui::approvals::native_approval_card(
            snapshot.chrome.palette,
            nyaterm_ai_gpui::approvals::ApprovalView {
                request_id: request.request_id.clone(),
                capability: request.capability.clone(),
                risk_level: request.risk_level.clone(),
                destructive: request.destructive,
                allow_session: request.allow_session,
                has_target: request.target.is_some(),
                has_command: view.command.is_some(),
            },
            target,
            content,
            handler,
        )
    }
}
