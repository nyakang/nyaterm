use crate::config::{self, AiProviderKind, AiProviderProfile, AiSettings};
use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatRequest {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub connection_id: Option<String>,
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
    command_cards: Vec<AiCommandCard>,
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

fn active_streams() -> &'static Mutex<HashMap<String, oneshot::Sender<()>>> {
    ACTIVE_STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
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

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn start_chat_stream(app: AppHandle, mut request: AiChatRequest) -> AppResult<AiStreamStart> {
    let settings = config::load_app_settings(&app)?;
    if !settings.ai.enabled {
        return Err(AppError::Config("AI assistant is disabled".to_string()));
    }

    let stream_id = format!("ai-stream-{}", uuid());
    let session_id = request
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ai-session-{}", uuid()));
    request.session_id = Some(session_id.clone());

    let (cancel_tx, cancel_rx) = oneshot::channel();
    active_streams()
        .lock()
        .unwrap()
        .insert(stream_id.clone(), cancel_tx);

    let task_app = app.clone();
    let task_stream_id = stream_id.clone();
    let task_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        run_chat_stream(task_app, task_stream_id, task_session_id, request, settings.ai, cancel_rx)
            .await;
    });

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

    let result = run_model_stream(
        &app,
        &stream_id,
        &request,
        &settings,
        &mut cancel_rx,
    )
    .await;

    active_streams().lock().unwrap().remove(&stream_id);

    match result {
        Ok(raw_text) => {
            let (text, mut command_cards) = parse_model_output(&raw_text);
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
                    message: Some(message),
                    command_cards,
                    usage: None,
                    error: None,
                },
            );
        }
        Err(error) => {
            emit_stream_event(
                &app,
                &stream_id,
                AiStreamEventPayload {
                    event_type: "error".to_string(),
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id),
                    text_delta: None,
                    message: None,
                    command_cards: vec![],
                    usage: None,
                    error: Some(error.to_string()),
                },
            );
        }
    }
}

async fn run_model_stream(
    app: &AppHandle,
    stream_id: &str,
    request: &AiChatRequest,
    settings: &AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<String> {
    let profile = active_profile(settings)?;
    let client = build_client(profile)?;
    let prompt = build_prompt(request, settings);

    let chat_req = ChatRequest::new(vec![
        ChatMessage::system(SYSTEM_PROMPT),
        ChatMessage::user(prompt),
    ]);
    let chat_options = ChatOptions::default().with_max_tokens(settings.max_output_tokens);

    let stream_result = tokio::time::timeout(
        Duration::from_millis(settings.timeout_ms),
        client.exec_chat_stream(&profile.model, chat_req, Some(&chat_options)),
    )
    .await
    .map_err(|_| AppError::Config("AI request timed out".to_string()))?
    .map_err(|error| AppError::Config(format!("AI request failed: {error}")))?;

    let mut stream = stream_result.stream;
    let mut output = String::new();
    let timeout = tokio::time::sleep(Duration::from_millis(settings.timeout_ms));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => {
                return Err(AppError::Config("AI stream timed out".to_string()));
            }
            _ = &mut *cancel_rx => {
                return Err(AppError::Cancelled("AI stream cancelled".to_string()));
            }
            item = stream.next() => {
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
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::End(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        return Err(AppError::Config(format!("AI stream failed: {error}")));
                    }
                }
            }
        }
    }

    Ok(output)
}

fn active_profile(settings: &AiSettings) -> AppResult<&AiProviderProfile> {
    settings
        .provider_profiles
        .iter()
        .find(|profile| profile.id == settings.active_profile_id && profile.enabled)
        .or_else(|| settings.provider_profiles.iter().find(|profile| profile.enabled))
        .ok_or_else(|| AppError::Config("No enabled AI provider profile configured".to_string()))
}

fn build_client(profile: &AiProviderProfile) -> AppResult<Client> {
    let adapter_kind = adapter_kind(&profile.provider_kind);
    let model = profile.model.clone();
    let mapped_model = model.clone();
    let api_key = profile.api_key.clone().filter(|value| !value.trim().is_empty());
    let base_url = profile.base_url.clone().filter(|value| !value.trim().is_empty());

    let resolver = ServiceTargetResolver::from_resolver_fn(
        move |service_target: genai::ServiceTarget| {
            let mut service_target = service_target;
            if let Some(api_key) = api_key.clone() {
                service_target.auth = AuthData::from_single(api_key);
            }
            if let Some(base_url) = base_url.clone() {
                service_target.endpoint = Endpoint::from_owned(base_url);
            }
            Ok(service_target)
        },
    );

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
    }
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

fn build_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let action = match request.action {
        AiAction::GenerateCommand => "根据自然语言需求生成 1 到 5 条 Shell 命令",
        AiAction::ExplainOutput => "解释最近终端输出并给出下一步建议",
        AiAction::ExplainSelected => "解释用户选中的终端文本并给出下一步建议",
        AiAction::AnalyzeError => "分析终端错误输出并给出排查步骤",
        AiAction::RepairFromSelection => "根据选中内容生成修复或排查命令",
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

fn parse_model_output(raw_text: &str) -> (String, Vec<AiCommandCard>) {
    let candidate = extract_json_object(raw_text).unwrap_or_else(|| raw_text.trim().to_string());
    match serde_json::from_str::<AiModelOutput>(&candidate) {
        Ok(output) => {
            let text = if output.text.trim().is_empty() {
                raw_text.trim().to_string()
            } else {
                output.text
            };
            (text, output.command_cards)
        }
        Err(_) => (raw_text.trim().to_string(), vec![]),
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
                Regex::new(r"(?i)(token|api[_-]?key|secret[_-]?key|access[_-]?key)\s*[:=]\s*[^\s;&|]+")
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
        "ls"
            | "pwd"
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

fn history_path(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    Ok(config::get_config_dir(app)?.join("ai-history.json"))
}

fn audit_path(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    Ok(config::get_config_dir(app)?.join("ai-audit.json"))
}

fn load_history(app: &AppHandle) -> AppResult<AiHistoryFile> {
    config::load_json(&history_path(app)?)
}

fn save_history(app: &AppHandle, history: &AiHistoryFile) -> AppResult<()> {
    config::save_json(&history_path(app)?, history)
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
    let mut history = load_history(app)?;
    if let Some(session) = history.sessions.iter_mut().find(|item| item.id == session_id) {
        session.updated_at = now.clone();
    } else {
        history.sessions.push(AiSession {
            id: session_id.to_string(),
            connection_id: request.connection_id.clone(),
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
        session_id: session_id.to_string(),
        role: AiMessageRole::User,
        content: request.user_input.clone(),
        created_at: now,
        command_cards: vec![],
    });
    save_history(app, &history)
}

fn append_message(app: &AppHandle, message: AiMessage) -> AppResult<()> {
    let mut history = load_history(app)?;
    if let Some(session) = history
        .sessions
        .iter_mut()
        .find(|item| item.id == message.session_id)
    {
        session.updated_at = message.created_at.clone();
    }
    history.messages.push(message);
    save_history(app, &history)
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
    save_history(app, &AiHistoryFile::default())
}

pub fn append_ai_audit(app: &AppHandle, request: AppendAiAuditRequest) -> AppResult<AiAuditLog> {
    let mut file: AiAuditFile = config::load_json(&audit_path(app)?)?;
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
    file.logs.push(log.clone());
    if file.logs.len() > 2_000 {
        let keep_from = file.logs.len().saturating_sub(2_000);
        file.logs = file.logs.split_off(keep_from);
    }
    config::save_json(&audit_path(app)?, &file)?;
    Ok(log)
}

pub fn get_ai_audit_logs(app: &AppHandle, limit: Option<usize>) -> AppResult<Vec<AiAuditLog>> {
    let mut logs: Vec<AiAuditLog> = config::load_json::<AiAuditFile>(&audit_path(app)?)?.logs;
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
        let (text, cards) = parse_model_output(raw);
        assert_eq!(text, "ok");
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn parse_failure_returns_text_without_cards() {
        let (text, cards) = parse_model_output("plain text");
        assert_eq!(text, "plain text");
        assert!(cards.is_empty());
    }
}
