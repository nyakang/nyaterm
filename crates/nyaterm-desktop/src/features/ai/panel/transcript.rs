use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use gpui::{Context, IntoElement};
use rust_i18n::t;

use crate::features::ai::panel::execution::AiExecutionGroup;
use crate::features::ai::panel::messages::disclosure;
use crate::features::ai::panel::{AiPanel, AiPanelSnapshot};

fn auxiliary_step(kind: AiAgentStepKind) -> bool {
    matches!(
        kind,
        AiAgentStepKind::Planning | AiAgentStepKind::ToolProgress | AiAgentStepKind::Diagnostic
    )
}

/// Stable identity and a lookup into the immutable panel snapshot. Domain data
/// remains owned by AI state; these rows only describe the visible timeline.
#[derive(Clone)]
pub(super) enum AiTranscriptRow {
    Empty,
    Message {
        index: usize,
        id: String,
    },
    ActivityMessage {
        index: usize,
        id: String,
        reasoning_only: bool,
    },
    FinalMessage {
        index: usize,
        id: String,
    },
    Execution {
        group: AiExecutionGroup,
    },
    NativeRun {
        run_id: String,
        history: bool,
    },
    NativeApproval {
        index: usize,
        request_id: String,
    },
    PendingCommand {
        message_index: usize,
        card_index: usize,
        id: String,
    },
    AgentHeader,
    AgentStep {
        index: usize,
        step_index: u16,
    },
    Command {
        index: usize,
        id: String,
    },
}

impl AiTranscriptRow {
    pub(super) fn project(snapshot: &AiPanelSnapshot) -> Vec<Self> {
        let mut rows = Vec::new();
        if snapshot.messages.is_empty() {
            rows.push(Self::Empty);
        } else {
            let groups: HashMap<usize, AiExecutionGroup> = AiExecutionGroup::project(snapshot)
                .into_iter()
                .map(|group| (group.messages.start, group))
                .collect();
            let mut index = 0;
            while index < snapshot.messages.len() {
                if let Some(group) = groups.get(&index) {
                    rows.push(Self::Execution {
                        group: group.clone(),
                    });
                    let open = snapshot.expanded_execution_groups.contains(&group.id);
                    // A native run is cleared when the next user request starts.
                    // Its current plan belongs to the last request's history.
                    if group.messages.end == snapshot.messages.len()
                        && let Some(view) = &snapshot.native_run
                        && ((open && (!view.plan.tasks.is_empty() || view.verification.is_some()))
                            || view.status
                                == nyaterm_core::ai::harness::AgentRunStatus::WaitingForUser)
                    {
                        rows.push(Self::NativeRun {
                            run_id: view.run_id.clone(),
                            history: open,
                        });
                    }
                    if open {
                        for index in group.messages.clone() {
                            let message = &snapshot.messages[index];
                            if group.final_message == Some(index) {
                                if message
                                    .reasoning_content
                                    .as_deref()
                                    .is_some_and(|text| !text.trim().is_empty())
                                    || crate::features::formatting::markdown::think_content_presence(
                                        &message.content,
                                    )
                                    .1
                                {
                                    rows.push(Self::ActivityMessage {
                                        index,
                                        id: message.id.clone(),
                                        reasoning_only: true,
                                    });
                                }
                            } else {
                                rows.push(Self::ActivityMessage {
                                    index,
                                    id: message.id.clone(),
                                    reasoning_only: false,
                                });
                            }
                        }
                    } else {
                        // User decisions remain accessible when the history is collapsed.
                        for message_index in group.messages.clone() {
                            for (card_index, card) in snapshot.messages[message_index]
                                .command_cards
                                .iter()
                                .enumerate()
                            {
                                if snapshot.command_phase(card).offers_approval() {
                                    rows.push(Self::PendingCommand {
                                        message_index,
                                        card_index,
                                        id: card.id.clone(),
                                    });
                                }
                            }
                        }
                    }
                    if let Some(index) = group.final_message {
                        rows.push(Self::FinalMessage {
                            index,
                            id: snapshot.messages[index].id.clone(),
                        });
                    }
                    if group.messages.end == snapshot.messages.len() {
                        rows.extend(Self::native_approval_rows(snapshot));
                    }
                    index = group.messages.end;
                } else {
                    let message = &snapshot.messages[index];
                    rows.push(Self::Message {
                        index,
                        id: message.id.clone(),
                    });
                    index += 1;
                }
            }
        }

        // A tool can request approval before a plan or execution group exists.
        if !rows
            .iter()
            .any(|row| matches!(row, Self::NativeApproval { .. }))
        {
            rows.extend(Self::native_approval_rows(snapshot));
        }

        let steps: Vec<_> = snapshot
            .agent_steps
            .iter()
            .enumerate()
            .skip(snapshot.agent_steps.len().saturating_sub(16))
            .filter(|(_, presentation)| {
                let step = &presentation.step;
                let linked_command = step
                    .command_card_id
                    .as_deref()
                    .is_some_and(|id| snapshot.command_step(id).is_some());
                let linked_answer = step.kind == AiAgentStepKind::FinalAnswer
                    && step.source_message_id.as_ref().is_some_and(|id| {
                        snapshot.messages.iter().any(|message| &message.id == id)
                    });
                !linked_command && !linked_answer && !snapshot.step_in_message(step.step_index)
            })
            .collect();
        if steps.iter().any(|(_, presentation)| {
            auxiliary_step(presentation.step.kind)
                && !snapshot.step_is_active(presentation.step.step_index)
        }) {
            rows.push(Self::AgentHeader);
        }
        rows.extend(
            steps
                .into_iter()
                .filter(|(_, presentation)| {
                    !auxiliary_step(presentation.step.kind)
                        || snapshot.step_is_active(presentation.step.step_index)
                        || snapshot.agent_history_expanded
                })
                .map(|(index, presentation)| Self::AgentStep {
                    index,
                    step_index: presentation.step.step_index,
                }),
        );
        let mut shown = HashSet::new();
        rows.extend(
            snapshot
                .command_cards
                .iter()
                .take(8)
                .enumerate()
                .filter(|(_, card)| {
                    snapshot.card_owner(&card.id).is_none() && shown.insert(card.id.as_str())
                })
                .map(|(index, card)| Self::Command {
                    index,
                    id: card.id.clone(),
                }),
        );
        // A truly empty conversation uses the full-height introduction outside
        // the list, so it can remain centered in the available viewport.
        if matches!(rows.as_slice(), [Self::Empty]) {
            rows.clear();
        }
        rows
    }

    fn native_approval_rows(snapshot: &AiPanelSnapshot) -> impl Iterator<Item = Self> + '_ {
        snapshot
            .native_approvals
            .iter()
            .enumerate()
            .map(|(index, view)| Self::NativeApproval {
                index,
                request_id: view.approval.request_id.clone(),
            })
    }

    fn same_item(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Message { id: a, .. }, Self::Message { id: b, .. })
            | (Self::FinalMessage { id: a, .. }, Self::FinalMessage { id: b, .. })
            | (Self::PendingCommand { id: a, .. }, Self::PendingCommand { id: b, .. })
            | (Self::Command { id: a, .. }, Self::Command { id: b, .. }) => a == b,
            (
                Self::ActivityMessage {
                    id: a,
                    reasoning_only: ar,
                    ..
                },
                Self::ActivityMessage {
                    id: b,
                    reasoning_only: br,
                    ..
                },
            ) => a == b && ar == br,
            (Self::Execution { group: a }, Self::Execution { group: b }) => a.id == b.id,
            (Self::NativeRun { run_id: a, .. }, Self::NativeRun { run_id: b, .. }) => a == b,
            (
                Self::NativeApproval { request_id: a, .. },
                Self::NativeApproval { request_id: b, .. },
            ) => a == b,
            (Self::AgentStep { step_index: a, .. }, Self::AgentStep { step_index: b, .. }) => {
                a == b
            }
            (Self::Empty, Self::Empty) | (Self::AgentHeader, Self::AgentHeader) => true,
            _ => false,
        }
    }

    fn content_changed(
        &self,
        previous: &AiPanelSnapshot,
        other: &Self,
        next: &AiPanelSnapshot,
    ) -> bool {
        match (self, other) {
            (Self::NativeApproval { index: a, .. }, Self::NativeApproval { index: b, .. }) => {
                previous.native_approvals[*a] != next.native_approvals[*b]
            }
            (Self::NativeRun { history: a, .. }, Self::NativeRun { history: b, .. }) => {
                a != b
                    || match (&previous.native_run, &next.native_run) {
                        (Some(a), Some(b)) => {
                            a.call_id != b.call_id
                                || a.status != b.status
                                || a.plan != b.plan
                                || a.questions != b.questions
                                || a.answers != b.answers
                                || a.verification != b.verification
                        }
                        _ => true,
                    }
            }
            (Self::Message { index: a, id }, Self::Message { index: b, .. })
            | (
                Self::ActivityMessage { index: a, id, .. },
                Self::ActivityMessage { index: b, .. },
            )
            | (Self::FinalMessage { index: a, id }, Self::FinalMessage { index: b, .. }) => {
                (!std::sync::Arc::ptr_eq(&previous.messages[*a], &next.messages[*b])
                    && previous.messages[*a] != next.messages[*b])
                    || (previous.streaming_assistant_id.as_ref() == Some(id))
                        != (next.streaming_assistant_id.as_ref() == Some(id))
                    || (next.streaming_assistant_id.as_ref() == Some(id)
                        && previous.response_phase != next.response_phase)
                    || previous.expanded_message_thoughts.contains(id)
                        != next.expanded_message_thoughts.contains(id)
                    || !previous.message_steps(id).eq(next.message_steps(id))
                    || next.message_steps(id).any(|presentation| {
                        let key = next.step_command_key(&presentation.step);
                        presentation.step.command.is_some()
                            && previous.expanded_command_scripts.contains(&key)
                                != next.expanded_command_scripts.contains(&key)
                    })
                    || (previous.running != next.running && next.message_steps(id).next().is_some())
                    || next.messages[*b].command_cards.iter().any(|card| {
                        (previous.running != next.running
                            && next.command_phase(card) == AiCommandPhase::Preparing)
                            || previous.command_step(&card.id) != next.command_step(&card.id)
                            || previous.card_owner(&card.id) != next.card_owner(&card.id)
                            || previous.expanded_command_details.contains(&card.id)
                                != next.expanded_command_details.contains(&card.id)
                            || previous.expanded_command_scripts.contains(&card.id)
                                != next.expanded_command_scripts.contains(&card.id)
                    })
            }
            (Self::Execution { group: a }, Self::Execution { group: b }) => {
                a != b
                    || a.is_live(previous) != b.is_live(next)
                    || (b.is_live(next) && previous.response_phase != next.response_phase)
                    || previous.expanded_execution_groups.contains(&a.id)
                        != next.expanded_execution_groups.contains(&b.id)
                    || a.steps
                        .iter()
                        .map(|index| &previous.agent_steps[*index])
                        .ne(b.steps.iter().map(|index| &next.agent_steps[*index]))
                    || previous.messages[a.messages.clone()] != next.messages[b.messages.clone()]
            }
            (
                Self::PendingCommand {
                    message_index: a,
                    card_index: ca,
                    ..
                },
                Self::PendingCommand {
                    message_index: b,
                    card_index: cb,
                    ..
                },
            ) => {
                let a = &previous.messages[*a].command_cards[*ca];
                let b = &next.messages[*b].command_cards[*cb];
                a != b
                    || previous.command_step(&a.id) != next.command_step(&b.id)
                    || previous.expanded_command_details.contains(&a.id)
                        != next.expanded_command_details.contains(&b.id)
            }
            (Self::AgentStep { index: a, .. }, Self::AgentStep { index: b, .. }) => {
                let a = &previous.agent_steps[*a];
                let b = &next.agent_steps[*b];
                a != b
                    || previous.step_is_active(a.step.step_index)
                        != next.step_is_active(b.step.step_index)
                    || previous
                        .expanded_command_scripts
                        .contains(&previous.step_command_key(&a.step))
                        != next
                            .expanded_command_scripts
                            .contains(&next.step_command_key(&b.step))
            }
            (Self::Command { index: a, .. }, Self::Command { index: b, .. }) => {
                let id = &next.command_cards[*b].id;
                previous.command_cards[*a] != next.command_cards[*b]
                    || (previous.running != next.running
                        && next.command_phase(&next.command_cards[*b]) == AiCommandPhase::Preparing)
                    || previous.command_step(id) != next.command_step(id)
                    || previous.expanded_command_details.contains(id)
                        != next.expanded_command_details.contains(id)
                    || previous.expanded_command_scripts.contains(id)
                        != next.expanded_command_scripts.contains(id)
            }
            (Self::AgentHeader, Self::AgentHeader) => {
                previous.agent_history_expanded != next.agent_history_expanded
            }
            // Setup and enabled-model changes can replace the introduction.
            (Self::Empty, Self::Empty) => {
                previous.enabled != next.enabled
                    || previous.external_agent != next.external_agent
                    || previous.selected_model_id != next.selected_model_id
                    || previous.enabled_models.is_empty() != next.enabled_models.is_empty()
            }
            _ => false,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct AiTranscriptUpdate {
    pub reset: bool,
    pub splice: Option<(Range<usize>, usize)>,
    pub remeasure_all: bool,
    pub remeasure: Vec<Range<usize>>,
}

impl AiTranscriptUpdate {
    pub(super) fn between(
        previous: Option<&AiPanelSnapshot>,
        next: &AiPanelSnapshot,
        old_rows: &[AiTranscriptRow],
        rows: &[AiTranscriptRow],
    ) -> Self {
        let Some(previous) = previous.filter(|previous| {
            previous.current_ai_session_id == next.current_ai_session_id
                && previous.owner_terminal_id == next.owner_terminal_id
                && previous.owner_connection_id == next.owner_connection_id
        }) else {
            return Self {
                reset: true,
                ..Self::default()
            };
        };
        let prefix = old_rows
            .iter()
            .zip(rows)
            .take_while(|(a, b)| a.same_item(b))
            .count();
        let suffix = old_rows[prefix..]
            .iter()
            .rev()
            .zip(rows[prefix..].iter().rev())
            .take_while(|(a, b)| a.same_item(b))
            .count();
        let old_end = old_rows.len() - suffix;
        let new_end = rows.len() - suffix;
        let mut update = Self {
            splice: (prefix != old_end || prefix != new_end)
                .then_some((prefix..old_end, new_end - prefix)),
            remeasure_all: previous.chrome.palette != next.chrome.palette
                || previous.ui_font_family != next.ui_font_family
                || previous.chrome.viewport_width != next.chrome.viewport_width
                || previous.chrome.viewport_height != next.chrome.viewport_height,
            ..Self::default()
        };
        if !update.remeasure_all {
            for (old_index, new_index) in (0..prefix)
                .map(|index| (index, index))
                .chain((0..suffix).map(|index| (old_end + index, new_end + index)))
            {
                if old_rows[old_index].content_changed(previous, &rows[new_index], next) {
                    update.remeasure.push(new_index..new_index + 1);
                }
            }
        }
        update
    }
}

impl AiPanel {
    pub(super) fn ai_transcript_row(
        &self,
        snapshot: &AiPanelSnapshot,
        row: &AiTranscriptRow,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match row {
            AiTranscriptRow::Empty => self.ai_empty_transcript(snapshot, cx),
            AiTranscriptRow::Message { index, .. } => self
                .ai_message_bubble(snapshot, &snapshot.messages[*index], cx)
                .into_any_element(),
            AiTranscriptRow::ActivityMessage {
                index,
                reasoning_only,
                ..
            } => {
                self.ai_activity_message(snapshot, &snapshot.messages[*index], *reasoning_only, cx)
            }
            AiTranscriptRow::FinalMessage { index, .. } => {
                self.ai_final_message(snapshot, &snapshot.messages[*index], cx)
            }
            AiTranscriptRow::Execution { group } => self.ai_execution_header(snapshot, group, cx),
            AiTranscriptRow::NativeApproval { index, .. } => self
                .native_approval_card(snapshot, &snapshot.native_approvals[*index], cx)
                .into_any_element(),
            AiTranscriptRow::NativeRun { history, .. } => self
                .native_run_card(
                    snapshot,
                    snapshot.native_run.clone().expect("projected native run"),
                    *history,
                    cx,
                )
                .into_any_element(),
            AiTranscriptRow::PendingCommand {
                message_index,
                card_index,
                ..
            } => self.ai_command_card_view(
                snapshot,
                snapshot.messages[*message_index].command_cards[*card_index].clone(),
                cx,
            ),
            AiTranscriptRow::AgentHeader => disclosure(
                "ai-agent-history".into(),
                t!("ai.agentActivityHistory"),
                snapshot.agent_history_expanded,
                cx.listener(|panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.ai.toggle_agent_history();
                        app.defer_ai_panel_snapshot_flush(cx);
                    });
                }),
            ),
            AiTranscriptRow::AgentStep { index, .. } => {
                self.ai_agent_step_card(snapshot, snapshot.agent_steps[*index].clone(), cx)
            }
            AiTranscriptRow::Command { index, .. } => {
                self.ai_command_card_view(snapshot, snapshot.command_cards[*index].clone(), cx)
            }
        }
    }
}
