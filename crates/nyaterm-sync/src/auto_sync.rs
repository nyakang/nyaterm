//! Automatic synchronization scheduling with caller-supplied monotonic time.
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSyncTrigger {
    Startup,
    Periodic,
    Focus,
    Settings,
    Change,
    DeferredPull,
    Retry,
}

impl AutoSyncTrigger {
    pub fn history_name(self) -> &'static str {
        match self {
            Self::Startup => "auto_startup",
            Self::Periodic => "auto_periodic",
            Self::Focus => "auto_focus",
            Self::Settings => "auto_settings",
            Self::Change => "auto_change",
            Self::DeferredPull => "auto_deferred_pull",
            Self::Retry => "auto_retry",
        }
    }
}

/// Scheduling facts only; reports, persistence and task handles stay with the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSyncCompletion {
    Disabled,
    Skipped,
    Debouncing(Duration),
    Busy,
    RemoteAvailable {
        retry_when_safe: bool,
    },
    Success,
    V4Unverified,
    Conflict {
        overwrite_blocked: bool,
    },
    Failure {
        permanent: bool,
        retry_after: Option<Duration>,
    },
}

impl AutoSyncCompletion {
    pub fn from_error(error: &nyaterm_core::CloudSyncError) -> Self {
        match error {
            nyaterm_core::CloudSyncError::Conflict(preview) => Self::Conflict {
                overwrite_blocked: preview.overwrite_blocked,
            },
            error => Self::Failure {
                permanent: permanent_auto_sync_error(error),
                retry_after: error.retry_after(),
            },
        }
    }
}

pub struct AutoSyncSchedule {
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
}

pub struct AutoSyncJob {
    pub trigger: AutoSyncTrigger,
    pub change_elapsed: Duration,
    pub pull_safe: bool,
    pub remote_available: bool,
}

impl AutoSyncSchedule {
    pub fn cancel_for_shutdown(&mut self, now: Instant) {
        self.running = false;
        self.pending = Some(AutoSyncTrigger::Retry);
        self.next_due = now;
    }

    pub fn next_job(
        &mut self,
        now: Instant,
        generation: u64,
        pull_safe: impl FnOnce() -> bool,
    ) -> Option<AutoSyncJob> {
        if self.running {
            return None;
        }
        self.record_sync_mutation(now, generation);
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
        self.in_flight_generation = self.mutation_generation;
        Some(AutoSyncJob {
            trigger,
            pull_safe,
            remote_available: self.remote_available,
            change_elapsed: self.change_since.map_or(Duration::ZERO, |since| {
                // Zero means there is no pending mutation. A generation
                // first observed during this tick still needs debounce,
                // even when `now` was captured just before its timestamp.
                now.saturating_duration_since(since)
                    .max(Duration::from_nanos(1))
            }),
        })
    }

    pub fn complete(
        &mut self,
        now: Instant,
        trigger: AutoSyncTrigger,
        result: &AutoSyncCompletion,
        observed_generation: Option<u64>,
    ) {
        // Mutation events and job completions arrive independently. Observe the
        // store generation before deciding whether this job cleared all changes.
        if let Some(generation) = observed_generation {
            self.record_sync_mutation(now, generation);
        }
        self.running = false;
        if matches!(
            result,
            AutoSyncCompletion::RemoteAvailable { .. }
                | AutoSyncCompletion::Success
                | AutoSyncCompletion::V4Unverified
        ) || (trigger != AutoSyncTrigger::Change
            && matches!(result, AutoSyncCompletion::Debouncing(_)))
            || matches!(result, AutoSyncCompletion::Conflict { .. })
        {
            self.observed_remote_check(now);
        }
        match result {
            AutoSyncCompletion::V4Unverified => {
                self.pending = Some(AutoSyncTrigger::Retry);
                self.next_due = now + auto_sync_backoff(self.failures);
                self.failures = self.failures.saturating_add(1);
                self.retry_not_before = Some(self.next_due);
            }
            AutoSyncCompletion::Debouncing(remaining) => {
                if matches!(
                    self.pending,
                    None | Some(AutoSyncTrigger::Change | AutoSyncTrigger::Focus)
                ) {
                    self.pending = Some(AutoSyncTrigger::Change);
                    self.next_due = now + *remaining;
                }
            }
            AutoSyncCompletion::Skipped if trigger == AutoSyncTrigger::Change => {
                if self.mutation_generation == self.in_flight_generation {
                    self.change_since = None;
                    if self.pending == Some(AutoSyncTrigger::Change) {
                        self.pending = None;
                    }
                }
            }
            AutoSyncCompletion::Busy => {
                self.pending = Some(trigger);
                self.next_due = now + Duration::from_secs(5);
            }
            AutoSyncCompletion::RemoteAvailable { retry_when_safe } => {
                self.remote_available = *retry_when_safe;
                self.failures = 0;
                self.retry_not_before = None;
            }
            AutoSyncCompletion::Success => {
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
            AutoSyncCompletion::Conflict { overwrite_blocked } => {
                self.remote_available = false;
                if *overwrite_blocked {
                    // The configured master password does not match this device,
                    // so neither side can be reconciled by retrying. Resume when
                    // the settings change, which is how the password is fixed.
                    self.paused = true;
                }
            }
            AutoSyncCompletion::Failure {
                permanent,
                retry_after,
            } => {
                self.paused = *permanent;
                let backoff = auto_sync_backoff(self.failures).max(retry_after.unwrap_or_default());
                self.failures = self.failures.saturating_add(1);
                self.pending = Some(AutoSyncTrigger::Retry);
                self.next_due = now
                    .checked_add(backoff)
                    .unwrap_or(now + Duration::from_secs(86_400 * 365 * 100));
                self.retry_not_before = Some(self.next_due);
            }
            AutoSyncCompletion::Disabled => self.paused = true,
            AutoSyncCompletion::Skipped => {}
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn record_sync_mutation(&mut self, now: Instant, generation: u64) {
        if generation <= self.mutation_generation {
            return;
        }
        self.mutation_generation = generation;
        self.change_since = Some(now);
        if matches!(
            self.pending,
            None | Some(AutoSyncTrigger::Change | AutoSyncTrigger::Focus)
        ) {
            self.pending = Some(AutoSyncTrigger::Change);
            self.next_due = now + Duration::from_secs(1);
        }
    }

    pub fn new(now: Instant, mutation_generation: u64) -> Self {
        Self {
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
        }
    }

    pub fn settings_changed(&mut self, now: Instant) {
        self.paused = false;
        self.failures = 0;
        self.retry_not_before = None;
        self.pending = Some(AutoSyncTrigger::Settings);
        self.next_due = now;
    }

    pub fn observed_remote_check(&mut self, at: Instant) {
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

    pub fn focused(&mut self, now: Instant) {
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
        nyaterm_core::CloudSyncError::Storage { kind, .. } => kind.is_permanent(),
        nyaterm_core::CloudSyncError::Authentication { .. }
        | nyaterm_core::CloudSyncError::Http {
            status: 401 | 403 | 501,
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
            | nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { .. }
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
