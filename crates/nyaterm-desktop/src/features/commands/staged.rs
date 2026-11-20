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
    owners: HashMap<String, (u64, bool)>,
    runs: HashMap<u64, StagedRun>,
}

struct StagedRun {
    wake: Sender<()>,
    receiver: Option<Receiver<()>>,
    completed: u64,
    visible: bool,
}

impl StagedQuickCommands {
    pub(in crate::features) fn begin(&mut self, targets: &[String]) -> u64 {
        self.next_id = self.next_id.checked_add(1).expect("quick command run id");
        let (wake, receiver) = mpsc::channel(1);
        self.runs.insert(
            self.next_id,
            StagedRun {
                wake,
                receiver: Some(receiver),
                completed: 0,
                visible: true,
            },
        );
        for target in targets {
            self.cancel(target);
            self.owners.insert(target.clone(), (self.next_id, false));
        }
        self.next_id
    }

    pub(in crate::features) fn owns(&self, session: &str, id: u64) -> bool {
        self.owners
            .get(session)
            .is_some_and(|(owner, _)| *owner == id)
    }

    pub(in crate::features) fn mark_receiver(&mut self, session: &str, id: u64) {
        if let Some((owner, receiver)) = self.owners.get_mut(session)
            && *owner == id
        {
            *receiver = true;
        }
    }

    pub(in crate::features) fn cancel_receiver(&mut self, session: &str) {
        if self
            .owners
            .get(session)
            .is_some_and(|(_, receiver)| *receiver)
        {
            self.cancel(session);
        }
    }

    pub(in crate::features) fn cancel(&mut self, session: &str) {
        if let Some((run, _)) = self.owners.remove(session)
            && let Some(run) = self.runs.get_mut(&run)
        {
            let _ = run.wake.try_send(());
        }
    }

    pub(in crate::features) fn finish(&mut self, session: &str, id: u64) {
        if self.owns(session, id) {
            self.owners.remove(session);
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
            .filter(|(_, (owner, _))| *owner == id)
            .map(|(session, _)| session.clone())
            .collect();
        for session in sessions {
            self.cancel(&session);
        }
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
