use super::{
    TerminalExecutionPolicy, TerminalExecutionRequest, TerminalExecutionState,
    TerminalExecutionStatus,
};
use crate::features::NyaTermApp;
use crate::features::capability_runtime::{CapabilityRequest, idempotency::Lookup, to_value};
use crate::features::mcp::server::rpc_failure;
use crate::models::SessionLaunchConfig;
use gpui::Context;
use nyaterm_core::capabilities::CapabilityScope;
use nyaterm_core::capabilities::stream_output::StreamOutput;
use nyaterm_core::models::connection::AiExecutionProfile;
use nyaterm_mcp_protocol::{
    TerminalAvailability, TerminalBackend, TerminalExecuteResult, TerminalJobStatus,
    TerminalPollArgs, TerminalPollResult, TerminalProcessState, TerminalStartArgs,
    TerminalStartResult, TerminalStopArgs, TerminalStopResult,
};
use nyaterm_terminal::recording_sanitizer::RecordingSanitizer;
use nyaterm_transport::ssh_execution::streaming::{SshExecEvent, stream_ssh_command};
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub(super) struct TerminalJob {
    pub mcp_owned: bool,
    pub owner: String,
    pub session_id: String,
    pub window_label: String,
    pub backend: TerminalBackend,
    pub status: TerminalJobStatus,
    pub process_state: TerminalProcessState,
    pub output: StreamOutput,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub finished: Option<Instant>,
    pub output_complete: bool,
    pub exit_code: Option<i32>,
    remote_failed: bool,
    pub stop_state: String,
    pub stop_started: Option<Instant>,
    pub interrupt: CancellationToken,
    pub receiver: Option<mpsc::Receiver<SshExecEvent>>,
    pub controller: Option<crate::ai_runtime::AiTask>,
    pub manual_takeover: bool,
    pub operation_id: Option<String>,
    sanitizer: RecordingSanitizer,
    stdout_tail: Vec<u8>,
    stderr_tail: Vec<u8>,
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
impl TerminalJob {
    pub fn from_request(request: &TerminalExecutionRequest) -> Self {
        Self {
            mcp_owned: request.mcp_owned,
            owner: request.owner.clone(),
            session_id: request.session_id.clone(),
            window_label: String::new(),
            backend: if request.policy.background {
                TerminalBackend::SshExec
            } else {
                TerminalBackend::InlinePty
            },
            status: TerminalJobStatus::Starting,
            process_state: TerminalProcessState::NotStarted,
            output: StreamOutput::new(4 * 1024 * 1024),
            started_at_ms: now_ms(),
            ended_at_ms: None,
            finished: None,
            output_complete: false,
            exit_code: None,
            remote_failed: false,
            stop_state: "not_requested".into(),
            stop_started: None,
            interrupt: CancellationToken::new(),
            receiver: None,
            controller: None,
            manual_takeover: false,
            operation_id: request.operation_id.clone(),
            sanitizer: RecordingSanitizer::default(),
            stdout_tail: Vec::new(),
            stderr_tail: Vec::new(),
        }
    }
    fn append(&mut self, text: &str) {
        let text = self.sanitizer.feed(text);
        self.output.append(&text);
    }
    fn exited(&mut self, code: Option<i32>, interrupted: bool) {
        self.exit_code = code;
        self.process_state = TerminalProcessState::ExitConfirmed;
        self.output_complete = true;
        if !matches!(
            self.status,
            TerminalJobStatus::TimedOut
                | TerminalJobStatus::Cancelled
                | TerminalJobStatus::Disconnected
        ) {
            self.status = if interrupted {
                TerminalJobStatus::Cancelled
            } else if self.remote_failed || code.is_some_and(|c| c != 0) {
                TerminalJobStatus::Failed
            } else {
                TerminalJobStatus::Completed
            };
        }
        if self.stop_started.is_some() {
            self.stop_state = "confirmed_ended".into();
        }
        self.ended_at_ms = Some(now_ms());
        self.finished = Some(Instant::now());
    }
    pub(super) fn disconnect(&mut self) {
        self.unknown(TerminalJobStatus::Disconnected);
        self.output.note_source_loss();
        self.output_complete = true;
        self.stop_state = "target_invalid".into();
    }
    fn unknown(&mut self, status: TerminalJobStatus) {
        self.status = status;
        if self.process_state != TerminalProcessState::ExitConfirmed {
            self.process_state = TerminalProcessState::Unknown;
        }
        self.ended_at_ms = Some(now_ms());
        self.finished = Some(Instant::now());
    }
}
impl TerminalExecutionState {
    pub fn stop_requested(&self, id: &str) -> bool {
        self.jobs
            .get(id)
            .is_some_and(|job| job.stop_started.is_some())
    }
    pub fn availability(&self, session_id: &str) -> TerminalAvailability {
        if self.recovery_required(session_id) {
            TerminalAvailability::RecoveryRequired
        } else if self.tasks.contains_key(session_id) {
            TerminalAvailability::Busy
        } else {
            TerminalAvailability::Ready
        }
    }
    pub fn owned_job_id(&self, session_id: &str, owner: &str) -> Option<String> {
        self.tasks
            .get(session_id)
            .filter(|task| !task.request.policy.background)
            .map(|t| t.capture_marker_id.as_str())
            .or_else(|| {
                self.recovery
                    .get(session_id)
                    .filter(|r| r.owner == owner)
                    .map(|r| r.marker_id.as_str())
            })
            .filter(|id| self.jobs.get(*id).is_some_and(|j| j.owner == owner))
            .map(str::to_string)
    }
    pub fn can_register_job(&self, owner: &str) -> bool {
        self.jobs.len() < 512
            && self.jobs.values().filter(|j| j.owner == owner).count() < 128
            && self.jobs.values().filter(|j| j.finished.is_none()).count() < 32
            && self
                .jobs
                .values()
                .filter(|j| j.owner == owner && j.finished.is_none())
                .count()
                < 8
    }
    pub fn note_manual_takeover(&mut self, session_id: &str) {
        if let Some(guard) = self.recovery.get(session_id)
            && let Some(job) = self.jobs.get_mut(&guard.marker_id)
        {
            job.manual_takeover = true;
            job.output.note_source_loss();
        }
        if let Some(task) = self.tasks.get_mut(session_id) {
            if let Some(job) = self.jobs.get_mut(&task.capture_marker_id) {
                job.manual_takeover = true;
                job.output.note_source_loss();
            }
            task.request.cancellation.cancel();
        }
    }
    pub(super) fn append_job_output(&mut self, id: &str, text: &str) {
        let owner = if let Some(job) = self.jobs.get_mut(id) {
            if !job.manual_takeover {
                job.append(text);
            }
            job.owner.clone()
        } else {
            return;
        };
        self.enforce_output_limits(&owner);
    }
    fn enforce_output_limits(&mut self, owner: &str) {
        for owner_only in [true, false] {
            let limit = if owner_only {
                16 * 1024 * 1024
            } else {
                64 * 1024 * 1024
            };
            let mut ids = self
                .jobs
                .iter()
                .filter(|(_, j)| !owner_only || j.owner == owner)
                .map(|(id, j)| (j.started_at_ms, id.clone()))
                .collect::<Vec<_>>();
            ids.sort();
            let total: usize = ids
                .iter()
                .map(|(_, id)| self.jobs[id].output.retained_bytes())
                .sum();
            let mut excess = total.saturating_sub(limit);
            for (_, id) in ids {
                let job = self.jobs.get_mut(&id).unwrap();
                let bytes = job.output.retained_bytes();
                let drop = bytes.min(excess);
                job.output.trim_to(bytes - drop);
                excess -= drop;
                if excess == 0 {
                    break;
                }
            }
        }
    }
    pub(super) fn observe_late_job_end(&mut self, id: &str, code: i32) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.exited(Some(code), job.stop_started.is_some() && code == 130);
        }
    }
    pub(super) fn finish_job(
        &mut self,
        id: &str,
        status: TerminalExecutionStatus,
        result: Option<&TerminalExecuteResult>,
        request: &TerminalExecutionRequest,
    ) {
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        if let Some(result) = result {
            if result.source_truncated {
                job.output.note_source_loss();
            }
            if request.policy.background {
                job.append(&result.output);
            }
        }
        match status {
            TerminalExecutionStatus::Completed if result.is_some_and(|r| r.exit_code.is_some()) => {
                let code = result.and_then(|r| r.exit_code);
                job.exited(code, job.stop_started.is_some() && code == Some(130));
            }
            TerminalExecutionStatus::TimedOut => job.unknown(TerminalJobStatus::TimedOut),
            TerminalExecutionStatus::Disconnected => job.unknown(TerminalJobStatus::Disconnected),
            TerminalExecutionStatus::Cancelled => job.unknown(TerminalJobStatus::Cancelled),
            TerminalExecutionStatus::Failed => job.unknown(TerminalJobStatus::Failed),
            _ => job.unknown(TerminalJobStatus::Unknown),
        }
    }
}
impl NyaTermApp {
    pub(in crate::features) fn authorize_terminal_job(
        &self,
        owner: &str,
        scope: &CapabilityScope,
        id: &str,
    ) -> Result<String, nyaterm_mcp_protocol::RpcError> {
        let job = self
            .mcp
            .capabilities
            .terminal_executions
            .jobs
            .get(id)
            .filter(|job| job.owner == owner)
            .ok_or_else(|| {
                rpc_failure(
                    "job_not_found",
                    "Job is unavailable or is owned by another client.",
                )
            })?;
        let permitted = if self.session.has_session(&job.session_id)
            && !self.session.is_disconnected(&job.session_id)
        {
            scope
                .resolve(&self.mcp_live_sessions())
                .session_ids
                .contains(&job.session_id)
        } else {
            match scope {
                CapabilityScope::AllSessions => true,
                CapabilityScope::Explicit { session_ids, .. } => {
                    session_ids.contains(&job.session_id)
                }
                CapabilityScope::CurrentWindow { owner_window_label } => {
                    owner_window_label == &job.window_label
                }
            }
        };
        if !permitted {
            return Err(rpc_failure(
                "scope_denied",
                "The job target is outside the current scope.",
            ));
        }
        Ok(job.session_id.clone())
    }
    pub(in crate::features) fn dispatch_terminal_start(
        &mut self,
        request: CapabilityRequest,
        cx: &mut Context<Self>,
    ) {
        let mut args: TerminalStartArgs =
            serde_json::from_value(request.arguments.clone()).expect("validated terminal start");
        let scope = request.scope.resolve(&self.mcp_live_sessions());
        let session_id = match scope.resolve_session(args.session_id.as_deref()) {
            Ok(id) => id,
            Err(_) => {
                let _ = request
                    .reply
                    .send(Err(rpc_failure("scope_denied", "Target is outside scope.")));
                return;
            }
        };
        if !matches!(
            self.ai_execution_profile_for_session(&session_id),
            AiExecutionProfile::Posix | AiExecutionProfile::Powershell | AiExecutionProfile::Cmd
        ) {
            let _ = request.reply.send(Err(rpc_failure(
                "permission_denied",
                "Long jobs require a supported command-capture shell.",
            )));
            return;
        }
        args.session_id = Some(session_id.clone());
        args.timeout_ms = Some(args.timeout_ms.unwrap_or(300_000));
        let canonical = serde_json::to_value(&args).expect("start arguments");
        let operation_id = match self.mcp.capabilities.ledger.begin(
            &request.connection_id,
            &request.tool,
            args.idempotency_key.as_deref(),
            &canonical,
        ) {
            Ok(Lookup::Complete(result)) => {
                let result = result.and_then(|v| {
                    self.authorize_terminal_job(
                        &request.connection_id,
                        &request.scope,
                        v["jobId"].as_str().unwrap_or_default(),
                    )?;
                    Ok(v)
                });
                let _ = request.reply.send(result);
                return;
            }
            Ok(Lookup::Pending(_)) => {
                let _ = request.reply.send(Err(rpc_failure(
                    "operation_pending",
                    "The original start is pending; do not replay it.",
                )));
                return;
            }
            Ok(Lookup::New(id)) => id,
            Err(error) => {
                let _ = request.reply.send(Err(error));
                return;
            }
        };
        let result = self.start_terminal_job(&request, args, session_id, operation_id.clone(), cx);
        self.mcp
            .capabilities
            .ledger
            .pin(&operation_id, result.is_ok());
        self.mcp
            .capabilities
            .ledger
            .finish(&operation_id, result.clone());
        let _ = request.reply.send(result);
        self.ensure_terminal_execution_clock(cx);
    }
    fn start_terminal_job(
        &mut self,
        request: &CapabilityRequest,
        args: TerminalStartArgs,
        session_id: String,
        operation_id: String,
        cx: &mut Context<Self>,
    ) -> Result<Value, nyaterm_mcp_protocol::RpcError> {
        if self.security.screen_locked() {
            return Err(rpc_failure("permission_denied", "Screen is locked."));
        }
        if request.cancellation.is_cancelled() {
            return Err(rpc_failure(
                "cancelled",
                "The start was cancelled before submission.",
            ));
        }
        if !self
            .mcp
            .capabilities
            .terminal_executions
            .can_register_job(&request.connection_id)
        {
            return Err(rpc_failure("resource_limit", "The job registry is full."));
        }
        let (reply, mut receiver) = oneshot::channel();
        let execution = TerminalExecutionRequest {
            mcp_owned: matches!(
                request.caller,
                crate::features::capability_runtime::CapabilityCaller::Mcp
            ),
            async_job: true,
            return_job: true,
            operation_id: Some(operation_id.clone()),
            id: request.request_id.clone(),
            owner: request.connection_id.clone(),
            source: request.client.clone(),
            session_id: session_id.clone(),
            command: args.command.clone(),
            timeout: Duration::from_millis(args.timeout_ms.unwrap_or(300_000)),
            cancellation: CancellationToken::new(),
            policy: TerminalExecutionPolicy {
                background: false,
                max_lines: match self.ai.settings_config().external_mcp.terminal_display_mode {
                    nyaterm_core::McpTerminalDisplayMode::Inline => {
                        self.ai.settings_config().terminal_output_lines
                    }
                    nyaterm_core::McpTerminalDisplayMode::Silent => 0,
                },
            },
            track_insertion: true,
            reply,
        };
        let id = if args.backend == TerminalBackend::InlinePty {
            self.start_terminal_execution(execution, cx);
            if let Ok(result) = receiver.try_recv() {
                result?;
            }
            let task = self
                .mcp
                .capabilities
                .terminal_executions
                .tasks
                .get(&session_id)
                .filter(|task| task.request.id == request.request_id)
                .ok_or_else(|| {
                    rpc_failure("execution_failed", "The job could not be submitted.")
                })?;
            task.capture_marker_id.clone()
        } else {
            let config = self
                .session
                .metadata(&session_id)
                .and_then(|m| match &m.launch_config {
                    SessionLaunchConfig::Ssh(config) => Some(config.as_ref().clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    rpc_failure(
                        "invalid_argument",
                        "ssh_exec requires an SSH terminal session.",
                    )
                })?;
            self.validate_ai_background_execution(&session_id)
                .map_err(|_| {
                    rpc_failure(
                        "permission_denied",
                        "Independent execution is disabled for this session.",
                    )
                })?;
            let multiplex = self.session.ssh_multiplex_handle_for_session(&session_id);
            let runtime = cx
                .try_global::<crate::app_shell::DesktopControllerGlobal>()
                .and_then(|g| g.0.read(cx).ai_runtime.clone().ok())
                .ok_or_else(|| {
                    rpc_failure("host_unavailable", "The task runtime is unavailable.")
                })?;
            let (tx, rx) = mpsc::channel(64);
            let mut job = TerminalJob::from_request(&execution);
            job.backend = TerminalBackend::SshExec;
            let interrupt = job.interrupt.clone();
            job.controller = Some(runtime.spawn(
                CancellationToken::new(),
                stream_ssh_command(
                    config,
                    multiplex,
                    args.command.into_bytes(),
                    execution.timeout,
                    interrupt,
                    tx,
                ),
            ));
            job.receiver = Some(rx);
            self.mcp
                .capabilities
                .terminal_executions
                .jobs
                .insert(operation_id.clone(), job);
            self.record_automated_terminal_command(&session_id, &execution.command);
            self.mcp
                .capabilities
                .terminal_executions
                .inserted_requests
                .insert(request.request_id.clone());
            operation_id
        };
        let job = self
            .mcp
            .capabilities
            .terminal_executions
            .jobs
            .get_mut(&id)
            .unwrap();
        job.window_label = self.mcp.owner_window_label.clone();
        let status = job.status;
        self.audit_terminal_job_lifecycle(&session_id, "mcp.job_registered", cx);
        let result = to_value(TerminalStartResult {
            job_id: id,
            session_id,
            backend: args.backend,
            status,
        });
        self.ensure_terminal_execution_clock(cx);
        result
    }
    pub(in crate::features) fn dispatch_terminal_poll(&mut self, request: CapabilityRequest) {
        let args: TerminalPollArgs =
            serde_json::from_value(request.arguments.clone()).expect("validated poll");
        let result = self.terminal_poll_result(&request, args);
        let _ = request.reply.send(result);
    }
    fn terminal_poll_result(
        &self,
        request: &CapabilityRequest,
        args: TerminalPollArgs,
    ) -> Result<Value, nyaterm_mcp_protocol::RpcError> {
        let session_id =
            self.authorize_terminal_job(&request.connection_id, &request.scope, &args.job_id)?;
        let state = &self.mcp.capabilities.terminal_executions;
        let job = &state.jobs[&args.job_id];
        let exists = self.session.has_session(&session_id);
        let connected = exists && !self.session.is_disconnected(&session_id);
        let terminal_state = if connected {
            state.availability(&session_id)
        } else {
            TerminalAvailability::Disconnected
        };
        // JSON escaping counts toward the inline limit; shrink only the page, never its metadata.
        let mut budget = args.max_bytes.unwrap_or(16 * 1024);
        loop {
            let page = job
                .output
                .read(args.offset.unwrap_or(0), budget)
                .map_err(|_| rpc_failure("invalid_argument", "Output offset is invalid."))?;
            let value = to_value(TerminalPollResult {
                job_id: args.job_id.clone(),
                session_id: session_id.clone(),
                backend: job.backend,
                status: job.status,
                process_state: job.process_state,
                terminal_state,
                session_exists: exists,
                connected,
                exit_code: job.exit_code,
                started_at_ms: job.started_at_ms,
                ended_at_ms: job.ended_at_ms,
                output: page.data,
                offset: page.offset,
                next_offset: page.next_offset,
                base_offset: page.base_offset,
                total_bytes: page.total_bytes,
                eof: page.eof,
                output_complete: job.output_complete,
                source_truncated: page.source_truncated,
                dropped_before_offset: page.dropped_before_offset,
                stop_state: job.stop_state.clone(),
            })?;
            if serde_json::to_vec(&value).unwrap().len()
                <= nyaterm_mcp_protocol::MAX_INLINE_OUTPUT_BYTES
            {
                return Ok(value);
            }
            budget = (budget / 2).max(4);
        }
    }
    pub(in crate::features) fn dispatch_terminal_stop(
        &mut self,
        request: CapabilityRequest,
        cx: &mut Context<Self>,
    ) {
        let args: TerminalStopArgs =
            serde_json::from_value(request.arguments.clone()).expect("validated stop");
        let result = self.stop_terminal_job(&request, &args.job_id, cx);
        let _ = request.reply.send(result);
    }
    fn stop_terminal_job(
        &mut self,
        request: &CapabilityRequest,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, nyaterm_mcp_protocol::RpcError> {
        let session_id = self.authorize_terminal_job(&request.connection_id, &request.scope, id)?;
        if self.security.screen_locked() {
            return Err(rpc_failure("permission_denied", "Screen is locked."));
        }
        let connected =
            self.session.has_session(&session_id) && !self.session.is_disconnected(&session_id);
        let state = &mut self.mcp.capabilities.terminal_executions;
        let job = state.jobs.get(id).unwrap();
        let send = connected
            && job.stop_started.is_none()
            && job.process_state != TerminalProcessState::ExitConfirmed
            && job.finished.is_none();
        let inline = job.backend == TerminalBackend::InlinePty;
        let safe = !job.manual_takeover
            && state.tasks.get(&session_id).is_some_and(|task| {
                task.capture_marker_id == id
                    && task.capture.as_ref().is_some_and(|c| c.marker_started(id))
            })
            && !state.recovery_required(&session_id);
        if !connected {
            let job = state.jobs.get_mut(id).unwrap();
            job.stop_started.get_or_insert_with(Instant::now);
            job.stop_state = "target_invalid".into();
        } else if job.process_state == TerminalProcessState::ExitConfirmed {
            let job = state.jobs.get_mut(id).unwrap();
            job.stop_started.get_or_insert_with(Instant::now);
            job.stop_state = "confirmed_ended".into();
        } else if job.finished.is_some() && job.stop_started.is_none() {
            let job = state.jobs.get_mut(id).unwrap();
            job.stop_started = Some(Instant::now());
            job.stop_state = if inline {
                "manual_recovery_required"
            } else {
                "observer_unavailable"
            }
            .into();
        }
        if send {
            let job = state.jobs.get_mut(id).unwrap();
            job.stop_started = Some(Instant::now());
            if inline && !safe {
                job.stop_state = "manual_recovery_required".into();
                job.unknown(TerminalJobStatus::Unknown);
            } else {
                job.status = TerminalJobStatus::Cancelling;
                job.stop_state = "accepted".into();
                if !inline {
                    job.interrupt.cancel();
                }
            }
            if inline && safe {
                let sent = self
                    .write_automated_terminal_input(&session_id, b"\x03", None)
                    .is_ok();
                self.mcp
                    .capabilities
                    .terminal_executions
                    .jobs
                    .get_mut(id)
                    .unwrap()
                    .stop_state = if sent { "sent" } else { "unconfirmed" }.into();
            }
        }
        if send
            && inline
            && safe
            && self.mcp.capabilities.terminal_executions.jobs[id].stop_state == "sent"
        {
            self.audit_terminal_job_lifecycle(&session_id, "mcp.interrupt_sent", cx);
        }
        if send && inline && !safe {
            self.finish_terminal_execution(
                &session_id,
                id,
                Err(rpc_failure(
                    "target_recovery_required",
                    "The command cannot be safely interrupted; recover the shell manually.",
                )),
                TerminalExecutionStatus::Failed,
                cx,
            );
        }
        self.poll_terminal_jobs(cx);
        let state = &self.mcp.capabilities.terminal_executions;
        let job = &state.jobs[id];
        to_value(TerminalStopResult {
            job_id: id.into(),
            status: job.status,
            stop_state: job.stop_state.clone(),
            process_state: job.process_state,
            terminal_state: if !self.session.has_session(&session_id)
                || self.session.is_disconnected(&session_id)
            {
                TerminalAvailability::Disconnected
            } else {
                state.availability(&session_id)
            },
        })
    }
    pub(super) fn poll_terminal_jobs(&mut self, cx: &mut Context<Self>) {
        let previous = self
            .mcp
            .capabilities
            .terminal_executions
            .jobs
            .iter()
            .map(|(id, job)| {
                (
                    id.clone(),
                    (
                        job.session_id.clone(),
                        job.stop_state.clone(),
                        job.process_state,
                    ),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        let now = Instant::now();
        let state = &mut self.mcp.capabilities.terminal_executions;
        for job in state.jobs.values_mut() {
            if job
                .stop_started
                .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(2))
                && matches!(job.stop_state.as_str(), "accepted" | "sent")
            {
                job.stop_state = "unconfirmed".into();
                if job.status == TerminalJobStatus::Cancelling {
                    job.status = TerminalJobStatus::Unknown;
                }
            }
            if job
                .controller
                .as_ref()
                .is_some_and(|task| task.is_finished())
                && job.receiver.is_none()
            {
                job.controller = None;
            }
            if job.receiver.is_none() {
                continue;
            }
            let mut events = Vec::new();
            let receiver = job.receiver.as_mut().unwrap();
            // Bound work per UI tick while keeping every receiver progressing.
            for _ in 0..8 {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(_) => break,
                }
            }
            for event in events {
                match event {
                    SshExecEvent::Started => {
                        if job.finished.is_none() {
                            if job.stop_started.is_none() {
                                job.status = TerminalJobStatus::Running;
                            }
                            job.process_state = TerminalProcessState::Active;
                        }
                    }
                    SshExecEvent::Output { data, stderr } => {
                        let text = decode_utf8(
                            if stderr {
                                &mut job.stderr_tail
                            } else {
                                &mut job.stdout_tail
                            },
                            &data,
                            false,
                        );
                        job.append(&text);
                    }
                    SshExecEvent::OutputDropped { bytes: _ } => job.output.note_source_loss(),
                    SshExecEvent::Eof | SshExecEvent::Closed => {}
                    SshExecEvent::InterruptSent => job.stop_state = "sent".into(),
                    SshExecEvent::ExitStatus(code) => {
                        job.remote_failed = code != 0;
                        job.exit_code = i32::try_from(code).ok();
                        job.process_state = TerminalProcessState::ExitConfirmed;
                    }
                    SshExecEvent::ExitSignal { interrupted } => {
                        job.remote_failed = true;
                        job.process_state = TerminalProcessState::ExitConfirmed;
                        if interrupted {
                            job.stop_state = "exit_signal_observed".into();
                        }
                    }
                    SshExecEvent::Finished {
                        confirmed,
                        interrupted,
                    } => {
                        let stdout = decode_utf8(&mut job.stdout_tail, &[], true);
                        let stderr = decode_utf8(&mut job.stderr_tail, &[], true);
                        job.append(&stdout);
                        job.append(&stderr);
                        if confirmed {
                            job.exited(job.exit_code, interrupted);
                        } else {
                            job.unknown(TerminalJobStatus::Unknown);
                        }
                    }
                    SshExecEvent::TimedOut => job.unknown(TerminalJobStatus::TimedOut),
                    SshExecEvent::Failed => job.unknown(TerminalJobStatus::Failed),
                    SshExecEvent::CancelledBeforeStart => {
                        job.unknown(TerminalJobStatus::Cancelled);
                        job.process_state = TerminalProcessState::NotStarted;
                        if job.stop_started.is_some() {
                            job.stop_state = "confirmed_ended".into();
                        }
                    }
                }
            }
            if job
                .receiver
                .as_ref()
                .is_some_and(|rx| rx.is_closed() && rx.is_empty())
                && job.finished.is_none()
            {
                job.unknown(TerminalJobStatus::Unknown);
            }
            if job.finished.is_some()
                && job
                    .receiver
                    .as_ref()
                    .is_some_and(|rx| rx.is_closed() && rx.is_empty())
            {
                job.output_complete = true;
                job.receiver = None;
                if job
                    .controller
                    .as_ref()
                    .is_some_and(|task| task.is_finished())
                {
                    job.controller = None;
                }
            }
        }
        for job in state.jobs.values() {
            if let Some(operation) = &job.operation_id {
                self.mcp
                    .capabilities
                    .ledger
                    .pin(operation, job.finished.is_none());
            }
        }
        let owners = state
            .jobs
            .values()
            .map(|j| j.owner.clone())
            .collect::<Vec<_>>();
        for owner in owners {
            state.enforce_output_limits(&owner);
        }
        state.jobs.retain(|_, job| {
            !job.finished
                .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(600))
        });
        self.mcp.capabilities.ledger.cleanup();
        let mut actions = Vec::new();
        for (id, (session, stop, process)) in previous {
            if let Some(job) = self
                .mcp
                .capabilities
                .terminal_executions
                .jobs
                .get(&id)
                .filter(|job| job.mcp_owned)
            {
                if job.stop_state == "sent" && stop != "sent" {
                    actions.push((session.clone(), "mcp.interrupt_sent"));
                }
                if job.process_state == TerminalProcessState::ExitConfirmed
                    && process != TerminalProcessState::ExitConfirmed
                {
                    actions.push((session, "mcp.remote_exit_confirmed"));
                }
            }
        }
        for (session, action) in actions {
            self.audit_terminal_job_lifecycle(&session, action, cx);
        }
    }
    pub(super) fn audit_terminal_job_lifecycle(
        &mut self,
        session: &str,
        action: &str,
        cx: &mut Context<Self>,
    ) {
        self.persist_mcp_audit(
            nyaterm_core::ai::AppendAiAuditRequest {
                action: action.into(),
                source: Some("mcp".into()),
                session_id: Some(session.into()),
                capability: Some("terminal_job".into()),
                ..Default::default()
            },
            cx,
        );
    }
}
fn decode_utf8(tail: &mut Vec<u8>, bytes: &[u8], flush: bool) -> String {
    tail.extend_from_slice(bytes);
    let mut result = String::new();
    let mut offset = 0;
    while offset < tail.len() {
        match std::str::from_utf8(&tail[offset..]) {
            Ok(text) => {
                result.push_str(text);
                offset = tail.len();
            }
            Err(error) => {
                let end = offset + error.valid_up_to();
                result.push_str(std::str::from_utf8(&tail[offset..end]).unwrap());
                offset = end;
                if let Some(len) = error.error_len() {
                    result.push('\u{fffd}');
                    offset += len;
                } else if flush {
                    result.push('\u{fffd}');
                    offset = tail.len();
                } else {
                    break;
                }
            }
        }
    }
    tail.drain(..offset);
    result
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalExecutionPolicy, TerminalExecutionRequest, TerminalExecutionState, TerminalJob,
        decode_utf8,
    };
    use nyaterm_mcp_protocol::{TerminalJobStatus, TerminalProcessState};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;
    fn job(owner: &str) -> TerminalJob {
        let (reply, _) = tokio::sync::oneshot::channel();
        TerminalJob::from_request(&TerminalExecutionRequest {
            mcp_owned: true,
            id: "fixture".into(),
            owner: owner.into(),
            source: "fixture".into(),
            session_id: "closed-target".into(),
            command: String::new(),
            async_job: true,
            return_job: true,
            operation_id: None,
            timeout: Duration::from_secs(300),
            cancellation: CancellationToken::new(),
            policy: TerminalExecutionPolicy {
                background: false,
                max_lines: 0,
            },
            track_insertion: false,
            reply,
        })
    }
    #[test]
    fn utf8_split_events_preserve_characters_and_flush_invalid_bytes() {
        let mut tail = Vec::new();
        let bytes = "中文😀".as_bytes();
        let mut output = String::new();
        for byte in bytes {
            output.push_str(&decode_utf8(&mut tail, &[*byte], false));
        }
        assert_eq!(output, "中文😀");
        assert!(tail.is_empty());
        assert_eq!(decode_utf8(&mut tail, &[0xf0, 0x9f], true), "�");
    }
    #[test]
    fn late_exit_supplements_timeout_without_rewriting_request_outcome() {
        let mut job = job("owner");
        job.unknown(TerminalJobStatus::TimedOut);
        job.stop_started = Some(std::time::Instant::now());
        job.exited(Some(0), false);
        assert_eq!(job.status, TerminalJobStatus::TimedOut);
        assert_eq!(job.exit_code, Some(0));
        assert_eq!(job.process_state, TerminalProcessState::ExitConfirmed);
        assert_eq!(job.stop_state, "confirmed_ended");
    }
    #[test]
    fn global_active_capacity_releases_only_when_an_operation_finishes() {
        let mut state = TerminalExecutionState::default();
        for index in 0..32 {
            state
                .jobs
                .insert(index.to_string(), job(&format!("owner-{}", index / 8)));
        }
        assert!(!state.can_register_job("new-owner"));
        state.jobs.get_mut("0").unwrap().exited(Some(0), false);
        assert!(state.can_register_job("new-owner"));
        assert!(state.can_register_job("owner-0"));
        assert!(!state.can_register_job("owner-1"));
    }
    #[test]
    fn per_owner_active_capacity_is_independent_and_output_eviction_keeps_absolute_cursors() {
        let mut state = TerminalExecutionState::default();
        for index in 0..8 {
            state.jobs.insert(index.to_string(), job("owner"));
        }
        assert!(!state.can_register_job("owner"));
        assert!(state.can_register_job("other"));
        let text = "x".repeat(4 * 1024 * 1024);
        for index in 0..8 {
            state.append_job_output(&index.to_string(), &text);
        }
        let retained: usize = state
            .jobs
            .values()
            .map(|job| job.output.retained_bytes())
            .sum();
        assert!(retained <= 16 * 1024 * 1024);
        assert!(
            state
                .jobs
                .values()
                .any(|job| job.output.read(0, 4).unwrap().dropped_before_offset)
        );
    }
    #[gpui::test]
    fn cancelled_before_ssh_exec_confirms_stop_and_expires_without_polling(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::TestConfigDir;
        use gpui::AppContext as _;
        use nyaterm_mcp_protocol::TerminalBackend;
        use nyaterm_transport::ssh_execution::streaming::SshExecEvent;
        let directory = TestConfigDir::new("nyaterm-job-cancel-before-exec");
        let app = app_with_visible_local_session(cx, directory.path(), "fixture");
        cx.update_entity(&app, |app, cx| {
            let mut job = job("owner");
            job.backend = TerminalBackend::SshExec;
            job.session_id = "fixture".into();
            job.stop_started = Some(std::time::Instant::now());
            job.stop_state = "accepted".into();
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            job.receiver = Some(receiver);
            sender.try_send(SshExecEvent::CancelledBeforeStart).unwrap();
            drop(sender);
            app.mcp
                .capabilities
                .terminal_executions
                .jobs
                .insert("job".into(), job);
            app.poll_terminal_jobs(cx);
            let job = &app.mcp.capabilities.terminal_executions.jobs["job"];
            assert_eq!(job.status, TerminalJobStatus::Cancelled);
            assert_eq!(job.process_state, TerminalProcessState::NotStarted);
            assert_eq!(job.stop_state, "confirmed_ended");
            assert!(job.output_complete);
            assert!(job.receiver.is_none());
            app.mcp
                .capabilities
                .terminal_executions
                .jobs
                .get_mut("job")
                .unwrap()
                .finished = Some(std::time::Instant::now() - Duration::from_secs(601));
            // This is the periodic clock path; no terminal_poll invocation is required.
            app.poll_terminal_jobs(cx);
            assert!(
                !app.mcp
                    .capabilities
                    .terminal_executions
                    .jobs
                    .contains_key("job")
            );
        });
    }
    #[gpui::test]
    fn retained_closed_job_checks_owner_and_original_scope(cx: &mut gpui::TestAppContext) {
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::TestConfigDir;
        use gpui::AppContext as _;
        use nyaterm_core::capabilities::CapabilityScope;
        let directory = TestConfigDir::new("nyaterm-job-authorization");
        let app = app_with_visible_local_session(cx, directory.path(), "fixture");
        cx.update_entity(&app, |app, _| {
            let mut job = job("owner");
            job.window_label = "original-window".into();
            job.exited(Some(0), false);
            app.mcp
                .capabilities
                .terminal_executions
                .jobs
                .insert("job".into(), job);
            assert!(
                app.authorize_terminal_job("foreign", &CapabilityScope::AllSessions, "job")
                    .is_err()
            );
            assert!(
                app.authorize_terminal_job(
                    "owner",
                    &CapabilityScope::CurrentWindow {
                        owner_window_label: "other-window".into()
                    },
                    "job"
                )
                .is_err()
            );
            assert!(
                app.authorize_terminal_job(
                    "owner",
                    &CapabilityScope::CurrentWindow {
                        owner_window_label: "original-window".into()
                    },
                    "job"
                )
                .is_ok()
            );
            assert!(
                app.authorize_terminal_job(
                    "owner",
                    &CapabilityScope::Explicit {
                        session_ids: std::collections::HashSet::from(["different-target".into()]),
                        default_session_id: None
                    },
                    "job"
                )
                .is_err()
            );
        });
    }
}
