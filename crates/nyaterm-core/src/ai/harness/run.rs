use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::CapabilityScope;

use super::registry::{AgentTool, AgentToolCall, AgentToolRegistry};
use super::{AgentToolBatch, AgentTurnRecord};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentPlanTask {
    pub id: String,
    pub description: String,
    pub status: AgentTaskStatus,
    #[serde(default)]
    pub verification: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentPlan {
    pub tasks: Vec<AgentPlanTask>,
}

impl AgentPlan {
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        if self.tasks.len() > 100 {
            return Err("plan exceeds 100 tasks".into());
        }
        for task in &self.tasks {
            if task.id.trim().is_empty()
                || task.id.len() > 128
                || !ids.insert(&task.id)
                || task.description.trim().is_empty()
            {
                return Err("plan requires unique ids and nonempty descriptions".into());
            }
        }
        if self
            .tasks
            .iter()
            .filter(|task| task.status == AgentTaskStatus::InProgress)
            .count()
            > 1
        {
            return Err("only one plan task may be in progress".into());
        }
        Ok(())
    }
    pub fn is_complete(&self) -> bool {
        self.tasks
            .iter()
            .all(|task| task.status == AgentTaskStatus::Completed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub options: Option<Vec<String>>,
}

impl AgentQuestion {
    pub fn validate_all(questions: &[Self]) -> Result<(), String> {
        let mut ids = HashSet::new();
        if !(1..=3).contains(&questions.len()) {
            return Err("ask one to three questions".into());
        }
        for question in questions {
            if question.id.trim().is_empty()
                || question.id.len() > 128
                || !ids.insert(&question.id)
                || question.question.trim().is_empty()
            {
                return Err("questions require unique ids and nonempty text".into());
            }
            if let Some(options) = &question.options {
                let mut seen = HashSet::new();
                if !(2..=6).contains(&options.len())
                    || options
                        .iter()
                        .any(|option| option.trim().is_empty() || !seen.insert(option))
                {
                    return Err("provide two to six distinct options or omit options".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentVerificationStatus {
    Verified,
    Unverified,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentVerification {
    pub status: AgentVerificationStatus,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRunStatus {
    Planning,
    Running,
    NeedsApproval,
    WaitingForUser,
    Completed,
    Unverified,
    Blocked,
    Failed,
    Cancelled,
}

impl AgentRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Unverified | Self::Blocked | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentToolResult {
    pub call_id: String,
    pub value: Value,
    pub is_error: bool,
}

impl std::fmt::Debug for AgentToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentToolResult")
            .field("call_id", &self.call_id)
            .field("is_error", &self.is_error)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRequestContext {
    pub run_id: String,
    pub turns: Vec<AgentTurnRecord>,
    pub remaining_steps: u16,
}

pub struct AgentRun {
    pub id: String,
    pub objective: String,
    pub scope: CapabilityScope,
    pub status: AgentRunStatus,
    pub plan: AgentPlan,
    pub final_verification: Option<AgentVerification>,
    pub max_steps: u16,
    pub used_steps: u16,
    pending_active: bool,
    pub questions: Vec<AgentQuestion>,
    pub turns: Vec<AgentTurnRecord>,
    seen_calls: HashSet<String>,
}

impl AgentRun {
    pub fn new(objective: String, scope: CapabilityScope, max_steps: u16) -> Self {
        Self {
            id: format!("run-{}", uuid::Uuid::new_v4()),
            objective,
            scope,
            status: AgentRunStatus::Planning,
            plan: AgentPlan::default(),
            final_verification: None,
            max_steps,
            used_steps: 0,
            pending_active: false,
            questions: Vec::new(),
            turns: Vec::new(),
            seen_calls: HashSet::new(),
        }
    }

    pub fn queue_batch(&mut self, batch: AgentToolBatch) -> Result<(), String> {
        if self.status != AgentRunStatus::Planning
            || self.pending_active
            || self.has_queued_call()
            || batch.calls.is_empty()
            || batch.calls.len() > 128
        {
            return Err("run is not ready for a model turn".into());
        }
        let mut ids = HashSet::new();
        for entry in &batch.calls {
            let id = entry.source.id.as_deref().ok_or("missing agent call id")?;
            if id.is_empty() || self.seen_calls.contains(id) || !ids.insert(id.to_owned()) {
                return Err("duplicate or missing agent call id".into());
            }
        }
        self.seen_calls.extend(ids);
        self.turns.push(AgentTurnRecord {
            batch,
            results: Vec::new(),
        });
        Ok(())
    }

    pub fn has_queued_call(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| turn.results.len() < turn.batch.calls.len())
    }

    pub fn pending_call(&self) -> Option<&AgentToolCall> {
        if !self.pending_active {
            return None;
        }
        let turn = self.turns.last()?;
        turn.batch
            .calls
            .get(turn.results.len())?
            .action
            .as_ref()
            .ok()
    }

    /// Resolve invalid/over-quota decisions without dispatching an operation.
    pub fn next_call(&mut self) -> Option<AgentToolCall> {
        if self.status != AgentRunStatus::Planning || self.pending_active {
            return None;
        }
        let turn = self.turns.last_mut()?;
        let entry = turn.batch.calls.get(turn.results.len())?;
        let error = match &entry.action {
            Err(_) => Some("invalid_argument"),
            Ok(call)
                if call.tool != AgentTool::FinalAnswer && self.used_steps >= self.max_steps =>
            {
                Some("step_limit")
            }
            _ => None,
        };
        if let Some(code) = error {
            self.used_steps = self.used_steps.saturating_add(1).min(self.max_steps);
            turn.results.push(AgentToolResult {
                call_id: entry.source.id.clone().expect("validated call id"),
                value: serde_json::json!({"code":code,"message":"Call was not executed. Correct the decision using the available results and remaining steps."}),
                is_error: true,
            });
            self.skip_remaining();
            return None;
        }
        entry.action.as_ref().ok().cloned()
    }

    pub fn begin_call(&mut self, mut call: AgentToolCall) -> Result<(), String> {
        if self.status != AgentRunStatus::Planning || self.pending_active {
            return Err("run is not ready for a tool call".into());
        }
        AgentToolRegistry::validate(&call)?;
        if call.tool != AgentTool::FinalAnswer && self.used_steps >= self.max_steps {
            return Err("Agent reached its step limit; provide a final summary".into());
        }
        if !self.has_queued_call() {
            self.queue_batch(call.clone().into())?;
        }
        let turn = self.turns.last().expect("queued turn");
        let expected = turn.batch.calls[turn.results.len()]
            .action
            .as_ref()
            .map_err(|_| "invalid agent call")?;
        // Provider continuation belongs to the turn, never to each executable call.
        call.assistant_turn = None;
        if expected.id != call.id
            || expected.tool != call.tool
            || expected.arguments != call.arguments
        {
            return Err("call does not match the queued decision".into());
        }
        if call.tool != AgentTool::FinalAnswer {
            self.used_steps = self.used_steps.saturating_add(1);
        }
        self.pending_active = true;
        self.status = AgentRunStatus::Running;
        Ok(())
    }

    pub fn complete_call(&mut self, result: AgentToolResult) -> bool {
        let Some(call) = self.pending_call() else {
            return false;
        };
        if self.status.is_terminal() || call.id != result.call_id {
            return false;
        }
        let stop_batch = result.is_error
            || call.tool == AgentTool::RequestUserInput
            || (call.tool == AgentTool::TerminalExecute
                && (result.value["exitCode"].is_null()
                    || result.value["exitCode"]
                        .as_i64()
                        .is_some_and(|code| code != 0)
                    || result.value["timedOut"] == true
                    || result.value["sourceTruncated"] == true));
        self.turns
            .last_mut()
            .expect("pending turn")
            .results
            .push(result);
        self.pending_active = false;
        self.questions.clear();
        self.status = AgentRunStatus::Planning;
        if stop_batch {
            self.skip_remaining();
        }
        true
    }

    fn skip_remaining(&mut self) {
        if let Some(turn) = self.turns.last_mut() {
            for entry in &turn.batch.calls[turn.results.len()..] {
                turn.results.push(AgentToolResult {
                    call_id: entry.source.id.clone().expect("validated call id"),
                    value: serde_json::json!({"code":"not_executed","message":"Batch stopped. Reconsider this operation using the preceding results."}),
                    is_error: true,
                });
            }
        }
    }

    /// Compatibility entry point for the old single-call coordinator.
    pub fn reject_over_limit(&mut self, call: AgentToolCall) -> bool {
        if self.status != AgentRunStatus::Planning
            || self.pending_active
            || self.has_queued_call()
            || self.used_steps < self.max_steps
            || call.tool == AgentTool::FinalAnswer
            || AgentToolRegistry::validate(&call).is_err()
        {
            return false;
        }
        if self.queue_batch(call.into()).is_err() {
            return false;
        }
        self.next_call();
        true
    }

    pub fn finish_text(&mut self) -> Result<(), String> {
        if self.status != AgentRunStatus::Planning || self.pending_active || self.has_queued_call()
        {
            return Err("run is not ready for a final answer".into());
        }
        self.final_verification = Some(AgentVerification {
            status: AgentVerificationStatus::Unverified,
            summary: "No explicit verification was supplied.".into(),
        });
        self.status = AgentRunStatus::Unverified;
        Ok(())
    }

    pub fn update_plan(&mut self, plan: AgentPlan) -> Result<(), String> {
        plan.validate()?;
        // Removing completed or unfinished tasks would hide remaining work.
        if self
            .plan
            .tasks
            .iter()
            .any(|task| !plan.tasks.iter().any(|next| next.id == task.id))
        {
            return Err("existing plan task ids must be retained".into());
        }
        self.plan = plan;
        Ok(())
    }

    pub fn wait_for_user(&mut self, questions: Vec<AgentQuestion>) -> Result<(), String> {
        AgentQuestion::validate_all(&questions)?;
        if self
            .pending_call()
            .is_none_or(|call| call.tool != AgentTool::RequestUserInput)
        {
            return Err("no pending question call".into());
        }
        self.questions = questions;
        self.status = AgentRunStatus::WaitingForUser;
        Ok(())
    }

    pub fn finish(&mut self, verification: AgentVerification) -> Result<(), String> {
        if self
            .pending_call()
            .is_none_or(|call| call.tool != AgentTool::FinalAnswer)
        {
            return Err("no pending final answer".into());
        }
        if verification.status == AgentVerificationStatus::Verified && !self.plan.is_complete() {
            return Err("unfinished plan cannot be marked verified".into());
        }
        let call_id = self.pending_call().expect("pending final").id.clone();
        self.complete_call(AgentToolResult {
            call_id,
            value: serde_json::json!({"finished":true}),
            is_error: false,
        });
        self.status = match verification.status {
            AgentVerificationStatus::Verified => AgentRunStatus::Completed,
            AgentVerificationStatus::Unverified => AgentRunStatus::Unverified,
            AgentVerificationStatus::Blocked => AgentRunStatus::Blocked,
        };
        self.final_verification = Some(verification);
        Ok(())
    }

    pub fn cancel(&mut self) {
        if let Some(call) = self.pending_call() {
            let result = AgentToolResult {
                call_id: call.id.clone(),
                value: serde_json::json!({"code":"cancelled","message":"Execution was cancelled; its final effect may be unknown."}),
                is_error: true,
            };
            self.turns
                .last_mut()
                .expect("pending turn")
                .results
                .push(result);
        }
        self.status = AgentRunStatus::Cancelled;
        self.pending_active = false;
        self.skip_remaining();
        self.questions.clear();
    }
    pub fn request_context(&self) -> AgentRequestContext {
        AgentRequestContext {
            run_id: self.id.clone(),
            turns: self
                .turns
                .iter()
                .filter(|turn| turn.results.len() == turn.batch.calls.len())
                .cloned()
                .collect(),
            remaining_steps: self.max_steps.saturating_sub(self.used_steps),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentRun, AgentRunStatus, AgentToolResult, AgentVerification, AgentVerificationStatus,
    };
    use crate::ai::AiToolCall;
    use crate::ai::harness::AgentToolRegistry;
    use crate::capabilities::CapabilityScope;
    use serde_json::json;

    fn call(name: &str, args: serde_json::Value, id: &str) -> crate::ai::harness::AgentToolCall {
        AgentToolRegistry::parse(
            &[AiToolCall {
                thought_signature: None,
                id: Some(id.into()),
                name: name.into(),
                arguments: args,
            }],
            "",
        )
        .unwrap()
    }

    #[test]
    fn questions_pause_and_resume_once_in_the_same_run() {
        let mut run = AgentRun::new(
            "diagnose".into(),
            CapabilityScope::explicit(["s".into()], Some("s".into())),
            2,
        );
        let question = call(
            "request_user_input",
            json!({"questions":[{"id":"host","question":"Which host?"}]}),
            "q",
        );
        run.begin_call(question.clone()).unwrap();
        run.wait_for_user(serde_json::from_value(question.arguments["questions"].clone()).unwrap())
            .unwrap();
        assert_eq!(run.status, AgentRunStatus::WaitingForUser);
        let answer = AgentToolResult {
            call_id: "q".into(),
            value: json!({"host":"staging"}),
            is_error: false,
        };
        assert!(run.complete_call(answer.clone()));
        assert!(!run.complete_call(answer));
        assert_eq!(run.used_steps, 1);
        assert_eq!(run.request_context().turns.len(), 1);
        assert!(run.begin_call(question).is_err());
    }

    #[test]
    fn step_limit_allows_a_final_summary_but_not_another_operation() {
        let mut run = AgentRun::new("task".into(), CapabilityScope::AllSessions, 0);
        assert!(
            run.begin_call(call("get_environment", json!({}), "read"))
                .is_err()
        );
        run.begin_call(call(
            "final_answer",
            json!({"answer":"limit reached"}),
            "final",
        ))
        .unwrap();
        run.finish(AgentVerification {
            status: AgentVerificationStatus::Unverified,
            summary: "limit reached".into(),
        })
        .unwrap();
        assert_eq!(run.status, AgentRunStatus::Unverified);
    }

    #[test]
    fn cancellation_discards_late_tool_results() {
        let mut run = AgentRun::new("task".into(), CapabilityScope::AllSessions, 2);
        run.begin_call(call("get_environment", json!({}), "read"))
            .unwrap();
        run.cancel();
        assert!(!run.complete_call(AgentToolResult {
            call_id: "read".into(),
            value: json!({}),
            is_error: false
        }));
        assert_eq!(run.status, AgentRunStatus::Cancelled);
    }

    #[test]
    fn verified_completion_requires_finished_plan_and_plan_edits_retain_task_ids() {
        let mut run = AgentRun::new("task".into(), CapabilityScope::AllSessions, 4);
        let plan: super::AgentPlan = serde_json::from_value(
            json!({"tasks":[{"id":"check","description":"Check service","status":"in_progress"}]}),
        )
        .unwrap();
        run.update_plan(plan).unwrap();
        assert!(run.update_plan(super::AgentPlan::default()).is_err());
        run.begin_call(call(
            "final_answer",
            json!({"answer":"done","verification":{"status":"verified","summary":"verified"}}),
            "final",
        ))
        .unwrap();
        assert!(
            run.finish(AgentVerification {
                status: AgentVerificationStatus::Verified,
                summary: "verified".into()
            })
            .is_err()
        );
        assert_eq!(run.plan.tasks.len(), 1);
        run.finish(AgentVerification {
            status: AgentVerificationStatus::Blocked,
            summary: "service unavailable".into(),
        })
        .unwrap();
        assert_eq!(run.status, AgentRunStatus::Blocked);
    }

    #[test]
    fn plans_and_questions_reject_ambiguous_ids_and_concurrent_current_tasks() {
        let plan: super::AgentPlan = serde_json::from_value(json!({"tasks":[
            {"id":"a","description":"first","status":"in_progress"},
            {"id":"b","description":"second","status":"in_progress"}
        ]}))
        .unwrap();
        assert!(plan.validate().is_err());
        assert!(AgentToolRegistry::parse(&[],r#"{"action":"request_user_input","arguments":{"questions":[{"id":"a","question":"first"},{"id":"a","question":"second"}]}}"#).is_err());
    }

    #[test]
    fn query_read_question_execute_verify_summary_flow_keeps_one_run_and_distinct_records() {
        let mut run = AgentRun::new(
            "inspect and repair service".into(),
            CapabilityScope::explicit(["s".into()], Some("s".into())),
            7,
        );
        let id = run.id.clone();
        let plan: super::AgentPlan = serde_json::from_value(json!({"tasks":[{"id":"repair","description":"Inspect, repair and verify","status":"in_progress"}]})).unwrap();
        run.begin_call(call(
            "update_plan",
            serde_json::to_value(&plan).unwrap(),
            "plan",
        ))
        .unwrap();
        run.update_plan(plan.clone()).unwrap();
        assert!(run.complete_call(AgentToolResult {
            call_id: "plan".into(),
            value: json!({"updated":true}),
            is_error: false
        }));
        for (name, args, call_id, result) in [
            (
                "get_environment",
                json!({}),
                "query",
                json!({"sessions":[{"id":"s"}]}),
            ),
            (
                "sftp_read_text",
                json!({"sessionId":"s","path":"/etc/service.conf"}),
                "read",
                json!({"content":"enabled=false"}),
            ),
        ] {
            run.begin_call(call(name, args, call_id)).unwrap();
            assert!(run.complete_call(AgentToolResult {
                call_id: call_id.into(),
                value: result,
                is_error: false
            }));
        }
        let question = call(
            "request_user_input",
            json!({"questions":[{"id":"repair","question":"Enable the service?","options":["yes","no"]}]}),
            "ask",
        );
        run.begin_call(question.clone()).unwrap();
        run.wait_for_user(serde_json::from_value(question.arguments["questions"].clone()).unwrap())
            .unwrap();
        assert!(run.complete_call(AgentToolResult {
            call_id: "ask".into(),
            value: json!({"answers":{"repair":"yes"}}),
            is_error: false
        }));
        for (call_id, command) in [("execute", "service start"), ("verify", "service status")] {
            run.begin_call(call(
                "terminal_execute",
                json!({"sessionId":"s","command":command}),
                call_id,
            ))
            .unwrap();
            assert!(run.complete_call(AgentToolResult{call_id:call_id.into(),value:json!({"output":"running","exitCode":0,"timedOut":false,"sourceTruncated":false}),is_error:false}));
        }
        let mut completed = plan;
        completed.tasks[0].status = super::AgentTaskStatus::Completed;
        completed.tasks[0].verification = Some("Status check returned running, exit 0".into());
        run.begin_call(call(
            "update_plan",
            serde_json::to_value(&completed).unwrap(),
            "verified-plan",
        ))
        .unwrap();
        run.update_plan(completed).unwrap();
        assert!(run.complete_call(AgentToolResult {
            call_id: "verified-plan".into(),
            value: json!({"updated":true}),
            is_error: false
        }));
        run.begin_call(call("final_answer",json!({"answer":"Service running","verification":{"status":"verified","summary":"status returned running, exit 0"}}),"summary")).unwrap();
        run.finish(AgentVerification {
            status: AgentVerificationStatus::Verified,
            summary: "status returned running, exit 0".into(),
        })
        .unwrap();
        assert_eq!(run.id, id);
        assert_eq!(run.turns.len(), 8);
        assert_eq!(run.used_steps, 7);
        assert_eq!(run.status, AgentRunStatus::Completed);
    }
}
