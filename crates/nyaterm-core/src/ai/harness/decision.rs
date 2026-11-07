//! A model turn is the unit of continuation; individual calls are execution units.

use std::collections::HashSet;

use crate::ai::completion::AiAssistantTurn;
use crate::ai::{AiChatCompletion, AiToolCall};

use super::{AgentTool, AgentToolCall, AgentToolRegistry, AgentToolResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentResponseError {
    #[error("Invalid Agent call structure")]
    InvalidStructure,
    #[error("Invalid Agent tool arguments")]
    InvalidArguments,
    #[error("Agent returned an empty answer")]
    Empty,
    #[error("Agent returned reasoning without an answer")]
    ReasoningOnly,
    #[error("Agent response was refused")]
    Refused,
    #[error("Agent response was truncated")]
    Truncated,
    #[error("Agent returned a malformed JSON action")]
    InvalidJsonAction,
}

#[derive(Clone, PartialEq, Eq)]
pub enum AgentDecision {
    Tools(AgentToolBatch),
    FinalText(String),
}

impl std::fmt::Debug for AgentDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tools(batch) => f.debug_tuple("Tools").field(batch).finish(),
            Self::FinalText(_) => f.write_str("FinalText([REDACTED])"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolBatch {
    pub assistant_turn: AiAssistantTurn,
    pub calls: Vec<AgentBatchCall>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentBatchCall {
    pub source: AiToolCall,
    pub action: Result<AgentToolCall, AgentResponseError>,
}

impl std::fmt::Debug for AgentBatchCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBatchCall")
            .field("valid", &self.action.is_ok())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnRecord {
    pub batch: AgentToolBatch,
    /// Completed prefix, in provider order. Never continue an unfinished turn.
    pub results: Vec<AgentToolResult>,
}

impl From<AgentToolCall> for AgentToolBatch {
    fn from(mut call: AgentToolCall) -> Self {
        let assistant_turn = call.assistant_turn.take().unwrap_or_default();
        let source = AiToolCall {
            id: Some(call.id.clone()),
            name: call.tool.name().into(),
            arguments: call.provider_arguments.clone(),
            thought_signature: call.thought_signature.clone(),
        };
        Self {
            assistant_turn,
            calls: vec![AgentBatchCall {
                source,
                action: Ok(call),
            }],
        }
    }
}

impl From<(AgentToolCall, AgentToolResult)> for AgentTurnRecord {
    fn from((call, result): (AgentToolCall, AgentToolResult)) -> Self {
        Self {
            batch: call.into(),
            results: vec![result],
        }
    }
}

impl AgentToolRegistry {
    pub fn decision(completion: &AiChatCompletion) -> Result<AgentDecision, AgentResponseError> {
        let stop = completion
            .metadata
            .finish_reason
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            stop.as_str(),
            "length" | "max_tokens" | "max_output_tokens" | "incomplete"
        ) || (completion.metadata.provider_data["status"] == "incomplete"
            && stop != "content_filter")
        {
            return Err(AgentResponseError::Truncated);
        }
        if matches!(
            stop.as_str(),
            "refusal"
                | "refused"
                | "safety"
                | "blocklist"
                | "prohibited_content"
                | "content_filter"
                | "recitation"
                | "spii"
        ) || completion
            .metadata
            .provider_data
            .pointer("/choices/0/delta/refusal")
            .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            || completion
                .metadata
                .provider_data
                .pointer("/choices/0/message/refusal")
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            || completion
                .metadata
                .assistant_turn
                .provider_items
                .iter()
                .any(|item| {
                    item["type"] == "refusal"
                        || item["content"]
                            .as_array()
                            .is_some_and(|parts| parts.iter().any(|part| part["type"] == "refusal"))
                })
            || completion
                .metadata
                .provider_data
                .pointer("/promptFeedback/blockReason")
                .is_some_and(|value| value.as_str().is_some_and(|reason| !reason.is_empty()))
        {
            return Err(AgentResponseError::Refused);
        }
        if matches!(
            stop.as_str(),
            "in_progress" | "queued" | "cancelled" | "failed"
        ) {
            return Err(AgentResponseError::InvalidStructure);
        }
        if completion.tool_calls.is_empty() {
            let text = completion.text.trim();
            if text.is_empty() {
                return Err(
                    if completion
                        .reasoning_content
                        .as_ref()
                        .is_some_and(|reasoning| !reasoning.trim().is_empty())
                    {
                        AgentResponseError::ReasoningOnly
                    } else {
                        AgentResponseError::Empty
                    },
                );
            }
            if text.starts_with('{')
                || text
                    .strip_prefix('[')
                    .is_some_and(|rest| rest.trim_start().starts_with('{'))
                || text.starts_with("```json")
            {
                // A prose answer may quote an example action. Only an action-shaped
                // reply can enter the fallback execution path.
                let call =
                    Self::parse(&[], text).map_err(|_| AgentResponseError::InvalidJsonAction)?;
                return Ok(AgentDecision::Tools(call.into()));
            }
            if matches!(stop.as_str(), "tool_calls" | "tool_use" | "function_call") {
                return Err(AgentResponseError::InvalidStructure);
            }
            return Ok(AgentDecision::FinalText(text.to_string()));
        }
        if completion.tool_calls.len() > 128 {
            return Err(AgentResponseError::InvalidStructure);
        }
        let mut ids = HashSet::new();
        let mut calls = Vec::new();
        for (index, source) in completion.tool_calls.iter().enumerate() {
            if source.id.as_deref().is_none_or(str::is_empty)
                || !ids.insert(source.id.as_deref())
                || source.name.is_empty()
            {
                return Err(AgentResponseError::InvalidStructure);
            }
            let action = Self::parse(std::slice::from_ref(source), "")
                .map_err(|_| AgentResponseError::InvalidArguments)
                .and_then(|mut call| {
                    if call.tool == AgentTool::FinalAnswer
                        && index + 1 != completion.tool_calls.len()
                    {
                        Err(AgentResponseError::InvalidStructure)
                    } else {
                        if call.thought.trim().is_empty() {
                            call.thought = completion.text.trim().to_string();
                        }
                        Ok(call)
                    }
                });
            calls.push(AgentBatchCall {
                source: source.clone(),
                action,
            });
        }
        Ok(AgentDecision::Tools(AgentToolBatch {
            assistant_turn: completion.metadata.assistant_turn.clone(),
            calls,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::ai::completion::{AiAssistantTurn, AiContentPart, AiResponseMetadata};
    use crate::ai::harness::{
        AgentDecision, AgentResponseError, AgentRun, AgentRunStatus, AgentToolRegistry,
        AgentToolResult,
    };
    use crate::ai::{AiChatCompletion, AiToolCall};
    use crate::capabilities::CapabilityScope;
    use serde_json::{Value, json};

    fn call(id: &str, name: &str, arguments: Value) -> AiToolCall {
        AiToolCall {
            id: Some(id.into()),
            name: name.into(),
            arguments,
            thought_signature: None,
        }
    }

    fn reply(text: &str, calls: Vec<AiToolCall>) -> AiChatCompletion {
        AiChatCompletion {
            metadata: AiResponseMetadata {
                finish_reason: Some("stop".into()),
                assistant_turn: AiAssistantTurn {
                    parts: calls
                        .iter()
                        .map(|call| AiContentPart::ToolCall {
                            id: call.id.clone().unwrap(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            signatures: Vec::new(),
                        })
                        .collect(),
                    ..Default::default()
                },
                ..Default::default()
            },
            text: text.into(),
            reasoning_content: None,
            tool_calls: calls,
        }
    }

    fn queued(calls: Vec<AiToolCall>, steps: u16) -> AgentRun {
        let AgentDecision::Tools(batch) =
            AgentToolRegistry::decision(&reply("Inspecting", calls)).unwrap()
        else {
            panic!("expected batch");
        };
        let mut run = AgentRun::new("inspect".into(), CapabilityScope::AllSessions, steps);
        run.queue_batch(batch).unwrap();
        run
    }

    fn complete(run: &mut AgentRun, value: Value, is_error: bool) {
        let call = run.next_call().unwrap();
        let id = call.id.clone();
        run.begin_call(call).unwrap();
        assert!(run.complete_call(AgentToolResult {
            call_id: id,
            value,
            is_error
        }));
    }

    #[test]
    fn plain_answers_are_unverified_and_protocol_failures_remain_distinct() {
        assert_eq!(
            AgentToolRegistry::decision(&reply("Done", vec![])).unwrap(),
            AgentDecision::FinalText("Done".into())
        );
        let mut run = AgentRun::new("task".into(), CapabilityScope::AllSessions, 2);
        run.finish_text().unwrap();
        assert_eq!(run.status, AgentRunStatus::Unverified);
        for (text, error) in [
            ("", AgentResponseError::Empty),
            ("{\"action\":", AgentResponseError::InvalidJsonAction),
            (
                "{\"action\":\"unknown\"}",
                AgentResponseError::InvalidJsonAction,
            ),
        ] {
            assert_eq!(
                AgentToolRegistry::decision(&reply(text, vec![])).unwrap_err(),
                error
            );
        }
        let mut response = reply("", vec![]);
        response.reasoning_content = Some("private reasoning".into());
        assert_eq!(
            AgentToolRegistry::decision(&response).unwrap_err(),
            AgentResponseError::ReasoningOnly
        );
        for (stop, error) in [
            ("length", AgentResponseError::Truncated),
            ("MAX_TOKENS", AgentResponseError::Truncated),
            ("refusal", AgentResponseError::Refused),
        ] {
            response.metadata.finish_reason = Some(stop.into());
            assert_eq!(AgentToolRegistry::decision(&response).unwrap_err(), error);
        }
    }

    #[test]
    fn multiple_calls_share_one_turn_and_advance_only_after_the_current_result() {
        let mut run = queued(
            vec![
                call("a", "get_environment", json!({})),
                call("b", "session_get", json!({"sessionId":"s"})),
            ],
            3,
        );
        let first = run.next_call().unwrap();
        run.begin_call(first).unwrap();
        assert!(run.next_call().is_none());
        assert!(run.request_context().turns.is_empty());
        assert!(!run.complete_call(AgentToolResult {
            call_id: "b".into(),
            value: json!({}),
            is_error: false
        }));
        assert!(run.complete_call(AgentToolResult {
            call_id: "a".into(),
            value: json!({}),
            is_error: false
        }));
        complete(&mut run, json!({}), false);
        assert_eq!(run.used_steps, 2);
        let context = run.request_context();
        assert_eq!(context.turns.len(), 1);
        assert_eq!(context.turns[0].batch.assistant_turn.parts.len(), 2);
        assert_eq!(
            context.turns[0]
                .results
                .iter()
                .map(|result| result.call_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn denial_and_invalid_arguments_stop_the_batch_without_dispatching_later_calls() {
        for invalid in [false, true] {
            let mut run = queued(
                vec![
                    call(
                        "a",
                        "get_environment",
                        if invalid {
                            json!({"unexpected":"private"})
                        } else {
                            json!({})
                        },
                    ),
                    call("b", "get_environment", json!({})),
                ],
                3,
            );
            if invalid {
                assert!(run.next_call().is_none());
            } else {
                complete(&mut run, json!({"code":"denied"}), true);
            }
            assert!(run.next_call().is_none());
            let results = &run.turns[0].results;
            assert!(results[0].is_error);
            assert_eq!(results[1].value["code"], "not_executed");
            assert_eq!(run.status, AgentRunStatus::Planning);
        }
    }

    #[test]
    fn uncertain_and_failed_terminal_execution_skip_later_writes() {
        for value in [
            json!({"exitCode":null}),
            json!({"exitCode":1}),
            json!({"exitCode":0,"timedOut":true}),
            json!({"exitCode":0,"sourceTruncated":true}),
        ] {
            let mut run = queued(
                vec![
                    call(
                        "a",
                        "terminal_execute",
                        json!({"command":"check","sessionId":"s"}),
                    ),
                    call(
                        "b",
                        "terminal_execute",
                        json!({"command":"write","sessionId":"s"}),
                    ),
                ],
                3,
            );
            complete(&mut run, value, false);
            assert!(run.next_call().is_none());
            assert_eq!(run.turns[0].results[1].value["code"], "not_executed");
        }
    }

    #[test]
    fn question_is_a_barrier_and_cancellation_rejects_late_results() {
        let mut run = queued(
            vec![
                call(
                    "q",
                    "request_user_input",
                    json!({"questions":[{"id":"target","question":"Which target?"}]}),
                ),
                call("b", "get_environment", json!({})),
            ],
            3,
        );
        let question = run.next_call().unwrap();
        let questions = serde_json::from_value(question.arguments["questions"].clone()).unwrap();
        run.begin_call(question).unwrap();
        run.wait_for_user(questions).unwrap();
        assert!(run.next_call().is_none());
        assert!(run.complete_call(AgentToolResult {
            call_id: "q".into(),
            value: json!({"answers":{"target":"s"}}),
            is_error: false
        }));
        assert_eq!(run.turns[0].results[1].value["code"], "not_executed");
        let mut run = queued(
            vec![
                call("a", "get_environment", json!({})),
                call("b", "get_environment", json!({})),
            ],
            3,
        );
        let next = run.next_call().unwrap();
        run.begin_call(next).unwrap();
        run.cancel();
        assert!(run.next_call().is_none());
        assert!(!run.complete_call(AgentToolResult {
            call_id: "a".into(),
            value: json!({}),
            is_error: false
        }));
        assert_eq!(run.turns[0].results[0].value["code"], "cancelled");
    }

    #[test]
    fn quotas_and_invalid_final_position_cannot_execute_operations_or_hide_results() {
        let mut run = queued(
            vec![
                call("a", "get_environment", json!({})),
                call("b", "get_environment", json!({})),
            ],
            1,
        );
        complete(&mut run, json!({}), false);
        assert!(run.next_call().is_none());
        assert_eq!(run.turns[0].results[1].value["code"], "step_limit");
        assert_eq!(run.used_steps, 1);
        let mut run = queued(
            vec![
                call("final", "final_answer", json!({"answer":"done"})),
                call("b", "get_environment", json!({})),
            ],
            3,
        );
        assert!(run.next_call().is_none());
        assert_eq!(run.turns[0].results.len(), 2);
        assert_eq!(run.status, AgentRunStatus::Planning);
    }

    #[test]
    fn duplicate_ids_and_oversized_batches_are_rejected_before_execution() {
        let duplicate = call("a", "get_environment", json!({}));
        assert_eq!(
            AgentToolRegistry::decision(&reply("", vec![duplicate.clone(), duplicate]))
                .unwrap_err(),
            AgentResponseError::InvalidStructure
        );
        assert_eq!(
            AgentToolRegistry::decision(&reply(
                "",
                (0..129)
                    .map(|i| call(&i.to_string(), "get_environment", json!({})))
                    .collect()
            ))
            .unwrap_err(),
            AgentResponseError::InvalidStructure
        );
    }

    #[test]
    fn decision_debug_never_exposes_answers_or_provider_arguments() {
        let decision = AgentToolRegistry::decision(&reply("private-answer", vec![])).unwrap();
        assert!(!format!("{decision:?}").contains("private"));
        let decision = AgentToolRegistry::decision(&reply(
            "",
            vec![call(
                "a",
                "terminal_execute",
                json!({"command":"private-command","sessionId":"s"}),
            )],
        ))
        .unwrap();
        assert!(!format!("{decision:?}").contains("private"));
    }

    #[test]
    fn provider_refusals_and_unfinished_responses_cannot_become_plain_answers() {
        let mut response = reply("partial text", vec![]);
        response.metadata.finish_reason = Some("in_progress".into());
        assert_eq!(
            AgentToolRegistry::decision(&response).unwrap_err(),
            AgentResponseError::InvalidStructure
        );
        response.metadata.finish_reason = Some("completed".into());
        response.metadata.assistant_turn.provider_items = vec![
            json!({"type":"message","content":[{"type":"refusal","refusal":"private-refusal"}]}),
        ];
        assert_eq!(
            AgentToolRegistry::decision(&response).unwrap_err(),
            AgentResponseError::Refused
        );
        response.metadata.assistant_turn.provider_items.clear();
        response.metadata.provider_data = json!({"promptFeedback":{"blockReason":"SAFETY"}});
        assert_eq!(
            AgentToolRegistry::decision(&response).unwrap_err(),
            AgentResponseError::Refused
        );
    }

    #[test]
    fn prose_quoting_an_action_does_not_execute_the_example() {
        for text in [
            r#"Example only: {"action":"get_environment","arguments":{}}"#,
            "[Done] No changes needed.",
        ] {
            assert_eq!(
                AgentToolRegistry::decision(&reply(text, vec![])).unwrap(),
                AgentDecision::FinalText(text.into())
            );
        }
    }
}
