use nyaterm_core::ai::{AiAgentStepStatus, AiCommandCard, AiMessageRole};

use super::{TextKey, TextOwner};
use crate::features::ai::panel::transcript::AiTranscriptRow;
use crate::features::ai::panel::{AiAgentStepPresentation, AiPanel, AiPanelSnapshot};
use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};

impl AiPanel {
    pub(super) fn transcript_text_order(
        &self,
        snapshot: &AiPanelSnapshot,
        rows: &[AiTranscriptRow],
    ) -> Vec<(usize, TextKey)> {
        let mut result = Vec::new();
        for (row_index, row) in rows.iter().enumerate() {
            let mut keys = Vec::new();
            match row {
                AiTranscriptRow::Message { index, .. }
                | AiTranscriptRow::ActivityMessage { index, .. }
                | AiTranscriptRow::FinalMessage { index, .. } => {
                    let message = &snapshot.messages[*index];
                    let activity = matches!(row, AiTranscriptRow::ActivityMessage { .. });
                    let final_answer = matches!(row, AiTranscriptRow::FinalMessage { .. });
                    let reasoning_only = matches!(
                        row,
                        AiTranscriptRow::ActivityMessage {
                            reasoning_only: true,
                            ..
                        }
                    );
                    let owner = TextOwner::Message(message.id.clone());
                    if let Some(prepared) = self.prepared_message(&message.id) {
                        if let Some(thought) = &prepared.reasoning {
                            let show = if activity {
                                thought.trim() != prepared.display.trim()
                                    && !message
                                        .command_cards
                                        .iter()
                                        .any(|card| card.explanation.trim() == thought.trim())
                                    && snapshot.activity_thought_open(&message.id)
                            } else {
                                !final_answer
                                    && snapshot.expanded_message_thoughts.contains(&message.id)
                            };
                            if show {
                                keys.push((owner.clone(), "thought".into()));
                            }
                        }
                        let duplicate = message.role != AiMessageRole::User
                            && !message.command_cards.is_empty()
                            && message.command_cards.iter().all(|card| {
                                !card.explanation.trim().is_empty()
                                    && card.explanation.trim() == prepared.display.trim()
                            });
                        if !reasoning_only && !duplicate {
                            keys.push((owner, "answer".into()));
                        }
                    }
                    for card in &message.command_cards {
                        if snapshot.card_owner(&card.id) == Some(message.id.as_str()) {
                            self.command_text_order(snapshot, card, activity, &mut keys);
                        }
                    }
                    for step in snapshot.message_steps(&message.id) {
                        self.step_text_order(snapshot, step, activity, &mut keys);
                    }
                }
                AiTranscriptRow::Command { index, .. } => self.command_text_order(
                    snapshot,
                    &snapshot.command_cards[*index],
                    false,
                    &mut keys,
                ),
                AiTranscriptRow::PendingCommand {
                    message_index,
                    card_index,
                    ..
                } => self.command_text_order(
                    snapshot,
                    &snapshot.messages[*message_index].command_cards[*card_index],
                    false,
                    &mut keys,
                ),
                AiTranscriptRow::AgentStep { index, .. } => {
                    self.step_text_order(snapshot, &snapshot.agent_steps[*index], false, &mut keys)
                }
                AiTranscriptRow::NativeApproval { request_id, .. } => {
                    let owner = TextOwner::Approval(request_id.clone());
                    if let Some(view) = snapshot
                        .native_approvals
                        .iter()
                        .find(|view| view.approval.request_id == *request_id)
                    {
                        keys.push((owner.clone(), "target".into()));
                        keys.push((
                            owner,
                            if view.command.is_some() {
                                "script"
                            } else {
                                "parameters"
                            }
                            .into(),
                        ));
                    }
                }
                AiTranscriptRow::NativeRun { history, .. } => {
                    if let Some(view) = &snapshot.native_run {
                        let owner = TextOwner::Run(view.run_id.clone());
                        if *history {
                            keys.push((owner.clone(), "verification".into()));
                        }
                        if view.status == nyaterm_core::ai::harness::AgentRunStatus::WaitingForUser
                        {
                            for index in 0..view.questions.len() {
                                keys.push((owner.clone(), format!("question-{index}")));
                            }
                        }
                    }
                }
                _ => {}
            }
            result.extend(keys.into_iter().map(|key| (row_index, key)));
        }
        result
    }

    fn step_text_order(
        &self,
        snapshot: &AiPanelSnapshot,
        presentation: &AiAgentStepPresentation,
        activity: bool,
        keys: &mut Vec<TextKey>,
    ) {
        let step = &presentation.step;
        let owner = TextOwner::Step(step.step_index);
        if step.kind == AiAgentStepKind::FinalAnswer {
            keys.push((owner, "detail".into()));
            return;
        }
        if let Some(thought) = step.thought.as_ref().filter(|text| !text.trim().is_empty()) {
            let duplicate = step.source_message_id.as_ref().is_some_and(|id| {
                snapshot.index.message(snapshot, id).is_some_and(|message| {
                    self.prepared_message(id)
                        .is_some_and(|prepared| prepared.display.trim() == thought.trim())
                        || message
                            .reasoning_content
                            .as_deref()
                            .is_some_and(|text| text.trim() == thought.trim())
                })
            });
            if if activity {
                step.kind != AiAgentStepKind::Planning && !duplicate
            } else {
                presentation.thought_open
            } {
                keys.push((owner.clone(), "thought".into()));
            }
        }
        if step.status == AiAgentStepStatus::NeedsApproval
            || snapshot
                .expanded_command_scripts
                .contains(&snapshot.step_command_key(step))
        {
            keys.push((owner.clone(), "script".into()));
        }
        if step.observation.is_some() {
            if presentation.output_open {
                keys.push((owner, "output".into()));
            }
        } else if step.kind == AiAgentStepKind::Diagnostic {
            keys.push((owner, "detail".into()));
        }
    }

    fn command_text_order(
        &self,
        snapshot: &AiPanelSnapshot,
        card: &AiCommandCard,
        activity: bool,
        keys: &mut Vec<TextKey>,
    ) {
        let owner = TextOwner::Command(card.id.clone());
        let phase = snapshot.command_phase(card);
        if activity {
            keys.push((owner.clone(), "explanation".into()));
            if !phase.offers_approval() && !snapshot.expanded_command_scripts.contains(&card.id) {
                return;
            }
        } else {
            keys.push((owner.clone(), "heading".into()));
        }
        keys.push((owner.clone(), "target-label".into()));
        if !activity {
            keys.push((owner.clone(), "remainder".into()));
        }
        if phase == AiCommandPhase::NeedsApproval {
            keys.push((owner.clone(), "risk".into()));
            keys.push((owner.clone(), "risk-reason".into()));
        }
        if let Some(presentation) = snapshot.command_step(&card.id) {
            let step = &presentation.step;
            if matches!(
                phase,
                AiCommandPhase::Failed | AiCommandPhase::Rejected | AiCommandPhase::Cancelled
            ) && step.observation.is_none()
            {
                keys.push((owner.clone(), "detail".into()));
            }
            let thought_in_message = step.source_message_id.as_ref().is_some_and(|id| {
                snapshot.index.message(snapshot, id).is_some_and(|message| {
                    message.reasoning_content.is_some()
                        || self
                            .prepared_message(id)
                            .is_some_and(|prepared| prepared.reasoning.is_some())
                })
            });
            if !activity
                && !thought_in_message
                && step.thought.as_ref().is_some_and(|text| {
                    !text.trim().is_empty() && text.trim() != card.explanation.trim()
                })
                && presentation.thought_open
            {
                keys.push((owner.clone(), "thought".into()));
            }
            keys.push((owner.clone(), "output-preview".into()));
            keys.push((owner.clone(), "exit-code".into()));
            if presentation.output_open {
                keys.push((owner.clone(), "output".into()));
            }
        }
        keys.push((owner.clone(), "script".into()));
        if snapshot.expanded_command_details.contains(&card.id) {
            if card.target_terminal_session_id.is_some() {
                keys.push((owner.clone(), "target-session".into()));
            }
            keys.push((owner.clone(), "host".into()));
            if phase != AiCommandPhase::NeedsApproval {
                keys.push((owner.clone(), "risk".into()));
                keys.push((owner.clone(), "risk-reason".into()));
            }
            keys.push((owner.clone(), "expected-effect".into()));
            keys.push((owner, "rollback".into()));
        }
    }
}
