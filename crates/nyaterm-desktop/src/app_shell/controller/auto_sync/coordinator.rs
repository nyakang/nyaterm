use crate::features::{AutoSyncResult, AutoSyncTrigger};
pub(super) use nyaterm_sync::auto_sync::AutoSyncJob;
use nyaterm_sync::auto_sync::{AutoSyncCompletion, AutoSyncSchedule};
use std::time::{Duration, Instant};

pub(in crate::app_shell::controller) struct AutoSyncCoordinator {
    pub(in crate::app_shell::controller) operations:
        crate::features::sync::operation::CloudSyncOperationRegistry,
    schedule: AutoSyncSchedule,
    started_at: Option<Instant>,
    history_errors: std::collections::HashMap<String, Instant>,
    had_error: bool,
}

impl AutoSyncCoordinator {
    pub(in crate::app_shell::controller) fn new(generation: u64) -> Self {
        Self {
            operations: Default::default(),
            schedule: AutoSyncSchedule::new(Instant::now(), generation),
            started_at: None,
            history_errors: Default::default(),
            had_error: false,
        }
    }

    pub(super) fn cancel_for_shutdown(&mut self) {
        self.schedule.cancel_for_shutdown(Instant::now());
    }

    pub(in crate::app_shell::controller) fn running(&self) -> bool {
        self.schedule.running()
    }

    pub(super) fn record_sync_mutation(&mut self, generation: u64) {
        self.schedule
            .record_sync_mutation(Instant::now(), generation);
    }

    pub(in crate::app_shell::controller) fn settings_changed(&mut self) {
        self.schedule.settings_changed(Instant::now());
    }

    pub(in crate::app_shell::controller) fn observed_remote_check(&mut self, at: Instant) {
        self.schedule.observed_remote_check(at);
    }

    pub(in crate::app_shell::controller) fn focused(&mut self) {
        self.schedule.focused(Instant::now());
    }

    pub(super) fn next_job(
        &mut self,
        now: Instant,
        generation: u64,
        pull_safe: impl FnOnce() -> bool,
    ) -> Option<AutoSyncJob> {
        let job = self.schedule.next_job(now, generation, pull_safe)?;
        self.started_at = Some(now);
        Some(job)
    }

    pub(super) fn complete(
        &mut self,
        trigger: AutoSyncTrigger,
        result: &Result<AutoSyncResult, nyaterm_core::CloudSyncError>,
        observed_generation: Option<u64>,
    ) {
        let completion = match result {
            Ok(AutoSyncResult::V4(report)) if !report.verified => AutoSyncCompletion::V4Unverified,
            Ok(AutoSyncResult::V4(_) | AutoSyncResult::Synced(_) | AutoSyncResult::UpToDate(_)) => {
                AutoSyncCompletion::Success
            }
            Ok(AutoSyncResult::Disabled) => AutoSyncCompletion::Disabled,
            Ok(AutoSyncResult::Skipped) => AutoSyncCompletion::Skipped,
            Ok(AutoSyncResult::Debouncing(remaining)) => AutoSyncCompletion::Debouncing(*remaining),
            Ok(AutoSyncResult::Busy) => AutoSyncCompletion::Busy,
            Ok(AutoSyncResult::RemoteAvailable {
                retry_when_safe, ..
            }) => AutoSyncCompletion::RemoteAvailable {
                retry_when_safe: *retry_when_safe,
            },
            Err(error) => AutoSyncCompletion::from_error(error),
        };
        self.schedule
            .complete(Instant::now(), trigger, &completion, observed_generation);
    }

    pub(super) fn history_entry(
        &mut self,
        trigger: AutoSyncTrigger,
        result: &Result<AutoSyncResult, nyaterm_core::CloudSyncError>,
    ) -> Option<nyaterm_core::CloudSyncHistoryEntry> {
        use nyaterm_core::{CloudSyncHistoryEntry, CloudSyncOutcome};
        let now = Instant::now();
        let duration = self
            .started_at
            .take()
            .map(|start| now.duration_since(start).as_millis() as u64);
        let mut entry = match result {
            Ok(AutoSyncResult::Synced(sync)) if sync.outcome != CloudSyncOutcome::UpToDate => {
                self.had_error = false;
                CloudSyncHistoryEntry::sync(
                    "success",
                    trigger.history_name(),
                    Some(sync.status.provider.clone()),
                    sync.pointer.as_ref().map(|p| p.revision_id.clone()),
                    match sync.outcome {
                        CloudSyncOutcome::Uploaded => "Cloud snapshot uploaded",
                        CloudSyncOutcome::Downloaded => "Cloud snapshot downloaded",
                        _ => "Cloud snapshot recovered",
                    },
                )
            }
            Ok(AutoSyncResult::UpToDate(_))
            | Ok(AutoSyncResult::Synced(_))
            | Ok(AutoSyncResult::RemoteAvailable { .. })
                if self.had_error =>
            {
                self.had_error = false;
                CloudSyncHistoryEntry::sync(
                    "success",
                    trigger.history_name(),
                    None,
                    match result {
                        Ok(AutoSyncResult::UpToDate(state)) => {
                            state.last_applied_remote_revision.clone()
                        }
                        Ok(AutoSyncResult::Synced(sync)) => {
                            sync.pointer.as_ref().map(|p| p.revision_id.clone())
                        }
                        Ok(AutoSyncResult::RemoteAvailable { revision, .. }) => revision.clone(),
                        _ => None,
                    },
                    "Cloud synchronization recovered",
                )
            }
            Err(error) => {
                self.had_error = true;
                let revision = match error {
                    nyaterm_core::CloudSyncError::Conflict(p) => Some(p.remote_revision.clone()),
                    _ => None,
                };
                let key = format!(
                    "{}:{}:{}",
                    trigger.history_name(),
                    error.classification(),
                    revision.as_deref().unwrap_or_default()
                );
                self.history_errors
                    .retain(|_, at| now.duration_since(*at) < Duration::from_secs(900));
                if self.history_errors.contains_key(&key) {
                    return None;
                }
                self.history_errors.insert(key, now);
                // Error classifications are controlled strings, never raw transport diagnostics.
                CloudSyncHistoryEntry::sync(
                    if matches!(error, nyaterm_core::CloudSyncError::Conflict(_)) {
                        "conflict"
                    } else {
                        "failed"
                    },
                    trigger.history_name(),
                    None,
                    revision,
                    error.classification(),
                )
            }
            _ => return None,
        };
        entry.duration_ms = duration;
        Some(entry)
    }
}

#[cfg(test)]
mod tests;
