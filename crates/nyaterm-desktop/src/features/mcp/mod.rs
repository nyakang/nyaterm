mod discovery;
pub(in crate::features) mod server;

use self::server::{McpHostEvent, McpHostRuntime, rpc_failure};
use super::NyaTermApp;
use super::ai::{McpHelperStatus, mcp_helper_status};
use futures::StreamExt;
use gpui::Context;
use nyaterm_core::{CapabilityScope, ExternalMcpSessionScope, ExternalMcpSettings};
use std::collections::HashMap;

pub(in crate::features) use nyaterm_core::capabilities::AgentApprovalDecision as McpApprovalDecision;

use super::capability_runtime::CapabilityRuntimeState;
use super::capability_runtime::terminal_execution::TerminalExecutionPolicy;
pub(in crate::features) use super::capability_runtime::{
    CapabilityApprovalOutcome as McpApprovalOutcome,
    CapabilityApprovalRequest as McpApprovalRequest,
};

const MCP_WINDOW_OWNER_PREFIX: &str = "mcp-window-";

pub(in crate::features) struct McpHostFeatureState {
    runtime: Option<McpHostRuntime>,
    helper_status: McpHelperStatus,
    requests: Option<futures::channel::mpsc::UnboundedReceiver<McpHostEvent>>,
    request_sender: futures::channel::mpsc::UnboundedSender<McpHostEvent>,
    pub(in crate::features) owner_window_label: String,
    generation: Option<String>,
    ephemeral_generations:
        std::sync::Arc<std::sync::Mutex<HashMap<String, TerminalExecutionPolicy>>>,
    pub(in crate::features) capabilities: CapabilityRuntimeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum McpHostStatus {
    Disabled,
    Running,
    Unavailable,
}

pub(in crate::features) struct McpEphemeralCredential {
    runtime: McpHostRuntime,
    generation: String,
    generation_registry: std::sync::Arc<std::sync::Mutex<HashMap<String, TerminalExecutionPolicy>>>,
    events: futures::channel::mpsc::UnboundedSender<McpHostEvent>,
}

impl McpEphemeralCredential {
    pub(in crate::features) fn environment(&self) -> HashMap<String, String> {
        let endpoint = self.runtime.endpoint();
        HashMap::from([
            ("NYATERM_MCP_EPHEMERAL".to_string(), "1".to_string()),
            ("NYATERM_MCP_HOST".to_string(), "127.0.0.1".to_string()),
            ("NYATERM_MCP_PORT".to_string(), endpoint.port.to_string()),
            ("NYATERM_MCP_TOKEN".to_string(), endpoint.token.clone()),
            (
                "NYATERM_MCP_GENERATION".to_string(),
                endpoint.generation.clone(),
            ),
        ])
    }
}

impl std::fmt::Debug for McpEphemeralCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpEphemeralCredential")
            .field("endpoint", &"[REDACTED]")
            .finish()
    }
}

impl Drop for McpEphemeralCredential {
    fn drop(&mut self) {
        if let Ok(mut generations) = self.generation_registry.lock() {
            generations.remove(&self.generation);
        }
        let _ = self.events.unbounded_send(McpHostEvent::GenerationExpired {
            generation: self.generation.clone(),
        });
    }
}

impl McpHostFeatureState {
    pub fn new(settings: &ExternalMcpSettings) -> Self {
        let owner_window_label = format!("{MCP_WINDOW_OWNER_PREFIX}{}", uuid::Uuid::new_v4());
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let helper_status = mcp_helper_status();
        if !settings.enabled {
            return Self::with_channel(owner_window_label, sender, receiver, helper_status);
        }
        let scope = match settings.session_scope {
            ExternalMcpSessionScope::CurrentWindow => CapabilityScope::CurrentWindow {
                owner_window_label: owner_window_label.clone(),
            },
            ExternalMcpSessionScope::AllSessions => CapabilityScope::AllSessions,
        };
        let runtime = discovery::default_config_dir().and_then(|directory| {
            McpHostRuntime::start(
                settings.permission_mode.clone(),
                scope,
                sender.clone(),
                Some(discovery::DiscoveryStore::new(&directory)),
            )
        });
        match runtime {
            Ok(runtime) => {
                debug_assert!(!runtime.endpoint().token.is_empty());
                let generation = Some(runtime.endpoint().generation.clone());
                Self {
                    runtime: Some(runtime),
                    helper_status,
                    requests: Some(receiver),
                    request_sender: sender,
                    owner_window_label,
                    generation,
                    ephemeral_generations: Default::default(),
                    capabilities: CapabilityRuntimeState::default(),
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "MCP Host failed to start");
                Self::with_channel(owner_window_label, sender, receiver, helper_status)
            }
        }
    }

    fn with_channel(
        owner_window_label: String,
        request_sender: futures::channel::mpsc::UnboundedSender<McpHostEvent>,
        requests: futures::channel::mpsc::UnboundedReceiver<McpHostEvent>,
        helper_status: McpHelperStatus,
    ) -> Self {
        Self {
            runtime: None,
            helper_status,
            requests: Some(requests),
            request_sender,
            owner_window_label,
            generation: None,
            ephemeral_generations: Default::default(),
            capabilities: CapabilityRuntimeState::default(),
        }
    }

    fn create_ephemeral(
        &self,
        session_ids: Vec<String>,
        default_session_id: Option<String>,
        permission_mode: nyaterm_core::AiPermissionMode,
        policy: TerminalExecutionPolicy,
    ) -> Result<McpEphemeralCredential, String> {
        let sender = self.request_sender.clone();
        let runtime = McpHostRuntime::start(
            permission_mode,
            CapabilityScope::explicit(session_ids, default_session_id),
            sender,
            None,
        )
        .map_err(|error| error.to_string())?;
        let generation = runtime.endpoint().generation.clone();
        self.ephemeral_generations
            .lock()
            .map_err(|_| "MCP ephemeral generation registry is unavailable".to_string())?
            .insert(generation.clone(), policy);
        Ok(McpEphemeralCredential {
            runtime,
            generation,
            generation_registry: self.ephemeral_generations.clone(),
            events: self.request_sender.clone(),
        })
    }

    pub(in crate::features) fn terminal_policy(
        &self,
        generation: &str,
    ) -> Option<TerminalExecutionPolicy> {
        self.ephemeral_generations
            .lock()
            .ok()?
            .get(generation)
            .copied()
    }

    fn take_request_receiver(
        &mut self,
    ) -> Option<futures::channel::mpsc::UnboundedReceiver<McpHostEvent>> {
        self.requests.take()
    }

    pub(in crate::features) fn generation_matches(&self, generation: &str) -> bool {
        (self.generation.as_deref() == Some(generation) && self.runtime.is_some())
            || self
                .ephemeral_generations
                .lock()
                .is_ok_and(|generations| generations.contains_key(generation))
    }

    #[cfg(test)]
    fn disabled_for_test() -> Self {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        Self::with_channel(
            "test-window".to_string(),
            sender,
            receiver,
            McpHelperStatus::Missing,
        )
    }

    fn status(&self, enabled: bool) -> McpHostStatus {
        if self.runtime.is_some() {
            McpHostStatus::Running
        } else if enabled {
            McpHostStatus::Unavailable
        } else {
            McpHostStatus::Disabled
        }
    }

    fn reconfigure(&mut self, settings: &ExternalMcpSettings) -> Result<(), String> {
        self.helper_status = mcp_helper_status();
        self.runtime.take();
        if let Some(generation) = self.generation.take() {
            self.capabilities.revoke_generation(&generation);
        }
        if !settings.enabled {
            return Ok(());
        }
        let scope = match settings.session_scope {
            ExternalMcpSessionScope::CurrentWindow => CapabilityScope::CurrentWindow {
                owner_window_label: self.owner_window_label.clone(),
            },
            ExternalMcpSessionScope::AllSessions => CapabilityScope::AllSessions,
        };
        let directory = discovery::default_config_dir().map_err(|error| error.to_string())?;
        let runtime = McpHostRuntime::start(
            settings.permission_mode.clone(),
            scope,
            self.request_sender.clone(),
            Some(discovery::DiscoveryStore::new(&directory)),
        )
        .map_err(|error| error.to_string())?;
        self.generation = Some(runtime.endpoint().generation.clone());
        self.runtime = Some(runtime);
        Ok(())
    }
}
impl NyaTermApp {
    pub(in crate::features) fn start_mcp_host_request_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut receiver) = self.mcp.take_request_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                if this
                    .update(cx, |this, cx| match event {
                        McpHostEvent::Execute(request) => {
                            this.dispatch_capability_request(*request, cx)
                        }
                        McpHostEvent::Cancelled { request_id } => {
                            this.handle_pending_mcp_cancel(&request_id, cx)
                        }
                        McpHostEvent::Disconnected { connection_id } => {
                            this.handle_mcp_disconnect(&connection_id, cx)
                        }
                        McpHostEvent::GenerationExpired { generation } => {
                            this.mcp
                                .capabilities
                                .terminal_executions
                                .forget_owner(&generation);
                            this.mcp.capabilities.revoke_generation(&generation);
                            this.poll_terminal_executions(cx);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn mcp_pending_approval_requests(&self) -> Vec<McpApprovalRequest> {
        self.mcp.capabilities.external_approval_requests()
    }

    pub(in crate::features) fn respond_to_mcp_approval(
        &mut self,
        request_id: &str,
        decision: McpApprovalDecision,
        cx: &mut Context<Self>,
    ) {
        match self.mcp.capabilities.decide_approval(request_id, decision) {
            Some(McpApprovalOutcome::Dispatch(request)) => {
                self.dispatch_capability_request(request, cx)
            }
            Some(McpApprovalOutcome::Denied(request)) => {
                let assessment =
                    nyaterm_core::capabilities::assess_tool_risk(&request.tool, &request.arguments);
                self.record_mcp_audit(
                    &request,
                    assessment.as_ref(),
                    Some("deny"),
                    Some(false),
                    true,
                    None,
                    Some("approval denied"),
                    cx,
                );
                let _ = request.reply.send(Err(rpc_failure(
                    "approval_denied",
                    "The MCP capability request was denied.",
                )));
            }
            None => {}
        }
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    fn handle_pending_mcp_cancel(&mut self, request_id: &str, cx: &mut Context<Self>) {
        for request in self.mcp.capabilities.request_cancelled(request_id) {
            let assessment =
                nyaterm_core::capabilities::assess_tool_risk(&request.tool, &request.arguments);
            self.record_mcp_audit(
                &request,
                assessment.as_ref(),
                None,
                Some(false),
                true,
                None,
                Some("request cancelled"),
                cx,
            );
            let _ = request.reply.send(Err(rpc_failure(
                "cancelled",
                "The MCP request was cancelled.",
            )));
        }
        self.poll_terminal_executions(cx);
        cx.notify();
    }

    fn handle_mcp_disconnect(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        self.mcp
            .capabilities
            .terminal_executions
            .forget_owner(connection_id);
        self.mcp.capabilities.outputs.remove(connection_id);
        for request in self.mcp.capabilities.connection_disconnected(connection_id) {
            let assessment =
                nyaterm_core::capabilities::assess_tool_risk(&request.tool, &request.arguments);
            self.record_mcp_audit(
                &request,
                assessment.as_ref(),
                None,
                Some(false),
                true,
                None,
                Some("client disconnected"),
                cx,
            );
            let _ = request.reply.send(Err(rpc_failure(
                "cancelled",
                "The MCP client disconnected.",
            )));
        }
        self.poll_terminal_executions(cx);
        cx.notify();
    }

    pub(in crate::features) fn complete_mcp_session_open_success(
        &mut self,
        connection_id: &str,
        session_id: String,
    ) {
        self.mcp
            .capabilities
            .complete_session_open_success(connection_id, session_id);
    }

    pub(in crate::features) fn complete_mcp_session_open_failure(
        &mut self,
        connection_id: &str,
        error: &str,
    ) {
        self.mcp
            .capabilities
            .complete_session_open_failure(connection_id, error);
    }

    pub(in crate::features) fn create_ephemeral_mcp_credential(
        &self,
        session_ids: Vec<String>,
        default_session_id: Option<String>,
        permission_mode: nyaterm_core::AiPermissionMode,
        policy: TerminalExecutionPolicy,
    ) -> Result<McpEphemeralCredential, String> {
        self.mcp
            .create_ephemeral(session_ids, default_session_id, permission_mode, policy)
    }

    pub(in crate::features) fn mcp_host_status(&self) -> McpHostStatus {
        self.mcp
            .status(self.ai.settings_config().external_mcp.enabled)
    }

    pub(in crate::features) fn reconfigure_mcp_host(&mut self) -> Result<(), String> {
        self.mcp
            .reconfigure(&self.ai.settings_config().external_mcp)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nyaterm_core::{
        AiPermissionMode, CapabilityAccess, CapabilityScope, ConnectionType, Group, PolicyDecision,
        decide_policy,
    };
    use nyaterm_mcp_protocol::{RpcError, tool};
    use serde_json::Value;

    use super::server::{McpHostRequest, rpc_failure};
    use super::{McpApprovalDecision, McpApprovalOutcome, McpApprovalRequest, McpHostFeatureState};
    use crate::features::capability_runtime::PendingCapabilityApproval as PendingMcpApproval;
    use crate::features::capability_runtime::{
        connection_group_path, connection_type_name, mcp_grant_key,
    };

    #[test]
    fn integrated_generations_keep_policies_until_their_own_credential_expires() {
        use crate::features::capability_runtime::terminal_execution::TerminalExecutionPolicy;
        let state = McpHostFeatureState::disabled_for_test();
        let foreground = TerminalExecutionPolicy {
            background: false,
            max_lines: 5,
        };
        let background = TerminalExecutionPolicy {
            background: true,
            max_lines: 0,
        };
        let codex = state
            .create_ephemeral(
                vec!["a".into()],
                Some("a".into()),
                AiPermissionMode::FullAccess,
                foreground,
            )
            .unwrap();
        let claude = state
            .create_ephemeral(
                vec!["b".into()],
                Some("b".into()),
                AiPermissionMode::FullAccess,
                background,
            )
            .unwrap();
        let codex_generation = codex
            .environment()
            .remove("NYATERM_MCP_GENERATION")
            .unwrap();
        let claude_generation = claude
            .environment()
            .remove("NYATERM_MCP_GENERATION")
            .unwrap();
        assert_eq!(state.terminal_policy(&codex_generation), Some(foreground));
        assert_eq!(state.terminal_policy(&claude_generation), Some(background));
        assert_eq!(state.terminal_policy("external"), None);
        drop(codex);
        assert!(!state.generation_matches(&codex_generation));
        assert_eq!(state.terminal_policy(&claude_generation), Some(background));
    }

    #[test]
    fn connection_group_paths_are_cycle_safe_and_graphical_connections_are_excluded() {
        let root = Group {
            id: "root".into(),
            name: "Production".into(),
            parent_id: None,
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        };
        let child = Group {
            id: "child".into(),
            name: "Linux".into(),
            parent_id: Some("root".into()),
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        };
        let groups = HashMap::from([("root", &root), ("child", &child)]);
        assert_eq!(
            connection_group_path(Some("child"), &groups),
            ["Production", "Linux"]
        );
        assert!(
            connection_type_name(&ConnectionType::Rdp {
                host: "example.invalid".into(),
                port: 3389,
                username: String::new(),
                domain: String::new(),
                backend: Default::default(),
                security: Default::default(),
                display: Default::default(),
                clipboard: Default::default(),
                reconnect: Default::default(),
            })
            .is_none()
        );
    }

    #[test]
    fn observer_matrix_requires_approval_for_sensitive_reads() {
        assert_eq!(
            decide_policy(
                &AiPermissionMode::Observer,
                CapabilityAccess::SensitiveRead,
                None
            ),
            PolicyDecision::RequireApproval
        );
        let state = McpHostFeatureState::disabled_for_test();
        assert!(state.runtime.is_none());
    }

    fn approval_request(
        request_id: &str,
        connection_id: &str,
    ) -> (
        McpHostRequest,
        tokio::sync::oneshot::Receiver<Result<Value, RpcError>>,
    ) {
        let (reply, receiver) = tokio::sync::oneshot::channel();
        (
            McpHostRequest {
                caller: crate::features::capability_runtime::CapabilityCaller::Mcp,
                connection_id: connection_id.to_string(),
                request_id: request_id.to_string(),
                generation: "test".to_string(),
                client: "fixture-client".to_string(),
                permission_mode: AiPermissionMode::Confirm,
                scope: CapabilityScope::AllSessions,
                tool: tool::SFTP_WRITE_TEXT.to_string(),
                arguments: serde_json::json!({
                    "sessionId": "session-1",
                    "path": "/tmp/test",
                    "content": "fixture"
                }),
                cancellation: tokio_util::sync::CancellationToken::new(),
                approved: false,
                approval_decision: None,
                reply,
            },
            receiver,
        )
    }

    #[test]
    fn approval_state_supports_session_grants_without_granting_destructive_tools() {
        let mut state = McpHostFeatureState::disabled_for_test();
        let (request, _receiver) = approval_request("request-1", "connection-1");
        let grant_key = mcp_grant_key(&request).expect("grant key");
        state.capabilities.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-1".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_WRITE_TEXT.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "fixture summary".to_string(),
                risk_level: "medium".to_string(),
                destructive: false,
                allow_session: true,
            },
            grant_key: Some(grant_key.clone()),
        });
        assert_eq!(state.capabilities.pending_approval_requests().len(), 1);
        let McpApprovalOutcome::Dispatch(approved) = state
            .capabilities
            .decide_approval("request-1", McpApprovalDecision::AllowSession)
            .expect("approved request")
        else {
            panic!("approval should dispatch");
        };
        assert!(approved.approved);
        assert!(state.capabilities.has_session_grant(&grant_key));

        let (request, _receiver) = approval_request("request-2", "connection-1");
        let destructive_key = (
            "connection-1".to_string(),
            "session-1".to_string(),
            tool::SFTP_DELETE.to_string(),
        );
        state.capabilities.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-2".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_DELETE.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "/tmp/test".to_string(),
                risk_level: "high".to_string(),
                destructive: true,
                allow_session: false,
            },
            grant_key: Some(destructive_key.clone()),
        });
        let McpApprovalOutcome::Dispatch(approved) = state
            .capabilities
            .decide_approval("request-2", McpApprovalDecision::AllowSession)
            .expect("one-time destructive approval")
        else {
            panic!("approval should dispatch");
        };
        assert!(approved.approved);
        assert!(!state.capabilities.has_session_grant(&destructive_key));
    }

    #[tokio::test]
    async fn approval_denial_and_disconnect_close_pending_requests() {
        let mut state = McpHostFeatureState::disabled_for_test();
        let (request, denied) = approval_request("request-denied", "connection-1");
        state.capabilities.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-denied".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_WRITE_TEXT.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "fixture".to_string(),
                risk_level: "medium".to_string(),
                destructive: false,
                allow_session: true,
            },
            grant_key: None,
        });
        let McpApprovalOutcome::Denied(request) = state
            .capabilities
            .decide_approval("request-denied", McpApprovalDecision::Deny)
            .expect("denied request")
        else {
            panic!("denial should return a terminal audit request");
        };
        let _ = request.reply.send(Err(rpc_failure(
            "approval_denied",
            "The MCP capability request was denied.",
        )));
        assert_eq!(denied.await.unwrap().unwrap_err().code, "approval_denied");

        let (request, disconnected) = approval_request("request-disconnect", "connection-2");
        state.capabilities.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-disconnect".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_WRITE_TEXT.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "fixture".to_string(),
                risk_level: "medium".to_string(),
                destructive: false,
                allow_session: true,
            },
            grant_key: None,
        });
        for request in state.capabilities.connection_disconnected("connection-2") {
            let _ = request.reply.send(Err(rpc_failure(
                "cancelled",
                "The MCP client disconnected.",
            )));
        }
        assert_eq!(disconnected.await.unwrap().unwrap_err().code, "cancelled");
        assert!(state.capabilities.pending_approval_requests().is_empty());
    }
}
