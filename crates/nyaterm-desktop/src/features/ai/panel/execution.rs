//! Request-scoped execution groups projected from the existing transcript.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::ai::AiMessageRole;
use nyaterm_ui::chat::running_indicator;
use rust_i18n::t;

use crate::features::ai::is_agent_command_card;
use crate::features::ai::panel::activity::{
    AiActivityItemState, AiActivityStatus, AiActivitySummary,
};
use crate::features::ai::panel::messages::disclosure;
use crate::features::ai::panel::transcript::AiTranscriptRow;
use crate::features::ai::panel::{AiPanel, AiPanelSnapshot};
use crate::features::ai::presentation::{AiAgentStepKind, AiResponsePhase};
use crate::features::formatting::markdown::think_content_presence;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AiExecutionGroup {
    pub id: String,
    pub messages: Range<usize>,
    pub final_message: Option<usize>,
    pub steps: Vec<usize>,
}

impl AiExecutionGroup {
    pub(super) fn project(snapshot: &AiPanelSnapshot) -> Vec<Self> {
        let indices: HashMap<&str, usize> = snapshot
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| (message.id.as_str(), index))
            .collect();
        let mut groups = Vec::new();
        let mut steps_by_message: HashMap<usize, Vec<usize>> = HashMap::new();
        for (position, presentation) in snapshot.agent_steps.iter().enumerate() {
            let step = &presentation.step;
            let source = step
                .source_message_id
                .as_deref()
                .filter(|id| indices.contains_key(id))
                .or_else(|| {
                    snapshot
                        .step_is_active(step.step_index)
                        .then_some(snapshot.streaming_assistant_id.as_deref())
                        .flatten()
                });
            if let Some(index) = source.and_then(|id| indices.get(id)) {
                steps_by_message.entry(*index).or_default().push(position);
            }
        }
        let mut start = 0;
        let mut owner = None;
        for end in 0..=snapshot.messages.len() {
            if end != snapshot.messages.len() && snapshot.messages[end].role != AiMessageRole::User
            {
                continue;
            }
            let range = start..end;
            let mut steps: Vec<usize> = range
                .clone()
                .flat_map(|index| steps_by_message.get(&index).into_iter().flatten().copied())
                .collect();
            steps.sort_unstable();
            let has_execution = snapshot.messages[range.clone()].iter().any(|message| {
                message.role == AiMessageRole::Assistant
                    && message.command_cards.iter().any(|card| {
                        is_agent_command_card(card) || snapshot.command_step(&card.id).is_some()
                    })
            }) || steps.iter().any(|index| {
                snapshot.agent_steps[*index].step.kind != AiAgentStepKind::FinalAnswer
            }) || (end == snapshot.messages.len()
                && (!snapshot.native_approvals.is_empty()
                    || snapshot.native_run.as_ref().is_some_and(|view| {
                        !view.plan.tasks.is_empty()
                            || !view.questions.is_empty()
                            || view.verification.is_some()
                    })));
            if has_execution && !range.is_empty() {
                let final_message = range
                    .clone()
                    .rev()
                    .find(|index| {
                        let message = &snapshot.messages[*index];
                        message.role == AiMessageRole::Assistant
                            && think_content_presence(&message.content).0
                    })
                    .filter(|index| {
                        let message = &snapshot.messages[*index];
                        if !message.command_cards.is_empty() {
                            return false;
                        }
                        let source_steps: Vec<_> = steps
                            .iter()
                            .map(|index| &snapshot.agent_steps[*index].step)
                            .filter(|step| {
                                step.source_message_id.as_deref() == Some(message.id.as_str())
                            })
                            .collect();
                        let final_step = source_steps
                            .iter()
                            .any(|step| step.kind == AiAgentStepKind::FinalAnswer);
                        let tool_step = source_steps
                            .iter()
                            .any(|step| step.kind != AiAgentStepKind::FinalAnswer);
                        let still_preparing = snapshot.running
                            && snapshot.streaming_assistant_id.as_deref()
                                == Some(message.id.as_str())
                            && snapshot.response_phase != AiResponsePhase::Responding;
                        final_step || (!tool_step && !still_preparing)
                    });
                groups.push(Self {
                    id: owner
                        .clone()
                        .unwrap_or_else(|| snapshot.messages[start].id.clone()),
                    messages: range,
                    final_message,
                    steps,
                });
            }
            if end < snapshot.messages.len() {
                owner = Some(snapshot.messages[end].id.clone());
                start = end + 1;
            }
        }
        groups
    }

    pub(super) fn is_live(&self, snapshot: &AiPanelSnapshot) -> bool {
        snapshot.running
            && (self
                .steps
                .iter()
                .any(|index| snapshot.step_is_active(snapshot.agent_steps[*index].step.step_index))
                || self.messages.clone().any(|index| {
                    snapshot.streaming_assistant_id.as_deref()
                        == Some(snapshot.messages[index].id.as_str())
                }))
    }

    pub(super) fn step_count(&self, snapshot: &AiPanelSnapshot) -> usize {
        let commands = snapshot.messages[self.messages.clone()]
            .iter()
            .flat_map(|message| &message.command_cards)
            .map(|card| card.id.as_str())
            .collect::<HashSet<_>>()
            .len();
        commands
            + self
                .steps
                .iter()
                .filter(|index| {
                    let step = &snapshot.agent_steps[**index].step;
                    step.command_card_id.is_none()
                        && matches!(
                            step.kind,
                            AiAgentStepKind::ToolProgress
                                | AiAgentStepKind::Diagnostic
                                | AiAgentStepKind::Command
                                | AiAgentStepKind::Observation
                        )
                })
                .count()
    }
}

impl AiPanel {
    pub(super) fn ai_execution_header(
        &self,
        snapshot: &AiPanelSnapshot,
        group: &AiExecutionGroup,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = group.id.clone();
        let palette = snapshot.chrome.palette;
        let open = snapshot.expanded_execution_groups.contains(&id);
        let summary = AiActivitySummary::project(snapshot, group);
        let total = if summary.total > 0 {
            summary.total
        } else {
            group.step_count(snapshot)
        };
        let completed = summary.completed.min(total);
        let (status_label, status_color) = match summary.status {
            AiActivityStatus::Running => (t!("ai.activity.running"), palette.link),
            AiActivityStatus::NeedsApproval => (t!("ai.activity.needsApproval"), palette.warning),
            AiActivityStatus::WaitingUser => (t!("ai.activity.waitingForUser"), palette.link),
            AiActivityStatus::Failed => (t!("ai.activity.failed"), palette.danger),
            AiActivityStatus::Completed => (t!("ai.activity.completed"), palette.success),
        };
        let label = if total > 0 {
            format!(
                "{} · {}",
                t!("ai.activity.title"),
                t!("ai.activity.progress", completed = completed, total = total)
            )
        } else {
            t!("ai.activity.title").to_string()
        };
        let detail = summary
            .current_label
            .clone()
            .unwrap_or(status_label.to_string());
        let live = group.is_live(snapshot);
        let mut card = div()
            .debug_selector({
                let id = id.clone();
                move || format!("ai-activity-{id}")
            })
            .min_w_0()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .pb_2()
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(self.ai_agent_turn_header(snapshot, live))
            .child(
                div()
                    .min_w_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .child(if summary.status == AiActivityStatus::Running {
                        running_indicator(format!("ai-execution-running-{id}"), rgb(palette.link))
                            .into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .size(px(6.))
                            .mx_1()
                            .rounded_full()
                            .bg(rgb(status_color))
                            .into_any_element()
                    })
                    .child(disclosure(
                        format!("ai-execution-toggle-{id}"),
                        label,
                        open,
                        cx.listener(move |panel, _, _, cx| {
                            let anchor = panel.transcript_rows.iter().position(
                                |row| matches!(row, AiTranscriptRow::Execution { group } if group.id == id),
                            );
                            if !open && let Some(index) = anchor {
                                // Anchor the reading position before inserting history rows.
                                panel.transcript_scroll.update(cx, |scroll, cx| {
                                    scroll.scroll_to_item(index, cx);
                                });
                            }
                            let id = id.clone();
                            panel.with_app(cx, move |app, cx| {
                                app.ai.toggle_execution_group(id);
                                app.defer_ai_panel_snapshot_flush(cx);
                            });
                        }),
                    ))
                    .child(
                        div()
                            .debug_selector({
                                let id = group.id.clone();
                                move || format!("ai-activity-detail-{id}")
                            })
                            .min_w_0()
                            .flex_1()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(
                                if summary.status == AiActivityStatus::NeedsApproval {
                                    palette.warning
                                } else {
                                    palette.text_muted
                                },
                            ))
                            .child(detail),
                    ),
            );
        if open && summary.from_plan && !summary.items.is_empty() {
            let mut list = div()
                .debug_selector({
                    let id = group.id.clone();
                    move || format!("ai-activity-plan-{id}")
                })
                .w_full()
                .min_w_0()
                .pl_2()
                .flex()
                .flex_col()
                .gap_1();
            for item in &summary.items {
                let (marker, color) = match item.state {
                    AiActivityItemState::Pending => ("○", palette.text_muted),
                    AiActivityItemState::Running => ("→", palette.link),
                    AiActivityItemState::NeedsApproval => ("!", palette.warning),
                    AiActivityItemState::Completed => ("✓", palette.success),
                    AiActivityItemState::Failed => ("!", palette.danger),
                    AiActivityItemState::Skipped => ("–", palette.text_dimmed),
                };
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .text_color(rgb(color))
                        .child(div().flex_none().w(px(12.)).child(marker))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_ellipsis()
                                .text_color(rgb(palette.text))
                                .child(item.label.clone()),
                        ),
                );
            }
            card = card.child(list);
        }
        card.into_any_element()
    }
}
