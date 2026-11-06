//! Admission barrier shared by every window using the same configuration store.

use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Default)]
pub(crate) struct ConfigurationReplacementGate(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    running: Weak<Permit>,
    refresh_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigurationReplacementBlockReason {
    CurrentWindowSession,
    OtherWindowSession,
    Running,
    RefreshRequired,
}

#[derive(Clone)]
pub(crate) struct ConfigurationReplacementLease(Arc<Permit>);

struct Permit {
    gate: ConfigurationReplacementGate,
}

impl ConfigurationReplacementGate {
    pub(crate) fn block_reason(&self) -> Option<ConfigurationReplacementBlockReason> {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.running.strong_count() > 0 {
            Some(ConfigurationReplacementBlockReason::Running)
        } else if state.refresh_required {
            Some(ConfigurationReplacementBlockReason::RefreshRequired)
        } else {
            None
        }
    }

    pub(crate) fn try_acquire(&self) -> Option<ConfigurationReplacementLease> {
        self.acquire(false)
    }

    /// A failed post-commit refresh can be retried without admitting another writer.
    pub(crate) fn try_recover(&self) -> Option<ConfigurationReplacementLease> {
        self.acquire(true)
    }

    fn acquire(&self, recovery: bool) -> Option<ConfigurationReplacementLease> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.running.strong_count() > 0 || state.refresh_required != recovery {
            return None;
        }
        let permit = Arc::new(Permit { gate: self.clone() });
        state.running = Arc::downgrade(&permit);
        Some(ConfigurationReplacementLease(permit))
    }
}

impl ConfigurationReplacementLease {
    /// Set before entering code that may commit, including code that can return
    /// an error after committing. Dropping a worker never clears this latch.
    pub(crate) fn require_refresh(&self) {
        self.0
            .gate
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_required = true;
    }

    pub(crate) fn refresh_completed(&self) {
        self.0
            .gate
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_required = false;
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigurationReplacementBlockReason, ConfigurationReplacementGate};

    #[test]
    fn worker_and_all_window_refreshes_retain_admission() {
        let gate = ConfigurationReplacementGate::default();
        let lease = gate.try_acquire().unwrap();
        let worker = lease.clone();
        lease.require_refresh();
        drop(lease);
        assert!(gate.try_acquire().is_none());
        let window_refresh = worker.clone();
        worker.refresh_completed();
        drop(worker);
        assert_eq!(
            gate.block_reason(),
            Some(ConfigurationReplacementBlockReason::Running)
        );
        drop(window_refresh);
        assert_eq!(gate.block_reason(), None);
    }

    #[test]
    fn failed_refresh_requires_recovery_even_after_origin_window_closes() {
        let gate = ConfigurationReplacementGate::default();
        let lease = gate.try_acquire().unwrap();
        lease.require_refresh();
        drop(lease);
        assert_eq!(
            gate.block_reason(),
            Some(ConfigurationReplacementBlockReason::RefreshRequired)
        );
        assert!(gate.try_acquire().is_none());
        let recovery = gate.try_recover().unwrap();
        recovery.refresh_completed();
        drop(recovery);
        assert!(gate.try_acquire().is_some());
    }

    #[test]
    fn rejected_or_cancelled_job_releases_unstarted_permit() {
        let gate = ConfigurationReplacementGate::default();
        drop(gate.try_acquire().unwrap());
        assert!(gate.try_acquire().is_some());
    }

    #[test]
    fn a_queued_worker_keeps_admission_closed_after_the_ui_owner_is_dropped() {
        use crate::blocking_jobs::BlockingJobScheduler;
        use std::sync::mpsc;
        use std::time::Duration;

        let scheduler = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        scheduler
            .submit_detached("occupied", move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let gate = ConfigurationReplacementGate::default();
        let lease = gate.try_acquire().unwrap();
        let worker = lease.clone();
        let task = scheduler
            .submit_task("configuration-replacement", move |_| {
                worker.require_refresh();
            })
            .unwrap();
        drop(lease);
        assert!(gate.try_acquire().is_none());
        release_tx.send(()).unwrap();
        futures::executor::block_on(task).unwrap();
        assert_eq!(
            gate.block_reason(),
            Some(ConfigurationReplacementBlockReason::RefreshRequired)
        );
        scheduler.shutdown();
    }
}
