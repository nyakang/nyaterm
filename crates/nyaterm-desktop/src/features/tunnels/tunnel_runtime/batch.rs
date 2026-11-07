use rust_i18n::t;

use gpui::Context;
use nyaterm_ui::notification::NyaNotificationKind;

use crate::features::NyaTermApp;
use crate::features::tunnels::batch::{TunnelBatchAction, TunnelBatchBegin, TunnelBatchReport};

/// Failures listed by name in a batch summary before the rest are counted.
const TUNNEL_BATCH_REPORTED_FAILURES: usize = 3;

impl NyaTermApp {
    pub(in crate::features) fn start_tunnel_group(
        &mut self,
        group_id: String,
        cx: &mut Context<Self>,
    ) {
        self.begin_tunnel_group_batch(group_id, TunnelBatchAction::Start, cx);
    }

    pub(in crate::features) fn stop_tunnel_group(
        &mut self,
        group_id: String,
        cx: &mut Context<Self>,
    ) {
        self.begin_tunnel_group_batch(group_id, TunnelBatchAction::Stop, cx);
    }

    fn begin_tunnel_group_batch(
        &mut self,
        group_id: String,
        action: TunnelBatchAction,
        cx: &mut Context<Self>,
    ) {
        let Some(label) = self.tunnel_group_name(&group_id) else {
            self.shell
                .set_status("tunnel group is no longer available".to_string());
            cx.notify();
            return;
        };
        match self.tunnel_state.begin_group_batch(&group_id, action) {
            TunnelBatchBegin::Started => {
                self.shell.set_status(match action {
                    TunnelBatchAction::Start => format!("starting tunnels in group '{label}'"),
                    TunnelBatchAction::Stop => format!("stopping tunnels in group '{label}'"),
                });
                self.pump_tunnel_batches(cx);
            }
            TunnelBatchBegin::AlreadyRunning => {
                self.shell.set_status(format!(
                    "tunnel group '{label}' batch operation is already running"
                ));
            }
            TunnelBatchBegin::Empty => {
                self.shell
                    .set_status(format!("tunnel group '{label}' has no tunnels"));
            }
        }
        cx.notify();
    }

    fn tunnel_group_name(&self, group_id: &str) -> Option<String> {
        self.tunnel_state
            .tunnel_groups()
            .iter()
            .find(|group| group.id == group_id)
            .map(|group| group.name.clone())
    }

    /// Dispatch queued batch work into free slots and report finished batches.
    ///
    /// Runs on batch start and after every tunnel result, so at most
    /// `TUNNEL_BATCH_MAX_IN_FLIGHT` batch jobs ever occupy the blocking pool.
    pub(super) fn pump_tunnel_batches(&mut self, cx: &mut Context<Self>) {
        loop {
            let dispatches = self.tunnel_state.next_batch_dispatches();
            if dispatches.is_empty() {
                break;
            }
            for dispatch in dispatches {
                let submitted = match dispatch.action {
                    TunnelBatchAction::Start => match self.tunnel_state.tunnel(&dispatch.tunnel_id)
                    {
                        Some(tunnel) => self.submit_tunnel_open(tunnel.clone()),
                        None => Err("tunnel profile is no longer available".to_string()),
                    },
                    TunnelBatchAction::Stop => self.submit_tunnel_close(dispatch.tunnel_id.clone()),
                };
                if let Err(error) = submitted {
                    self.tunnel_state
                        .record_batch_dispatch_rejected(&dispatch.tunnel_id, error);
                }
            }
        }

        for report in self.tunnel_state.take_completed_batches() {
            self.report_tunnel_batch(report, cx);
        }
    }

    fn report_tunnel_batch(&mut self, report: TunnelBatchReport, cx: &mut Context<Self>) {
        let name = self
            .tunnel_group_name(&report.group_id)
            .unwrap_or_else(|| report.group_id.clone());
        let failed = report.failures.len();
        let mut message = match report.action {
            TunnelBatchAction::Start => t!(
                "network.batchStartSummary",
                name = name,
                succeeded = report.succeeded,
                skipped = report.skipped,
                failed = failed
            ),
            TunnelBatchAction::Stop => t!(
                "network.batchStopSummary",
                name = name,
                succeeded = report.succeeded,
                skipped = report.skipped,
                failed = failed
            ),
        }
        .to_string();
        for failure in report.failures.iter().take(TUNNEL_BATCH_REPORTED_FAILURES) {
            message.push('\n');
            message.push_str(&format!("{}: {}", failure.name, failure.error));
        }
        if failed > TUNNEL_BATCH_REPORTED_FAILURES {
            message.push('\n');
            message.push_str(
                t!(
                    "network.batchMoreFailures",
                    count = failed - TUNNEL_BATCH_REPORTED_FAILURES
                )
                .as_ref(),
            );
        }

        let kind = if failed == 0 {
            NyaNotificationKind::Success
        } else if report.succeeded > 0 || report.skipped > 0 {
            NyaNotificationKind::Warning
        } else {
            NyaNotificationKind::Error
        };
        self.notify_background_operation("tunnel-batch", kind, message, cx);
        self.shell.set_status(format!(
            "tunnel group '{name}': {} succeeded, {} skipped, {failed} failed",
            report.succeeded, report.skipped
        ));
    }
}
