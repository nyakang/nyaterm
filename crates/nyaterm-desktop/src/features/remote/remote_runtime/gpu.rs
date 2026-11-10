use crate::features::{NyaTermApp, runtime_jobs::GpuJobResult, runtime_jobs::NpuJobResult};
use futures::StreamExt as _;
use gpui::Context;
use nyaterm_remote_ops::ascend_npu::{RemoteNpuOverview, RemoteNpuService};
use nyaterm_remote_ops::gpu::{RemoteGpuOverview, RemoteGpuService};

impl NyaTermApp {
    pub(in crate::features) fn apply_gpu_search(&mut self, text: String, cx: &mut Context<Self>) {
        self.remote_ops.apply_gpu_search(text);
        cx.notify();
    }

    pub(in crate::features) fn apply_npu_search(&mut self, text: String, cx: &mut Context<Self>) {
        self.remote_ops.apply_npu_search(text);
        cx.notify();
    }

    pub(in crate::features) fn toggle_gpu_device_expanded(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.toggle_gpu_device_expanded(key);
        cx.notify();
    }

    pub(in crate::features) fn toggle_npu_device_expanded(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops.toggle_npu_device_expanded(key);
        cx.notify();
    }

    pub(in crate::features) fn refresh_gpu(&mut self, cx: &mut Context<Self>) {
        self.refresh_gpu_with_mode(false, cx);
    }

    pub(in crate::features) fn refresh_gpu_auto(&mut self, cx: &mut Context<Self>) {
        self.refresh_gpu_with_mode(true, cx);
    }

    fn refresh_gpu_with_mode(&mut self, skip_unavailable: bool, cx: &mut Context<Self>) {
        if self.local_host_tools_unsupported() {
            self.remote_ops
                .set_gpu_status(rust_i18n::t!("hostTools.localWindowsUnsupported").to_string());
            cx.notify();
            return;
        }
        let context = match self.active_ssh_runtime_context("inspecting GPU") {
            Ok(context) => context,
            Err(message) => {
                self.remote_ops.set_gpu_status(message);
                self.shell
                    .set_status(self.remote_ops.gpu_status().to_string());
                cx.notify();
                return;
            }
        };
        let target = context.target;
        let job_session_id = context.session_id;
        if skip_unavailable && self.remote_ops.gpu_unavailable_for(&job_session_id) {
            return;
        }
        if self.remote_ops.gpu_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_gpu_status("GPU refresh already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_gpu_job(job_session_id.clone());
        self.remote_ops.mark_gpu_refresh_started();
        self.remote_ops
            .set_gpu_status("loading NVIDIA GPU overview");
        let job_id = ticket.job_id;
        let tx = ticket.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = job_session_id.clone();
        if let Err(error) = self.blocking_jobs.submit_detached("remote-gpu", move |_| {
            let result = RemoteGpuService::with_target(target)
                .overview()
                .map_err(|error| error.to_string());
            let _ = tx.unbounded_send(GpuJobResult {
                job_id,
                session_id: job_session_id,
                result,
            });
        }) {
            let _ = rejected_tx.unbounded_send(GpuJobResult {
                job_id,
                session_id: rejected_session_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    /// Deliver GPU job replies as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `next_gpu_event`, which meant a reply waited for the next tick and
    /// forced `runtime_quiet_tick_allowed` to carry a `remote_ops` term to keep
    /// that wait short.
    pub(in crate::features) fn start_gpu_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.remote_ops.take_gpu_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.apply_gpu_event(event) {
                            cx.notify();
                        }
                        // Flush boundary: a reply changed the pane, so its panel
                        // gets the snapshot now rather than at the next paint.
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
    fn apply_gpu_event(&mut self, event: GpuJobResult) -> bool {
        if !self
            .remote_ops
            .complete_gpu_event(event.job_id, &event.session_id)
        {
            // A superseded job's reply; the pane has already moved on.
            return false;
        }
        if self.session.active_id() != Some(event.session_id.as_str()) {
            // Another session is active now, but completing the job is
            // itself a state change worth painting.
            return true;
        }
        match event.result {
            Ok(overview) => {
                self.remote_ops.reset_gpu_refresh_failures();
                self.remote_ops
                    .set_gpu_status(gpu_overview_status(&overview));
                self.shell
                    .set_status(self.remote_ops.gpu_status().to_string());
                self.cache_asset_gpu_snapshot(&event.session_id, &overview);
                self.remote_ops.apply_gpu(&event.session_id, overview);
            }
            Err(error) => {
                self.remote_ops.record_gpu_refresh_failure();
                self.remote_ops
                    .set_gpu_status(format!("GPU refresh failed: {error}"));
                self.shell
                    .set_status(self.remote_ops.gpu_status().to_string());
            }
        }
        true
    }

    pub(in crate::features) fn refresh_npu(&mut self, cx: &mut Context<Self>) {
        self.refresh_npu_with_mode(false, cx);
    }

    pub(in crate::features) fn refresh_npu_auto(&mut self, cx: &mut Context<Self>) {
        self.refresh_npu_with_mode(true, cx);
    }

    fn refresh_npu_with_mode(&mut self, skip_unavailable: bool, cx: &mut Context<Self>) {
        if self.local_host_tools_unsupported() {
            self.remote_ops
                .set_npu_status(rust_i18n::t!("hostTools.localWindowsUnsupported").to_string());
            cx.notify();
            return;
        }
        let context = match self.active_ssh_runtime_context("inspecting NPU") {
            Ok(context) => context,
            Err(message) => {
                self.remote_ops.set_npu_status(message);
                self.shell
                    .set_status(self.remote_ops.npu_status().to_string());
                cx.notify();
                return;
            }
        };
        let target = context.target;
        let job_session_id = context.session_id;
        if skip_unavailable && self.remote_ops.npu_unavailable_for(&job_session_id) {
            return;
        }
        if self.remote_ops.npu_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_npu_status("NPU refresh already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_npu_job(job_session_id.clone());
        self.remote_ops.mark_npu_refresh_started();
        self.remote_ops
            .set_npu_status("loading Ascend NPU overview");
        let job_id = ticket.job_id;
        let tx = ticket.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = job_session_id.clone();
        if let Err(error) = self.blocking_jobs.submit_detached("remote-npu", move |_| {
            let result = RemoteNpuService::with_target(target)
                .overview()
                .map_err(|error| error.to_string());
            let _ = tx.unbounded_send(NpuJobResult {
                job_id,
                session_id: job_session_id,
                result,
            });
        }) {
            let _ = rejected_tx.unbounded_send(NpuJobResult {
                job_id,
                session_id: rejected_session_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    /// Deliver NPU job replies as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `next_npu_event`, which meant a reply waited for the next tick and
    /// forced `runtime_quiet_tick_allowed` to carry a `remote_ops` term to keep
    /// that wait short.
    pub(in crate::features) fn start_npu_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.remote_ops.take_npu_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.apply_npu_event(event) {
                            cx.notify();
                        }
                        // Flush boundary: a reply changed the pane, so its panel
                        // gets the snapshot now rather than at the next paint.
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
    fn apply_npu_event(&mut self, event: NpuJobResult) -> bool {
        if !self
            .remote_ops
            .complete_npu_event(event.job_id, &event.session_id)
        {
            // A superseded job's reply; the pane has already moved on.
            return false;
        }
        if self.session.active_id() != Some(event.session_id.as_str()) {
            // Another session is active now, but completing the job is
            // itself a state change worth painting.
            return true;
        }
        match event.result {
            Ok(overview) => {
                self.remote_ops.reset_npu_refresh_failures();
                self.remote_ops
                    .set_npu_status(npu_overview_status(&overview));
                self.shell
                    .set_status(self.remote_ops.npu_status().to_string());
                self.cache_asset_npu_snapshot(&event.session_id, &overview);
                self.remote_ops.apply_npu(&event.session_id, overview);
            }
            Err(error) => {
                self.remote_ops.record_npu_refresh_failure();
                self.remote_ops
                    .set_npu_status(format!("NPU refresh failed: {error}"));
                self.shell
                    .set_status(self.remote_ops.npu_status().to_string());
            }
        }
        true
    }
}

fn gpu_overview_status(overview: &RemoteGpuOverview) -> String {
    if !overview.available {
        return "NVIDIA GPU is not available on this SSH host".to_string();
    }
    let used = overview
        .gpus
        .iter()
        .map(|gpu| gpu.memory_used_mb)
        .sum::<u64>();
    let total = overview
        .gpus
        .iter()
        .map(|gpu| gpu.memory_total_mb)
        .sum::<u64>();
    format!(
        "NVIDIA GPU · {} device(s) · {used}/{total} MiB",
        overview.gpus.len()
    )
}

fn npu_overview_status(overview: &RemoteNpuOverview) -> String {
    if !overview.available {
        return "Ascend NPU is not available on this SSH host".to_string();
    }
    let used = overview
        .npus
        .iter()
        .map(|npu| npu.memory_used_mb)
        .sum::<u64>();
    let total = overview
        .npus
        .iter()
        .map(|npu| npu.memory_total_mb)
        .sum::<u64>();
    format!(
        "Ascend NPU · {} device(s) · {used}/{total} MiB",
        overview.npus.len()
    )
}
