use futures::channel::mpsc::{self, Receiver, Sender};
use nyaterm_core::send_command::{SendCommandBatch, SendCommandPlan};
use std::collections::HashMap;
use std::time::Instant;

use zeroize::Zeroizing;

pub(in crate::features) struct StagedQuickCommandTarget {
    pub(in crate::features) session_id: String,
    pub(in crate::features) source_session_id: Option<String>,
    pub(in crate::features) history: StagedCommandHistory,
    pub(in crate::features) plan: SendCommandPlan,
    pub(in crate::features) pending: Option<SendCommandBatch>,
    pub(in crate::features) ready_at: Instant,
    pub(in crate::features) finished: bool,
}

/// Session ownership is independent of focus and invalidated by competing input.
#[derive(Default)]
pub(in crate::features) struct StagedQuickCommands {
    next_id: u64,
    owners: HashMap<String, HashMap<u64, bool>>,
    runs: HashMap<u64, StagedRun>,
}

struct StagedRun {
    wake: Sender<()>,
    receiver: Option<Receiver<()>>,
    completed: u64,
    visible: bool,
    shared: bool,
}

impl StagedQuickCommands {
    pub(in crate::features) fn begin(&mut self, targets: &[String]) -> u64 {
        self.register(targets, false)
    }

    /// Send tabs may share a terminal; ordinary scripts retain exclusive takeover.
    pub(in crate::features) fn begin_shared(&mut self, targets: &[String]) -> u64 {
        self.register(targets, true)
    }

    fn register(&mut self, targets: &[String], shared: bool) -> u64 {
        self.next_id = self.next_id.checked_add(1).expect("quick command run id");
        let (wake, receiver) = mpsc::channel(1);
        self.runs.insert(
            self.next_id,
            StagedRun {
                wake,
                receiver: Some(receiver),
                completed: 0,
                visible: true,
                shared,
            },
        );
        for target in targets {
            if shared {
                let exclusive: Vec<_> = self
                    .owners
                    .get(target)
                    .into_iter()
                    .flat_map(|owners| owners.keys())
                    .copied()
                    .filter(|id| self.runs.get(id).is_some_and(|run| !run.shared))
                    .collect();
                for id in exclusive {
                    self.finish(target, id);
                    self.wake_run(id);
                }
            } else {
                self.cancel(target);
            }
            self.owners
                .entry(target.clone())
                .or_default()
                .insert(self.next_id, false);
        }
        self.next_id
    }

    pub(in crate::features) fn owns(&self, session: &str, id: u64) -> bool {
        self.owners
            .get(session)
            .is_some_and(|owners| owners.contains_key(&id))
    }

    pub(in crate::features) fn mark_receiver(&mut self, session: &str, id: u64) {
        if let Some(receiver) = self
            .owners
            .get_mut(session)
            .and_then(|owners| owners.get_mut(&id))
        {
            *receiver = true;
        }
    }

    pub(in crate::features) fn cancel_receiver(&mut self, session: &str) {
        let receivers: Vec<_> = self
            .owners
            .get(session)
            .into_iter()
            .flat_map(|owners| owners.iter())
            .filter_map(|(id, receiver)| receiver.then_some(*id))
            .collect();
        for id in receivers {
            self.finish(session, id);
            self.wake_run(id);
        }
    }

    fn wake_run(&mut self, id: u64) {
        if let Some(run) = self.runs.get_mut(&id) {
            let _ = run.wake.try_send(());
        }
    }

    pub(in crate::features) fn cancel(&mut self, session: &str) {
        if let Some(owners) = self.owners.remove(session) {
            for id in owners.into_keys() {
                self.wake_run(id);
            }
        }
    }

    pub(in crate::features) fn finish(&mut self, session: &str, id: u64) {
        if let Some(owners) = self.owners.get_mut(session) {
            owners.remove(&id);
            if owners.is_empty() {
                self.owners.remove(session);
            }
        }
    }

    pub(in crate::features) fn take_wake_receiver(&mut self, id: u64) -> Receiver<()> {
        self.runs
            .get_mut(&id)
            .expect("registered staged run")
            .receiver
            .take()
            .expect("one task owns the receiver")
    }

    pub(in crate::features) fn end_run(&mut self, id: u64) {
        self.cancel_run(id);
        self.runs.remove(&id);
    }

    pub(in crate::features) fn note_progress(&mut self, id: u64, units: u64) {
        if let Some(run) = self.runs.get_mut(&id).filter(|run| run.visible) {
            run.completed = run.completed.saturating_add(units);
        }
    }

    pub(in crate::features) fn progress(&self) -> Option<u64> {
        self.runs.values().any(|run| run.visible).then(|| {
            self.runs
                .values()
                .filter(|run| run.visible)
                .map(|run| run.completed)
                .sum()
        })
    }

    pub(in crate::features) fn hide_run(&mut self, id: u64) {
        if let Some(run) = self.runs.get_mut(&id) {
            run.visible = false;
        }
    }

    pub(in crate::features) fn cancel_run(&mut self, id: u64) {
        let sessions: Vec<_> = self
            .owners
            .iter()
            .filter(|(_, owners)| owners.contains_key(&id))
            .map(|(session, _)| session.clone())
            .collect();
        for session in sessions {
            self.finish(&session, id);
        }
        self.wake_run(id);
    }

    pub(in crate::features) fn cancel_all(&mut self) {
        let runs: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, run)| run.visible)
            .map(|(id, _)| *id)
            .collect();
        for id in runs {
            self.cancel_run(id);
        }
    }
}

#[derive(Default)]
pub(in crate::features) struct StagedCommandHistory {
    pending: Zeroizing<Vec<u8>>,
    suppressed: bool,
}

impl StagedCommandHistory {
    /// Any sensitive chunk suppresses the entire logical line, including its
    /// earlier prefix. Never feed this through the active editor's pending Enter.
    pub(in crate::features) fn push(&mut self, bytes: &[u8], sensitive: bool) -> Vec<String> {
        let mut submitted = Vec::new();
        for byte in bytes {
            self.suppressed |= sensitive;
            if matches!(byte, b'\r' | b'\n') {
                if !self.suppressed
                    && let Ok(text) = std::str::from_utf8(&self.pending)
                {
                    let command = text.trim();
                    if !command.is_empty() {
                        submitted.push(command.to_string());
                    }
                }
                self.pending = Zeroizing::new(Vec::new());
                self.suppressed = false;
            } else if self.suppressed {
                self.pending = Zeroizing::new(Vec::new());
            } else {
                self.pending.push(*byte);
            }
        }
        submitted
    }
}

#[cfg(test)]
mod tests {
    use super::StagedQuickCommands;
    use futures::{FutureExt as _, StreamExt as _};

    #[test]
    fn shared_runs_release_only_their_own_targets_and_wake_on_cancel() {
        let mut staged = StagedQuickCommands::default();
        let first = staged.begin_shared(&["one".into(), "shared".into()]);
        let second = staged.begin_shared(&["shared".into(), "two".into()]);
        let mut wake = staged.take_wake_receiver(first);
        staged.cancel_run(first);
        assert!(wake.next().now_or_never().is_some());
        assert!(!staged.owns("one", first));
        assert!(!staged.owns("shared", first));
        assert!(staged.owns("shared", second));
        assert!(staged.owns("two", second));
        staged.end_run(first);
        assert!(staged.owns("shared", second));
        staged.end_run(second);
        assert!(staged.owners.is_empty());
        assert!(staged.runs.is_empty());
    }

    #[test]
    fn competing_input_cancels_and_wakes_every_shared_owner_of_the_session() {
        let mut staged = StagedQuickCommands::default();
        let first = staged.begin_shared(&["shared".into(), "other".into()]);
        let second = staged.begin_shared(&["shared".into()]);
        let mut first_wake = staged.take_wake_receiver(first);
        let mut second_wake = staged.take_wake_receiver(second);
        staged.cancel("shared");
        assert!(!staged.owns("shared", first));
        assert!(!staged.owns("shared", second));
        assert!(staged.owns("other", first));
        assert!(first_wake.next().now_or_never().is_some());
        assert!(second_wake.next().now_or_never().is_some());
    }

    #[test]
    fn exclusive_scripts_and_shared_tabs_preserve_takeover_and_receiver_rules() {
        let mut staged = StagedQuickCommands::default();
        let first = staged.begin_shared(&["target".into()]);
        let second = staged.begin_shared(&["target".into()]);
        let script = staged.begin(&["target".into()]);
        assert!(!staged.owns("target", first));
        assert!(!staged.owns("target", second));
        assert!(staged.owns("target", script));
        let shared = staged.begin_shared(&["target".into()]);
        assert!(!staged.owns("target", script));
        assert!(staged.owns("target", shared));
        let receiver = staged.begin_shared(&["target".into()]);
        staged.mark_receiver("target", receiver);
        staged.cancel_receiver("target");
        assert!(!staged.owns("target", receiver));
        assert!(staged.owns("target", shared));
        staged.finish("target", script);
        assert!(staged.owns("target", shared));
    }
}
