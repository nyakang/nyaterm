use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use nyaterm_core::ai::{
    ClaudeCodeStreamEvent, ClaudeCodeStreamParser, build_claude_code_invocation,
    sanitize_claude_code_log_line,
};
use nyaterm_core::{AiChatRequest, AiSettings};

use crate::external_process::ExternalAgentChild;
use crate::features::mcp::McpEphemeralCredential;
use crate::features::runtime_jobs::AiChatWorkerEvent;

use super::helper_resolver::resolve_mcp_helper;

#[derive(Debug)]
pub(in crate::features) struct ClaudeCodeRunResult {
    pub text: String,
    pub external_session_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::features) fn run_claude_code(
    settings: &AiSettings,
    request: &AiChatRequest,
    stream_tx: Option<&UnboundedSender<AiChatWorkerEvent>>,
    cancel: &Arc<AtomicBool>,
    job_id: u64,
    session_id: &str,
    mcp_credential: McpEphemeralCredential,
    mut on_session_id: impl FnMut(&str) -> Result<(), String>,
) -> Result<ClaudeCodeRunResult, String> {
    if !settings.claude_code.enabled {
        return Err("Claude Code integration is disabled".to_string());
    }
    let environment = mcp_credential.environment();
    let mcp_helper = resolve_mcp_helper()?;
    let result = run_claude_code_with_environment(
        settings,
        request,
        stream_tx,
        cancel,
        job_id,
        session_id,
        environment,
        &mcp_helper,
        &mut on_session_id,
    );
    drop(mcp_credential);
    result
}

#[allow(clippy::too_many_arguments)]
fn run_claude_code_with_environment(
    settings: &AiSettings,
    request: &AiChatRequest,
    stream_tx: Option<&UnboundedSender<AiChatWorkerEvent>>,
    cancel: &Arc<AtomicBool>,
    job_id: u64,
    session_id: &str,
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
                            if let Some(tx) = stream_tx {
                                let _ = tx.unbounded_send(AiChatWorkerEvent::Delta {
                                    job_id,
                                    session_id: session_id.to_string(),
                                    text_delta: delta,
                                    reasoning_delta: None,
                                });
                            }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use nyaterm_core::{
        AiAction, AiAgentKind, AiChatRequest, AiMode, AiPermissionMode, AiSettings,
    };

    use super::run_claude_code_with_environment;

    #[test]
    fn fake_claude_cli_accepts_localized_system_arguments_on_windows() {
        let (_script_dir, script) = fake_claude_script(false);
        for language in ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"] {
            let (settings, mut request) = fixture_request(&script);
            request.options.language = language.into();
            let completion = run_claude_code_with_environment(
                &settings,
                &request,
                None,
                &Arc::new(AtomicBool::new(false)),
                1,
                "chat-1",
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
        let result = run_claude_code_with_environment(
            &settings,
            &request,
            None,
            &cancel,
            1,
            "chat-1",
            fixture_mcp_environment(),
            &script,
            &mut |_| Ok(()),
        )
        .expect("fake Claude completion");
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
            run_claude_code_with_environment(
                &settings,
                &request,
                None,
                &cancel,
                2,
                "chat-1",
                fixture_mcp_environment(),
                &hanging,
                &mut |_| Ok(()),
            )
            .unwrap_err()
        });
        assert!(error.contains("cancelled"));
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
