use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use nyaterm_core::ai::{
    ClaudeCodeStreamEvent, ClaudeCodeStreamParser, build_claude_code_invocation,
    sanitize_claude_code_log_line,
};
use nyaterm_core::{AiChatRequest, AiSettings};

use crate::external_process::ExternalAgentChild;

pub struct ClaudeCodeRunResult {
    pub text: String,
    pub external_session_id: Option<String>,
}

/// Runs synchronously on a caller-owned worker; callbacks execute on that worker.
/// The caller must keep injected MCP credentials valid until this returns or unwinds.
pub fn run_claude_code(
    settings: &AiSettings,
    request: &AiChatRequest,
    on_delta: &mut dyn FnMut(String, Option<String>),
    cancel: &AtomicBool,
    mcp_environment: std::collections::HashMap<String, String>,
    mcp_helper: &std::path::Path,
    on_session_id: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<ClaudeCodeRunResult, String> {
    if !settings.claude_code.enabled {
        return Err("Claude Code integration is disabled".to_string());
    }
    let executable = settings
        .claude_code
        .executable_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("claude");
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "nyaterm": {
                "command": mcp_helper,
                "args": [],
                "env": mcp_environment
            }
        }
    });
    let invocation = build_claude_code_invocation(request, settings, Some(&mcp_config));
    let mut command = Command::new(executable);
    command.args(&invocation.args);
    for (key, value) in &invocation.env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ExternalAgentChild::spawn(&mut command)
        .map_err(|error| format!("failed to start Claude Code: {error}"))?;
    let mut stdin = child
        .take_stdin()
        .ok_or_else(|| "Claude Code stdin is unavailable".to_string())?;
    stdin
        .write_all(invocation.prompt_stdin.as_bytes())
        .map_err(|error| format!("failed to write Claude Code prompt: {error}"))?;
    drop(stdin);
    let stdout = child
        .take_stdout()
        .ok_or_else(|| "Claude Code stdout is unavailable".to_string())?;
    let stderr = child
        .take_stderr()
        .ok_or_else(|| "Claude Code stderr is unavailable".to_string())?;
    let line_rx = child
        .capture_output("nyaterm-claude-code", stdout, stderr, |line| {
            let sanitized = sanitize_claude_code_log_line(&line);
            if !sanitized.trim().is_empty() {
                tracing::debug!(target: "claude_code", message = %sanitized);
            }
        })
        .map_err(|error| format!("failed to read Claude Code output: {error}"))?;

    collect_response(
        request,
        on_delta,
        cancel,
        on_session_id,
        &mut child,
        &line_rx,
    )
}

fn collect_response(
    request: &AiChatRequest,
    on_delta: &mut dyn FnMut(String, Option<String>),
    cancel: &AtomicBool,
    on_session_id: &mut dyn FnMut(&str) -> Result<(), String>,
    child: &mut ExternalAgentChild,
    line_rx: &mpsc::Receiver<Result<String, String>>,
) -> Result<ClaudeCodeRunResult, String> {
    let mut parser = ClaudeCodeStreamParser::default();
    let mut content = String::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.child_mut().kill();
            break Err("AI request cancelled".to_string());
        }
        match line_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(line)) => {
                let events = match parser.parse_line(&line) {
                    Ok(events) => events,
                    Err(_) => continue,
                };
                let mut error = None;
                for event in events {
                    match event {
                        ClaudeCodeStreamEvent::TextDelta(delta) => {
                            content.push_str(&delta);
                            on_delta(delta, None);
                        }
                        ClaudeCodeStreamEvent::Error(message) => error = Some(message),
                        ClaudeCodeStreamEvent::SessionId(external_session_id) => {
                            on_session_id(&external_session_id)?;
                        }
                        ClaudeCodeStreamEvent::Ignored => {}
                    }
                }
                if let Some(error) = error {
                    break Err(error);
                }
            }
            Ok(Err(error)) => break Err(error),
            Err(
                error @ (mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected),
            ) => {
                if let Some(status) = child
                    .child_mut()
                    .try_wait()
                    .map_err(|error| error.to_string())?
                {
                    if !status.success() {
                        break Err(format!("Claude Code exited with {status}"));
                    }
                    break Ok(ClaudeCodeRunResult {
                        text: content,
                        external_session_id: parser
                            .session_id()
                            .map(ToOwned::to_owned)
                            .or_else(|| request.existing_external_session_id.clone()),
                    });
                }
                // Closing stdout does not mean the CLI has exited. Keep polling
                // cancellation instead of blocking in Child::wait indefinitely.
                if error == mpsc::RecvTimeoutError::Disconnected {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

impl std::fmt::Debug for ClaudeCodeRunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCodeRunResult")
            .field("content", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use nyaterm_core::{
        AiAction, AiAgentKind, AiChatRequest, AiMode, AiPermissionMode, AiSettings,
    };

    use super::run_claude_code;

    #[test]
    fn fake_claude_cli_accepts_localized_system_arguments_on_windows() {
        let (_script_dir, script) = fake_claude_script(false);
        for language in ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"] {
            let (settings, mut request) = fixture_request(&script);
            request.options.language = language.into();
            let completion = run_claude_code(
                &settings,
                &request,
                &mut |_, _| {},
                &Arc::new(AtomicBool::new(false)),
                fixture_mcp_environment(),
                &script,
                &mut |_| Ok(()),
            )
            .unwrap_or_else(|error| panic!("{language}: {error}"));
            assert_eq!(completion.text, "hello world");
        }
    }

    #[test]
    fn fake_claude_cli_streams_resumes_and_cancels() {
        let (_script_dir, script) = fake_claude_script(false);
        let (settings, request) = fixture_request(&script);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut deltas = Vec::new();
        let result = run_claude_code(
            &settings,
            &request,
            &mut |text, reasoning| {
                assert!(reasoning.is_none());
                deltas.push(text);
            },
            &cancel,
            fixture_mcp_environment(),
            &script,
            &mut |_| Ok(()),
        )
        .expect("fake Claude completion");
        assert_eq!(deltas.concat(), "hello world");
        assert_eq!(result.text, "hello world");
        assert_eq!(
            result.external_session_id.as_deref(),
            Some("claude-session-1")
        );

        let (_hanging_dir, hanging) = fake_claude_script(true);
        let (settings, request) = fixture_request(&hanging);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = cancel.clone();
        let error = std::thread::scope(|scope| {
            scope.spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                cancel_worker.store(true, Ordering::Relaxed);
            });
            run_claude_code(
                &settings,
                &request,
                &mut |_, _| {},
                &cancel,
                fixture_mcp_environment(),
                &hanging,
                &mut |_| Ok(()),
            )
            .unwrap_err()
        });
        assert!(error.contains("cancelled"));
    }

    #[test]
    fn session_persistence_failure_stops_stream_delivery() {
        let (_directory, script) = fake_claude_script(false);
        let (settings, request) = fixture_request(&script);
        let mut deltas = Vec::new();
        let mut sessions = Vec::new();
        let error = run_claude_code(
            &settings,
            &request,
            &mut |text, _| deltas.push(text),
            &Arc::new(AtomicBool::new(false)),
            fixture_mcp_environment(),
            &script,
            &mut |session| {
                sessions.push(session.to_string());
                Err("persistence fixture failed".into())
            },
        )
        .unwrap_err();
        assert_eq!(error, "persistence fixture failed");
        assert_eq!(sessions, ["claude-session-1"]);
        assert!(deltas.is_empty());
    }

    #[test]
    fn early_failure_is_reported_and_empty_success_keeps_resume_metadata() {
        for success in [false, true] {
            let (_directory, script) = fake_claude_script(false);
            #[cfg(windows)]
            let source = format!(
                "@echo off\r\nmore >nul\r\nexit /b {}\r\n",
                if success { 0 } else { 7 }
            );
            #[cfg(not(windows))]
            let source = format!(
                "#!/bin/sh\ncat >/dev/null\nexit {}\n",
                if success { 0 } else { 7 }
            );
            std::fs::write(&script, source).unwrap();
            let (settings, request) = fixture_request(&script);
            let result = run_claude_code(
                &settings,
                &request,
                &mut |_, _| {},
                &Arc::new(AtomicBool::new(false)),
                fixture_mcp_environment(),
                &script,
                &mut |_| Ok(()),
            );
            if success {
                let result = result.unwrap();
                assert!(result.text.is_empty());
                assert_eq!(result.external_session_id.as_deref(), Some("claude-resume"));
                assert!(!format!("{result:?}").contains("claude-resume"));
            } else {
                assert!(result.unwrap_err().contains("Claude Code exited with"));
            }
        }
    }

    #[test]
    fn closed_stdout_still_polls_cancellation_until_child_exit() {
        let (_directory, script) = fake_claude_script(true);
        let (_, request) = fixture_request(&script);
        let mut command = std::process::Command::new(script);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = crate::external_process::ExternalAgentChild::spawn(&mut command).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx); // EOF while the child is still running, as with a CLI closing stdout.
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::Relaxed);
            });
            let error = super::collect_response(
                &request,
                &mut |_, _| {},
                &cancel,
                &mut |_| Ok(()),
                &mut child,
                &rx,
            )
            .unwrap_err();
            assert_eq!(error, "AI request cancelled");
        });
        drop(child);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    fn fixture_request(path: &std::path::Path) -> (AiSettings, AiChatRequest) {
        let mut settings = AiSettings::default();
        settings.claude_code.enabled = true;
        settings.claude_code.executable_path = Some(path.to_string_lossy().into_owned());
        let request = AiChatRequest {
            stream_id: None,
            session_id: Some("chat-1".into()),
            connection_id: Some("terminal-1".into()),
            terminal_session_id: Some("terminal-1".into()),
            owner_scope: Default::default(),
            targets: Vec::new(),
            target_contexts: Vec::new(),
            mode: AiMode::Agent,
            agent_kind: AiAgentKind::ClaudeCode,
            permission_mode: AiPermissionMode::Confirm,
            model_id: None,
            model_name: Some("claude-fixture".into()),
            default_target_session_id: Some("terminal-1".into()),
            existing_external_session_id: Some("claude-resume".into()),
            attachments: Vec::new(),
            action: AiAction::GenerateCommand,
            user_input: "inspect".into(),
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

    fn fake_claude_script(
        hang: bool,
    ) -> (nyaterm_core::test_support::TestTempDir, std::path::PathBuf) {
        let directory = nyaterm_core::test_support::TestTempDir::new("nyaterm-fake-claude");
        std::fs::create_dir_all(directory.path()).unwrap();
        #[cfg(windows)]
        let path = directory.join("claude.cmd");
        #[cfg(not(windows))]
        let path = directory.join("claude");
        #[cfg(windows)]
        let content = if hang {
            "@echo off\r\nmore >nul\r\nfor /L %%i in (1,1,2147483647) do @rem\r\n".to_string()
        } else {
            "@echo off\r\nmore >nul\r\necho {\"session_id\":\"claude-session-1\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\r\necho {\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello world\"}]}}\r\n".to_string()
        };
        #[cfg(not(windows))]
        let content = if hang {
            "#!/bin/sh\ncat >/dev/null\nwhile :; do :; done\n".to_string()
        } else {
            "#!/bin/sh\ncat >/dev/null\necho '{\"session_id\":\"claude-session-1\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}'\necho '{\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello world\"}]}}'\n".to_string()
        };
        std::fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        (directory, path)
    }
}
