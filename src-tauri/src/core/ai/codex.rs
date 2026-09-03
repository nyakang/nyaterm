use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nyaterm_mcp_protocol::MCP_TOOL_REGISTRY;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, oneshot};
use tokio::time::timeout;

use crate::config::{AiAgentKind, AiBackendKind, AiModelSource, AiSettings, CodexThreadMode};
use crate::core::SessionManager;
use crate::error::{AppError, AppResult};
use crate::utils::process::hide_window;

use super::agent::{AgentApprovalManager, run_external_agent_command_step};
use super::history::{
    append_ai_audit, append_message, get_session_backend_metadata, save_user_message,
    set_session_backend_metadata, set_session_external_session_id,
};
use super::prompt::build_agent_prompt;
use super::redaction::{redact_context, redact_marker_values, redact_sensitive_text};
use super::stream::{active_streams, emit_stream_event};
use super::types::{
    AiChatRequest, AiMessage, AiMessageRole, AiModelDiscovery, AiSessionBackendMetadata,
    AiStreamEventPayload, AppendAiAuditRequest, CommandObservation, now_rfc3339, uuid,
};

const CODEX_TERMINAL_TOOLS_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexRuntimeState {
    Stopped,
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCliStatus {
    pub installed: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub checked_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccountStatus {
    pub connected: bool,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub requires_openai_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLoginStart {
    pub login_id: Option<String>,
    pub login_type: String,
    #[serde(default)]
    pub auth_url: Option<String>,
    #[serde(default)]
    pub verification_url: Option<String>,
    #[serde(default)]
    pub user_code: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexLoginFlow {
    Browser,
    DeviceCode,
}

pub struct CodexAppServerManager {
    state: RwLock<CodexRuntimeState>,
    writer: Mutex<Option<BufWriter<ChildStdin>>>,
    child: Mutex<Option<Child>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<AppResult<Value>>>>,
    active_turns: Mutex<HashMap<String, Arc<CodexTurnContext>>>,
    next_request_id: AtomicU64,
    last_account: Mutex<Option<CodexAccountStatus>>,
}

struct CodexTurnContext {
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    approval_manager: Arc<AgentApprovalManager>,
    stream_id: String,
    session_id: String,
    request: AiChatRequest,
    settings: AiSettings,
    step_counter: Mutex<u16>,
    dynamic_tool_call_count: AtomicU64,
    text_accumulator: Mutex<String>,
}

impl CodexAppServerManager {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(CodexRuntimeState::Stopped),
            writer: Mutex::new(None),
            child: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            active_turns: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(1),
            last_account: Mutex::new(None),
        }
    }

    pub async fn detect_cli(path: Option<String>) -> CodexCliStatus {
        let candidates = discover_codex_candidates(path.as_deref()).await;
        let checked_paths = candidates
            .iter()
            .map(|candidate| candidate.executable.clone())
            .collect::<Vec<_>>();
        let mut errors = Vec::new();

        for candidate in &candidates {
            match probe_codex_cli(&candidate.executable).await {
                Ok(version) => {
                    return CodexCliStatus {
                        installed: true,
                        path: Some(candidate.executable.clone()),
                        version: Some(version),
                        error: None,
                        source: Some(candidate.source.to_string()),
                        checked_paths,
                    };
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", candidate.executable));
                }
            }
        }

        CodexCliStatus {
            installed: false,
            path: path
                .as_deref()
                .map(|value| codex_executable(Some(value)))
                .or_else(|| Some("codex".to_string())),
            version: None,
            error: Some(detect_error_message(&errors)),
            source: None,
            checked_paths,
        }
    }

    pub async fn ensure_started(self: &Arc<Self>, path: Option<String>) -> AppResult<()> {
        if *self.state.read().await == CodexRuntimeState::Ready {
            return Ok(());
        }

        let mut child_guard = self.child.lock().await;
        if *self.state.read().await == CodexRuntimeState::Ready {
            return Ok(());
        }

        *self.state.write().await = CodexRuntimeState::Starting;
        let executable = codex_executable(path.as_deref());
        let mut command = Command::new(&executable);
        hide_window(&mut command);
        let mut child = command
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                AppError::Config(format!("Failed to start codex app-server: {error}"))
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Channel("codex stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Channel("codex stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::Channel("codex stderr unavailable".to_string()))?;

        *self.writer.lock().await = Some(BufWriter::new(stdin));
        *child_guard = Some(child);

        let reader_manager = self.clone();
        tauri::async_runtime::spawn(async move {
            reader_manager.read_stdout(stdout).await;
        });
        tauri::async_runtime::spawn(async move {
            read_stderr(stderr).await;
        });

        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "nyaterm",
                    "title": "NyaTerm",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true
                }
            }),
        )
        .await?;
        self.notify("initialized", json!({})).await?;
        *self.state.write().await = CodexRuntimeState::Ready;
        Ok(())
    }

    pub async fn account_read(
        self: &Arc<Self>,
        settings: &AiSettings,
    ) -> AppResult<CodexAccountStatus> {
        self.ensure_started(settings.codex.executable_path.clone())
            .await?;
        let value = self
            .request("account/read", json!({ "refreshToken": false }))
            .await?;
        let status = parse_account_status(&value);
        *self.last_account.lock().await = Some(status.clone());
        Ok(status)
    }

    pub async fn login_start(
        self: &Arc<Self>,
        settings: &AiSettings,
        flow: CodexLoginFlow,
    ) -> AppResult<CodexLoginStart> {
        self.ensure_started(settings.codex.executable_path.clone())
            .await?;
        let params = match flow {
            CodexLoginFlow::Browser => json!({
                "type": "chatgpt",
                "useHostedLoginSuccessPage": true,
                "appBrand": "codex"
            }),
            CodexLoginFlow::DeviceCode => json!({ "type": "chatgptDeviceCode" }),
        };
        let value = self.request("account/login/start", params).await?;
        Ok(CodexLoginStart {
            login_id: value
                .get("loginId")
                .and_then(Value::as_str)
                .map(str::to_string),
            login_type: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("chatgpt")
                .to_string(),
            auth_url: value
                .get("authUrl")
                .and_then(Value::as_str)
                .map(str::to_string),
            verification_url: value
                .get("verificationUrl")
                .and_then(Value::as_str)
                .map(str::to_string),
            user_code: value
                .get("userCode")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    pub async fn login_cancel(
        self: &Arc<Self>,
        settings: &AiSettings,
        login_id: String,
    ) -> AppResult<()> {
        self.ensure_started(settings.codex.executable_path.clone())
            .await?;
        self.request("account/login/cancel", json!({ "loginId": login_id }))
            .await?;
        Ok(())
    }

    pub async fn logout(self: &Arc<Self>, settings: &AiSettings) -> AppResult<()> {
        self.ensure_started(settings.codex.executable_path.clone())
            .await?;
        self.request("account/logout", json!({})).await?;
        *self.last_account.lock().await = Some(CodexAccountStatus::default());
        Ok(())
    }

    pub async fn list_models(
        self: &Arc<Self>,
        settings: &AiSettings,
    ) -> AppResult<Vec<AiModelDiscovery>> {
        if !settings.codex.enabled {
            return Ok(Vec::new());
        }
        self.ensure_started(settings.codex.executable_path.clone())
            .await?;
        let value = self
            .request("model/list", json!({ "includeHidden": false }))
            .await?;
        let items = value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items
            .into_iter()
            .filter_map(|item| {
                let model = item
                    .get("model")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)?;
                Some(AiModelDiscovery {
                    id: format!("codex:{model}"),
                    name: model.to_string(),
                    backend: AiBackendKind::Codex,
                    provider_kind: None,
                    credential_id: None,
                    source: AiModelSource::RustGenai,
                })
            })
            .collect())
    }

    async fn request(&self, method: &str, params: Value) -> AppResult<Value> {
        let id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.write_message(json!({ "method": method, "id": id, "params": params }))
            .await?;
        rx.await
            .map_err(|_| AppError::Channel("codex response channel closed".to_string()))?
    }

    async fn notify(&self, method: &str, params: Value) -> AppResult<()> {
        self.write_message(json!({ "method": method, "params": params }))
            .await
    }

    async fn write_message(&self, value: Value) -> AppResult<()> {
        let mut writer = self.writer.lock().await;
        let Some(writer) = writer.as_mut() else {
            return Err(AppError::Channel(
                "codex app-server is not started".to_string(),
            ));
        };
        let line = serde_json::to_string(&value)?;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }

    async fn read_stdout(self: Arc<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                tracing::warn!("Ignoring invalid codex app-server JSONL line");
                continue;
            };
            self.handle_message(value).await;
        }
        *self.state.write().await = CodexRuntimeState::Failed;
        let mut pending = self.pending.lock().await;
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(AppError::Channel(
                "codex app-server exited".to_string(),
            )));
        }
    }

    async fn handle_message(&self, value: Value) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if value.get("method").is_some() {
                self.handle_server_request(id, value).await;
                return;
            }
            let sender = self.pending.lock().await.remove(&id);
            if let Some(sender) = sender {
                let result = if let Some(error) = value.get("error") {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| error.to_string());
                    Err(AppError::Config(format!(
                        "Codex app-server error: {message}"
                    )))
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = sender.send(result);
            }
            return;
        }

        if let Some(method) = value.get("method").and_then(Value::as_str) {
            self.handle_notification(method, value.get("params").cloned().unwrap_or(Value::Null))
                .await;
        }
    }

    async fn handle_server_request(&self, id: u64, value: Value) {
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let result = match method.as_str() {
            "item/tool/call" => self.handle_dynamic_tool_call(params).await,
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                Ok(json!({ "decision": "decline" }))
            }
            "item/permissions/requestApproval" => Ok(json!({ "decision": "decline" })),
            "mcpServer/elicitation/request" => Ok(json!({ "action": "decline", "content": null })),
            _ => Ok(json!({})),
        };

        let response = match result {
            Ok(result) => json!({ "id": id, "result": result }),
            Err(error) => json!({
                "id": id,
                "error": { "code": -32000, "message": error.to_string() }
            }),
        };
        let _ = self.write_message(response).await;
    }

    async fn handle_dynamic_tool_call(&self, params: Value) -> AppResult<Value> {
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Config("Codex tool call missing turnId".to_string()))?;
        let namespace = params
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let tool = params
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let context = {
            let active = self.active_turns.lock().await;
            active.get(turn_id).cloned()
        }
        .ok_or_else(|| AppError::Config("No active Codex turn for tool call".to_string()))?;
        context
            .dynamic_tool_call_count
            .fetch_add(1, Ordering::SeqCst);

        if namespace != "nyaterm_terminal" {
            return Ok(dynamic_text(false, "Unsupported dynamic tool namespace"));
        }

        match tool {
            "get_context" => Ok(dynamic_text(true, &terminal_context_text(&context.request))),
            "execute_command" => {
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                let command = args
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| AppError::Config("terminal command is required".to_string()))?
                    .to_string();
                let target = args
                    .get("targetTerminalSessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let reason = args
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let step_index = {
                    let mut counter = context.step_counter.lock().await;
                    *counter = counter.saturating_add(1);
                    *counter
                };
                match run_external_agent_command_step(
                    &context.app,
                    context.session_manager.clone(),
                    context.approval_manager.clone(),
                    &context.stream_id,
                    &context.session_id,
                    &context.request,
                    &context.settings,
                    step_index,
                    command,
                    reason,
                    target,
                )
                .await
                {
                    Ok(observation) => Ok(dynamic_text(true, &observation_text(&observation))),
                    Err(error) => Ok(dynamic_text(false, &error.to_string())),
                }
            }
            _ => Ok(dynamic_text(false, "Unsupported dynamic tool")),
        }
    }

    async fn handle_notification(&self, method: &str, params: Value) {
        match method {
            "account/updated" => {
                let status = CodexAccountStatus {
                    connected: params
                        .get("authMode")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty()),
                    auth_mode: params
                        .get("authMode")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    plan_type: params
                        .get("planType")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    email: None,
                    requires_openai_auth: true,
                };
                *self.last_account.lock().await = Some(status);
            }
            "item/agentMessage/delta" => {
                if let Some(turn_id) = params.get("turnId").and_then(Value::as_str) {
                    let context = {
                        let active = self.active_turns.lock().await;
                        active.get(turn_id).cloned()
                    };
                    if let Some(context) = context
                        && let Some(delta) = params.get("delta").and_then(Value::as_str)
                        && !delta.is_empty()
                    {
                        context.text_accumulator.lock().await.push_str(delta);
                        emit_stream_event(
                            &context.app,
                            &context.stream_id,
                            AiStreamEventPayload {
                                event_type: "delta".to_string(),
                                stream_id: context.stream_id.clone(),
                                session_id: Some(context.session_id.clone()),
                                text_delta: Some(delta.to_string()),
                                reasoning_delta: None,
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            },
                        );
                    }
                }
            }
            "item/reasoning/delta" | "item/reasoning/summaryDelta" => {
                if let Some(turn_id) = params.get("turnId").and_then(Value::as_str) {
                    let context = {
                        let active = self.active_turns.lock().await;
                        active.get(turn_id).cloned()
                    };
                    if let Some(context) = context {
                        let delta = params
                            .get("delta")
                            .or_else(|| params.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !delta.is_empty() {
                            emit_stream_event(
                                &context.app,
                                &context.stream_id,
                                AiStreamEventPayload {
                                    event_type: "reasoning_delta".to_string(),
                                    stream_id: context.stream_id.clone(),
                                    session_id: Some(context.session_id.clone()),
                                    text_delta: None,
                                    reasoning_delta: Some(delta.to_string()),
                                    message: None,
                                    command_cards: vec![],
                                    usage: None,
                                    error: None,
                                },
                            );
                        }
                    }
                }
            }
            "turn/completed" => {
                let turn_id = params
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(turn_id) = turn_id {
                    self.complete_turn(&turn_id, params).await;
                }
            }
            "error" => {
                let message = params
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Codex request failed")
                    .to_string();
                let contexts: Vec<Arc<CodexTurnContext>> =
                    self.active_turns.lock().await.values().cloned().collect();
                for context in contexts {
                    emit_stream_event(
                        &context.app,
                        &context.stream_id,
                        AiStreamEventPayload {
                            event_type: "error".to_string(),
                            stream_id: context.stream_id.clone(),
                            session_id: Some(context.session_id.clone()),
                            text_delta: None,
                            reasoning_delta: None,
                            message: None,
                            command_cards: vec![],
                            usage: None,
                            error: Some(message.clone()),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    async fn complete_turn(&self, turn_id: &str, params: Value) {
        let context = self.active_turns.lock().await.remove(turn_id);
        let Some(context) = context else {
            return;
        };
        active_streams().lock().unwrap().remove(&context.stream_id);

        let turn = params.get("turn").cloned().unwrap_or(Value::Null);
        let status = turn.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "failed" {
            let error = turn
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Codex turn failed")
                .to_string();
            emit_stream_event(
                &context.app,
                &context.stream_id,
                AiStreamEventPayload {
                    event_type: "error".to_string(),
                    stream_id: context.stream_id.clone(),
                    session_id: Some(context.session_id.clone()),
                    text_delta: None,
                    reasoning_delta: None,
                    message: None,
                    command_cards: vec![],
                    usage: None,
                    error: Some(error),
                },
            );
            return;
        }

        let content = {
            let final_text = final_agent_text(&turn);
            if final_text.is_empty() {
                context.text_accumulator.lock().await.clone()
            } else {
                final_text
            }
        };
        if context.dynamic_tool_call_count.load(Ordering::SeqCst) == 0
            && looks_like_codex_command_plan(&content)
        {
            let error = "Codex returned a command plan as text instead of calling the NyaTerm terminal tools. The command was not executed. Please start a new Codex Agent session or check Codex app-server dynamic tool compatibility.".to_string();
            emit_stream_event(
                &context.app,
                &context.stream_id,
                AiStreamEventPayload {
                    event_type: "error".to_string(),
                    stream_id: context.stream_id.clone(),
                    session_id: Some(context.session_id.clone()),
                    text_delta: None,
                    reasoning_delta: None,
                    message: None,
                    command_cards: vec![],
                    usage: turn.get("usage").cloned(),
                    error: Some(error),
                },
            );
            return;
        }
        let message = AiMessage {
            id: format!("msg-{}", uuid()),
            session_id: context.session_id.clone(),
            role: AiMessageRole::Assistant,
            content,
            created_at: now_rfc3339(),
            reasoning_content: None,
            command_cards: vec![],
        };
        if context.settings.record_history {
            let _ = append_message(&context.app, message.clone());
        }
        emit_stream_event(
            &context.app,
            &context.stream_id,
            AiStreamEventPayload {
                event_type: "done".to_string(),
                stream_id: context.stream_id.clone(),
                session_id: Some(context.session_id.clone()),
                text_delta: None,
                reasoning_delta: None,
                message: Some(message),
                command_cards: vec![],
                usage: turn.get("usage").cloned(),
                error: None,
            },
        );
    }
}

pub async fn run_codex_stream(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    approval_manager: Arc<AgentApprovalManager>,
    manager: Arc<CodexAppServerManager>,
    stream_id: String,
    session_id: String,
    mut request: AiChatRequest,
    settings: AiSettings,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let result = run_codex_stream_inner(
        app.clone(),
        session_manager,
        approval_manager,
        manager.clone(),
        stream_id.clone(),
        session_id.clone(),
        &mut request,
        settings,
        &mut cancel_rx,
    )
    .await;

    if let Err(error) = result {
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

async fn run_codex_stream_inner(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    approval_manager: Arc<AgentApprovalManager>,
    manager: Arc<CodexAppServerManager>,
    stream_id: String,
    session_id: String,
    request: &mut AiChatRequest,
    settings: AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<()> {
    if !settings.codex.enabled {
        return Err(AppError::Config(
            "Codex integration is disabled".to_string(),
        ));
    }
    let selected_model_name = resolve_codex_model_name(&settings, request);

    manager
        .ensure_started(settings.codex.executable_path.clone())
        .await?;

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
        save_user_message(&app, &session_id, request)?;
    }

    let use_nyaterm_mcp = settings.codex.tool_integration_mode.as_deref() == Some("nyaterm_mcp");
    let mut mcp_fallback = false;
    let mut mcp_credential = if use_nyaterm_mcp {
        if let Some(mcp) = app.try_state::<Arc<crate::core::mcp::McpManager>>() {
            let (scope, default_id) = codex_request_terminal_scope(request);
            let owner =
                if let Some(id) = default_id.as_deref().or(scope.first().map(String::as_str)) {
                    session_manager
                        .session_info(id)
                        .await
                        .ok()
                        .and_then(|info| info.owner_window_label)
                } else {
                    None
                };
            match mcp
                .create_ephemeral_credential(
                    "codex_mcp",
                    scope,
                    default_id,
                    request.permission_mode.clone(),
                    owner,
                )
                .await
            {
                Ok(credential) => Some(credential),
                Err(error) => {
                    mcp_fallback = true;
                    emit_codex_mcp_fallback(
                        &app,
                        &stream_id,
                        &session_id,
                        request,
                        &error.to_string(),
                    );
                    None
                }
            }
        } else {
            mcp_fallback = true;
            emit_codex_mcp_fallback(
                &app,
                &stream_id,
                &session_id,
                request,
                "The NyaTerm MCP manager is unavailable.",
            );
            None
        }
    } else {
        None
    };

    let metadata = get_session_backend_metadata(&app, &session_id)?;
    let thread_id = if mcp_credential.is_some() || mcp_fallback {
        None
    } else {
        reusable_codex_thread_id(metadata.as_ref())
    };

    let thread_id = if let Some(thread_id) = thread_id {
        manager
            .request("thread/resume", json!({ "threadId": thread_id }))
            .await?;
        thread_id
    } else {
        let params = codex_thread_start_params_with_mcp(
            selected_model_name.as_deref(),
            settings.codex.thread_mode == CodexThreadMode::Ephemeral,
            mcp_credential.as_ref(),
        );
        let response = match manager.request("thread/start", params).await {
            Ok(response) => response,
            Err(error) if mcp_credential.is_some() => {
                emit_codex_mcp_fallback(&app, &stream_id, &session_id, request, &error.to_string());
                mcp_credential.take();
                manager
                    .request(
                        "thread/start",
                        codex_thread_start_params_with_mcp(
                            selected_model_name.as_deref(),
                            settings.codex.thread_mode == CodexThreadMode::Ephemeral,
                            None,
                        ),
                    )
                    .await?
            }
            Err(error) => return Err(error),
        };
        let new_thread_id = response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .or_else(|| response.get("id").and_then(Value::as_str))
            .ok_or_else(|| {
                AppError::Config("Codex thread/start returned no thread id".to_string())
            })?
            .to_string();
        set_session_backend_metadata(
            &app,
            &session_id,
            AiSessionBackendMetadata {
                backend: AiBackendKind::Codex,
                external_thread_id: Some(new_thread_id.clone()),
                codex_terminal_tools_version: Some(CODEX_TERMINAL_TOOLS_VERSION),
            },
        )?;
        set_session_external_session_id(
            &app,
            &session_id,
            AiAgentKind::Codex,
            new_thread_id.clone(),
        )?;
        new_thread_id
    };

    let prompt = build_codex_agent_prompt(request, &settings);
    let response = manager
        .request(
            "turn/start",
            codex_turn_start_params(&thread_id, &prompt, selected_model_name.as_deref()),
        )
        .await?;
    let turn_id = response
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Config("Codex turn/start returned no turn id".to_string()))?
        .to_string();

    manager.active_turns.lock().await.insert(
        turn_id.clone(),
        Arc::new(CodexTurnContext {
            app,
            session_manager,
            approval_manager,
            stream_id: stream_id.clone(),
            session_id,
            request: request.clone(),
            settings,
            step_counter: Mutex::new(0),
            dynamic_tool_call_count: AtomicU64::new(0),
            text_accumulator: Mutex::new(String::new()),
        }),
    );

    tokio::select! {
        _ = cancel_rx => {
            let _ = manager
                .request(
                    "turn/interrupt",
                    json!({ "threadId": thread_id.clone(), "turnId": turn_id.clone() }),
                )
                .await;
            manager.active_turns.lock().await.remove(&turn_id);
            Err(AppError::Cancelled("AI stream cancelled".to_string()))
        }
        _ = wait_until_turn_removed(manager.clone(), turn_id.clone()) => Ok(())
    }
}

async fn wait_until_turn_removed(manager: Arc<CodexAppServerManager>, turn_id: String) {
    loop {
        if !manager.active_turns.lock().await.contains_key(&turn_id) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

const CODEX_DETECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
struct CodexCliCandidate {
    executable: String,
    source: &'static str,
}

fn codex_executable(path: Option<&str>) -> String {
    path.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("codex")
        .to_string()
}

async fn discover_codex_candidates(path: Option<&str>) -> Vec<CodexCliCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) {
        add_codex_candidate(&mut candidates, &mut seen, path, "configured");
    }
    add_codex_candidate(&mut candidates, &mut seen, "codex", "path");
    add_common_codex_candidates(&mut candidates, &mut seen);
    for discovered in discover_codex_with_path_command().await {
        add_codex_candidate(&mut candidates, &mut seen, discovered, "path_lookup");
    }

    candidates
}

fn add_codex_candidate(
    candidates: &mut Vec<CodexCliCandidate>,
    seen: &mut HashSet<String>,
    executable: impl AsRef<str>,
    source: &'static str,
) {
    let executable = executable.as_ref().trim();
    if executable.is_empty() {
        return;
    }
    let key = codex_candidate_key(executable);
    if seen.insert(key) {
        candidates.push(CodexCliCandidate {
            executable: executable.to_string(),
            source,
        });
    }
}

fn add_existing_codex_candidate(
    candidates: &mut Vec<CodexCliCandidate>,
    seen: &mut HashSet<String>,
    path: PathBuf,
    source: &'static str,
) {
    if path.exists() {
        add_codex_candidate(candidates, seen, path.to_string_lossy(), source);
    }
}

fn codex_candidate_key(executable: &str) -> String {
    #[cfg(windows)]
    {
        executable.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        executable.to_string()
    }
}

fn add_common_codex_candidates(
    candidates: &mut Vec<CodexCliCandidate>,
    seen: &mut HashSet<String>,
) {
    #[cfg(windows)]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            let npm = Path::new(&appdata).join("npm");
            for name in ["codex.cmd", "codex.exe", "codex"] {
                add_existing_codex_candidate(candidates, seen, npm.join(name), "common");
            }
        }
        if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
            let pnpm = Path::new(&local_appdata).join("pnpm");
            for name in ["codex.cmd", "codex.exe", "codex"] {
                add_existing_codex_candidate(candidates, seen, pnpm.join(name), "common");
            }
        }
        if let Ok(userprofile) = env::var("USERPROFILE") {
            let home = Path::new(&userprofile);
            for dir in [
                home.join("scoop").join("shims"),
                home.join(".bun").join("bin"),
            ] {
                for name in ["codex.cmd", "codex.exe", "codex"] {
                    add_existing_codex_candidate(candidates, seen, dir.join(name), "common");
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(home) = env::var("HOME") {
            let home = Path::new(&home);
            for path in [
                home.join(".local").join("bin").join("codex"),
                home.join(".npm-global").join("bin").join("codex"),
                home.join(".bun").join("bin").join("codex"),
                home.join(".cargo").join("bin").join("codex"),
            ] {
                add_existing_codex_candidate(candidates, seen, path, "common");
            }
        }
        for path in [
            PathBuf::from("/opt/homebrew/bin/codex"),
            PathBuf::from("/usr/local/bin/codex"),
            PathBuf::from("/usr/bin/codex"),
        ] {
            add_existing_codex_candidate(candidates, seen, path, "common");
        }
    }
}

async fn discover_codex_with_path_command() -> Vec<String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("where.exe");
        command.arg("codex");
        command
    } else {
        let mut command = Command::new("which");
        command.args(["-a", "codex"]);
        command
    };
    hide_window(&mut command);
    let output = command.output().await;

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

async fn probe_codex_cli(executable: &str) -> Result<String, String> {
    let mut command = Command::new(executable);
    command.arg("--version");
    hide_window(&mut command);
    let output = timeout(CODEX_DETECT_TIMEOUT, command.output())
        .await
        .map_err(|_| "timed out while running --version".to_string())?
        .map_err(|error| error.to_string())?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Ok(if stdout.is_empty() { stderr } else { stdout });
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        output.status.to_string()
    };
    Err(details)
}

fn detect_error_message(errors: &[String]) -> String {
    if errors.is_empty() {
        return "Codex CLI was not found in PATH or common install locations".to_string();
    }

    let mut message = "Codex CLI was not detected".to_string();
    for error in errors.iter().take(4) {
        message.push_str("; ");
        message.push_str(error);
    }
    if errors.len() > 4 {
        message.push_str(&format!("; {} more candidates failed", errors.len() - 4));
    }
    message
}

fn parse_account_status(value: &Value) -> CodexAccountStatus {
    let account = value.get("account").unwrap_or(&Value::Null);
    let auth_mode = account
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    CodexAccountStatus {
        connected: auth_mode.is_some(),
        auth_mode,
        plan_type: account
            .get("planType")
            .and_then(Value::as_str)
            .map(str::to_string),
        email: account
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string),
        requires_openai_auth: value
            .get("requiresOpenaiAuth")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn reusable_codex_thread_id(metadata: Option<&AiSessionBackendMetadata>) -> Option<String> {
    let metadata = metadata?;
    if metadata.backend != AiBackendKind::Codex
        || metadata.codex_terminal_tools_version != Some(CODEX_TERMINAL_TOOLS_VERSION)
    {
        return None;
    }
    metadata.external_thread_id.clone()
}

fn resolve_codex_model_name(settings: &AiSettings, request: &AiChatRequest) -> Option<String> {
    request
        .model_name
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .or_else(|| {
            settings
                .codex
                .default_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
fn codex_thread_start_params(model: Option<&str>, ephemeral: bool) -> Value {
    codex_thread_start_params_with_mcp(model, ephemeral, None)
}

fn codex_thread_start_params_with_mcp(
    model: Option<&str>,
    ephemeral: bool,
    credential: Option<&crate::core::mcp::EphemeralMcpCredential>,
) -> Value {
    let mut params = json!({
        "cwd": null,
        "ephemeral": ephemeral,
        "approvalPolicy": {
            "granular": {
                "rules": false,
                "mcp_elicitations": false,
                "request_permissions": false,
                "sandbox_approval": false
            }
        },
        "approvalsReviewer": "user",
        "sandbox": "read-only",
        "developerInstructions": codex_developer_instructions()
    });
    if let Some(credential) = credential {
        params["config"] = codex_nyaterm_mcp_config(&credential.sidecar_path, credential.env());
    } else {
        params["dynamicTools"] = json!([terminal_tool_namespace()]);
    }
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        params["model"] = json!(model);
    }
    params
}

fn nyaterm_mcp_tool_names() -> Vec<&'static str> {
    MCP_TOOL_REGISTRY
        .iter()
        .map(|definition| definition.tool)
        .collect()
}

fn codex_nyaterm_mcp_config(sidecar_path: &Path, environment: HashMap<String, String>) -> Value {
    json!({
        "mcp_servers": {
            "nyaterm": {
                "command": sidecar_path,
                "args": [],
                "env": environment,
                "required": true,
                "enabled_tools": nyaterm_mcp_tool_names(),
                "default_tools_approval_mode": "approve",
            }
        }
    })
}

fn emit_codex_mcp_fallback(
    app: &AppHandle,
    stream_id: &str,
    session_id: &str,
    request: &AiChatRequest,
    reason: &str,
) {
    let reason = redact_sensitive_text(reason);
    tracing::warn!(
        stream_id,
        session_id,
        permission_mode = ?request.permission_mode,
        reason,
        "Codex MCP integration failed; using the NyaTerm dynamic terminal tool"
    );
    emit_stream_event(
        app,
        stream_id,
        AiStreamEventPayload {
            event_type: "warning".to_string(),
            stream_id: stream_id.to_string(),
            session_id: Some(session_id.to_string()),
            text_delta: None,
            reasoning_delta: None,
            message: None,
            command_cards: vec![],
            usage: None,
            error: Some("codex_mcp_fallback".to_string()),
        },
    );
    let _ = append_ai_audit(
        app,
        AppendAiAuditRequest {
            connection_id: request.connection_id.clone(),
            action: "ai.codex_mcp_fallback".to_string(),
            user_input: None,
            generated_command: None,
            risk_level: None,
            inserted_to_terminal: false,
            executed: false,
            blocked: false,
            source: Some("codex_mcp".to_string()),
            client: Some("Codex".to_string()),
            capability: None,
            session_id: request.terminal_session_id.clone(),
            permission_mode: Some(request.permission_mode.clone()),
            approval_decision: Some("fallback".to_string()),
            success: Some(false),
            duration_ms: None,
            error: Some(reason),
        },
    );
}

fn codex_request_terminal_scope(request: &AiChatRequest) -> (Vec<String>, Option<String>) {
    let mut scope = request
        .targets
        .iter()
        .map(|target| target.terminal_session_id.clone())
        .collect::<HashSet<_>>();
    if let Some(id) = request.terminal_session_id.as_ref() {
        scope.insert(id.clone());
    }
    if let Some(id) = request.default_target_session_id.as_ref() {
        scope.insert(id.clone());
    }
    let mut scope = scope.into_iter().collect::<Vec<_>>();
    scope.sort();
    let default_id = request
        .default_target_session_id
        .clone()
        .or_else(|| request.terminal_session_id.clone())
        .filter(|id| scope.contains(id));
    (scope, default_id)
}

fn codex_turn_start_params(thread_id: &str, prompt: &str, model: Option<&str>) -> Value {
    let mut params = json!({
        "threadId": thread_id,
        "clientUserMessageId": format!("msg-{}", uuid()),
        "input": [{ "type": "text", "text": prompt, "text_elements": [] }],
    });
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        params["model"] = json!(model);
    }
    params
}

fn terminal_tool_namespace() -> Value {
    json!({
        "type": "namespace",
        "name": "nyaterm_terminal",
        "description": "Operate the active NyaTerm terminal sessions. Use these tools instead of local shell or file tools when working with remote terminals.",
        "tools": [
            {
                "type": "function",
                "name": "get_context",
                "description": "Read the available NyaTerm terminal targets and recent terminal context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "type": "function",
                "name": "execute_command",
                "description": "Execute a shell command in a NyaTerm terminal session after NyaTerm approval policy is applied.",
                "inputSchema": {
                    "type": "object",
                    "required": ["targetTerminalSessionId", "command"],
                    "properties": {
                        "targetTerminalSessionId": { "type": "string" },
                        "command": { "type": "string" },
                        "reason": { "type": "string" }
                    }
                }
            }
        ]
    })
}

fn codex_developer_instructions() -> &'static str {
    "You are running inside NyaTerm as a terminal automation agent. Use the nyaterm_terminal namespace for terminal work. Call nyaterm_terminal.get_context when target context is unclear. Call nyaterm_terminal.execute_command for commands that need to run in a NyaTerm terminal session. Do not output command-plan JSON such as {\"commands\":[...]}; if a command is needed, call the tool. Do not use local shell or file tools for the user's remote terminal. After observations are sufficient, answer the user in normal assistant text."
}

fn build_codex_agent_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    format!(
        "{}\n\nCodex Agent protocol:\n- Use the nyaterm_terminal namespace for all NyaTerm terminal actions.\n- If you need to inspect or change the terminal, call nyaterm_terminal.execute_command with targetTerminalSessionId and command.\n- If the target is unclear, call nyaterm_terminal.get_context first.\n- Do not return command cards, protocol JSON, or {{\"commands\":[...]}} in assistant text.\n- When finished, reply with a normal user-facing final answer.",
        build_agent_prompt(request, settings)
    )
}

fn terminal_context_text(request: &AiChatRequest) -> String {
    serde_json::to_string_pretty(&json!({
        "primaryContext": request.context,
        "targets": request.targets,
        "targetContexts": request.target_contexts,
        "instruction": "Use nyaterm_terminal.execute_command for remote terminal actions. Do not use local shell/file tools for the user's remote terminal."
    }))
    .unwrap_or_else(|_| "Terminal context unavailable".to_string())
}

fn observation_text(observation: &CommandObservation) -> String {
    serde_json::to_string_pretty(observation).unwrap_or_else(|_| observation.output.clone())
}

fn dynamic_text(success: bool, text: &str) -> Value {
    json!({
        "success": success,
        "contentItems": [{ "type": "inputText", "text": text }]
    })
}

fn final_agent_text(turn: &Value) -> String {
    turn.get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().rev().find_map(|item| {
                if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                    return None;
                }
                item.get("text").and_then(Value::as_str).map(str::to_string)
            })
        })
        .unwrap_or_default()
}

fn looks_like_codex_command_plan(text: &str) -> bool {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return false;
    };
    value
        .get("commands")
        .and_then(Value::as_array)
        .is_some_and(|commands| !commands.is_empty())
}

async fn read_stderr(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let sanitized = sanitize_codex_log_line(&line);
        if !sanitized.trim().is_empty() {
            tracing::debug!(target: "codex_app_server", message = %sanitized);
        }
    }
}

fn sanitize_codex_log_line(line: &str) -> String {
    redact_marker_values(
        line,
        &["access_token=", "refresh_token=", "id_token=", "code="],
    )
}

pub async fn manager_from_app(app: &AppHandle) -> AppResult<Arc<CodexAppServerManager>> {
    Ok(app.state::<Arc<CodexAppServerManager>>().inner().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_codex_auth_material_from_logs() {
        let line =
            r#"login access_token=abc123 refresh_token=def456 id_token=ghi789 code=xyz&state=ok"#;

        let sanitized = sanitize_codex_log_line(line);

        assert!(!sanitized.contains("abc123"));
        assert!(!sanitized.contains("def456"));
        assert!(!sanitized.contains("ghi789"));
        assert!(!sanitized.contains("code=xyz"));
        assert!(sanitized.contains("access_token=[redacted]"));
        assert!(sanitized.contains("refresh_token=[redacted]"));
        assert!(sanitized.contains("id_token=[redacted]"));
        assert!(sanitized.contains("code=[redacted]&state=ok"));
    }

    #[test]
    fn extracts_final_agent_text_from_turn_items() {
        let turn = json!({
            "items": [
                { "type": "reasoning", "text": "thinking" },
                { "type": "agentMessage", "text": "first" },
                { "type": "agentMessage", "text": "final" }
            ]
        });

        assert_eq!(final_agent_text(&turn), "final");
    }

    #[test]
    fn thread_start_params_register_nyaterm_terminal_tools() {
        let params = codex_thread_start_params(Some("gpt-5-codex"), false);

        let namespace = params
            .get("dynamicTools")
            .and_then(Value::as_array)
            .and_then(|tools| tools.first())
            .expect("terminal tool namespace");

        assert_eq!(
            namespace.get("name").and_then(Value::as_str),
            Some("nyaterm_terminal")
        );
        let tools = namespace
            .get("tools")
            .and_then(Value::as_array)
            .expect("namespace tools");
        assert!(
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some("execute_command")
            })
        );
        assert!(
            params
                .get("developerInstructions")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains("nyaterm_terminal.execute_command"))
        );
    }

    #[test]
    fn codex_preapproves_only_registered_nyaterm_mcp_tools() {
        let config = codex_nyaterm_mcp_config(
            Path::new("nyaterm-mcp"),
            HashMap::from([("NYATERM_MCP_TOKEN".to_string(), "secret".to_string())]),
        );
        let server = config
            .pointer("/mcp_servers/nyaterm")
            .expect("nyaterm MCP server config");
        let enabled_tools = server["enabled_tools"]
            .as_array()
            .expect("enabled tool allow-list")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();

        assert_eq!(
            server["default_tools_approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(enabled_tools, nyaterm_mcp_tool_names());

        let thread = codex_thread_start_params(None, false);
        assert_eq!(thread["sandbox"].as_str(), Some("read-only"));
        assert_eq!(
            thread["approvalPolicy"]["granular"]["sandbox_approval"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn codex_model_is_optional_in_app_server_payloads() {
        let thread_params = codex_thread_start_params(None, false);
        let turn_params = codex_turn_start_params("thread-1", "hello", None);

        assert!(thread_params.get("model").is_none());
        assert!(turn_params.get("model").is_none());

        let thread_params = codex_thread_start_params(Some("gpt-5-codex"), false);
        let turn_params = codex_turn_start_params("thread-1", "hello", Some("gpt-5-codex"));

        assert_eq!(
            thread_params.get("model").and_then(Value::as_str),
            Some("gpt-5-codex")
        );
        assert_eq!(
            turn_params.get("model").and_then(Value::as_str),
            Some("gpt-5-codex")
        );
    }

    #[test]
    fn codex_agent_prompt_does_not_request_command_cards() {
        let request = AiChatRequest {
            stream_id: None,
            session_id: None,
            connection_id: None,
            terminal_session_id: Some("term-1".to_string()),
            owner_scope: Default::default(),
            targets: vec![],
            target_contexts: vec![],
            mode: crate::config::AiMode::Agent,
            agent_kind: AiAgentKind::Codex,
            permission_mode: crate::config::AiPermissionMode::Confirm,
            model_id: None,
            model_name: None,
            default_target_session_id: Some("term-1".to_string()),
            existing_external_session_id: None,
            attachments: vec![],
            action: super::super::types::AiAction::GenerateCommand,
            user_input: "check load".to_string(),
            context: Default::default(),
            options: Default::default(),
        };

        let prompt = build_codex_agent_prompt(&request, &AiSettings::default());

        assert!(prompt.contains("nyaterm_terminal.execute_command"));
        assert!(!prompt.contains("commandCards"));
        assert!(!prompt.contains("必须返回 JSON 对象"));
    }

    #[test]
    fn reusable_codex_thread_requires_matching_terminal_tool_version() {
        let old_metadata = AiSessionBackendMetadata {
            backend: AiBackendKind::Codex,
            external_thread_id: Some("thread-old".to_string()),
            codex_terminal_tools_version: None,
        };
        let current_metadata = AiSessionBackendMetadata {
            backend: AiBackendKind::Codex,
            external_thread_id: Some("thread-current".to_string()),
            codex_terminal_tools_version: Some(CODEX_TERMINAL_TOOLS_VERSION),
        };
        let other_backend_metadata = AiSessionBackendMetadata {
            backend: AiBackendKind::Genai,
            external_thread_id: Some("thread-other".to_string()),
            codex_terminal_tools_version: Some(CODEX_TERMINAL_TOOLS_VERSION),
        };

        assert_eq!(reusable_codex_thread_id(Some(&old_metadata)), None);
        assert_eq!(
            reusable_codex_thread_id(Some(&current_metadata)),
            Some("thread-current".to_string())
        );
        assert_eq!(
            reusable_codex_thread_id(Some(&other_backend_metadata)),
            None
        );
    }

    #[test]
    fn detects_codex_command_plan_text() {
        assert!(looks_like_codex_command_plan(
            r#"{"commands":[{"command":"uptime"}]}"#
        ));
        assert!(looks_like_codex_command_plan(
            "```json\n{\"commands\":[{\"command\":\"uptime\"}]}\n```"
        ));
        assert!(!looks_like_codex_command_plan(
            r#"{"text":"ok","commandCards":[]}"#
        ));
        assert!(!looks_like_codex_command_plan("load is normal"));
    }
}
