use nyaterm_mcp_protocol::tool;

use super::{
    CapabilityApprovalRequest, CapabilityCaller, CapabilityRequest, CapabilityRuntimeState,
    mcp_request_target,
};

/// A read-only projection. Authorization and cancellation remain owned by the
/// capability queue; the transcript never owns a second pending request.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::features) struct NativeCapabilityApproval {
    pub run_id: String,
    pub ai_session_id: String,
    pub approval: CapabilityApprovalRequest,
    pub command: Option<String>,
}

impl CapabilityRuntimeState {
    pub(in crate::features) fn cancel_native_approvals_for_target(
        &mut self,
        session_id: &str,
    ) -> Vec<CapabilityRequest> {
        let ids: Vec<_> = self
            .pending_approvals
            .iter()
            .filter(|(_, pending)| {
                matches!(pending.request.caller, CapabilityCaller::Native { .. })
                    && mcp_request_target(&pending.request).as_deref() == Some(session_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .flat_map(|id| self.request_cancelled(&id))
            .collect()
    }

    pub(in crate::features) fn external_approval_requests(&self) -> Vec<CapabilityApprovalRequest> {
        self.approval_order
            .iter()
            .filter_map(|id| self.pending_approvals.get(id))
            .filter(|pending| {
                matches!(pending.request.caller, CapabilityCaller::Mcp)
                    && !pending.request.cancellation.is_cancelled()
            })
            .map(|pending| pending.approval.clone())
            .collect()
    }

    pub(in crate::features) fn native_approval_requests(
        &self,
        run_id: &str,
        ai_session_id: &str,
    ) -> Vec<NativeCapabilityApproval> {
        self.approval_order
            .iter()
            .filter_map(|id| self.pending_approvals.get(id))
            .filter(|pending| {
                matches!(&pending.request.caller, CapabilityCaller::Native {
                    run_id: owner_run, ai_session_id: owner_session, ..
                } if owner_run == run_id && owner_session == ai_session_id)
                    && !pending.request.cancellation.is_cancelled()
            })
            .map(|pending| NativeCapabilityApproval {
                run_id: run_id.to_string(),
                ai_session_id: ai_session_id.to_string(),
                approval: pending.approval.clone(),
                command: (pending.request.tool == tool::TERMINAL_EXECUTE)
                    .then(|| {
                        pending.request.arguments["command"]
                            .as_str()
                            .map(str::to_string)
                    })
                    .flatten(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        CapabilityApprovalOutcome, CapabilityApprovalRequest, CapabilityCaller, CapabilityRequest,
        CapabilityRuntimeState, PendingCapabilityApproval, mcp_grant_key,
    };
    use gpui::AppContext as _;
    use nyaterm_core::AiPermissionMode;
    use nyaterm_core::capabilities::{AgentApprovalDecision, CapabilityScope};
    use nyaterm_mcp_protocol::tool;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn native(run: &str, session: &str) -> CapabilityCaller {
        CapabilityCaller::Native {
            run_id: run.into(),
            ai_session_id: session.into(),
            model_risk: None,
        }
    }

    fn pending(
        id: &str,
        caller: CapabilityCaller,
        allow_session: bool,
    ) -> PendingCapabilityApproval {
        let (reply, _) = tokio::sync::oneshot::channel();
        let request = CapabilityRequest {
            request_id: id.into(),
            connection_id: match &caller {
                CapabilityCaller::Mcp => "external".into(),
                CapabilityCaller::Native { run_id, .. } => format!("native:{run_id}"),
            },
            generation: String::new(),
            client: "fixture".into(),
            permission_mode: AiPermissionMode::Confirm,
            scope: CapabilityScope::AllSessions,
            tool: tool::TERMINAL_EXECUTE.into(),
            arguments: json!({"sessionId":"target", "command":"echo 猫\n".repeat(60)}),
            cancellation: CancellationToken::new(),
            approved: false,
            approval_decision: None,
            caller,
            reply,
        };
        PendingCapabilityApproval {
            approval: CapabilityApprovalRequest {
                request_id: id.into(),
                client: "fixture".into(),
                capability: request.tool.clone(),
                target: Some("target".into()),
                parameter_summary: "echo 猫…".into(),
                risk_level: "low".into(),
                destructive: !allow_session,
                allow_session,
            },
            grant_key: mcp_grant_key(&request),
            request,
        }
    }

    #[test]
    fn projections_keep_external_fifo_and_isolate_native_run_and_conversation() {
        let mut state = CapabilityRuntimeState::default();
        for (id, caller) in [
            ("one", CapabilityCaller::Mcp),
            ("native", native("run", "chat")),
            ("other-run", native("other", "chat")),
            ("other-chat", native("run", "other")),
            ("two", CapabilityCaller::Mcp),
        ] {
            state.queue_approval(pending(id, caller, true));
        }
        let external = state.external_approval_requests();
        assert_eq!(
            external
                .iter()
                .map(|r| r.request_id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        let views = state.native_approval_requests("run", "chat");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].approval.request_id, "native");
        assert_eq!(
            views[0].command.as_deref(),
            Some("echo 猫\n".repeat(60).as_str())
        );
        assert!(views[0].approval.allow_session);

        state.pending_approvals["one"].request.cancellation.cancel();
        state.pending_approvals["native"]
            .request
            .cancellation
            .cancel();
        assert_eq!(state.external_approval_requests()[0].request_id, "two");
        assert!(state.native_approval_requests("run", "chat").is_empty());
        assert_eq!(
            state.pending_approvals.len(),
            5,
            "projections must not own or mutate requests"
        );
    }

    #[test]
    fn file_authorization_decisions_are_consumed_once_and_keep_existing_grant_rules() {
        let mut state = CapabilityRuntimeState::default();
        let mut file = pending("write", native("run", "chat"), true);
        file.request.tool = tool::SFTP_WRITE_TEXT.into();
        file.request.arguments =
            json!({"sessionId":"target","path":"/tmp/file","content":"fixture"});
        file.approval.capability = file.request.tool.clone();
        file.approval.parameter_summary = "write 7 bytes to /tmp/file".into();
        file.grant_key = mcp_grant_key(&file.request);
        let grant = file.grant_key.clone().unwrap();
        state.queue_approval(file);
        let views = state.native_approval_requests("run", "chat");
        assert!(views[0].command.is_none());
        assert_eq!(
            views[0].approval.parameter_summary,
            "write 7 bytes to /tmp/file"
        );
        let Some(CapabilityApprovalOutcome::Dispatch(request)) =
            state.decide_approval("write", AgentApprovalDecision::AllowSession)
        else {
            panic!("file approval must dispatch");
        };
        assert!(request.approved);
        assert!(state.has_session_grant(&grant));
        assert!(state.native_approval_requests("run", "chat").is_empty());
        assert!(
            state
                .decide_approval("write", AgentApprovalDecision::AllowOnce)
                .is_none()
        );

        let destructive = pending("delete", native("run", "chat"), false);
        let forbidden_grant = destructive.grant_key.clone().unwrap();
        state.queue_approval(destructive);
        assert!(
            !state.native_approval_requests("run", "chat")[0]
                .approval
                .allow_session
        );
        let Some(CapabilityApprovalOutcome::Dispatch(request)) =
            state.decide_approval("delete", AgentApprovalDecision::AllowSession)
        else {
            panic!("destructive requests only allow once");
        };
        assert_eq!(request.approval_decision.as_deref(), Some("allow_once"));
        assert!(!state.has_session_grant(&forbidden_grant));

        state.queue_approval(pending("denied", native("run", "chat"), true));
        assert!(matches!(
            state.decide_approval("denied", AgentApprovalDecision::Deny),
            Some(CapabilityApprovalOutcome::Denied(_))
        ));
        let cancelled = pending("cancelled", native("run", "chat"), true);
        cancelled.request.cancellation.cancel();
        state.queue_approval(cancelled);
        assert!(
            state
                .decide_approval("cancelled", AgentApprovalDecision::AllowOnce)
                .is_none()
        );
        state.queue_approval(pending("closed", native("run", "chat"), true));
        state.connection_disconnected("native:run");
        assert!(state.native_approval_requests("run", "chat").is_empty());
    }

    #[test]
    fn closing_a_target_revokes_native_approvals_in_other_conversations_only() {
        let mut state = CapabilityRuntimeState::default();
        state.queue_approval(pending("native", native("run", "other-terminal"), true));
        state.queue_approval(pending("mcp", CapabilityCaller::Mcp, true));
        let mut other = pending("other-target", native("other-run", "chat"), true);
        other.request.arguments["sessionId"] = json!("still-open");
        state.queue_approval(other);
        let cancelled = state.cancel_native_approvals_for_target("target");
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].request_id, "native");
        assert!(cancelled[0].cancellation.is_cancelled());
        assert!(
            state
                .native_approval_requests("run", "other-terminal")
                .is_empty()
        );
        assert_eq!(state.external_approval_requests().len(), 1);
        assert_eq!(state.native_approval_requests("other-run", "chat").len(), 1);
        assert!(
            state
                .decide_approval("native", AgentApprovalDecision::AllowOnce)
                .is_none()
        );
    }

    #[gpui::test]
    fn native_command_approval_becomes_one_execution_card_and_ignores_repeat_clicks(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use nyaterm_core::ai::harness::AgentToolRegistry;
        use nyaterm_core::{AgentCommandExecutionMode, AiMode};
        let _cleanup = TestAppCleanup::new(cx);
        let directory = TestConfigDir::new("nyaterm-native-approval-execution-card");
        let app = app_with_visible_local_session(cx, directory.path(), "s");
        cx.update_entity(&app, |app, cx| {
            let mut settings = app.ai.settings_config_cloned();
            settings.agent_command_execution_mode = AgentCommandExecutionMode::ConfirmEach;
            app.ai.replace_settings_config(settings, true);
            app.ai.begin_chat_request("inspect".into(), AiMode::Agent, None);
            app.ai.begin_native_run(serde_json::from_value(json!({
                "action":"generate_command", "mode":"agent", "userInput":"inspect",
                "targets":[{"terminalSessionId":"s","label":"fixture","sessionType":"local"}]
            })).unwrap());
            let call = AgentToolRegistry::parse(&[], r#"{"action":"terminal_execute","arguments":{"sessionId":"s","command":"echo fixture"}}"#).unwrap();
            app.dispatch_native_agent_call(call, cx);
            let run = app.ai.native_run_view().unwrap();
            let chat = app.ai.chat_session_id().to_string();
            let views = app.mcp.capabilities.native_approval_requests(&run.run_id, &chat);
            assert_eq!(views.len(), 1);
            assert!(!views[0].approval.allow_session);
            assert!(app.mcp_pending_approval_requests().is_empty());
            assert!(app.ai.chat_command_cards().is_empty());
            let id = &views[0].approval.request_id;
            app.respond_to_native_capability_approval(&run.run_id, &chat, id, AgentApprovalDecision::AllowOnce, cx);
            assert!(app.mcp.capabilities.native_approval_requests(&run.run_id, &chat).is_empty());
            assert_eq!(app.ai.chat_command_cards().len(), 1);
            assert_eq!(app.ai.chat_command_cards()[0].command, "echo fixture");
            app.respond_to_native_capability_approval(&run.run_id, &chat, id, AgentApprovalDecision::AllowOnce, cx);
            assert_eq!(app.ai.chat_command_cards().len(), 1);
            // The fixture has no PTY. Stop before its synthetic execution result
            // starts a provider continuation; this test exercises dispatch UI.
            app.cancel_ai_chat(cx);
        });
    }
}
