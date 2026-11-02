use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::capabilities::AgentApprovalDecision;
use nyaterm_mcp_protocol::tool;
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

use super::{AiPanel, AiPanelSnapshot};
use crate::features::capability_runtime::approvals::NativeCapabilityApproval;
use crate::features::shell::gpui_code_font_family;

impl AiPanel {
    pub(super) fn native_approval_card(
        &self,
        snapshot: &AiPanelSnapshot,
        view: &NativeCapabilityApproval,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let request = &view.approval;
        let capability = match request.capability.as_str() {
            tool::TERMINAL_EXECUTE => t!("ai.terminalCommand"),
            tool::SFTP_READ_TEXT => t!("ai.activityReadFile"),
            tool::SFTP_LIST => t!("ai.activityListFiles"),
            tool::SFTP_STAT => t!("ai.activityFileInfo"),
            tool::SESSION_GET => t!("ai.activitySession"),
            _ => request.capability.clone().into(),
        };
        let accent = if request.destructive {
            palette.danger
        } else {
            palette.warning
        };
        let mut card = div()
            .id(format!("ai-native-approval-{}", request.request_id))
            .debug_selector(|| "ai-native-approval".into())
            .w_full()
            .min_w_0()
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(accent))
            .bg(rgb(palette.surface))
            .text_size(px(12.))
            .child(
                div()
                    .text_color(rgb(accent))
                    .child(t!("ai.harness.approvalTitle")),
            )
            .child(div().child(format!("{}: {}", t!("ai.mcp.capability"), capability)))
            .when_some(request.target.clone(), |card, target| {
                card.child(div().child(format!("{}: {target}", t!("ai.mcp.target"))))
            });
        if let Some(command) = &view.command {
            card = card.child(
                div()
                    .debug_selector(|| "ai-native-approval-command".into())
                    .w_full()
                    .min_w_0()
                    .font_family(gpui_code_font_family())
                    .child(self.highlighted_command(command)),
            );
        } else {
            card = card.child(div().min_w_0().child(format!(
                "{}: {}",
                t!("ai.mcp.parameters"),
                request.parameter_summary
            )));
        }
        let risk = match request.risk_level.as_str() {
            "low" => t!("ai.riskLow"),
            "medium" => t!("ai.riskMedium"),
            "high" => t!("ai.riskHigh"),
            "critical" => t!("ai.riskCritical"),
            _ => request.risk_level.clone().into(),
        };
        card = card
            .child(div().child(format!("{}: {risk}", t!("ai.mcp.risk"))))
            .when(request.destructive, |card| {
                card.child(
                    div()
                        .text_color(rgb(palette.danger))
                        .child(t!("ai.mcp.destructiveWarning")),
                )
            });
        let mut actions = div().flex().flex_wrap().items_center().gap_2();
        let mut choices = vec![
            (
                "deny",
                t!("ai.mcp.deny"),
                AgentApprovalDecision::Deny,
                NyaButtonVariant::Secondary,
            ),
            (
                "once",
                t!("ai.mcp.allowOnce"),
                AgentApprovalDecision::AllowOnce,
                NyaButtonVariant::Primary,
            ),
        ];
        if request.allow_session {
            choices.push((
                "run",
                t!("ai.harness.allowRun"),
                AgentApprovalDecision::AllowSession,
                NyaButtonVariant::Secondary,
            ));
        }
        for (key, label, decision, variant) in choices {
            let run_id = view.run_id.clone();
            let ai_session_id = view.ai_session_id.clone();
            let request_id = request.request_id.clone();
            actions = actions.child(
                div()
                    .debug_selector(move || format!("ai-native-approval-{key}"))
                    .child(
                        NyaButton::new(
                            format!("ai-native-approval-{key}-{}", request.request_id),
                            label,
                        )
                        .small()
                        .variant(variant)
                        .when(key == "run", |button| {
                            button.tooltip(t!("ai.harness.allowRunDescription"))
                        })
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.respond_to_native_capability_approval(
                                    &run_id,
                                    &ai_session_id,
                                    &request_id,
                                    decision,
                                    cx,
                                );
                            });
                        })),
                    ),
            );
        }
        card.child(actions)
    }
}
