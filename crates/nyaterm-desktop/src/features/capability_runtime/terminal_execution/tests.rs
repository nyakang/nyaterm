use super::{
    TerminalExecutionPolicy, TerminalExecutionRequest, TerminalExecutionState,
    external_mcp_terminal_policy,
};
use crate::features::terminal::ai_execution::{TerminalExecutionEvent, TerminalExecutionStatus};
use gpui::AppContext as _;
use nyaterm_mcp_protocol::{RpcError, TerminalExecuteResult};
use nyaterm_terminal::agent_capture::AgentOutputCaptureProcessor;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[test]
fn external_mcp_terminal_display_mode_only_changes_regular_presentation() {
    use nyaterm_core::McpTerminalDisplayMode;

    let inline = external_mcp_terminal_policy(None, McpTerminalDisplayMode::Inline, 17);
    assert_eq!(
        inline,
        TerminalExecutionPolicy {
            background: false,
            max_lines: 17
        }
    );
    let silent = external_mcp_terminal_policy(None, McpTerminalDisplayMode::Silent, 17);
    assert_eq!(
        silent,
        TerminalExecutionPolicy {
            background: false,
            max_lines: 0
        }
    );

    for integrated in [
        TerminalExecutionPolicy {
            background: false,
            max_lines: 9,
        },
        TerminalExecutionPolicy {
            background: true,
            max_lines: 3,
        },
    ] {
        for mode in [
            McpTerminalDisplayMode::Inline,
            McpTerminalDisplayMode::Silent,
        ] {
            assert_eq!(
                external_mcp_terminal_policy(Some(integrated), mode, 17),
                integrated
            );
        }
    }
}

fn execution_request(
    id: &str,
    owner: &str,
    session: &str,
    background: bool,
) -> (
    TerminalExecutionRequest,
    oneshot::Receiver<Result<Value, RpcError>>,
) {
    let (reply, receiver) = oneshot::channel();
    (
        TerminalExecutionRequest {
            mcp_owned: false,
            async_job: false,
            return_job: false,
            operation_id: None,
            id: id.into(),
            owner: owner.into(),
            source: owner.into(),
            session_id: session.into(),
            command: "pwd".into(),
            timeout: Duration::from_secs(30),
            cancellation: CancellationToken::new(),
            track_insertion: true,
            policy: TerminalExecutionPolicy {
                background,
                max_lines: 2,
            },
            reply,
        },
        receiver,
    )
}

fn result() -> TerminalExecuteResult {
    TerminalExecuteResult {
        job_id: None,
        output: "first\nsecond\nthird".into(),
        exit_code: Some(0),
        duration_ms: 0,
        timed_out: false,
        source_truncated: false,
    }
}

fn start_execution_recording(
    app: &crate::features::NyaTermApp,
    session_id: &str,
    path: &std::path::Path,
    mode: nyaterm_recording::recording::RecordingMode,
    include_input: bool,
) {
    use nyaterm_recording::recording::{
        ExistingFileBehavior, RecordingContext, RecordingProfile, RecordingRotationPolicy,
    };
    app.recording
        .writer()
        .start(
            session_id.into(),
            RecordingContext {
                session_id: session_id.into(),
                session_name: "fixture".into(),
                connection_id: None,
                connection_name: None,
                group_path: None,
                protocol: "local".into(),
                host: None,
                port: None,
                username: None,
                started_at: time::OffsetDateTime::now_utc(),
            },
            RecordingProfile {
                mode,
                base_path: path.parent().unwrap().into(),
                path_template: "unused.log".into(),
                include_timestamps: false,
                include_io_labels: true,
                include_session_metadata: false,
                rotation: RecordingRotationPolicy::Session,
                existing_file_behavior: ExistingFileBehavior::Overwrite,
                include_binary_transfer_payloads: false,
                include_input,
            },
            Some(path.into()),
            1024 * 1024,
        )
        .unwrap();
}

#[gpui::test]
fn automatic_execution_records_logical_commands_and_streamed_output_without_changing_raw(
    cx: &mut gpui::TestAppContext,
) {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::SessionLaunchConfig;
    use crate::test_support::TestConfigDir;
    use nyaterm_core::agent_capture::build_agent_capture_command;
    use nyaterm_core::models::connection::AiExecutionProfile;
    use nyaterm_recording::recording::RecordingMode;
    use nyaterm_transport::{SessionEvent, TelnetSessionConfig};
    use std::io::Read;
    use std::net::TcpListener;
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-execution-recording");
    let app = app_with_visible_local_session(cx, directory.path(), "fixture");
    for (index, (profile, mode, include_input, sensitive, ending)) in [
        (
            AiExecutionProfile::Posix,
            RecordingMode::Transcript,
            true,
            false,
            "complete",
        ),
        (
            AiExecutionProfile::Powershell,
            RecordingMode::Transcript,
            true,
            false,
            "complete",
        ),
        (
            AiExecutionProfile::Cmd,
            RecordingMode::Transcript,
            true,
            false,
            "complete",
        ),
        (
            AiExecutionProfile::Posix,
            RecordingMode::Transcript,
            false,
            false,
            "complete",
        ),
        (
            AiExecutionProfile::Posix,
            RecordingMode::Transcript,
            true,
            true,
            "complete",
        ),
        (
            AiExecutionProfile::Posix,
            RecordingMode::Raw,
            true,
            false,
            "complete",
        ),
        (
            AiExecutionProfile::Posix,
            RecordingMode::Transcript,
            true,
            false,
            "cancel",
        ),
        (
            AiExecutionProfile::Posix,
            RecordingMode::Transcript,
            true,
            false,
            "timeout",
        ),
        (
            AiExecutionProfile::SendOnly,
            RecordingMode::Transcript,
            true,
            false,
            "complete",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp: true,
            ..Default::default()
        };
        let (manager, id) = cx.update_entity(&app, |app, _| {
            let manager = app.session.manager_handle();
            let info = manager.create_telnet_session(config.clone()).unwrap();
            let mut metadata = app.session.metadata("fixture").unwrap().clone();
            metadata.launch_config = SessionLaunchConfig::Telnet(config);
            metadata.ai_execution_profile = profile;
            app.session.register_session_metadata(&info.id, metadata);
            app.terminal
                .seed_session_view(info.id.clone(), String::new(), "UTF-8");
            (manager, info.id)
        });
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let path = directory.path().join(format!("{index}.log"));
        let (mut receiver, marker, cancellation) = cx.update_entity(&app, |app, cx| {
            start_execution_recording(app, &id, &path, mode, include_input);
            if mode == RecordingMode::Transcript {
                app.submit_terminal_decoded_output(&id, "before>".into());
            }
            app.observe_recording_prompt_output(&id, if sensitive { "Password: " } else { "$ " });
            let (mut request, receiver) = execution_request("audit", "external", &id, false);
            request.command = "echo fixture".into();
            request.policy.max_lines = 0;
            let cancellation = request.cancellation.clone();
            app.start_terminal_execution(request, cx);
            let marker = app.mcp.capabilities.terminal_executions.tasks[&id]
                .capture_marker_id
                .clone();
            (receiver, marker, cancellation)
        });
        let input = build_agent_capture_command(profile, &marker, "echo fixture")
            .unwrap_or_else(|| "echo fixture\r".into());
        // The fixture uses CR Enter mode, including for Raw TCP text writes.
        let wire = format!(
            "{}\r",
            input
                .trim_end_matches(['\r', '\n'])
                .replace("\r\n", "\r")
                .replace('\n', "\r")
        );
        let mut sent = vec![0; wire.len()];
        stream.read_exact(&mut sent).unwrap();
        assert_eq!(sent, wire.as_bytes());
        let output = if profile == AiExecutionProfile::SendOnly {
            "echo fixture\r\nfixture\r\nRouter#".to_string()
        } else if ending == "complete" {
            format!(
                "{input}\r\n__DF_CMD_START_{marker}__\r\necho fixture\r\n\r\n__DF_CMD_END_{marker}_0__\r\nprompt>"
            )
        } else {
            format!("{input}\r\n__DF_CMD_START_{marker}__\r\npartial\r\n")
        };
        let completion = cx.update_entity(&app, |app, cx| {
            app.session.extend_pending_events([
                SessionEvent::OutputBoundary {
                    session_id: id.clone(),
                    token: marker.clone(),
                },
                SessionEvent::Output {
                    session_id: id.clone(),
                    data: output.as_bytes().to_vec(),
                },
            ]);
            app.drain_session_events(cx);
            if ending != "complete" {
                if ending == "cancel" {
                    cancellation.cancel();
                } else {
                    app.mcp
                        .capabilities
                        .terminal_executions
                        .tasks
                        .get_mut(&id)
                        .unwrap()
                        .started -= Duration::from_secs(31);
                }
                app.poll_terminal_executions(cx);
                app.session.extend_pending_events([SessionEvent::Output {
                    session_id: id.clone(),
                    data: format!("late\r\n__DF_CMD_END_{marker}_0__\r\nprompt>").into_bytes(),
                }]);
                app.drain_session_events(cx);
            } else if profile == AiExecutionProfile::SendOnly {
                app.finish_terminal_execution(
                    &id,
                    &marker,
                    Ok(result()),
                    TerminalExecutionStatus::Completed,
                    cx,
                );
            }
            if mode == RecordingMode::Transcript {
                app.submit_terminal_decoded_output(&id, "after>".into());
            }
            app.terminal.recording_output_fence()().unwrap();
            app.recording.writer().stop_complete(id.clone()).unwrap()
        });
        let result = receiver.try_recv().unwrap();
        if ending == "cancel" {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap()["timedOut"],
                ending == "timeout",
                "case {index}"
            );
        }
        if mode == RecordingMode::Raw {
            assert_eq!(std::fs::read(&path).unwrap(), output.as_bytes());
            // Raw input recording retains the encoded logical input, before
            // transport newline conversion, just as it does for Telnet.
            assert_eq!(
                std::fs::read(completion.input_file_path.unwrap()).unwrap(),
                input.as_bytes()
            );
        } else {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains("__DF_CMD_"), "case {index}");
            assert!(!text.contains("┌ AI #"), "case {index}");
            assert!(
                text.find("before>").unwrap() < text.find("after>").unwrap(),
                "case {index}"
            );
            if profile != AiExecutionProfile::SendOnly && ending == "complete" {
                assert!(text.contains("[OUTPUT] after>\n"), "case {index}");
            }
            assert_eq!(
                text.matches("[INPUT] echo fixture").count(),
                usize::from(include_input && !sensitive),
                "case {index}"
            );
            if ending == "complete" {
                let expected = if profile == AiExecutionProfile::SendOnly {
                    "fixture"
                } else {
                    "echo fixture"
                };
                assert_eq!(
                    text.matches(&format!("[OUTPUT] {expected}\n")).count(),
                    1,
                    "case {index}: {text}"
                );
            } else {
                assert_eq!(
                    text.matches("[OUTPUT] partial\n").count(),
                    1,
                    "case {index}"
                );
                assert_eq!(text.matches("[OUTPUT] late\n").count(), 1, "case {index}");
            }
        }
        manager.close(&id).unwrap();
    }
}

#[cfg(windows)]
#[gpui::test]
#[ignore = "requires Windows ConPTY and PowerShell; explicitly run by Windows CI in isolation"]
fn shared_entry_routes_native_codex_claude_and_external_calls_on_a_hidden_terminal(
    cx: &mut gpui::TestAppContext,
) {
    use crate::features::capability_runtime::{CapabilityCaller, CapabilityRequest};
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;
    use gpui::AppContext as _;
    use nyaterm_core::{AiExecutionProfile, AiPermissionMode, CapabilityScope};
    use nyaterm_transport::LocalSessionConfig;
    use serde_json::json;
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-shared-terminal-execution");
    let app = app_with_visible_local_session(cx, directory.path(), "visible");
    let manager = cx.update_entity(&app, |app, _| app.session.manager_handle());
    let config = LocalSessionConfig {
        shell_path: Some("cmd.exe".into()),
        shell_args: vec!["/D".into(), "/Q".into()],
        ..LocalSessionConfig::default()
    };
    let session = manager.create_local_session(config).expect("Windows PTY");
    manager
        .write(&session.id, b"set NYATERM_CAPTURE_FIXTURE=current-pty\r")
        .unwrap();
    cx.update_entity(&app, |app, _| {
        let mut metadata = app.session.metadata("visible").unwrap().clone();
        metadata.ai_execution_profile = AiExecutionProfile::Cmd;
        app.session.register_session_metadata(&session.id, metadata);
        start_execution_recording(
            app,
            &session.id,
            &directory.path().join("execution.log"),
            nyaterm_recording::recording::RecordingMode::Transcript,
            true,
        );
    });
    let mut credentials = Vec::new();
    for (source, background) in [
        ("NyaTerm Agent", false),
        ("Codex", false),
        ("Claude Code", true),
        ("external MCP", false),
    ] {
        let mut receiver = cx.update_entity(&app, |app, cx| {
            let policy = TerminalExecutionPolicy { background, max_lines: 2 };
            let native = source == "NyaTerm Agent";
            let integrated = matches!(source, "Codex" | "Claude Code");
            let mut settings = app.ai.settings_config_cloned();
            settings.agent_background_execution_enabled = background;
            settings.terminal_output_lines = 2;
            app.ai.replace_settings_config(settings, true);
            let caller = if native {
                app.ai.begin_native_run(serde_json::from_value(json!({"mode":"agent","action":"generate_command","userInput":"fixture"})).unwrap());
                CapabilityCaller::Native { run_id: "fixture".into(), ai_session_id: app.ai.chat_session_id().into(), model_risk: None }
            } else { CapabilityCaller::Mcp };
            let generation = if integrated {
                let credential = app.create_ephemeral_mcp_credential(vec![session.id.clone()], Some(session.id.clone()), AiPermissionMode::FullAccess, policy).unwrap();
                let generation = credential.environment().remove("NYATERM_MCP_GENERATION").unwrap();
                credentials.push(credential);
                generation
            } else { "external-generation".into() };
            // The visible settings and scope must not identify an integrated caller.
            let mut settings = app.ai.settings_config_cloned();
            settings.agent_background_execution_enabled = !background;
            app.ai.replace_settings_config(settings, true);
            let (reply, receiver) = oneshot::channel();
            app.dispatch_shared_terminal_execute(CapabilityRequest {
            extended_results: true, resolved_session_id: None,
                connection_id: source.into(), request_id: source.into(), generation,
                client: source.into(), permission_mode: AiPermissionMode::FullAccess,
                scope: CapabilityScope::AllSessions,
                tool: nyaterm_mcp_protocol::tool::TERMINAL_EXECUTE.into(),
                arguments: json!({"sessionId":session.id,"command":if background { "echo background-value" } else { "echo %NYATERM_CAPTURE_FIXTURE%" },"timeoutMs":10000}),
                cancellation: CancellationToken::new(), approved: true, approval_decision: None, caller, reply,
            }, session.id.clone(), cx);
            assert_eq!(app.mcp.capabilities.terminal_executions.tasks[&session.id].request.policy.background, background);
            if !background {
                assert!(app.write_session_input_recorded(&session.id, b"interference\r").is_err());
                assert!(app.write_session_raw_input_recorded(&session.id, b"interference\r").is_err());
            }
            app.ai.switch_scope("terminal:unrelated");
            receiver
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let result = loop {
            cx.run_until_parked();
            cx.update_entity(&app, |app, cx| {
                app.drain_session_events(cx);
                app.drain_terminal_frame_events(cx);
                app.poll_terminal_executions(cx);
            });
            if let Ok(result) = receiver.try_recv() {
                break result.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "execution did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(result["exitCode"], 0, "{source}: {result}");
        assert!(
            !result["output"].as_str().unwrap().contains("__DF_CMD_"),
            "{source}: captured an internal marker"
        );
        assert!(
            result["output"].as_str().unwrap().contains(if background {
                "background-value"
            } else {
                "current-pty"
            }),
            "{source}"
        );
        cx.update_entity(&app, |app, _| {
            assert!(!app.mcp.capabilities.terminal_executions.busy(&session.id));
            assert_eq!(
                app.mcp
                    .capabilities
                    .terminal_executions
                    .take_inserted(source),
                !background
            );
        });
    }
    cx.update_entity(&app, |app, _| {
        app.terminal.recording_output_fence()().unwrap();
        app.recording
            .writer()
            .stop_complete(session.id.clone())
            .unwrap();
    });
    let transcript = std::fs::read_to_string(directory.path().join("execution.log")).unwrap();
    assert!(!transcript.contains("__DF_CMD_"));
    assert!(!transcript.contains("┌ AI #"));
    assert_eq!(
        transcript
            .matches("[INPUT] echo %NYATERM_CAPTURE_FIXTURE%")
            .count(),
        3
    );
    assert_eq!(transcript.matches("[OUTPUT] current-pty").count(), 3);
    assert_eq!(
        transcript.matches("[INPUT] echo background-value").count(),
        1
    );
    assert_eq!(transcript.matches("[OUTPUT] background-value").count(), 1);
    let text = cx.update_entity(&app, |app, _| {
        app.terminal.request_frame_text_for_test(&session.id)
    });
    let text = futures::executor::block_on(text).unwrap().unwrap();
    assert_eq!(text.matches("┌ AI #").count(), 3);
    assert_eq!(text.matches("└").count(), 3);
    assert!(
        !text.contains("__DF_CMD_"),
        "internal marker leaked: start={}, end={}",
        text.matches("__DF_CMD_START_").count(),
        text.matches("__DF_CMD_END_").count()
    );
    assert!(!text.contains("background-value"));
    let mut send_only = cx.update_entity(&app, |app, cx| {
        app.session
            .metadata_mut(&session.id)
            .unwrap()
            .ai_execution_profile = AiExecutionProfile::SendOnly;
        let (request, receiver) = execution_request("send-only", "external", &session.id, true);
        app.start_terminal_execution(request, cx);
        assert!(!app.mcp.capabilities.terminal_executions.busy(&session.id));
        assert!(
            !app.mcp
                .capabilities
                .terminal_executions
                .take_inserted("send-only")
        );
        receiver
    });
    assert_eq!(
        send_only.try_recv().unwrap().unwrap_err().code,
        "permission_denied"
    );
    let powershell = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("powershell.exe".into()),
            shell_args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NoExit".into(),
                "-Command".into(),
                // The prompt is emitted after startup, unlike process creation or
                // output from the initialization command itself.
                "$nyatermCaptureFixture = 'powershell-pty'; function global:prompt { 'nyaterm-powershell-ready> ' }".into(),
            ],
            ..LocalSessionConfig::default()
        })
        .unwrap();
    cx.update_entity(&app, |app, _| {
        let mut metadata = app.session.metadata("visible").unwrap().clone();
        metadata.ai_execution_profile = AiExecutionProfile::Powershell;
        app.session
            .register_session_metadata(&powershell.id, metadata);
    });
    let startup_deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut frame = cx.update_entity(&app, |app, _| {
        app.terminal.request_frame_text_for_test(&powershell.id)
    });
    let mut saw_output = false;
    loop {
        cx.run_until_parked();
        cx.update_entity(&app, |app, cx| {
            app.drain_session_events(cx);
            app.drain_terminal_frame_events(cx);
        });
        if let Some(text) = frame.try_recv().expect("PowerShell frame worker stopped") {
            let text = text.unwrap_or_default();
            saw_output |= !text.is_empty();
            if text.contains("nyaterm-powershell-ready>") {
                break;
            }
            frame = cx.update_entity(&app, |app, _| {
                app.terminal.request_frame_text_for_test(&powershell.id)
            });
        }
        assert!(
            std::time::Instant::now() < startup_deadline,
            "PowerShell startup timed out before its prompt (terminal output received: {saw_output})"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let execution_timeout = Duration::from_secs(30);
    let mut powershell_reply = cx.update_entity(&app, |app, cx| {
        let (mut request, receiver) =
            execution_request("powershell", "external", &powershell.id, false);
        request.command = "Write-Output $nyatermCaptureFixture".into();
        request.timeout = execution_timeout;
        request.policy.max_lines = 0;
        app.start_terminal_execution(request, cx);
        receiver
    });
    // Let the execution layer report its own timeout before the test watchdog.
    let deadline = std::time::Instant::now() + execution_timeout + Duration::from_secs(5);
    let powershell_result = loop {
        cx.run_until_parked();
        cx.update_entity(&app, |app, cx| {
            app.drain_session_events(cx);
            app.drain_terminal_frame_events(cx);
            app.poll_terminal_executions(cx);
        });
        match powershell_reply.try_recv() {
            Ok(result) => break result.expect("PowerShell execution failed after readiness"),
            Err(oneshot::error::TryRecvError::Closed) => {
                panic!("PowerShell execution reply closed after readiness");
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "PowerShell execution did not reply after its request timeout (startup completed)"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        powershell_result["timedOut"], false,
        "PowerShell execution timed out after its prompt was ready"
    );
    assert_eq!(powershell_result["exitCode"], 0);
    assert!(
        powershell_result["output"]
            .as_str()
            .unwrap()
            .contains("powershell-pty")
    );
    let text = cx.update_entity(&app, |app, _| {
        app.terminal.request_frame_text_for_test(&powershell.id)
    });
    let text = futures::executor::block_on(text)
        .unwrap()
        .unwrap_or_default();
    assert!(!text.contains("AI #") && !text.contains("__DF_CMD_"));
    manager.close(&powershell.id).unwrap();
    manager.close(&session.id).unwrap();
    drop(credentials);
    let cleanup = cx.update_entity(&app, |app, cx| app.shutdown_blocking_jobs(cx));
    cx.foreground_executor().block_test(cleanup);
    cx.run_until_parked();
}

#[test]
fn reusing_an_rpc_id_does_not_let_a_late_worker_finish_a_new_execution() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("same-rpc-id", "external", "a", true);
    assert!(state.insert(request, None).is_ok());
    state.tasks.get_mut("a").unwrap().capture_marker_id = "first-internal-id".into();
    state
        .finish(
            "a",
            "first-internal-id",
            Ok(result()),
            TerminalExecutionStatus::TimedOut,
        )
        .unwrap();
    let (request, _receiver) = execution_request("same-rpc-id", "external", "a", true);
    assert!(state.insert(request, None).is_ok());
    state.tasks.get_mut("a").unwrap().capture_marker_id = "second-internal-id".into();
    assert!(
        state
            .finish(
                "a",
                "first-internal-id",
                Ok(result()),
                TerminalExecutionStatus::Completed
            )
            .is_none()
    );
    assert!(state.busy("a"));
    assert!(
        state
            .finish(
                "a",
                "second-internal-id",
                Ok(result()),
                TerminalExecutionStatus::Completed
            )
            .is_some()
    );
}

#[test]
fn output_before_the_boundary_is_preserved_and_stale_boundaries_do_nothing() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("new", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("new".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    assert_eq!(
        state.process_output("a", "previous output\n").visible_text,
        "previous output\n"
    );
    assert!(state.begin_capture("a", "old").is_none());
    assert!(state.begin_capture("a", "new").is_some());
    assert!(state.begin_capture("a", "new").is_none());
    assert!(
        state
            .process_output("a", "wrapper echo\n__DF_CMD_START_new__\nhello")
            .visible_text
            .is_empty()
    );
    let cancelled = state
        .finish("a", "new", Ok(result()), TerminalExecutionStatus::TimedOut)
        .unwrap();
    assert!(
        cancelled.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .contains("hello")
    );
    assert!(state.begin_capture("a", "new").is_none());
}

#[test]
fn a_prompt_from_the_previous_task_cannot_overtake_the_next_start_boundary() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("old", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("old".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "old");
    state.process_output("a", "__DF_CMD_START_old__\nok\n__DF_CMD_END_old_0__\n");
    state
        .finish("a", "old", Ok(result()), TerminalExecutionStatus::Completed)
        .unwrap();
    let (request, _receiver) = execution_request("new", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("new".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    assert!(state.process_output("a", "PS>").visible_text.is_empty());
    assert!(state.begin_capture("a", "new").is_some());
    assert!(
        state
            .process_output("a", "wrapper echo")
            .visible_text
            .is_empty()
    );
}

#[test]
fn drain_timeout_waits_for_a_split_title_terminator() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("old", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("old".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "old");
    let output = state.process_output(
        "a",
        "__DF_CMD_START_old__\nok\n__DF_CMD_END_old_0__\n\x1b]0;call echo ",
    );
    assert_eq!(output.completed.len(), 1);
    state
        .finish("a", "old", Ok(result()), TerminalExecutionStatus::Completed)
        .unwrap();
    let expired = std::time::Instant::now() + Duration::from_secs(1);
    assert!(state.expire_draining(expired).is_empty());
    assert!(state.intercepting("a"));
    let tail = state.process_output("a", "__DF_CMD_END_old_%ERRORLEVEL%__\x07\nC:\\>");
    assert!(!tail.visible_text.contains("__DF_CMD_"));
    assert!(!state.intercepting("a"));
}

#[test]
fn a_new_capture_inherits_an_unfinished_title_from_the_previous_execution() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("old", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("old".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "old");
    state.process_output(
        "a",
        "__DF_CMD_START_old__\nok\n__DF_CMD_END_old_0__\n\x1b]0;",
    );
    state
        .finish("a", "old", Ok(result()), TerminalExecutionStatus::Completed)
        .unwrap();
    let (request, _receiver) = execution_request("new", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("new".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "new");
    let output = state.process_output(
        "a",
        "__DF_CMD_START_new__wrong__DF_CMD_END_new_9__\x07\
         __DF_CMD_START_new__\nexpected\n__DF_CMD_END_new_0__\nC:\\>",
    );
    assert!(output.visible_text.is_empty());
    assert_eq!(output.completed.len(), 1);
    assert_eq!(output.completed[0].output, "expected");
    assert_eq!(output.completed[0].exit_code, Some(0));
}

#[test]
fn retired_markers_remain_hidden_after_cancellation_and_a_new_task() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("old", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("old".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "old");
    state.process_output("a", "__DF_CMD_START_old__\npartial\n__DF_CMD_EN");
    state
        .finish("a", "old", Ok(result()), TerminalExecutionStatus::Cancelled)
        .unwrap();
    assert!(!state.capturing("a"));
    assert_eq!(
        state.process_output("a", "D_old_0__\nnormal").visible_text,
        "\nnormal"
    );
    assert!(!state.intercepting("a"));
    let (request, _receiver) = execution_request("new", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("new".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "new");
    assert!(
        state
            .finish("a", "old", Ok(result()), TerminalExecutionStatus::Completed)
            .is_none()
    );
    assert!(state.busy("a"));
}

#[test]
fn all_callers_share_target_occupancy_and_other_targets_can_run_in_parallel() {
    let mut state = TerminalExecutionState::default();
    let (native, _native_reply) = execution_request("native", "NyaTerm", "a", false);
    let (codex, _codex_reply) = execution_request("codex", "Codex", "a", true);
    let (claude, _claude_reply) = execution_request("claude", "Claude Code", "b", false);
    assert!(state.insert(native, None).is_ok());
    assert!(state.insert(codex, None).is_err());
    assert!(state.insert(claude, None).is_ok());
    assert!(state.busy("a") && state.busy("b"));
}

#[test]
fn generations_keep_separate_counters_and_execution_policies() {
    let mut state = TerminalExecutionState::default();
    for (id, owner, session, background, expected_step) in [
        ("a1", "codex-run", "a", false, 0),
        ("b1", "claude-run", "b", true, 0),
        ("a2", "codex-run", "c", false, 1),
    ] {
        let (request, _receiver) = execution_request(id, owner, session, background);
        let event = state
            .insert(request, None)
            .unwrap_or_else(|_| panic!("free target"));
        let TerminalExecutionEvent::Started { step_index, .. } = event else {
            panic!("start event");
        };
        assert_eq!(step_index, expected_step);
        assert_eq!(state.tasks[session].request.policy.background, background);
    }
    state.forget_owner("codex-run");
    assert!(state.tasks["a"].request.cancellation.is_cancelled());
    assert!(!state.tasks["b"].request.cancellation.is_cancelled());
}

#[test]
fn completion_replies_once_keeps_full_output_and_ignores_late_results() {
    let mut state = TerminalExecutionState::default();
    let (request, mut receiver) = execution_request("one", "external", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("one".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    assert!(state.begin_capture("a", "one").is_some());
    let captured = state.process_output(
        "a",
        "__DF_CMD_START_one__\nfirst\nsecond\nthird\n__DF_CMD_END_one_0__\nPS>",
    );
    assert_eq!(captured.completed.len(), 1);
    let completion = state
        .finish("a", "one", Ok(result()), TerminalExecutionStatus::Completed)
        .unwrap();
    assert!(matches!(
        completion.event,
        Some(TerminalExecutionEvent::Finished { max_lines: 2, .. })
    ));
    completion.reply.send(completion.result).unwrap();
    assert_eq!(
        receiver.try_recv().unwrap().unwrap()["output"],
        "first\nsecond\nthird"
    );
    assert!(
        state
            .finish("a", "one", Ok(result()), TerminalExecutionStatus::Completed)
            .is_none()
    );
    let (next, _receiver) = execution_request("two", "external", "a", false);
    assert!(state.insert(next, None).is_ok());
    assert!(
        state
            .finish("a", "one", Ok(result()), TerminalExecutionStatus::Completed)
            .is_none()
    );
    assert!(state.busy("a"));
}

#[test]
fn uncertain_terminal_outcomes_protect_occupancy_and_background_never_draws_a_block() {
    for status in [
        TerminalExecutionStatus::Failed,
        TerminalExecutionStatus::TimedOut,
        TerminalExecutionStatus::Cancelled,
        TerminalExecutionStatus::Disconnected,
    ] {
        let mut state = TerminalExecutionState::default();
        let (request, _receiver) = execution_request("task", "owner", "a", false);
        let mut capture = AgentOutputCaptureProcessor::new();
        capture.register("task".into());
        assert!(state.insert(request, Some(capture)).is_ok());
        assert!(state.begin_capture("a", "task").is_some());
        state.process_output("a", "__DF_CMD_START_task__\npartial");
        assert!(state.capturing("a"));
        let completed = state.finish("a", "task", Ok(result()), status).unwrap();
        assert!(
            matches!(completed.event, Some(TerminalExecutionEvent::Finished { status: ended, .. }) if ended == status)
        );
        assert!(!state.capturing("a"));
        assert_eq!(
            state.busy("a"),
            status != TerminalExecutionStatus::Disconnected
        );
        if status != TerminalExecutionStatus::Disconnected {
            assert!(state.confirm_recovered("a"));
            assert!(!state.busy("a"));
        }
        assert_eq!(
            state.process_output("a", "normal output").visible_text,
            "normal output"
        );
    }
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("background", "owner", "b", true);
    assert!(state.insert(request, None).is_ok());
    assert!(
        state
            .finish(
                "b",
                "background",
                Ok(result()),
                TerminalExecutionStatus::Completed
            )
            .unwrap()
            .event
            .is_none()
    );
}

#[test]
fn cancellation_wins_a_racing_success_and_insertion_audit_is_consumed_once() {
    let mut state = TerminalExecutionState::default();
    let (request, _receiver) = execution_request("race", "owner", "a", true);
    let cancellation = request.cancellation.clone();
    assert!(state.insert(request, None).is_ok());
    cancellation.cancel();
    let completion = state
        .finish(
            "a",
            "race",
            Ok(result()),
            TerminalExecutionStatus::Completed,
        )
        .unwrap();
    assert_eq!(completion.result.unwrap_err().code, "cancelled");
    assert!(!state.take_inserted("race"));
    state.inserted_requests.insert("foreground".into());
    assert!(state.take_inserted("foreground"));
    assert!(!state.take_inserted("foreground"));
}

#[test]
fn raw_capture_respects_output_boundary_and_replies_with_unknown_exit_status() {
    use nyaterm_core::raw_cli_capture::{RawCliCapture, RawCliEnd};
    use std::time::Instant;
    let mut state = TerminalExecutionState::default();
    let (request, mut receiver) = execution_request("raw", "external", "a", false);
    assert!(state.insert(request, None).is_ok());
    state.tasks.get_mut("a").unwrap().raw_capture =
        Some(RawCliCapture::new("Router#", Instant::now()));
    assert!(state.capturing("a"));
    state.process_output("a", "queued-before-command\r\nRouter#");
    assert!(state.begin_capture("a", "wrong").is_none());
    assert!(state.begin_capture("a", "raw").is_some());
    state.process_output("other", "unrelated");
    state.process_output("a", "result\r\nRouter(config)#");
    let task = &state.tasks["a"];
    assert_eq!(
        task.raw_capture.as_ref().unwrap().poll(
            Instant::now() + Duration::from_secs(1),
            task.started + task.request.timeout
        ),
        Some(RawCliEnd::Prompt)
    );
    let completion = state
        .finish(
            "a",
            "raw",
            Ok(TerminalExecuteResult {
                job_id: None,
                output: RawCliEnd::Prompt.description().into(),
                exit_code: None,
                duration_ms: 0,
                timed_out: false,
                source_truncated: false,
            }),
            TerminalExecutionStatus::Completed,
        )
        .unwrap();
    completion.reply.send(completion.result).unwrap();
    let output = receiver.try_recv().unwrap().unwrap();
    assert!(output["exitCode"].is_null());
    assert!(
        output["output"]
            .as_str()
            .unwrap()
            .starts_with("result\nRouter(config)#")
    );
    assert!(
        !output["output"]
            .as_str()
            .unwrap()
            .contains("queued-before-command")
    );
    assert!(!state.busy("a") && !state.capturing("a"));
    // A subsequent command must not inherit the previous capture.
    let (request, _receiver) = execution_request("next", "external", "a", false);
    assert!(state.insert(request, None).is_ok());
    state.tasks.get_mut("a").unwrap().raw_capture =
        Some(RawCliCapture::new("Router#", Instant::now()));
    state.begin_capture("a", "next");
    state.process_output("a", "next-result\r\nRouter#");
    let completion = state
        .finish(
            "a",
            "next",
            Ok(TerminalExecuteResult {
                job_id: None,
                exit_code: None,
                ..result()
            }),
            TerminalExecutionStatus::Completed,
        )
        .unwrap();
    assert!(
        !completion.result.unwrap()["output"]
            .as_str()
            .unwrap()
            .contains("Router(config)")
    );
}

#[test]
fn raw_failures_keep_partial_output_and_require_recovery() {
    use nyaterm_core::raw_cli_capture::RawCliCapture;
    use std::time::Instant;
    for status in [
        TerminalExecutionStatus::Cancelled,
        TerminalExecutionStatus::Disconnected,
        TerminalExecutionStatus::TimedOut,
        TerminalExecutionStatus::Failed,
    ] {
        let mut state = TerminalExecutionState::default();
        let (request, _receiver) = execution_request("raw", "native", "a", false);
        assert!(state.insert(request, None).is_ok());
        state.tasks.get_mut("a").unwrap().raw_capture =
            Some(RawCliCapture::new("Router#", Instant::now()));
        state.begin_capture("a", "raw");
        state.process_output("a", "partial\r\n---- More ----");
        let completion = state
            .finish(
                "a",
                "raw",
                Err(super::rpc_failure(
                    "execution_incomplete",
                    "Review terminal",
                )),
                status,
            )
            .unwrap();
        assert!(completion.result.is_err());
        assert!(
            matches!(completion.event, Some(TerminalExecutionEvent::Finished { output, .. }) if output.contains("partial") && output.contains("More"))
        );
        assert!(!state.capturing("a"));
        assert_eq!(
            state.busy("a"),
            status != TerminalExecutionStatus::Disconnected
        );
        if status != TerminalExecutionStatus::Disconnected {
            assert!(state.confirm_recovered("a"));
            assert!(!state.busy("a"));
        }
    }
}

#[gpui::test]
fn raw_session_events_survive_full_degraded_ui_tail(cx: &mut gpui::TestAppContext) {
    // The frame worker and session bridge wake this test from real threads.
    cx.executor().allow_parking();
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::TERMINAL_UI_OUTPUT_TAIL_CAP;
    use crate::test_support::TestConfigDir;
    use nyaterm_core::raw_cli_capture::{RawCliCapture, RawCliEnd};
    use nyaterm_transport::SessionEvent;
    use std::time::Instant;
    let directory = TestConfigDir::new("nyaterm-raw-session-events");
    let app = app_with_visible_local_session(cx, directory.path(), "raw");
    cx.update_entity(&app, |app, cx| {
        app.terminal.seed_session_view(
            "raw".into(),
            "x".repeat(TERMINAL_UI_OUTPUT_TAIL_CAP),
            "UTF-8",
        );
        app.terminal.enter_session_render_degraded("raw");
        app.shell.complete_terminal_windows_restore();
        let (request, _receiver) = execution_request("raw-task", "native", "raw", false);
        let state = &mut app.mcp.capabilities.terminal_executions;
        assert!(state.insert(request, None).is_ok());
        state.tasks.get_mut("raw").unwrap().raw_capture =
            Some(RawCliCapture::new("Router#", Instant::now()));
        app.sync_session_event_bridge_policy();
        app.start_runtime_data_plane_drain(cx);
        for event in [
            SessionEvent::OutputBoundary {
                session_id: "raw".into(),
                token: "raw-task".into(),
            },
            SessionEvent::Output {
                session_id: "raw".into(),
                data: b"command-result\r\nRouter#".to_vec(),
            },
        ] {
            app.session.push_event_bridge_ui_event_for_test(event);
        }
    });
    cx.run_until_parked();
    cx.update_entity(&app, |app, _| {
        let task = &app.mcp.capabilities.terminal_executions.tasks["raw"];
        assert!(task.capture_started);
        assert_eq!(
            task.raw_capture.as_ref().unwrap().poll(
                Instant::now() + Duration::from_secs(1),
                task.started + task.request.timeout
            ),
            Some(RawCliEnd::Prompt)
        );
    });
}

#[gpui::test]
fn network_session_profile_and_background_policy_are_shared(cx: &mut gpui::TestAppContext) {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::SessionLaunchConfig;
    use crate::test_support::TestConfigDir;
    use nyaterm_core::AiExecutionProfile;
    use nyaterm_transport::{SshSessionConfig, SshSessionProfile};

    let directory = TestConfigDir::new("nyaterm-network-policy");
    let app = app_with_visible_local_session(cx, directory.path(), "ssh");
    cx.update_entity(&app, |app, cx| {
        let metadata = app.session.metadata_mut("ssh").unwrap();
        metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
            profile: SshSessionProfile::NetworkDevice,
            ..SshSessionConfig::default()
        }));
        metadata.ai_execution_profile = AiExecutionProfile::Auto;
        assert_eq!(
            app.ai_execution_profile_for_session("ssh"),
            AiExecutionProfile::SendOnly
        );
        for profile in [
            AiExecutionProfile::Auto,
            AiExecutionProfile::Posix,
            AiExecutionProfile::SendOnly,
            AiExecutionProfile::Disabled,
        ] {
            app.session
                .metadata_mut("ssh")
                .unwrap()
                .ai_execution_profile = profile;
            if profile != AiExecutionProfile::Auto {
                assert_eq!(app.ai_execution_profile_for_session("ssh"), profile);
            }
            assert!(app.validate_ai_background_execution("ssh").is_err());
            let (request, mut receiver) =
                execution_request("background", "external MCP", "ssh", true);
            app.start_terminal_execution(request, cx);
            assert_eq!(
                receiver.try_recv().unwrap().unwrap_err().code,
                "permission_denied"
            );
            assert!(!app.mcp.capabilities.terminal_executions.busy("ssh"));
        }
        let metadata = app.session.metadata_mut("ssh").unwrap();
        metadata.launch_config = SessionLaunchConfig::Ssh(Box::default());
        metadata.ai_execution_profile = AiExecutionProfile::Auto;
        app.terminal
            .seed_session_view("ssh".into(), "root#".into(), "UTF-8");
        assert_eq!(
            app.ai_execution_profile_for_session("ssh"),
            AiExecutionProfile::Posix
        );
        assert!(app.validate_ai_background_execution("ssh").is_ok());
    });
}

#[gpui::test]
fn raw_output_loss_cancellation_and_disconnect_release_the_shared_task(
    cx: &mut gpui::TestAppContext,
) {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;
    use nyaterm_core::raw_cli_capture::RawCliCapture;
    use std::time::Instant;
    let directory = TestConfigDir::new("nyaterm-raw-endings");
    let app = app_with_visible_local_session(cx, directory.path(), "raw");
    for outcome in ["loss", "cancel", "disconnect", "timeout"] {
        cx.update_entity(&app, |app, cx| {
            app.session.metadata_mut("raw").unwrap().disconnected = false;
            let (request, mut receiver) = execution_request(outcome, "native", "raw", false);
            let cancellation = request.cancellation.clone();
            let state = &mut app.mcp.capabilities.terminal_executions;
            assert!(state.insert(request, None).is_ok());
            state.tasks.get_mut("raw").unwrap().raw_capture =
                Some(RawCliCapture::new("Router#", Instant::now()));
            state.begin_capture("raw", outcome);
            state.process_output("raw", "retained-prefix");
            match outcome {
                "loss" => app.note_terminal_execution_output_dropped("raw", cx),
                "cancel" => {
                    cancellation.cancel();
                    app.poll_terminal_executions(cx);
                }
                "disconnect" => {
                    app.session.metadata_mut("raw").unwrap().disconnected = true;
                    app.poll_terminal_executions(cx);
                }
                _ => {
                    app.mcp
                        .capabilities
                        .terminal_executions
                        .tasks
                        .get_mut("raw")
                        .unwrap()
                        .started = Instant::now() - Duration::from_secs(31);
                    app.poll_terminal_executions(cx);
                }
            }
            let result = receiver.try_recv().unwrap();
            if outcome == "loss" {
                let value = result.unwrap();
                assert_eq!(value["sourceTruncated"], true);
                assert!(
                    value["output"]
                        .as_str()
                        .unwrap()
                        .contains("retained-prefix")
                );
            } else {
                assert!(result.is_err());
            }
            if app
                .mcp
                .capabilities
                .terminal_executions
                .recovery_required("raw")
            {
                assert!(
                    app.mcp
                        .capabilities
                        .terminal_executions
                        .confirm_recovered("raw")
                );
            }
            assert!(!app.mcp.capabilities.terminal_executions.busy("raw"));
            assert!(!app.mcp.capabilities.terminal_executions.capturing("raw"));
        });
    }
}

#[test]
fn cancellation_after_end_keeps_exit_evidence_and_does_not_require_shell_recovery() {
    use nyaterm_mcp_protocol::{TerminalJobStatus, TerminalProcessState};
    let mut state = TerminalExecutionState::default();
    let (request, _) = execution_request("job", "owner", "a", false);
    let cancellation = request.cancellation.clone();
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("job".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "job");
    let captured = state
        .process_output(
            "a",
            "__DF_CMD_START_job__\nprefix\n__DF_CMD_END_job_17__\nprompt>",
        )
        .completed
        .remove(0);
    cancellation.cancel();
    let mut outcome = result();
    outcome.exit_code = captured.exit_code;
    outcome.output = captured.output;
    let completion = state
        .finish("a", "job", Ok(outcome), TerminalExecutionStatus::Completed)
        .unwrap();
    assert_eq!(completion.result.unwrap_err().code, "cancelled");
    assert!(!state.busy("a"));
    assert_eq!(state.jobs["job"].status, TerminalJobStatus::Cancelled);
    assert_eq!(
        state.jobs["job"].process_state,
        TerminalProcessState::ExitConfirmed
    );
    assert_eq!(state.jobs["job"].exit_code, Some(17));
}

#[test]
fn timeout_blocks_reexecution_until_matching_late_end_and_retains_job_outcome() {
    use nyaterm_mcp_protocol::{TerminalJobStatus, TerminalProcessState};
    let mut state = TerminalExecutionState::default();
    let (request, _) = execution_request("job", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register_streaming("job".into(), true);
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "job");
    state.process_output("a", "__DF_CMD_START_job__\nprefix\n");
    state
        .finish("a", "job", Ok(result()), TerminalExecutionStatus::TimedOut)
        .unwrap();
    assert!(state.recovery_required("a"));
    let (request, _) = execution_request("other", "owner", "a", false);
    assert!(state.insert(request, None).is_err());
    state.process_output("a", "tail\n__DF_CMD_END_other_0__");
    assert!(state.busy("a"));
    state.process_output("a", "\n__DF_CMD_END_job_7__\nprompt>");
    assert!(!state.busy("a"));
    let job = &state.jobs["job"];
    assert_eq!(job.status, TerminalJobStatus::TimedOut);
    assert_eq!(job.process_state, TerminalProcessState::ExitConfirmed);
    assert_eq!(job.exit_code, Some(7));
    let output = job.output.read(0, 16384).unwrap().data;
    assert!(output.contains("prefix") && output.contains("tail"));
    assert!(!output.contains("prompt>"));
}
#[test]
fn manual_takeover_and_result_expiry_do_not_release_recovery_guard() {
    let mut state = TerminalExecutionState::default();
    let (request, _) = execution_request("job", "owner", "a", false);
    let mut capture = AgentOutputCaptureProcessor::new();
    capture.register("job".into());
    assert!(state.insert(request, Some(capture)).is_ok());
    state.begin_capture("a", "job");
    state.process_output("a", "__DF_CMD_START_job__\nprefix");
    state
        .finish("a", "job", Ok(result()), TerminalExecutionStatus::TimedOut)
        .unwrap();
    state.note_manual_takeover("a");
    state.process_output("a", "manual command output");
    assert!(
        !state.jobs["job"]
            .output
            .read(0, 16384)
            .unwrap()
            .data
            .contains("manual command")
    );
    state.jobs.clear();
    assert!(state.busy("a"));
    assert!(state.confirm_recovered("a"));
    assert!(!state.busy("a"));
}
#[test]
fn cancelling_one_waiter_does_not_cancel_another_waiter_or_the_job() {
    let mut state = TerminalExecutionState::default();
    let cancelled = CancellationToken::new();
    let live = CancellationToken::new();
    let (a, mut ar) = oneshot::channel();
    let (b, mut br) = oneshot::channel();
    state.waiters.insert(
        "operation".into(),
        vec![
            super::TerminalExecutionWaiter {
                cancellation: cancelled.clone(),
                reply: a,
            },
            super::TerminalExecutionWaiter {
                cancellation: live.clone(),
                reply: b,
            },
        ],
    );
    cancelled.cancel();
    state.prune_waiters();
    assert_eq!(state.waiters["operation"].len(), 1);
    let result = Ok(serde_json::json!({"output":"once"}));
    state.complete_waiters("operation", &result);
    assert!(ar.try_recv().is_err());
    assert_eq!(br.try_recv().unwrap().unwrap()["output"], "once");
    assert!(!live.is_cancelled());
}

#[gpui::test]
fn inline_job_returns_before_completion_survives_request_cancellation_and_stops_once(
    cx: &mut gpui::TestAppContext,
) {
    use crate::features::capability_runtime::{CapabilityCaller, CapabilityRequest};
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::SessionLaunchConfig;
    use crate::test_support::TestConfigDir;
    use nyaterm_core::{AiPermissionMode, CapabilityScope};
    use nyaterm_mcp_protocol::tool;
    use nyaterm_transport::{SessionEvent, TelnetSessionConfig};
    use std::io::Read;
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-inline-job-contract");
    let app = app_with_visible_local_session(cx, directory.path(), "fixture");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = TelnetSessionConfig {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        raw_tcp: true,
        ..Default::default()
    };
    let (manager, session) = cx.update_entity(&app, |app, _| {
        let manager = app.session.manager_handle();
        let info = manager.create_telnet_session(config.clone()).unwrap();
        let mut metadata = app.session.metadata("fixture").unwrap().clone();
        metadata.launch_config = SessionLaunchConfig::Telnet(config);
        app.session.register_session_metadata(&info.id, metadata);
        app.terminal
            .seed_session_view(info.id.clone(), String::new(), "UTF-8");
        (manager, info.id)
    });
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let request = |tool: &str, arguments: Value| {
        let (reply, receiver) = oneshot::channel();
        (
            CapabilityRequest {
                extended_results: true,
                resolved_session_id: None,
                connection_id: "owner".into(),
                request_id: nyaterm_core::uuid(),
                generation: "generation".into(),
                client: "fixture".into(),
                permission_mode: AiPermissionMode::FullAccess,
                scope: CapabilityScope::AllSessions,
                tool: tool.into(),
                arguments,
                cancellation: CancellationToken::new(),
                approved: true,
                approval_decision: None,
                caller: CapabilityCaller::Mcp,
                reply,
            },
            receiver,
        )
    };
    let (start, mut receiver) = request(
        tool::TERMINAL_START,
        serde_json::json!({"sessionId":session,"command":"sleep 30","idempotencyKey":"once"}),
    );
    let cancellation = start.cancellation.clone();
    let job = cx.update_entity(&app, |app, cx| {
        app.dispatch_terminal_start(start, cx);
        receiver.try_recv().unwrap().unwrap()["jobId"]
            .as_str()
            .unwrap()
            .to_owned()
    });
    let input = nyaterm_core::agent_capture::build_agent_capture_command(
        nyaterm_core::AiExecutionProfile::Posix,
        &job,
        "sleep 30",
    )
    .unwrap();
    let wire = input.replace('\n', "\r");
    let mut bytes = vec![0; wire.len()];
    stream.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, wire.as_bytes());
    cx.update_entity(&app, |app, cx| {
        let (retry, mut receiver) = request(
            tool::TERMINAL_START,
            serde_json::json!({"sessionId":session,"command":"sleep 30","backend":"inline_pty","timeoutMs":300000,"idempotencyKey":"once"}),
        );
        app.dispatch_terminal_start(retry, cx);
        assert_eq!(receiver.try_recv().unwrap().unwrap()["jobId"], job);
        let (conflict, mut receiver) = request(
            tool::TERMINAL_START,
            serde_json::json!({"sessionId":session,"command":"sleep 30","timeoutMs":310000,"idempotencyKey":"once"}),
        );
        app.dispatch_terminal_start(conflict, cx);
        assert_eq!(receiver.try_recv().unwrap().unwrap_err().code, "idempotency_conflict");
    });
    cancellation.cancel();
    cx.update_entity(&app, |app, cx| {
        app.poll_terminal_executions(cx);
        assert!(app.mcp.capabilities.terminal_executions.busy(&session));
        app.session.extend_pending_events([
            SessionEvent::OutputBoundary {
                session_id: session.clone(),
                token: job.clone(),
            },
            SessionEvent::Output {
                session_id: session.clone(),
                data: format!("__DF_CMD_START_{job}__\nrunning\n").into_bytes(),
            },
        ]);
        app.drain_session_events(cx);
        let (stop, mut receiver) = request(tool::TERMINAL_STOP, serde_json::json!({"jobId":job}));
        app.dispatch_terminal_stop(stop, cx);
        assert_eq!(receiver.try_recv().unwrap().unwrap()["stopState"], "sent");
    });
    let mut interrupt = [0];
    stream.read_exact(&mut interrupt).unwrap();
    assert_eq!(interrupt, [3]);
    cx.update_entity(&app, |app, cx| {
        let (stop, mut receiver) = request(tool::TERMINAL_STOP, serde_json::json!({"jobId":job}));
        app.dispatch_terminal_stop(stop, cx);
        assert_eq!(receiver.try_recv().unwrap().unwrap()["stopState"], "sent");
        app.session.extend_pending_events([SessionEvent::Output {
            session_id: session.clone(),
            data: format!("tail\n__DF_CMD_END_{job}_130__\nprompt>").into_bytes(),
        }]);
        app.drain_session_events(cx);
        let (poll, mut receiver) = request(tool::TERMINAL_POLL, serde_json::json!({"jobId":job}));
        app.dispatch_terminal_poll(poll);
        let result = receiver.try_recv().unwrap().unwrap();
        assert_eq!(result["status"], "cancelled");
        assert_eq!(result["processState"], "exit_confirmed");
        assert_eq!(result["terminalState"], "ready");
        assert!(result["output"].as_str().unwrap().contains("running"));
        assert!(result["output"].as_str().unwrap().contains("tail"));
        assert!(!result["output"].as_str().unwrap().contains("prompt>"));
        let (stop, mut receiver) = request(tool::TERMINAL_STOP, serde_json::json!({"jobId":job}));
        app.dispatch_terminal_stop(stop, cx);
        let result = receiver.try_recv().unwrap().unwrap();
        assert_eq!(result["stopState"], "confirmed_ended");
        assert_eq!(result["processState"], "exit_confirmed");
    });
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert!(stream.read(&mut interrupt).is_err());
    manager.close(&session).unwrap();
    cx.update_entity(&app, |app, cx| {
        app.session.metadata_mut(&session).unwrap().disconnected = true;
        let (stop, mut receiver) = request(tool::TERMINAL_STOP, serde_json::json!({"jobId":job}));
        app.dispatch_terminal_stop(stop, cx);
        let result = receiver.try_recv().unwrap().unwrap();
        assert_eq!(result["stopState"], "target_invalid");
        assert_eq!(result["terminalState"], "disconnected");
    });
}

#[test]
fn mcp_capture_blocks_navigation_fallback_even_with_a_visible_shell_prompt() {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::TestAppContext;
    let dir = TestConfigDir::new("nyaterm-navigation-mcp-capture");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, _| {
        app.terminal
            .seed_session_view("s1".into(), "$ echo visible".into(), "UTF-8");
        assert!(app.prepare_command_navigation_input(b"\r").is_some());
        let (request, _receiver) = execution_request("capture", "owner", "s1", false);
        let mut capture = AgentOutputCaptureProcessor::new();
        capture.register("capture".into());
        assert!(
            app.mcp
                .capabilities
                .terminal_executions
                .insert(request, Some(capture))
                .is_ok()
        );
        assert!(app.prepare_command_navigation_input(b"\r").is_none());
    });
}
