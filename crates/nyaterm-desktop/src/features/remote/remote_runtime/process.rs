use futures::StreamExt as _;
use rust_i18n::t;

use gpui::{ClipboardItem, Context, Window};
use nyaterm_transport::SshProcessService;

use crate::features::NyaTermApp;
use crate::features::remote::state::ProcessApplyOutcome;
use crate::features::runtime_jobs::{ProcessJobOutput, ProcessJobResult};
use crate::models::{DockerTab, RemoteProcessSortKey};

impl NyaTermApp {
    pub(in crate::features) fn set_docker_tab(&mut self, tab: DockerTab, cx: &mut Context<Self>) {
        self.remote_ops.set_docker_tab(tab);
        self.load_docker_resource_if_needed(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_docker_tab_menu(&mut self, cx: &mut Context<Self>) {
        self.remote_ops.toggle_docker_tab_menu();
        cx.notify();
    }

    /// Apply an edit from the Docker filter box.
    pub(in crate::features) fn apply_docker_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.apply_docker_search(text);
        cx.notify();
    }

    /// Apply an edit from the process filter box.
    pub(in crate::features) fn apply_process_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.apply_process_search(text);
        cx.notify();
    }

    pub(in crate::features) fn toggle_process_sort(
        &mut self,
        key: RemoteProcessSortKey,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.toggle_process_sort(key);
        cx.notify();
    }

    pub(in crate::features) fn toggle_process_selection(
        &mut self,
        pid: u32,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.toggle_process_selection(pid);
        cx.notify();
    }

    /// Apply an edit from the nice value box.
    ///
    /// A nice value is a small signed number, so the draft keeps a leading
    /// minus and up to three digits and drops everything else.
    pub(in crate::features) fn apply_process_nice_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.apply_process_nice_input(text);
        cx.notify();
    }

    pub(in crate::features) fn apply_process_nice_draft(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pid, nice)) = self.remote_ops.validated_process_nice_draft() else {
            cx.notify();
            return;
        };
        self.renice_process(pid, nice, window, cx);
    }

    pub(in crate::features) fn copy_process_text(
        &mut self,
        value: String,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.remote_ops
            .set_process_status(format!("copied process {label}"));
        self.shell
            .set_status(self.remote_ops.process_status().to_string());
        cx.notify();
    }

    pub(in crate::features) fn copy_docker_text(
        &mut self,
        value: String,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.remote_ops
            .set_docker_status(format!("copied Docker {label}"));
        self.shell
            .set_status(self.remote_ops.docker_status().to_string());
        cx.notify();
    }

    pub(in crate::features) fn request_process_signal(
        &mut self,
        pid: u32,
        signal: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if signal != "KILL" {
            self.signal_process(pid, signal, window, cx);
            return;
        }
        let description = t!(
            "processManager.confirmSignalDesc",
            signal = signal,
            pid = pid,
            command = format!("kill -{signal} -- {pid}")
        )
        .to_string();
        self.open_confirm_dialog(
            (
                t!("processManager.confirmSignalTitle").to_string(),
                description,
                t!("common.confirm").to_string(),
                true,
                move |app, window, cx| {
                    app.signal_process(pid, signal, window, cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn refresh_processes(&mut self, cx: &mut Context<Self>) {
        let context = match self.active_ssh_runtime_context("listing processes") {
            Ok(context) => context,
            Err(message) => {
                self.remote_ops.set_process_status(message);
                self.shell
                    .set_status(self.remote_ops.process_status().to_string());
                cx.notify();
                return;
            }
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.process_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_process_status("process operation already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_process_job(job_session_id.clone());
        self.remote_ops.close_process_menu();
        self.remote_ops.mark_process_refresh_started();
        self.remote_ops
            .set_process_status("listing remote processes");
        let job_id = ticket.job_id;
        let tx = ticket.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = job_session_id.clone();
        if let Err(error) = self
            .blocking_jobs
            .submit_detached("remote-process-list", move |_| {
                let result = (|| {
                    SshProcessService::with_multiplex(config, multiplex)?
                        .list_processes()
                        .map(ProcessJobOutput::Listed)
                })()
                .map_err(|error: anyhow::Error| error.to_string());
                let _ = tx.unbounded_send(ProcessJobResult {
                    job_id,
                    session_id: job_session_id,
                    result,
                });
            })
        {
            let _ = rejected_tx.unbounded_send(ProcessJobResult {
                job_id,
                session_id: rejected_session_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn signal_process(
        &mut self,
        pid: u32,
        signal: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = match self.active_ssh_runtime_context("signalling processes") {
            Ok(context) => context,
            Err(message) => {
                self.remote_ops.set_process_status(message);
                self.shell
                    .set_status(self.remote_ops.process_status().to_string());
                cx.notify();
                return;
            }
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.process_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_process_status("process operation already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_process_job(job_session_id.clone());
        self.remote_ops
            .set_process_status(format!("sending {signal} to pid {pid}"));
        let job_id = ticket.job_id;
        let tx = ticket.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = job_session_id.clone();
        if let Err(error) = self
            .blocking_jobs
            .submit_detached("remote-process-signal", move |_| {
                let result = (|| {
                    let service = SshProcessService::with_multiplex(config, multiplex)?;
                    service.signal_process(pid, signal)?;
                    let processes = service.list_processes()?;
                    Ok(ProcessJobOutput::Signalled {
                        pid,
                        signal: signal.to_string(),
                        processes,
                    })
                })()
                .map_err(|error: anyhow::Error| error.to_string());
                let _ = tx.unbounded_send(ProcessJobResult {
                    job_id,
                    session_id: job_session_id,
                    result,
                });
            })
        {
            let _ = rejected_tx.unbounded_send(ProcessJobResult {
                job_id,
                session_id: rejected_session_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn renice_process(
        &mut self,
        pid: u32,
        nice: i32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = match self.active_ssh_runtime_context("renicing processes") {
            Ok(context) => context,
            Err(message) => {
                self.remote_ops.set_process_status(message);
                self.shell
                    .set_status(self.remote_ops.process_status().to_string());
                cx.notify();
                return;
            }
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.process_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_process_status("process operation already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_process_job(job_session_id.clone());
        self.remote_ops
            .set_process_status(format!("renicing pid {pid} to {nice}"));
        let job_id = ticket.job_id;
        let tx = ticket.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = job_session_id.clone();
        if let Err(error) = self
            .blocking_jobs
            .submit_detached("remote-process-renice", move |_| {
                let result = (|| {
                    let service = SshProcessService::with_multiplex(config, multiplex)?;
                    service.renice_process(pid, nice)?;
                    let processes = service.list_processes()?;
                    Ok(ProcessJobOutput::Reniced {
                        pid,
                        nice,
                        processes,
                    })
                })()
                .map_err(|error: anyhow::Error| error.to_string());
                let _ = tx.unbounded_send(ProcessJobResult {
                    job_id,
                    session_id: job_session_id,
                    result,
                });
            })
        {
            let _ = rejected_tx.unbounded_send(ProcessJobResult {
                job_id,
                session_id: rejected_session_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    /// Deliver remote process job replies as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `next_process_event`, which meant a reply waited for the next tick and
    /// forced `runtime_quiet_tick_allowed` to carry a `remote_ops` term to keep
    /// that wait short.
    pub(in crate::features) fn start_process_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.remote_ops.take_process_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.apply_process_event(event) {
                            cx.notify();
                        }
                        // Flush boundary: a reply changed the pane, so its panel
                        // gets the snapshot before the next paint.
                        this.flush_remote_panel_snapshots(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Apply one reply, reporting whether the UI needs a repaint.
    fn apply_process_event(&mut self, event: ProcessJobResult) -> bool {
        match self
            .remote_ops
            .apply_process_event(event, self.session.active_id())
        {
            ProcessApplyOutcome::Ignored => false,
            ProcessApplyOutcome::CompletedInactive => true,
            ProcessApplyOutcome::Applied { status } => {
                self.shell.set_status(status);
                true
            }
        }
    }
}
