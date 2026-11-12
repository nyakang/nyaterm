use std::io::{BufWriter, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use nyaterm_core::ai::{
    CodexMcpConfig, CodexServerEvent, build_codex_agent_prompt, codex_decline_server_request,
    codex_initialize_request, codex_initialized_notification,
    codex_thread_resume_request_with_language, codex_thread_start_request_with_language,
    codex_turn_interrupt_request, codex_turn_start_request, parse_codex_server_line,
    sanitize_codex_log_line,
};
use nyaterm_core::{AiChatRequest, AiSettings, CodexThreadMode};
use serde_json::Value;

use crate::external_process::ExternalAgentChild;

pub struct CodexRunResult {
    pub text: String,
    pub reasoning: Option<String>,
    pub thread_id: String,
}

/// Runs synchronously on a caller-owned worker; callbacks execute on that worker.
/// The caller must keep injected MCP credentials valid until this returns or unwinds.
pub fn run_codex_app_server(
    settings: &AiSettings,
    request: &AiChatRequest,
    existing_thread_id: Option<&str>,
    on_delta: &mut dyn FnMut(String, Option<String>),
    cancel: &AtomicBool,
    mcp_environment: std::collections::HashMap<String, String>,
    mcp_helper: &std::path::Path,
) -> Result<CodexRunResult, String> {
    if !settings.codex.enabled {
        return Err("Codex integration is disabled".to_string());
    }
    let executable =
        crate::executable::resolve_codex_executable(settings.codex.executable_path.as_deref())?;
    let mut command = Command::new(executable);
    command
        .args(["app-server", "--listen", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ExternalAgentChild::spawn(&mut command)
        .map_err(|error| format!("failed to start Codex app-server: {error}"))?;
    let stdin = child
        .take_stdin()
        .ok_or_else(|| "Codex app-server stdin is unavailable".to_string())?;
    let stdout = child
        .take_stdout()
        .ok_or_else(|| "Codex app-server stdout is unavailable".to_string())?;
    let stderr = child
        .take_stderr()
        .ok_or_else(|| "Codex app-server stderr is unavailable".to_string())?;
    let mut writer = BufWriter::new(stdin);
    let line_rx = child
        .capture_output("nyaterm-codex", stdout, stderr, |line| {
            let sanitized = sanitize_codex_log_line(&line);
            if !sanitized.trim().is_empty() {
                tracing::debug!(target: "codex_app_server", message = %sanitized);
            }
        })
        .map_err(|error| format!("failed to read Codex app-server output: {error}"))?;

    run_protocol(
        settings,
        request,
        existing_thread_id,
        on_delta,
        cancel,
        &mcp_environment,
        mcp_helper,
        child.child_mut(),
        &mut writer,
        &line_rx,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_protocol(
    settings: &AiSettings,
    request: &AiChatRequest,
    existing_thread_id: Option<&str>,
    on_delta: &mut dyn FnMut(String, Option<String>),
    cancel: &AtomicBool,
    mcp_environment: &std::collections::HashMap<String, String>,
    mcp_helper: &std::path::Path,
    child: &mut Child,
    writer: &mut BufWriter<ChildStdin>,
    lines: &mpsc::Receiver<Result<String, String>>,
) -> Result<CodexRunResult, String> {
    send(
        writer,
        &codex_initialize_request(1, env!("CARGO_PKG_VERSION")),
    )?;
    wait_response(1, writer, lines, cancel, child)?;
    send(writer, &codex_initialized_notification())?;

    let model = request
        .model_name
        .as_deref()
        .or(settings.codex.default_model.as_deref());
    let mcp = CodexMcpConfig {
        command: mcp_helper.to_string_lossy().into_owned(),
        env: mcp_environment.clone(),
    };
    let reusable = existing_thread_id.filter(|value| !value.trim().is_empty());
    let thread_id = if let Some(thread_id) = reusable {
        send(
            writer,
            &codex_thread_resume_request_with_language(2, thread_id, &request.options.language),
        )?;
        wait_response(2, writer, lines, cancel, child)?;
        thread_id.to_string()
    } else {
        send(
            writer,
            &codex_thread_start_request_with_language(
                2,
                model,
                settings.codex.thread_mode == CodexThreadMode::Ephemeral,
                &mcp,
                &request.options.language,
            ),
        )?;
        let result = wait_response(2, writer, lines, cancel, child)?;
        result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .or_else(|| result.get("id").and_then(Value::as_str))
            .ok_or_else(|| "Codex thread/start returned no thread id".to_string())?
            .to_string()
    };

    let prompt = build_codex_agent_prompt(request, settings);
    send(
        writer,
        &codex_turn_start_request(3, &thread_id, &prompt, model),
    )?;
    let turn = wait_response(3, writer, lines, cancel, child)?;
    let turn_id = turn
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex turn/start returned no turn id".to_string())?
        .to_string();

    let mut text = String::new();
    let mut reasoning = String::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = send(
                writer,
                &codex_turn_interrupt_request(4, &thread_id, &turn_id),
            );
            return Err("AI request cancelled".to_string());
        }
        let line = match lines.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => line?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if child
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    return Err("Codex app-server exited before completing the turn".to_string());
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Codex app-server output closed".to_string());
            }
        };
        match parse_codex_server_line(&line)? {
            CodexServerEvent::ServerRequest { id, method, .. } => {
                send(writer, &codex_decline_server_request(id, &method))?;
            }
            CodexServerEvent::TextDelta {
                turn_id: event_turn,
                delta,
            } if event_turn == turn_id => {
                text.push_str(&delta);
                on_delta(delta, None);
            }
            CodexServerEvent::ReasoningDelta {
                turn_id: event_turn,
                delta,
            } if event_turn == turn_id => {
                reasoning.push_str(&delta);
                on_delta(String::new(), Some(delta));
            }
            CodexServerEvent::TurnCompleted {
                turn_id: event_turn,
                status,
                text: final_text,
                error,
                ..
            } if event_turn == turn_id => {
                if status == "failed" {
                    return Err(error.unwrap_or_else(|| "Codex turn failed".to_string()));
                }
                if !final_text.is_empty() {
                    text = final_text;
                }
                if looks_like_command_plan(&text) {
                    return Err("Codex returned a command plan instead of using NyaTerm MCP; no command was executed".to_string());
                }
                return Ok(CodexRunResult {
                    text,
                    reasoning: (!reasoning.is_empty()).then_some(reasoning),
                    thread_id,
                });
            }
            CodexServerEvent::Error(error) => return Err(error),
            _ => {}
        }
    }
}

fn wait_response(
    expected_id: u64,
    writer: &mut BufWriter<ChildStdin>,
    lines: &mpsc::Receiver<Result<String, String>>,
    cancel: &AtomicBool,
    child: &mut Child,
) -> Result<Value, String> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("AI request cancelled".to_string());
        }
        let line = match lines.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => line?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if child
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    return Err("Codex app-server exited during a request".to_string());
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Codex app-server output closed".to_string());
            }
        };
        match parse_codex_server_line(&line)? {
            CodexServerEvent::Response { id, result } if id == expected_id => return result,
            CodexServerEvent::ServerRequest { id, method, .. } => {
                send(writer, &codex_decline_server_request(id, &method))?;
            }
            CodexServerEvent::Error(error) => return Err(error),
            _ => {}
        }
    }
}

fn send(writer: &mut BufWriter<ChildStdin>, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

fn looks_like_command_plan(text: &str) -> bool {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .and_then(|value| value.get("commands").and_then(Value::as_array).cloned())
        .is_some_and(|commands| !commands.is_empty())
}

impl std::fmt::Debug for CodexRunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexRunResult")
            .field("content", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use nyaterm_core::{AiChatRequest, AiSettings};

    use super::{looks_like_command_plan, run_codex_app_server};

    #[test]
    fn rejects_command_plan_fallback() {
        assert!(looks_like_command_plan(
            r#"{"commands":[{"command":"uptime"}]}"#
        ));
        assert!(!looks_like_command_plan("system is healthy"));
    }

    #[test]
    fn fake_app_server_streams_resumes_and_cancels() {
        let (_script_dir, script) = fake_codex_script(false);
        let (settings, request) = fixture_request(&script);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut deltas = Vec::new();
        let first = run_codex_app_server(
            &settings,
            &request,
            None,
            &mut |text, reasoning| deltas.push((text, reasoning)),
            &cancel,
            fixture_mcp_environment(),
            &script,
        )
        .expect("fake Codex completion");
        assert_eq!(
            deltas,
            vec![
                (String::new(), Some("thinking".into())),
                ("partial".into(), None)
            ]
        );
        assert!(!format!("{first:?}").contains("final answer"));
        assert_eq!(first.thread_id, "thread-fixture");
        assert_eq!(first.text, "final answer");
        assert_eq!(first.reasoning.as_deref(), Some("thinking"));

        let resumed = run_codex_app_server(
            &settings,
            &request,
            Some("thread-existing"),
            &mut |_, _| {},
            &cancel,
            fixture_mcp_environment(),
            &script,
        )
        .expect("fake Codex resume");
        assert_eq!(resumed.thread_id, "thread-existing");

        let (_hanging_dir, hanging_script) = fake_codex_script(true);
        let (settings, request) = fixture_request(&hanging_script);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = cancel.clone();
        let error = std::thread::scope(|scope| {
            scope.spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                cancel_worker.store(true, Ordering::Relaxed);
            });
            run_codex_app_server(
                &settings,
                &request,
                None,
                &mut |_, _| {},
                &cancel,
                fixture_mcp_environment(),
                &hanging_script,
            )
            .unwrap_err()
        });
        assert!(error.contains("cancelled"));
    }

    #[test]
    fn output_eof_before_turn_completion_is_an_error() {
        let (_directory, script) = fake_codex_script(true);
        let source = std::fs::read_to_string(&script).unwrap();
        let source = source
            .replace("for /L %%i in (1,1,2147483647) do @rem", "exit /b 0")
            .replace("while :; do :; done", "exit 0");
        std::fs::write(&script, source).unwrap();
        let (settings, request) = fixture_request(&script);
        assert!(
            run_codex_app_server(
                &settings,
                &request,
                None,
                &mut |_, _| {},
                &Arc::new(AtomicBool::new(false)),
                fixture_mcp_environment(),
                &script
            )
            .is_err()
        );
    }

    fn fixture_request(path: &std::path::Path) -> (AiSettings, AiChatRequest) {
        use nyaterm_core::{AiAction, AiAgentKind, AiMode, AiPermissionMode};
        let mut settings = AiSettings::default();
        settings.codex.enabled = true;
        settings.codex.executable_path = Some(path.to_string_lossy().into_owned());
        let request = AiChatRequest {
            stream_id: None,
            session_id: Some("chat-1".into()),
            connection_id: Some("terminal-1".into()),
            terminal_session_id: Some("terminal-1".into()),
            owner_scope: Default::default(),
            targets: Vec::new(),
            target_contexts: Vec::new(),
            mode: AiMode::Agent,
            agent_kind: AiAgentKind::Codex,
            permission_mode: AiPermissionMode::Confirm,
            model_id: None,
            model_name: Some("gpt-5-codex".into()),
            default_target_session_id: Some("terminal-1".into()),
            existing_external_session_id: None,
            attachments: Vec::new(),
            action: AiAction::GenerateCommand,
            user_input: "inspect fixture".into(),
            context: Default::default(),
            options: Default::default(),
        };
        (settings, request)
    }

    fn fixture_mcp_environment() -> std::collections::HashMap<String, String> {
        std::collections::HashMap::from([
            ("NYATERM_MCP_EPHEMERAL".into(), "1".into()),
            ("NYATERM_MCP_HOST".into(), "127.0.0.1".into()),
            ("NYATERM_MCP_PORT".into(), "1".into()),
            ("NYATERM_MCP_TOKEN".into(), "fixture-token".into()),
            ("NYATERM_MCP_GENERATION".into(), "fixture-generation".into()),
        ])
    }

    fn fake_codex_script(
        hang: bool,
    ) -> (nyaterm_core::test_support::TestTempDir, std::path::PathBuf) {
        let directory = nyaterm_core::test_support::TestTempDir::new("nyaterm-fake-codex");
        std::fs::create_dir_all(directory.path()).unwrap();
        #[cfg(windows)]
        let path = directory.join("codex.cmd");
        #[cfg(not(windows))]
        let path = directory.join("codex");
        #[cfg(windows)]
        let content = format!(
            "@echo off\r\necho {{\"id\":1,\"result\":{{}}}}\r\necho {{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-fixture\"}}}}}}\r\necho {{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-fixture\"}}}}}}\r\necho {{\"method\":\"item/reasoning/delta\",\"params\":{{\"turnId\":\"turn-fixture\",\"delta\":\"thinking\"}}}}\r\necho {{\"method\":\"item/agentMessage/delta\",\"params\":{{\"turnId\":\"turn-fixture\",\"delta\":\"partial\"}}}}\r\n{}",
            if hang {
                "for /L %%i in (1,1,2147483647) do @rem\r\n"
            } else {
                "echo {\"method\":\"turn/completed\",\"params\":{\"turn\":{\"id\":\"turn-fixture\",\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"text\":\"final answer\"}]}}}\r\n"
            }
        );
        // Keep the Unix fixture alive until each client message arrives; exiting after eager
        // responses races the client's writes and intermittently turns them into EPIPE.
        #[cfg(not(windows))]
        let content = format!(
            "#!/bin/sh\nIFS= read -r request || exit 1\necho '{{\"id\":1,\"result\":{{}}}}'\nIFS= read -r notification || exit 1\nIFS= read -r request || exit 1\necho '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-fixture\"}}}}}}'\nIFS= read -r request || exit 1\necho '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-fixture\"}}}}}}'\necho '{{\"method\":\"item/reasoning/delta\",\"params\":{{\"turnId\":\"turn-fixture\",\"delta\":\"thinking\"}}}}'\necho '{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"turnId\":\"turn-fixture\",\"delta\":\"partial\"}}}}'\n{}",
            if hang {
                "while :; do :; done\n"
            } else {
                "echo '{\"method\":\"turn/completed\",\"params\":{\"turn\":{\"id\":\"turn-fixture\",\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"text\":\"final answer\"}]}}}'\n"
            }
        );
        std::fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        (directory, path)
    }
}
