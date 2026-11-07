use std::collections::HashMap;
use std::sync::Arc;

use nyaterm_core::ai::harness::{
    AgentPlan, AgentQuestion, AgentRequestContext, AgentRun, AgentRunStatus, AgentToolCall,
    AgentToolResult, AgentVerification,
};
use nyaterm_core::ai::{AiChatRequest, AiCommandCard, AiMessage, AiMessageRole};
use nyaterm_core::capabilities::CapabilityScope;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{AiChatLaunch, AiFeatureState};

use crate::features::capability_runtime::terminal_execution::TerminalExecutionPolicy;

pub(super) struct NativeRunState {
    terminal_policy: TerminalExecutionPolicy,
    pub run: AgentRun,
    pub request: AiChatRequest,
    initial_history: Vec<AiMessage>,
    pub cancellation: CancellationToken,
    answers: HashMap<String, String>,
    call_started_at: Option<std::time::Instant>,
}

#[derive(Clone)]
pub(in crate::features) struct NativeRunView {
    pub run_id: String,
    pub call_id: Option<String>,
    pub status: AgentRunStatus,
    pub plan: AgentPlan,
    pub questions: Vec<AgentQuestion>,
    pub answers: HashMap<String, String>,
    pub verification: Option<AgentVerification>,
}

impl AiFeatureState {
    pub(in crate::features) fn begin_native_run(&mut self, request: AiChatRequest) {
        self.agent.task_language = None;
        let target_ids = request
            .targets
            .iter()
            .map(|target| target.terminal_session_id.clone())
            .collect::<Vec<_>>();
        let default = if target_ids.len() == 1 {
            target_ids.first().cloned()
        } else {
            None
        };
        let run = AgentRun::new(
            request.user_input.clone(),
            CapabilityScope::explicit(target_ids, default),
            self.settings_max_agent_steps(),
        );
        let mut initial_history = self
            .chat
            .messages
            .iter()
            .filter(|message| Some(message.session_id.as_str()) == request.session_id.as_deref())
            .filter_map(|message| {
                let mut message = message.as_ref().clone();
                if message.role == AiMessageRole::Assistant {
                    message.content =
                        nyaterm_core::ai::extract_text_from_assistant(&message.content);
                }
                (message.role != AiMessageRole::System && !message.content.trim().is_empty())
                    .then_some(message)
            })
            .collect::<Vec<_>>();
        // begin_chat_request already appended this task and an empty assistant slot.
        if initial_history.last().is_some_and(|message| {
            message.role == AiMessageRole::User && message.content == request.user_input
        }) {
            initial_history.pop();
        }
        let skip = initial_history
            .len()
            .saturating_sub(request.options.history_turns as usize);
        initial_history.drain(..skip);
        self.agent.task_prompt = Some(request.user_input.clone());
        self.agent.native = Some(NativeRunState {
            terminal_policy: TerminalExecutionPolicy {
                background: self.settings.config.agent_background_execution_enabled,
                max_lines: self.settings.config.terminal_output_lines,
            },
            run,
            request,
            initial_history,
            cancellation: CancellationToken::new(),
            answers: HashMap::new(),
            call_started_at: None,
        });
    }

    pub(in crate::features) fn native_terminal_policy(&self) -> Option<TerminalExecutionPolicy> {
        self.agent
            .native
            .as_ref()
            .map(|native| native.terminal_policy)
    }

    pub(in crate::features) fn native_initial_history(&self) -> Option<Vec<AiMessage>> {
        self.agent
            .native
            .as_ref()
            .map(|native| native.initial_history.clone())
    }

    pub(in crate::features) fn native_request_context(&self) -> Option<AgentRequestContext> {
        self.agent
            .native
            .as_ref()
            .map(|native| native.run.request_context())
    }

    pub(in crate::features) fn native_run_view(&self) -> Option<NativeRunView> {
        self.agent.native.as_ref().map(|native| NativeRunView {
            run_id: native.run.id.clone(),
            call_id: native.run.pending_call().map(|call| call.id.clone()),
            status: native.run.status,
            plan: native.run.plan.clone(),
            questions: native.run.questions.clone(),
            answers: native.answers.clone(),
            verification: native.run.final_verification.clone(),
        })
    }

    pub(in crate::features) fn native_run_is_live(
        &self,
        run_id: &str,
        ai_session_id: &str,
    ) -> bool {
        let matches = |chat: &super::AiChatState, agent: &super::AiAgentState| {
            chat.session_id == ai_session_id
                && agent.native.as_ref().is_some_and(|native| {
                    native.run.id == run_id
                        && !native.run.status.is_terminal()
                        && !native.cancellation.is_cancelled()
                })
        };
        matches(&self.chat, &self.agent)
            || self
                .inactive_scopes
                .values()
                .any(|scope| matches(&scope.chat, &scope.agent))
    }

    pub(in crate::features) fn native_owner(&self) -> Option<String> {
        self.agent
            .native
            .as_ref()
            .map(|native| format!("native:{}", native.run.id))
    }

    pub(in crate::features) fn native_owner_in_scope(&self, scope: &str) -> Option<String> {
        let agent = if scope == self.active_scope_key {
            &self.agent
        } else {
            &self.inactive_scopes.get(scope)?.agent
        };
        agent
            .native
            .as_ref()
            .filter(|native| !native.run.status.is_terminal())
            .map(|native| format!("native:{}", native.run.id))
    }

    pub(in crate::features) fn native_run_scopes_for_session(
        &self,
        session_id: &str,
    ) -> Vec<String> {
        let owner_scope = format!("terminal:{session_id}");
        std::iter::once((self.active_scope_key.as_str(), &self.agent))
            .chain(self.inactive_scopes.iter().map(|(scope, state)| (scope.as_str(), &state.agent)))
            .filter_map(|(scope, agent)| {
                let native = agent.native.as_ref()?;
                let targets_session = matches!(&native.run.scope,
                    CapabilityScope::Explicit { session_ids, .. } if session_ids.contains(session_id));
                (!native.run.status.is_terminal() && (scope == owner_scope || targets_session))
                    .then(|| scope.to_string())
            })
            .collect()
    }

    pub(in crate::features) fn ended_native_owner(&self) -> Option<String> {
        self.agent
            .native
            .as_ref()
            .filter(|native| native.run.status.is_terminal())
            .map(|native| format!("native:{}", native.run.id))
    }

    pub(in crate::features) fn queue_native_batch(
        &mut self,
        batch: nyaterm_core::ai::harness::AgentToolBatch,
    ) -> Result<(), String> {
        let native = self.agent.native.as_mut().ok_or("no native run")?;
        if native.cancellation.is_cancelled() {
            return Err("Agent run cancelled".into());
        }
        native.run.queue_batch(batch)
    }

    pub(in crate::features) fn next_native_call(&mut self) -> Option<AgentToolCall> {
        let native = self.agent.native.as_mut()?;
        if native.cancellation.is_cancelled() {
            return None;
        }
        native.run.next_call()
    }

    pub(in crate::features) fn finish_native_text(&mut self, answer: String) -> Result<(), String> {
        let native = self.agent.native.as_mut().ok_or("no native run")?;
        native.run.finish_text()?;
        native.cancellation.cancel();
        self.agent.step_index = native.run.used_steps;
        self.agent.task_prompt = None;
        self.agent.loop_state = None;
        self.upsert_agent_step(
            self.agent.step_index,
            crate::features::runtime_jobs::AiAgentStepStatus::Completed,
            crate::features::ai::presentation::AiAgentStepKind::FinalAnswer,
            "Final Answer",
            answer.clone(),
        );
        self.link_native_step();
        self.set_native_message(answer);
        Ok(())
    }

    pub(in crate::features) fn begin_native_call(
        &mut self,
        call: AgentToolCall,
    ) -> Result<(String, CapabilityScope, CancellationToken), String> {
        let native = self.agent.native.as_mut().ok_or("no native run")?;
        let is_final = call.tool == nyaterm_core::ai::harness::AgentTool::FinalAnswer;
        native.run.begin_call(call)?;
        native.call_started_at = Some(std::time::Instant::now());
        self.agent.step_index = if is_final {
            native.run.used_steps
        } else {
            native.run.used_steps.saturating_sub(1)
        };
        Ok((
            native.run.id.clone(),
            native.run.scope.clone(),
            native.cancellation.child_token(),
        ))
    }

    pub(in crate::features) fn reject_native_over_limit(&mut self, call: AgentToolCall) -> bool {
        self.agent
            .native
            .as_mut()
            .is_some_and(|native| native.run.reject_over_limit(call))
    }

    pub(in crate::features) fn native_question_matches(&self, run_id: &str, call_id: &str) -> bool {
        self.agent.native.as_ref().is_some_and(|native| {
            native.run.id == run_id
                && native.run.status == AgentRunStatus::WaitingForUser
                && native
                    .run
                    .pending_call()
                    .is_some_and(|call| call.id == call_id)
        })
    }

    pub(in crate::features) fn native_call_elapsed_ms(&self) -> Option<u64> {
        self.agent
            .native
            .as_ref()?
            .call_started_at
            .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    pub(in crate::features) fn native_pending_call(&self) -> Option<AgentToolCall> {
        self.agent
            .native
            .as_ref()
            .and_then(|native| native.run.pending_call().cloned())
    }

    pub(in crate::features) fn set_native_run_status(
        &mut self,
        run_id: &str,
        status: AgentRunStatus,
    ) {
        for agent in std::iter::once(&mut self.agent).chain(
            self.inactive_scopes
                .values_mut()
                .map(|scope| &mut scope.agent),
        ) {
            if let Some(native) = agent
                .native
                .as_mut()
                .filter(|native| native.run.id == run_id && !native.run.status.is_terminal())
            {
                native.run.status = status;
                return;
            }
        }
    }

    pub(in crate::features) fn complete_native_call(
        &mut self,
        run_id: &str,
        result: AgentToolResult,
    ) -> bool {
        let Some(native) = self
            .agent
            .native
            .as_mut()
            .filter(|native| native.run.id == run_id)
        else {
            return false;
        };
        let accepted = native.run.complete_call(result);
        if accepted {
            native.answers.clear();
        }
        accepted
    }

    pub(in crate::features) fn update_native_plan(
        &mut self,
        plan: AgentPlan,
    ) -> Result<(), String> {
        self.agent
            .native
            .as_mut()
            .ok_or("no native run")?
            .run
            .update_plan(plan)
    }

    pub(in crate::features) fn wait_native_user(
        &mut self,
        questions: Vec<AgentQuestion>,
    ) -> Result<(), String> {
        let native = self.agent.native.as_mut().ok_or("no native run")?;
        native.run.wait_for_user(questions)?;
        native.answers.clear();
        Ok(())
    }

    pub(in crate::features) fn set_native_answer(
        &mut self,
        run_id: &str,
        call_id: &str,
        question_id: &str,
        answer: String,
    ) -> bool {
        let Some(native) = self.agent.native.as_mut().filter(|native| {
            native.run.id == run_id
                && native.run.status == AgentRunStatus::WaitingForUser
                && native
                    .run
                    .pending_call()
                    .is_some_and(|call| call.id == call_id)
        }) else {
            return false;
        };
        if !native
            .run
            .questions
            .iter()
            .any(|question| question.id == question_id)
        {
            return false;
        }
        native.answers.insert(question_id.to_string(), answer);
        true
    }

    pub(in crate::features) fn native_answers(&self, run_id: &str, call_id: &str) -> Option<Value> {
        let native = self.agent.native.as_ref().filter(|native| {
            native.run.id == run_id
                && native.run.status == AgentRunStatus::WaitingForUser
                && native
                    .run
                    .pending_call()
                    .is_some_and(|call| call.id == call_id)
        })?;
        if native.run.questions.iter().any(|question| {
            native
                .answers
                .get(&question.id)
                .is_none_or(|answer| answer.trim().is_empty())
        }) {
            return None;
        }
        Some(json!({"answers":native.answers}))
    }

    pub(in crate::features) fn finish_native_run(
        &mut self,
        verification: AgentVerification,
        answer: String,
    ) -> Result<(), String> {
        let native = self.agent.native.as_mut().ok_or("no native run")?;
        native.run.finish(verification)?;
        native.cancellation.cancel();
        self.agent.task_prompt = None;
        self.agent.loop_state = None;
        self.set_native_message(answer);
        Ok(())
    }

    pub(in crate::features) fn set_native_message(&mut self, text: String) {
        if let Some(message) = self
            .chat
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.role == AiMessageRole::Assistant)
        {
            Arc::make_mut(message).content = text.clone();
        }
        self.chat.response_preview = text.clone();
        self.panel.status = text;
    }

    pub(in crate::features) fn begin_native_continuation(
        &mut self,
    ) -> Option<(AiChatLaunch, AiChatRequest)> {
        let native = self.agent.native.as_ref()?;
        if native.run.status != AgentRunStatus::Planning
            || native.cancellation.is_cancelled()
            || native.run.has_queued_call()
            || self.chat.pending
        {
            return None;
        }
        let mut request = native.request.clone();
        request.options.agent_context = Some(native.run.request_context());
        request.options.agent_json_protocol = self.agent.json_protocol;
        self.agent.step_index = native.run.used_steps;
        self.upsert_agent_step(
            self.agent.step_index,
            crate::features::runtime_jobs::AiAgentStepStatus::Planning,
            crate::features::ai::presentation::AiAgentStepKind::Planning,
            "Planning",
            "Choosing the next tool",
        );
        let launch = self.begin_chat_job();
        self.chat.pending = true;
        let id = format!("assistant-{}", uuid::Uuid::new_v4());
        self.chat.messages.push(Arc::new(AiMessage {
            id: id.clone(),
            session_id: self.chat.session_id.clone(),
            role: AiMessageRole::Assistant,
            content: String::new(),
            created_at: nyaterm_core::now_rfc3339(),
            reasoning_content: None,
            command_cards: Vec::new(),
        }));
        self.chat.streaming_assistant_id = Some(id);
        self.chat.command_cards.clear();
        self.panel.status = "Agent continuing".into();
        Some((launch, request))
    }

    pub(in crate::features) fn record_native_tool_output(&mut self, value: &Value) {
        let index = self.last_agent_step_index();
        if let Some(step) = self
            .agent
            .steps
            .iter_mut()
            .find(|step| step.step_index == index)
        {
            if step.command.is_some() {
                step.observation = value
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|output| nyaterm_core::truncate_preview(output, 12_000));
                step.exit_code = value
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .and_then(|code| i32::try_from(code).ok());
            } else {
                step.observation = serde_json::to_string_pretty(value).ok();
            }
        }
    }

    pub(in crate::features) fn link_native_step(&mut self) {
        let source = self.chat.messages.last().map(|message| message.id.clone());
        if let Some(step) = self
            .agent
            .steps
            .iter_mut()
            .find(|step| step.step_index == self.agent.step_index)
        {
            step.source_message_id = source;
        }
    }

    pub(in crate::features) fn present_native_command(&mut self, card: AiCommandCard) {
        if let Some(message) = self
            .chat
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.role == AiMessageRole::Assistant)
        {
            Arc::make_mut(message).command_cards = vec![card.clone()];
        }
        if let Some(step) = self
            .agent
            .steps
            .iter_mut()
            .find(|step| step.step_index == self.agent.step_index)
        {
            step.command_card_id = Some(card.id.clone());
            step.command = Some(card.command.clone());
            step.thought = Some(card.explanation.clone());
        }
        self.chat.command_cards = vec![card];
    }
}
