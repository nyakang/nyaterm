use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use genai::chat::{
    ChatMessage, ChatRequest, ChatStreamEvent, Tool, ToolCall, ToolChoice, ToolResponse,
};
use russh::ChannelMsg;
use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio::sync::oneshot;

#[cfg(test)]
use crate::config::AiExecutionProfile;
use crate::config::{
    AgentCommandExecutionMode, AiAgentKind, AiPermissionMode, AiSettings, RiskLevel,
};
use crate::core::capabilities::{
    TerminalExecuteRequest, TerminalExecutionPresentation, execute_terminal_command,
};
use crate::core::session::{SessionManager, SessionType};
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use crate::utils::process::hide_window;

use super::history::{append_ai_audit, append_message, save_user_message};
use super::model::{ResolvedAiModel, build_chat_options, build_client, resolve_request_model};
use super::parser::{extract_json_object, parse_model_output, trim_string_to_option};
use super::prompt::{
    agent_execution_disabled_message, agent_max_steps_message, agent_send_only_observation,
    agent_system_prompt, build_agent_failed_message, build_agent_prompt,
    build_agent_rejected_message, build_agent_unknown_action_message, build_observation_message,
};
use super::redaction::{redact_context, redact_sensitive_text};
use super::stream::{active_streams, emit_stream_event, is_cancelled};
use super::types::{
    AgentActionKind, AgentLlmResponse, AgentStepAction, AgentStepPayload, AgentStepStatus,
    AiChatRequest, AiMessage, AiMessageRole, AiStreamEventPayload, AiTerminalTarget,
    AppendAiAuditRequest, CommandObservation, now_rfc3339, uuid,
};

// ---------------------------------------------------------------------------
// Agent approval
// ---------------------------------------------------------------------------

/// Manages pending agent step approvals awaiting user confirmation from the frontend.
pub struct AgentApprovalManager {
    pending: tokio::sync::Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl AgentApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, key: String) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(key, tx);
        rx
    }

    pub async fn respond(&self, key: &str, approved: bool) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(key) {
            tx.send(approved).is_ok()
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub async fn cancel_all_for_stream(&self, stream_id: &str) {
        let mut pending = self.pending.lock().await;
        let keys_to_remove: Vec<String> = pending
            .keys()
            .filter(|k| k.starts_with(stream_id))
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(tx) = pending.remove(&key) {
                let _ = tx.send(false);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Agent (ReAct) loop
// ---------------------------------------------------------------------------

const DEFAULT_MAX_AGENT_STEPS: u16 = 10;
const DEFAULT_AGENT_STEP_TIMEOUT_MS: u64 = 30_000;
const TOOL_EXECUTE_COMMAND: &str = "execute_command";
const TOOL_FINAL_ANSWER: &str = "final_answer";

fn emit_agent_step(app: &AppHandle, stream_id: &str, payload: AgentStepPayload) {
    let _ = app.emit(format!("ai-stream-{stream_id}").as_str(), payload);
}

fn emit_agent_error(app: &AppHandle, stream_id: &str, session_id: &str, error: &str) {
    tracing::warn!(
        stream_id = %stream_id,
        session_id = %session_id,
        error = %error,
        "AI agent stream failed"
    );
    active_streams().lock().unwrap().remove(stream_id);
    emit_stream_event(
        app,
        stream_id,
        AiStreamEventPayload {
            event_type: "error".to_string(),
            stream_id: stream_id.to_string(),
            session_id: Some(session_id.to_string()),
            text_delta: None,
            reasoning_delta: None,
            message: None,
            command_cards: vec![],
            usage: None,
            error: Some(error.to_string()),
        },
    );
}

/// Execute a command via marker-based PTY capture.
///
/// Injects the command directly into the interactive PTY session (wrapped with
/// unique boundary markers), then captures the output from the PTY stream.
/// This works regardless of the user's current context: nested SSH, containers,
/// `sudo su`, etc.
///
/// Emits structured `AiCaptureEvent` payloads to the frontend so the terminal
/// can render a styled inline block showing the command and its output.
async fn execute_command_on_session(
    app: &AppHandle,
    session_manager: Arc<SessionManager>,
    terminal_session_id: &str,
    command: &str,
    language: &str,
    timeout_ms: u64,
    step_index: u16,
    terminal_output_lines: u16,
) -> AppResult<CommandObservation> {
    let result = execute_terminal_command(
        session_manager,
        TerminalExecuteRequest {
            session_id: terminal_session_id.to_string(),
            command: command.to_string(),
            timeout_ms,
        },
        Some(TerminalExecutionPresentation {
            app: app.clone(),
            step_index,
            max_lines: terminal_output_lines,
            send_only_output: Some(agent_send_only_observation(language).to_string()),
            disabled_error: Some(agent_execution_disabled_message(language).to_string()),
        }),
        tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    Ok(CommandObservation {
        output: result.output,
        exit_code: result.exit_code,
        duration_ms: result.duration_ms,
    })
}

enum BackgroundExecutionTarget {
    Ssh(Arc<SshConnectionHandles>),
    Local { cwd: Option<String> },
    Unsupported(SessionType),
}

async fn resolve_background_execution_target(
    session_manager: &SessionManager,
    terminal_session_id: &str,
) -> AppResult<BackgroundExecutionTarget> {
    let sessions = session_manager.sessions.lock().await;
    let session = sessions.get(terminal_session_id).ok_or_else(|| {
        AppError::SessionNotFound(format!("Session '{}' not found", terminal_session_id))
    })?;

    match session.info.session_type {
        SessionType::SSH => {
            let ssh_handle = session
                .ssh_handle
                .as_ref()
                .ok_or_else(|| {
                    AppError::Config("SSH session is missing its command handle".to_string())
                })?
                .clone()
                .downcast::<SshConnectionHandles>()
                .map_err(|_| AppError::Config("Failed to access SSH command handle".to_string()))?;
            Ok(BackgroundExecutionTarget::Ssh(ssh_handle))
        }
        SessionType::Local => {
            let cwd_arc = session.cwd.clone();
            drop(sessions);
            let cwd = cwd_arc.lock().await.safe_local_execution_cwd();
            Ok(BackgroundExecutionTarget::Local { cwd })
        }
        ref session_type => Ok(BackgroundExecutionTarget::Unsupported(session_type.clone())),
    }
}

async fn execute_command_in_background(
    session_manager: &SessionManager,
    terminal_session_id: &str,
    command: &str,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    match resolve_background_execution_target(session_manager, terminal_session_id).await? {
        BackgroundExecutionTarget::Ssh(ssh_handle) => {
            execute_ssh_background_command(ssh_handle, command, timeout_ms).await
        }
        BackgroundExecutionTarget::Local { cwd } => {
            execute_local_background_command(command, cwd.as_deref(), timeout_ms).await
        }
        BackgroundExecutionTarget::Unsupported(session_type) => Err(AppError::Config(format!(
            "Background AI command execution is not supported for {:?} sessions",
            session_type
        ))),
    }
}

async fn execute_ssh_background_command(
    ssh_handle: Arc<SshConnectionHandles>,
    command: &str,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    let started = Instant::now();
    let handle_mtx = ssh_handle.target_handle();
    let timeout_dur = Duration::from_millis(timeout_ms);

    let result = tokio::time::timeout(timeout_dur, async {
        let mut channel = {
            let handle = handle_mtx.lock().await;
            handle.channel_open_session().await.map_err(|error| {
                AppError::Channel(format!("Failed to open SSH exec channel: {error}"))
            })?
        };

        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|error| {
                AppError::Channel(format!("Failed to execute SSH command: {error}"))
            })?;

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code: Option<i32> = None;

        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                ChannelMsg::ExtendedData { ref data, .. } => {
                    stderr.push_str(&String::from_utf8_lossy(data));
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = Some(exit_status as i32);
                }
                ChannelMsg::Eof => break,
                _ => {}
            }
        }

        Ok(CommandObservation {
            output: merge_command_output(&stdout, &stderr),
            exit_code,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    })
    .await;

    match result {
        Ok(result) => result,
        Err(_) => Ok(CommandObservation {
            output: "Background command timed out.".to_string(),
            exit_code: None,
            duration_ms: timeout_ms,
        }),
    }
}

async fn execute_local_background_command(
    command: &str,
    cwd: Option<&str>,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    let started = Instant::now();
    let timeout_dur = Duration::from_millis(timeout_ms);
    let mut child = local_shell_command(command);
    if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
        child.current_dir(cwd);
    }
    child.kill_on_drop(true);
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());

    match tokio::time::timeout(timeout_dur, child.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(CommandObservation {
                output: merge_command_output(&stdout, &stderr),
                exit_code: output.status.code(),
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
        Ok(Err(error)) => Err(AppError::Channel(format!(
            "Failed to run local background command: {error}"
        ))),
        Err(_) => Ok(CommandObservation {
            output: "Background command timed out.".to_string(),
            exit_code: None,
            duration_ms: timeout_ms,
        }),
    }
}

#[cfg(windows)]
fn local_shell_command(command: &str) -> Command {
    let mut child = Command::new("cmd");
    child.args(["/C", command]);
    hide_window(&mut child);
    child
}

#[cfg(not(windows))]
fn local_shell_command(command: &str) -> Command {
    let mut child = Command::new("sh");
    child.args(["-lc", command]);
    child
}

fn merge_command_output(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn truncate_for_log(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn safe_command_preview(command: &str) -> String {
    redact_sensitive_text(&truncate_for_log(command, 200))
}

#[derive(Debug, Clone)]
struct RiskAssessment {
    model_risk: RiskLevel,
    local_risk: RiskLevel,
    local_auto_executable: bool,
    effective_risk: RiskLevel,
    risk_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteCommandToolArgs {
    thought: String,
    command: String,
    #[serde(default)]
    target_terminal_session_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_risk_level")]
    risk_level: RiskLevel,
    risk_reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinalAnswerToolArgs {
    thought: String,
    answer: String,
}

struct LegacyAgentStep {
    parsed: AgentLlmResponse,
    raw_output: String,
}

#[derive(Debug, Clone)]
enum AgentToolInvocation {
    ExecuteCommand {
        tool_call: ToolCall,
        args: ExecuteCommandToolArgs,
    },
    FinalAnswer {
        args: FinalAnswerToolArgs,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeToolCallMode {
    Enabled,
    DisabledForTurn,
}

impl NativeToolCallMode {
    fn should_attempt(self) -> bool {
        matches!(self, Self::Enabled)
    }

    fn disable_for_turn(&mut self) {
        *self = Self::DisabledForTurn;
    }
}

fn should_fallback_to_legacy_after_tool_error(
    mode: &mut NativeToolCallMode,
    error: &AppError,
) -> bool {
    if matches!(error, AppError::Cancelled(_)) {
        return false;
    }
    mode.disable_for_turn();
    true
}

fn deserialize_required_risk_level<'de, D>(deserializer: D) -> Result<RiskLevel, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    super::types::parse_risk_level_label(&value)
        .ok_or_else(|| serde::de::Error::custom(format!("invalid riskLevel '{value}'")))
}

fn agent_tools() -> Vec<Tool> {
    vec![
        Tool::new(TOOL_EXECUTE_COMMAND)
            .with_description(
                "Execute exactly one shell command in the active terminal session. Use this when \
                 more observation is needed or when the user requested an action that requires a \
                 command.",
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "thought": {
                        "type": "string",
                        "description": "Brief reasoning for this step and why this command is needed."
                    },
                    "command": {
                        "type": "string",
                        "description": "A single shell command to execute."
                    },
                    "targetTerminalSessionId": {
                        "type": "string",
                        "description": "Terminal session id to execute the command in. Required when multiple terminal targets are available."
                    },
                    "riskLevel": {
                        "type": "string",
                        "enum": ["low", "medium", "high", "critical"],
                        "description": "Risk level of this command."
                    },
                    "riskReason": {
                        "type": "string",
                        "description": "Brief reason for the selected risk level."
                    }
                },
                "required": ["thought", "command", "riskLevel", "riskReason"],
                "additionalProperties": false
            }))
            .with_strict(true),
        Tool::new(TOOL_FINAL_ANSWER)
            .with_description(
                "Finish the agent task and provide the user-facing final answer. Use this when \
                 no more command execution is needed.",
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "thought": {
                        "type": "string",
                        "description": "Brief reason why the task is complete or cannot continue."
                    },
                    "answer": {
                        "type": "string",
                        "description": "Final user-facing answer."
                    }
                },
                "required": ["thought", "answer"],
                "additionalProperties": false
            }))
            .with_strict(true),
    ]
}

fn parse_agent_tool_invocation(tool_calls: Vec<ToolCall>) -> AppResult<AgentToolInvocation> {
    let mut tool_calls: Vec<ToolCall> = tool_calls
        .into_iter()
        .filter(|call| !call.fn_name.trim().is_empty())
        .collect();
    if tool_calls.len() != 1 {
        return Err(AppError::Config(format!(
            "Expected exactly one AI agent tool call, got {}",
            tool_calls.len()
        )));
    }

    let tool_call = tool_calls.remove(0);
    match tool_call.fn_name.as_str() {
        TOOL_EXECUTE_COMMAND => {
            let args: ExecuteCommandToolArgs =
                serde_json::from_value(tool_call.fn_arguments.clone()).map_err(|error| {
                    AppError::Config(format!("Invalid execute_command tool arguments: {error}"))
                })?;
            if args.command.trim().is_empty() {
                return Err(AppError::Config(
                    "execute_command tool call is missing command".to_string(),
                ));
            }
            Ok(AgentToolInvocation::ExecuteCommand { tool_call, args })
        }
        TOOL_FINAL_ANSWER => {
            let args: FinalAnswerToolArgs = serde_json::from_value(tool_call.fn_arguments.clone())
                .map_err(|error| {
                    AppError::Config(format!("Invalid final_answer tool arguments: {error}"))
                })?;
            Ok(AgentToolInvocation::FinalAnswer { args })
        }
        other => Err(AppError::Config(format!(
            "Unknown AI agent tool call '{other}'"
        ))),
    }
}

fn parsed_from_execute_tool(args: &ExecuteCommandToolArgs) -> AgentLlmResponse {
    AgentLlmResponse {
        thought: args.thought.clone(),
        action: TOOL_EXECUTE_COMMAND.to_string(),
        command: Some(args.command.clone()),
        target_terminal_session_id: args.target_terminal_session_id.clone(),
        risk_level: Some(args.risk_level.clone()),
        risk_reason: Some(args.risk_reason.clone()),
        answer: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalDecision {
    Auto,
    NeedsApproval,
}

fn max_risk(a: RiskLevel, b: RiskLevel) -> RiskLevel {
    if a >= b { a } else { b }
}

fn risk_label(risk: &RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

fn assess_local_command_risk(command: &str) -> (RiskLevel, String, bool) {
    let risk = crate::core::capabilities::assess_command_risk(command);
    (risk.level, risk.reason, risk.auto_executable)
}

fn assess_agent_command_risk(parsed: &AgentLlmResponse, command: &str) -> RiskAssessment {
    let model_risk = parsed.risk_level.clone().unwrap_or(RiskLevel::Medium);
    let (local_risk, local_reason, local_auto_executable) = assess_local_command_risk(command);
    let effective_risk = max_risk(model_risk.clone(), local_risk.clone());
    let risk_reason = parsed
        .risk_reason
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("AI: {}; local: {}", value.trim(), local_reason))
        .or_else(|| Some(format!("local: {local_reason}")));

    RiskAssessment {
        model_risk,
        local_risk,
        local_auto_executable,
        effective_risk,
        risk_reason,
    }
}

fn decide_agent_command_execution(
    settings: &AiSettings,
    assessment: &RiskAssessment,
) -> (ApprovalDecision, Option<String>) {
    match settings.agent_command_execution_mode {
        AgentCommandExecutionMode::ConfirmEach => (
            ApprovalDecision::NeedsApproval,
            Some("execution policy requires confirmation for every command".to_string()),
        ),
        AgentCommandExecutionMode::Auto => (ApprovalDecision::Auto, None),
        AgentCommandExecutionMode::Smart => {
            if assessment.effective_risk == RiskLevel::Critical {
                return (
                    ApprovalDecision::NeedsApproval,
                    Some(
                        "critical risk always requires manual confirmation in smart mode"
                            .to_string(),
                    ),
                );
            }
            if assessment.effective_risk <= settings.agent_smart_auto_execute_max_risk {
                (ApprovalDecision::Auto, None)
            } else {
                (
                    ApprovalDecision::NeedsApproval,
                    Some(format!(
                        "effective risk {} exceeds smart auto-execute threshold {}",
                        risk_label(&assessment.effective_risk),
                        risk_label(&settings.agent_smart_auto_execute_max_risk)
                    )),
                )
            }
        }
    }
}

fn decide_external_agent_command_execution(
    mode: &AiPermissionMode,
    assessment: &RiskAssessment,
) -> (ApprovalDecision, Option<String>) {
    match mode {
        AiPermissionMode::Observer | AiPermissionMode::Confirm => (
            ApprovalDecision::NeedsApproval,
            Some("external agent permission mode requires confirmation".to_string()),
        ),
        AiPermissionMode::Auto
            if assessment.local_auto_executable && assessment.effective_risk < RiskLevel::High =>
        {
            (ApprovalDecision::Auto, None)
        }
        AiPermissionMode::Auto => (
            ApprovalDecision::NeedsApproval,
            Some("safe auto requires confirmation for unknown or high-risk commands".to_string()),
        ),
        AiPermissionMode::FullAccess => (ApprovalDecision::Auto, None),
    }
}

fn build_execute_action(
    command: &str,
    target: Option<AiTerminalTarget>,
    assessment: &RiskAssessment,
    approval_reason: Option<String>,
) -> AgentStepAction {
    AgentStepAction {
        kind: AgentActionKind::ExecuteCommand,
        command: Some(command.to_string()),
        target,
        risk_level: Some(assessment.effective_risk.clone()),
        model_risk_level: Some(assessment.model_risk.clone()),
        local_risk_level: Some(assessment.local_risk.clone()),
        risk_reason: assessment.risk_reason.clone(),
        approval_reason,
        answer: None,
    }
}

fn build_final_action(answer: String) -> AgentStepAction {
    AgentStepAction {
        kind: AgentActionKind::FinalAnswer,
        command: None,
        target: None,
        risk_level: None,
        model_risk_level: None,
        local_risk_level: None,
        risk_reason: None,
        approval_reason: None,
        answer: Some(answer),
    }
}

fn resolve_agent_command_target(
    request: &AiChatRequest,
    target_terminal_session_id: Option<&str>,
) -> AppResult<AiTerminalTarget> {
    match request.targets.as_slice() {
        [] => request
            .terminal_session_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .map(|terminal_session_id| AiTerminalTarget {
                terminal_session_id: terminal_session_id.clone(),
                connection_id: request.connection_id.clone(),
                label: terminal_session_id.clone(),
                host: request.context.host.clone(),
                username: request.context.username.clone(),
                session_type: "Unknown".to_string(),
            })
            .ok_or_else(|| AppError::Config("Agent mode requires a terminal session".to_string())),
        [target] => Ok(target.clone()),
        targets => {
            let Some(target_id) =
                target_terminal_session_id.filter(|value| !value.trim().is_empty())
            else {
                return Err(AppError::Config(
                    "Agent command is missing targetTerminalSessionId".to_string(),
                ));
            };
            targets
                .iter()
                .find(|target| target.terminal_session_id == target_id)
                .cloned()
                .ok_or_else(|| {
                    AppError::Config(format!(
                        "Agent command target '{target_id}' is not an available terminal"
                    ))
                })
        }
    }
}

fn append_agent_command_audit(
    app: &AppHandle,
    request: &AiChatRequest,
    action: &str,
    command: &str,
    risk_level: RiskLevel,
    executed: bool,
    blocked: bool,
) {
    let _ = append_ai_audit(
        app,
        AppendAiAuditRequest {
            connection_id: request.connection_id.clone(),
            action: action.to_string(),
            user_input: Some(request.user_input.clone()),
            generated_command: Some(command.to_string()),
            risk_level: Some(risk_level),
            inserted_to_terminal: false,
            executed,
            blocked,
            source: None,
            client: None,
            capability: None,
            session_id: None,
            permission_mode: (request.agent_kind != AiAgentKind::Nyaterm)
                .then(|| request.permission_mode.clone()),
            approval_decision: None,
            success: None,
            duration_ms: None,
            error: None,
        },
    );
}

pub(super) async fn run_external_agent_command_step(
    app: &AppHandle,
    session_manager: Arc<SessionManager>,
    approval_manager: Arc<AgentApprovalManager>,
    stream_id: &str,
    session_id: &str,
    request: &AiChatRequest,
    settings: &AiSettings,
    step_index: u16,
    command: String,
    reason: Option<String>,
    target_terminal_session_id: Option<String>,
) -> AppResult<CommandObservation> {
    let parsed = AgentLlmResponse {
        thought: reason
            .clone()
            .unwrap_or_else(|| "Codex requested terminal command execution".to_string()),
        action: "execute_command".to_string(),
        command: Some(command.clone()),
        target_terminal_session_id,
        risk_level: None,
        risk_reason: reason,
        answer: None,
    };
    let assessment = assess_agent_command_risk(&parsed, &command);
    let command_target =
        resolve_agent_command_target(request, parsed.target_terminal_session_id.as_deref())?;
    let (decision, approval_reason) = if request.agent_kind == AiAgentKind::Nyaterm {
        decide_agent_command_execution(settings, &assessment)
    } else {
        decide_external_agent_command_execution(&request.permission_mode, &assessment)
    };

    if request.agent_kind != AiAgentKind::Nyaterm
        && request.permission_mode == AiPermissionMode::Observer
    {
        append_agent_command_audit(
            app,
            request,
            "ai.external_agent_observer_block",
            &command,
            assessment.effective_risk.clone(),
            false,
            true,
        );
        return Err(AppError::Config(
            "Observer permission mode does not allow terminal writes".to_string(),
        ));
    }

    if decision == ApprovalDecision::NeedsApproval {
        let approval_step = AgentStepPayload {
            stream_id: stream_id.to_string(),
            session_id: Some(session_id.to_string()),
            step_index,
            thought: parsed.thought.clone(),
            action: build_execute_action(
                &command,
                Some(command_target.clone()),
                &assessment,
                approval_reason.clone(),
            ),
            observation: None,
            status: AgentStepStatus::NeedsApproval,
            error: None,
        };
        emit_agent_step(app, stream_id, approval_step);

        let approval_key = format!("{stream_id}-{step_index}");
        let approved = approval_manager
            .register(approval_key)
            .await
            .await
            .unwrap_or(false);

        if !approved {
            let step = AgentStepPayload {
                stream_id: stream_id.to_string(),
                session_id: Some(session_id.to_string()),
                step_index,
                thought: parsed.thought,
                action: build_execute_action(
                    &command,
                    Some(command_target.clone()),
                    &assessment,
                    approval_reason,
                ),
                observation: None,
                status: AgentStepStatus::Rejected,
                error: None,
            };
            emit_agent_step(app, stream_id, step);
            append_agent_command_audit(
                app,
                request,
                "ai.agent_reject_execute",
                &command,
                assessment.effective_risk,
                false,
                true,
            );
            return Err(AppError::Cancelled(build_agent_rejected_message(
                &command,
                &request.options.language,
            )));
        }
    }

    emit_agent_step(
        app,
        stream_id,
        AgentStepPayload {
            stream_id: stream_id.to_string(),
            session_id: Some(session_id.to_string()),
            step_index,
            thought: parsed.thought.clone(),
            action: build_execute_action(&command, Some(command_target.clone()), &assessment, None),
            observation: None,
            status: AgentStepStatus::Running,
            error: None,
        },
    );

    let step_timeout = settings
        .agent_step_timeout_ms
        .unwrap_or(DEFAULT_AGENT_STEP_TIMEOUT_MS);
    let result = if settings.agent_background_execution_enabled {
        execute_command_in_background(
            &session_manager,
            &command_target.terminal_session_id,
            &command,
            step_timeout,
        )
        .await
    } else {
        execute_command_on_session(
            app,
            session_manager,
            &command_target.terminal_session_id,
            &command,
            &request.options.language,
            step_timeout,
            step_index,
            settings.terminal_output_lines,
        )
        .await
    };

    match result {
        Ok(observation) => {
            emit_agent_step(
                app,
                stream_id,
                AgentStepPayload {
                    stream_id: stream_id.to_string(),
                    session_id: Some(session_id.to_string()),
                    step_index,
                    thought: parsed.thought,
                    action: build_execute_action(&command, Some(command_target), &assessment, None),
                    observation: Some(observation.clone()),
                    status: AgentStepStatus::Completed,
                    error: None,
                },
            );
            append_agent_command_audit(
                app,
                request,
                if decision == ApprovalDecision::Auto {
                    "ai.agent_auto_execute"
                } else {
                    "ai.agent_authorized_execute"
                },
                &command,
                assessment.effective_risk,
                true,
                false,
            );
            Ok(observation)
        }
        Err(error) => {
            emit_agent_step(
                app,
                stream_id,
                AgentStepPayload {
                    stream_id: stream_id.to_string(),
                    session_id: Some(session_id.to_string()),
                    step_index,
                    thought: parsed.thought,
                    action: build_execute_action(&command, Some(command_target), &assessment, None),
                    observation: None,
                    status: AgentStepStatus::Failed,
                    error: Some(error.to_string()),
                },
            );
            append_agent_command_audit(
                app,
                request,
                "ai.agent_execute_failed",
                &command,
                assessment.effective_risk,
                false,
                true,
            );
            Err(error)
        }
    }
}

async fn run_agent_tool_step(
    app: &AppHandle,
    stream_id: &str,
    session_id: &str,
    resolved_model: &ResolvedAiModel,
    conversation: &[ChatMessage],
    settings: &AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<AgentToolInvocation> {
    let client = build_client(resolved_model, settings)?;
    let chat_req = ChatRequest::new(conversation.to_vec()).with_tools(agent_tools());
    let chat_options = build_chat_options(settings)
        .with_capture_tool_calls(true)
        .with_tool_choice(ToolChoice::Required);

    let stream_result = tokio::time::timeout(
        Duration::from_millis(settings.timeout_ms),
        client.exec_chat_stream(&resolved_model.model_name, chat_req, Some(&chat_options)),
    )
    .await
    .map_err(|_| AppError::Config("AI request timed out".to_string()))?
    .map_err(|error| AppError::Config(format!("AI request failed: {error}")))?;

    let mut raw_text = String::new();
    let mut stream = stream_result.stream;
    let idle_duration = Duration::from_millis(settings.timeout_ms);
    let idle_deadline = tokio::time::sleep(idle_duration);
    tokio::pin!(idle_deadline);

    loop {
        tokio::select! {
            _ = &mut idle_deadline => {
                return Err(AppError::Config("AI stream timed out (no data received)".to_string()));
            }
            _ = &mut *cancel_rx => {
                return Err(AppError::Cancelled("AI stream cancelled".to_string()));
            }
            item = stream.next() => {
                idle_deadline.as_mut().reset(tokio::time::Instant::now() + idle_duration);
                match item {
                    Some(Ok(ChatStreamEvent::Start)) => {}
                    Some(Ok(ChatStreamEvent::Chunk(chunk))) => {
                        raw_text.push_str(&chunk.content);
                    }
                    Some(Ok(ChatStreamEvent::ReasoningChunk(chunk))) => {
                        if !chunk.content.is_empty() {
                            emit_stream_event(app, stream_id, AiStreamEventPayload {
                                event_type: "reasoning_delta".to_string(),
                                stream_id: stream_id.to_string(),
                                session_id: Some(session_id.to_string()),
                                text_delta: None,
                                reasoning_delta: Some(chunk.content),
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::ToolCallChunk(_))) => {}
                    Some(Ok(ChatStreamEvent::ThoughtSignatureChunk(_))) => {}
                    Some(Ok(ChatStreamEvent::End(end))) => {
                        let tool_calls = end.captured_into_tool_calls().unwrap_or_default();
                        return parse_agent_tool_invocation(tool_calls).map_err(|error| {
                            AppError::Config(format!(
                                "{error}; streamed text fallback candidate length {}",
                                raw_text.len()
                            ))
                        });
                    }
                    None => {
                        return Err(AppError::Config(
                            "AI stream ended without a tool call".to_string(),
                        ));
                    }
                    Some(Err(error)) => {
                        return Err(AppError::Config(format!("AI stream failed: {error}")));
                    }
                }
            }
        }
    }
}

async fn run_agent_legacy_json_step(
    app: &AppHandle,
    stream_id: &str,
    session_id: &str,
    resolved_model: &ResolvedAiModel,
    conversation: &[ChatMessage],
    settings: &AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<LegacyAgentStep> {
    let mut legacy_conversation = conversation.to_vec();
    legacy_conversation.push(ChatMessage::system(
        r#"Fallback protocol: tool calling is unavailable for this step. Return exactly one JSON object and no Markdown. For command execution use {"thought":"...","action":"execute_command","command":"...","riskLevel":"low|medium|high|critical","riskReason":"..."}. For final answer use {"thought":"...","action":"final_answer","answer":"..."}."#,
    ));

    if super::responses::uses_responses_api(resolved_model) {
        let synthetic_request = AiChatRequest {
            stream_id: None,
            session_id: Some(session_id.to_string()),
            connection_id: None,
            terminal_session_id: None,
            owner_scope: Default::default(),
            targets: vec![],
            target_contexts: vec![],
            mode: crate::config::AiMode::Agent,
            agent_kind: crate::config::AiAgentKind::Nyaterm,
            permission_mode: crate::config::AiPermissionMode::Confirm,
            model_id: None,
            model_name: None,
            default_target_session_id: None,
            existing_external_session_id: None,
            attachments: vec![],
            action: super::types::AiAction::GenerateCommand,
            user_input: String::new(),
            context: Default::default(),
            options: Default::default(),
        };
        let stream_result =
            super::responses::run_responses_chat_messages_stream_without_text_deltas(
                app,
                stream_id,
                &synthetic_request,
                settings,
                resolved_model,
                &legacy_conversation,
                cancel_rx,
            )
            .await?;
        return Ok(parse_legacy_agent_step_response(
            stream_id,
            session_id,
            stream_result.text,
            stream_result.reasoning_content.unwrap_or_default(),
        ));
    }

    let client = build_client(resolved_model, settings)?;
    let chat_req = ChatRequest::new(legacy_conversation);
    let chat_options = build_chat_options(settings);

    let stream_result = tokio::time::timeout(
        Duration::from_millis(settings.timeout_ms),
        client.exec_chat_stream(&resolved_model.model_name, chat_req, Some(&chat_options)),
    )
    .await
    .map_err(|_| AppError::Config("AI request timed out".to_string()))?
    .map_err(|error| AppError::Config(format!("AI request failed: {error}")))?;

    let mut raw_output = String::new();
    let mut reasoning_output = String::new();
    let mut stream = stream_result.stream;
    let idle_duration = Duration::from_millis(settings.timeout_ms);
    let idle_deadline = tokio::time::sleep(idle_duration);
    tokio::pin!(idle_deadline);

    loop {
        tokio::select! {
            _ = &mut idle_deadline => break,
            _ = &mut *cancel_rx => {
                return Err(AppError::Cancelled("AI stream cancelled".to_string()));
            }
            item = stream.next() => {
                idle_deadline.as_mut().reset(tokio::time::Instant::now() + idle_duration);
                match item {
                    Some(Ok(ChatStreamEvent::Chunk(chunk))) => {
                        if !chunk.content.is_empty() {
                            raw_output.push_str(&chunk.content);
                        }
                    }
                    Some(Ok(ChatStreamEvent::ReasoningChunk(chunk))) => {
                        if !chunk.content.is_empty() {
                            reasoning_output.push_str(&chunk.content);
                            emit_stream_event(app, stream_id, AiStreamEventPayload {
                                event_type: "reasoning_delta".to_string(),
                                stream_id: stream_id.to_string(),
                                session_id: Some(session_id.to_string()),
                                text_delta: None,
                                reasoning_delta: Some(chunk.content),
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::End(end))) => {
                        if reasoning_output.is_empty()
                            && let Some(r) = end.captured_reasoning_content
                        {
                            reasoning_output = r;
                        }
                        break;
                    }
                    None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        return Err(AppError::Config(format!("AI stream failed: {error}")));
                    }
                }
            }
        }
    }

    Ok(parse_legacy_agent_step_response(
        stream_id,
        session_id,
        raw_output,
        reasoning_output,
    ))
}

fn parse_legacy_agent_step_response(
    stream_id: &str,
    session_id: &str,
    raw_output: String,
    reasoning_output: String,
) -> LegacyAgentStep {
    let candidate =
        extract_json_object(&raw_output).unwrap_or_else(|| raw_output.trim().to_string());
    let parsed = match serde_json::from_str(&candidate) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(
                stream_id = %stream_id,
                session_id = %session_id,
                error = %error,
                raw_output_len = raw_output.len(),
                "Failed to parse legacy AI agent JSON response; falling back to final text"
            );
            let (text, _, _) =
                parse_model_output(&raw_output, trim_string_to_option(reasoning_output));
            AgentLlmResponse {
                thought: String::new(),
                action: "final_answer".to_string(),
                command: None,
                target_terminal_session_id: None,
                risk_level: None,
                risk_reason: None,
                answer: Some(text),
            }
        }
    };

    LegacyAgentStep { parsed, raw_output }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_response(risk: Option<RiskLevel>) -> AgentLlmResponse {
        AgentLlmResponse {
            thought: "next".to_string(),
            action: "execute_command".to_string(),
            command: Some("ls".to_string()),
            target_terminal_session_id: None,
            risk_level: risk,
            risk_reason: Some("model reason".to_string()),
            answer: None,
        }
    }

    #[test]
    fn parses_execute_command_tool_args_case_insensitively() {
        let call = ToolCall {
            call_id: "call-1".to_string(),
            fn_name: TOOL_EXECUTE_COMMAND.to_string(),
            fn_arguments: json!({
                "thought": "inspect",
                "command": "ls -la",
                "riskLevel": "LOW",
                "riskReason": "read only"
            }),
            thought_signatures: None,
        };

        let parsed = parse_agent_tool_invocation(vec![call]).unwrap();
        match parsed {
            AgentToolInvocation::ExecuteCommand { args, .. } => {
                assert_eq!(args.thought, "inspect");
                assert_eq!(args.command, "ls -la");
                assert_eq!(args.risk_level, RiskLevel::Low);
                assert_eq!(args.risk_reason, "read only");
            }
            AgentToolInvocation::FinalAnswer { .. } => panic!("expected execute_command"),
        }
    }

    #[test]
    fn parses_final_answer_tool_args() {
        let call = ToolCall {
            call_id: "call-2".to_string(),
            fn_name: TOOL_FINAL_ANSWER.to_string(),
            fn_arguments: json!({
                "thought": "done",
                "answer": "All set"
            }),
            thought_signatures: None,
        };

        let parsed = parse_agent_tool_invocation(vec![call]).unwrap();
        match parsed {
            AgentToolInvocation::FinalAnswer { args } => {
                assert_eq!(args.thought, "done");
                assert_eq!(args.answer, "All set");
            }
            AgentToolInvocation::ExecuteCommand { .. } => panic!("expected final_answer"),
        }
    }

    #[test]
    fn rejects_unknown_tool_name() {
        let call = ToolCall {
            call_id: "call-3".to_string(),
            fn_name: "unknown".to_string(),
            fn_arguments: json!({}),
            thought_signatures: None,
        };

        assert!(parse_agent_tool_invocation(vec![call]).is_err());
    }

    #[test]
    fn rejects_invalid_tool_risk_level() {
        let call = ToolCall {
            call_id: "call-4".to_string(),
            fn_name: TOOL_EXECUTE_COMMAND.to_string(),
            fn_arguments: json!({
                "thought": "inspect",
                "command": "ls",
                "riskLevel": "spicy",
                "riskReason": "not a real level"
            }),
            thought_signatures: None,
        };

        assert!(parse_agent_tool_invocation(vec![call]).is_err());
    }

    #[test]
    fn native_tool_call_error_disables_native_calls_for_turn() {
        let mut mode = NativeToolCallMode::Enabled;
        let error =
            AppError::Config("AI request failed: HTTP error. Status: 400 Bad Request".to_string());

        assert!(should_fallback_to_legacy_after_tool_error(
            &mut mode, &error
        ));
        assert_eq!(mode, NativeToolCallMode::DisabledForTurn);
        assert!(!mode.should_attempt());
    }

    #[test]
    fn disabled_native_tool_call_mode_skips_subsequent_attempts() {
        let mut mode = NativeToolCallMode::Enabled;
        let error = AppError::Config("AI stream ended without a tool call".to_string());

        assert!(mode.should_attempt());
        assert!(should_fallback_to_legacy_after_tool_error(
            &mut mode, &error
        ));
        assert!(!mode.should_attempt());
    }

    #[test]
    fn cancelled_native_tool_call_does_not_fallback_or_disable_native_calls() {
        let mut mode = NativeToolCallMode::Enabled;
        let error = AppError::Cancelled("AI stream cancelled".to_string());

        assert!(!should_fallback_to_legacy_after_tool_error(
            &mut mode, &error
        ));
        assert_eq!(mode, NativeToolCallMode::Enabled);
        assert!(mode.should_attempt());
    }

    #[test]
    fn local_risk_rules_cover_expected_levels() {
        assert_eq!(assess_local_command_risk("ls -la").0, RiskLevel::Low);
        assert_eq!(
            assess_local_command_risk("touch app.log").0,
            RiskLevel::Medium
        );
        assert_eq!(
            assess_local_command_risk("sudo apt install nginx").0,
            RiskLevel::High
        );
        assert_eq!(
            assess_local_command_risk("rm -rf /tmp/build-cache").0,
            RiskLevel::High
        );
        assert_eq!(assess_local_command_risk("rm -rf /").0, RiskLevel::Critical);
    }

    #[test]
    fn smart_policy_auto_executes_within_threshold() {
        let mut settings = AiSettings::default();
        settings.agent_command_execution_mode = AgentCommandExecutionMode::Smart;
        settings.agent_smart_auto_execute_max_risk = RiskLevel::Low;
        let assessment = assess_agent_command_risk(&parsed_response(Some(RiskLevel::Low)), "ls");
        assert_eq!(
            decide_agent_command_execution(&settings, &assessment).0,
            ApprovalDecision::Auto
        );
    }

    #[test]
    fn smart_policy_requires_approval_above_threshold_and_for_critical() {
        let mut settings = AiSettings::default();
        settings.agent_command_execution_mode = AgentCommandExecutionMode::Smart;
        settings.agent_smart_auto_execute_max_risk = RiskLevel::High;

        let medium_threshold = assess_agent_command_risk(
            &parsed_response(Some(RiskLevel::High)),
            "rm -rf /tmp/build-cache",
        );
        assert_eq!(
            decide_agent_command_execution(&settings, &medium_threshold).0,
            ApprovalDecision::Auto
        );

        let critical =
            assess_agent_command_risk(&parsed_response(Some(RiskLevel::Low)), "rm -rf /");
        assert_eq!(
            decide_agent_command_execution(&settings, &critical).0,
            ApprovalDecision::NeedsApproval
        );
    }

    #[test]
    fn confirm_and_auto_policy_decisions_are_explicit() {
        let assessment = assess_agent_command_risk(&parsed_response(None), "ls");

        let mut settings = AiSettings::default();
        settings.agent_command_execution_mode = AgentCommandExecutionMode::ConfirmEach;
        assert_eq!(
            decide_agent_command_execution(&settings, &assessment).0,
            ApprovalDecision::NeedsApproval
        );

        settings.agent_command_execution_mode = AgentCommandExecutionMode::Auto;
        assert_eq!(
            decide_agent_command_execution(&settings, &assessment).0,
            ApprovalDecision::Auto
        );
    }

    #[test]
    fn external_agent_permission_modes_are_explicit() {
        let safe = assess_agent_command_risk(&parsed_response(None), "ls -la");
        let high = assess_agent_command_risk(&parsed_response(None), "sudo reboot");
        let unknown = assess_agent_command_risk(&parsed_response(None), "custom-deploy production");

        assert_eq!(
            decide_external_agent_command_execution(&AiPermissionMode::Observer, &safe).0,
            ApprovalDecision::NeedsApproval
        );
        assert_eq!(
            decide_external_agent_command_execution(&AiPermissionMode::Confirm, &safe).0,
            ApprovalDecision::NeedsApproval
        );
        assert_eq!(
            decide_external_agent_command_execution(&AiPermissionMode::Auto, &safe).0,
            ApprovalDecision::Auto
        );
        assert_eq!(
            decide_external_agent_command_execution(&AiPermissionMode::Auto, &high).0,
            ApprovalDecision::NeedsApproval
        );
        assert_eq!(
            decide_external_agent_command_execution(&AiPermissionMode::Auto, &unknown).0,
            ApprovalDecision::NeedsApproval
        );
        assert_eq!(
            decide_external_agent_command_execution(&AiPermissionMode::FullAccess, &high).0,
            ApprovalDecision::Auto
        );
    }

    #[test]
    fn merge_command_output_preserves_stdout_and_stderr() {
        assert_eq!(merge_command_output("", ""), "");
        assert_eq!(merge_command_output("out", ""), "out");
        assert_eq!(merge_command_output("", "err"), "err");
        assert_eq!(merge_command_output("out", "err"), "out\nerr");
    }

    #[tokio::test]
    async fn background_execution_rejects_unsupported_session_types() {
        use crate::core::{
            DynamicTitleCapabilities, SessionHandle, SessionInfo, session_command_channel,
        };
        use tokio::sync::Mutex;

        let manager = SessionManager::new();
        let (cmd_tx, _cmd_rx) = session_command_channel("serial-1");
        manager
            .add_session(SessionHandle {
                info: SessionInfo {
                    id: "serial-1".to_string(),
                    name: "serial-1".to_string(),
                    session_type: SessionType::Serial,
                    started_at: crate::core::now_session_started_at(),
                    connection_id: None,
                    connected: true,
                    owner_window_label: None,
                    ai_execution_profile: AiExecutionProfile::SendOnly,
                    injection_active: false,
                    dynamic_title_capabilities: DynamicTitleCapabilities::default(),
                    remote_file_browser_enabled: false,
                    remote_stats_enabled: false,
                    ssh_profile: None,
                },
                cmd_tx,
                startup_input_barrier: None,
                ssh_config: None,
                ssh_handle: None,
                cwd: Arc::new(Mutex::new(Default::default())),
                remote_fs: None,
            })
            .await;

        let error = execute_command_in_background(&manager, "serial-1", "help", 1000)
            .await
            .expect_err("serial sessions should not support background execution");

        assert!(error.to_string().contains("not supported"));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_stream(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    approval_manager: Arc<AgentApprovalManager>,
    stream_id: String,
    session_id: String,
    mut request: AiChatRequest,
    settings: AiSettings,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        action = ?request.action,
        connection_id = ?request.connection_id,
        terminal_session_id = ?request.terminal_session_id,
        "Running AI agent stream"
    );

    emit_stream_event(
        &app,
        &stream_id,
        AiStreamEventPayload {
            event_type: "start".to_string(),
            stream_id: stream_id.clone(),
            session_id: Some(session_id.clone()),
            text_delta: None,
            reasoning_delta: None,
            message: None,
            command_cards: vec![],
            usage: None,
            error: None,
        },
    );

    if settings.redaction_enabled {
        redact_context(&mut request.context);
        request.user_input = redact_sensitive_text(&request.user_input);
    }

    if settings.record_history {
        if let Err(error) = save_user_message(&app, &session_id, &request) {
            tracing::warn!(
                stream_id = %stream_id,
                session_id = %session_id,
                error = %error,
                "Failed to save agent user message before execution"
            );
        }
    }

    let _terminal_session_id = match &request.terminal_session_id {
        Some(id) if !id.trim().is_empty() => id.clone(),
        _ => {
            emit_agent_error(
                &app,
                &stream_id,
                &session_id,
                "Agent mode requires a terminal session",
            );
            return;
        }
    };

    let resolved_model = match resolve_request_model(&settings, &request) {
        Ok(m) => m,
        Err(e) => {
            emit_agent_error(&app, &stream_id, &session_id, &e.to_string());
            return;
        }
    };

    let max_steps = settings.max_agent_steps.unwrap_or(DEFAULT_MAX_AGENT_STEPS);
    let step_timeout = settings
        .agent_step_timeout_ms
        .unwrap_or(DEFAULT_AGENT_STEP_TIMEOUT_MS);

    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        model_name = %resolved_model.model_name,
        provider_kind = ?resolved_model.provider_kind,
        max_steps,
        step_timeout,
        "AI agent stream resolved configuration"
    );

    let mut conversation = vec![ChatMessage::system(agent_system_prompt(
        &request.options.language,
    ))];
    let initial_prompt = build_agent_prompt(&request, &settings);
    conversation.push(ChatMessage::user(initial_prompt));

    let mut final_answer: Option<String> = None;
    let mut all_steps: Vec<AgentStepPayload> = Vec::new();
    let mut native_tool_call_mode = if super::responses::uses_responses_api(&resolved_model) {
        NativeToolCallMode::DisabledForTurn
    } else {
        NativeToolCallMode::Enabled
    };

    for step_index in 0..max_steps {
        tracing::debug!(
            stream_id = %stream_id,
            session_id = %session_id,
            step_index,
            conversation_len = conversation.len(),
            "Starting AI agent step"
        );

        if is_cancelled(&mut cancel_rx) {
            emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
            return;
        }

        let tool_invocation = if native_tool_call_mode.should_attempt() {
            match run_agent_tool_step(
                &app,
                &stream_id,
                &session_id,
                &resolved_model,
                &conversation,
                &settings,
                &mut cancel_rx,
            )
            .await
            {
                Ok(invocation) => Some(invocation),
                Err(error) => {
                    if !should_fallback_to_legacy_after_tool_error(
                        &mut native_tool_call_mode,
                        &error,
                    ) {
                        emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
                        return;
                    }
                    tracing::warn!(
                        stream_id = %stream_id,
                        session_id = %session_id,
                        step_index,
                        error = %error,
                        "AI agent native tool call step failed; native tool calling disabled for this agent turn; falling back to legacy JSON protocol"
                    );
                    None
                }
            }
        } else {
            None
        };

        let mut execute_tool_call: Option<ToolCall> = None;
        let mut legacy_raw_output: Option<String> = None;
        let parsed = match tool_invocation {
            Some(AgentToolInvocation::ExecuteCommand { tool_call, args }) => {
                execute_tool_call = Some(tool_call);
                parsed_from_execute_tool(&args)
            }
            Some(AgentToolInvocation::FinalAnswer { args }) => AgentLlmResponse {
                thought: args.thought,
                action: TOOL_FINAL_ANSWER.to_string(),
                command: None,
                target_terminal_session_id: None,
                risk_level: None,
                risk_reason: None,
                answer: Some(args.answer),
            },
            None => match run_agent_legacy_json_step(
                &app,
                &stream_id,
                &session_id,
                &resolved_model,
                &conversation,
                &settings,
                &mut cancel_rx,
            )
            .await
            {
                Ok(step) => {
                    legacy_raw_output = Some(step.raw_output);
                    step.parsed
                }
                Err(error) => {
                    let message = if matches!(error, AppError::Cancelled(_)) {
                        "AI stream cancelled".to_string()
                    } else {
                        error.to_string()
                    };
                    emit_agent_error(&app, &stream_id, &session_id, &message);
                    return;
                }
            },
        };

        if let Some(raw_output) = legacy_raw_output {
            conversation.push(ChatMessage::assistant(&raw_output));
        }

        tracing::debug!(
            stream_id = %stream_id,
            session_id = %session_id,
            step_index,
            action = %parsed.action,
            has_command = parsed.command.as_ref().is_some_and(|value| !value.trim().is_empty()),
            has_answer = parsed.answer.as_ref().is_some_and(|value| !value.trim().is_empty()),
            "Parsed AI agent step response"
        );

        match parsed.action.as_str() {
            "final_answer" => {
                let answer = parsed.answer.unwrap_or_default();
                tracing::info!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    answer_len = answer.len(),
                    "AI agent produced final answer"
                );
                let step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought,
                    action: build_final_action(answer.clone()),
                    observation: None,
                    status: AgentStepStatus::Completed,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, step.clone());
                all_steps.push(step);
                final_answer = Some(answer);
                break;
            }
            "execute_command" => {
                let command = match &parsed.command {
                    Some(c) if !c.trim().is_empty() => c.trim().to_string(),
                    _ => {
                        emit_agent_error(
                            &app,
                            &stream_id,
                            &session_id,
                            "Agent returned execute_command without a command",
                        );
                        return;
                    }
                };

                let assessment = assess_agent_command_risk(&parsed, &command);
                let command_target = match resolve_agent_command_target(
                    &request,
                    parsed.target_terminal_session_id.as_deref(),
                ) {
                    Ok(target) => target,
                    Err(error) => {
                        let step = AgentStepPayload {
                            stream_id: stream_id.clone(),
                            session_id: Some(session_id.clone()),
                            step_index,
                            thought: parsed.thought,
                            action: build_execute_action(&command, None, &assessment, None),
                            observation: None,
                            status: AgentStepStatus::Failed,
                            error: Some(error.to_string()),
                        };
                        emit_agent_step(&app, &stream_id, step.clone());
                        all_steps.push(step);
                        let message = error.to_string();
                        if let Some(tool_call) = execute_tool_call.as_ref() {
                            conversation.push(ChatMessage::from(vec![tool_call.clone()]));
                            conversation.push(ChatMessage::from(ToolResponse::from_tool_call(
                                tool_call,
                                json!({
                                    "status": "failed",
                                    "error": message,
                                })
                                .to_string(),
                            )));
                        } else {
                            conversation.push(ChatMessage::user(message));
                        }
                        continue;
                    }
                };
                let (decision, approval_reason) =
                    decide_agent_command_execution(&settings, &assessment);

                tracing::info!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    mode = ?settings.agent_command_execution_mode,
                    model_risk = ?assessment.model_risk,
                    local_risk = ?assessment.local_risk,
                    effective_risk = ?assessment.effective_risk,
                    needs_approval = decision == ApprovalDecision::NeedsApproval,
                    command_preview = %safe_command_preview(&command),
                    "AI agent proposed command"
                );

                if decision == ApprovalDecision::NeedsApproval {
                    let approval_step = AgentStepPayload {
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id.clone()),
                        step_index,
                        thought: parsed.thought.clone(),
                        action: build_execute_action(
                            &command,
                            Some(command_target.clone()),
                            &assessment,
                            approval_reason.clone(),
                        ),
                        observation: None,
                        status: AgentStepStatus::NeedsApproval,
                        error: None,
                    };
                    emit_agent_step(&app, &stream_id, approval_step.clone());
                    all_steps.push(approval_step);

                    let approval_key = format!("{}-{}", stream_id, step_index);
                    let approval_rx = approval_manager.register(approval_key).await;

                    let approved = tokio::select! {
                        _ = &mut cancel_rx => {
                            emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
                            return;
                        }
                        result = approval_rx => result.unwrap_or(false),
                    };

                    if !approved {
                        let step = AgentStepPayload {
                            stream_id: stream_id.clone(),
                            session_id: Some(session_id.clone()),
                            step_index,
                            thought: parsed.thought,
                            action: build_execute_action(
                                &command,
                                Some(command_target.clone()),
                                &assessment,
                                approval_reason,
                            ),
                            observation: None,
                            status: AgentStepStatus::Rejected,
                            error: None,
                        };
                        emit_agent_step(&app, &stream_id, step.clone());
                        if let Some(last) = all_steps.last_mut() {
                            *last = step;
                        }

                        append_agent_command_audit(
                            &app,
                            &request,
                            "ai.agent_reject_execute",
                            &command,
                            assessment.effective_risk,
                            false,
                            true,
                        );

                        let skipped_msg =
                            build_agent_rejected_message(&command, &request.options.language);
                        if let Some(tool_call) = execute_tool_call.as_ref() {
                            conversation.push(ChatMessage::from(vec![tool_call.clone()]));
                            conversation.push(ChatMessage::from(ToolResponse::from_tool_call(
                                tool_call,
                                json!({
                                    "status": "rejected",
                                    "message": skipped_msg,
                                })
                                .to_string(),
                            )));
                        } else {
                            conversation.push(ChatMessage::user(skipped_msg));
                        }
                        continue;
                    }
                }

                let running_step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought.clone(),
                    action: build_execute_action(
                        &command,
                        Some(command_target.clone()),
                        &assessment,
                        None,
                    ),
                    observation: None,
                    status: AgentStepStatus::Running,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, running_step);
                if decision == ApprovalDecision::Auto {
                    all_steps.push(AgentStepPayload {
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id.clone()),
                        step_index,
                        thought: parsed.thought.clone(),
                        action: build_execute_action(
                            &command,
                            Some(command_target.clone()),
                            &assessment,
                            None,
                        ),
                        observation: None,
                        status: AgentStepStatus::Running,
                        error: None,
                    });
                }

                let obs = match if settings.agent_background_execution_enabled {
                    execute_command_in_background(
                        &session_manager,
                        &command_target.terminal_session_id,
                        &command,
                        step_timeout,
                    )
                    .await
                } else {
                    execute_command_on_session(
                        &app,
                        session_manager.clone(),
                        &command_target.terminal_session_id,
                        &command,
                        &request.options.language,
                        step_timeout,
                        step_index,
                        settings.terminal_output_lines,
                    )
                    .await
                } {
                    Ok(obs) => obs,
                    Err(e) => {
                        let step = AgentStepPayload {
                            stream_id: stream_id.clone(),
                            session_id: Some(session_id.clone()),
                            step_index,
                            thought: parsed.thought,
                            action: build_execute_action(
                                &command,
                                Some(command_target.clone()),
                                &assessment,
                                None,
                            ),
                            observation: None,
                            status: AgentStepStatus::Failed,
                            error: Some(e.to_string()),
                        };
                        emit_agent_step(&app, &stream_id, step.clone());
                        if let Some(last) = all_steps.last_mut() {
                            *last = step;
                        }

                        tracing::warn!(
                            stream_id = %stream_id,
                            session_id = %session_id,
                            step_index,
                            error = %e,
                            command_preview = %safe_command_preview(&command),
                            "AI agent command execution failed"
                        );

                        append_agent_command_audit(
                            &app,
                            &request,
                            "ai.agent_execute_failed",
                            &command,
                            assessment.effective_risk.clone(),
                            false,
                            false,
                        );

                        let err_msg =
                            build_agent_failed_message(&e.to_string(), &request.options.language);
                        if let Some(tool_call) = execute_tool_call.as_ref() {
                            conversation.push(ChatMessage::from(vec![tool_call.clone()]));
                            conversation.push(ChatMessage::from(ToolResponse::from_tool_call(
                                tool_call,
                                json!({
                                    "status": "failed",
                                    "error": e.to_string(),
                                    "message": err_msg,
                                })
                                .to_string(),
                            )));
                        } else {
                            conversation.push(ChatMessage::user(err_msg));
                        }
                        continue;
                    }
                };

                tracing::info!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    exit_code = obs.exit_code,
                    duration_ms = obs.duration_ms,
                    output_len = obs.output.len(),
                    command_preview = %safe_command_preview(&command),
                    "AI agent command executed successfully"
                );

                let completed_step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought,
                    action: build_execute_action(&command, Some(command_target), &assessment, None),
                    observation: Some(obs.clone()),
                    status: AgentStepStatus::Completed,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, completed_step.clone());
                if let Some(last) = all_steps.last_mut() {
                    *last = completed_step;
                }

                append_agent_command_audit(
                    &app,
                    &request,
                    if decision == ApprovalDecision::Auto {
                        "ai.agent_auto_execute"
                    } else {
                        "ai.agent_authorized_execute"
                    },
                    &command,
                    assessment.effective_risk,
                    true,
                    false,
                );

                let obs_msg = build_observation_message(&obs, &command, &request.options.language);
                if let Some(tool_call) = execute_tool_call.as_ref() {
                    conversation.push(ChatMessage::from(vec![tool_call.clone()]));
                    conversation.push(ChatMessage::from(ToolResponse::from_tool_call(
                        tool_call,
                        serde_json::to_string(&obs).unwrap_or_else(|_| obs_msg.clone()),
                    )));
                } else {
                    conversation.push(ChatMessage::user(obs_msg));
                }
            }
            other => {
                let fallback = build_agent_unknown_action_message(
                    other,
                    parsed.answer.as_deref().unwrap_or(&parsed.thought),
                    &request.options.language,
                );
                final_answer = Some(fallback);
                break;
            }
        }
    }

    active_streams().lock().unwrap().remove(&stream_id);

    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        step_count = all_steps.len(),
        has_final_answer = final_answer.is_some(),
        "AI agent stream finished loop"
    );

    let answer_text =
        final_answer.unwrap_or_else(|| agent_max_steps_message(&request.options.language).into());

    let message = AiMessage {
        id: format!("msg-{}", uuid()),
        session_id: session_id.clone(),
        role: AiMessageRole::Assistant,
        content: answer_text,
        created_at: now_rfc3339(),
        reasoning_content: None,
        command_cards: vec![],
    };

    if settings.record_history {
        if let Err(error) = append_message(&app, message.clone()) {
            tracing::warn!(
                stream_id = %stream_id,
                session_id = %session_id,
                error = %error,
                "Failed to append AI agent assistant message"
            );
        }
    }

    emit_stream_event(
        &app,
        &stream_id,
        AiStreamEventPayload {
            event_type: "done".to_string(),
            stream_id: stream_id.clone(),
            session_id: Some(session_id),
            text_delta: None,
            reasoning_delta: None,
            message: Some(message),
            command_cards: vec![],
            usage: None,
            error: None,
        },
    );
}
