use crate::config::{
    self, ai_model_id_for_credential, AiMode, AiModelConfigItem, AiModelSource,
    AiProviderCredential, AiProviderKind, AiSettings,
};
use crate::core::session::{SessionManager, SessionType};
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl Default for RiskLevel {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCommandCard {
    pub id: String,
    pub title: String,
    pub command: String,
    pub explanation: String,
    pub risk_level: RiskLevel,
    pub risk_reason: String,
    pub expected_effect: String,
    #[serde(default)]
    pub rollback: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

// ---------------------------------------------------------------------------
// Agent (ReAct) types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionKind {
    ExecuteCommand,
    FinalAnswer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepAction {
    pub kind: AgentActionKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandObservation {
    pub output: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStepStatus {
    Running,
    Completed,
    NeedsApproval,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepPayload {
    pub stream_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub step_index: u16,
    pub thought: String,
    pub action: AgentStepAction,
    #[serde(default)]
    pub observation: Option<CommandObservation>,
    pub status: AgentStepStatus,
    #[serde(default)]
    pub error: Option<String>,
}

/// Parsed single-step agent response from the LLM.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct AgentLlmResponse {
    #[serde(default)]
    thought: String,
    action: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    risk_level: Option<RiskLevel>,
    #[serde(default)]
    answer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiContext {
    #[serde(default)]
    pub connection_name: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub recent_output: String,
    #[serde(default)]
    pub selected_text: String,
    #[serde(default)]
    pub input_buffer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAction {
    GenerateCommand,
    ExplainOutput,
    ExplainSelected,
    AnalyzeError,
    RepairFromSelection,
    CustomTerminalAction,
    CustomFileAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiRequestOptions {
    #[serde(default = "default_max_output_commands")]
    pub max_output_commands: u8,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_safety_mode")]
    pub safety_mode: String,
    #[serde(default = "default_history_turns")]
    pub history_turns: u16,
    #[serde(default = "default_allowed_command_risk_level")]
    pub allowed_command_risk_level: RiskLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatRequest {
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub connection_id: Option<String>,
    /// The terminal session id to execute commands on (Agent mode).
    #[serde(default)]
    pub terminal_session_id: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: AiMode,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    pub action: AiAction,
    pub user_input: String,
    #[serde(default)]
    pub context: AiContext,
    #[serde(default)]
    pub options: AiRequestOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamStart {
    pub stream_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamEventPayload {
    #[serde(rename = "type")]
    pub event_type: String,
    pub stream_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub text_delta: Option<String>,
    #[serde(default)]
    pub reasoning_delta: Option<String>,
    #[serde(default)]
    pub message: Option<AiMessage>,
    #[serde(default)]
    pub command_cards: Vec<AiCommandCard>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRiskRequest {
    pub command: String,
    #[serde(default)]
    pub context: AiContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRiskResponse {
    pub risk_level: RiskLevel,
    pub blocked: bool,
    pub reason: String,
    pub safe_alternatives: Vec<String>,
    #[serde(default)]
    pub confirm_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSession {
    pub id: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessage {
    pub id: String,
    pub session_id: String,
    pub role: AiMessageRole,
    pub content: String,
    pub created_at: String,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub command_cards: Vec<AiCommandCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAuditLog {
    pub id: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub action: String,
    #[serde(default)]
    pub user_input: Option<String>,
    #[serde(default)]
    pub generated_command: Option<String>,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub inserted_to_terminal: bool,
    #[serde(default)]
    pub executed: bool,
    #[serde(default)]
    pub blocked: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendAiAuditRequest {
    #[serde(default)]
    pub connection_id: Option<String>,
    pub action: String,
    #[serde(default)]
    pub user_input: Option<String>,
    #[serde(default)]
    pub generated_command: Option<String>,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub inserted_to_terminal: bool,
    #[serde(default)]
    pub executed: bool,
    #[serde(default)]
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AiHistoryFile {
    #[serde(default)]
    sessions: Vec<AiSession>,
    #[serde(default)]
    messages: Vec<AiMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AiAuditFile {
    #[serde(default)]
    logs: Vec<AiAuditLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiModelOutput {
    #[serde(default)]
    text: String,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    command_cards: Vec<AiCommandCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDiscovery {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub provider_kind: Option<AiProviderKind>,
    #[serde(default)]
    pub credential_id: Option<String>,
    pub source: AiModelSource,
}

struct RiskPattern {
    regex: Regex,
    level: RiskLevel,
    blocked: bool,
    reason: &'static str,
    alternatives: &'static [&'static str],
    confirm_text: Option<&'static str>,
}

static ACTIVE_STREAMS: OnceLock<Mutex<HashMap<String, oneshot::Sender<()>>>> = OnceLock::new();
const AI_HISTORY_MAX_SESSIONS: usize = 200;
const AI_HISTORY_MAX_MESSAGES: usize = 2_000;
const AI_AUDIT_MAX_LOGS: usize = 2_000;

fn active_streams() -> &'static Mutex<HashMap<String, oneshot::Sender<()>>> {
    ACTIVE_STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cancel_all_chat_streams() {
    let senders: Vec<oneshot::Sender<()>> = active_streams()
        .lock()
        .unwrap()
        .drain()
        .map(|(_, tx)| tx)
        .collect();
    for sender in senders {
        let _ = sender.send(());
    }
}

fn default_max_output_commands() -> u8 {
    5
}

fn default_language() -> String {
    "zh-CN".to_string()
}

fn default_safety_mode() -> String {
    "strict".to_string()
}

fn default_history_turns() -> u16 {
    20
}

fn default_allowed_command_risk_level() -> RiskLevel {
    RiskLevel::Medium
}

fn default_mode() -> AiMode {
    AiMode::Ask
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn start_chat_stream(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    mut request: AiChatRequest,
) -> AppResult<AiStreamStart> {
    let settings = config::load_app_settings(&app)?;
    if !settings.ai.enabled {
        return Err(AppError::Config("AI assistant is disabled".to_string()));
    }

    let stream_id = request
        .stream_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ai-stream-{}", uuid()));
    let session_id = request
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ai-session-{}", uuid()));
    request.session_id = Some(session_id.clone());

    let (cancel_tx, cancel_rx) = oneshot::channel();
    {
        let mut streams = active_streams().lock().unwrap();
        if streams.contains_key(&stream_id) {
            return Err(AppError::Config("AI stream is already active".to_string()));
        }
        streams.insert(stream_id.clone(), cancel_tx);
    }

    let is_agent = request.mode == AiMode::Agent;
    let task_app = app.clone();
    let task_stream_id = stream_id.clone();
    let task_session_id = session_id.clone();

    if is_agent {
        tauri::async_runtime::spawn(async move {
            run_agent_stream(
                task_app,
                session_manager,
                task_stream_id,
                task_session_id,
                request,
                settings.ai,
                cancel_rx,
            )
            .await;
        });
    } else {
        tauri::async_runtime::spawn(async move {
            run_chat_stream(
                task_app,
                task_stream_id,
                task_session_id,
                request,
                settings.ai,
                cancel_rx,
            )
            .await;
        });
    }

    Ok(AiStreamStart {
        stream_id,
        session_id,
    })
}

pub fn cancel_chat_stream(stream_id: String) -> AppResult<()> {
    if let Some(sender) = active_streams().lock().unwrap().remove(&stream_id) {
        let _ = sender.send(());
    }
    Ok(())
}

async fn run_chat_stream(
    app: AppHandle,
    stream_id: String,
    session_id: String,
    mut request: AiChatRequest,
    settings: AiSettings,
    mut cancel_rx: oneshot::Receiver<()>,
) {
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
        let _ = save_user_message(&app, &session_id, &request);
    }

    let result = run_model_stream(&app, &stream_id, &request, &settings, &mut cancel_rx).await;

    match result {
        Ok(stream_result) => {
            if active_streams()
                .lock()
                .unwrap()
                .remove(&stream_id)
                .is_none()
            {
                emit_stream_event(
                    &app,
                    &stream_id,
                    AiStreamEventPayload {
                        event_type: "error".to_string(),
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id),
                        text_delta: None,
                        reasoning_delta: None,
                        message: None,
                        command_cards: vec![],
                        usage: None,
                        error: Some("AI stream cancelled".to_string()),
                    },
                );
                return;
            }

            let (text, reasoning_content, mut command_cards) =
                parse_model_output(&stream_result.text, stream_result.reasoning_content);
            for card in &mut command_cards {
                let risk = check_command_risk(CommandRiskRequest {
                    command: card.command.clone(),
                    context: request.context.clone(),
                });
                card.risk_level = risk.risk_level;
                card.risk_reason = risk.reason;
            }

            let message = AiMessage {
                id: format!("msg-{}", uuid()),
                session_id: session_id.clone(),
                role: AiMessageRole::Assistant,
                content: text,
                created_at: now_rfc3339(),
                reasoning_content,
                command_cards: command_cards.clone(),
            };

            if settings.record_history {
                let _ = append_message(&app, message.clone());
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
                    command_cards,
                    usage: None,
                    error: None,
                },
            );
        }
        Err(error) => {
            active_streams().lock().unwrap().remove(&stream_id);
            emit_stream_event(
                &app,
                &stream_id,
                AiStreamEventPayload {
                    event_type: "error".to_string(),
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id),
                    text_delta: None,
                    reasoning_delta: None,
                    message: None,
                    command_cards: vec![],
                    usage: None,
                    error: Some(error.to_string()),
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Agent (ReAct) loop
// ---------------------------------------------------------------------------

const DEFAULT_MAX_AGENT_STEPS: u16 = 10;
const DEFAULT_AGENT_STEP_TIMEOUT_MS: u64 = 30_000;

fn emit_agent_step(app: &AppHandle, stream_id: &str, payload: AgentStepPayload) {
    let _ = app.emit(format!("ai-stream-{stream_id}").as_str(), payload);
}

/// Execute a command out-of-band — never touches the interactive terminal.
///
/// - **SSH**: opens a dedicated exec channel on the existing connection.
/// - **Local**: spawns a child process via the system shell.
/// - Other session types are not supported for agent mode.
async fn execute_command_on_session(
    session_manager: &SessionManager,
    terminal_session_id: &str,
    command: &str,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    let (session_type, ssh_handle, cwd) = {
        let sessions = session_manager.sessions.lock().await;
        let session = sessions.get(terminal_session_id).ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' not found", terminal_session_id))
        })?;
        (
            session.info.session_type.clone(),
            session.ssh_handle.clone(),
            session.cwd.clone(),
        )
    };

    match session_type {
        SessionType::SSH => exec_via_ssh_channel(ssh_handle, cwd, command, timeout_ms).await,
        SessionType::Local => exec_via_subprocess(cwd, command, timeout_ms).await,
        other => Err(AppError::Channel(format!(
            "Agent mode is not supported for {:?} sessions",
            other
        ))),
    }
}

/// SSH: open a separate exec channel so nothing appears in the interactive PTY.
async fn exec_via_ssh_channel(
    ssh_handle: Option<Arc<dyn std::any::Any + Send + Sync>>,
    cwd: crate::core::SharedCwd,
    command: &str,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    let handles = ssh_handle
        .ok_or_else(|| AppError::Config("Not an SSH session".to_string()))?
        .downcast::<SshConnectionHandles>()
        .map_err(|_| AppError::Config("Failed to downcast SSH handle".to_string()))?;
    let handle_mtx = handles.target_handle();

    let mut channel = {
        let handle = handle_mtx.lock().await;
        handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Channel(format!("Failed to open exec channel: {e}")))?
    };

    let full_cmd = match cwd.lock().await.as_deref() {
        Some(dir) if !dir.is_empty() => format!("cd {} && {}", shell_quote(dir), command),
        _ => command.to_string(),
    };

    channel
        .exec(true, full_cmd.as_bytes())
        .await
        .map_err(|e| AppError::Channel(format!("Failed to exec command: {e}")))?;

    let start = std::time::Instant::now();
    let timeout_dur = Duration::from_millis(timeout_ms);
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code: Option<i32> = None;

    loop {
        let remaining = timeout_dur.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, channel.wait()).await {
            Ok(Some(russh::ChannelMsg::Data { ref data })) => {
                stdout.push_str(&String::from_utf8_lossy(data));
            }
            Ok(Some(russh::ChannelMsg::ExtendedData { ref data, .. })) => {
                stderr.push_str(&String::from_utf8_lossy(data));
            }
            Ok(Some(russh::ChannelMsg::ExitStatus { exit_status })) => {
                exit_code = Some(exit_status as i32);
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let output = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    };

    Ok(CommandObservation {
        output,
        exit_code,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Local: spawn a child process so nothing appears in the interactive PTY.
async fn exec_via_subprocess(
    cwd: crate::core::SharedCwd,
    command: &str,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    let working_dir = cwd.lock().await.clone();

    let start = std::time::Instant::now();

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };

    if let Some(ref dir) = working_dir {
        cmd.current_dir(dir);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let child = cmd.spawn().map_err(|e| {
        AppError::Channel(format!("Failed to spawn subprocess: {e}"))
    })?;

    let timeout_dur = Duration::from_millis(timeout_ms);
    match tokio::time::timeout(timeout_dur, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.is_empty() {
                stdout
            } else if stdout.is_empty() {
                stderr
            } else {
                format!("{stdout}\n{stderr}")
            };
            Ok(CommandObservation {
                output: combined,
                exit_code: output.status.code(),
                duration_ms: start.elapsed().as_millis() as u64,
            })
        }
        Ok(Err(e)) => Err(AppError::Channel(format!("Subprocess error: {e}"))),
        Err(_) => Ok(CommandObservation {
            output: "(command timed out)".to_string(),
            exit_code: None,
            duration_ms: start.elapsed().as_millis() as u64,
        }),
    }
}

/// Simple quoting to make a path safe for `cd`.
fn shell_quote(s: &str) -> String {
    if s.contains(' ') || s.contains('\'') || s.contains('"') || s.contains('\\') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

fn is_cancelled(cancel_rx: &mut oneshot::Receiver<()>) -> bool {
    matches!(cancel_rx.try_recv(), Ok(()) | Err(oneshot::error::TryRecvError::Closed))
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_stream(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    stream_id: String,
    session_id: String,
    mut request: AiChatRequest,
    settings: AiSettings,
    mut cancel_rx: oneshot::Receiver<()>,
) {
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
        let _ = save_user_message(&app, &session_id, &request);
    }

    let terminal_session_id = match &request.terminal_session_id {
        Some(id) if !id.trim().is_empty() => id.clone(),
        _ => {
            emit_agent_error(&app, &stream_id, &session_id, "Agent mode requires a terminal session");
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
    let step_timeout = settings.agent_step_timeout_ms.unwrap_or(DEFAULT_AGENT_STEP_TIMEOUT_MS);
    let allowed_risk = &request.options.allowed_command_risk_level;

    let mut conversation = vec![ChatMessage::system(AGENT_SYSTEM_PROMPT)];
    let initial_prompt = build_agent_prompt(&request, &settings);
    conversation.push(ChatMessage::user(initial_prompt));

    let mut final_answer: Option<String> = None;
    let mut all_steps: Vec<AgentStepPayload> = Vec::new();

    for step_index in 0..max_steps {
        if is_cancelled(&mut cancel_rx) {
            emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
            return;
        }

        let client = match build_client(&resolved_model) {
            Ok(c) => c,
            Err(e) => {
                emit_agent_error(&app, &stream_id, &session_id, &e.to_string());
                return;
            }
        };

        let chat_req = ChatRequest::new(conversation.clone());
        let chat_options = ChatOptions::default()
            .with_capture_reasoning_content(true)
            .with_normalize_reasoning_content(true);

        let stream_result = match tokio::time::timeout(
            Duration::from_millis(settings.timeout_ms),
            client.exec_chat_stream(
                &resolved_model.model_name,
                chat_req,
                Some(&chat_options),
            ),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                emit_agent_error(&app, &stream_id, &session_id, &format!("AI request failed: {e}"));
                return;
            }
            Err(_) => {
                emit_agent_error(&app, &stream_id, &session_id, "AI request timed out");
                return;
            }
        };

        let mut raw_output = String::new();
        let mut reasoning_output = String::new();
        let mut stream = stream_result.stream;
        let idle_duration = Duration::from_millis(settings.timeout_ms);
        let idle_deadline = tokio::time::sleep(idle_duration);
        tokio::pin!(idle_deadline);

        loop {
            tokio::select! {
                _ = &mut idle_deadline => break,
                _ = &mut cancel_rx => {
                    emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
                    return;
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
                                emit_stream_event(&app, &stream_id, AiStreamEventPayload {
                                    event_type: "reasoning_delta".to_string(),
                                    stream_id: stream_id.clone(),
                                    session_id: Some(session_id.clone()),
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
                            if reasoning_output.is_empty() {
                                if let Some(r) = end.captured_reasoning_content {
                                    reasoning_output = r;
                                }
                            }
                            break;
                        }
                        None => break,
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            emit_agent_error(&app, &stream_id, &session_id, &format!("AI stream failed: {e}"));
                            return;
                        }
                    }
                }
            }
        }

        let candidate = extract_json_object(&raw_output)
            .unwrap_or_else(|| raw_output.trim().to_string());

        let parsed: AgentLlmResponse = match serde_json::from_str(&candidate) {
            Ok(r) => r,
            Err(_) => {
                let (text, _, _) = parse_model_output(&raw_output, trim_string_to_option(reasoning_output));
                final_answer = Some(text);
                break;
            }
        };

        conversation.push(ChatMessage::assistant(&raw_output));

        match parsed.action.as_str() {
            "final_answer" => {
                let answer = parsed.answer.unwrap_or_default();
                let step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought,
                    action: AgentStepAction {
                        kind: AgentActionKind::FinalAnswer,
                        command: None,
                        risk_level: None,
                        answer: Some(answer.clone()),
                    },
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
                        emit_agent_error(&app, &stream_id, &session_id, "Agent returned execute_command without a command");
                        return;
                    }
                };

                let risk = check_command_risk(CommandRiskRequest {
                    command: command.clone(),
                    context: request.context.clone(),
                });

                if risk.blocked {
                    let step = AgentStepPayload {
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id.clone()),
                        step_index,
                        thought: parsed.thought.clone(),
                        action: AgentStepAction {
                            kind: AgentActionKind::ExecuteCommand,
                            command: Some(command.clone()),
                            risk_level: Some(risk.risk_level.clone()),
                            answer: None,
                        },
                        observation: None,
                        status: AgentStepStatus::Failed,
                        error: Some(format!("命令被安全策略阻止：{}", risk.reason)),
                    };
                    emit_agent_step(&app, &stream_id, step.clone());
                    all_steps.push(step);

                    let blocked_msg = format!(
                        "命令 `{}` 被安全策略阻止：{}。安全替代方案：{}。请换用安全命令或给出 final_answer。",
                        command,
                        risk.reason,
                        if risk.safe_alternatives.is_empty() {
                            "无".to_string()
                        } else {
                            risk.safe_alternatives.join(", ")
                        }
                    );
                    conversation.push(ChatMessage::user(blocked_msg));
                    continue;
                }

                let needs_approval = !is_risk_allowed(&risk.risk_level, allowed_risk);

                if needs_approval {
                    let step = AgentStepPayload {
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id.clone()),
                        step_index,
                        thought: parsed.thought.clone(),
                        action: AgentStepAction {
                            kind: AgentActionKind::ExecuteCommand,
                            command: Some(command.clone()),
                            risk_level: Some(risk.risk_level.clone()),
                            answer: None,
                        },
                        observation: None,
                        status: AgentStepStatus::NeedsApproval,
                        error: Some(risk.reason.clone()),
                    };
                    emit_agent_step(&app, &stream_id, step.clone());
                    all_steps.push(step);

                    let skipped_msg = format!(
                        "命令 `{}` 风险等级为 {:?}，超出自动执行阈值，已跳过。请换用更安全的命令或给出 final_answer。",
                        command, risk.risk_level
                    );
                    conversation.push(ChatMessage::user(skipped_msg));
                    continue;
                }

                let step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought.clone(),
                    action: AgentStepAction {
                        kind: AgentActionKind::ExecuteCommand,
                        command: Some(command.clone()),
                        risk_level: Some(risk.risk_level),
                        answer: None,
                    },
                    observation: None,
                    status: AgentStepStatus::Running,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, step);

                let obs = match execute_command_on_session(
                    &session_manager,
                    &terminal_session_id,
                    &command,
                    step_timeout,
                )
                .await
                {
                    Ok(obs) => obs,
                    Err(e) => {
                        let step = AgentStepPayload {
                            stream_id: stream_id.clone(),
                            session_id: Some(session_id.clone()),
                            step_index,
                            thought: parsed.thought,
                            action: AgentStepAction {
                                kind: AgentActionKind::ExecuteCommand,
                                command: Some(command.clone()),
                                risk_level: None,
                                answer: None,
                            },
                            observation: None,
                            status: AgentStepStatus::Failed,
                            error: Some(e.to_string()),
                        };
                        emit_agent_step(&app, &stream_id, step.clone());
                        all_steps.push(step);

                        let err_msg = format!("命令执行失败：{}。请分析原因并给出下一步。", e);
                        conversation.push(ChatMessage::user(err_msg));
                        continue;
                    }
                };

                let completed_step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought,
                    action: AgentStepAction {
                        kind: AgentActionKind::ExecuteCommand,
                        command: Some(command.clone()),
                        risk_level: None,
                        answer: None,
                    },
                    observation: Some(obs.clone()),
                    status: AgentStepStatus::Completed,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, completed_step.clone());
                all_steps.push(completed_step);

                let obs_msg = build_observation_message(&obs, &command);
                conversation.push(ChatMessage::user(obs_msg));
            }
            other => {
                let fallback = format!(
                    "Unknown action '{}'. Treating as final answer. {}",
                    other,
                    parsed.answer.as_deref().unwrap_or(&parsed.thought)
                );
                final_answer = Some(fallback);
                break;
            }
        }
    }

    active_streams().lock().unwrap().remove(&stream_id);

    let answer_text = final_answer.unwrap_or_else(|| {
        "Agent 已达到最大步数限制，任务可能未完成。".to_string()
    });

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
        let _ = append_message(&app, message.clone());
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

fn emit_agent_error(app: &AppHandle, stream_id: &str, session_id: &str, error: &str) {
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

fn is_risk_allowed(risk: &RiskLevel, allowed: &RiskLevel) -> bool {
    risk_rank(risk) <= risk_rank(allowed)
}

fn risk_rank(level: &RiskLevel) -> u8 {
    match level {
        RiskLevel::Low => 1,
        RiskLevel::Medium => 2,
        RiskLevel::High => 3,
        RiskLevel::Critical => 4,
    }
}

// ---------------------------------------------------------------------------
// Ask mode model stream
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct AiStreamResult {
    text: String,
    reasoning_content: Option<String>,
}

async fn run_model_stream(
    app: &AppHandle,
    stream_id: &str,
    request: &AiChatRequest,
    settings: &AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<AiStreamResult> {
    let resolved_model = resolve_request_model(settings, request)?;
    let client = build_client(&resolved_model)?;
    let prompt = build_prompt(request, settings);

    let mut messages = vec![ChatMessage::system(SYSTEM_PROMPT)];

    if let Some(session_id) = &request.session_id {
        let max_turns = request.options.history_turns as usize;
        if max_turns > 0 {
            if let Ok(history) = load_history(app) {
                let history_msgs: Vec<&AiMessage> = history
                    .messages
                    .iter()
                    .filter(|m| m.session_id == *session_id)
                    .collect();
                let skip = history_msgs.len().saturating_sub(max_turns);
                for msg in history_msgs.into_iter().skip(skip) {
                    match msg.role {
                        AiMessageRole::User => {
                            messages.push(ChatMessage::user(&msg.content));
                        }
                        AiMessageRole::Assistant => {
                            let content = extract_text_from_assistant(&msg.content);
                            if !content.is_empty() {
                                messages.push(ChatMessage::assistant(&content));
                            }
                        }
                        AiMessageRole::System => {}
                    }
                }
            }
        }
    }

    messages.push(ChatMessage::user(prompt));

    let chat_req = ChatRequest::new(messages);
    let chat_options = ChatOptions::default()
        .with_capture_reasoning_content(true)
        .with_normalize_reasoning_content(true);

    let stream_result = tokio::time::timeout(
        Duration::from_millis(settings.timeout_ms),
        client.exec_chat_stream(&resolved_model.model_name, chat_req, Some(&chat_options)),
    )
    .await
    .map_err(|_| AppError::Config("AI request timed out".to_string()))?
    .map_err(|error| AppError::Config(format!("AI request failed: {error}")))?;

    let mut stream = stream_result.stream;
    let mut output = String::new();
    let mut reasoning_output = String::new();
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
                    Some(Ok(ChatStreamEvent::Chunk(chunk))) => {
                        let text_delta = chunk.content;
                        if !text_delta.is_empty() {
                            output.push_str(&text_delta);
                            emit_stream_event(app, stream_id, AiStreamEventPayload {
                                event_type: "delta".to_string(),
                                stream_id: stream_id.to_string(),
                                session_id: request.session_id.clone(),
                                text_delta: Some(text_delta),
                                reasoning_delta: None,
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::ReasoningChunk(chunk))) => {
                        let reasoning_delta = chunk.content;
                        if !reasoning_delta.is_empty() {
                            reasoning_output.push_str(&reasoning_delta);
                            emit_stream_event(app, stream_id, AiStreamEventPayload {
                                event_type: "reasoning_delta".to_string(),
                                stream_id: stream_id.to_string(),
                                session_id: request.session_id.clone(),
                                text_delta: None,
                                reasoning_delta: Some(reasoning_delta),
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::End(end))) => {
                        if reasoning_output.is_empty() {
                            if let Some(captured_reasoning_content) = end.captured_reasoning_content {
                                reasoning_output = captured_reasoning_content;
                            }
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

    Ok(AiStreamResult {
        text: output,
        reasoning_content: trim_string_to_option(reasoning_output),
    })
}

#[derive(Debug, Clone)]
struct ResolvedAiModel {
    model_name: String,
    provider_kind: AiProviderKind,
    credential: Option<AiProviderCredential>,
}

fn resolve_request_model(
    settings: &AiSettings,
    request: &AiChatRequest,
) -> AppResult<ResolvedAiModel> {
    let selected_model = request
        .model_id
        .as_deref()
        .and_then(|id| {
            settings
                .models
                .iter()
                .find(|model| model.enabled && model.id == id)
        })
        .or_else(|| {
            settings.default_model_id.as_deref().and_then(|id| {
                settings
                    .models
                    .iter()
                    .find(|model| model.enabled && model.id == id)
            })
        })
        .or_else(|| settings.models.iter().find(|model| model.enabled))
        .ok_or_else(|| AppError::Config("No enabled AI model configured".to_string()))?;

    let model_provider_kind = selected_model
        .provider_kind
        .clone()
        .or_else(|| infer_provider_kind_from_model_id(&selected_model.id));

    let credential =
        resolve_model_credential(settings, selected_model, model_provider_kind.as_ref())?;
    let provider_kind = credential
        .as_ref()
        .map(|credential| credential.provider_kind.clone())
        .or(model_provider_kind)
        .ok_or_else(|| {
            AppError::Config(format!(
                "AI model '{}' is missing provider information",
                selected_model.name
            ))
        })?;
    validate_model_credential(&provider_kind, credential.as_ref())?;

    Ok(ResolvedAiModel {
        model_name: selected_model.name.clone(),
        provider_kind,
        credential,
    })
}

fn infer_provider_kind_from_model_id(model_id: &str) -> Option<AiProviderKind> {
    let (prefix, _) = model_id.split_once(':')?;
    match prefix {
        "openai" => Some(AiProviderKind::Openai),
        "anthropic" => Some(AiProviderKind::Anthropic),
        "gemini" => Some(AiProviderKind::Gemini),
        "deepseek" => Some(AiProviderKind::Deepseek),
        "groq" => Some(AiProviderKind::Groq),
        "ollama" => Some(AiProviderKind::Ollama),
        "xai" => Some(AiProviderKind::Xai),
        "cohere" => Some(AiProviderKind::Cohere),
        "mimo" => Some(AiProviderKind::Mimo),
        "zai" => Some(AiProviderKind::Zai),
        "openai_compatible" => Some(AiProviderKind::OpenaiCompatible),
        _ => None,
    }
}

fn resolve_model_credential(
    settings: &AiSettings,
    model: &AiModelConfigItem,
    provider_kind: Option<&AiProviderKind>,
) -> AppResult<Option<AiProviderCredential>> {
    if let Some(credential_id) = model.credential_id.as_deref() {
        let credential = settings
            .provider_credentials
            .iter()
            .find(|item| item.id == credential_id && item.enabled)
            .cloned()
            .ok_or_else(|| {
                AppError::Config(format!(
                    "No enabled AI credential found for model '{}'",
                    model.name
                ))
            })?;
        return Ok(Some(credential));
    }

    Ok(provider_kind.and_then(|provider_kind| {
        settings
            .provider_credentials
            .iter()
            .find(|item| item.enabled && &item.provider_kind == provider_kind)
            .cloned()
    }))
}

fn validate_model_credential(
    provider_kind: &AiProviderKind,
    credential: Option<&AiProviderCredential>,
) -> AppResult<()> {
    match provider_kind {
        AiProviderKind::Ollama => Ok(()),
        AiProviderKind::OpenaiCompatible => {
            if credential.is_none() {
                return Err(AppError::Config(
                    "No enabled OpenAI-compatible AI credential configured".to_string(),
                ));
            }
            Ok(())
        }
        _ => {
            let credential = credential.ok_or_else(|| {
                AppError::Config(format!(
                    "No enabled AI credential configured for {:?}",
                    provider_kind
                ))
            })?;
            if credential
                .api_key
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(AppError::Config(format!(
                    "No API key configured for AI credential '{}'",
                    credential.name
                )));
            }
            Ok(())
        }
    }
}

fn build_client(model: &ResolvedAiModel) -> AppResult<Client> {
    let adapter_kind = adapter_kind(&model.provider_kind);
    let mapped_model = model.model_name.clone();
    let api_key = model
        .credential
        .as_ref()
        .and_then(|credential| credential.api_key.clone())
        .filter(|value| !value.trim().is_empty());
    let base_url = model
        .credential
        .as_ref()
        .and_then(|credential| credential.base_url.clone())
        .filter(|value| !value.trim().is_empty());

    let resolver =
        ServiceTargetResolver::from_resolver_fn(move |service_target: genai::ServiceTarget| {
            let mut service_target = service_target;
            if let Some(api_key) = api_key.clone() {
                service_target.auth = AuthData::from_single(api_key);
            }
            if let Some(base_url) = base_url.clone() {
                service_target.endpoint = Endpoint::from_owned(base_url);
            }
            Ok(service_target)
        });

    Ok(Client::builder()
        .with_model_mapper_fn(move |_model| Ok(ModelIden::new(adapter_kind, mapped_model.clone())))
        .with_service_target_resolver(resolver)
        .build())
}

fn adapter_kind(kind: &AiProviderKind) -> AdapterKind {
    match kind {
        AiProviderKind::Openai | AiProviderKind::OpenaiCompatible => AdapterKind::OpenAI,
        AiProviderKind::Anthropic => AdapterKind::Anthropic,
        AiProviderKind::Gemini => AdapterKind::Gemini,
        AiProviderKind::Deepseek => AdapterKind::DeepSeek,
        AiProviderKind::Groq => AdapterKind::Groq,
        AiProviderKind::Ollama => AdapterKind::Ollama,
        AiProviderKind::Xai
        | AiProviderKind::Cohere
        | AiProviderKind::Mimo
        | AiProviderKind::Zai => AdapterKind::OpenAI,
    }
}

pub async fn list_model_names(app: &AppHandle) -> AppResult<Vec<AiModelDiscovery>> {
    let settings = config::load_app_settings(app)?;

    let custom_credentials: Vec<_> = settings
        .ai
        .provider_credentials
        .iter()
        .filter(|c| c.enabled && c.provider_kind == AiProviderKind::OpenaiCompatible)
        .collect();

    let mut models = BTreeMap::new();
    let mut errors = Vec::new();

    for credential in &custom_credentials {
        let base_url = credential
            .base_url
            .as_deref()
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        let api_key = credential
            .api_key
            .clone()
            .filter(|v| !v.trim().is_empty());
        let label = credential.name.as_str();
        tracing::info!(credential = label, url = base_url, "Fetching model list from custom provider");
        match fetch_openai_compatible_models(&base_url, api_key.as_deref()).await {
            Ok(names) => {
                tracing::info!(
                    credential = label,
                    count = names.len(),
                    models = ?names,
                    "Fetched models from custom provider"
                );
                for name in names {
                    let trimmed = name.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let id = ai_model_id_for_credential(&credential.id, trimmed);
                    models.entry(id.clone()).or_insert(AiModelDiscovery {
                        id,
                        name: trimmed.to_string(),
                        provider_kind: Some(AiProviderKind::OpenaiCompatible),
                        credential_id: Some(credential.id.clone()),
                        source: AiModelSource::RustGenai,
                    });
                }
            }
            Err(error) => {
                tracing::warn!(credential = label, %error, "Failed to fetch models from custom provider");
                errors.push(format!("{label}: {error}"));
            }
        }
    }

    if models.is_empty() && !errors.is_empty() {
        return Err(AppError::Config(format!(
            "Failed to list AI models: {}",
            errors.join("; ")
        )));
    }

    Ok(models.into_values().collect())
}

/// Fetches model names from an OpenAI-compatible `/v1/models` endpoint directly via HTTP,
/// bypassing `genai::Client::all_model_names` which does not apply the `ServiceTargetResolver`
/// (and therefore ignores custom auth/endpoint configuration).
async fn fetch_openai_compatible_models(
    base_url: &str,
    api_key: Option<&str>,
) -> AppResult<Vec<String>> {
    let url = format!("{base_url}/models");
    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let resp = req
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| AppError::Config(format!("Failed to fetch models from {url}: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Config(format!(
            "Failed to fetch models from {url}: {status} {body}"
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Config(format!("Invalid JSON from {url}: {e}")))?;
    let names: Vec<String> = body["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(names)
}

const SYSTEM_PROMPT: &str = r#"你是一个专业、谨慎、安全优先的 Linux / DevOps / 云原生终端助手。
你的任务是帮助用户解释终端输出、生成 Shell 命令、分析错误、提供排查步骤。

必须遵守：
1. 不要建议不可逆高危操作，除非明确说明风险和安全替代方案。
2. 默认生成只读诊断命令。
3. 对任何删除、格式化、重启、停服务、改权限、批量变更命令标记风险。
4. 命令必须适配用户当前系统、架构、shell 和权限上下文。
5. 输出必须结构化，包含命令、说明、风险等级、影响范围和回滚建议。
6. 不要编造当前系统不存在的信息；不确定时给出验证命令。
7. 不要要求用户粘贴密码、私钥、token。

只返回一个 JSON 对象，不要使用 Markdown 代码块。格式：
{
  "text": "给用户看的说明",
  "commandCards": [
    {
      "id": "cmd-uuid",
      "title": "标题",
      "command": "shell command",
      "explanation": "命令说明",
      "riskLevel": "low|medium|high|critical",
      "riskReason": "风险原因",
      "expectedEffect": "预计影响",
      "rollback": "回滚方式或无需回滚",
      "category": "Linux 性能"
    }
  ]
}"#;

const AGENT_SYSTEM_PROMPT: &str = r#"你是一个终端自动化 Agent，通过"思考—执行—观察"循环完成用户的任务。

每一轮你只能做一件事：执行一条命令或给出最终回答。

规则：
1. 每轮只返回一个 JSON 对象，不要使用 Markdown。
2. 如果需要执行命令，返回 action 为 "execute_command"。
3. 任务完成或无需执行命令时，返回 action 为 "final_answer"。
4. 优先使用只读命令收集信息，再做修改操作。
5. 不要执行不可逆高危命令（如 rm -rf /、mkfs、停止 SSH 等），改为在 thought 中说明风险并给出 final_answer。
6. 不要编造信息；不确定时先用验证命令确认。
7. 不要要求用户提供密码、私钥、token。
8. 命令必须适配用户当前的系统和 shell 环境。
9. riskLevel 规则：只读命令 → low，普通写操作 → medium，删除/重启/权限修改 → high，不可逆破坏 → critical。

执行命令的 JSON 格式：
{
  "thought": "分析当前状态和下一步计划",
  "action": "execute_command",
  "command": "要执行的单条 shell 命令",
  "riskLevel": "low"
}

给出最终回答的 JSON 格式：
{
  "thought": "任务完成的原因",
  "action": "final_answer",
  "answer": "向用户展示的总结"
}"#;

fn build_agent_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let ctx = &request.context;
    format!(
        r#"用户任务：
{user_input}

当前连接上下文：
- 连接名：{connection_name}
- 主机：{host}
- 用户：{username}
- 当前目录：{cwd}
- 操作系统：{os}
- 架构：{arch}

最近终端输出（最多 {line_limit} 行）：
{recent_output}

请开始执行任务。每轮只返回一个 JSON 对象。"#,
        user_input = request.user_input,
        connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
        host = ctx.host.as_deref().unwrap_or("-"),
        username = ctx.username.as_deref().unwrap_or("-"),
        cwd = ctx.cwd.as_deref().unwrap_or("-"),
        os = ctx.os.as_deref().unwrap_or("-"),
        arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
        line_limit = settings.context_line_limit,
        recent_output = ctx.recent_output,
    )
}

fn build_observation_message(obs: &CommandObservation, command: &str) -> String {
    let status = obs
        .exit_code
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "unknown exit code".to_string());
    let output = if obs.output.len() > 8000 {
        let truncated = &obs.output[obs.output.len() - 8000..];
        format!("...(truncated)\n{truncated}")
    } else {
        obs.output.clone()
    };
    format!(
        "命令 `{command}` 执行完成（{status}，耗时 {duration}ms）。\n\n输出：\n{output}\n\n请根据观察结果决定下一步。只返回 JSON 对象。",
        duration = obs.duration_ms,
    )
}

fn build_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let action = match request.action {
        AiAction::GenerateCommand => "根据自然语言需求生成 1 到 2 条 Shell 命令",
        AiAction::ExplainOutput => "解释最近终端输出并给出下一步建议",
        AiAction::ExplainSelected => "解释用户选中的终端文本并给出下一步建议",
        AiAction::AnalyzeError => "分析终端错误输出并给出排查步骤",
        AiAction::RepairFromSelection => "根据选中内容生成修复或排查命令",
        AiAction::CustomTerminalAction => "根据用户配置的终端 AI 功能处理选中内容",
        AiAction::CustomFileAction => "根据用户配置的文件 AI 功能处理文件内容",
    };
    let ctx = &request.context;
    format!(
        r#"任务：{action}
用户需求：
{user_input}

当前连接上下文：
- 连接名：{connection_name}
- 主机：{host}
- 端口：{port}
- 用户：{username}
- 当前目录：{cwd}
- 操作系统：{os}
- 架构：{arch}
- 当前输入：{input_buffer}

选中文本：
{selected_text}

最近终端输出（最多 {line_limit} 行）：
{recent_output}

要求：
- 语言：{language}
- 安全模式：{safety_mode}
- 最多生成 {max_commands} 条命令
- 优先生成只读诊断命令
- 如果信息不足，请给出验证命令
- 必须返回 JSON 对象，不要返回 Markdown"#,
        user_input = request.user_input,
        connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
        host = ctx.host.as_deref().unwrap_or("-"),
        port = ctx
            .port
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        username = ctx.username.as_deref().unwrap_or("-"),
        cwd = ctx.cwd.as_deref().unwrap_or("-"),
        os = ctx.os.as_deref().unwrap_or("-"),
        arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
        input_buffer = ctx.input_buffer,
        selected_text = ctx.selected_text,
        line_limit = settings.context_line_limit,
        recent_output = ctx.recent_output,
        language = request.options.language,
        safety_mode = request.options.safety_mode,
        max_commands = request.options.max_output_commands,
    )
}

fn extract_text_from_assistant(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(json_str) = extract_json_object(trimmed) {
        if let Ok(output) = serde_json::from_str::<AiModelOutput>(&json_str) {
            if !output.text.trim().is_empty() {
                return output.text;
            }
        }
    }
    trimmed.to_string()
}

fn parse_model_output(
    raw_text: &str,
    stream_reasoning: Option<String>,
) -> (String, Option<String>, Vec<AiCommandCard>) {
    let candidate = extract_json_object(raw_text).unwrap_or_else(|| raw_text.trim().to_string());
    match serde_json::from_str::<AiModelOutput>(&candidate) {
        Ok(output) => {
            let text = if output.text.trim().is_empty() {
                raw_text.trim().to_string()
            } else {
                output.text
            };
            let reasoning_content = trim_optional_to_option(output.reasoning)
                .or_else(|| trim_optional_to_option(stream_reasoning));
            let (text, extracted_reasoning) = extract_think_block(&text);
            (
                text,
                extracted_reasoning.or(reasoning_content),
                output.command_cards,
            )
        }
        Err(_) => {
            let normalized_reasoning = trim_optional_to_option(stream_reasoning);
            let (text, extracted_reasoning) = extract_think_block(raw_text);
            (text, extracted_reasoning.or(normalized_reasoning), vec![])
        }
    }
}

fn extract_json_object(raw_text: &str) -> Option<String> {
    let trimmed = raw_text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    Some(trimmed[start..=end].to_string())
}

fn extract_think_block(raw_text: &str) -> (String, Option<String>) {
    static THINK_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = THINK_REGEX.get_or_init(|| Regex::new(r"(?is)<think>(.*?)</think>").unwrap());

    let mut reasoning_parts = Vec::new();
    for captures in regex.captures_iter(raw_text) {
        if let Some(value) = captures.get(1) {
            let reasoning = value.as_str().trim();
            if !reasoning.is_empty() {
                reasoning_parts.push(reasoning.to_string());
            }
        }
    }

    let visible_text = regex.replace_all(raw_text, "").to_string();
    (
        visible_text.trim().to_string(),
        trim_string_to_option(reasoning_parts.join("\n\n")),
    )
}

fn trim_string_to_option(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn trim_optional_to_option(value: Option<String>) -> Option<String> {
    value.and_then(trim_string_to_option)
}

fn redact_context(context: &mut AiContext) {
    context.recent_output = redact_sensitive_text(&context.recent_output);
    context.selected_text = redact_sensitive_text(&context.selected_text);
    context.input_buffer = redact_sensitive_text(&context.input_buffer);
}

pub fn redact_sensitive_text(input: &str) -> String {
    let mut output = input.to_string();
    for (pattern, replacement) in redaction_patterns() {
        output = pattern.replace_all(&output, *replacement).to_string();
    }
    output
}

fn redaction_patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                Regex::new(
                    r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
                )
                .unwrap(),
                "[REDACTED_PRIVATE_KEY]",
            ),
            (
                Regex::new(r"(?i)Authorization:\s*Bearer\s+[A-Za-z0-9._\-]+").unwrap(),
                "Authorization: Bearer [REDACTED]",
            ),
            (
                Regex::new(r"(?i)(password|passwd|pwd)\s*[:=]\s*[^\s;&|]+").unwrap(),
                "$1=[REDACTED]",
            ),
            (
                Regex::new(
                    r"(?i)(token|api[_-]?key|secret[_-]?key|access[_-]?key)\s*[:=]\s*[^\s;&|]+",
                )
                .unwrap(),
                "$1=[REDACTED]",
            ),
            (
                Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
                "[REDACTED_AWS_ACCESS_KEY]",
            ),
            (
                Regex::new(r"(?i)(postgres|mysql|mongodb)://[^@\s]+@").unwrap(),
                "$1://[REDACTED]@",
            ),
        ]
    })
}

pub fn check_command_risk(request: CommandRiskRequest) -> CommandRiskResponse {
    let command = request.command.trim();
    let mut response = classify_command(command);
    let username = request.context.username.as_deref().unwrap_or_default();

    if username == "root" && is_root_sensitive_command(command) {
        response.risk_level = bump_risk(&response.risk_level);
        if response.reason == "未发现明显高危操作。" {
            response.reason = "root 用户下执行删除、权限或服务变更命令，影响范围更大。".to_string();
        } else if !response.reason.contains("root") {
            response.reason = format!("{} root 用户下风险上调。", response.reason);
        }
    }

    response
}

fn classify_command(command: &str) -> CommandRiskResponse {
    if command.is_empty() {
        return CommandRiskResponse {
            risk_level: RiskLevel::Low,
            blocked: false,
            reason: "空命令。".to_string(),
            safe_alternatives: vec![],
            confirm_text: None,
        };
    }

    for pattern in risk_patterns() {
        if pattern.regex.is_match(command) {
            return CommandRiskResponse {
                risk_level: pattern.level.clone(),
                blocked: pattern.blocked,
                reason: pattern.reason.to_string(),
                safe_alternatives: pattern
                    .alternatives
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                confirm_text: pattern.confirm_text.map(str::to_string),
            };
        }
    }

    let lower = command.to_ascii_lowercase();
    let level = if is_read_only_command(&lower) {
        RiskLevel::Low
    } else if lower.contains(" rm ")
        || lower.starts_with("rm ")
        || lower.contains(" chmod ")
        || lower.starts_with("chmod ")
        || lower.contains(" chown ")
        || lower.starts_with("chown ")
        || lower.contains(" mv ")
        || lower.starts_with("mv ")
        || lower.contains(" > ")
        || lower.contains(" tee ")
        || lower.contains(" systemctl restart ")
        || lower.starts_with("systemctl restart ")
    {
        RiskLevel::Medium
    } else {
        RiskLevel::Medium
    };

    CommandRiskResponse {
        risk_level: level,
        blocked: false,
        reason: "未发现明显高危操作。".to_string(),
        safe_alternatives: vec![],
        confirm_text: None,
    }
}

fn risk_patterns() -> &'static [RiskPattern] {
    static PATTERNS: OnceLock<Vec<RiskPattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            RiskPattern {
                regex: Regex::new(r"(?i)(^|[;&|]\s*)rm\s+-[^\n]*r[^\n]*f[^\n]*\s+(/|/\*|--no-preserve-root\s+/)(\s|$)").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "该命令可能递归删除根目录或根目录下的大量文件，风险不可恢复。",
                alternatives: &["ls -lah /", "find / -maxdepth 1 -mindepth 1 -print | head -n 50"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bmkfs\.[a-z0-9]+\s+/dev/\S+").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "该命令会格式化磁盘或分区，可能导致数据不可恢复。",
                alternatives: &["lsblk -f", "blkid"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bdd\s+.+\bof=/dev/(sd|vd|xvd|hd|nvme)\S+").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "该 dd 命令会直接写入块设备，可能破坏磁盘数据。",
                alternatives: &["lsblk -f", "df -hT"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bsystemctl\s+stop\s+(ssh|sshd)\b").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "停止 SSH 服务可能导致当前远程连接断开并无法重新登录。",
                alternatives: &["systemctl status ssh --no-pager", "systemctl status sshd --no-pager"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\b(iptables\s+-F|ufw\s+disable)\b").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "清空防火墙规则或关闭防火墙可能暴露服务或切断访问策略。",
                alternatives: &["iptables -S", "ufw status verbose"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\b(shutdown|poweroff|halt)\b|\breboot\b").unwrap(),
                level: RiskLevel::High,
                blocked: false,
                reason: "该命令会重启或关闭系统，可能中断业务和当前连接。",
                alternatives: &["uptime", "who", "systemctl list-jobs"],
                confirm_text: Some("我确认要重启或关闭系统"),
            },
            RiskPattern {
                regex: Regex::new(r"(?i)(^|[;&|]\s*)rm\s+-[^\n]*r[^\n]*f[^\n]*\s+\S*[*?]\S*").unwrap(),
                level: RiskLevel::High,
                blocked: false,
                reason: "该命令会递归强制删除匹配路径，可能不可恢复。",
                alternatives: &["ls -lah", "find . -maxdepth 1 -print | head -n 50"],
                confirm_text: Some("我确认要删除这些文件"),
            },
            RiskPattern {
                regex: Regex::new(r"(?i)(^|[;&|]\s*)rm\s+-[^\n]*r[^\n]*f[^\n]*\s+/[^;&|]+").unwrap(),
                level: RiskLevel::High,
                blocked: false,
                reason: "该命令会递归强制删除绝对路径下的内容，可能不可恢复。",
                alternatives: &["ls -lah <target>", "find <target> -maxdepth 1 -print | head -n 50"],
                confirm_text: Some("我确认要删除目标路径"),
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bchmod\s+-R\s+777\s+/").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "递归修改根目录权限会破坏系统安全和可用性。",
                alternatives: &["stat /", "namei -l <path>"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bchown\s+-R\s+\S+\s+/").unwrap(),
                level: RiskLevel::Critical,
                blocked: true,
                reason: "递归修改根目录属主会破坏系统文件权限。",
                alternatives: &["stat /", "namei -l <path>"],
                confirm_text: None,
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bdocker\s+system\s+prune\b.*\s-a\b").unwrap(),
                level: RiskLevel::High,
                blocked: false,
                reason: "该命令会删除未使用镜像、容器、网络和缓存，可能影响回滚能力。",
                alternatives: &["docker system df", "docker ps -a", "docker images"],
                confirm_text: Some("我确认要清理 Docker 资源"),
            },
            RiskPattern {
                regex: Regex::new(r"(?i)\bkubectl\s+delete\s+(namespace|ns)\b").unwrap(),
                level: RiskLevel::High,
                blocked: false,
                reason: "删除 Kubernetes namespace 会删除其中的大量资源。",
                alternatives: &["kubectl get ns", "kubectl get all -n <namespace>"],
                confirm_text: Some("我确认要删除 Kubernetes 命名空间"),
            },
        ]
    })
}

fn is_read_only_command(lower: &str) -> bool {
    let mut parts = lower.split_whitespace();
    let first = match parts.next().unwrap_or_default() {
        "sudo" => parts.next().unwrap_or_default(),
        other => other,
    };
    matches!(
        first,
        "ls" | "pwd"
            | "cat"
            | "tail"
            | "head"
            | "less"
            | "more"
            | "grep"
            | "rg"
            | "find"
            | "ps"
            | "top"
            | "htop"
            | "free"
            | "df"
            | "du"
            | "uptime"
            | "who"
            | "w"
            | "id"
            | "uname"
            | "hostname"
            | "hostnamectl"
            | "ip"
            | "ss"
            | "netstat"
            | "curl"
            | "journalctl"
            | "systemctl"
            | "docker"
            | "kubectl"
            | "git"
    )
}

fn is_root_sensitive_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("rm ")
        || lower.starts_with("rm ")
        || lower.contains("chmod ")
        || lower.starts_with("chmod ")
        || lower.contains("chown ")
        || lower.starts_with("chown ")
        || lower.contains("systemctl ")
        || lower.starts_with("systemctl ")
}

fn bump_risk(level: &RiskLevel) -> RiskLevel {
    match level {
        RiskLevel::Low => RiskLevel::Medium,
        RiskLevel::Medium => RiskLevel::High,
        RiskLevel::High | RiskLevel::Critical => RiskLevel::Critical,
    }
}

fn load_history(_app: &AppHandle) -> AppResult<AiHistoryFile> {
    crate::storage::load_json_doc(crate::storage::JSON_AI_HISTORY)
}

fn trim_history(history: &mut AiHistoryFile) {
    history
        .sessions
        .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    if history.sessions.len() > AI_HISTORY_MAX_SESSIONS {
        history.sessions.truncate(AI_HISTORY_MAX_SESSIONS);
    }

    let retained_sessions: HashSet<&str> = history
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    history
        .messages
        .retain(|message| retained_sessions.contains(message.session_id.as_str()));

    if history.messages.len() > AI_HISTORY_MAX_MESSAGES {
        history
            .messages
            .sort_by(|left, right| left.created_at.cmp(&right.created_at));
        let remove_count = history.messages.len() - AI_HISTORY_MAX_MESSAGES;
        history.messages.drain(0..remove_count);
    }

    let sessions_with_messages: HashSet<&str> = history
        .messages
        .iter()
        .map(|message| message.session_id.as_str())
        .collect();
    history
        .sessions
        .retain(|session| sessions_with_messages.contains(session.id.as_str()));
}

fn save_user_message(app: &AppHandle, session_id: &str, request: &AiChatRequest) -> AppResult<()> {
    let now = now_rfc3339();
    let title = request
        .user_input
        .chars()
        .take(42)
        .collect::<String>()
        .trim()
        .to_string();
    let connection_id = request.connection_id.clone();
    let user_input = request.user_input.clone();
    let session_id = session_id.to_string();

    let _ = app;
    crate::storage::update_json_doc::<AiHistoryFile, _, _>(
        crate::storage::JSON_AI_HISTORY,
        |history| {
            if let Some(session) = history
                .sessions
                .iter_mut()
                .find(|item| item.id == session_id)
            {
                session.updated_at = now.clone();
            } else {
                history.sessions.push(AiSession {
                    id: session_id.clone(),
                    connection_id,
                    title: if title.is_empty() {
                        "AI Session".to_string()
                    } else {
                        title
                    },
                    created_at: now.clone(),
                    updated_at: now.clone(),
                });
            }
            history.messages.push(AiMessage {
                id: format!("msg-{}", uuid()),
                session_id,
                role: AiMessageRole::User,
                content: user_input,
                created_at: now,
                reasoning_content: None,
                command_cards: vec![],
            });
            trim_history(history);
            Ok(())
        },
    )
}

fn append_message(app: &AppHandle, message: AiMessage) -> AppResult<()> {
    let _ = app;
    crate::storage::update_json_doc::<AiHistoryFile, _, _>(
        crate::storage::JSON_AI_HISTORY,
        |history| {
            if let Some(session) = history
                .sessions
                .iter_mut()
                .find(|item| item.id == message.session_id)
            {
                session.updated_at = message.created_at.clone();
            }
            history.messages.push(message);
            trim_history(history);
            Ok(())
        },
    )
}

pub fn get_ai_sessions(app: &AppHandle) -> AppResult<Vec<AiSession>> {
    let mut sessions = load_history(app)?.sessions;
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(sessions)
}

pub fn get_ai_messages(app: &AppHandle, session_id: String) -> AppResult<Vec<AiMessage>> {
    Ok(load_history(app)?
        .messages
        .into_iter()
        .filter(|message| message.session_id == session_id)
        .collect())
}

pub fn clear_ai_history(app: &AppHandle) -> AppResult<()> {
    let _ = app;
    cancel_all_chat_streams();
    crate::storage::save_json_doc(crate::storage::JSON_AI_HISTORY, &AiHistoryFile::default())
}

pub fn append_ai_audit(app: &AppHandle, request: AppendAiAuditRequest) -> AppResult<AiAuditLog> {
    let _ = app;
    let log = AiAuditLog {
        id: format!("audit-{}", uuid()),
        connection_id: request.connection_id,
        action: request.action,
        user_input: request.user_input,
        generated_command: request.generated_command,
        risk_level: request.risk_level,
        inserted_to_terminal: request.inserted_to_terminal,
        executed: request.executed,
        blocked: request.blocked,
        created_at: now_rfc3339(),
    };
    crate::storage::update_json_doc::<AiAuditFile, _, _>(crate::storage::JSON_AI_AUDIT, |file| {
        file.logs.push(log.clone());
        if file.logs.len() > AI_AUDIT_MAX_LOGS {
            let keep_from = file.logs.len().saturating_sub(AI_AUDIT_MAX_LOGS);
            file.logs = file.logs.split_off(keep_from);
        }
        Ok(log)
    })
}

pub fn get_ai_audit_logs(app: &AppHandle, limit: Option<usize>) -> AppResult<Vec<AiAuditLog>> {
    let _ = app;
    let mut logs =
        crate::storage::load_json_doc::<AiAuditFile>(crate::storage::JSON_AI_AUDIT)?.logs;
    logs.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    if let Some(limit) = limit {
        logs.truncate(limit);
    }
    Ok(logs)
}

fn emit_stream_event(app: &AppHandle, stream_id: &str, payload: AiStreamEventPayload) {
    let _ = app.emit(format!("ai-stream-{stream_id}").as_str(), payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_root_delete_as_critical() {
        let result = check_command_risk(CommandRiskRequest {
            command: "rm -rf /".to_string(),
            context: AiContext {
                username: Some("root".to_string()),
                ..AiContext::default()
            },
        });
        assert_eq!(result.risk_level, RiskLevel::Critical);
        assert!(result.blocked);
    }

    #[test]
    fn detects_mkfs_as_critical() {
        let result = check_command_risk(CommandRiskRequest {
            command: "mkfs.ext4 /dev/sda".to_string(),
            context: AiContext::default(),
        });
        assert_eq!(result.risk_level, RiskLevel::Critical);
        assert!(result.blocked);
    }

    #[test]
    fn detects_recursive_root_permission_changes_as_critical() {
        for command in ["chmod -R 777 /", "chown -R deploy /"] {
            let result = check_command_risk(CommandRiskRequest {
                command: command.to_string(),
                context: AiContext::default(),
            });
            assert_eq!(result.risk_level, RiskLevel::Critical);
            assert!(result.blocked);
        }
    }

    #[test]
    fn detects_reboot_as_high() {
        let result = check_command_risk(CommandRiskRequest {
            command: "reboot".to_string(),
            context: AiContext::default(),
        });
        assert!(result.risk_level >= RiskLevel::High);
        assert!(!result.blocked);
    }

    #[test]
    fn bumps_root_delete_risk() {
        let user_result = check_command_risk(CommandRiskRequest {
            command: "rm ./old.log".to_string(),
            context: AiContext {
                username: Some("deploy".to_string()),
                ..AiContext::default()
            },
        });
        let root_result = check_command_risk(CommandRiskRequest {
            command: "rm ./old.log".to_string(),
            context: AiContext {
                username: Some("root".to_string()),
                ..AiContext::default()
            },
        });
        assert!(root_result.risk_level > user_result.risk_level);
    }

    #[test]
    fn redacts_sensitive_values() {
        let raw = "password=secret token:abc Authorization: Bearer abc.def AKIA1234567890ABCDEF";
        let redacted = redact_sensitive_text(raw);
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("abc.def"));
        assert!(!redacted.contains("AKIA1234567890ABCDEF"));
    }

    #[test]
    fn parses_json_command_cards() {
        let raw = r#"{"text":"ok","commandCards":[{"id":"1","title":"CPU","command":"ps aux","explanation":"x","riskLevel":"low","riskReason":"read only","expectedEffect":"list","rollback":"none"}]}"#;
        let (text, reasoning, cards) = parse_model_output(raw, None);
        assert_eq!(text, "ok");
        assert_eq!(reasoning, None);
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn parse_failure_returns_text_without_cards() {
        let (text, reasoning, cards) = parse_model_output("plain text", None);
        assert_eq!(text, "plain text");
        assert_eq!(reasoning, None);
        assert!(cards.is_empty());
    }

    #[test]
    fn extracts_think_block_into_reasoning() {
        let (text, reasoning, cards) =
            parse_model_output("<think>step 1\nstep 2</think>final answer", None);
        assert_eq!(text, "final answer");
        assert_eq!(reasoning.as_deref(), Some("step 1\nstep 2"));
        assert!(cards.is_empty());
    }

    #[test]
    fn keeps_markdown_text_when_json_parse_fails() {
        let markdown = "## Summary\n\n- item 1\n- item 2";
        let (text, reasoning, cards) = parse_model_output(markdown, None);
        assert_eq!(text, markdown);
        assert_eq!(reasoning, None);
        assert!(cards.is_empty());
    }

    #[test]
    fn prefers_json_reasoning_when_present() {
        let raw = r#"{"text":"answer","reasoning":"first\nsecond","commandCards":[]}"#;
        let (text, reasoning, cards) = parse_model_output(raw, None);
        assert_eq!(text, "answer");
        assert_eq!(reasoning.as_deref(), Some("first\nsecond"));
        assert!(cards.is_empty());
    }

    #[test]
    fn old_history_without_reasoning_defaults_cleanly() {
        let raw = r#"{"sessions":[],"messages":[{"id":"m1","sessionId":"s1","role":"assistant","content":"hello","createdAt":"2026-04-28T00:00:00Z","commandCards":[]}]}"#;
        let history: AiHistoryFile = serde_json::from_str(raw).unwrap();
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].reasoning_content, None);
    }

    #[test]
    fn trims_ai_history_to_session_and_message_limits() {
        let mut history = AiHistoryFile::default();
        for session_idx in 0..220 {
            let session_id = format!("s-{session_idx:03}");
            let updated_at = format!(
                "2026-04-28T00:{:02}:{:02}Z",
                session_idx / 60,
                session_idx % 60
            );
            history.sessions.push(AiSession {
                id: session_id.clone(),
                connection_id: None,
                title: session_id.clone(),
                created_at: updated_at.clone(),
                updated_at,
            });
            for message_idx in 0..10 {
                history.messages.push(AiMessage {
                    id: format!("m-{session_idx:03}-{message_idx:02}"),
                    session_id: session_id.clone(),
                    role: if message_idx % 2 == 0 {
                        AiMessageRole::User
                    } else {
                        AiMessageRole::Assistant
                    },
                    content: "message".to_string(),
                    created_at: format!(
                        "2026-04-28T00:{:02}:{:02}.{:03}Z",
                        session_idx / 60,
                        session_idx % 60,
                        message_idx
                    ),
                    reasoning_content: None,
                    command_cards: vec![],
                });
            }
        }

        trim_history(&mut history);

        assert_eq!(history.sessions.len(), AI_HISTORY_MAX_SESSIONS);
        assert_eq!(history.messages.len(), AI_HISTORY_MAX_MESSAGES);
        let retained_sessions: HashSet<&str> = history
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect();
        assert!(!retained_sessions.contains("s-000"));
        assert!(retained_sessions.contains("s-219"));
        assert!(history
            .messages
            .iter()
            .all(|message| retained_sessions.contains(message.session_id.as_str())));
    }
}
