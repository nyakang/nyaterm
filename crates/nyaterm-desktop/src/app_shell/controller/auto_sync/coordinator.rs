use crate::features::{AutoSyncResult, AutoSyncTrigger};
use std::time::{Duration, Instant};

pub(in crate::app_shell::controller) struct AutoSyncCoordinator {
    pub(in crate::app_shell::controller) operations:
        crate::features::sync::operation::CloudSyncOperationRegistry,
    running: bool,
    pending: Option<AutoSyncTrigger>,
    next_due: Instant,
    next_periodic: Instant,
    last_focus: Option<Instant>,
    last_remote_check: Option<Instant>,
    mutation_generation: u64,
    in_flight_generation: u64,
    change_since: Option<Instant>,
    remote_available: bool,
    failures: usize,
    paused: bool,
    retry_not_before: Option<Instant>,
    started_at: Option<Instant>,
    history_errors: std::collections::HashMap<String, Instant>,
    had_error: bool,
}

pub(super) struct AutoSyncJob {
    pub(super) trigger: AutoSyncTrigger,
    pub(super) change_elapsed: Duration,
    pub(super) pull_safe: bool,
}

impl AutoSyncCoordinator {
    pub(super) fn cancel_for_shutdown(&mut self) {
        self.running = false;
        self.pending = Some(AutoSyncTrigger::Retry);
        self.next_due = Instant::now();
    }

    pub(super) fn next_job(
        &mut self,
        now: Instant,
        generation: u64,
        pull_safe: impl FnOnce() -> bool,
    ) -> Option<AutoSyncJob> {
        if self.running {
            return None;
        }
        self.record_sync_mutation(generation);
        if now >= self.next_periodic
            && matches!(
                self.pending,
                None | Some(AutoSyncTrigger::Change | AutoSyncTrigger::Focus)
            )
        {
            self.pending = Some(AutoSyncTrigger::Periodic);
            self.next_due = now;
        }
        if self.paused || self.retry_not_before.is_some_and(|deadline| now < deadline) {
            return None;
        }
        let deferred_pull = self.remote_available && self.pending.is_none();
        if !deferred_pull && (now < self.next_due || self.pending.is_none()) {
            return None;
        }
        // Inspect windows only when a job can start, preserving the idle tick cost.
        let pull_safe = pull_safe();
        if deferred_pull && pull_safe {
            self.pending = Some(AutoSyncTrigger::DeferredPull);
            self.next_due = now;
        }
        if now < self.next_due {
            return None;
        }
        let trigger = self.pending.take()?;
        self.running = true;
        self.started_at = Some(now);
        self.in_flight_generation = self.mutation_generation;
        Some(AutoSyncJob {
            trigger,
            pull_safe,
            change_elapsed: self.change_since.map_or(Duration::ZERO, |since| {
                // Zero means there is no pending mutation. A generation
                // first observed during this tick still needs debounce,
                // even when `now` was captured just before its timestamp.
                now.saturating_duration_since(since)
                    .max(Duration::from_nanos(1))
            }),
        })
    }

    pub(super) fn complete(
        &mut self,
        trigger: AutoSyncTrigger,
        result: &Result<AutoSyncResult, nyaterm_core::CloudSyncError>,
        observed_generation: Option<u64>,
    ) {
        // Mutation events and job completions arrive independently. Observe the
        // store generation before deciding whether this job cleared all changes.
        if let Some(generation) = observed_generation {
            self.record_sync_mutation(generation);
        }
        let now = Instant::now();
        self.running = false;
        if matches!(
            result,
            Ok(AutoSyncResult::RemoteAvailable { .. }
                | AutoSyncResult::Synced(_)
                | AutoSyncResult::UpToDate(_)
                | AutoSyncResult::V4(_))
        ) || (trigger != AutoSyncTrigger::Change
            && matches!(result, Ok(AutoSyncResult::Debouncing(_))))
            || matches!(result, Err(nyaterm_core::CloudSyncError::Conflict(_)))
        {
            self.observed_remote_check(now);
        }
        match result {
            Ok(AutoSyncResult::V4(report)) if !report.verified => {
                self.pending = Some(AutoSyncTrigger::Retry);
                self.next_due = now + auto_sync_backoff(self.failures);
                self.failures = self.failures.saturating_add(1);
                self.retry_not_before = Some(self.next_due);
            }
            Ok(AutoSyncResult::Debouncing(remaining)) => {
                if matches!(
                    self.pending,
                    None | Some(AutoSyncTrigger::Change | AutoSyncTrigger::Focus)
                ) {
                    self.pending = Some(AutoSyncTrigger::Change);
                    self.next_due = now + *remaining;
                }
            }
            Ok(AutoSyncResult::Skipped) if trigger == AutoSyncTrigger::Change => {
                if self.mutation_generation == self.in_flight_generation {
                    self.change_since = None;
                    if self.pending == Some(AutoSyncTrigger::Change) {
                        self.pending = None;
                    }
                }
            }
            Ok(AutoSyncResult::Busy) => {
                self.pending = Some(trigger);
                self.next_due = now + Duration::from_secs(5);
            }
            Ok(AutoSyncResult::RemoteAvailable {
                retry_when_safe, ..
            }) => {
                self.remote_available = *retry_when_safe;
                self.failures = 0;
                self.retry_not_before = None;
            }
            Ok(AutoSyncResult::Synced(_))
            | Ok(AutoSyncResult::UpToDate(_))
            | Ok(AutoSyncResult::V4(_)) => {
                self.remote_available = false;
                self.failures = 0;
                self.retry_not_before = None;
                if self.mutation_generation == self.in_flight_generation {
                    self.change_since = None;
                    if self.pending == Some(AutoSyncTrigger::Change) {
                        self.pending = None;
                    }
                }
            }
            Err(nyaterm_core::CloudSyncError::Conflict(preview)) => {
                self.remote_available = false;
                if preview.overwrite_blocked {
                    // The configured master password does not match this device,
                    // so neither side can be reconciled by retrying. Resume when
                    // the settings change, which is how the password is fixed.
                    self.paused = true;
                }
            }
            Err(error) => {
                self.paused = permanent_auto_sync_error(error);
                let backoff =
                    auto_sync_backoff(self.failures).max(error.retry_after().unwrap_or_default());
                self.failures = self.failures.saturating_add(1);
                self.pending = Some(AutoSyncTrigger::Retry);
                self.next_due = now
                    .checked_add(backoff)
                    .unwrap_or(now + Duration::from_secs(86_400 * 365 * 100));
                self.retry_not_before = Some(self.next_due);
            }
            Ok(AutoSyncResult::Disabled) => self.paused = true,
            Ok(AutoSyncResult::Skipped) => {}
        }
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

    pub(in crate::app_shell::controller) fn running(&self) -> bool {
        self.running
    }

    pub(super) fn record_sync_mutation(&mut self, generation: u64) {
        if generation <= self.mutation_generation {
            return;
        }
        self.mutation_generation = generation;
        let now = Instant::now();
        self.change_since = Some(now);
        if matches!(
            self.pending,
            None | Some(AutoSyncTrigger::Change | AutoSyncTrigger::Focus)
        ) {
            self.pending = Some(AutoSyncTrigger::Change);
            self.next_due = now + Duration::from_secs(1);
        }
    }

    pub(in crate::app_shell::controller) fn new(mutation_generation: u64) -> Self {
        let now = Instant::now();
        Self {
            operations: Default::default(),
            running: false,
            pending: Some(AutoSyncTrigger::Startup),
            next_due: now + Duration::from_secs(3),
            next_periodic: now + Duration::from_secs(15 * 60),
            last_focus: None,
            last_remote_check: None,
            mutation_generation,
            in_flight_generation: mutation_generation,
            change_since: None,
            remote_available: false,
            failures: 0,
            paused: false,
            retry_not_before: None,
            started_at: None,
            history_errors: Default::default(),
            had_error: false,
        }
    }

    pub(in crate::app_shell::controller) fn settings_changed(&mut self) {
        self.paused = false;
        self.failures = 0;
        self.retry_not_before = None;
        self.pending = Some(AutoSyncTrigger::Settings);
        self.next_due = Instant::now();
    }

    pub(in crate::app_shell::controller) fn observed_remote_check(&mut self, at: Instant) {
        if self.last_remote_check.is_none_or(|old| at > old) {
            self.last_remote_check = Some(at);
            self.next_periodic = at + Duration::from_secs(15 * 60);
            if self.pending == Some(AutoSyncTrigger::Focus) {
                self.pending = None;
            }
            if self.pending == Some(AutoSyncTrigger::Periodic) {
                self.pending = self.change_since.map(|_| AutoSyncTrigger::Change);
                self.next_due = at + Duration::from_secs(1);
            }
        }
    }

    pub(in crate::app_shell::controller) fn focused(&mut self) {
        let now = Instant::now();
        if self
            .last_remote_check
            .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(120))
        {
            return;
        }
        if self
            .last_focus
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(120))
        {
            self.last_focus = Some(now);
            if self.pending.is_none() {
                self.pending = Some(AutoSyncTrigger::Focus);
                self.next_due = now;
            }
        }
    }
}

fn permanent_auto_sync_error(error: &nyaterm_core::CloudSyncError) -> bool {
    match error {
        nyaterm_core::CloudSyncError::Authentication { .. }
        | nyaterm_core::CloudSyncError::Http {
            status: 401 | 403,
            retry_after_seconds: None,
            ..
        }
        | nyaterm_core::CloudSyncError::CredentialPersistence => true,
        nyaterm_core::CloudSyncError::LocalStore(message)
            if message == "crypto" || message == "invalid_data" =>
        {
            true
        }
        nyaterm_core::CloudSyncError::Disabled
        | nyaterm_core::CloudSyncError::PortableSnapshot(
            nyaterm_core::PortableSnapshotError::MissingMasterPassword
            | nyaterm_core::PortableSnapshotError::UnsupportedEncryptionVersion(_)
            | nyaterm_core::PortableSnapshotError::InvalidEncryptionEnvelope
            | nyaterm_core::PortableSnapshotError::UnsupportedVersion(_)
            | nyaterm_core::PortableSnapshotError::Decrypt { .. },
        ) => true,
        nyaterm_core::CloudSyncError::Remote(message) => {
            let message = message.to_ascii_lowercase();
            [
                "401",
                "403",
                "unauthorized",
                "forbidden",
                "invalid token",
                "missing token",
                "access token is required",
                "endpoint is required",
                "bucket is required",
                "credential is required",
            ]
            .iter()
            .any(|marker| message.contains(marker))
        }
        _ => false,
    }
}

fn auto_sync_backoff(failures: usize) -> Duration {
    Duration::from_secs([1, 5, 15, 60][failures.min(3)] * 60)
}

#[cfg(test)]
mod tests;
