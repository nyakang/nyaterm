//! Shared capability authorization, execution, bounded output and audit adapters.

pub(in crate::features) mod approvals;
pub(in crate::features) mod terminal_execution;

use super::NyaTermApp;
use super::formatting::recent_terminal_output;
use super::mcp::server::rpc_failure;
use super::runtime_jobs::await_blocking_result;
use gpui::Context;
use nyaterm_core::ai::{AppendAiAuditRequest, RiskLevel, sanitize_ai_diagnostic};
use nyaterm_core::capabilities::{
    AgentApprovalDecision, CapabilityScope, CapabilityScopeSnapshot, CapabilitySession,
    PolicyDecision, RiskAssessment, decide_native_policy, decide_policy, requires_single_approval,
};
use nyaterm_core::models::connection::{AiExecutionProfile, ConnectionType};
use nyaterm_core::models::sessions::Group;

use nyaterm_mcp_protocol::{
    ConnectionListResult, ConnectionSummary, EnvironmentResult, MAX_INLINE_OUTPUT_BYTES,
    MutationResult, OutputReadArgs, PathArgs, RpcError, SessionArgs, SessionGetResult,
    SessionOpenArgs, SessionOpenResult, SessionSummary, SftpChmodArgs, SftpFileEntry,
    SftpHomeResult, SftpMkdirArgs, SftpReadTextArgs, SftpReadTextResult, SftpRenameArgs,
    SftpStatResult, SftpWriteTextArgs, SftpWriteTextResult, TerminalRecentOutputArgs,
    TerminalRecentOutputResult, definition_for_tool, tool, validate_tool_arguments,
    validate_tool_result,
};
use nyaterm_store::StoreDomain;
use nyaterm_transport::{SessionKind, SftpFileEntry as TransportFileEntry};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

struct McpAuditContext {
    source: String,
    connection_id: Option<String>,
    client: String,
    capability: String,
    session_id: Option<String>,
    permission_mode: nyaterm_core::AiPermissionMode,
    risk_level: Option<RiskLevel>,
    approval_decision: Option<String>,
    started_at: Instant,
    inserted_to_terminal: bool,
}

#[derive(Clone)]
pub(in crate::features) enum CapabilityCaller {
    Mcp,
    Native {
        run_id: String,
        ai_session_id: String,
        model_risk: Option<RiskLevel>,
    },
}

pub(in crate::features) struct CapabilityRequest {
    pub connection_id: String,
    pub request_id: String,
    pub generation: String,
    pub client: String,
    pub permission_mode: nyaterm_core::AiPermissionMode,
    pub scope: CapabilityScope,
    pub tool: String,
    pub arguments: Value,
    pub cancellation: tokio_util::sync::CancellationToken,
    pub approved: bool,
    pub approval_decision: Option<String>,
    pub caller: CapabilityCaller,
    pub reply: tokio::sync::oneshot::Sender<Result<Value, RpcError>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct CapabilityApprovalRequest {
    pub request_id: String,
    pub client: String,
    pub capability: String,
    pub target: Option<String>,
    pub parameter_summary: String,
    pub risk_level: String,
    pub destructive: bool,
    pub allow_session: bool,
}

pub(in crate::features) struct PendingCapabilitySessionOpen {
    pub(in crate::features) host_request_id: String,
    pub(in crate::features) client_connection_id: String,
    pub(in crate::features) cancellation: tokio_util::sync::CancellationToken,
    pub(in crate::features) reply: tokio::sync::oneshot::Sender<Result<Value, RpcError>>,
    pub(in crate::features) connection_id: String,
    pub(in crate::features) connection_name: String,
    pub(in crate::features) connection_type: String,
}

pub(in crate::features) struct PendingCapabilityApproval {
    pub(in crate::features) request: CapabilityRequest,
    pub(in crate::features) approval: CapabilityApprovalRequest,
    pub(in crate::features) grant_key: Option<(String, String, String)>,
}

pub(in crate::features) enum CapabilityApprovalOutcome {
    Dispatch(CapabilityRequest),
    Denied(CapabilityRequest),
}

#[derive(Default)]
pub(in crate::features) struct CapabilityRuntimeState {
    pub(in crate::features) pending_approvals: HashMap<String, PendingCapabilityApproval>,
    pub(in crate::features) approval_order: VecDeque<String>,
    pub(in crate::features) session_grants: HashSet<(String, String, String)>,
    pub(in crate::features) terminal_executions: terminal_execution::TerminalExecutionState,
    pending_session_opens: HashMap<String, PendingCapabilitySessionOpen>,
    pub(in crate::features) outputs: HashMap<String, nyaterm_core::OutputStore>,
    active_requests: HashMap<String, (String, tokio_util::sync::CancellationToken)>,
    owners: HashMap<String, (CapabilityCaller, String)>,
}

impl CapabilityRuntimeState {
    fn read_output(&mut self, owner: &str, args: OutputReadArgs) -> Result<Value, RpcError> {
        self.outputs
            .get_mut(owner)
            .ok_or_else(|| rpc_failure("output_not_found", "Output is unavailable or expired."))
            .and_then(|store| {
                store
                    .read(
                        &args.output_id,
                        args.offset,
                        args.max_bytes.unwrap_or(MAX_INLINE_OUTPUT_BYTES),
                    )
                    .map_err(|_| {
                        rpc_failure(
                            "output_not_found",
                            "Output is unavailable or offset is invalid.",
                        )
                    })
            })
            .and_then(to_value)
    }
    pub(in crate::features) fn protect_output(
        &mut self,
        owner: &str,
        tool_name: &str,
        value: Value,
    ) -> Result<Value, RpcError> {
        validate_tool_result(tool_name, &value)
            .map_err(|_| rpc_failure("internal_error", "Invalid capability result."))?;
        if tool_name == tool::OUTPUT_READ {
            return Ok(value);
        }
        let text = serde_json::to_string(&value)
            .map_err(|_| rpc_failure("internal_error", "Cannot serialize capability result."))?;
        if text.len() <= MAX_INLINE_OUTPUT_BYTES {
            return Ok(value);
        }
        let store = self.outputs.entry(owner.to_string()).or_default();
        to_value(store.protect(text, MAX_INLINE_OUTPUT_BYTES))
    }

    pub(in crate::features) fn revoke_generation(&mut self, generation: &str) {
        let owners = self
            .owners
            .iter()
            .filter(|(_, (_, owner_generation))| owner_generation == generation)
            .map(|(owner, _)| owner.clone())
            .collect::<Vec<_>>();
        for owner in owners {
            for request in self.connection_disconnected(&owner) {
                let _ = request.reply.send(Err(rpc_failure(
                    "cancelled",
                    "Capability credential expired.",
                )));
            }
        }
    }

    pub(in crate::features) fn prune_cancelled(&mut self) {
        self.active_requests
            .retain(|_, (_, cancellation)| !cancellation.is_cancelled());
        self.outputs
            .values_mut()
            .for_each(nyaterm_core::OutputStore::cleanup);
        self.pending_approvals
            .retain(|_, pending| !pending.request.cancellation.is_cancelled());
        self.approval_order
            .retain(|request_id| self.pending_approvals.contains_key(request_id));
        self.pending_session_opens
            .retain(|_, pending| !pending.cancellation.is_cancelled());
    }

    #[cfg(test)]
    pub(in crate::features) fn pending_approval_requests(
        &mut self,
    ) -> Vec<CapabilityApprovalRequest> {
        self.prune_cancelled();
        self.approval_order
            .iter()
            .filter_map(|request_id| self.pending_approvals.get(request_id))
            .map(|pending| pending.approval.clone())
            .collect()
    }

    pub(in crate::features) fn has_session_grant(&self, key: &(String, String, String)) -> bool {
        self.session_grants.contains(key)
    }

    pub(in crate::features) fn queue_approval(&mut self, pending: PendingCapabilityApproval) {
        let request_id = pending.approval.request_id.clone();
        if self
            .pending_approvals
            .insert(request_id.clone(), pending)
            .is_none()
        {
            self.approval_order.push_back(request_id);
        }
    }

    pub(in crate::features) fn decide_approval(
        &mut self,
        request_id: &str,
        decision: AgentApprovalDecision,
    ) -> Option<CapabilityApprovalOutcome> {
        self.prune_cancelled();
        let mut pending = self.pending_approvals.remove(request_id)?;
        self.approval_order.retain(|id| id != request_id);
        if pending.request.cancellation.is_cancelled() {
            return None;
        }
        match decision {
            AgentApprovalDecision::Deny => {
                self.active_requests.remove(request_id);
                pending.request.approval_decision = Some("deny".to_string());
                Some(CapabilityApprovalOutcome::Denied(pending.request))
            }
            AgentApprovalDecision::AllowOnce => {
                pending.request.approved = true;
                pending.request.approval_decision = Some("allow_once".to_string());
                Some(CapabilityApprovalOutcome::Dispatch(pending.request))
            }
            AgentApprovalDecision::AllowSession => {
                if pending.approval.allow_session
                    && let Some(key) = pending.grant_key
                {
                    self.session_grants.insert(key);
                }
                pending.request.approved = true;
                pending.request.approval_decision = Some(
                    if pending.approval.allow_session {
                        "allow_session"
                    } else {
                        "allow_once"
                    }
                    .to_string(),
                );
                Some(CapabilityApprovalOutcome::Dispatch(pending.request))
            }
        }
    }

    pub(in crate::features) fn request_cancelled(
        &mut self,
        request_id: &str,
    ) -> Vec<CapabilityRequest> {
        let mut cancelled = Vec::new();
        if let Some((_, cancellation)) = self.active_requests.remove(request_id) {
            cancellation.cancel();
        }
        self.approval_order.retain(|id| id != request_id);
        if let Some(pending) = self.pending_approvals.remove(request_id) {
            pending.request.cancellation.cancel();
            cancelled.push(pending.request);
        }
        let open_id = self
            .pending_session_opens
            .iter()
            .find_map(|(connection_id, pending)| {
                (pending.host_request_id == request_id).then(|| connection_id.clone())
            });
        if let Some(open_id) = open_id
            && let Some(pending) = self.pending_session_opens.remove(&open_id)
        {
            pending.cancellation.cancel();
            let _ = pending.reply.send(Err(rpc_failure(
                "cancelled",
                "The MCP request was cancelled.",
            )));
        }
        cancelled
    }

    pub(in crate::features) fn connection_disconnected(
        &mut self,
        client_connection_id: &str,
    ) -> Vec<CapabilityRequest> {
        let mut disconnected = Vec::new();
        self.outputs.remove(client_connection_id);
        self.owners.remove(client_connection_id);
        self.active_requests.retain(|_, (owner, cancellation)| {
            if owner == client_connection_id {
                cancellation.cancel();
                false
            } else {
                true
            }
        });
        let request_ids = self
            .pending_approvals
            .iter()
            .filter(|(_, pending)| pending.request.connection_id == client_connection_id)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for request_id in request_ids {
            self.approval_order.retain(|id| id != &request_id);
            if let Some(pending) = self.pending_approvals.remove(&request_id) {
                pending.request.cancellation.cancel();
                disconnected.push(pending.request);
            }
        }
        let open_ids = self
            .pending_session_opens
            .iter()
            .filter(|(_, pending)| pending.client_connection_id == client_connection_id)
            .map(|(connection_id, _)| connection_id.clone())
            .collect::<Vec<_>>();
        for open_id in open_ids {
            if let Some(pending) = self.pending_session_opens.remove(&open_id) {
                pending.cancellation.cancel();
                let _ = pending.reply.send(Err(rpc_failure(
                    "cancelled",
                    "The MCP client disconnected.",
                )));
            }
        }
        self.session_grants
            .retain(|(connection_id, _, _)| connection_id != client_connection_id);
        disconnected
    }

    pub(in crate::features) fn session_open_is_pending(&mut self, connection_id: &str) -> bool {
        self.prune_cancelled();
        self.pending_session_opens.contains_key(connection_id)
    }

    pub(in crate::features) fn register_session_open(
        &mut self,
        pending: PendingCapabilitySessionOpen,
    ) {
        debug_assert!(
            !self
                .pending_session_opens
                .contains_key(&pending.connection_id)
        );
        self.pending_session_opens
            .insert(pending.connection_id.clone(), pending);
    }

    pub(in crate::features) fn complete_session_open_success(
        &mut self,
        connection_id: &str,
        session_id: String,
    ) {
        let Some(pending) = self.pending_session_opens.remove(connection_id) else {
            return;
        };
        if pending.cancellation.is_cancelled() {
            return;
        }
        let result = to_value(SessionOpenResult {
            session_id,
            connection_id: pending.connection_id,
            name: pending.connection_name,
            r#type: pending.connection_type,
            connected: true,
        });
        let _ = pending.reply.send(result);
    }

    pub(in crate::features) fn complete_session_open_failure(
        &mut self,
        connection_id: &str,
        error: &str,
    ) {
        let Some(pending) = self.pending_session_opens.remove(connection_id) else {
            return;
        };
        if pending.cancellation.is_cancelled() {
            return;
        }
        let _ = pending
            .reply
            .send(Err(rpc_failure("execution_failed", error)));
    }
}

impl NyaTermApp {
    pub(in crate::features) fn record_native_control_audit(
        &mut self,
        capability: &str,
        success: bool,
        cx: &mut Context<Self>,
    ) {
        let permission_mode = if self.ai.settings_config().agent_command_execution_mode
            == nyaterm_core::AgentCommandExecutionMode::ConfirmEach
        {
            nyaterm_core::AiPermissionMode::Confirm
        } else {
            nyaterm_core::AiPermissionMode::Auto
        };
        self.persist_mcp_audit(
            AppendAiAuditRequest {
                connection_id: None,
                action: "agent.capability".into(),
                user_input: None,
                generated_command: None,
                risk_level: None,
                inserted_to_terminal: false,
                executed: success,
                blocked: !success,
                source: Some("native".into()),
                client: Some("NyaTerm Native Agent".into()),
                capability: Some(capability.into()),
                session_id: None,
                permission_mode: Some(permission_mode),
                approval_decision: None,
                success: Some(success),
                duration_ms: self.ai.native_call_elapsed_ms(),
                error: (!success).then(|| "capability_failed".into()),
            },
            cx,
        );
    }

    fn reject_capability(
        &mut self,
        request: CapabilityRequest,
        code: &str,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        self.record_mcp_audit(
            &request,
            None,
            Some("denied_before_execution"),
            Some(false),
            true,
            Some(0),
            Some(code),
            cx,
        );
        self.mcp
            .capabilities
            .active_requests
            .remove(&request.request_id);
        let _ = request.reply.send(Err(rpc_failure(code, message)));
    }

    pub(in crate::features) fn clear_capability_owner(
        &mut self,
        owner: &str,
        cx: &mut Context<Self>,
    ) {
        self.mcp
            .capabilities
            .terminal_executions
            .forget_owner(owner);
        self.mcp.capabilities.outputs.remove(owner);
        for request in self.mcp.capabilities.connection_disconnected(owner) {
            request.cancellation.cancel();
            let _ = request
                .reply
                .send(Err(rpc_failure("cancelled", "Capability caller ended.")));
        }
        self.poll_terminal_executions(cx);
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    fn persist_mcp_audit(&mut self, audit: AppendAiAuditRequest, cx: &mut Context<Self>) {
        let store = self.store_blocking_client();
        let write_lock = self.ai.history_audit_write_lock();
        let task = self.blocking_jobs.submit_task("mcp-audit-save", move |_| {
            let _guard = write_lock
                .lock()
                .map_err(|_| "AI audit write lock poisoned".to_string())?;
            store
                .request_fn(StoreDomain::Ai, move |database| {
                    database.append_ai_audit(audit)
                })
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |_this, _cx| {
            if let Err(error) = await_blocking_result(task).await {
                tracing::warn!(error = %error, "failed to persist MCP audit event");
            }
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::features) fn record_mcp_audit(
        &mut self,
        request: &CapabilityRequest,
        assessment: Option<&RiskAssessment>,
        approval_decision: Option<&str>,
        success: Option<bool>,
        blocked: bool,
        duration_ms: Option<u64>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let audit = AppendAiAuditRequest {
            connection_id: mcp_request_target(request),
            action: "mcp.capability".to_string(),
            user_input: None,
            generated_command: None,
            risk_level: assessment.map(|risk| risk.level.clone()),
            inserted_to_terminal: false,
            executed: success == Some(true),
            blocked,
            source: Some(
                match request.caller {
                    CapabilityCaller::Mcp => "mcp",
                    CapabilityCaller::Native { .. } => "native",
                }
                .to_string(),
            ),
            client: Some(sanitize_ai_diagnostic(&request.client, 128)),
            capability: Some(
                definition_for_tool(&request.tool)
                    .map_or("unknown", |definition| definition.tool)
                    .to_string(),
            ),
            session_id: request
                .arguments
                .get("sessionId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            permission_mode: Some(request.permission_mode.clone()),
            approval_decision: approval_decision.map(ToOwned::to_owned),
            success,
            duration_ms,
            error: error.map(|_| "capability_failed".to_string()),
        };
        self.persist_mcp_audit(audit, cx);
    }

    fn record_mcp_audit_result(
        &mut self,
        audit_context: McpAuditContext,
        success: bool,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let duration_ms =
            u64::try_from(audit_context.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let audit = AppendAiAuditRequest {
            connection_id: audit_context.connection_id,
            action: "mcp.capability".to_string(),
            user_input: None,
            generated_command: None,
            risk_level: audit_context.risk_level,
            inserted_to_terminal: audit_context.inserted_to_terminal,
            executed: success,
            blocked: !success,
            source: Some(audit_context.source),
            client: Some(audit_context.client),
            capability: Some(audit_context.capability),
            session_id: audit_context.session_id,
            permission_mode: Some(audit_context.permission_mode),
            approval_decision: audit_context.approval_decision,
            success: Some(success),
            duration_ms: Some(duration_ms),
            error: error.map(|_| "capability_failed".to_string()),
        };
        self.persist_mcp_audit(audit, cx);
    }
    pub(in crate::features) fn dispatch_capability_request(
        &mut self,
        mut request: CapabilityRequest,
        cx: &mut Context<Self>,
    ) {
        if matches!(request.caller, CapabilityCaller::Native { .. }) {
            request.permission_mode = if self.ai.settings_config().agent_command_execution_mode
                == nyaterm_core::AgentCommandExecutionMode::ConfirmEach
            {
                nyaterm_core::AiPermissionMode::Confirm
            } else {
                nyaterm_core::AiPermissionMode::Auto
            };
        }
        let _client_identity = request.client.as_str();
        if matches!(request.caller, CapabilityCaller::Mcp)
            && !self.mcp.generation_matches(&request.generation)
        {
            self.reject_capability(
                request,
                "authentication_failed",
                "MCP credential was rotated or expired.",
                cx,
            );
            return;
        }
        if request.cancellation.is_cancelled() {
            self.reject_capability(request, "cancelled", "The MCP request was cancelled.", cx);
            return;
        }
        let Some(definition) = definition_for_tool(&request.tool) else {
            self.reject_capability(request, "invalid_argument", "Unknown NyaTerm MCP tool.", cx);
            return;
        };
        if validate_tool_arguments(&request.tool, &request.arguments).is_err() {
            self.reject_capability(
                request,
                "invalid_argument",
                "Invalid capability arguments.",
                cx,
            );
            return;
        }
        if let CapabilityCaller::Native {
            run_id,
            ai_session_id,
            ..
        } = &request.caller
            && !self.ai.native_run_is_live(run_id, ai_session_id)
        {
            self.reject_capability(request, "cancelled", "Native run is no longer active.", cx);
            return;
        }
        let scope = request.scope.resolve(&self.mcp_live_sessions());
        if definition.requires_session {
            let requested = request.arguments.get("sessionId").and_then(Value::as_str);
            let session_id = match scope.resolve_session(requested) {
                Ok(id) => id,
                Err(_) => {
                    self.reject_capability(
                        request,
                        "scope_denied",
                        "Target is unavailable or outside this capability scope.",
                        cx,
                    );
                    return;
                }
            };
            request.arguments["sessionId"] = Value::String(session_id.clone());
            if request.tool == tool::TERMINAL_EXECUTE
                && self.session.metadata(&session_id).is_some_and(|metadata| {
                    metadata.ai_execution_profile == AiExecutionProfile::Disabled
                })
            {
                self.reject_capability(
                    request,
                    "permission_denied",
                    "AI execution is disabled for this session.",
                    cx,
                );
                return;
            }
        }
        let mut assessment =
            nyaterm_core::capabilities::assess_tool_risk(&request.tool, &request.arguments);
        let policy = match &request.caller {
            CapabilityCaller::Mcp => decide_policy(
                &request.permission_mode,
                definition.access,
                assessment.as_ref(),
            ),
            CapabilityCaller::Native { model_risk, .. } => {
                if let (Some(risk), Some(model)) = (assessment.as_mut(), model_risk) {
                    risk.level = risk.level.clone().max(model.clone());
                }
                let settings = self.ai.settings_config();
                decide_native_policy(
                    &settings.agent_command_execution_mode,
                    &settings.agent_smart_auto_execute_max_risk,
                    definition.access,
                    assessment.as_ref(),
                )
            }
        };
        let single_approval = requires_single_approval(definition.access, assessment.as_ref());
        let confirm_each = matches!(request.caller, CapabilityCaller::Native { .. })
            && request.tool == tool::TERMINAL_EXECUTE
            && self.ai.settings_config().agent_command_execution_mode
                == nyaterm_core::AgentCommandExecutionMode::ConfirmEach;
        self.mcp.capabilities.owners.insert(
            request.connection_id.clone(),
            (request.caller.clone(), request.generation.clone()),
        );
        self.mcp.capabilities.active_requests.insert(
            request.request_id.clone(),
            (request.connection_id.clone(), request.cancellation.clone()),
        );
        if request.approval_decision.as_deref() == Some("allow_session")
            && (single_approval || confirm_each)
        {
            request.approved = false;
        }
        match policy {
            PolicyDecision::Deny => {
                self.record_mcp_audit(
                    &request,
                    assessment.as_ref(),
                    Some("denied_by_policy"),
                    Some(false),
                    true,
                    None,
                    Some("permission denied"),
                    cx,
                );
                self.mcp
                    .capabilities
                    .active_requests
                    .remove(&request.request_id);
                let _ = request.reply.send(Err(rpc_failure(
                    "permission_denied",
                    "The current MCP permission mode denies this capability.",
                )));
                return;
            }
            PolicyDecision::RequireApproval => {
                let grant_key = mcp_grant_key(&request);
                let session_granted = !single_approval
                    && !confirm_each
                    && grant_key
                        .as_ref()
                        .is_some_and(|key| self.mcp.capabilities.has_session_grant(key));
                if !request.approved && !session_granted {
                    let approval = CapabilityApprovalRequest {
                        request_id: request.request_id.clone(),
                        client: request.client.clone(),
                        capability: request.tool.clone(),
                        target: mcp_request_target(&request),
                        parameter_summary: mcp_parameter_summary(&request.tool, &request.arguments),
                        risk_level: assessment
                            .as_ref()
                            .map(|risk| format!("{:?}", risk.level).to_ascii_lowercase())
                            .unwrap_or_else(|| "sensitive".to_string()),
                        allow_session: !single_approval && !confirm_each,
                        destructive: matches!(
                            definition.access,
                            nyaterm_mcp_protocol::CapabilityAccess::DestructiveWrite
                        ),
                    };
                    if let CapabilityCaller::Native { run_id, .. } = &request.caller {
                        self.ai.set_native_run_status(
                            run_id,
                            nyaterm_core::ai::harness::AgentRunStatus::NeedsApproval,
                        );
                    }
                    self.mcp
                        .capabilities
                        .queue_approval(PendingCapabilityApproval {
                            request,
                            approval,
                            grant_key,
                        });
                    self.defer_ai_panel_snapshot_flush(cx);
                    cx.notify();
                    return;
                }
            }
            PolicyDecision::Allow => {}
        }

        if let CapabilityCaller::Native { run_id, .. } = &request.caller {
            self.ai
                .set_native_run_status(run_id, nyaterm_core::ai::harness::AgentRunStatus::Running);
        }

        let approval_decision = if request.approved {
            request.approval_decision.as_deref()
        } else if !single_approval
            && !confirm_each
            && mcp_grant_key(&request)
                .as_ref()
                .is_some_and(|key| self.mcp.capabilities.has_session_grant(key))
        {
            Some("session_grant")
        } else {
            None
        };
        let audit_context = McpAuditContext {
            source: match request.caller {
                CapabilityCaller::Mcp => "mcp",
                CapabilityCaller::Native { .. } => "native",
            }
            .into(),
            connection_id: mcp_request_target(&request),
            client: sanitize_ai_diagnostic(&request.client, 128),
            capability: request.tool.clone(),
            session_id: request
                .arguments
                .get("sessionId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            permission_mode: request.permission_mode.clone(),
            risk_level: assessment.as_ref().map(|risk| risk.level.clone()),
            approval_decision: approval_decision.map(ToOwned::to_owned),
            started_at: Instant::now(),
            inserted_to_terminal: false,
        };
        let (audit_reply, audit_result) = tokio::sync::oneshot::channel();
        let original_reply = std::mem::replace(&mut request.reply, audit_reply);
        let owner = request.connection_id.clone();
        let request_id = request.request_id.clone();
        let output_tool = request.tool.clone();
        let cancellation = request.cancellation.clone();
        let generation = request.generation.clone();
        let caller = request.caller.clone();
        cx.spawn(async move |this, cx| {
            let result = audit_result.await.unwrap_or_else(|_| {
                Err(rpc_failure(
                    "host_unavailable",
                    "Capability request was closed.",
                ))
            });
            let result = this
                .update(cx, |this, cx| {
                    this.mcp.capabilities.active_requests.remove(&request_id);
                    let valid_caller = match &caller {
                        CapabilityCaller::Mcp => this.mcp.generation_matches(&generation),
                        CapabilityCaller::Native {
                            run_id,
                            ai_session_id,
                            ..
                        } => this.ai.native_run_is_live(run_id, ai_session_id),
                    };
                    let successful = result.as_ref().is_ok_and(|value| {
                        output_tool != tool::TERMINAL_EXECUTE
                            || (value["exitCode"] == 0
                                && value["timedOut"] == false
                                && value["sourceTruncated"] == false)
                    });
                    let result = if cancellation.is_cancelled() || !valid_caller {
                        Err(rpc_failure(
                            "cancelled",
                            "Capability caller is no longer active.",
                        ))
                    } else {
                        result.and_then(|value| {
                            this.mcp
                                .capabilities
                                .protect_output(&owner, &output_tool, value)
                        })
                    };
                    let mut audit_context = audit_context;
                    audit_context.inserted_to_terminal = this
                        .mcp
                        .capabilities
                        .terminal_executions
                        .take_inserted(&request_id);
                    this.record_mcp_audit_result(
                        audit_context,
                        successful && result.is_ok(),
                        result.as_ref().err().map(|error| error.code.as_str()),
                        cx,
                    );
                    result
                })
                .unwrap_or_else(|_| {
                    Err(rpc_failure(
                        "host_unavailable",
                        "Capability host was closed.",
                    ))
                });
            let _ = original_reply.send(result);
        })
        .detach();

        match request.tool.as_str() {
            tool::OUTPUT_READ => {
                let args = serde_json::from_value::<OutputReadArgs>(request.arguments)
                    .expect("validated output arguments");
                let result = self
                    .mcp
                    .capabilities
                    .read_output(&request.connection_id, args);
                respond(request.reply, result);
            }
            tool::GET_ENVIRONMENT => {
                respond(request.reply, self.mcp_environment(&scope));
            }
            tool::CONNECTION_LIST => {
                respond(request.reply, self.mcp_connection_list());
            }
            tool::SESSION_GET => {
                let args = serde_json::from_value::<SessionArgs>(request.arguments)
                    .expect("sidecar validated session arguments");
                respond(
                    request.reply,
                    self.mcp_session_get(&scope, &args.session_id),
                );
            }
            tool::TERMINAL_RECENT_OUTPUT => {
                let args = serde_json::from_value::<TerminalRecentOutputArgs>(request.arguments)
                    .expect("sidecar validated terminal output arguments");
                respond(
                    request.reply,
                    self.mcp_recent_output(&scope, &args.session_id, args.lines.unwrap_or(100)),
                );
            }
            tool::SFTP_HOME | tool::SFTP_LIST | tool::SFTP_STAT | tool::SFTP_READ_TEXT => {
                self.dispatch_mcp_sftp_read(request, &scope, cx);
            }
            tool::SFTP_WRITE_TEXT
            | tool::SFTP_MKDIR
            | tool::SFTP_RENAME
            | tool::SFTP_DELETE
            | tool::SFTP_CHMOD => {
                self.dispatch_mcp_sftp_mutation(request, &scope, cx);
            }
            tool::TERMINAL_EXECUTE => {
                if matches!(request.caller, CapabilityCaller::Native { .. }) {
                    let CapabilityCaller::Native { ai_session_id, .. } = &request.caller else {
                        unreachable!()
                    };
                    let visible_scope = self.ai.active_scope_key().to_string();
                    if let Some(owner_scope) = self.ai.scope_for_ai_session(ai_session_id) {
                        self.ai.switch_scope(&owner_scope);
                        self.dispatch_native_terminal_execute(request, &scope, cx);
                        self.ai.switch_scope(&visible_scope);
                    } else {
                        respond(
                            request.reply,
                            Err(rpc_failure("cancelled", "Native conversation ended.")),
                        );
                    }
                } else {
                    self.dispatch_mcp_terminal_execute(request, &scope, cx);
                }
            }
            tool::SESSION_OPEN => {
                self.dispatch_mcp_session_open(request, cx);
            }
            _ => {
                let _ = request.reply.send(Err(rpc_failure(
                    "invalid_argument",
                    "The requested MCP capability is not implemented.",
                )));
            }
        }
    }

    pub(in crate::features) fn mcp_live_sessions(&self) -> Vec<CapabilitySession> {
        self.session
            .ordered_sessions()
            .into_iter()
            .filter(|session| !self.session.is_disconnected(&session.id))
            .map(|session| CapabilitySession {
                id: session.id,
                owner_window_label: Some(self.mcp.owner_window_label.clone()),
                live: true,
            })
            .collect()
    }

    fn mcp_environment(&self, scope: &CapabilityScopeSnapshot) -> Result<Value, RpcError> {
        let mut sessions = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| scope.session_ids.contains(&session.id))
            .map(|session| SessionSummary {
                id: session.id.clone(),
                name: self
                    .session
                    .display_name(&session.id)
                    .unwrap_or(session.name),
                r#type: session_kind_name(session.kind).to_string(),
                connected: !self.session.is_disconnected(&session.id),
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        let active_session_id = self
            .session
            .active_id()
            .filter(|session_id| scope.session_ids.contains(*session_id))
            .map(str::to_string);
        to_value(EnvironmentResult {
            active_session_id,
            default_session_id: scope.default_session_id.clone(),
            sessions,
        })
    }

    fn mcp_connection_list(&self) -> Result<Value, RpcError> {
        let groups = self
            .connection_state
            .groups()
            .iter()
            .map(|group| (group.id.as_str(), group))
            .collect::<HashMap<_, _>>();
        let mut connections = self
            .connection_state
            .connections()
            .iter()
            .filter_map(|connection| {
                connection_type_name(&connection.config).map(|kind| ConnectionSummary {
                    id: connection.id.clone(),
                    name: connection.name.clone(),
                    r#type: kind.to_string(),
                    group_path: connection_group_path(connection.group_id.as_deref(), &groups),
                })
            })
            .collect::<Vec<_>>();
        connections.sort_by(|left, right| {
            left.group_path
                .cmp(&right.group_path)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        to_value(ConnectionListResult { connections })
    }

    fn mcp_session_get(
        &self,
        scope: &CapabilityScopeSnapshot,
        session_id: &str,
    ) -> Result<Value, RpcError> {
        scope
            .require(session_id)
            .map_err(|error| rpc_failure("scope_denied", &error.to_string()))?;
        let session = self.session.session_info(session_id).ok_or_else(|| {
            rpc_failure("invalid_argument", "The requested session is unavailable.")
        })?;
        let metadata = self.session.metadata(session_id).ok_or_else(|| {
            rpc_failure("invalid_argument", "The requested session is unavailable.")
        })?;
        to_value(SessionGetResult {
            id: session.id.clone(),
            name: self
                .session
                .display_name(&session.id)
                .unwrap_or(session.name),
            r#type: session_kind_name(session.kind).to_string(),
            connected: !metadata.disconnected,
            cwd: self.session.cwd(session_id).map(str::to_string),
            terminal_execution: execution_profile_name(metadata.ai_execution_profile).to_string(),
            sftp_available: session.kind == SessionKind::Ssh
                && metadata.ssh_config.is_some()
                && !metadata.disconnected,
        })
    }

    fn mcp_recent_output(
        &self,
        scope: &CapabilityScopeSnapshot,
        session_id: &str,
        lines: usize,
    ) -> Result<Value, RpcError> {
        scope
            .require(session_id)
            .map_err(|error| rpc_failure("scope_denied", &error.to_string()))?;
        let output = recent_terminal_output(
            self.terminal_buffer_tail_for_session(session_id),
            lines.clamp(1, 500),
        );
        to_value(TerminalRecentOutputResult {
            session_id: session_id.to_string(),
            output,
        })
    }

    fn dispatch_mcp_sftp_read(
        &mut self,
        request: CapabilityRequest,
        scope: &CapabilityScopeSnapshot,
        cx: &mut Context<Self>,
    ) {
        let session_id = request
            .arguments
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Err(error) = scope.require(&session_id) {
            let _ = request
                .reply
                .send(Err(rpc_failure("scope_denied", &error.to_string())));
            return;
        }
        let config = self
            .session
            .metadata(&session_id)
            .and_then(|metadata| metadata.ssh_config.clone());
        let Some(config) = config else {
            let _ = request.reply.send(Err(rpc_failure(
                "permission_denied",
                "SFTP is unavailable for this session.",
            )));
            return;
        };
        let service = match self.remote_file_service_for_session(&session_id, config) {
            Ok(service) => service,
            Err(error) => {
                let _ = request
                    .reply
                    .send(Err(rpc_failure("execution_failed", &error.to_string())));
                return;
            }
        };
        let tool_name = request.tool.clone();
        let arguments = request.arguments;
        let cancellation = request.cancellation;
        let task = self.blocking_jobs.submit_task("mcp-sftp-read", move |_| {
            if cancellation.is_cancelled() {
                return Err("The MCP request was cancelled.".to_string());
            }
            let value: Result<Value, String> = match tool_name.as_str() {
                tool::SFTP_HOME => to_value_string(SftpHomeResult {
                    path: service.home_dir().map_err(|error| error.to_string())?,
                }),
                tool::SFTP_LIST => {
                    let args = serde_json::from_value::<PathArgs>(arguments)
                        .map_err(|error| error.to_string())?;
                    let entries = service
                        .list_dir(&args.path)
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .map(map_file_entry)
                        .collect::<Vec<_>>();
                    to_value_string(entries)
                }
                tool::SFTP_STAT => {
                    let args = serde_json::from_value::<PathArgs>(arguments)
                        .map_err(|error| error.to_string())?;
                    let value = service
                        .file_properties(&args.path)
                        .map_err(|error| error.to_string())?;
                    let is_dir = value.is_directory();
                    to_value_string(SftpStatResult {
                        name: value.name,
                        is_dir,
                        is_symlink: value.file_type == nyaterm_transport::SftpFileType::Symlink,
                        symlink_target: None,
                        size: value.size.unwrap_or(0),
                        permissions: permissions(value.permissions),
                        owner: value.owner,
                        group: value.group,
                        uid: value
                            .uid
                            .map_or_else(String::new, |value| value.to_string()),
                        gid: value
                            .gid
                            .map_or_else(String::new, |value| value.to_string()),
                        mtime: value.modified_at.map_or(0, u64::from),
                        atime: value.accessed_at.map_or(0, u64::from),
                    })
                }
                tool::SFTP_READ_TEXT => {
                    let args = serde_json::from_value::<SftpReadTextArgs>(arguments)
                        .map_err(|error| error.to_string())?;
                    let file = service
                        .read_text_file(
                            &args.path,
                            args.max_bytes
                                .unwrap_or(nyaterm_mcp_protocol::MAX_TEXT_READ_BYTES),
                        )
                        .map_err(|error| error.to_string())?;
                    let content_hash = hex::encode(Sha256::digest(file.content.as_bytes()));
                    to_value_string(SftpReadTextResult {
                        path: file.path,
                        content: file.content,
                        size: file.size,
                        mtime: file.modified_at,
                        mtime_nanos: None,
                        content_hash,
                    })
                }
                _ => Err("Unknown SFTP read tool.".to_string()),
            };
            if cancellation.is_cancelled() {
                Err("The MCP request was cancelled.".to_string())
            } else {
                value
            }
        });
        let reply = request.reply;
        cx.spawn(async move |_this, _cx| {
            let result = await_blocking_result(task)
                .await
                .map_err(|error| rpc_failure("execution_failed", &error));
            let _ = reply.send(result);
        })
        .detach();
    }
    fn dispatch_mcp_session_open(&mut self, request: CapabilityRequest, cx: &mut Context<Self>) {
        let args = serde_json::from_value::<SessionOpenArgs>(request.arguments)
            .expect("sidecar validated session open arguments");
        let Some(connection) = self
            .connection_state
            .connection_by_id(&args.connection_id)
            .cloned()
        else {
            let _ = request.reply.send(Err(rpc_failure(
                "invalid_argument",
                "The requested saved connection does not exist.",
            )));
            return;
        };
        let Some(connection_type) = connection_type_name(&connection.config) else {
            let _ = request.reply.send(Err(rpc_failure(
                "permission_denied",
                "MCP cannot open graphical remote desktop connections.",
            )));
            return;
        };
        if self
            .mcp
            .capabilities
            .session_open_is_pending(&connection.id)
        {
            let _ = request.reply.send(Err(rpc_failure(
                "execution_failed",
                "This saved connection is already being opened by MCP.",
            )));
            return;
        }
        self.mcp
            .capabilities
            .register_session_open(PendingCapabilitySessionOpen {
                host_request_id: request.request_id,
                client_connection_id: request.connection_id,
                cancellation: request.cancellation,
                reply: request.reply,
                connection_id: connection.id.clone(),
                connection_name: connection.name.clone(),
                connection_type: connection_type.to_string(),
            });
        self.continue_saved_connection_start(connection, Default::default(), cx);
    }

    fn dispatch_mcp_terminal_execute(
        &mut self,
        request: CapabilityRequest,
        scope: &CapabilityScopeSnapshot,
        cx: &mut Context<Self>,
    ) {
        let requested = request.arguments.get("sessionId").and_then(Value::as_str);
        let session_id = match scope.resolve_session(requested) {
            Ok(id) => id,
            Err(error) => {
                let _ = request
                    .reply
                    .send(Err(rpc_failure("scope_denied", &error.to_string())));
                return;
            }
        };
        self.dispatch_shared_terminal_execute(request, session_id, cx);
    }

    fn dispatch_mcp_sftp_mutation(
        &mut self,
        request: CapabilityRequest,
        scope: &CapabilityScopeSnapshot,
        cx: &mut Context<Self>,
    ) {
        let session_id = request
            .arguments
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Err(error) = scope.require(&session_id) {
            let _ = request
                .reply
                .send(Err(rpc_failure("scope_denied", &error.to_string())));
            return;
        }
        let config = self
            .session
            .metadata(&session_id)
            .and_then(|metadata| metadata.ssh_config.clone());
        let Some(config) = config else {
            let _ = request.reply.send(Err(rpc_failure(
                "permission_denied",
                "SFTP is unavailable for this session.",
            )));
            return;
        };
        let service = match self.remote_file_service_for_session(&session_id, config) {
            Ok(service) => service,
            Err(error) => {
                let _ = request
                    .reply
                    .send(Err(rpc_failure("execution_failed", &error.to_string())));
                return;
            }
        };
        let tool_name = request.tool.clone();
        let arguments = request.arguments;
        let cancellation = request.cancellation;
        let task = self
            .blocking_jobs
            .submit_task("mcp-sftp-mutation", move |_| {
                if cancellation.is_cancelled() {
                    return Err("The MCP request was cancelled.".to_string());
                }
                let value: Result<Value, String> = match tool_name.as_str() {
                    tool::SFTP_WRITE_TEXT => {
                        let args = serde_json::from_value::<SftpWriteTextArgs>(arguments)
                            .map_err(|error| error.to_string())?;
                        let force = args.force.unwrap_or(false);
                        let result = if let Some(expected_hash) = args.expected_hash.as_deref() {
                            let current = service
                                .read_text_file(
                                    &args.path,
                                    nyaterm_mcp_protocol::MAX_TEXT_WRITE_BYTES as u64,
                                )
                                .map_err(|error| error.to_string())?;
                            let actual_hash =
                                hex::encode(Sha256::digest(current.content.as_bytes()));
                            let metadata_matches =
                                args.expected_size.is_none_or(|size| size == current.size)
                                    && args
                                        .expected_mtime
                                        .is_none_or(|mtime| mtime == current.modified_at);
                            if !actual_hash.eq_ignore_ascii_case(expected_hash) || !metadata_matches
                            {
                                nyaterm_transport::RemoteTextWriteResult::Conflict
                            } else {
                                let revision = nyaterm_transport::RemoteTextRevision::from_bytes(
                                    current.content.as_bytes(),
                                    nyaterm_transport::RemoteTextMetadata {
                                        size: current.size,
                                        modified_at: Some(current.modified_at),
                                    },
                                );
                                service
                                    .write_text_document_path(
                                        &nyaterm_transport::RemoteFilePath::new(&args.path),
                                        &args.content,
                                        Some(&revision),
                                        force,
                                    )
                                    .map_err(|error| error.to_string())?
                            }
                        } else {
                            match service
                                .write_text_file(
                                    &args.path,
                                    &args.content,
                                    args.expected_mtime,
                                    args.expected_size,
                                    force,
                                )
                                .map_err(|error| error.to_string())?
                            {
                                nyaterm_transport::SftpWriteTextResult::Saved {
                                    modified_at,
                                    size,
                                } => nyaterm_transport::RemoteTextWriteResult::Saved {
                                    revision: nyaterm_transport::RemoteTextRevision::from_bytes(
                                        args.content.as_bytes(),
                                        nyaterm_transport::RemoteTextMetadata {
                                            size,
                                            modified_at: Some(modified_at),
                                        },
                                    ),
                                },
                                nyaterm_transport::SftpWriteTextResult::Conflict { .. } => {
                                    nyaterm_transport::RemoteTextWriteResult::Conflict
                                }
                            }
                        };
                        match result {
                            nyaterm_transport::RemoteTextWriteResult::Saved { revision } => {
                                to_value_string(SftpWriteTextResult {
                                    status: "saved".to_string(),
                                    mtime: revision.metadata.modified_at,
                                    size: Some(revision.metadata.size),
                                    mtime_nanos: None,
                                    content_hash: Some(hex::encode(revision.content_sha256)),
                                    backup_path: None,
                                })
                            }
                            nyaterm_transport::RemoteTextWriteResult::SavedWithBackup {
                                revision,
                                backup_path,
                            } => to_value_string(SftpWriteTextResult {
                                status: "saved_with_backup".to_string(),
                                mtime: revision.metadata.modified_at,
                                size: Some(revision.metadata.size),
                                mtime_nanos: None,
                                content_hash: Some(hex::encode(revision.content_sha256)),
                                backup_path: Some(backup_path),
                            }),
                            nyaterm_transport::RemoteTextWriteResult::Conflict => {
                                to_value_string(SftpWriteTextResult {
                                    status: "conflict".to_string(),
                                    mtime: None,
                                    size: None,
                                    mtime_nanos: None,
                                    content_hash: None,
                                    backup_path: None,
                                })
                            }
                        }
                    }
                    tool::SFTP_MKDIR => {
                        let args = serde_json::from_value::<SftpMkdirArgs>(arguments)
                            .map_err(|error| error.to_string())?;
                        service
                            .create_dir_path(&args.path, parse_remote_mode(args.mode.as_deref())?)
                            .map_err(|error| error.to_string())?;
                        to_value_string(MutationResult {
                            created: Some(true),
                            renamed: None,
                            deleted: None,
                            changed: None,
                        })
                    }
                    tool::SFTP_RENAME => {
                        let args = serde_json::from_value::<SftpRenameArgs>(arguments)
                            .map_err(|error| error.to_string())?;
                        service
                            .rename_path(&args.old_path, &args.new_path)
                            .map_err(|error| error.to_string())?;
                        to_value_string(MutationResult {
                            created: None,
                            renamed: Some(true),
                            deleted: None,
                            changed: None,
                        })
                    }
                    tool::SFTP_DELETE => {
                        let args = serde_json::from_value::<PathArgs>(arguments)
                            .map_err(|error| error.to_string())?;
                        service
                            .delete_path(&args.path)
                            .map_err(|error| error.to_string())?;
                        to_value_string(MutationResult {
                            created: None,
                            renamed: None,
                            deleted: Some(true),
                            changed: None,
                        })
                    }
                    tool::SFTP_CHMOD => {
                        let args = serde_json::from_value::<SftpChmodArgs>(arguments)
                            .map_err(|error| error.to_string())?;
                        service
                            .update_path_attributes(
                                &args.path,
                                nyaterm_transport::SftpAttributeUpdate {
                                    symlink_target: None,
                                    mode: Some(
                                        parse_remote_mode(Some(&args.mode))?
                                            .ok_or_else(|| "mode is required".to_string())?,
                                    ),
                                    owner: None,
                                    group: None,
                                    recursive: false,
                                },
                            )
                            .map_err(|error| error.to_string())?;
                        to_value_string(MutationResult {
                            created: None,
                            renamed: None,
                            deleted: None,
                            changed: Some(true),
                        })
                    }
                    _ => Err("Unknown SFTP mutation tool.".to_string()),
                };
                if cancellation.is_cancelled() {
                    Err("The MCP request was cancelled.".to_string())
                } else {
                    value
                }
            });
        let reply = request.reply;
        cx.spawn(async move |_this, _cx| {
            let result = await_blocking_result(task)
                .await
                .map_err(|error| rpc_failure("execution_failed", &error))
                .and_then(|value| {
                    if value.get("status").and_then(Value::as_str) == Some("conflict") {
                        Err(rpc_failure(
                            "conflict",
                            "The remote file changed before the MCP write completed.",
                        ))
                    } else {
                        Ok(value)
                    }
                });
            let _ = reply.send(result);
        })
        .detach();
    }
}

fn respond(
    reply: tokio::sync::oneshot::Sender<Result<Value, RpcError>>,
    result: Result<Value, RpcError>,
) {
    let _ = reply.send(result);
}

pub(in crate::features) fn to_value<T: serde::Serialize>(value: T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| rpc_failure("internal_error", &error.to_string()))
}

fn to_value_string<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn session_kind_name(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::LocalPty => "local",
        SessionKind::Ssh => "ssh",
        SessionKind::Telnet => "telnet",
        SessionKind::RawTcp => "raw_tcp",
        SessionKind::Serial => "serial",
        SessionKind::Rdp => "rdp",
        SessionKind::Vnc => "vnc",
    }
}

fn execution_profile_name(profile: AiExecutionProfile) -> &'static str {
    match profile {
        AiExecutionProfile::Disabled => "disabled",
        AiExecutionProfile::Auto | AiExecutionProfile::SendOnly => "send_only",
        AiExecutionProfile::Posix => "posix",
        AiExecutionProfile::Powershell => "powershell",
        AiExecutionProfile::Cmd => "cmd",
    }
}

pub(in crate::features) fn connection_type_name(
    connection: &ConnectionType,
) -> Option<&'static str> {
    match connection {
        ConnectionType::Ssh { .. } => Some("ssh"),
        ConnectionType::LocalTerminal { .. } => Some("local_terminal"),
        ConnectionType::Telnet { .. } => Some("telnet"),
        ConnectionType::Serial { .. } => Some("serial"),
        ConnectionType::Rdp { .. } | ConnectionType::Vnc { .. } => None,
    }
}

pub(in crate::features) fn connection_group_path(
    group_id: Option<&str>,
    groups: &HashMap<&str, &Group>,
) -> Vec<String> {
    let mut path = Vec::new();
    let mut visited = HashSet::new();
    let mut current = group_id;
    while let Some(id) = current {
        if !visited.insert(id.to_string()) {
            break;
        }
        let Some(group) = groups.get(id) else {
            break;
        };
        path.push(group.name.clone());
        current = group.parent_id.as_deref();
    }
    path.reverse();
    path
}

fn map_file_entry(entry: TransportFileEntry) -> SftpFileEntry {
    let is_dir = entry.is_directory();
    let is_symlink = entry.is_symlink();
    SftpFileEntry {
        name: entry.name,
        is_dir,
        is_symlink,
        size: entry.size.unwrap_or(0),
        permissions: permissions(entry.permissions),
        owner: entry.owner,
        group: entry.group,
        mtime: entry.modified_at.map_or(0, u64::from),
        raw_path_token: entry.raw_path_token,
    }
}

fn parse_remote_mode(mode: Option<&str>) -> Result<Option<u32>, String> {
    mode.map(|mode| {
        let mode = mode.trim().strip_prefix("0o").unwrap_or(mode.trim());
        u32::from_str_radix(mode, 8).map_err(|error| error.to_string())
    })
    .transpose()
}

fn permissions(mode: Option<u32>) -> String {
    mode.map_or_else(String::new, |mode| format!("{:04o}", mode & 0o7777))
}

pub(in crate::features) fn mcp_request_target(request: &CapabilityRequest) -> Option<String> {
    request
        .arguments
        .get("sessionId")
        .or_else(|| request.arguments.get("connectionId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub(in crate::features) fn mcp_grant_key(
    request: &CapabilityRequest,
) -> Option<(String, String, String)> {
    mcp_request_target(request).map(|target| {
        let tool_scope = if matches!(request.caller, CapabilityCaller::Native { .. }) {
            format!(
                "{}:{}",
                request.tool,
                hex::encode(Sha256::digest(request.arguments.to_string().as_bytes()))
            )
        } else {
            request.tool.clone()
        };
        (request.connection_id.clone(), target, tool_scope)
    })
}

fn mcp_parameter_summary(tool_name: &str, arguments: &Value) -> String {
    match tool_name {
        tool::TERMINAL_EXECUTE => arguments
            .get("command")
            .and_then(Value::as_str)
            .map(|command| truncate_summary(command, 240))
            .unwrap_or_else(|| "terminal command".to_string()),
        tool::SFTP_RENAME => format!(
            "{} -> {}",
            arguments
                .get("oldPath")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            arguments
                .get("newPath")
                .and_then(Value::as_str)
                .unwrap_or("?"),
        ),
        tool::SFTP_WRITE_TEXT => format!(
            "write {} bytes to {}{}",
            arguments
                .get("content")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0),
            arguments.get("path").and_then(Value::as_str).unwrap_or("?"),
            if arguments
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                " (force)"
            } else {
                ""
            },
        ),
        _ => arguments
            .get("path")
            .or_else(|| arguments.get("connectionId"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tool_name.to_string()),
    }
}

fn truncate_summary(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let summary = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilityCaller, CapabilityRequest, CapabilityRuntimeState, mcp_grant_key};
    use gpui::AppContext as _;
    use nyaterm_core::{AiPermissionMode, CapabilityScope};
    use nyaterm_mcp_protocol::{MAX_INLINE_OUTPUT_BYTES, OutputReadArgs, tool};
    use serde_json::json;

    fn request(owner: &str, caller: CapabilityCaller, command: &str) -> CapabilityRequest {
        let (reply, _) = tokio::sync::oneshot::channel();
        CapabilityRequest {
            connection_id: owner.into(),
            request_id: format!("{owner}:call"),
            generation: "generation".into(),
            client: "fixture".into(),
            permission_mode: AiPermissionMode::Confirm,
            scope: CapabilityScope::AllSessions,
            tool: tool::TERMINAL_EXECUTE.into(),
            arguments: json!({"sessionId":"s","command":command}),
            cancellation: tokio_util::sync::CancellationToken::new(),
            approved: false,
            approval_decision: None,
            caller,
            reply,
        }
    }

    #[gpui::test]
    fn approval_rechecks_live_scope_and_execution_disabled_before_dispatch(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::CapabilityApprovalOutcome;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::TestConfigDir;
        use nyaterm_core::ai::harness::AgentToolRegistry;
        use nyaterm_core::{AgentCommandExecutionMode, AiExecutionProfile};
        cx.executor().allow_parking();
        let directory = TestConfigDir::new("nyaterm-capability-approval-revalidation");
        let app = app_with_visible_local_session(cx, directory.path(), "s");
        let mut receiver = cx.update_entity(&app,|app,cx| {
            let mut settings = app.ai.settings_config_cloned();
            settings.agent_command_execution_mode = AgentCommandExecutionMode::ConfirmEach;
            app.ai.replace_settings_config(settings,true);
            app.ai.begin_native_run(serde_json::from_value(json!({"action":"generate_command","mode":"agent","userInput":"inspect","targets":[{"terminalSessionId":"s","label":"fixture","sessionType":"local"}]})).unwrap());
            let call = AgentToolRegistry::parse(&[],r#"{"action":"terminal_execute","arguments":{"sessionId":"s","command":"echo private-command"}}"#).unwrap();
            let call_id = call.id.clone();
            let (run_id,scope,cancellation) = app.ai.begin_native_call(call).unwrap();
            let (reply,receiver) = tokio::sync::oneshot::channel();
            let request_id = format!("native:{run_id}:{call_id}");
            let mut invocation = request(&format!("native:{run_id}"),CapabilityCaller::Native{run_id,ai_session_id:app.ai.chat_session_id().into(),model_risk:None},"echo private-command");
            invocation.request_id = request_id.clone();
            invocation.scope = scope;
            invocation.cancellation = cancellation;
            invocation.reply = reply;
            app.dispatch_capability_request(invocation,cx);
            let approvals = app.mcp.capabilities.pending_approval_requests();
            assert_eq!(approvals.len(),1);
            assert!(!approvals[0].allow_session);
            let CapabilityApprovalOutcome::Dispatch(approved) = app.mcp.capabilities.decide_approval(&request_id,nyaterm_core::capabilities::AgentApprovalDecision::AllowOnce).unwrap() else { panic!("approved invocation"); };
            app.session.metadata_mut("s").unwrap().ai_execution_profile = AiExecutionProfile::Disabled;
            app.dispatch_capability_request(approved,cx);
            assert!(app.ai.agent_loop_snapshot().is_none());
            receiver
        });
        assert_eq!(
            receiver.try_recv().unwrap().unwrap_err().code,
            "permission_denied"
        );
        let mut receiver = cx.update_entity(&app, |app, cx| {
            let native = app.ai.native_run_view().unwrap();
            let (reply, receiver) = tokio::sync::oneshot::channel();
            let mut invocation = request(
                &format!("native:{}", native.run_id),
                CapabilityCaller::Native {
                    run_id: native.run_id,
                    ai_session_id: app.ai.chat_session_id().into(),
                    model_risk: None,
                },
                "echo private-command",
            );
            invocation.scope =
                CapabilityScope::explicit(["outside".into()], Some("outside".into()));
            invocation.approved = true;
            invocation.reply = reply;
            app.dispatch_capability_request(invocation, cx);
            receiver
        });
        assert_eq!(
            receiver.try_recv().unwrap().unwrap_err().code,
            "scope_denied"
        );
        let (credential, mut receiver) = cx.update_entity(&app, |app, cx| {
            let credential = app
                .create_ephemeral_mcp_credential(
                    vec!["s".into()],
                    Some("s".into()),
                    AiPermissionMode::FullAccess,
                    super::terminal_execution::TerminalExecutionPolicy {
                        background: false,
                        max_lines: 10,
                    },
                )
                .unwrap();
            let generation = credential
                .environment()
                .remove("NYATERM_MCP_GENERATION")
                .unwrap();
            let (reply, receiver) = tokio::sync::oneshot::channel();
            let mut invocation = request("external", CapabilityCaller::Mcp, "echo private-command");
            invocation.generation = generation;
            invocation.permission_mode = AiPermissionMode::FullAccess;
            invocation.scope = CapabilityScope::explicit(["s".into()], Some("s".into()));
            invocation.reply = reply;
            app.dispatch_capability_request(invocation, cx);
            (credential, receiver)
        });
        assert_eq!(
            receiver.try_recv().unwrap().unwrap_err().code,
            "permission_denied"
        );
        drop(credential);
        let cleanup = cx.update_entity(&app, |app, cx| app.shutdown_blocking_jobs(cx));
        cx.foreground_executor().block_test(cleanup);
        cx.run_until_parked();
    }

    #[test]
    fn runtime_pages_utf8_output_per_owner_and_revokes_only_expired_credentials() {
        let mut state = CapabilityRuntimeState::default();
        let value = json!({"output":"猫".repeat(MAX_INLINE_OUTPUT_BYTES),"exitCode":null,"durationMs":1,"timedOut":false,"sourceTruncated":false});
        let protected = state
            .protect_output("native:run", tool::TERMINAL_EXECUTE, value.clone())
            .unwrap();
        assert_eq!(protected["truncated"], true);
        let output_id = protected["outputId"].as_str().unwrap().to_string();
        let args = || OutputReadArgs {
            output_id: output_id.clone(),
            offset: 0,
            max_bytes: Some(7),
        };
        let chunk = state.read_output("native:run", args()).unwrap();
        assert!(chunk["data"].as_str().unwrap().len() <= 7);
        assert_eq!(
            state.read_output("other-owner", args()).unwrap_err().code,
            "output_not_found"
        );
        state.owners.insert(
            "native:run".into(),
            (
                CapabilityCaller::Native {
                    run_id: "run".into(),
                    ai_session_id: "chat".into(),
                    model_risk: None,
                },
                String::new(),
            ),
        );
        state
            .owners
            .insert("mcp".into(), (CapabilityCaller::Mcp, "expired".into()));
        state
            .protect_output("mcp", tool::TERMINAL_EXECUTE, value)
            .unwrap();
        let active = request("mcp", CapabilityCaller::Mcp, "pwd");
        let cancellation = active.cancellation.clone();
        state.active_requests.insert(
            active.request_id.clone(),
            ("mcp".into(), cancellation.clone()),
        );
        state.revoke_generation("expired");
        assert!(cancellation.is_cancelled());
        assert!(!state.outputs.contains_key("mcp"));
        assert!(state.read_output("native:run", args()).is_ok());
        state.connection_disconnected("native:run");
        assert_eq!(
            state.read_output("native:run", args()).unwrap_err().code,
            "output_not_found"
        );
        assert!(
            state
                .protect_output("owner", tool::TERMINAL_EXECUTE, json!({"output":"invalid"}))
                .is_err()
        );
    }

    #[test]
    fn native_grants_bind_run_target_tool_and_identical_arguments_while_mcp_retains_tool_granularity()
     {
        let caller = CapabilityCaller::Native {
            run_id: "run".into(),
            ai_session_id: "chat".into(),
            model_risk: None,
        };
        let first = request("native:run", caller.clone(), "pwd");
        let mut changed = request("native:run", caller.clone(), "ls");
        assert_ne!(mcp_grant_key(&first), mcp_grant_key(&changed));
        changed.arguments = first.arguments.clone();
        assert_eq!(mcp_grant_key(&first), mcp_grant_key(&changed));
        changed.arguments["sessionId"] = json!("other");
        assert_ne!(mcp_grant_key(&first), mcp_grant_key(&changed));
        assert_ne!(
            mcp_grant_key(&first),
            mcp_grant_key(&request("native:other-run", caller, "pwd"))
        );
        assert_eq!(
            mcp_grant_key(&request("mcp", CapabilityCaller::Mcp, "pwd")),
            mcp_grant_key(&request("mcp", CapabilityCaller::Mcp, "ls"))
        );
    }
}
