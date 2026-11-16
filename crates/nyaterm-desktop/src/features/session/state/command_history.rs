//! Per-session history and reconciliation of input with ordered shell reports.

use std::collections::VecDeque;

use super::SessionFeatureState;

const COMMAND_HISTORY_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum CommandHistoryLearning {
    GlobalOnly,
    Scoped,
    Deleted,
}

struct PendingSubmission {
    command: String,
    learning: CommandHistoryLearning,
}

#[derive(Default)]
pub(super) struct SessionCommandHistory {
    entries: Vec<String>,
    pending_submissions: VecDeque<PendingSubmission>,
}

impl SessionCommandHistory {
    fn record(&mut self, command: &str) {
        self.entries.insert(0, command.to_owned());
        self.entries.truncate(COMMAND_HISTORY_LIMIT);
    }

    fn record_submission(&mut self, command: &str, learning: CommandHistoryLearning) {
        self.record(command);
        // Unintegrated shells never acknowledge input. Bound this evidence
        // independently of the displayed entries, without a latency timeout.
        self.pending_submissions.push_back(PendingSubmission {
            command: command.to_owned(),
            learning,
        });
        if self.pending_submissions.len() > COMMAND_HISTORY_LIMIT {
            self.pending_submissions.pop_front();
        }
    }

    fn take_submission(&mut self, command: &str) -> Option<CommandHistoryLearning> {
        // Shell reports preserve execution order. A matching report consumes
        // exactly one submission, plus earlier input that received no matching
        // report (e.g. a shell-expanded command). Repeated executions remain
        // separate, even when several submissions precede their reports.
        if let Some(index) = self
            .pending_submissions
            .iter()
            .position(|entry| entry.command == command)
        {
            let learning = self.pending_submissions[index].learning;
            self.pending_submissions.drain(..=index);
            Some(learning)
        } else {
            // OSC-only input (including a privacy-filtered submission) can
            // precede these queued submissions. An unmatched report cannot
            // identify which input it confirms, so keep the pending evidence.
            None
        }
    }
}

impl SessionFeatureState {
    pub(in crate::features) fn command_history_for(&self, session_id: &str) -> Option<&[String]> {
        self.command_history
            .get(session_id)
            .map(|history| history.entries.as_slice())
    }

    pub(in crate::features) fn active_command_history_snapshot(&self) -> Vec<String> {
        self.active_id()
            .and_then(|session_id| self.command_history_for(session_id))
            .map(<[String]>::to_vec)
            .unwrap_or_default()
    }

    pub(in crate::features) fn active_command_history_entry(&self, index: usize) -> Option<String> {
        let session_id = self.active_id()?;
        self.command_history_for(session_id)?.get(index).cloned()
    }

    /// Called only after the input submission passes the learning privacy gate.
    pub(in crate::features) fn record_submitted_command_history(
        &mut self,
        session_id: &str,
        command: &str,
        learning: CommandHistoryLearning,
    ) {
        let command = command.trim();
        if !command.is_empty() {
            self.command_history
                .entry(session_id.to_owned())
                .or_default()
                .record_submission(command, learning);
        }
    }

    #[cfg(test)]
    pub(in crate::features) fn record_command_history(&mut self, session_id: &str, command: &str) {
        self.record_submitted_command_history(
            session_id,
            command,
            CommandHistoryLearning::GlobalOnly,
        );
    }

    pub(in crate::features) fn take_recorded_command_submission(
        &mut self,
        session_id: &str,
        command: &str,
    ) -> Option<CommandHistoryLearning> {
        self.command_history
            .get_mut(session_id)
            .and_then(|history| history.take_submission(command.trim()))
    }

    pub(in crate::features) fn record_accepted_command_history(
        &mut self,
        session_id: &str,
        command: &str,
    ) {
        let command = command.trim();
        if !command.is_empty() {
            self.command_history
                .entry(session_id.to_owned())
                .or_default()
                .record(command);
        }
    }

    pub(in crate::features) fn migrate_command_history(&mut self, old_id: &str, new_id: &str) {
        if let Some(mut history) = self.command_history.remove(old_id) {
            // Reconnection keeps the display history, but starts a new shell.
            history.pending_submissions.clear();
            self.command_history.insert(new_id.to_owned(), history);
        }
    }

    pub(super) fn clear_pending_command_submissions(&mut self, session_id: &str) {
        if let Some(history) = self.command_history.get_mut(session_id) {
            history.pending_submissions.clear();
        }
    }

    pub(in crate::features) fn remove_command_from_all_history(&mut self, command: &str) {
        for history in self.command_history.values_mut() {
            history.entries.retain(|entry| entry != command);
            // A late report must not restore a deleted entry or its scoped learning.
            for pending in &mut history.pending_submissions {
                if pending.command == command {
                    pending.learning = CommandHistoryLearning::Deleted;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMAND_HISTORY_LIMIT, CommandHistoryLearning, SessionCommandHistory};

    #[test]
    fn delayed_reports_reconcile_each_repeated_submission_once() {
        let mut history = SessionCommandHistory::default();
        for command in ["ls", "ls", "pwd"] {
            history.record_submission(command, CommandHistoryLearning::GlobalOnly);
        }
        for command in ["ls", "ls", "pwd"] {
            assert_eq!(
                history.take_submission(command),
                Some(CommandHistoryLearning::GlobalOnly)
            );
        }
        assert_eq!(history.entries, ["pwd", "ls", "ls"]);
        assert!(history.take_submission("ls").is_none());
    }

    #[test]
    fn unmatched_reports_preserve_later_pending_submissions() {
        let mut history = SessionCommandHistory::default();
        history.record_submission("ll", CommandHistoryLearning::GlobalOnly);
        history.record_submission("pwd", CommandHistoryLearning::GlobalOnly);
        assert!(history.take_submission("ls -l").is_none());
        assert_eq!(
            history.take_submission("pwd"),
            Some(CommandHistoryLearning::GlobalOnly)
        );
        assert!(history.take_submission("ll").is_none());
    }

    #[test]
    fn osc_only_reports_do_not_consume_unrelated_pending_input() {
        let mut history = SessionCommandHistory::default();
        history.record_submission("ls", CommandHistoryLearning::GlobalOnly);
        assert!(history.take_submission("pwd").is_none());
        assert_eq!(
            history.take_submission("ls"),
            Some(CommandHistoryLearning::GlobalOnly)
        );
    }

    #[test]
    fn matching_later_reports_retire_earlier_unreported_input() {
        let mut history = SessionCommandHistory::default();
        history.record_submission("unreported", CommandHistoryLearning::GlobalOnly);
        history.record_submission("pwd", CommandHistoryLearning::GlobalOnly);
        assert_eq!(
            history.take_submission("pwd"),
            Some(CommandHistoryLearning::GlobalOnly)
        );
        assert!(history.take_submission("unreported").is_none());
    }

    #[test]
    fn unintegrated_shell_history_and_pending_evidence_stay_bounded() {
        let mut history = SessionCommandHistory::default();
        history.record_submission("oldest", CommandHistoryLearning::GlobalOnly);
        for _ in 0..COMMAND_HISTORY_LIMIT {
            history.record_submission("ls", CommandHistoryLearning::GlobalOnly);
        }
        assert_eq!(history.entries.len(), COMMAND_HISTORY_LIMIT);
        assert_eq!(history.pending_submissions.len(), COMMAND_HISTORY_LIMIT);
        assert!(history.take_submission("oldest").is_none());
    }
}
