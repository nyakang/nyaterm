use super::jobs::TerminalJob;
use crate::features::NyaTermApp;
use crate::features::capability_runtime::{CapabilityRequest, idempotency::Lookup, to_value};
use crate::features::mcp::server::rpc_failure;
use crate::features::runtime_jobs::await_blocking_result;
use gpui::Context;
use nyaterm_mcp_protocol::{
    ConnectionPollResult, ConnectionStartResult, ConnectionStopResult, ExecutionTarget, RpcError,
    TerminalBackend, TerminalJobStatus, TerminalPollArgs, TerminalProcessState, TerminalStartArgs,
};
use nyaterm_transport::ssh_execution::streaming::{SshExecEvent, stream_ssh_command};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

impl super::TerminalExecutionState {
    pub(in crate::features::capability_runtime) fn accepts_background_operations(&self) -> bool {
        !self.background_closed
    }

    pub(in crate::features) fn has_background_operations(&self) -> bool {
        self.jobs.values().any(|job| {
            job.target.connection_id().is_some()
                && (job.finished.is_none()
                    || job.receiver.is_some()
                    || job
                        .controller
                        .as_ref()
                        .is_some_and(|task| !task.is_finished()))
        })
    }
    pub(in crate::features) fn request_background_shutdown(&mut self) {
        for job in self
            .jobs
            .values_mut()
            .filter(|job| job.target.connection_id().is_some() && job.finished.is_none())
        {
            job.stop_started.get_or_insert_with(Instant::now);
            job.stop_state = "accepted".into();
            job.status = TerminalJobStatus::Cancelling;
            job.interrupt.cancel();
            job.shutdown_requested = true;
        }
    }

    pub(in crate::features) fn take_background_shutdown(
        &mut self,
    ) -> impl FnOnce() + Send + 'static {
        self.background_closed = true;
        self.request_background_shutdown();
        let ids = self
            .jobs
            .iter()
            .filter(|(_, job)| job.target.connection_id().is_some())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let jobs = ids
            .into_iter()
            .filter_map(|id| self.jobs.remove(&id))
            .collect::<Vec<_>>();
        move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline
                && jobs.iter().any(|job| {
                    job.controller
                        .as_ref()
                        .is_some_and(|task| !task.is_finished())
                })
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(jobs);
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn toggle_background_tasks(&mut self, cx: &mut Context<Self>) {
        self.mcp.background_tasks.open = !self.mcp.background_tasks.open;
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn select_background_task(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.mcp.background_tasks.selected = Some(id);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn background_tasks_view(
        &self,
    ) -> crate::features::mcp::tasks::BackgroundTasksView {
        use crate::features::mcp::tasks::{
            BackgroundJobDetails, BackgroundJobView, BackgroundTasksView,
        };
        let presentation = &self.mcp.background_tasks;
        if !presentation.open {
            return BackgroundTasksView::default();
        }
        let registry = &self.mcp.capabilities.terminal_executions.jobs;
        let visible = |job: &&TerminalJob| {
            job.target.connection_id().is_some() && job.window_label == self.mcp.owner_window_label
        };
        let mut jobs = registry
            .iter()
            .filter(|(_, job)| visible(job))
            .map(|(id, job)| BackgroundJobView {
                id: id.clone(),
                host: job.target_name.clone(),
                command: job.command.clone(),
                client: job.client.clone(),
                started_at_ms: job.started_at_ms,
                status: job.status,
            })
            .collect::<Vec<_>>();
        jobs.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        let selected = presentation.selected.as_ref().and_then(|id| {
            registry.get(id).filter(visible).map(|job| {
                let first = job.output.read(0, 4).ok();
                let offset = first.as_ref().map_or(0, |page| {
                    page.base_offset
                        .max(page.total_bytes.saturating_sub(64 * 1024))
                });
                let tail = (offset..=offset.saturating_add(3))
                    .find_map(|offset| job.output.read(offset, 64 * 1024).ok());
                BackgroundJobDetails {
                    id: id.clone(),
                    connection_id: job.target.connection_id().unwrap().into(),
                    truncated: tail
                        .as_ref()
                        .is_some_and(|page| page.source_truncated || !page.eof || page.offset != 0),
                    output: tail.map_or_else(String::new, |page| page.data),
                    exit_code: job.exit_code,
                    process_state: job.process_state,
                    stop_state: job.stop_state.clone(),
                    can_stop: job.finished.is_none() && job.stop_started.is_none(),
                }
            })
        });
        BackgroundTasksView {
            open: true,
            jobs,
            selected,
        }
    }

    pub(in crate::features) fn stop_mcp_background_operations(&mut self, cx: &mut Context<Self>) {
        self.mcp
            .capabilities
            .terminal_executions
            .request_background_shutdown();
        self.session.ssh_connection_pool().invalidate_background();
        for (_, cancellation) in self.mcp.capabilities.active_requests.values() {
            cancellation.cancel();
        }
        self.mcp.capabilities.prune_cancelled();
        self.ensure_terminal_execution_clock(cx);
    }

    pub(super) fn dispatch_connection_start(
        &mut self,
        request: CapabilityRequest,
        mut args: TerminalStartArgs,
        cx: &mut Context<Self>,
    ) {
        let target = args.target.clone().expect("validated connection target");
        let connection_id = target.connection_id().expect("connection target");
        if let Err(error) = self.authorize_saved_target(&request, connection_id) {
            let _ = request.reply.send(Err(error));
            return;
        }
        args.backend = Some(TerminalBackend::SshExec);
        args.timeout_ms = Some(args.timeout_ms.unwrap_or(300_000));
        let canonical = serde_json::to_value(&args).expect("start arguments");
        let operation = match self.mcp.capabilities.ledger.begin(
            &request.connection_id,
            &request.tool,
            args.idempotency_key.as_deref(),
            &canonical,
        ) {
            Ok(Lookup::Complete(result)) => {
                let result = result.and_then(|value| {
                    self.authorize_terminal_job(
                        &request.connection_id,
                        &request.scope,
                        value["jobId"].as_str().unwrap_or_default(),
                    )?;
                    Ok(value)
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
        let result = self.register_connection_job(&request, args, target, operation.clone(), cx);
        self.mcp.capabilities.ledger.pin(&operation, result.is_ok());
        self.mcp
            .capabilities
            .ledger
            .finish(&operation, result.clone());
        let _ = request.reply.send(result);
        self.ensure_terminal_execution_clock(cx);
    }

    fn register_connection_job(
        &mut self,
        request: &CapabilityRequest,
        args: TerminalStartArgs,
        target: ExecutionTarget,
        id: String,
        cx: &mut Context<Self>,
    ) -> Result<Value, RpcError> {
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
        let connection_id = target.connection_id().expect("connection target");
        let prepared = self.prepare_background_target(connection_id, true)?;
        let name = self
            .connection_state
            .connection_by_id(connection_id)
            .map(|connection| connection.name.clone())
            .unwrap_or_else(|| connection_id.into());
        let runtime = cx
            .try_global::<crate::app_shell::DesktopControllerGlobal>()
            .and_then(|global| global.0.read(cx).ai_runtime.clone().ok())
            .ok_or_else(|| rpc_failure("host_unavailable", "The task runtime is unavailable."))?;
        let mut job = TerminalJob::new(
            true,
            request.connection_id.clone(),
            target.clone(),
            TerminalBackend::SshExec,
            Some(id.clone()),
            args.command.clone(),
            request.client.clone(),
            name,
        );
        job.window_label = self.mcp.owner_window_label.clone();
        let interrupt = job.interrupt.clone();
        let acquiring = interrupt.clone();
        let deadline = Instant::now() + Duration::from_millis(args.timeout_ms.unwrap_or(300_000));
        let task = self
            .blocking_jobs
            .submit_task("mcp-background-ssh", move |_| {
                prepared.acquire(&acquiring, deadline)
            });
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        job.receiver = Some(receiver);
        job.controller = Some(runtime.spawn(CancellationToken::new(), async move {
            match await_blocking_result(task).await {
                Ok(lease) => {
                    let timeout = deadline.saturating_duration_since(Instant::now());
                    stream_ssh_command(
                        lease.config(),
                        Some(lease.connection.handle()),
                        args.command.into_bytes(),
                        timeout,
                        interrupt,
                        sender,
                    )
                    .await;
                    drop(lease);
                }
                Err(_) => {
                    let event = if interrupt.is_cancelled() {
                        SshExecEvent::CancelledBeforeStart
                    } else if Instant::now() >= deadline {
                        SshExecEvent::TimedOut
                    } else {
                        SshExecEvent::FailedBeforeStart
                    };
                    let _ = sender.send(event).await;
                }
            }
        }));
        self.mcp
            .capabilities
            .terminal_executions
            .jobs
            .insert(id.clone(), job);
        self.audit_execution_target(&target, "mcp.job_registered", cx);
        to_value(ConnectionStartResult {
            job_id: id,
            target,
            backend: TerminalBackend::SshExec,
            status: TerminalJobStatus::Starting,
        })
    }

    pub(super) fn connection_poll_result(&self, args: TerminalPollArgs) -> Result<Value, RpcError> {
        let job = &self.mcp.capabilities.terminal_executions.jobs[&args.job_id];
        let mut budget = args.max_bytes.unwrap_or(16 * 1024);
        loop {
            let page = job
                .output
                .read(args.offset.unwrap_or(0), budget)
                .map_err(|_| rpc_failure("invalid_argument", "Output offset is invalid."))?;
            let result = to_value(ConnectionPollResult {
                job_id: args.job_id.clone(),
                target: job.target.clone(),
                backend: job.backend,
                status: job.status,
                process_state: job.process_state,
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
            if serde_json::to_vec(&result)
                .expect("serialized result")
                .len()
                <= nyaterm_mcp_protocol::MAX_INLINE_OUTPUT_BYTES
            {
                return Ok(result);
            }
            budget = (budget / 2).max(4);
        }
    }

    pub(in crate::features) fn stop_connection_job(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, RpcError> {
        let job = self
            .mcp
            .capabilities
            .terminal_executions
            .jobs
            .get_mut(id)
            .filter(|job| job.target.connection_id().is_some())
            .ok_or_else(|| rpc_failure("job_not_found", "Background job is unavailable."))?;
        if job.process_state == TerminalProcessState::ExitConfirmed {
            job.stop_state = "confirmed_ended".into();
        } else if job.stop_started.is_none() {
            job.stop_started = Some(Instant::now());
            if job.receiver.is_some() {
                job.status = TerminalJobStatus::Cancelling;
                job.stop_state = "accepted".into();
                job.interrupt.cancel();
            } else {
                job.stop_state = "observer_unavailable".into();
            }
        }
        self.poll_terminal_jobs(cx);
        let job = &self.mcp.capabilities.terminal_executions.jobs[id];
        to_value(ConnectionStopResult {
            job_id: id.into(),
            target: job.target.clone(),
            status: job.status,
            stop_state: job.stop_state.clone(),
            process_state: job.process_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalJob;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;
    use gpui::AppContext as _;
    use nyaterm_core::capabilities::CapabilityScope;
    use nyaterm_mcp_protocol::{
        ExecutionTarget, TerminalBackend, TerminalJobStatus, TerminalPollArgs, TerminalProcessState,
    };
    use nyaterm_transport::ssh_execution::streaming::SshExecEvent;
    use std::time::{Duration, Instant};

    #[test]
    fn workspace_shutdown_closes_background_admission_before_waiting_for_cleanup() {
        let mut state = super::super::TerminalExecutionState::default();
        assert!(state.accepts_background_operations());
        let cleanup = state.take_background_shutdown();
        assert!(!state.accepts_background_operations());
        cleanup();
    }

    #[gpui::test]
    fn background_jobs_survive_owner_detach_enforce_scope_and_never_need_a_terminal(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = TestConfigDir::new("nyaterm-mcp-background-job-scope");
        let app = app_with_visible_local_session(cx, directory.path(), "visible");
        cx.update_entity(&app, |app, cx| {
            let sessions = app
                .session
                .session_ids()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let mut job = TerminalJob::new(
                true,
                "owner".into(),
                ExecutionTarget::Connection {
                    connection_id: "saved".into(),
                },
                TerminalBackend::SshExec,
                None,
                "pwd".into(),
                "client".into(),
                "host".into(),
            );
            job.window_label = app.mcp.owner_window_label.clone();
            let (sender, receiver) = tokio::sync::mpsc::channel(8);
            job.receiver = Some(receiver);
            app.mcp
                .capabilities
                .terminal_executions
                .jobs
                .insert("background".into(), job);
            app.mcp.capabilities.connection_disconnected("owner");
            assert!(
                app.authorize_terminal_job("owner", &CapabilityScope::AllSessions, "background")
                    .is_ok()
            );
            assert!(
                app.authorize_terminal_job("other", &CapabilityScope::AllSessions, "background")
                    .is_err()
            );
            assert!(
                app.authorize_terminal_job(
                    "owner",
                    &CapabilityScope::explicit(["visible".into()], None),
                    "background"
                )
                .is_err()
            );
            assert!(
                app.authorize_terminal_job(
                    "owner",
                    &CapabilityScope::current_window("other-window"),
                    "background"
                )
                .is_err()
            );
            sender.try_send(SshExecEvent::Started).unwrap();
            sender
                .try_send(SshExecEvent::Output {
                    data: "猫\n".as_bytes().to_vec(),
                    stderr: false,
                })
                .unwrap();
            app.poll_terminal_jobs(cx);
            assert_eq!(
                app.session
                    .session_ids()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                sessions
            );
            let page = app
                .connection_poll_result(TerminalPollArgs {
                    job_id: "background".into(),
                    offset: None,
                    max_bytes: None,
                })
                .unwrap();
            assert_eq!(page["output"], "猫\n");
            assert!(page.get("sessionId").is_none() && page.get("terminalState").is_none());
            app.mcp.background_tasks.open = true;
            app.mcp.background_tasks.selected = Some("background".into());
            let view = app.background_tasks_view();
            assert_eq!(view.jobs.len(), 1);
            assert_eq!(view.jobs[0].host, "host");
            assert_eq!(view.selected.unwrap().output, "猫\n");
            let stopped = app.stop_connection_job("background", cx).unwrap();
            assert_eq!(stopped["stopState"], "accepted");
            assert_ne!(stopped["processState"], "exit_confirmed");
            sender.try_send(SshExecEvent::ExitStatus(130)).unwrap();
            sender
                .try_send(SshExecEvent::Finished {
                    confirmed: true,
                    interrupted: true,
                })
                .unwrap();
            drop(sender);
            app.poll_terminal_jobs(cx);
            let job = &app.mcp.capabilities.terminal_executions.jobs["background"];
            assert_eq!(job.process_state, TerminalProcessState::ExitConfirmed);
            assert_eq!(job.status, TerminalJobStatus::Cancelled);
            assert_eq!(job.stop_state, "confirmed_ended");
        });
    }

    #[gpui::test]
    fn background_shutdown_is_bounded_and_does_not_invent_remote_exit(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.executor().allow_parking();
        let directory = TestConfigDir::new("nyaterm-mcp-background-shutdown");
        let app = app_with_visible_local_session(cx, directory.path(), "visible");
        cx.update_entity(&app, |app, cx| {
            let mut job = TerminalJob::new(
                true,
                "owner".into(),
                ExecutionTarget::Connection {
                    connection_id: "saved".into(),
                },
                TerminalBackend::SshExec,
                None,
                "sleep".into(),
                "client".into(),
                "host".into(),
            );
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            job.receiver = Some(receiver);
            job.process_state = TerminalProcessState::Active;
            app.mcp
                .capabilities
                .terminal_executions
                .jobs
                .insert("background".into(), job);
            app.mcp
                .capabilities
                .terminal_executions
                .request_background_shutdown();
            let job = app
                .mcp
                .capabilities
                .terminal_executions
                .jobs
                .get_mut("background")
                .unwrap();
            assert!(job.interrupt.is_cancelled());
            job.stop_started = Some(Instant::now() - Duration::from_secs(3));
            app.poll_terminal_jobs(cx);
            let job = &app.mcp.capabilities.terminal_executions.jobs["background"];
            assert_eq!(job.process_state, TerminalProcessState::Unknown);
            assert_eq!(job.status, TerminalJobStatus::Unknown);
            assert!(job.receiver.is_none() && job.exit_code.is_none());
            assert!(sender.is_closed());
        });
    }
}
