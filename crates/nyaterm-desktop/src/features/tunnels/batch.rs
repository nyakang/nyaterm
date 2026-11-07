//! Group-scoped batch start/stop coordination for SSH tunnels.
//!
//! This is pure bookkeeping: it decides which single-tunnel jobs to dispatch
//! next and aggregates their outcomes. Dispatch stays with `NyaTermApp`, which
//! reuses the ordinary single-tunnel job path, so a batch never owns an SSH
//! transport and never blocks a GPUI callback.

use std::collections::{HashMap, HashSet, VecDeque};

/// Batch jobs share the application-wide blocking pool with store and file
/// work, and a tunnel open can wait up to 30 seconds for its SSH handshake.
/// Keeping batches well below the pool size leaves workers for everything else.
pub(in crate::features) const TUNNEL_BATCH_MAX_IN_FLIGHT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum TunnelBatchAction {
    Start,
    Stop,
}

/// Runtime state of one tunnel as seen by the batch at a decision point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum TunnelRuntimeState {
    Closed,
    Open,
    /// An open or close job is already running for this tunnel.
    Pending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct TunnelBatchMember {
    pub(in crate::features) id: String,
    pub(in crate::features) name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct TunnelBatchFailure {
    pub(in crate::features) tunnel_id: String,
    pub(in crate::features) name: String,
    pub(in crate::features) error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct TunnelBatchReport {
    pub(in crate::features) group_id: String,
    pub(in crate::features) action: TunnelBatchAction,
    pub(in crate::features) succeeded: usize,
    pub(in crate::features) skipped: usize,
    pub(in crate::features) failures: Vec<TunnelBatchFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum TunnelBatchBegin {
    Started,
    /// The same action is already running for the group; nothing changed.
    AlreadyRunning,
    /// The group has no tunnels.
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct TunnelBatchDispatch {
    pub(in crate::features) action: TunnelBatchAction,
    pub(in crate::features) tunnel_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TunnelBatchProgress {
    pub(in crate::features) action: TunnelBatchAction,
    pub(in crate::features) done: usize,
    pub(in crate::features) total: usize,
}

struct TunnelGroupBatch {
    group_id: String,
    action: TunnelBatchAction,
    names: HashMap<String, String>,
    /// Waiting for a free dispatch slot.
    queued: VecDeque<String>,
    /// Dispatched by this batch; their results are owned by the batch.
    in_flight: HashSet<String>,
    /// Busy with a job the batch did not start. Re-evaluated once that job
    /// finishes, so a stop never reports a still-starting tunnel as closed.
    awaiting: HashSet<String>,
    succeeded: usize,
    skipped: usize,
    failures: Vec<TunnelBatchFailure>,
}

impl TunnelGroupBatch {
    fn is_finished(&self) -> bool {
        self.queued.is_empty() && self.in_flight.is_empty() && self.awaiting.is_empty()
    }

    fn name(&self, tunnel_id: &str) -> String {
        self.names
            .get(tunnel_id)
            .cloned()
            .unwrap_or_else(|| tunnel_id.to_string())
    }

    /// Route a tunnel that is not in flight according to its current state.
    fn classify(&mut self, tunnel_id: String, state: TunnelRuntimeState) {
        match (self.action, state) {
            (_, TunnelRuntimeState::Pending) => {
                self.awaiting.insert(tunnel_id);
            }
            (TunnelBatchAction::Start, TunnelRuntimeState::Open)
            | (TunnelBatchAction::Stop, TunnelRuntimeState::Closed) => self.skipped += 1,
            (TunnelBatchAction::Start, TunnelRuntimeState::Closed)
            | (TunnelBatchAction::Stop, TunnelRuntimeState::Open) => {
                self.queued.push_back(tunnel_id);
            }
        }
    }

    fn record_failure(&mut self, tunnel_id: &str, error: String) {
        self.failures.push(TunnelBatchFailure {
            tunnel_id: tunnel_id.to_string(),
            name: self.name(tunnel_id),
            error,
        });
    }
}

/// All running group batches. Owned by `TunnelFeatureState`.
#[derive(Default)]
pub(in crate::features) struct TunnelBatchCoordinator {
    batches: Vec<TunnelGroupBatch>,
}

impl TunnelBatchCoordinator {
    #[cfg(test)]
    fn is_running(&self, group_id: &str) -> bool {
        self.batches.iter().any(|batch| batch.group_id == group_id)
    }

    pub(in crate::features) fn progress(&self, group_id: &str) -> Option<TunnelBatchProgress> {
        self.batches
            .iter()
            .find(|batch| batch.group_id == group_id)
            .map(|batch| TunnelBatchProgress {
                action: batch.action,
                done: batch.succeeded + batch.skipped + batch.failures.len(),
                total: batch.names.len(),
            })
    }

    fn in_flight_count(&self) -> usize {
        self.batches.iter().map(|batch| batch.in_flight.len()).sum()
    }

    /// Start a batch for exactly the given members.
    ///
    /// The opposite action supersedes a running batch: its queued work is
    /// dropped, and its in-flight jobs become pending tunnels that the new
    /// batch waits for before deciding what to do with them.
    pub(in crate::features) fn begin(
        &mut self,
        group_id: &str,
        action: TunnelBatchAction,
        members: Vec<TunnelBatchMember>,
        state: impl Fn(&str) -> TunnelRuntimeState,
    ) -> TunnelBatchBegin {
        if let Some(index) = self
            .batches
            .iter()
            .position(|batch| batch.group_id == group_id)
        {
            if self.batches[index].action == action {
                return TunnelBatchBegin::AlreadyRunning;
            }
            self.batches.remove(index);
        }
        if members.is_empty() {
            return TunnelBatchBegin::Empty;
        }

        let mut batch = TunnelGroupBatch {
            group_id: group_id.to_string(),
            action,
            names: HashMap::new(),
            queued: VecDeque::new(),
            in_flight: HashSet::new(),
            awaiting: HashSet::new(),
            succeeded: 0,
            skipped: 0,
            failures: Vec::new(),
        };
        for member in members {
            if batch.names.contains_key(&member.id) {
                continue;
            }
            let runtime = state(&member.id);
            batch.names.insert(member.id.clone(), member.name);
            batch.classify(member.id, runtime);
        }
        self.batches.push(batch);
        TunnelBatchBegin::Started
    }

    /// Drop a group's batch without reporting it, e.g. when the group is deleted.
    pub(in crate::features) fn cancel_group(&mut self, group_id: &str) -> bool {
        let before = self.batches.len();
        self.batches.retain(|batch| batch.group_id != group_id);
        self.batches.len() != before
    }

    /// Take queued tunnels up to the global in-flight limit.
    ///
    /// State is re-read at dispatch time because a queued tunnel may have been
    /// toggled individually, or by auto-open, since the batch began.
    pub(in crate::features) fn next_dispatches(
        &mut self,
        state: impl Fn(&str) -> TunnelRuntimeState,
    ) -> Vec<TunnelBatchDispatch> {
        let mut slots = TUNNEL_BATCH_MAX_IN_FLIGHT.saturating_sub(self.in_flight_count());
        let mut dispatches = Vec::new();
        for batch in &mut self.batches {
            while slots > 0 {
                let Some(tunnel_id) = batch.queued.pop_front() else {
                    break;
                };
                let runtime = state(&tunnel_id);
                let ready = matches!(
                    (batch.action, runtime),
                    (TunnelBatchAction::Start, TunnelRuntimeState::Closed)
                        | (TunnelBatchAction::Stop, TunnelRuntimeState::Open)
                );
                if !ready {
                    batch.classify(tunnel_id, runtime);
                    continue;
                }
                batch.in_flight.insert(tunnel_id.clone());
                dispatches.push(TunnelBatchDispatch {
                    action: batch.action,
                    tunnel_id,
                });
                slots -= 1;
            }
        }
        dispatches
    }

    /// The dispatched job could not be submitted at all.
    pub(in crate::features) fn record_dispatch_rejected(&mut self, tunnel_id: &str, error: String) {
        if let Some(batch) = self
            .batches
            .iter_mut()
            .find(|batch| batch.in_flight.contains(tunnel_id))
        {
            batch.in_flight.remove(tunnel_id);
            batch.record_failure(tunnel_id, error);
        }
    }

    /// Apply a finished tunnel job. Returns whether a batch dispatched it, in
    /// which case the batch report replaces any per-tunnel notification.
    pub(in crate::features) fn job_finished(
        &mut self,
        tunnel_id: &str,
        outcome: Result<(), &str>,
        state_after: TunnelRuntimeState,
    ) -> bool {
        let mut owned = false;
        for batch in &mut self.batches {
            if batch.in_flight.remove(tunnel_id) {
                owned = true;
                match outcome {
                    Ok(()) => batch.succeeded += 1,
                    Err(error) => batch.record_failure(tunnel_id, error.to_string()),
                }
            } else if batch.awaiting.remove(tunnel_id) {
                batch.classify(tunnel_id.to_string(), state_after);
            }
        }
        owned
    }

    pub(in crate::features) fn take_completed(&mut self) -> Vec<TunnelBatchReport> {
        let mut reports = Vec::new();
        self.batches.retain_mut(|batch| {
            if !batch.is_finished() {
                return true;
            }
            reports.push(TunnelBatchReport {
                group_id: std::mem::take(&mut batch.group_id),
                action: batch.action,
                succeeded: batch.succeeded,
                skipped: batch.skipped,
                failures: std::mem::take(&mut batch.failures),
            });
            false
        });
        reports
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{
        TUNNEL_BATCH_MAX_IN_FLIGHT, TunnelBatchAction, TunnelBatchBegin, TunnelBatchCoordinator,
        TunnelBatchMember, TunnelRuntimeState,
    };

    fn members(ids: &[&str]) -> Vec<TunnelBatchMember> {
        ids.iter()
            .map(|id| TunnelBatchMember {
                id: id.to_string(),
                name: format!("name-{id}"),
            })
            .collect()
    }

    struct Runtime(RefCell<HashMap<String, TunnelRuntimeState>>);

    impl Runtime {
        fn new(states: &[(&str, TunnelRuntimeState)]) -> Self {
            Self(RefCell::new(
                states
                    .iter()
                    .map(|(id, state)| (id.to_string(), *state))
                    .collect(),
            ))
        }

        fn get(&self, id: &str) -> TunnelRuntimeState {
            self.0
                .borrow()
                .get(id)
                .copied()
                .unwrap_or(TunnelRuntimeState::Closed)
        }

        fn set(&self, id: &str, state: TunnelRuntimeState) {
            self.0.borrow_mut().insert(id.to_string(), state);
        }
    }

    #[test]
    fn start_skips_open_tunnels_and_reports_partial_failure_without_rollback() {
        let runtime = Runtime::new(&[("a", TunnelRuntimeState::Open)]);
        let mut batches = TunnelBatchCoordinator::default();
        assert_eq!(
            batches.begin(
                "g1",
                TunnelBatchAction::Start,
                members(&["a", "b", "c"]),
                |id| runtime.get(id)
            ),
            TunnelBatchBegin::Started
        );

        let dispatched = batches.next_dispatches(|id| runtime.get(id));
        let ids: Vec<_> = dispatched.iter().map(|d| d.tunnel_id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);

        runtime.set("b", TunnelRuntimeState::Open);
        assert!(batches.job_finished("b", Ok(()), TunnelRuntimeState::Open));
        assert!(batches.job_finished(
            "c",
            Err("address already in use"),
            TunnelRuntimeState::Closed
        ));

        let reports = batches.take_completed();
        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(report.group_id, "g1");
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].tunnel_id, "c");
        assert_eq!(report.failures[0].name, "name-c");
        assert_eq!(report.failures[0].error, "address already in use");
        // The successful tunnel is left open; the batch never closes it.
        assert_eq!(runtime.get("b"), TunnelRuntimeState::Open);
    }

    #[test]
    fn dispatch_is_limited_across_all_groups() {
        let runtime = Runtime::new(&[]);
        let mut batches = TunnelBatchCoordinator::default();
        let ids: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        batches.begin(
            "g1",
            TunnelBatchAction::Start,
            members(&id_refs[..5]),
            |id| runtime.get(id),
        );
        batches.begin(
            "g2",
            TunnelBatchAction::Start,
            members(&id_refs[5..]),
            |id| runtime.get(id),
        );

        let first = batches.next_dispatches(|id| runtime.get(id));
        assert_eq!(first.len(), TUNNEL_BATCH_MAX_IN_FLIGHT);
        assert!(batches.next_dispatches(|id| runtime.get(id)).is_empty());

        batches.job_finished(&first[0].tunnel_id, Ok(()), TunnelRuntimeState::Open);
        assert_eq!(batches.next_dispatches(|id| runtime.get(id)).len(), 1);
    }

    #[test]
    fn repeated_request_for_same_action_is_ignored() {
        let runtime = Runtime::new(&[]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin("g1", TunnelBatchAction::Start, members(&["a"]), |id| {
            runtime.get(id)
        });
        assert_eq!(
            batches.begin("g1", TunnelBatchAction::Start, members(&["a"]), |id| {
                runtime.get(id)
            }),
            TunnelBatchBegin::AlreadyRunning
        );
        assert_eq!(batches.next_dispatches(|id| runtime.get(id)).len(), 1);
    }

    #[test]
    fn stop_waits_for_starting_tunnel_and_then_closes_it() {
        let runtime = Runtime::new(&[
            ("a", TunnelRuntimeState::Pending),
            ("b", TunnelRuntimeState::Open),
            ("c", TunnelRuntimeState::Closed),
        ]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin(
            "g1",
            TunnelBatchAction::Stop,
            members(&["a", "b", "c"]),
            |id| runtime.get(id),
        );
        let dispatched = batches.next_dispatches(|id| runtime.get(id));
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].tunnel_id, "b");
        assert_eq!(dispatched[0].action, TunnelBatchAction::Stop);

        runtime.set("b", TunnelRuntimeState::Closed);
        batches.job_finished("b", Ok(()), TunnelRuntimeState::Closed);
        // `a` is still starting, so the batch must not finish yet.
        assert!(batches.take_completed().is_empty());

        // The foreign start job succeeds; the batch now queues a close for it.
        runtime.set("a", TunnelRuntimeState::Open);
        assert!(!batches.job_finished("a", Ok(()), TunnelRuntimeState::Open));
        let dispatched = batches.next_dispatches(|id| runtime.get(id));
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].tunnel_id, "a");
        runtime.set("a", TunnelRuntimeState::Closed);
        batches.job_finished("a", Ok(()), TunnelRuntimeState::Closed);

        let reports = batches.take_completed();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].succeeded, 2);
        assert_eq!(reports[0].skipped, 1);
        assert!(reports[0].failures.is_empty());
    }

    #[test]
    fn stop_skips_tunnel_whose_foreign_start_failed() {
        let runtime = Runtime::new(&[("a", TunnelRuntimeState::Pending)]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin("g1", TunnelBatchAction::Stop, members(&["a"]), |id| {
            runtime.get(id)
        });
        assert!(batches.next_dispatches(|id| runtime.get(id)).is_empty());
        assert!(!batches.job_finished("a", Err("refused"), TunnelRuntimeState::Closed));
        let reports = batches.take_completed();
        assert_eq!(reports[0].skipped, 1);
        assert!(reports[0].failures.is_empty());
    }

    #[test]
    fn queued_tunnel_toggled_elsewhere_is_reclassified_at_dispatch() {
        let runtime = Runtime::new(&[]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin(
            "g1",
            TunnelBatchAction::Start,
            members(&["a", "b", "c", "d"]),
            |id| runtime.get(id),
        );
        // Opened by auto-open before the batch reaches it.
        runtime.set("d", TunnelRuntimeState::Open);
        let dispatched = batches.next_dispatches(|id| runtime.get(id));
        assert_eq!(dispatched.len(), 3);
        for dispatch in &dispatched {
            batches.job_finished(&dispatch.tunnel_id, Ok(()), TunnelRuntimeState::Open);
        }
        assert!(batches.next_dispatches(|id| runtime.get(id)).is_empty());
        let reports = batches.take_completed();
        assert_eq!(reports[0].succeeded, 3);
        assert_eq!(reports[0].skipped, 1);
    }

    #[test]
    fn opposite_action_supersedes_running_batch() {
        let runtime = Runtime::new(&[]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin(
            "g1",
            TunnelBatchAction::Start,
            members(&["a", "b", "c", "d"]),
            |id| runtime.get(id),
        );
        let started = batches.next_dispatches(|id| runtime.get(id));
        for dispatch in &started {
            runtime.set(&dispatch.tunnel_id, TunnelRuntimeState::Pending);
        }

        assert_eq!(
            batches.begin(
                "g1",
                TunnelBatchAction::Stop,
                members(&["a", "b", "c", "d"]),
                |id| runtime.get(id)
            ),
            TunnelBatchBegin::Started
        );
        assert_eq!(
            batches.progress("g1").map(|progress| progress.action),
            Some(TunnelBatchAction::Stop)
        );
        // `d` was never started, the rest are awaited.
        assert!(batches.next_dispatches(|id| runtime.get(id)).is_empty());
        for dispatch in &started {
            runtime.set(&dispatch.tunnel_id, TunnelRuntimeState::Open);
            assert!(!batches.job_finished(&dispatch.tunnel_id, Ok(()), TunnelRuntimeState::Open));
        }
        assert_eq!(batches.next_dispatches(|id| runtime.get(id)).len(), 3);
    }

    #[test]
    fn rejected_dispatch_is_reported_and_frees_its_slot() {
        let runtime = Runtime::new(&[]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin(
            "g1",
            TunnelBatchAction::Start,
            members(&["a", "b", "c", "d"]),
            |id| runtime.get(id),
        );
        let dispatched = batches.next_dispatches(|id| runtime.get(id));
        batches.record_dispatch_rejected(&dispatched[0].tunnel_id, "no SSH connection".into());
        let next = batches.next_dispatches(|id| runtime.get(id));
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].tunnel_id, "d");
        assert_eq!(
            batches.progress("g1").map(|progress| progress.done),
            Some(1)
        );
    }

    #[test]
    fn groups_are_isolated_and_cancel_drops_only_the_target() {
        let runtime = Runtime::new(&[]);
        let mut batches = TunnelBatchCoordinator::default();
        batches.begin("g1", TunnelBatchAction::Start, members(&["a"]), |id| {
            runtime.get(id)
        });
        batches.begin("g2", TunnelBatchAction::Start, members(&["b"]), |id| {
            runtime.get(id)
        });
        assert!(batches.cancel_group("g1"));
        assert!(!batches.is_running("g1"));
        assert!(batches.is_running("g2"));
        let dispatched = batches.next_dispatches(|id| runtime.get(id));
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].tunnel_id, "b");
        // A late result for a cancelled group's tunnel is not batch-owned.
        assert!(!batches.job_finished("a", Ok(()), TunnelRuntimeState::Open));
    }

    #[test]
    fn empty_group_does_not_start_a_batch() {
        let mut batches = TunnelBatchCoordinator::default();
        assert_eq!(
            batches.begin("g1", TunnelBatchAction::Start, Vec::new(), |_| {
                TunnelRuntimeState::Closed
            }),
            TunnelBatchBegin::Empty
        );
        assert!(!batches.is_running("g1"));
    }
}
