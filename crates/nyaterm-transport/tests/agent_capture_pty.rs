//! Real interactive Bash, using the same queued writer as the application.
#![cfg(unix)]
use nyaterm_core::AiExecutionProfile;
use nyaterm_core::agent_capture::build_agent_capture_command;
use nyaterm_terminal::agent_capture::{AgentCapturedOutput, AgentOutputCaptureProcessor};
use nyaterm_transport::{LocalSessionConfig, SessionEvent, SessionManager};
use std::time::{Duration, Instant};

fn receive(
    manager: &SessionManager,
    capture: &mut AgentOutputCaptureProcessor,
    timeout: Duration,
) -> Option<AgentCapturedOutput> {
    let deadline = Instant::now() + timeout;
    let mut pending = Vec::new();
    while Instant::now() < deadline {
        for event in manager.drain_events(64).unwrap().events {
            if let SessionEvent::Output { data, .. } = event {
                pending.extend_from_slice(&data);
                let end = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(e) => e.valid_up_to(),
                };
                if end == 0 {
                    continue;
                }
                let result = capture.process(std::str::from_utf8(&pending[..end]).unwrap());
                assert!(
                    !result.visible_text.contains("__DF_CMD_"),
                    "markers must not become ordinary terminal text"
                );
                pending.drain(..end);
                if let Some(output) = result.completed.into_iter().next() {
                    return Some(output);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}
fn start(manager: &SessionManager, id: &str, command: &str) -> AgentOutputCaptureProcessor {
    let marker = nyaterm_core::uuid();
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register(marker.clone());
    let input = build_agent_capture_command(AiExecutionProfile::Posix, &marker, command).unwrap();
    manager.write(id, input.as_bytes()).unwrap();
    capture
}
#[test]
fn interactive_bash_preserves_heredocs_comments_quotes_exit_codes_and_shell_state() {
    let manager = SessionManager::new();
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("/bin/bash".into()),
            shell_args: vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            cols: 240,
            ..Default::default()
        })
        .unwrap();
    let cases = [
        (
            "cat <<'MCP_EOF'\nMCP_HEREDOC_ONLY\nMCP_EOF",
            "MCP_HEREDOC_ONLY",
            0,
        ),
        (
            "cat <<'MCP_EOF'\nMCP_HEREDOC_ONLY\nMCP_EOF\n",
            "MCP_HEREDOC_ONLY",
            0,
        ),
        (
            "cat <<'MCP_PROBE_EOF'\nMCP_QUOTE: 'single' \"double\" $literal \\\n中文日志：正常\nMCP_PROBE_EOF\nprintf 'MCP_HEREDOC_END\\n'\n# MCP_TRAILING_COMMENT",
            "MCP_QUOTE: 'single' \"double\" $literal \\\n中文日志：正常\nMCP_HEREDOC_END",
            0,
        ),
        (
            "printf 'LINE_ONE\\n'\nprintf 'LINE_TWO\\n'",
            "LINE_ONE\nLINE_TWO",
            0,
        ),
        ("false", "", 1),
        ("", "", 0),
        ("# only a comment", "", 0),
        (
            "printf '%s\\n' \\\n 'single' \"double\" '\\'",
            "single\ndouble\n\\",
            0,
        ),
        ("cat <<'A'\none\nA\ncat <<'B'\ntwo\nB", "one\ntwo", 0),
        (
            "MCP_PERSISTED='中文'; printf '%s\\n' \"$MCP_PERSISTED\"",
            "中文",
            0,
        ),
        ("printf '%s\\n' \"$MCP_PERSISTED\"", "中文", 0),
    ];
    for (command, expected, code) in cases {
        let mut capture = start(&manager, &session.id, command);
        let output = receive(&manager, &mut capture, Duration::from_secs(5))
            .expect("interactive command must finish");
        assert_eq!(output.output.replace("\r\n", "\n"), expected);
        assert_eq!(output.exit_code, Some(code));
        assert!(!output.source_truncated);
    }
    let mut capture = start(
        &manager,
        &session.id,
        "for n in $(seq 1 1500); do printf '中文:%s\\n' \"$n\"; done",
    );
    let output = receive(&manager, &mut capture, Duration::from_secs(10)).unwrap();
    assert_eq!(output.output.lines().count(), 1500);
    assert_eq!(output.exit_code, Some(0));
    manager.close(&session.id).unwrap();
}
#[test]
fn local_wait_timeout_does_not_end_the_remote_command_and_manual_interrupt_restores_bash() {
    let manager = SessionManager::new();
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("/bin/bash".into()),
            shell_args: vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            ..Default::default()
        })
        .unwrap();
    let mut capture = start(
        &manager,
        &session.id,
        "printf 'BEGIN\\n'; sleep 3; printf 'END\\n'",
    );
    assert!(receive(&manager, &mut capture, Duration::from_millis(250)).is_none());
    let output = receive(&manager, &mut capture, Duration::from_secs(5)).unwrap();
    assert_eq!(output.output.replace("\r\n", "\n"), "BEGIN\nEND");
    assert_eq!(output.exit_code, Some(0));
    let mut incomplete = start(&manager, &session.id, "cat <<'MISSING_EOF'\nwaiting");
    assert!(receive(&manager, &mut incomplete, Duration::from_millis(250)).is_none());
    manager.write(&session.id, b"\x03").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let mut after = start(&manager, &session.id, "printf 'AFTER_INTERRUPT\\n'");
    assert_eq!(
        receive(&manager, &mut after, Duration::from_secs(5))
            .unwrap()
            .output,
        "AFTER_INTERRUPT"
    );
    manager.close(&session.id).unwrap();
}
