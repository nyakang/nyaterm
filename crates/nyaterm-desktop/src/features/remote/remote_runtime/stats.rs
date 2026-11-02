use futures::StreamExt as _;
use gpui::Context;
use nyaterm_transport::{RemoteStatsOptions, RemoteStatsService};

use crate::features::NyaTermApp;
use crate::features::remote::state::StatsApplyOutcome;
use crate::features::runtime_jobs::StatsJobResult;

impl NyaTermApp {
    pub(in crate::features) fn remote_stats_users_setting_changed(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.remote_ops
            .invalidate_stats_users(self.settings.summary().ui_remote_stats_show_users);
        self.flush_remote_panel_snapshots(cx);
        if self.settings.summary().ui_remote_stats_show_users {
            self.refresh_remote_monitor_if_due(
                crate::features::pages::remote::RemoteMonitorKind::Stats,
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::features) fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        self.start_stats_refresh(true, cx);
    }

    pub(in crate::features) fn refresh_stats_auto(&mut self, cx: &mut Context<Self>) {
        self.start_stats_refresh(false, cx);
    }

    fn start_stats_refresh(&mut self, manual: bool, cx: &mut Context<Self>) {
        let context = match self.active_ssh_runtime_context("inspecting stats") {
            Ok(context) => context,
            Err(message) => {
                self.remote_ops.set_stats_status(message);
                self.shell
                    .set_status(self.remote_ops.stats_status().to_string());
                cx.notify();
                return;
            }
        };
        let config = context.config;
        let multiplex = context.multiplex;
        if !config.remote_stats_enabled() {
            self.remote_ops
                .set_stats_status("remote stats are unavailable for network device sessions");
            self.shell
                .set_status(self.remote_ops.stats_status().to_string());
            cx.notify();
            return;
        }
        let job_session_id = context.session_id;
        if self.remote_ops.stats_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_stats_status("stats refresh already running");
            cx.notify();
            return;
        }

        let ticket = self
            .remote_ops
            .begin_stats_job(job_session_id.clone(), manual);
        if self.connection_icon_probe_needed() {
            self.remote_ops
                .begin_icon_probe(&job_session_id, ticket.job_id);
        }
        self.remote_ops.mark_stats_refresh_started();
        self.remote_ops
            .set_stats_status("loading remote system stats");
        let job_id = ticket.job_id;
        let tx = ticket.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = job_session_id.clone();
        let sampling_session_id = job_session_id.clone();
        let sampler = self.remote_ops.stats_sampler();
        let users_revision = self.remote_ops.stats_users_revision();
        let options = RemoteStatsOptions {
            collect_logged_in_users: self.settings.summary().ui_remote_stats_show_users,
        };
        if let Err(error) =
            self.blocking_jobs
                .submit_detached("remote-stats", move |_| {
                    let result = (|| {
                        RemoteStatsService::with_multiplex(config, multiplex)?
                            .snapshot_with_options(sampler.as_ref(), &sampling_session_id, options)
                    })()
                    .map_err(|error| error.to_string());
                    let _ = tx.unbounded_send(StatsJobResult {
                        users_revision,
                        job_id,
                        session_id: job_session_id,
                        result,
                    });
                })
        {
            let _ = rejected_tx.unbounded_send(StatsJobResult {
                users_revision,
                job_id,
                session_id: rejected_session_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn toggle_stats_cpu_expanded(&mut self, cx: &mut Context<Self>) {
        self.remote_ops.toggle_stats_cpu_expanded();
        cx.notify();
    }

    pub(in crate::features) fn select_stats_network_interface(
        &mut self,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let interface = match value {
            "all" => None,
            value => {
                let Some(nic) = value.strip_prefix("nic:") else {
                    return;
                };
                Some(nic)
            }
        };
        let Some(session_id) = self.session.active_id() else {
            return;
        };
        if self
            .remote_ops
            .select_stats_network_interface(session_id, interface)
        {
            self.flush_remote_panel_snapshots(cx);
        }
    }

    /// Deliver stats job replies as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `next_stats_event`, which meant a reply waited for the next tick and
    /// forced `runtime_quiet_tick_allowed` to carry a `remote_ops` term to keep
    /// that wait short.
    pub(in crate::features) fn start_stats_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.remote_ops.take_stats_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.apply_stats_event(event, cx) {
                            cx.notify();
                        }
                        this.sync_remote_panel_demand(cx);
                        if this.remote_ops.stats_users_refresh_pending() {
                            this.refresh_remote_monitor_if_due(
                                crate::features::pages::remote::RemoteMonitorKind::Stats,
                                cx,
                            );
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
    fn apply_stats_event(&mut self, event: StatsJobResult, cx: &mut Context<Self>) -> bool {
        match self
            .remote_ops
            .apply_stats_event(event, self.session.active_id())
        {
            StatsApplyOutcome::Ignored => false,
            StatsApplyOutcome::CompletedInactive => true,
            StatsApplyOutcome::IconProbeCompleted { session_id, stats } => {
                self.apply_auto_detected_connection_icon(&session_id, &stats.system, cx);
                true
            }
            StatsApplyOutcome::Applied {
                session_id,
                stats,
                status,
            } => {
                self.shell.set_status(status);
                // The snapshot is the only place the remote OS is reported,
                // so this is where a connection's icon can be filled in.
                self.apply_auto_detected_connection_icon(&session_id, &stats.system, cx);
                self.cache_asset_stats_snapshot(&session_id, stats.as_ref());
                true
            }
            StatsApplyOutcome::Failed { status } => {
                self.shell.set_status(status);
                true
            }
        }
    }
}
