use super::{CapabilityCaller, CapabilityRequest};
use crate::features::NyaTermApp;
use crate::features::mcp::server::rpc_failure;
use crate::features::runtime_jobs::await_blocking_result;
use crate::features::terminal::ai_execution::{TerminalExecutionEvent, TerminalExecutionStatus};
use crate::models::SessionLaunchConfig;
use crate::models::recording::ExecutionRecordingEvent;
use gpui::Context;
use nyaterm_core::McpTerminalDisplayMode;
use nyaterm_core::agent_capture::build_agent_capture_command;
use nyaterm_core::models::connection::AiExecutionProfile;
use nyaterm_core::raw_cli_capture::{RawCliCapture, RawCliEnd};
use nyaterm_mcp_protocol::{RpcError, TerminalExecuteArgs, TerminalExecuteResult};
use nyaterm_remote_ops::remote_process::SshProcessService;
use nyaterm_terminal::agent_capture::{
    AgentCaptureProcessResult, AgentOutputCaptureProcessor, RetiredCaptureFilter,
};
use nyaterm_transport::ssh_execution::run_local_command;
use nyaterm_transport::{SessionKind, SshSessionConfig};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

enum BackgroundTerminalTarget {
    Ssh(Box<SshSessionConfig>),
    Local { working_dir: Option<PathBuf> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TerminalExecutionPolicy {
    pub background: bool,
    pub max_lines: u16,
}

fn external_mcp_terminal_policy(
    integrated: Option<TerminalExecutionPolicy>,
    display_mode: McpTerminalDisplayMode,
    terminal_output_lines: u16,
) -> TerminalExecutionPolicy {
    integrated.unwrap_or(TerminalExecutionPolicy {
        background: false,
        max_lines: match display_mode {
            McpTerminalDisplayMode::Inline => terminal_output_lines,
            McpTerminalDisplayMode::Silent => 0,
        },
    })
}

pub(in crate::features) struct TerminalExecutionRequest {
    pub mcp_owned: bool,
    pub async_job: bool,
    pub return_job: bool,
    pub operation_id: Option<String>,
    pub id: String,
    pub owner: String,
    pub source: String,
    pub session_id: String,
    pub command: String,
    pub timeout: Duration,
    pub cancellation: CancellationToken,
    pub policy: TerminalExecutionPolicy,
    pub track_insertion: bool,
    pub reply: oneshot::Sender<Result<Value, RpcError>>,
}

struct TerminalExecutionTask {
    request: TerminalExecutionRequest,
    step_index: u16,
    started: Instant,
    capture: Option<AgentOutputCaptureProcessor>,
    raw_capture: Option<RawCliCapture>,
    capture_started: bool,
    capture_marker_id: String,
    worker_cancellation: CancellationToken,
}

struct TerminalExecutionWaiter {
    cancellation: CancellationToken,
    reply: oneshot::Sender<Result<Value, RpcError>>,
}
struct TerminalExecutionCompletion {
    event: Option<TerminalExecutionEvent>,
    reply: oneshot::Sender<Result<Value, RpcError>>,
    result: Result<Value, RpcError>,
    recording_tail: String,
    recording_boundary: bool,
    operation_id: Option<String>,
}

struct TerminalRecoveryGuard {
    marker_id: String,
    owner: String,
    mcp_owned: bool,
}

/// Owns every automatic terminal execution, independently of the visible AI scope.
#[derive(Default)]
pub(in crate::features) struct TerminalExecutionState {
    tasks: HashMap<String, TerminalExecutionTask>,
    next_steps: HashMap<String, u16>,
    inserted_requests: HashSet<String>,
    // Consume the prompt following an END marker, even after its reply was sent.
    draining: HashMap<String, (AgentOutputCaptureProcessor, Instant)>,
    retired: HashMap<String, Vec<RetiredCaptureFilter>>,
    recovery: HashMap<String, TerminalRecoveryGuard>,
    jobs: HashMap<String, jobs::TerminalJob>,
    waiters: HashMap<String, Vec<TerminalExecutionWaiter>>,
    clock_armed: bool,
    background_closed: bool,
}

impl TerminalExecutionState {
    pub fn busy(&self, session_id: &str) -> bool {
        self.tasks.contains_key(session_id) || self.recovery.contains_key(session_id)
    }

    pub fn recovery_required(&self, session_id: &str) -> bool {
        self.recovery.contains_key(session_id)
    }

    pub fn confirm_recovered(&mut self, session_id: &str) -> bool {
        self.recovery.remove(session_id).is_some()
    }

    pub fn capturing(&self, session_id: &str) -> bool {
        self.tasks
            .get(session_id)
            .is_some_and(|task| task.capture.is_some() || task.raw_capture.is_some())
            || self.draining.contains_key(session_id)
    }

    pub fn intercepting(&self, session_id: &str) -> bool {
        self.capturing(session_id) || self.retired.contains_key(session_id)
    }

    pub fn begin_capture(&mut self, session_id: &str, id: &str) -> Option<TerminalExecutionEvent> {
        let task = self.tasks.get_mut(session_id)?;
        if task.capture_marker_id != id
            || task.capture_started
            || (task.capture.is_none() && task.raw_capture.is_none())
        {
            return None;
        }
        task.capture_started = true;
        if let Some((previous, _)) = self.draining.remove(session_id)
            && let Some(capture) = task.capture.as_mut()
        {
            capture.continue_control_sequences_from(previous);
        }
        Some(TerminalExecutionEvent::Started {
            id: id.into(),
            session_id: session_id.into(),
            source: task.request.source.clone(),
            command: task.request.command.clone(),
            step_index: task.step_index,
            max_lines: task.request.policy.max_lines,
        })
    }

    pub fn process_output(&mut self, session_id: &str, text: &str) -> AgentCaptureProcessResult {
        let mut text = text.to_string();
        let mut late_ends = Vec::new();
        let mut late_output = Vec::new();
        if let Some(filters) = self.retired.get_mut(session_id) {
            filters.retain_mut(|filter| {
                if !self
                    .recovery
                    .get(session_id)
                    .is_some_and(|guard| guard.marker_id == filter.marker_id())
                    || !self
                        .jobs
                        .get(filter.marker_id())
                        .is_some_and(|job| !job.manual_takeover)
                {
                    filter.disable_observation();
                }
                let (visible, finished) = filter.process(&text);
                text = visible;
                let observed = filter.take_observed_output();
                if self
                    .recovery
                    .get(session_id)
                    .is_some_and(|guard| guard.marker_id == filter.marker_id())
                    && self
                        .jobs
                        .get(filter.marker_id())
                        .is_some_and(|job| !job.manual_takeover)
                {
                    late_output.push((filter.marker_id().to_owned(), observed));
                }
                if let Some((id, _)) = filter.completed_marker()
                    && self
                        .recovery
                        .get(session_id)
                        .is_some_and(|guard| guard.marker_id == id)
                {
                    self.recovery.remove(session_id);
                    late_ends.push((id.to_string(), filter.completed_marker().unwrap().1));
                }
                !finished
            });
            if filters.is_empty() {
                self.retired.remove(session_id);
            }
        }
        for (id, output) in late_output {
            self.append_job_output(&id, &output);
        }
        for (id, code) in late_ends {
            self.observe_late_job_end(&id, code);
        }
        if let Some(raw) = self
            .tasks
            .get_mut(session_id)
            .filter(|task| task.capture_started)
            .and_then(|task| task.raw_capture.as_mut())
        {
            raw.feed(&text, Instant::now());
            return AgentCaptureProcessResult {
                visible_text: text,
                recording_text: String::new(),
                completed: Vec::new(),
            };
        }
        let capture_id = self
            .tasks
            .get(session_id)
            .map(|task| task.capture_marker_id.clone());
        if let Some(capture) = self
            .tasks
            .get_mut(session_id)
            .filter(|task| task.capture_started)
            .and_then(|task| task.capture.as_mut())
        {
            let result = capture.process(&text);
            let started = capture.marker_started(capture_id.as_deref().unwrap());
            let id = capture_id.unwrap();
            if started && let Some(job) = self.jobs.get_mut(&id) {
                job.process_state = nyaterm_mcp_protocol::TerminalProcessState::Active;
                if job.stop_started.is_none() {
                    job.status = nyaterm_mcp_protocol::TerminalJobStatus::Running;
                }
            }
            self.append_job_output(&id, &result.recording_text);
            return result;
        }
        if let Some((capture, _)) = self.draining.get_mut(session_id) {
            let result = capture.process(&text);
            if !capture.has_active() {
                self.draining.remove(session_id);
            }
            return result;
        }
        AgentCaptureProcessResult {
            visible_text: text,
            recording_text: String::new(),
            completed: Vec::new(),
        }
    }

    fn insert(
        &mut self,
        request: TerminalExecutionRequest,
        capture: Option<AgentOutputCaptureProcessor>,
    ) -> Result<TerminalExecutionEvent, Box<TerminalExecutionRequest>> {
        if self.busy(&request.session_id) {
            return Err(Box::new(request));
        }
        self.jobs.insert(
            request.id.clone(),
            jobs::TerminalJob::from_request(&request),
        );
        let step = self.next_steps.entry(request.owner.clone()).or_default();
        let step_index = *step;
        *step = step.saturating_add(1);
        let event = TerminalExecutionEvent::Started {
            id: request.id.clone(),
            session_id: request.session_id.clone(),
            source: request.source.clone(),
            command: request.command.clone(),
            step_index,
            max_lines: request.policy.max_lines,
        };
        self.tasks.insert(
            request.session_id.clone(),
            TerminalExecutionTask {
                worker_cancellation: request.cancellation.child_token(),
                capture_marker_id: request.id.clone(),
                request,
                step_index,
                started: Instant::now(),
                capture,
                raw_capture: None,
                capture_started: false,
            },
        );
        Ok(event)
    }

    pub fn execution_id_for_capture(&self, session_id: &str, marker_id: &str) -> Option<String> {
        self.tasks
            .get(session_id)
            .filter(|task| task.capture_marker_id == marker_id)
            .map(|task| task.capture_marker_id.clone())
    }

    pub fn take_inserted(&mut self, id: &str) -> bool {
        self.inserted_requests.remove(id)
    }

    fn finish(
        &mut self,
        session_id: &str,
        id: &str,
        mut result: Result<TerminalExecuteResult, RpcError>,
        mut status: TerminalExecutionStatus,
    ) -> Option<TerminalExecutionCompletion> {
        let mut event = None;
        if !self
            .tasks
            .get(session_id)
            .is_some_and(|task| task.capture_marker_id == id)
        {
            return None;
        }
        let mut task = self.tasks.remove(session_id).expect("matched task");
        // Ending the waiter cannot erase exit evidence already received from the worker.
        let confirmed_exit_code = (status == TerminalExecutionStatus::Completed)
            .then(|| result.as_ref().ok().and_then(|result| result.exit_code))
            .flatten();
        let recording_boundary = task.request.policy.background || task.capture.is_some();
        let mut recording_tail = String::new();
        let duration_ms = task
            .started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        if let Ok(result) = &mut result {
            result.duration_ms = duration_ms;
        }
        if task.request.cancellation.is_cancelled() {
            status = TerminalExecutionStatus::Cancelled;
            result = Err(rpc_failure(
                "cancelled",
                if confirmed_exit_code.is_some() {
                    "The request was cancelled after the command's exit was confirmed."
                } else {
                    "Stopped waiting for the command; it may still be running."
                },
            ));
        }
        if result.is_err() && status == TerminalExecutionStatus::Completed {
            status = TerminalExecutionStatus::Failed;
        }
        if !task.request.policy.background
            && confirmed_exit_code.is_none()
            && !matches!(
                status,
                TerminalExecutionStatus::Completed | TerminalExecutionStatus::Disconnected
            )
        {
            self.recovery.insert(
                session_id.into(),
                TerminalRecoveryGuard {
                    marker_id: task.capture_marker_id.clone(),
                    owner: task.request.owner.clone(),
                    mcp_owned: task.request.mcp_owned,
                },
            );
        }
        if task.capture.is_some() || task.raw_capture.is_some() {
            let mut partial_output = String::new();
            let mut partial_truncated = false;
            if let Some(raw) = task.raw_capture.take() {
                partial_truncated = raw.is_truncated();
                let output = raw.into_output();
                match &mut result {
                    Ok(value) => {
                        // The caller supplies the completion explanation.
                        value.output = format!("{output}\n{}", value.output);
                        value.source_truncated |= partial_truncated;
                    }
                    Err(_) => {
                        partial_output =
                            nyaterm_terminal::recording_sanitizer::RecordingSanitizer::default()
                                .feed(&output)
                    }
                }
            }
            if let Some(mut capture) = task.capture.take() {
                if status == TerminalExecutionStatus::Completed && capture.has_active() {
                    self.draining.insert(
                        session_id.into(),
                        (capture, Instant::now() + Duration::from_millis(150)),
                    );
                } else if status != TerminalExecutionStatus::Completed {
                    let filter = capture.retire(&task.capture_marker_id);
                    let filters = self.retired.entry(session_id.into()).or_default();
                    // The newest observer always carries the current recovery guard.
                    if filters.len() == 8 {
                        filters.remove(0);
                    }
                    filters.push(filter);
                    if let Some(partial) = capture.cancel(&task.capture_marker_id) {
                        recording_tail = partial.recording_text;
                        partial_truncated = partial.source_truncated;
                        match &mut result {
                            Ok(value) => {
                                value.output = partial.output;
                                value.source_truncated |= partial.source_truncated;
                            }
                            Err(_) => {
                                partial_output =
                                nyaterm_terminal::recording_sanitizer::RecordingSanitizer::default(
                                )
                                .feed(&partial.output);
                            }
                        }
                    }
                }
            }
            if let Ok(value) = &mut result {
                value.output = nyaterm_terminal::recording_sanitizer::RecordingSanitizer::default()
                    .feed(&value.output);
            }
            let (output, exit_code, truncated) = match &result {
                Ok(value) => (
                    value.output.clone(),
                    value.exit_code,
                    value.source_truncated,
                ),
                Err(error) => {
                    if !partial_output.is_empty() {
                        partial_output.push('\n');
                    }
                    partial_output.push_str(&error.message);
                    (partial_output, None, partial_truncated)
                }
            };
            if task.capture_started {
                event = Some(TerminalExecutionEvent::Finished {
                    id: id.into(),
                    session_id: session_id.into(),
                    source: task.request.source.clone(),
                    command: task.request.command.clone(),
                    step_index: task.step_index,
                    max_lines: task.request.policy.max_lines,
                    output,
                    exit_code,
                    duration_ms,
                    source_truncated: truncated,
                    status,
                });
            }
        }
        self.append_job_output(id, &recording_tail);
        self.finish_job(id, status, result.as_ref().ok(), &task.request);
        if status == TerminalExecutionStatus::Cancelled
            && let Some(code) = confirmed_exit_code
        {
            self.observe_late_job_end(id, code);
        }
        if task.request.return_job
            && let Ok(value) = &mut result
        {
            value.job_id = Some(id.into());
        }
        // Stop queued background work without changing the caller's outcome.
        task.worker_cancellation.cancel();
        Some(TerminalExecutionCompletion {
            operation_id: if task.request.async_job {
                None
            } else {
                task.request.operation_id.clone()
            },
            event,
            recording_tail,
            recording_boundary,
            reply: task.request.reply,
            result: result.and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|_| rpc_failure("internal_error", "Cannot encode terminal result."))
            }),
        })
    }

    fn complete_waiters(&mut self, operation: &str, result: &Result<Value, RpcError>) {
        if let Some(waiters) = self.waiters.remove(operation) {
            for waiter in waiters {
                if !waiter.cancellation.is_cancelled() {
                    let _ = waiter.reply.send(result.clone());
                }
            }
        }
    }
    fn prune_waiters(&mut self) {
        for waiters in self.waiters.values_mut() {
            waiters.retain(|waiter| !waiter.cancellation.is_cancelled());
        }
    }
    pub fn forget_owner(&mut self, owner: &str) {
        self.next_steps.remove(owner);
        for task in self.tasks.values() {
            if task.request.owner == owner
                && task.request.operation_id.is_none()
                && !task.request.async_job
            {
                task.request.cancellation.cancel();
            }
        }
    }

    fn expire_draining(&mut self, now: Instant) -> Vec<String> {
        let expired = self
            .draining
            .iter()
            .filter_map(|(session, (capture, until))| {
                // A time limit cannot turn a split OSC title into printable text.
                (now >= *until && !capture.has_pending_control()).then_some(session.clone())
            })
            .collect::<Vec<_>>();
        for session in &expired {
            self.draining.remove(session);
        }
        expired
    }
}

impl NyaTermApp {
    pub(in crate::features) fn process_terminal_execution_output(
        &mut self,
        session_id: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> AgentCaptureProcessResult {
        let previous_guard = self
            .mcp
            .capabilities
            .terminal_executions
            .recovery
            .get(session_id)
            .map(|guard| (guard.marker_id.clone(), guard.mcp_owned));
        let result = self
            .mcp
            .capabilities
            .terminal_executions
            .process_output(session_id, text);
        if let Some((marker_id, mcp_owned)) = previous_guard {
            let still_guarded = self
                .mcp
                .capabilities
                .terminal_executions
                .recovery
                .get(session_id)
                .is_some_and(|guard| guard.marker_id == marker_id);
            if !still_guarded && mcp_owned {
                self.audit_terminal_job_lifecycle(session_id, "mcp.remote_exit_confirmed", cx);
            }
        }
        result
    }

    pub(in crate::features) fn dispatch_shared_terminal_execute(
        &mut self,
        request: CapabilityRequest,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        let mut args: TerminalExecuteArgs = serde_json::from_value(request.arguments.clone())
            .expect("validated terminal arguments");
        let policy = match &request.caller {
            CapabilityCaller::Native { .. } => {
                self.ai
                    .native_terminal_policy()
                    .unwrap_or(TerminalExecutionPolicy {
                        background: self.ai.run_settings().agent_background_execution_enabled,
                        max_lines: self.ai.run_settings().terminal_output_lines,
                    })
            }
            CapabilityCaller::Mcp => external_mcp_terminal_policy(
                self.mcp.terminal_policy(&request.generation),
                self.ai.settings_config().external_mcp.terminal_display_mode,
                self.ai.settings_config().terminal_output_lines,
            ),
        };
        let default_timeout = if matches!(&request.caller, CapabilityCaller::Native { .. }) {
            self.ai
                .run_settings()
                .agent_step_timeout_ms
                .unwrap_or(30_000)
        } else {
            30_000
        };
        let source = match &request.caller {
            CapabilityCaller::Native { .. } => "NyaTerm Agent".into(),
            CapabilityCaller::Mcp => request.client.clone(),
        };
        args.session_id = Some(session_id.clone());
        args.timeout_ms = Some(args.timeout_ms.unwrap_or(default_timeout));
        let canonical = serde_json::to_value(&args).expect("terminal arguments");
        let operation_id = match self.mcp.capabilities.ledger.begin(
            &request.connection_id,
            &request.tool,
            args.idempotency_key.as_deref(),
            &canonical,
        ) {
            Ok(super::idempotency::Lookup::Complete(result)) => {
                let _ = request.reply.send(result);
                return;
            }
            Ok(super::idempotency::Lookup::Pending(id)) => {
                if let Some(waiters) = self
                    .mcp
                    .capabilities
                    .terminal_executions
                    .waiters
                    .get_mut(&id)
                {
                    waiters.push(TerminalExecutionWaiter {
                        cancellation: request.cancellation,
                        reply: request.reply,
                    });
                } else {
                    let _ = request.reply.send(Err(rpc_failure(
                        "operation_pending",
                        "The original operation is still being observed.",
                    )));
                }
                return;
            }
            Ok(super::idempotency::Lookup::New(id)) => id,
            Err(error) => {
                let _ = request.reply.send(Err(error));
                return;
            }
        };
        let native = matches!(request.caller, CapabilityCaller::Native { .. });
        let (reply, cancellation) = if native {
            (request.reply, request.cancellation)
        } else {
            self.mcp.capabilities.terminal_executions.waiters.insert(
                operation_id.clone(),
                vec![TerminalExecutionWaiter {
                    cancellation: request.cancellation,
                    reply: request.reply,
                }],
            );
            let (reply, _) = oneshot::channel();
            (reply, CancellationToken::new())
        };
        self.ensure_terminal_execution_clock(cx);
        self.start_terminal_execution(
            TerminalExecutionRequest {
                mcp_owned: matches!(request.caller, CapabilityCaller::Mcp),
                async_job: false,
                return_job: args.return_job && request.extended_results,
                operation_id: Some(operation_id),
                id: request.request_id,
                owner: if native && self.mcp.terminal_policy(&request.generation).is_some() {
                    request.generation
                } else {
                    request.connection_id
                },
                source,
                track_insertion: true,
                session_id,
                command: args.command,
                timeout: Duration::from_millis(args.timeout_ms.unwrap_or(default_timeout)),
                cancellation,
                policy,
                reply,
            },
            cx,
        );
    }

    pub(in crate::features) fn start_terminal_execution(
        &mut self,
        request: TerminalExecutionRequest,
        cx: &mut Context<Self>,
    ) {
        let reject =
            |this: &mut Self, request: TerminalExecutionRequest, code: &str, message: &str| {
                let result = Err(rpc_failure(code, message));
                if let Some(id) = &request.operation_id {
                    this.mcp.capabilities.ledger.finish(id, result.clone());
                    this.mcp
                        .capabilities
                        .terminal_executions
                        .complete_waiters(id, &result);
                }
                let _ = request.reply.send(result);
            };
        if request.cancellation.is_cancelled() {
            reject(
                self,
                request,
                "cancelled",
                "Terminal execution was cancelled.",
            );
            return;
        }
        if self.security.screen_locked() {
            reject(self, request, "permission_denied", "Screen is locked.");
            return;
        }
        if self
            .mcp
            .capabilities
            .terminal_executions
            .busy(&request.session_id)
        {
            let recovering = self
                .mcp
                .capabilities
                .terminal_executions
                .recovery_required(&request.session_id);
            reject(
                self,
                request,
                if recovering {
                    "target_recovery_required"
                } else {
                    "target_busy"
                },
                if recovering {
                    "The previous command may still be running or waiting for input. Recover the shell and confirm it is ready in the terminal tab menu."
                } else {
                    "Another automatic command is using this terminal."
                },
            );
            return;
        }
        let Some(session) = self
            .session
            .session_info(&request.session_id)
            .filter(|_| !self.session.is_disconnected(&request.session_id))
        else {
            reject(
                self,
                request,
                "invalid_argument",
                "The target terminal session is unavailable.",
            );
            return;
        };
        if !self
            .mcp
            .capabilities
            .terminal_executions
            .can_register_job(&request.owner)
        {
            reject(
                self,
                request,
                "resource_limit",
                "Too many active or retained jobs.",
            );
            return;
        }
        let profile = self.ai_execution_profile_for_session(&request.session_id);
        if profile == AiExecutionProfile::Disabled {
            reject(
                self,
                request,
                "permission_denied",
                "Terminal execution is disabled for this session.",
            );
            return;
        }
        if request.policy.background
            && let Err(error) = self.validate_ai_background_execution(&request.session_id)
        {
            reject(self, request, "permission_denied", &error);
            return;
        }
        let target = if request.policy.background {
            match session.kind {
                SessionKind::Ssh => {
                    self.session
                        .metadata(&request.session_id)
                        .and_then(|metadata| match &metadata.launch_config {
                            SessionLaunchConfig::Ssh(config) => Some(
                                BackgroundTerminalTarget::Ssh(Box::new(config.as_ref().clone())),
                            ),
                            _ => None,
                        })
                }
                SessionKind::LocalPty => Some(BackgroundTerminalTarget::Local {
                    working_dir: session.working_dir,
                }),
                _ => None,
            }
        } else {
            None
        };
        if request.return_job && matches!(target, Some(BackgroundTerminalTarget::Local { .. })) {
            reject(
                self,
                request,
                "invalid_argument",
                "Legacy local background execution cannot expose an SSH channel job. Use terminal_start with inline_pty.",
            );
            return;
        }
        if request.policy.background && target.is_none() {
            reject(
                self,
                request,
                "permission_denied",
                "Background execution is supported only for SSH and local sessions.",
            );
            return;
        }
        // Caller-provided RPC IDs must never become shell syntax or predictable markers.
        let capture_marker_id = nyaterm_core::uuid();
        let wrapped = (!request.policy.background)
            .then(|| build_agent_capture_command(profile, &capture_marker_id, &request.command))
            .flatten();
        let async_job = request.async_job;
        let capture = wrapped.as_ref().map(|_| {
            let mut capture = AgentOutputCaptureProcessor::new();
            if async_job {
                capture.register_streaming(
                    capture_marker_id.clone(),
                    profile != AiExecutionProfile::Cmd,
                );
            } else {
                capture.register_with_end_newline(
                    capture_marker_id.clone(),
                    profile != AiExecutionProfile::Cmd,
                );
            }
            capture
        });
        let raw_capture = (profile == AiExecutionProfile::SendOnly).then(|| {
            let tail = self.terminal_buffer_tail_for_session(&request.session_id);
            let mut start = tail.len().saturating_sub(4096);
            while !tail.is_char_boundary(start) {
                start += 1;
            }
            RawCliCapture::new(&tail[start..], Instant::now())
        });
        let session_id = request.session_id.clone();
        let id = capture_marker_id.clone();
        let audit_request_id = request.id.clone();
        let command = request.command.clone();
        let timeout = request.timeout;
        let background = request.policy.background;
        let track_insertion = request.track_insertion;
        let _event = self
            .mcp
            .capabilities
            .terminal_executions
            .insert(request, capture)
            .unwrap_or_else(|_| unreachable!("target checked on UI thread"));
        let task = self
            .mcp
            .capabilities
            .terminal_executions
            .tasks
            .get_mut(&session_id)
            .expect("inserted task");
        task.capture_marker_id = capture_marker_id.clone();
        if let Some(mut job) = self
            .mcp
            .capabilities
            .terminal_executions
            .jobs
            .remove(&audit_request_id)
        {
            job.window_label = self.mcp.owner_window_label.clone();
            self.mcp
                .capabilities
                .terminal_executions
                .jobs
                .insert(capture_marker_id, job);
        }
        task.raw_capture = raw_capture;
        let cancellation = self.mcp.capabilities.terminal_executions.tasks[&session_id]
            .worker_cancellation
            .clone();
        if !background {
            self.session
                .begin_terminal_execution_boundary(&session_id, &id);
        }
        self.sync_session_event_bridge_session_policy(&session_id);
        self.ensure_terminal_execution_clock(cx);
        if let Some(target) = target {
            self.record_automated_terminal_command(&session_id, &command);
            let task = self
                .blocking_jobs
                .submit_task("terminal-background-execute", move |_| {
                    if cancellation.is_cancelled() {
                        return Err("Command was cancelled.".to_string());
                    }
                    let output = match target {
                        BackgroundTerminalTarget::Ssh(config) => {
                            SshProcessService::new(*config).run_command(&command, timeout)
                        }
                        BackgroundTerminalTarget::Local { working_dir } => {
                            run_local_command(&command, working_dir, timeout)
                        }
                    }
                    .map_err(|error| error.to_string())?;
                    let mut combined = output.stdout;
                    if !output.stderr.is_empty() {
                        if !combined.is_empty() && !combined.ends_with('\n') {
                            combined.push('\n');
                        }
                        combined.push_str(&output.stderr);
                    }
                    Ok((
                        combined,
                        output.exit_status.and_then(|code| i32::try_from(code).ok()),
                    ))
                });
            cx.spawn(async move |this, cx| {
                let result = await_blocking_result(task).await;
                let _ = this.update(cx, |this, cx| {
                    let timed_out = result
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.contains("timed out"));
                    let result = result
                        .map(|(output, exit_code)| TerminalExecuteResult {
                            job_id: None,
                            output,
                            exit_code,
                            duration_ms: 0,
                            timed_out: false,
                            source_truncated: false,
                        })
                        .map_err(|_| {
                            rpc_failure("execution_failed", "Background terminal execution failed.")
                        });
                    let (result, status) = if timed_out {
                        (
                            Ok(TerminalExecuteResult {
                                job_id: None,
                                output: String::new(),
                                exit_code: None,
                                duration_ms: 0,
                                timed_out: true,
                                source_truncated: false,
                            }),
                            TerminalExecutionStatus::TimedOut,
                        )
                    } else {
                        (result, TerminalExecutionStatus::Completed)
                    };
                    this.finish_terminal_execution(&session_id, &id, result, status, cx);
                });
            })
            .detach();
        } else {
            let recorded_command = wrapped.as_ref().map(|_| command.as_str());
            let input = wrapped.unwrap_or_else(|| format!("{command}\r"));
            // The automatic writer bypasses the user-input gate, never broadcast/history.
            let result = self.write_automated_terminal_input(
                &session_id,
                input.as_bytes(),
                recorded_command,
            );
            if result.is_err() {
                self.finish_terminal_execution(
                    &session_id,
                    &id,
                    Err(rpc_failure(
                        "execution_failed",
                        "Cannot write to the terminal.",
                    )),
                    TerminalExecutionStatus::Failed,
                    cx,
                );
            } else {
                if track_insertion {
                    self.mcp
                        .capabilities
                        .terminal_executions
                        .inserted_requests
                        .insert(audit_request_id);
                }
            }
        }
    }

    pub(in crate::features) fn finish_terminal_execution(
        &mut self,
        session_id: &str,
        id: &str,
        result: Result<TerminalExecuteResult, RpcError>,
        status: TerminalExecutionStatus,
        cx: &mut Context<Self>,
    ) {
        let background_output = self
            .mcp
            .capabilities
            .terminal_executions
            .tasks
            .get(session_id)
            .filter(|task| task.capture_marker_id == id && task.request.policy.background)
            .and_then(|_| result.as_ref().ok())
            .map(|result| result.output.clone());
        let Some(completion) = self
            .mcp
            .capabilities
            .terminal_executions
            .finish(session_id, id, result, status)
        else {
            return;
        };
        if self
            .mcp
            .capabilities
            .terminal_executions
            .jobs
            .get(id)
            .is_some_and(|job| {
                job.process_state == nyaterm_mcp_protocol::TerminalProcessState::ExitConfirmed
                    && job.mcp_owned
            })
        {
            self.audit_terminal_job_lifecycle(session_id, "mcp.remote_exit_confirmed", cx);
        }
        if let Some(operation_id) = &completion.operation_id {
            self.mcp
                .capabilities
                .ledger
                .finish(operation_id, completion.result.clone());
            self.mcp
                .capabilities
                .terminal_executions
                .complete_waiters(operation_id, &completion.result);
        }
        if let Some(output) = background_output {
            self.terminal
                .submit_execution_recording(session_id, ExecutionRecordingEvent::Output(output));
        }
        if !completion.recording_tail.is_empty() {
            self.terminal.submit_execution_recording(
                session_id,
                ExecutionRecordingEvent::Output(completion.recording_tail),
            );
        }
        if completion.recording_boundary {
            self.terminal.submit_execution_recording(
                session_id,
                ExecutionRecordingEvent::Boundary { command: None },
            );
        }
        if let Some(event) = completion.event {
            self.present_terminal_execution(event);
        }
        let _ = completion.reply.send(completion.result);
        self.sync_session_event_bridge_session_policy(session_id);
        self.arm_terminal_input_wake(cx);
    }

    pub(in crate::features) fn close_terminal_executions_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = self
            .mcp
            .capabilities
            .terminal_executions
            .tasks
            .get(session_id)
        {
            let id = task.capture_marker_id.clone();
            self.finish_terminal_execution(
                session_id,
                &id,
                Err(rpc_failure(
                    "target_disconnected",
                    "The target terminal disconnected.",
                )),
                TerminalExecutionStatus::Disconnected,
                cx,
            );
        }
        let state = &mut self.mcp.capabilities.terminal_executions;
        for job in state
            .jobs
            .values_mut()
            .filter(|job| job.target.session_id() == Some(session_id) && job.finished.is_none())
        {
            job.receiver = None; // Receiver closure asks the auxiliary worker to close its own channel.
            job.disconnect();
        }
        state.draining.remove(session_id);
        state.retired.remove(session_id);
        state.recovery.remove(session_id);
        self.sync_session_event_bridge_session_policy(session_id);
    }

    pub(in crate::features) fn note_terminal_execution_output_dropped(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self
            .mcp
            .capabilities
            .terminal_executions
            .tasks
            .get(session_id)
        else {
            return;
        };
        let id = task.capture_marker_id.clone();
        self.finish_terminal_execution(
            session_id,
            &id,
            Ok(TerminalExecuteResult {
                job_id: None,
                output: String::new(),
                exit_code: None,
                duration_ms: 0,
                timed_out: false,
                source_truncated: true,
            }),
            TerminalExecutionStatus::Failed,
            cx,
        );
    }

    pub(super) fn ensure_terminal_execution_clock(&mut self, cx: &mut Context<Self>) {
        if self.mcp.capabilities.terminal_executions.clock_armed {
            return;
        }
        self.mcp.capabilities.terminal_executions.clock_armed = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let Ok(running) = this.update(cx, |this, cx| {
                    this.poll_terminal_executions(cx);
                    let state = &mut this.mcp.capabilities.terminal_executions;
                    let running = !state.tasks.is_empty() || !state.draining.is_empty();
                    let running = running
                        || this.mcp.capabilities.session_opens.pending()
                        || this.mcp.capabilities.ledger.requires_cleanup()
                        || !this.mcp.capabilities.terminal_executions.jobs.is_empty();
                    this.mcp.capabilities.terminal_executions.clock_armed = running;
                    running
                }) else {
                    break;
                };
                if !running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn poll_terminal_executions(&mut self, cx: &mut Context<Self>) {
        self.mcp.capabilities.terminal_executions.prune_waiters();
        self.poll_terminal_jobs(cx);
        self.poll_mcp_session_opens(cx);
        let now = Instant::now();
        let raw_ended = self
            .mcp
            .capabilities
            .terminal_executions
            .tasks
            .iter()
            .filter_map(|(session, task)| {
                if task.request.cancellation.is_cancelled()
                    || self.session.is_disconnected(session)
                    || self.session.session_info(session).is_none()
                {
                    return None;
                }
                let end = task
                    .raw_capture
                    .as_ref()?
                    .poll(now, task.started + task.request.timeout)?;
                Some((session.clone(), task.capture_marker_id.clone(), end))
            })
            .collect::<Vec<_>>();
        for (session, id, end) in raw_ended {
            let (result, status) = if end == RawCliEnd::Prompt {
                (
                    Ok(TerminalExecuteResult {
                        job_id: None,
                        output: end.description().into(),
                        exit_code: None,
                        duration_ms: 0,
                        timed_out: false,
                        source_truncated: false,
                    }),
                    TerminalExecutionStatus::Completed,
                )
            } else {
                (
                    Err(rpc_failure("execution_incomplete", end.description())),
                    if end == RawCliEnd::TimedOut {
                        TerminalExecutionStatus::TimedOut
                    } else {
                        TerminalExecutionStatus::Failed
                    },
                )
            };
            self.finish_terminal_execution(&session, &id, result, status, cx);
        }
        let ended = self
            .mcp
            .capabilities
            .terminal_executions
            .tasks
            .iter()
            .filter_map(|(session, task)| {
                let status = if task.request.cancellation.is_cancelled() {
                    TerminalExecutionStatus::Cancelled
                } else if self.session.is_disconnected(session)
                    || self.session.session_info(session).is_none()
                {
                    TerminalExecutionStatus::Disconnected
                } else if now.duration_since(task.started) >= task.request.timeout {
                    TerminalExecutionStatus::TimedOut
                } else {
                    return None;
                };
                Some((session.clone(), task.capture_marker_id.clone(), status))
            })
            .collect::<Vec<_>>();
        for (session, id, status) in ended {
            let result = match status {
                TerminalExecutionStatus::TimedOut => Ok(TerminalExecuteResult {
                    job_id: None,
                    output: String::new(),
                    exit_code: None,
                    duration_ms: 0,
                    timed_out: true,
                    source_truncated: false,
                }),
                _ => Err(rpc_failure(
                    "cancelled",
                    "Stopped waiting for the command; it may still be running.",
                )),
            };
            self.finish_terminal_execution(&session, &id, result, status, cx);
        }
        for session in self
            .mcp
            .capabilities
            .terminal_executions
            .expire_draining(now)
        {
            self.sync_session_event_bridge_session_policy(&session);
        }
    }
}

#[cfg(test)]
mod tests;

mod jobs;

mod background_jobs;
