//! Flag and timing diagnostics; never log input text or candidate contents.
use crate::features::NyaTermApp;
use std::time::{Duration, Instant};
const COMMAND_SUGGESTION_INPUT_SLOW_THRESHOLD: Duration = Duration::from_millis(4);
const COMMAND_SUGGESTION_REFRESH_SLOW_THRESHOLD: Duration = Duration::from_millis(8);
#[derive(Clone, Copy, Default)]
pub(super) struct CommandSuggestionInputTiming {
    pub(super) utf8: Duration,
    pub(super) submission: Duration,
    pub(super) resync: Duration,
    pub(super) apply_tracker: Duration,
    pub(super) min_chars: Duration,
    pub(super) pattern: Duration,
    pub(super) pager: Duration,
    pub(super) eligibility: Duration,
    pub(super) hide_popup: Duration,
    pub(super) schedule: Duration,
}

impl CommandSuggestionInputTiming {
    fn max_stage(self) -> Duration {
        [
            self.utf8,
            self.submission,
            self.resync,
            self.apply_tracker,
            self.min_chars,
            self.pattern,
            self.pager,
            self.eligibility,
            self.hide_popup,
            self.schedule,
        ]
        .into_iter()
        .max()
        .unwrap_or(Duration::ZERO)
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct CommandSuggestionRefreshTiming {
    pub(super) pattern: Duration,
    pub(super) search: Duration,
    pub(super) update_state: Duration,
    pub(super) hide_popup: Duration,
}

impl CommandSuggestionRefreshTiming {
    fn max_stage(self) -> Duration {
        [
            self.pattern,
            self.search,
            self.update_state,
            self.hide_popup,
        ]
        .into_iter()
        .max()
        .unwrap_or(Duration::ZERO)
    }
}

impl NyaTermApp {
    pub(super) fn log_command_suggestion_input_diagnostic(
        &mut self,
        started_at: Instant,
        byte_count: usize,
        popup_visible_at_start: bool,
        outcome: &'static str,
        pattern_chars: usize,
        timing: CommandSuggestionInputTiming,
    ) {
        let total_duration = started_at.elapsed();
        if total_duration < COMMAND_SUGGESTION_INPUT_SLOW_THRESHOLD
            && timing.max_stage() < COMMAND_SUGGESTION_INPUT_SLOW_THRESHOLD
        {
            return;
        }
        if !self.should_log_slow_diagnostic("terminal_suggestion_input", Instant::now()) {
            return;
        }
        tracing::warn!(
            diagnostic = "terminal_suggestion_input",
            outcome,
            byte_count,
            pattern_chars,
            tracker_value_bytes = self.terminal.editing.predicted_input().value.len(),
            tracker_cursor = self.terminal.editing.predicted_input().cursor,
            tracker_desynced = self.terminal.editing.predicted_input().desynced,
            tracker_desync_reason = self
                .terminal
                .editing
                .predicted_input()
                .desync_reason
                .unwrap_or(""),
            tracker_multiline = self.terminal.editing.predicted_input().multiline,
            tracker_paste_mode = self.terminal.editing.predicted_input().paste_mode,
            popup_visible_at_start,
            popup_visible = self.terminal.assist.commands.popup.is_some(),
            total_us = total_duration.as_micros(),
            utf8_us = timing.utf8.as_micros(),
            submission_us = timing.submission.as_micros(),
            resync_us = timing.resync.as_micros(),
            apply_tracker_us = timing.apply_tracker.as_micros(),
            min_chars_us = timing.min_chars.as_micros(),
            pattern_us = timing.pattern.as_micros(),
            pager_us = timing.pager.as_micros(),
            eligibility_us = timing.eligibility.as_micros(),
            hide_popup_us = timing.hide_popup.as_micros(),
            schedule_us = timing.schedule.as_micros(),
            "slow terminal suggestion input"
        );
    }

    /// Explains why no popup was offered. Flags only, never input text.
    pub(super) fn log_command_suggestions_unavailable(&mut self, session_id: &str) {
        if !self.should_log_slow_diagnostic("terminal_suggestion_unavailable", Instant::now()) {
            return;
        }
        let allowed = self.command_suggestions_allowed_for_session(session_id);
        let state = self.terminal.editing.state();
        tracing::info!(
            diagnostic = "terminal_suggestion_unavailable",
            allowed,
            editing_active = self.terminal.editing.is_active(session_id),
            suppression = ?self.terminal.assist.commands.suppression,
            phase = ?state.map(|s| s.model.phase),
            has_mapping = state.is_some_and(|s| s.mapping.is_some()),
            awaiting_snapshot = state.is_some_and(|s| s.awaiting_snapshot),
            desynced = state.is_some_and(|s| s.model.input.desynced),
            multiline = state.is_some_and(|s| s.model.input.multiline),
            line_rewrite = state.is_some_and(|s| s.model.input.line_rewrite_required),
            "command suggestions unavailable"
        );
    }

    pub(super) fn log_command_suggestion_refresh_diagnostic(
        &mut self,
        started_at: Instant,
        popup_visible_at_start: bool,
        outcome: &'static str,
        pattern_chars: usize,
        result_count: usize,
        timing: CommandSuggestionRefreshTiming,
    ) {
        let total_duration = started_at.elapsed();
        if total_duration < COMMAND_SUGGESTION_REFRESH_SLOW_THRESHOLD
            && timing.max_stage() < COMMAND_SUGGESTION_REFRESH_SLOW_THRESHOLD
        {
            return;
        }
        if !self.should_log_slow_diagnostic("terminal_suggestion_refresh", Instant::now()) {
            return;
        }
        tracing::warn!(
            diagnostic = "terminal_suggestion_refresh",
            outcome,
            pattern_chars,
            result_count,
            command_history_count = self.commands.command_history().len(),
            quick_command_count = self.commands.quick_commands().len(),
            tracker_value_bytes = self.terminal.editing.predicted_input().value.len(),
            tracker_desynced = self.terminal.editing.predicted_input().desynced,
            tracker_multiline = self.terminal.editing.predicted_input().multiline,
            popup_visible_at_start,
            popup_visible = self.terminal.assist.commands.popup.is_some(),
            total_us = total_duration.as_micros(),
            pattern_us = timing.pattern.as_micros(),
            search_us = timing.search.as_micros(),
            update_state_us = timing.update_state.as_micros(),
            hide_popup_us = timing.hide_popup.as_micros(),
            "slow terminal suggestion refresh"
        );
    }
}
