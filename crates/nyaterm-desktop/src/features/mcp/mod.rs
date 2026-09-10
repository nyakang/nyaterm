mod discovery;
mod server;
#[cfg(windows)]
mod windows_acl;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::models::SessionLaunchConfig;
use futures::StreamExt;
use gpui::Context;
use nyaterm_core::{
    AiExecutionProfile, AppendAiAuditRequest, CapabilityScope, CapabilityScopeSnapshot,
    CapabilitySession, ConnectionType, ExternalMcpSessionScope, ExternalMcpSettings, Group,
    PolicyDecision, RiskAssessment, RiskLevel, SftpRiskOperation, assess_command_risk,
    assess_sftp_risk, decide_policy, sanitize_ai_diagnostic,
};
use nyaterm_mcp_protocol::{
    ConnectionListResult, ConnectionSummary, EnvironmentResult, MutationResult, PathArgs, RpcError,
    SessionArgs, SessionGetResult, SessionOpenArgs, SessionOpenResult, SessionSummary,
    SftpChmodArgs, SftpFileEntry, SftpHomeResult, SftpMkdirArgs, SftpReadTextArgs,
    SftpReadTextResult, SftpRenameArgs, SftpStatResult, SftpWriteTextArgs, SftpWriteTextResult,
    TerminalExecuteArgs, TerminalExecuteResult, TerminalRecentOutputArgs,
    TerminalRecentOutputResult, definition_for_tool, tool,
};
use nyaterm_store::StoreDomain;

use nyaterm_transport::{
    SessionKind, SftpFileEntry as TransportFileEntry, SshProcessService, SshSessionConfig,
    run_local_command,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use self::server::{McpHostEvent, McpHostRequest, McpHostRuntime, rpc_failure};
use super::NyaTermApp;
use super::ai::{McpHelperStatus, mcp_helper_status};
use super::formatting::recent_terminal_output;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum McpApprovalDecision {
    Deny,
    AllowOnce,
    AllowSession,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct McpApprovalRequest {
    pub request_id: String,
    pub client: String,
    pub capability: String,
    pub target: Option<String>,
    pub parameter_summary: String,
    pub risk_level: String,
    pub destructive: bool,
}

enum McpCommandTarget {
    Ssh(Box<SshSessionConfig>),
    Local { working_dir: Option<PathBuf> },
}

struct PendingMcpSessionOpen {
    host_request_id: String,
    client_connection_id: String,
    cancellation: tokio_util::sync::CancellationToken,
    reply: tokio::sync::oneshot::Sender<Result<Value, RpcError>>,
    connection_id: String,
    connection_name: String,
    connection_type: String,
}

#[derive(Clone)]
struct McpAuditContext {
    connection_id: Option<String>,
    client: String,
    capability: String,
    session_id: Option<String>,
    permission_mode: nyaterm_core::AiPermissionMode,
    risk_level: Option<RiskLevel>,
    approval_decision: Option<String>,
    started_at: Instant,
}

struct PendingMcpApproval {
    request: McpHostRequest,
    approval: McpApprovalRequest,
    grant_key: Option<(String, String, String)>,
}

enum McpApprovalOutcome {
    Dispatch(McpHostRequest),
    Denied(McpHostRequest),
}

use super::runtime_jobs::await_blocking_result;

const MCP_WINDOW_OWNER_PREFIX: &str = "mcp-window-";

pub(in crate::features) struct McpHostFeatureState {
    runtime: Option<McpHostRuntime>,
    helper_status: McpHelperStatus,
    requests: Option<futures::channel::mpsc::UnboundedReceiver<McpHostEvent>>,
    request_sender: futures::channel::mpsc::UnboundedSender<McpHostEvent>,
    owner_window_label: String,
    generation: Option<String>,
    ephemeral_generations: std::sync::Arc<std::sync::Mutex<HashSet<String>>>,
    pending_approvals: HashMap<String, PendingMcpApproval>,
    approval_order: VecDeque<String>,
    session_grants: HashSet<(String, String, String)>,
    pending_session_opens: HashMap<String, PendingMcpSessionOpen>,
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
    generation_registry: std::sync::Arc<std::sync::Mutex<HashSet<String>>>,
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
                    pending_approvals: HashMap::new(),
                    approval_order: VecDeque::new(),
                    session_grants: HashSet::new(),
                    pending_session_opens: HashMap::new(),
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
            pending_approvals: HashMap::new(),
            approval_order: VecDeque::new(),
            session_grants: HashSet::new(),
            pending_session_opens: HashMap::new(),
        }
    }

    fn prune_cancelled(&mut self) {
        self.pending_approvals
            .retain(|_, pending| !pending.request.cancellation.is_cancelled());
        self.approval_order
            .retain(|request_id| self.pending_approvals.contains_key(request_id));
        self.pending_session_opens
            .retain(|_, pending| !pending.cancellation.is_cancelled());
    }

    fn pending_approval_requests(&mut self) -> Vec<McpApprovalRequest> {
        self.prune_cancelled();
        self.approval_order
            .iter()
            .filter_map(|request_id| self.pending_approvals.get(request_id))
            .map(|pending| pending.approval.clone())
            .collect()
    }

    fn has_session_grant(&self, key: &(String, String, String)) -> bool {
        self.session_grants.contains(key)
    }

    fn queue_approval(&mut self, pending: PendingMcpApproval) {
        let request_id = pending.approval.request_id.clone();
        if self
            .pending_approvals
            .insert(request_id.clone(), pending)
            .is_none()
        {
            self.approval_order.push_back(request_id);
        }
    }

    fn decide_approval(
        &mut self,
        request_id: &str,
        decision: McpApprovalDecision,
    ) -> Option<McpApprovalOutcome> {
        self.prune_cancelled();
        let mut pending = self.pending_approvals.remove(request_id)?;
        self.approval_order.retain(|id| id != request_id);
        if pending.request.cancellation.is_cancelled() {
            return None;
        }
        match decision {
            McpApprovalDecision::Deny => {
                pending.request.approval_decision = Some("deny".to_string());
                Some(McpApprovalOutcome::Denied(pending.request))
            }
            McpApprovalDecision::AllowOnce => {
                pending.request.approved = true;
                pending.request.approval_decision = Some("allow_once".to_string());
                Some(McpApprovalOutcome::Dispatch(pending.request))
            }
            McpApprovalDecision::AllowSession => {
                if !pending.approval.destructive
                    && let Some(key) = pending.grant_key
                {
                    self.session_grants.insert(key);
                }
                pending.request.approved = true;
                pending.request.approval_decision = Some("allow_session".to_string());
                Some(McpApprovalOutcome::Dispatch(pending.request))
            }
        }
    }

    fn request_cancelled(&mut self, request_id: &str) -> Vec<McpHostRequest> {
        let mut cancelled = Vec::new();
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

    fn connection_disconnected(&mut self, client_connection_id: &str) -> Vec<McpHostRequest> {
        let mut disconnected = Vec::new();
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

    fn session_open_is_pending(&mut self, connection_id: &str) -> bool {
        self.prune_cancelled();
        self.pending_session_opens.contains_key(connection_id)
    }

    fn register_session_open(&mut self, pending: PendingMcpSessionOpen) {
        debug_assert!(
            !self
                .pending_session_opens
                .contains_key(&pending.connection_id)
        );
        self.pending_session_opens
            .insert(pending.connection_id.clone(), pending);
    }

    fn complete_session_open_success(&mut self, connection_id: &str, session_id: String) {
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

    fn complete_session_open_failure(&mut self, connection_id: &str, error: &str) {
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

    fn create_ephemeral(
        &self,
        session_ids: Vec<String>,
        default_session_id: Option<String>,
        permission_mode: nyaterm_core::AiPermissionMode,
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
            .insert(generation.clone());
        Ok(McpEphemeralCredential {
            runtime,
            generation,
            generation_registry: self.ephemeral_generations.clone(),
        })
    }

    fn take_request_receiver(
        &mut self,
    ) -> Option<futures::channel::mpsc::UnboundedReceiver<McpHostEvent>> {
        self.requests.take()
    }

    fn generation_matches(&self, generation: &str) -> bool {
        (self.generation.as_deref() == Some(generation) && self.runtime.is_some())
            || self
                .ephemeral_generations
                .lock()
                .is_ok_and(|generations| generations.contains(generation))
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

    fn helper_status(&self) -> McpHelperStatus {
        self.helper_status
    }

    fn reconfigure(&mut self, settings: &ExternalMcpSettings) -> Result<(), String> {
        self.helper_status = mcp_helper_status();
        self.runtime.take();
        self.generation = None;
        self.pending_approvals.clear();
        self.approval_order.clear();
        self.session_grants.clear();
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
                            this.dispatch_mcp_host_request(*request, cx)
                        }
                        McpHostEvent::Cancelled { request_id } => {
                            this.handle_pending_mcp_cancel(&request_id, cx)
                        }
                        McpHostEvent::Disconnected { connection_id } => {
                            this.handle_mcp_disconnect(&connection_id, cx)
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

    pub(in crate::features) fn mcp_pending_approval_requests(&mut self) -> Vec<McpApprovalRequest> {
        self.mcp.pending_approval_requests()
    }

    pub(in crate::features) fn respond_to_mcp_approval(
        &mut self,
        request_id: &str,
        decision: McpApprovalDecision,
        cx: &mut Context<Self>,
    ) {
        match self.mcp.decide_approval(request_id, decision) {
            Some(McpApprovalOutcome::Dispatch(request)) => {
                self.dispatch_mcp_host_request(request, cx)
            }
            Some(McpApprovalOutcome::Denied(request)) => {
                let assessment = mcp_risk_assessment(&request.tool, &request.arguments);
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
        cx.notify();
    }

    fn handle_pending_mcp_cancel(&mut self, request_id: &str, cx: &mut Context<Self>) {
        for request in self.mcp.request_cancelled(request_id) {
            let assessment = mcp_risk_assessment(&request.tool, &request.arguments);
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
        cx.notify();
    }

    fn handle_mcp_disconnect(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        for request in self.mcp.connection_disconnected(connection_id) {
            let assessment = mcp_risk_assessment(&request.tool, &request.arguments);
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
        cx.notify();
    }

    pub(in crate::features) fn complete_mcp_session_open_success(
        &mut self,
        connection_id: &str,
        session_id: String,
    ) {
        self.mcp
            .complete_session_open_success(connection_id, session_id);
    }

    pub(in crate::features) fn complete_mcp_session_open_failure(
        &mut self,
        connection_id: &str,
        error: &str,
    ) {
        self.mcp.complete_session_open_failure(connection_id, error);
    }

    pub(in crate::features) fn create_ephemeral_mcp_credential(
        &self,
        session_ids: Vec<String>,
        default_session_id: Option<String>,
        permission_mode: nyaterm_core::AiPermissionMode,
    ) -> Result<McpEphemeralCredential, String> {
        self.mcp
            .create_ephemeral(session_ids, default_session_id, permission_mode)
    }

    pub(in crate::features) fn mcp_host_status(&self) -> McpHostStatus {
        self.mcp
            .status(self.ai.settings_config().external_mcp.enabled)
    }

    pub(in crate::features) fn mcp_helper_status(&self) -> McpHelperStatus {
        self.mcp.helper_status()
    }

    pub(in crate::features) fn reconfigure_mcp_host(&mut self) -> Result<(), String> {
        self.mcp
            .reconfigure(&self.ai.settings_config().external_mcp)
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
    fn record_mcp_audit(
        &mut self,
        request: &McpHostRequest,
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
            source: Some("mcp".to_string()),
            client: Some(sanitize_ai_diagnostic(&request.client, 128)),
            capability: Some(request.tool.clone()),
            session_id: request
                .arguments
                .get("sessionId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            permission_mode: Some(request.permission_mode.clone()),
            approval_decision: approval_decision.map(ToOwned::to_owned),
            success,
            duration_ms,
            error: error.map(|error| sanitize_ai_diagnostic(error, 256)),
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
            inserted_to_terminal: audit_context.capability == tool::TERMINAL_EXECUTE && success,
            executed: success,
            blocked: !success,
            source: Some("mcp".to_string()),
            client: Some(audit_context.client),
            capability: Some(audit_context.capability),
            session_id: audit_context.session_id,
            permission_mode: Some(audit_context.permission_mode),
            approval_decision: audit_context.approval_decision,
            success: Some(success),
            duration_ms: Some(duration_ms),
            error: error.map(|error| sanitize_ai_diagnostic(error, 256)),
        };
        self.persist_mcp_audit(audit, cx);
    }
    fn dispatch_mcp_host_request(&mut self, mut request: McpHostRequest, cx: &mut Context<Self>) {
        let _client_identity = request.client.as_str();
        if !self.mcp.generation_matches(&request.generation) {
            let _ = request.reply.send(Err(rpc_failure(
                "authentication_failed",
                "MCP credential was rotated or expired.",
            )));
            return;
        }
        if request.cancellation.is_cancelled() {
            let _ = request.reply.send(Err(rpc_failure(
                "cancelled",
                "The MCP request was cancelled.",
            )));
            return;
        }
        let Some(definition) = definition_for_tool(&request.tool) else {
            let _ = request.reply.send(Err(rpc_failure(
                "invalid_argument",
                "Unknown NyaTerm MCP tool.",
            )));
            return;
        };
        let assessment = mcp_risk_assessment(&request.tool, &request.arguments);
        let policy = decide_policy(
            &request.permission_mode,
            definition.access,
            assessment.as_ref(),
        );
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
                let _ = request.reply.send(Err(rpc_failure(
                    "permission_denied",
                    "The current MCP permission mode denies this capability.",
                )));
                return;
            }
            PolicyDecision::RequireApproval => {
                let grant_key = mcp_grant_key(&request);
                let session_granted = grant_key
                    .as_ref()
                    .is_some_and(|key| self.mcp.has_session_grant(key));
                if !request.approved && !session_granted {
                    let approval = McpApprovalRequest {
                        request_id: request.request_id.clone(),
                        client: request.client.clone(),
                        capability: request.tool.clone(),
                        target: mcp_request_target(&request),
                        parameter_summary: mcp_parameter_summary(&request.tool, &request.arguments),
                        risk_level: assessment
                            .as_ref()
                            .map(|risk| format!("{:?}", risk.level).to_ascii_lowercase())
                            .unwrap_or_else(|| "sensitive".to_string()),
                        destructive: matches!(
                            definition.access,
                            nyaterm_mcp_protocol::CapabilityAccess::DestructiveWrite
                        ),
                    };
                    self.mcp.queue_approval(PendingMcpApproval {
                        request,
                        approval,
                        grant_key,
                    });
                    cx.notify();
                    return;
                }
            }
            PolicyDecision::Allow => {}
        }

        let approval_decision = if request.approved {
            request.approval_decision.as_deref()
        } else if mcp_grant_key(&request)
            .as_ref()
            .is_some_and(|key| self.mcp.has_session_grant(key))
        {
            Some("session_grant")
        } else {
            None
        };
        let audit_context = McpAuditContext {
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
        };
        let (audit_reply, audit_result) = tokio::sync::oneshot::channel();
        let original_reply = std::mem::replace(&mut request.reply, audit_reply);
        cx.spawn(async move |this, cx| {
            let result = audit_result.await.unwrap_or_else(|_| {
                Err(rpc_failure(
                    "host_unavailable",
                    "NyaTerm closed the MCP capability request.",
                ))
            });
            let success = result.is_ok();
            let error = result.as_ref().err().map(|error| error.message.clone());
            let _ = this.update(cx, |this, cx| {
                this.record_mcp_audit_result(audit_context, success, error.as_deref(), cx);
            });
            let _ = original_reply.send(result);
        })
        .detach();

        let scope = request.scope.resolve(&self.mcp_live_sessions());
        match request.tool.as_str() {
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
                self.dispatch_mcp_terminal_execute(request, &scope, cx);
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

    fn mcp_live_sessions(&self) -> Vec<CapabilitySession> {
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
        request: McpHostRequest,
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
    fn dispatch_mcp_session_open(&mut self, request: McpHostRequest, cx: &mut Context<Self>) {
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
        if self.mcp.session_open_is_pending(&connection.id) {
            let _ = request.reply.send(Err(rpc_failure(
                "execution_failed",
                "This saved connection is already being opened by MCP.",
            )));
            return;
        }
        self.mcp.register_session_open(PendingMcpSessionOpen {
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
        request: McpHostRequest,
        scope: &CapabilityScopeSnapshot,
        cx: &mut Context<Self>,
    ) {
        let args = serde_json::from_value::<TerminalExecuteArgs>(request.arguments)
            .expect("sidecar validated terminal execute arguments");
        let session_id = match args.session_id.or_else(|| scope.default_session_id.clone()) {
            Some(session_id) => session_id,
            None => {
                let _ = request.reply.send(Err(rpc_failure(
                    "invalid_argument",
                    "terminal_execute requires a target session when no default is available.",
                )));
                return;
            }
        };
        if let Err(error) = scope.require(&session_id) {
            let _ = request
                .reply
                .send(Err(rpc_failure("scope_denied", &error.to_string())));
            return;
        }
        let Some(session) = self
            .session
            .session_info(&session_id)
            .filter(|_| !self.session.is_disconnected(&session_id))
        else {
            let _ = request.reply.send(Err(rpc_failure(
                "invalid_argument",
                "The target terminal session is unavailable.",
            )));
            return;
        };
        let target = match session.kind {
            SessionKind::Ssh => self.session.metadata(&session_id).and_then(|metadata| {
                match &metadata.launch_config {
                    SessionLaunchConfig::Ssh(config) => {
                        Some(McpCommandTarget::Ssh(Box::new(config.as_ref().clone())))
                    }
                    _ => None,
                }
            }),
            SessionKind::LocalPty => Some(McpCommandTarget::Local {
                working_dir: session.working_dir,
            }),
            _ => None,
        };
        let Some(target) = target else {
            let _ = request.reply.send(Err(rpc_failure(
                "permission_denied",
                "Background execution is supported only for SSH and local sessions.",
            )));
            return;
        };
        let command = args.command;
        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(30_000));
        let cancellation = request.cancellation;
        let task = self
            .blocking_jobs
            .submit_task("mcp-terminal-execute", move |_| {
                if cancellation.is_cancelled() {
                    return Err("The MCP request was cancelled.".to_string());
                }
                let started = Instant::now();
                let result = match target {
                    McpCommandTarget::Ssh(config) => {
                        SshProcessService::new(*config).run_command(&command, timeout)
                    }
                    McpCommandTarget::Local { working_dir } => {
                        run_local_command(&command, working_dir, timeout)
                    }
                };
                let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                if cancellation.is_cancelled() {
                    return Err("The MCP request was cancelled.".to_string());
                }
                match result {
                    Ok(output) => {
                        let mut combined = output.stdout;
                        if !output.stderr.is_empty() {
                            if !combined.is_empty() && !combined.ends_with('\n') {
                                combined.push('\n');
                            }
                            combined.push_str(&output.stderr);
                        }
                        to_value_string(TerminalExecuteResult {
                            output: combined,
                            exit_code: output.exit_status.and_then(|code| i32::try_from(code).ok()),
                            duration_ms,
                            timed_out: false,
                            source_truncated: false,
                        })
                    }
                    Err(error) if error.to_string().to_ascii_lowercase().contains("timed out") => {
                        to_value_string(TerminalExecuteResult {
                            output: String::new(),
                            exit_code: None,
                            duration_ms,
                            timed_out: true,
                            source_truncated: false,
                        })
                    }
                    Err(error) => Err(error.to_string()),
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

    fn dispatch_mcp_sftp_mutation(
        &mut self,
        request: McpHostRequest,
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
                                })
                            }
                            nyaterm_transport::RemoteTextWriteResult::Conflict => {
                                to_value_string(SftpWriteTextResult {
                                    status: "conflict".to_string(),
                                    mtime: None,
                                    size: None,
                                    mtime_nanos: None,
                                    content_hash: None,
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

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, RpcError> {
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

fn connection_type_name(connection: &ConnectionType) -> Option<&'static str> {
    match connection {
        ConnectionType::Ssh { .. } => Some("ssh"),
        ConnectionType::LocalTerminal { .. } => Some("local_terminal"),
        ConnectionType::Telnet { .. } => Some("telnet"),
        ConnectionType::Serial { .. } => Some("serial"),
        ConnectionType::Rdp { .. } | ConnectionType::Vnc { .. } => None,
    }
}

fn connection_group_path(group_id: Option<&str>, groups: &HashMap<&str, &Group>) -> Vec<String> {
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

fn mcp_risk_assessment(tool_name: &str, arguments: &Value) -> Option<RiskAssessment> {
    match tool_name {
        tool::SESSION_OPEN => Some(RiskAssessment {
            level: RiskLevel::Medium,
            reason: "opening a saved connection changes live session state".to_string(),
            auto_executable: true,
        }),
        tool::TERMINAL_EXECUTE => serde_json::from_value::<TerminalExecuteArgs>(arguments.clone())
            .ok()
            .map(|args| assess_command_risk(&args.command)),
        tool::SFTP_WRITE_TEXT => serde_json::from_value::<SftpWriteTextArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Write,
                    &args.path,
                    None,
                    args.force.unwrap_or(false),
                    None,
                )
            }),
        tool::SFTP_MKDIR => serde_json::from_value::<SftpMkdirArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Mkdir,
                    &args.path,
                    None,
                    false,
                    args.mode.as_deref(),
                )
            }),
        tool::SFTP_RENAME => serde_json::from_value::<SftpRenameArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Rename,
                    &args.old_path,
                    Some(&args.new_path),
                    false,
                    None,
                )
            }),
        tool::SFTP_DELETE => serde_json::from_value::<PathArgs>(arguments.clone())
            .ok()
            .map(|args| assess_sftp_risk(SftpRiskOperation::Delete, &args.path, None, false, None)),
        tool::SFTP_CHMOD => serde_json::from_value::<SftpChmodArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Chmod,
                    &args.path,
                    None,
                    false,
                    Some(&args.mode),
                )
            }),
        _ => None,
    }
}

fn mcp_request_target(request: &McpHostRequest) -> Option<String> {
    request
        .arguments
        .get("sessionId")
        .or_else(|| request.arguments.get("connectionId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn mcp_grant_key(request: &McpHostRequest) -> Option<(String, String, String)> {
    mcp_request_target(request)
        .map(|target| (request.connection_id.clone(), target, request.tool.clone()))
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
    use std::collections::HashMap;

    use nyaterm_core::{
        AiPermissionMode, CapabilityAccess, CapabilityScope, ConnectionType, Group, PolicyDecision,
        decide_policy,
    };
    use nyaterm_mcp_protocol::{RpcError, tool};
    use serde_json::Value;

    use super::server::{McpHostRequest, rpc_failure};
    use super::{
        McpApprovalDecision, McpApprovalOutcome, McpApprovalRequest, McpHostFeatureState,
        PendingMcpApproval, connection_group_path, connection_type_name, mcp_grant_key,
    };

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
        state.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-1".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_WRITE_TEXT.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "fixture summary".to_string(),
                risk_level: "medium".to_string(),
                destructive: false,
            },
            grant_key: Some(grant_key.clone()),
        });
        assert_eq!(state.pending_approval_requests().len(), 1);
        let McpApprovalOutcome::Dispatch(approved) = state
            .decide_approval("request-1", McpApprovalDecision::AllowSession)
            .expect("approved request")
        else {
            panic!("approval should dispatch");
        };
        assert!(approved.approved);
        assert!(state.has_session_grant(&grant_key));

        let (request, _receiver) = approval_request("request-2", "connection-1");
        let destructive_key = (
            "connection-1".to_string(),
            "session-1".to_string(),
            tool::SFTP_DELETE.to_string(),
        );
        state.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-2".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_DELETE.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "/tmp/test".to_string(),
                risk_level: "high".to_string(),
                destructive: true,
            },
            grant_key: Some(destructive_key.clone()),
        });
        let McpApprovalOutcome::Dispatch(approved) = state
            .decide_approval("request-2", McpApprovalDecision::AllowSession)
            .expect("one-time destructive approval")
        else {
            panic!("approval should dispatch");
        };
        assert!(approved.approved);
        assert!(!state.has_session_grant(&destructive_key));
    }

    #[tokio::test]
    async fn approval_denial_and_disconnect_close_pending_requests() {
        let mut state = McpHostFeatureState::disabled_for_test();
        let (request, denied) = approval_request("request-denied", "connection-1");
        state.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-denied".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_WRITE_TEXT.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "fixture".to_string(),
                risk_level: "medium".to_string(),
                destructive: false,
            },
            grant_key: None,
        });
        let McpApprovalOutcome::Denied(request) = state
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
        state.queue_approval(PendingMcpApproval {
            request,
            approval: McpApprovalRequest {
                request_id: "request-disconnect".to_string(),
                client: "fixture-client".to_string(),
                capability: tool::SFTP_WRITE_TEXT.to_string(),
                target: Some("session-1".to_string()),
                parameter_summary: "fixture".to_string(),
                risk_level: "medium".to_string(),
                destructive: false,
            },
            grant_key: None,
        });
        for request in state.connection_disconnected("connection-2") {
            let _ = request.reply.send(Err(rpc_failure(
                "cancelled",
                "The MCP client disconnected.",
            )));
        }
        assert_eq!(disconnected.await.unwrap().unwrap_err().code, "cancelled");
        assert!(state.pending_approval_requests().is_empty());
    }
}
