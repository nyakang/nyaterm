//! "Refresh this pane if its interval has come due."
//!
//! One method per polling pane, split out of `drive_remote_auto_refresh`, which
//! evaluated all five in one function because one shell-wide clock called it. The
//! *decision* stays here on `NyaTermApp` rather than moving onto the panel entities:
//! it reads `remote_ops`' in-flight flag and `last_refresh_at` plus the settings
//! interval, and `remote_ops` is the authoritative owner of all of that. What the
//! panels own is the schedule that does the asking.
//!
//! Each condition is carried over unchanged, including the details that are easy to
//! read as accidents and are not:
//!
//! * Stats, GPU and NPU refresh when their panel is open **or** when the header
//!   status bar is showing that metric, so demand for them outlives their panel.
//! * The GPU and NPU auto path skips a session latched unavailable
//!   (`refresh_*_auto` rather than `refresh_*`); a manual refresh still tries.
//! * The interval floors differ: one second for stats/GPU/NPU, three for
//!   processes/Docker.
//! * Docker refreshes an open container's details only on the beats where the
//!   overview itself is not due, so one interval does not submit two jobs.

use std::time::{Duration, Instant};

use gpui::Context;

use crate::features::NyaTermApp;

/// Whether `interval_seconds` has elapsed since `last_refresh_at`.
///
/// A pane that has never refreshed is always due, which is what makes the first paint
/// after a panel opens fetch immediately instead of waiting out an interval.
pub(in crate::features) fn remote_refresh_due(
    last_refresh_at: Option<Instant>,
    interval_seconds: u32,
) -> bool {
    last_refresh_at.is_none_or(|last_refresh_at| {
        last_refresh_at.elapsed() >= Duration::from_secs(u64::from(interval_seconds))
    })
}

impl NyaTermApp {
    pub(in crate::features) fn refresh_stats_if_due(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.summary().ui_show_remote_stats || self.remote_ops.stats_is_pending() {
            return false;
        }
        if self.remote_ops.take_stats_warmup_retry_due()
            || remote_refresh_due(
                self.remote_ops.stats_last_refresh_at(),
                self.settings.summary().ui_remote_stats_interval.max(1),
            )
        {
            self.refresh_stats_auto(cx);
            return true;
        }
        false
    }

    pub(in crate::features) fn refresh_gpu_if_due(&mut self, cx: &mut Context<Self>) -> bool {
        if self.settings.summary().ui_show_gpu_monitor
            && !self.remote_ops.gpu_is_pending()
            && remote_refresh_due(
                self.remote_ops.gpu_last_refresh_at(),
                self.settings.summary().ui_gpu_monitor_interval.max(1),
            )
        {
            self.refresh_gpu_auto(cx);
            return true;
        }
        false
    }

    pub(in crate::features) fn refresh_npu_if_due(&mut self, cx: &mut Context<Self>) -> bool {
        if self.settings.summary().ui_show_ascend_npu_monitor
            && !self.remote_ops.npu_is_pending()
            && remote_refresh_due(
                self.remote_ops.npu_last_refresh_at(),
                self.settings
                    .summary()
                    .ui_ascend_npu_monitor_interval
                    .max(1),
            )
        {
            self.refresh_npu_auto(cx);
            return true;
        }
        false
    }

    pub(in crate::features) fn refresh_processes_if_due(&mut self, cx: &mut Context<Self>) -> bool {
        if self.settings.summary().ui_show_process_manager
            && !self.remote_ops.process_is_pending()
            && remote_refresh_due(
                self.remote_ops.process_last_refresh_at(),
                self.settings.summary().ui_process_manager_interval.max(3),
            )
        {
            self.refresh_processes(cx);
            return true;
        }
        false
    }

    /// The overview and visible resource, or an open container's details on the off beats.
    pub(in crate::features) fn refresh_docker_if_due(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.summary().ui_show_docker_manager || self.remote_ops.docker_is_pending() {
            return false;
        }
        let interval = self.settings.summary().ui_docker_manager_interval.max(3);
        if remote_refresh_due(self.remote_ops.docker_last_refresh_at(), interval) {
            self.refresh_docker(cx);
            return true;
        }
        if self.remote_ops.docker_resource_load_due(interval) {
            self.refresh_docker_resource(cx);
            return true;
        }
        if let Some((container_id, last_refresh_at)) = self.remote_ops.docker_details_refresh()
            && remote_refresh_due(Some(last_refresh_at), interval)
        {
            self.load_docker_details(container_id, cx);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::remote_refresh_due;

    /// A pane that has never fetched must be due, so opening a panel fetches at once
    /// rather than after an interval of blank.
    #[test]
    fn a_pane_that_never_refreshed_is_due() {
        assert!(remote_refresh_due(None, 3));
        assert!(remote_refresh_due(None, 0));
    }

    /// The comparison is `>=`, so a refresh exactly on the boundary goes.
    #[test]
    fn the_interval_boundary_counts_as_due() {
        let now = Instant::now();
        assert!(!remote_refresh_due(Some(now), 3));
        assert!(!remote_refresh_due(
            Some(now - Duration::from_millis(2_999)),
            3
        ));
        assert!(remote_refresh_due(Some(now - Duration::from_secs(3)), 3));
        // A zero interval is what an unclamped setting would produce; the callers
        // floor it, and this pins that the helper itself does not.
        assert!(remote_refresh_due(Some(now), 0));
    }
}
