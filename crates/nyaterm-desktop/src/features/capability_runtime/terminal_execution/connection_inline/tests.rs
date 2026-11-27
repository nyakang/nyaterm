use super::PendingInlineStart;
use crate::features::capability_runtime::terminal_execution::TerminalExecutionPolicy;
use crate::features::capability_runtime::terminal_execution::TerminalExecutionState;
use crate::features::capability_runtime::{
    CapabilityApprovalOutcome, CapabilityCaller, CapabilityRequest,
};
use crate::features::mcp::server::rpc_failure;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::SessionLaunchConfig;
use crate::test_support::TestConfigDir;
use gpui::AppContext as _;
use nyaterm_core::ai::mcp_execution::McpTerminalExecutionMode;
use nyaterm_core::capabilities::{AgentApprovalDecision, CapabilityScope};
use nyaterm_core::models::connection::ConnectionType;
use nyaterm_core::models::sessions::SavedConnection;
use nyaterm_core::{AiExecutionProfile, AiPermissionMode};
use nyaterm_mcp_protocol::{SessionOpenResult, TerminalBackend, TerminalStartArgs, tool};
use nyaterm_transport::TelnetSessionConfig;
use serde_json::{Value, json};
use std::io::Read;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn request(
    generation: &str,
    arguments: Value,
) -> (
    CapabilityRequest,
    oneshot::Receiver<Result<Value, nyaterm_mcp_protocol::RpcError>>,
) {
    let (reply, receiver) = oneshot::channel();
    (
        CapabilityRequest {
            terminal_plan: None,
            extended_results: true,
            resolved_session_id: None,
            connection_id: "public-owner".into(),
            request_id: nyaterm_core::uuid(),
            generation: generation.into(),
            client: "fixture".into(),
            permission_mode: AiPermissionMode::FullAccess,
            scope: CapabilityScope::AllSessions,
            tool: tool::TERMINAL_START.into(),
            arguments,
            cancellation: CancellationToken::new(),
            approved: false,
            approval_decision: None,
            caller: CapabilityCaller::Mcp,
            reply,
        },
        receiver,
    )
}

fn saved_connection() -> SavedConnection {
    serde_json::from_value(json!({"id":"saved", "name":"fixture", "type":"ssh", "host":"127.0.0.1", "username":"fixture"})).unwrap()
}

#[test]
fn preparing_inline_jobs_count_towards_client_limits_and_shutdown_cancels_opening() {
    let mut state = TerminalExecutionState::default();
    let mut opening_tokens = Vec::new();
    for index in 0..8 {
        assert!(state.can_register_job("public-owner"));
        let (request, _) = request(
            "generation",
            json!({"target":{"type":"connection","connectionId":"saved"},"command":"pwd"}),
        );
        let args: TerminalStartArgs = serde_json::from_value(request.arguments.clone()).unwrap();
        let opening = request.cancellation.child_token();
        opening_tokens.push(opening.clone());
        let (_, receiver) = oneshot::channel();
        state.pending_inline_starts.insert(
            index.to_string(),
            PendingInlineStart {
                request,
                args,
                deadline: Instant::now() + Duration::from_secs(30),
                opening,
                receiver,
            },
        );
    }
    assert!(!state.can_register_job("public-owner"));
    assert!(state.can_register_job("other-owner"));
    state.request_background_shutdown();
    assert!(opening_tokens.iter().all(CancellationToken::is_cancelled));
}

#[test]
fn revoking_http_credentials_stops_only_their_jobs_and_pending_starts() {
    use crate::features::capability_runtime::CapabilityRuntimeState;
    use crate::features::capability_runtime::terminal_execution::jobs::TerminalJob;
    use nyaterm_mcp_protocol::{ExecutionTarget, TerminalJobStatus};

    let mut state = CapabilityRuntimeState::default();
    let mut tokens = Vec::new();
    for (owner, generation) in [("http-client", "http"), ("embedded-agent", "agent")] {
        state
            .owners
            .insert(owner.into(), (CapabilityCaller::Mcp, generation.into()));
        let (mut request, _reply) = request(
            generation,
            json!({"target":{"type":"connection","connectionId":"saved"},"command":"pwd"}),
        );
        request.connection_id = owner.into();
        let args = serde_json::from_value(request.arguments.clone()).unwrap();
        let opening = request.cancellation.child_token();
        let (_, receiver) = oneshot::channel();
        let job = TerminalJob::new(
            true,
            owner.into(),
            ExecutionTarget::Connection {
                connection_id: "saved".into(),
            },
            TerminalBackend::SshExec,
            None,
            "pwd".into(),
            "fixture".into(),
            "saved".into(),
        );
        tokens.push((opening.clone(), job.interrupt.clone()));
        state.terminal_executions.jobs.insert(owner.into(), job);
        state.terminal_executions.pending_inline_starts.insert(
            owner.into(),
            PendingInlineStart {
                request,
                args,
                deadline: Instant::now() + Duration::from_secs(30),
                opening,
                receiver,
            },
        );
    }

    state.revoke_generation("http");
    assert!(tokens[0].0.is_cancelled());
    assert!(tokens[0].1.is_cancelled());
    assert!(!tokens[1].0.is_cancelled());
    assert!(!tokens[1].1.is_cancelled());
    assert_eq!(
        state.terminal_executions.jobs["http-client"].status,
        TerminalJobStatus::Cancelling
    );
    assert!(state.owners.contains_key("embedded-agent"));

    state.expire_mcp_owner("embedded-agent");
    assert!(tokens[1].0.is_cancelled());
    assert!(tokens[1].1.is_cancelled());
}

#[gpui::test]
fn public_settings_override_clients_and_remain_frozen_while_approval_is_pending(
    cx: &mut gpui::TestAppContext,
) {
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-mcp-execution-policy-approval");
    let app = app_with_visible_local_session(cx, directory.path(), "existing");
    cx.update_entity(&app, |app, cx| {
        let generation = app.mcp.enable_public_for_test();
        app.connection_state.replace_loaded(vec![saved_connection()], Vec::new());
        app.ai.set_settings_terminal_output_lines(17);
        app.ai.set_settings_mcp_terminal_execution_mode(McpTerminalExecutionMode::Inline);
        let (mut request, _receiver) = request(&generation, json!({"target":{"type":"connection","connectionId":"saved"},"command":"pwd","backend":"ssh_exec"}));
        request.permission_mode = AiPermissionMode::Confirm;
        let id = request.request_id.clone();
        app.dispatch_capability_request(request, cx);
        let approval = app.mcp.capabilities.external_approval_request(&id).unwrap();
        assert_eq!(approval.risk_level, "medium");
        assert!(approval.parameter_summary.contains("reuse or open"));
        app.ai.set_settings_mcp_terminal_execution_mode(McpTerminalExecutionMode::Background);
        app.ai.set_settings_terminal_output_lines(0);
        let Some(CapabilityApprovalOutcome::Dispatch(request)) = app.mcp.capabilities.decide_approval(&id, AgentApprovalDecision::AllowOnce) else { panic!("approved request") };
        assert_eq!(request.terminal_plan.unwrap().backend, TerminalBackend::InlinePty);
        assert_eq!(request.terminal_plan.unwrap().policy.max_lines, 17);
        let (next, _) = self::request(&generation, request.arguments.clone());
        assert_eq!(app.resolve_public_mcp_terminal_plan(&next).unwrap().backend, TerminalBackend::SshExec);
        let credential = app.create_ephemeral_mcp_credential(vec!["existing".into()], Some("existing".into()), AiPermissionMode::FullAccess, TerminalExecutionPolicy { background: false, max_lines: 3 }).unwrap();
        let internal_generation = credential.environment().remove("NYATERM_MCP_GENERATION").unwrap();
        let (mut internal, _) = self::request(&internal_generation, json!({"sessionId":"existing","command":"pwd"}));
        internal.tool = tool::TERMINAL_EXECUTE.into();
        assert!(app.resolve_public_mcp_terminal_plan(&internal).is_none());
    });
}

#[gpui::test]
fn public_background_execute_rejects_local_sessions_without_falling_back(
    cx: &mut gpui::TestAppContext,
) {
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-mcp-background-local-rejection");
    let app = app_with_visible_local_session(cx, directory.path(), "existing");
    let mut receiver = cx.update_entity(&app, |app, cx| {
        let generation = app.mcp.enable_public_for_test();
        app.ai
            .set_settings_mcp_terminal_execution_mode(McpTerminalExecutionMode::Background);
        let (mut request, receiver) =
            request(&generation, json!({"sessionId":"existing","command":"pwd"}));
        request.tool = tool::TERMINAL_EXECUTE.into();
        app.dispatch_capability_request(request, cx);
        assert!(app.mcp.capabilities.terminal_executions.jobs.is_empty());
        receiver
    });
    cx.run_until_parked();
    assert_eq!(
        receiver.try_recv().unwrap().unwrap_err().code,
        "invalid_argument"
    );
}

#[gpui::test]
fn saved_inline_reuses_a_terminal_and_retries_keep_the_original_job_after_settings_change(
    cx: &mut gpui::TestAppContext,
) {
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-mcp-saved-inline-retry");
    let app = app_with_visible_local_session(cx, directory.path(), "existing");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = TelnetSessionConfig {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        raw_tcp: true,
        ..Default::default()
    };
    // A deterministic terminal byte stream exercises PTY submission without SSH authentication.
    let (manager, session, generation) = cx.update_entity(&app, |app, _| {
        let manager = app.session.manager_handle();
        let info = manager.create_telnet_session(config.clone()).unwrap();
        let mut metadata = app.session.metadata("existing").unwrap().clone();
        metadata.launch_config = SessionLaunchConfig::Telnet(config);
        metadata.source_connection_id = Some("saved".into());
        app.session.register_session_metadata(&info.id, metadata);
        app.terminal
            .seed_session_view(info.id.clone(), String::new(), "UTF-8");
        app.connection_state
            .replace_loaded(vec![saved_connection()], Vec::new());
        app.ai
            .set_settings_mcp_terminal_execution_mode(McpTerminalExecutionMode::Inline);
        let generation = app.mcp.enable_public_for_test();
        (manager, info.id, generation)
    });
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let arguments = json!({"target":{"type":"connection","connectionId":"saved"}, "command":"pwd", "backend":"ssh_exec", "idempotencyKey":"once"});
    let (start, mut receiver) = request(&generation, arguments.clone());
    let cancellation = start.cancellation.clone();
    let sessions = cx.update_entity(&app, |app, cx| {
        let sessions = app
            .session
            .session_ids()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        app.dispatch_capability_request(start, cx);
        app.poll_terminal_executions(cx);
        sessions
    });
    cx.run_until_parked();
    let started = receiver.try_recv().unwrap().unwrap();
    assert_eq!(started["backend"], "inline_pty");
    assert_eq!(started["sessionId"], session);
    cx.update_entity(&app, |app, _| {
        assert_eq!(
            app.session
                .session_ids()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            sessions
        );
    });
    let job = started["jobId"].as_str().unwrap();
    let expected = nyaterm_core::agent_capture::build_agent_capture_command(
        AiExecutionProfile::Posix,
        job,
        "pwd",
    )
    .unwrap()
    .replace('\n', "\r");
    let mut bytes = vec![0; expected.len()];
    stream.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, expected.as_bytes());
    cancellation.cancel();
    let (retry, mut receiver) = request(&generation, arguments.clone());
    cx.update_entity(&app, |app, cx| {
        app.ai
            .set_settings_mcp_terminal_execution_mode(McpTerminalExecutionMode::Background);
        app.dispatch_capability_request(retry, cx);
        assert!(app.mcp.capabilities.terminal_executions.busy(&session));
    });
    cx.run_until_parked();
    assert_eq!(receiver.try_recv().unwrap().unwrap(), started);
    let mut conflicting = arguments;
    conflicting["command"] = json!("uname");
    let (retry, mut receiver) = request(&generation, conflicting);
    cx.update_entity(&app, |app, cx| app.dispatch_capability_request(retry, cx));
    cx.run_until_parked();
    assert_eq!(
        receiver.try_recv().unwrap().unwrap_err().code,
        "idempotency_conflict"
    );
    stream.set_nonblocking(true).unwrap();
    let mut extra = [0; 1];
    assert_eq!(
        stream.read(&mut extra).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    manager.close(&session).unwrap();
}

#[gpui::test]
fn pending_saved_inline_starts_cancel_or_timeout_without_submitting_commands(
    cx: &mut gpui::TestAppContext,
) {
    cx.executor().allow_parking();
    let directory = TestConfigDir::new("nyaterm-mcp-saved-inline-cancellation");
    let app = app_with_visible_local_session(cx, directory.path(), "existing");
    cx.update_entity(&app, |app, cx| {
        let generation = app.mcp.enable_public_for_test();
        app.connection_state
            .replace_loaded(vec![saved_connection()], Vec::new());
        for (index, expected) in [
            (0, "cancelled"),
            (1, "execution_failed"),
            (2, "authentication_failed"),
            (3, "execution_failed"),
            (4, "scope_denied"),
            (5, "execution_failed"),
            (6, "permission_denied"),
        ] {
            let (mut request, mut receiver) = self::request(
                &generation,
                json!({"target":{"type":"connection","connectionId":"saved"},"command":"pwd"}),
            );
            let opening = request.cancellation.child_token();
            if index == 0 {
                request.cancellation.cancel();
            }
            if index == 2 {
                request.generation = "expired".into();
            }
            if index == 6 {
                let mut connection = saved_connection();
                let ConnectionType::Ssh {
                    ai_execution_profile,
                    ..
                } = &mut connection.config
                else {
                    unreachable!()
                };
                *ai_execution_profile = AiExecutionProfile::Disabled;
                app.connection_state
                    .replace_loaded(vec![connection], Vec::new());
            }
            let (reply, opened) = oneshot::channel();
            if index == 3 {
                reply
                    .send(Err(rpc_failure("execution_failed", "fixture open failed")))
                    .unwrap();
            } else if index == 4 {
                request.scope = CapabilityScope::explicit(Vec::<String>::new(), None);
                reply
                    .send(Ok(SessionOpenResult {
                        session_id: "existing".into(),
                        connection_id: "saved".into(),
                        name: "fixture".into(),
                        r#type: "ssh".into(),
                        connected: true,
                    }))
                    .unwrap();
            } else {
                drop(reply);
            }
            let args: TerminalStartArgs =
                serde_json::from_value(request.arguments.clone()).unwrap();
            app.mcp
                .capabilities
                .terminal_executions
                .pending_inline_starts
                .insert(
                    format!("pending-{index}"),
                    PendingInlineStart {
                        request,
                        args,
                        deadline: if index == 1 {
                            Instant::now() - Duration::from_secs(1)
                        } else {
                            Instant::now() + Duration::from_secs(30)
                        },
                        opening: opening.clone(),
                        receiver: opened,
                    },
                );
            app.poll_connection_inline_starts(cx);
            assert_eq!(receiver.try_recv().unwrap().unwrap_err().code, expected);
            assert!(opening.is_cancelled());
            assert!(app.mcp.capabilities.terminal_executions.jobs.is_empty());
        }
        assert!(
            !app.mcp
                .capabilities
                .terminal_executions
                .has_pending_inline_starts()
        );
    });
}
