use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use nyaterm_mcp_protocol::MCP_TOOL_REGISTRY;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;

use crate::config::{AiAgentKind, AiPermissionMode, AiSettings};
use crate::error::{AppError, AppResult};
use crate::utils::process::hide_window;

use super::super::history::{append_message, save_user_message, set_session_external_session_id};
use super::super::prompt::build_prompt;
use super::super::redaction::{redact_context, redact_marker_values, redact_sensitive_text};
use super::super::stream::{active_streams, emit_stream_event};
use super::super::types::{AiChatRequest, AiMessage, AiMessageRole, AiStreamEventPayload};
use super::super::types::{now_rfc3339, uuid};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeCliStatus {
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
pub struct ClaudeCodeAccountStatus {
    pub connected: bool,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Default)]
pub struct ClaudeCodeRuntime {
    active_turns: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl ClaudeCodeRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn detect_cli(path: Option<String>) -> ClaudeCodeCliStatus {
        let candidates = discover_claude_candidates(path.as_deref()).await;
        let checked_paths = candidates
            .iter()
            .map(|candidate| candidate.executable.clone())
            .collect::<Vec<_>>();
        let mut errors = Vec::new();

        for candidate in &candidates {
            match probe_claude_cli(&candidate.executable).await {
                Ok(version) => {
                    return ClaudeCodeCliStatus {
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

        ClaudeCodeCliStatus {
            installed: false,
            path: path
                .as_deref()
                .map(|value| claude_executable(Some(value)))
                .or_else(|| Some("claude".to_string())),
            version: None,
            error: Some(detect_error_message(
                "Claude Code CLI was not detected",
                &errors,
            )),
            source: None,
            checked_paths,
        }
    }

    pub async fn auth_status(&self, settings: &AiSettings) -> AppResult<ClaudeCodeAccountStatus> {
        let status = Self::detect_cli(settings.claude_code.executable_path.clone()).await;
        Ok(ClaudeCodeAccountStatus {
            connected: status.installed,
            auth_mode: Some("claude_code".to_string()),
            message: status
                .installed
                .then(|| {
                    "Claude Code CLI is available; NyaTerm does not inspect OAuth tokens."
                        .to_string()
                })
                .or(status.error),
        })
    }

    pub async fn cancel_turn(&self, turn_id: &str) -> AppResult<()> {
        if let Some(sender) = self.active_turns.lock().await.remove(turn_id) {
            let _ = sender.send(());
        }
        Ok(())
    }
}

pub async fn run_claude_code_stream(
    app: AppHandle,
    runtime: Arc<ClaudeCodeRuntime>,
    stream_id: String,
    session_id: String,
    mut request: AiChatRequest,
    settings: AiSettings,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let result = run_claude_code_stream_inner(
        app.clone(),
        runtime,
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

async fn run_claude_code_stream_inner(
    app: AppHandle,
    runtime: Arc<ClaudeCodeRuntime>,
    stream_id: String,
    session_id: String,
    request: &mut AiChatRequest,
    settings: AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<()> {
    if !settings.claude_code.enabled {
        return Err(AppError::Config(
            "Claude Code integration is disabled".to_string(),
        ));
    }

    let cli = ClaudeCodeRuntime::detect_cli(settings.claude_code.executable_path.clone()).await;
    if !cli.installed {
        return Err(AppError::Config(
            cli.error
                .unwrap_or_else(|| "Claude Code CLI was not detected".to_string()),
        ));
    }
    let executable = cli.path.unwrap_or_else(|| claude_executable(None));

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

    let use_nyaterm_mcp =
        settings.claude_code.tool_integration_mode.as_deref() == Some("nyaterm_mcp");
    let mcp_credential = if use_nyaterm_mcp {
        let manager = app
            .try_state::<Arc<crate::core::mcp::McpManager>>()
            .ok_or_else(|| claude_mcp_unavailable("The NyaTerm MCP manager is unavailable."))?;
        let (scope, default_id) = request_terminal_scope(request);
        let owner = if let Some(id) = default_id.as_deref().or(scope.first().map(String::as_str)) {
            app.state::<Arc<crate::core::SessionManager>>()
                .session_info(id)
                .await
                .ok()
                .and_then(|info| info.owner_window_label)
        } else {
            None
        };
        Some(
            manager
                .create_ephemeral_credential(
                    "claude_code_mcp",
                    scope,
                    default_id,
                    request.permission_mode.clone(),
                    owner,
                )
                .await
                .map_err(|error| claude_mcp_unavailable(&error.to_string()))?,
        )
    } else {
        None
    };
    let mut invocation =
        build_claude_invocation(request, &settings, build_prompt(request, &settings));
    if let Some(credential) = mcp_credential.as_ref() {
        let config = serde_json::json!({
            "mcpServers": {
                "nyaterm": {
                    "command": credential.sidecar_path,
                    "args": [],
                    "env": credential.env(),
                }
            }
        });
        append_claude_mcp_args(&mut invocation.args, config);
    }
    let mut child = Command::new(&executable);
    hide_window(&mut child);
    for arg in &invocation.args {
        child.arg(arg);
    }
    for (key, value) in &invocation.env {
        child.env(key, value);
    }

    child.stdin(Stdio::piped());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    child.kill_on_drop(true);

    let mut child = child
        .spawn()
        .map_err(|error| AppError::Config(format!("Failed to start Claude Code: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::Channel("Claude Code stdin unavailable".to_string()))?;
    stdin
        .write_all(invocation.prompt_stdin.as_bytes())
        .await
        .map_err(|error| {
            AppError::Channel(format!("Failed to write Claude Code prompt: {error}"))
        })?;
    stdin.shutdown().await.map_err(|error| {
        AppError::Channel(format!("Failed to close Claude Code stdin: {error}"))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Channel("Claude Code stdout unavailable".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Channel("Claude Code stderr unavailable".to_string()))?;

    let turn_id = format!("claude-turn-{}", uuid());
    let (turn_cancel_tx, mut turn_cancel_rx) = oneshot::channel();
    runtime
        .active_turns
        .lock()
        .await
        .insert(turn_id.clone(), turn_cancel_tx);

    let stderr_task = tauri::async_runtime::spawn(read_claude_stderr(stderr));

    let mut lines = BufReader::new(stdout).lines();
    let mut content = String::new();
    let mut last_partial = String::new();
    let mut external_session_id = request.existing_external_session_id.clone();

    let loop_result: AppResult<()> = loop {
        tokio::select! {
            _ = &mut *cancel_rx => {
                let _ = child.kill().await;
                break Err(AppError::Cancelled("AI stream cancelled".to_string()));
            }
            _ = &mut turn_cancel_rx => {
                let _ = child.kill().await;
                break Err(AppError::Cancelled("Claude Code turn cancelled".to_string()));
            }
            line = lines.next_line() => {
                let Some(line) = line? else {
                    break Ok(());
                };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    tracing::debug!("Ignoring non-JSON Claude Code stream line");
                    continue;
                };

                if external_session_id.is_none() {
                    external_session_id = extract_session_id(&value);
                    if let Some(id) = external_session_id.clone() {
                        set_session_external_session_id(
                            &app,
                            &session_id,
                            AiAgentKind::ClaudeCode,
                            id,
                        )?;
                    }
                }

                if let Some(delta) = extract_text_delta(&value, &mut last_partial) {
                    content.push_str(&delta);
                    emit_stream_event(
                        &app,
                        &stream_id,
                        AiStreamEventPayload {
                            event_type: "delta".to_string(),
                            stream_id: stream_id.clone(),
                            session_id: Some(session_id.clone()),
                            text_delta: Some(delta),
                            reasoning_delta: None,
                            message: None,
                            command_cards: vec![],
                            usage: None,
                            error: None,
                        },
                    );
                }

                if let Some(error) = extract_error_message(&value) {
                    break Err(AppError::Config(error));
                }
            }
        }
    };

    runtime.active_turns.lock().await.remove(&turn_id);
    let status = child.wait().await.ok();
    let stderr_summary = stderr_task.await.unwrap_or_default();
    if let Err(error) = loop_result {
        if use_nyaterm_mcp && looks_like_mcp_error(&error.to_string(), &stderr_summary) {
            return Err(claude_mcp_unavailable(&error_details(
                &error.to_string(),
                &stderr_summary,
            )));
        }
        return Err(error);
    }
    if status.as_ref().is_some_and(|status| !status.success()) {
        let details = error_details(
            &format!("Claude Code exited with {}", status.as_ref().unwrap()),
            &stderr_summary,
        );
        if use_nyaterm_mcp && looks_like_mcp_error(&details, &stderr_summary) {
            return Err(claude_mcp_unavailable(&details));
        }
        return Err(AppError::Config(details));
    }

    active_streams().lock().unwrap().remove(&stream_id);
    let message = AiMessage {
        id: format!("msg-{}", uuid()),
        session_id: session_id.clone(),
        role: AiMessageRole::Assistant,
        content,
        created_at: now_rfc3339(),
        reasoning_content: None,
        command_cards: vec![],
    };
    if settings.record_history {
        append_message(&app, message.clone())?;
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

    Ok(())
}

fn claude_permission_mode(mode: &AiPermissionMode) -> &'static str {
    match mode {
        AiPermissionMode::Observer => "plan",
        AiPermissionMode::Confirm => "manual",
        AiPermissionMode::Auto => "auto",
        // Full access only bypasses NyaTerm's own capability approvals. Do not
        // widen Claude Code's native local-tool permissions.
        AiPermissionMode::FullAccess => "auto",
    }
}

fn claude_nyaterm_mcp_tool_names() -> Vec<String> {
    MCP_TOOL_REGISTRY
        .iter()
        .map(|definition| format!("mcp__nyaterm__{}", definition.tool))
        .collect()
}

fn append_claude_mcp_args(args: &mut Vec<String>, config: Value) {
    args.push("--mcp-config".into());
    args.push(config.to_string());
    args.push("--strict-mcp-config".into());
    args.push("--allowedTools".into());
    args.extend(claude_nyaterm_mcp_tool_names());
}

fn claude_mcp_unavailable(reason: &str) -> AppError {
    let reason = sanitize_claude_log_line(&redact_sensitive_text(reason));
    AppError::Config(format!("claude_mcp_unavailable:{}", reason))
}

fn looks_like_mcp_error(error: &str, stderr: &str) -> bool {
    let details = format!("{error}\n{stderr}").to_ascii_lowercase();
    details.contains("mcp") || details.contains("nyaterm")
}

fn error_details(error: &str, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() || error.contains(stderr) {
        error.to_string()
    } else {
        format!("{error}: {stderr}")
    }
}

fn claude_system_context(request: &AiChatRequest) -> String {
    let default_target = request
        .default_target_session_id
        .as_deref()
        .or(request.terminal_session_id.as_deref())
        .unwrap_or("none");
    format!(
        "You are running inside NyaTerm. Use NyaTerm MCP tools for terminal sessions when available. Do not read SSH passwords, private keys, OAuth tokens, or internal app credentials. Do not create separate SSH connections to bypass NyaTerm SessionManager. Default terminal session: {default_target}."
    )
}

fn request_terminal_scope(request: &AiChatRequest) -> (Vec<String>, Option<String>) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeInvocation {
    args: Vec<String>,
    env: Vec<(String, String)>,
    prompt_stdin: String,
}

fn build_claude_invocation(
    request: &AiChatRequest,
    settings: &AiSettings,
    prompt_stdin: String,
) -> ClaudeInvocation {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--include-partial-messages".to_string(),
        "--permission-mode".to_string(),
        claude_permission_mode(&request.permission_mode).to_string(),
        "--append-system-prompt".to_string(),
        claude_system_context(request),
    ];

    if let Some(model) = request
        .model_name
        .as_deref()
        .or(settings.claude_code.default_model.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    if let Some(session_id) = request
        .existing_external_session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        args.push("--resume".to_string());
        args.push(session_id.to_string());
    }

    let mut env = Vec::new();
    if let Some(config_dir) = settings
        .claude_code
        .config_directory
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        env.push(("CLAUDE_CONFIG_DIR".to_string(), config_dir.to_string()));
    }

    ClaudeInvocation {
        args,
        env,
        prompt_stdin,
    }
}

fn extract_session_id(value: &Value) -> Option<String> {
    value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn extract_error_message(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) == Some("error") {
        return value
            .get("message")
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    None
}

fn extract_text_delta(value: &Value, last_partial: &mut String) -> Option<String> {
    if let Some(delta) = value
        .pointer("/delta/text")
        .or_else(|| value.pointer("/delta/content"))
        .and_then(Value::as_str)
    {
        if !delta.is_empty() {
            return Some(delta.to_string());
        }
    }

    let full_text = extract_message_text(value)?;
    if full_text.is_empty() || full_text == *last_partial {
        return None;
    }
    let delta = full_text
        .strip_prefix(last_partial.as_str())
        .unwrap_or(full_text.as_str())
        .to_string();
    *last_partial = full_text;
    (!delta.is_empty()).then_some(delta)
}

fn extract_message_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let message = value.get("message").unwrap_or(value);
    let content = message.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|item| {
            if item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "text" | "output_text" | "assistant_text"))
            {
                item.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    Some(text)
}

const CLAUDE_DETECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
struct ClaudeCliCandidate {
    executable: String,
    source: &'static str,
}

fn claude_executable(path: Option<&str>) -> String {
    path.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("claude")
        .to_string()
}

async fn discover_claude_candidates(path: Option<&str>) -> Vec<ClaudeCliCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) {
        add_claude_candidate(&mut candidates, &mut seen, path, "configured");
    }
    add_claude_candidate(&mut candidates, &mut seen, "claude", "path");
    add_common_claude_candidates(&mut candidates, &mut seen);
    for discovered in discover_claude_with_path_command().await {
        add_claude_candidate(&mut candidates, &mut seen, discovered, "path_lookup");
    }

    candidates
}

fn add_claude_candidate(
    candidates: &mut Vec<ClaudeCliCandidate>,
    seen: &mut HashSet<String>,
    executable: impl AsRef<str>,
    source: &'static str,
) {
    let executable = executable.as_ref().trim();
    if executable.is_empty() {
        return;
    }
    let key = claude_candidate_key(executable);
    if seen.insert(key) {
        candidates.push(ClaudeCliCandidate {
            executable: executable.to_string(),
            source,
        });
    }
}

fn add_existing_claude_candidate(
    candidates: &mut Vec<ClaudeCliCandidate>,
    seen: &mut HashSet<String>,
    path: PathBuf,
    source: &'static str,
) {
    if path.exists() {
        add_claude_candidate(candidates, seen, path.to_string_lossy(), source);
    }
}

fn claude_candidate_key(executable: &str) -> String {
    #[cfg(windows)]
    {
        executable.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        executable.to_string()
    }
}

fn add_common_claude_candidates(
    candidates: &mut Vec<ClaudeCliCandidate>,
    seen: &mut HashSet<String>,
) {
    #[cfg(windows)]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            let npm = Path::new(&appdata).join("npm");
            for name in ["claude.cmd", "claude.exe", "claude"] {
                add_existing_claude_candidate(candidates, seen, npm.join(name), "common");
            }
        }
        if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
            let pnpm = Path::new(&local_appdata).join("pnpm");
            for name in ["claude.cmd", "claude.exe", "claude"] {
                add_existing_claude_candidate(candidates, seen, pnpm.join(name), "common");
            }
        }
        if let Ok(userprofile) = env::var("USERPROFILE") {
            let home = Path::new(&userprofile);
            for dir in [
                home.join("scoop").join("shims"),
                home.join(".bun").join("bin"),
            ] {
                for name in ["claude.cmd", "claude.exe", "claude"] {
                    add_existing_claude_candidate(candidates, seen, dir.join(name), "common");
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(home) = env::var("HOME") {
            let home = Path::new(&home);
            for path in [
                home.join(".local").join("bin").join("claude"),
                home.join(".npm-global").join("bin").join("claude"),
                home.join(".bun").join("bin").join("claude"),
            ] {
                add_existing_claude_candidate(candidates, seen, path, "common");
            }
        }
        for path in [
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
            PathBuf::from("/usr/bin/claude"),
        ] {
            add_existing_claude_candidate(candidates, seen, path, "common");
        }
    }
}

async fn discover_claude_with_path_command() -> Vec<String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("where.exe");
        command.arg("claude");
        command
    } else {
        let mut command = Command::new("which");
        command.args(["-a", "claude"]);
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

async fn probe_claude_cli(executable: &str) -> Result<String, String> {
    let mut command = Command::new(executable);
    command.arg("--version");
    hide_window(&mut command);
    let output = timeout(CLAUDE_DETECT_TIMEOUT, command.output())
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

fn detect_error_message(prefix: &str, errors: &[String]) -> String {
    if errors.is_empty() {
        return format!("{prefix} in PATH or common install locations");
    }

    let mut message = prefix.to_string();
    for error in errors.iter().take(4) {
        message.push_str("; ");
        message.push_str(error);
    }
    if errors.len() > 4 {
        message.push_str(&format!("; {} more candidates failed", errors.len() - 4));
    }
    message
}

async fn read_claude_stderr(stderr: tokio::process::ChildStderr) -> String {
    let mut lines = BufReader::new(stderr).lines();
    let mut recent = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let sanitized = sanitize_claude_log_line(&line);
        if !sanitized.trim().is_empty() {
            tracing::debug!(target: "claude_code", message = %sanitized);
            recent.push(sanitized.chars().take(512).collect::<String>());
            if recent.len() > 20 {
                recent.remove(0);
            }
        }
    }
    recent.join("\n")
}

fn sanitize_claude_log_line(line: &str) -> String {
    redact_marker_values(
        line,
        &[
            "access_token=",
            "refresh_token=",
            "id_token=",
            "api_key=",
            "code=",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_request() -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: None,
            connection_id: None,
            terminal_session_id: Some("term-1".to_string()),
            owner_scope: Default::default(),
            targets: vec![],
            target_contexts: vec![],
            mode: crate::config::AiMode::Agent,
            agent_kind: AiAgentKind::ClaudeCode,
            permission_mode: AiPermissionMode::Confirm,
            model_id: None,
            model_name: Some("claude-request-model".to_string()),
            default_target_session_id: Some("term-1".to_string()),
            existing_external_session_id: Some("claude-session-1".to_string()),
            attachments: vec![],
            action: crate::core::ai::types::AiAction::GenerateCommand,
            user_input: "inspect".to_string(),
            context: Default::default(),
            options: Default::default(),
        }
    }

    fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].as_str())
    }

    #[test]
    fn builds_claude_invocation_with_prompt_on_stdin_not_argv() {
        let request = test_request();
        let mut settings = AiSettings::default();
        settings.claude_code.default_model = Some("claude-default-model".to_string());
        settings.claude_code.config_directory = Some("C:\\Users\\test\\.claude".to_string());
        let prompt = "你好\nline1\nline2\n\"quoted\"\n'quoted'\n&\n|\n<\n>\n%\n^\n(\n)".to_string();

        let invocation = build_claude_invocation(&request, &settings, prompt.clone());

        assert_eq!(invocation.prompt_stdin, prompt);
        assert!(!invocation.args.iter().any(|arg| arg == &prompt));
        for required in [
            "-p",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--append-system-prompt",
        ] {
            assert!(invocation.args.iter().any(|arg| arg == required));
        }
        assert!(
            invocation
                .args
                .iter()
                .any(|arg| arg.contains("Default terminal session: term-1"))
        );
        assert_eq!(
            arg_value(&invocation.args, "--permission-mode"),
            Some("manual")
        );
        assert_eq!(
            arg_value(&invocation.args, "--model"),
            Some("claude-request-model")
        );
        assert_eq!(
            arg_value(&invocation.args, "--resume"),
            Some("claude-session-1")
        );
        assert_eq!(
            invocation.env,
            vec![(
                "CLAUDE_CONFIG_DIR".to_string(),
                "C:\\Users\\test\\.claude".to_string()
            )]
        );
    }

    #[test]
    fn builds_claude_invocation_with_default_model_when_request_model_is_empty() {
        let mut request = test_request();
        request.model_name = Some("  ".to_string());
        let mut settings = AiSettings::default();
        settings.claude_code.default_model = Some("claude-default-model".to_string());

        let invocation = build_claude_invocation(&request, &settings, "prompt".to_string());

        assert_eq!(
            arg_value(&invocation.args, "--model"),
            Some("claude-default-model")
        );
    }

    #[test]
    fn full_access_does_not_enable_claude_native_permission_bypass() {
        let mut request = test_request();
        request.permission_mode = AiPermissionMode::FullAccess;

        let invocation =
            build_claude_invocation(&request, &AiSettings::default(), "prompt".to_string());

        assert_eq!(
            arg_value(&invocation.args, "--permission-mode"),
            Some("auto")
        );
        assert!(!invocation.args.iter().any(|arg| arg == "bypassPermissions"));
    }

    #[test]
    fn claude_preapproves_only_registered_nyaterm_mcp_tools() {
        let mut args = Vec::new();
        append_claude_mcp_args(
            &mut args,
            serde_json::json!({ "mcpServers": { "nyaterm": {} } }),
        );

        assert!(args.iter().any(|arg| arg == "--strict-mcp-config"));
        let allowed_index = args
            .iter()
            .position(|arg| arg == "--allowedTools")
            .expect("allowed tools argument");
        assert_eq!(
            &args[allowed_index + 1..],
            claude_nyaterm_mcp_tool_names().as_slice()
        );
        assert_eq!(args[allowed_index + 1..].len(), MCP_TOOL_REGISTRY.len());
    }

    #[test]
    fn claude_mcp_errors_are_coded_and_sanitized() {
        let error = claude_mcp_unavailable("api_key=secret sidecar failed");
        let message = error.to_string();

        assert!(message.contains("claude_mcp_unavailable:"));
        assert!(!message.contains("secret"));
        assert!(message.contains("api_key=[redacted]"));
        assert!(looks_like_mcp_error("MCP initialization failed", ""));
        assert!(looks_like_mcp_error(
            "Claude Code exited with status 1",
            "Failed to connect to server nyaterm"
        ));
        assert_eq!(
            error_details("failed", "stderr details"),
            "failed: stderr details"
        );
    }

    #[test]
    fn extracts_delta_from_partial_message() {
        let mut last = String::new();
        let first = serde_json::json!({
            "message": { "content": [{ "type": "text", "text": "hello" }] }
        });
        let second = serde_json::json!({
            "message": { "content": [{ "type": "text", "text": "hello world" }] }
        });

        assert_eq!(
            extract_text_delta(&first, &mut last).as_deref(),
            Some("hello")
        );
        assert_eq!(
            extract_text_delta(&second, &mut last).as_deref(),
            Some(" world")
        );
    }

    #[test]
    fn sanitizes_claude_auth_material_from_logs() {
        let line = "auth access_token=abc refresh_token=def api_key=ghi code=xyz&state=ok";
        let sanitized = sanitize_claude_log_line(line);

        assert!(!sanitized.contains("abc"));
        assert!(!sanitized.contains("def"));
        assert!(!sanitized.contains("ghi"));
        assert!(!sanitized.contains("code=xyz"));
        assert!(sanitized.contains("access_token=[redacted]"));
        assert!(sanitized.contains("refresh_token=[redacted]"));
        assert!(sanitized.contains("api_key=[redacted]"));
        assert!(sanitized.contains("code=[redacted]&state=ok"));
    }
}
