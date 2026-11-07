use std::collections::HashSet;

use nyaterm_core::AiMessageRole;
use nyaterm_core::ai::CommandObservation;

use crate::features::ai::presentation::{AiAgentStepKind, AiResponsePhase};
use crate::features::ai::state::AiFeatureState;
use crate::features::formatting::markdown::think_content_presence;
use crate::features::runtime_jobs::{AiAgentStepStatus, AiAgentStepView};

impl AiFeatureState {
    pub(in crate::features) fn conversation_title(&self) -> Option<String> {
        self.chat
            .messages
            .iter()
            .find(|message| {
                message.role == AiMessageRole::User && !message.content.trim().is_empty()
            })
            .map(|message| {
                message
                    .content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
    }

    pub(in crate::features) fn associate_agent_command(&mut self, step_index: u16, card_id: &str) {
        let source = self
            .chat
            .messages
            .iter()
            .find(|message| message.command_cards.iter().any(|card| card.id == card_id))
            .map(|message| message.id.clone());
        if let Some(step) = self
            .agent
            .steps
            .iter_mut()
            .find(|step| step.step_index == step_index)
        {
            step.command_card_id = Some(card_id.to_string());
            step.source_message_id = source;
        }
    }
    pub(in crate::features) fn response_phase(&self) -> AiResponsePhase {
        self.chat.response_phase
    }

    pub(super) fn refresh_response_phase(&mut self) {
        if self.chat.response_phase == AiResponsePhase::Responding {
            return;
        }
        let Some(message) = self
            .chat
            .streaming_assistant_id
            .as_ref()
            .and_then(|id| self.chat.messages.iter().find(|message| &message.id == id))
        else {
            return;
        };
        let (visible, thought) = think_content_presence(&message.content);
        // Once visible text has started, late reasoning chunks must not revive Thinking.
        if visible {
            self.chat.response_phase = AiResponsePhase::Responding;
        } else if self.chat.response_phase != AiResponsePhase::ToolArguments
            && (thought || message.reasoning_content.is_some())
        {
            self.chat.response_phase = AiResponsePhase::Thinking;
        }
    }

    pub(in crate::features) fn expanded_execution_groups(&self) -> &HashSet<String> {
        &self.chat.execution_groups_expanded
    }

    pub(in crate::features) fn toggle_execution_group(&mut self, id: String) {
        if !self.chat.execution_groups_expanded.remove(&id) {
            self.chat.execution_groups_expanded.insert(id);
        }
    }

    pub(in crate::features) fn expanded_message_thoughts(&self) -> &HashSet<String> {
        &self.chat.thought_expanded
    }

    pub(in crate::features) fn expanded_command_details(&self) -> &HashSet<String> {
        &self.chat.command_details_expanded
    }

    pub(in crate::features) fn expanded_command_scripts(&self) -> &HashSet<String> {
        &self.chat.command_scripts_expanded
    }

    pub(in crate::features) fn toggle_command_script(&mut self, id: String) {
        if !self.chat.command_scripts_expanded.remove(&id) {
            self.chat.command_scripts_expanded.insert(id);
        }
    }

    pub(in crate::features) fn agent_history_expanded(&self) -> bool {
        self.chat.agent_history_expanded
    }

    pub(in crate::features) fn toggle_agent_history(&mut self) {
        self.chat.agent_history_expanded = !self.chat.agent_history_expanded;
    }

    pub(in crate::features) fn toggle_message_thought(&mut self, id: String) {
        if !self.chat.thought_expanded.remove(&id) {
            self.chat.thought_expanded.insert(id);
        }
    }

    pub(in crate::features) fn toggle_command_details(&mut self, id: String) {
        if !self.chat.command_details_expanded.remove(&id) {
            self.chat.command_details_expanded.insert(id);
        }
    }

    pub(in crate::features) fn upsert_agent_step(
        &mut self,
        step_index: u16,
        status: AiAgentStepStatus,
        kind: AiAgentStepKind,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let title = title.into();
        let detail = detail.into();
        let index = self
            .agent
            .steps
            .iter()
            .position(|step| step.step_index == step_index);
        let index = index.unwrap_or_else(|| {
            self.agent.steps.push(AiAgentStepView {
                step_index,
                status,
                kind,
                title: String::new(),
                detail: String::new(),
                thought: None,
                command: None,
                observation: None,
                source_message_id: None,
                command_card_id: None,
                exit_code: None,
            });
            self.agent.steps.len() - 1
        });
        let step = &mut self.agent.steps[index];
        step.status = status;
        step.kind = kind;
        step.title = title;
        step.detail = detail.clone();
        match kind {
            AiAgentStepKind::Planning => {
                step.thought = (!detail.trim().is_empty()).then_some(detail)
            }
            AiAgentStepKind::Command => {
                // The linked card holds the complete command; update strings may be previews.
                if step.command_card_id.is_none() {
                    step.command = (!detail.trim().is_empty()).then_some(detail);
                }
            }
            AiAgentStepKind::Observation | AiAgentStepKind::Diagnostic => {
                step.observation = (!detail.trim().is_empty()).then_some(detail);
            }
            AiAgentStepKind::FinalAnswer => {
                step.thought = None;
                step.command = None;
                step.observation = None;
                step.command_card_id = None;
                step.exit_code = None;
            }
            AiAgentStepKind::ToolProgress => {}
        }
        let overflow = self.agent.steps.len().saturating_sub(16);
        for step in self.agent.steps.drain(..overflow) {
            self.agent.thought_expanded.remove(&step.step_index);
            self.agent.output_expanded.remove(&step.step_index);
        }
    }

    pub(in crate::features) fn record_agent_observation(
        &mut self,
        step_index: u16,
        observation: &CommandObservation,
        summary: String,
    ) {
        self.upsert_agent_step(
            step_index,
            if observation.exit_code.is_some_and(|code| code != 0) {
                AiAgentStepStatus::Failed
            } else {
                AiAgentStepStatus::Completed
            },
            AiAgentStepKind::Observation,
            "Observed",
            summary,
        );
        if let Some(step) = self
            .agent
            .steps
            .iter_mut()
            .find(|step| step.step_index == step_index)
        {
            step.exit_code = observation.exit_code;
            step.observation = Some(nyaterm_core::truncate_preview(&observation.output, 12_000));
        }
    }
}
