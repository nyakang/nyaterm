use rust_i18n::t;

use nyaterm_ui::{NyaScrollable, NyaTooltip};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Bounds, Context, FontWeight, IntoElement, KeyDownEvent, Pixels, SharedString, div, prelude::*,
    px, rgb, rgba, svg,
};
use nyaterm_core::terminal::editing::{EDIT_CONFIRMATION_TIMEOUT, EditPhase};
use nyaterm_core::{
    CommandHistoryEntry, QuickCommand, TerminalInputState, can_suggest_from_tracked_command,
    get_tracked_command, manual_empty_command_suggestions, search_command_sources,
    terminal_input_tracker_below_min_chars,
};
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_terminal::editing::resolve_input;

use crate::features::NyaTermApp;
use crate::features::terminal::editing_state::SuggestionFillRequest;
use crate::features::terminal::terminal_runtime::TERMINAL_INPUT_LATENCY_WINDOW;
use crate::features::terminal::terminal_surface_entity::terminal_snapshot_anchor_row_for_display_offset;
use crate::models::{CommandSuggestionItem, CommandSuggestionState};
use crate::terminal::terminal_byte_index_for_cell_col;

mod fill;
#[cfg(test)]
mod fill_tests;
mod helpers;

use self::fill::{SuggestionFill, SuggestionSubmitResult, plan_suggestion_fill};
use self::helpers::command_suggestion_highlight_parts;

const COMMAND_SUGGESTION_INPUT_SLOW_THRESHOLD: Duration = Duration::from_millis(4);
const COMMAND_SUGGESTION_REFRESH_SLOW_THRESHOLD: Duration = Duration::from_millis(8);
const COMMAND_SUGGESTION_REFRESH_DEBOUNCE: Duration = Duration::from_millis(80);
const COMMAND_SUGGESTION_REFRESH_PRESSURE_RETRY: Duration = Duration::from_millis(120);
pub(in crate::features) const SUGGESTION_OVERLAY_HEADER_HEIGHT: f32 = 24.0;
pub(in crate::features) const SUGGESTION_OVERLAY_FOOTER_HEIGHT: f32 = 24.0;
const SUGGESTION_OVERLAY_CHROME_HEIGHT: f32 =
    SUGGESTION_OVERLAY_HEADER_HEIGHT + SUGGESTION_OVERLAY_FOOTER_HEIGHT;
const SUGGESTION_OVERLAY_MAX_HEIGHT: f32 = 272.0;
const SUGGESTION_OVERLAY_CURSOR_GAP: f32 = 4.0;
const SUGGESTION_OVERLAY_VIEWPORT_MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Default)]
struct CommandSuggestionInputTiming {
    utf8: Duration,
    submission: Duration,
    resync: Duration,
    apply_tracker: Duration,
    min_chars: Duration,
    pattern: Duration,
    pager: Duration,
    eligibility: Duration,
    hide_popup: Duration,
    schedule: Duration,
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
struct CommandSuggestionRefreshTiming {
    pattern: Duration,
    search: Duration,
    cursor: Duration,
    update_state: Duration,
    hide_popup: Duration,
}

struct CommandSuggestionSearchRequest {
    request_id: u64,
    session_id: String,
    input_version: u64,
    sync_revision: u64,
    pattern: String,
    pattern_chars: usize,
    min_chars: usize,
    max_chars: usize,
    history: Arc<[CommandHistoryEntry]>,
    quick_commands: Arc<[QuickCommand]>,
    started_at: Instant,
    popup_visible_at_start: bool,
    timing: CommandSuggestionRefreshTiming,
}

impl CommandSuggestionRefreshTiming {
    fn max_stage(self) -> Duration {
        [
            self.pattern,
            self.search,
            self.cursor,
            self.update_state,
            self.hide_popup,
        ]
        .into_iter()
        .max()
        .unwrap_or(Duration::ZERO)
    }
}

fn command_suggestion_refresh_input_delay(
    last_terminal_input_at: Option<Instant>,
    now: Instant,
) -> Option<Duration> {
    last_terminal_input_at.and_then(|last| {
        let elapsed = now.saturating_duration_since(last);
        (elapsed < TERMINAL_INPUT_LATENCY_WINDOW).then(|| TERMINAL_INPUT_LATENCY_WINDOW - elapsed)
    })
}

fn command_suggestion_input_can_defer_refresh(state: &TerminalInputState) -> bool {
    !state.desynced && !state.multiline && state.cursor == state.value.len()
}

fn command_suggestion_input_obvious_pager_prefix(state: &TerminalInputState) -> bool {
    let value = state.value.trim_start();
    value.starts_with('/') || value.starts_with('?') || value.starts_with(':')
}

fn command_suggestion_input_candidate_chars(state: &TerminalInputState) -> usize {
    state.value.trim_start().chars().count()
}

#[cfg(test)]
fn command_history_input_update(state: &mut TerminalInputState, text: &str) -> Option<String> {
    let submitted = if text.contains('\r') || text.contains('\n') {
        let submitted =
            nyaterm_core::terminal::input_tracker::get_tracked_submission_command(state);
        (!submitted.is_empty()).then_some(submitted)
    } else {
        None
    };
    nyaterm_core::terminal::input_tracker::apply_terminal_input_data_in_place(state, text);
    submitted
}

impl NyaTermApp {
    /// Whether the active session's program is driving the alternate screen.
    ///
    /// Full-screen programs such as vim, less, top and nano own the keyboard
    /// while they are there: their input is not a shell command line, so command
    /// suggestions must stay closed and must never consume keys. This is a
    /// protocol signal, unlike the submitted-command heuristic in
    /// `command_starts_suggestion_suppressing_program`, which only covers
    /// programs the shell line happened to reveal.
    pub(in crate::features) fn active_terminal_uses_alternate_screen(&self) -> bool {
        self.session
            .active_id()
            .filter(|session_id| !session_id.is_empty())
            .and_then(|session_id| self.terminal.view.views.get(session_id))
            .is_some_and(|view| view.protocol_state.alternate_screen)
    }

    pub(in crate::features) fn dismiss_command_suggestions(&mut self, cx: &mut Context<Self>) {
        self.terminal.assist.command_suggestion_search_gen = self
            .terminal
            .assist
            .command_suggestion_search_gen
            .saturating_add(1);
        self.terminal.assist.command_suggestion_refresh_task = None;
        let mut changed = false;
        if self.terminal.assist.command_suggestions.take().is_some() {
            changed = true;
        }
        // Keep draft for continued typing; only clear list visibility.
        if changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn clear_command_suggestion_draft(&mut self, cx: &mut Context<Self>) {
        self.terminal.assist.command_suggestion_search_gen = self
            .terminal
            .assist
            .command_suggestion_search_gen
            .saturating_add(1);
        self.terminal.assist.command_suggestion_refresh_task = None;
        let mut changed = false;

        if self.terminal.assist.command_suggestions.take().is_some() {
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn note_command_suggestion_input(
        &mut self,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) {
        let started_at = Instant::now();
        let byte_count = bytes.len();
        let popup_visible_at_start = self.terminal.assist.command_suggestions.is_some();
        let mut timing = CommandSuggestionInputTiming::default();
        macro_rules! finish {
            ($outcome:expr, $pattern_chars:expr) => {{
                self.log_command_suggestion_input_diagnostic(
                    started_at,
                    byte_count,
                    popup_visible_at_start,
                    $outcome,
                    $pattern_chars,
                    timing,
                );
                return;
            }};
        }

        if self.terminal.assist.credential_suggestions.is_some()
            || self.is_credential_prompt_input_mode()
        {
            return;
        }
        if !self
            .settings
            .summary()
            .interaction_command_suggestions_enabled
        {
            self.clear_command_suggestion_draft(cx);
            return;
        }
        if self.session.active_id().is_none() {
            self.clear_command_suggestion_draft(cx);
            return;
        }
        if self.active_terminal_uses_alternate_screen() {
            self.clear_command_suggestion_draft(cx);
            finish!("alternate_screen", 0);
        }
        let utf8_started_at = Instant::now();
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => {
                timing.utf8 = utf8_started_at.elapsed();
                text
            }
            Err(_) => {
                timing.utf8 = utf8_started_at.elapsed();
                self.clear_command_suggestion_draft(cx);
                finish!("non_utf8", 0);
            }
        };
        if text.is_empty() {
            finish!("empty_text", 0);
        }

        if self.terminal.assist.command_suggestions_suppressed {
            let hide_started_at = Instant::now();
            self.terminal.assist.command_suggestion_search_gen = self
                .terminal
                .assist
                .command_suggestion_search_gen
                .saturating_add(1);
            if self.terminal.assist.command_suggestions.take().is_some() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("suppressed", 0);
        }

        let min_chars_started_at = Instant::now();
        let min_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_min_chars
            .max(1) as usize;
        let below_min_chars =
            terminal_input_tracker_below_min_chars(self.terminal.editing.assist_input(), min_chars);
        timing.min_chars = min_chars_started_at.elapsed();
        if below_min_chars {
            let hide_started_at = Instant::now();
            self.terminal.assist.command_suggestion_search_gen = self
                .terminal
                .assist
                .command_suggestion_search_gen
                .saturating_add(1);
            if self.terminal.assist.command_suggestions.take().is_some() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("below_min_chars", 0);
        }

        let pattern_started_at = Instant::now();
        let pattern_chars =
            command_suggestion_input_candidate_chars(self.terminal.editing.assist_input());
        timing.pattern = pattern_started_at.elapsed();

        let pager_started_at = Instant::now();
        let pager_input =
            command_suggestion_input_obvious_pager_prefix(self.terminal.editing.assist_input());
        timing.pager = pager_started_at.elapsed();
        if pager_input {
            let hide_started_at = Instant::now();
            self.terminal.assist.command_suggestion_search_gen = self
                .terminal
                .assist
                .command_suggestion_search_gen
                .saturating_add(1);
            if self.terminal.assist.command_suggestions.take().is_some() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("pager_input", pattern_chars);
        }

        let eligibility_started_at = Instant::now();
        let can_suggest =
            command_suggestion_input_can_defer_refresh(self.terminal.editing.assist_input());
        timing.eligibility = eligibility_started_at.elapsed();
        if !can_suggest {
            let hide_started_at = Instant::now();
            self.terminal.assist.command_suggestion_search_gen = self
                .terminal
                .assist
                .command_suggestion_search_gen
                .saturating_add(1);
            if self.terminal.assist.command_suggestions.take().is_some() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("not_eligible", pattern_chars);
        }
        let schedule_started_at = Instant::now();
        self.schedule_command_suggestion_refresh(cx);
        timing.schedule = schedule_started_at.elapsed();
        finish!("scheduled", pattern_chars);
    }

    pub(in crate::features) fn schedule_command_suggestion_refresh(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        // Tauri useCommandHistory: 80ms debounce before fuzzy search.
        self.schedule_command_suggestion_refresh_after(COMMAND_SUGGESTION_REFRESH_DEBOUNCE, cx);
    }

    fn schedule_command_suggestion_refresh_after(
        &mut self,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        self.terminal.assist.command_suggestion_search_gen = self
            .terminal
            .assist
            .command_suggestion_search_gen
            .saturating_add(1);
        let request_id = self.terminal.assist.command_suggestion_search_gen;
        self.terminal.assist.command_suggestion_refresh_task =
            Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let request = this.update(cx, |this, cx| {
                    if this.terminal.assist.command_suggestion_search_gen != request_id {
                        return None;
                    }
                    let now = Instant::now();
                    if let Some(delay) = command_suggestion_refresh_input_delay(
                        this.shell.last_terminal_input_at(),
                        now,
                    ) {
                        this.schedule_command_suggestion_refresh_after(delay, cx);
                        return None;
                    }
                    if this.runtime_output_pressure_active() {
                        this.hide_command_suggestions_if_present(cx);
                        this.schedule_command_suggestion_refresh_after(
                            COMMAND_SUGGESTION_REFRESH_PRESSURE_RETRY,
                            cx,
                        );
                        return None;
                    }
                    this.prepare_command_suggestion_search(request_id, cx)
                });
                let Ok(Some(request)) = request else {
                    return;
                };
                let search_started_at = Instant::now();
                let pattern = request.pattern.clone();
                let min_chars = request.min_chars;
                let max_chars = request.max_chars;
                let history = request.history.clone();
                let quick_commands = request.quick_commands.clone();
                let search_task = cx.background_spawn(async move {
                    search_command_sources(
                        &history,
                        &quick_commands,
                        &pattern,
                        12,
                        Some(min_chars),
                        Some(max_chars),
                    )
                });
                let results = search_task.await;
                let search_duration = search_started_at.elapsed();
                let _ = this.update(cx, |this, cx| {
                    this.publish_command_suggestion_search(request, results, search_duration, cx);
                });
            }));
    }

    pub(in crate::features) fn read_terminal_input_line_for_session(
        &self,
        session_id: &str,
    ) -> Option<String> {
        let snapshot = self.terminal_snapshot_for_session(Some(session_id), 0);
        if snapshot.cursor.row == usize::MAX {
            return None;
        }
        let line = snapshot.line(snapshot.cursor.row)?;
        Some(terminal_line_prefix_for_cell_col(line, snapshot.cursor.col))
    }

    pub(in crate::features) fn refresh_command_suggestions(&mut self, cx: &mut Context<Self>) {
        self.schedule_command_suggestion_refresh_after(Duration::ZERO, cx);
    }

    pub(in crate::features) fn show_manual_command_suggestions(&mut self, cx: &mut Context<Self>) {
        if self.terminal.assist.credential_suggestions.is_some()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.command_suggestions_suppressed
            || !self
                .settings
                .summary()
                .interaction_command_suggestions_enabled
        {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        let Some(session_id) = self
            .session
            .active_id()
            .filter(|session_id| !session_id.is_empty())
            .map(ToOwned::to_owned)
        else {
            self.hide_command_suggestions_if_present(cx);
            return;
        };
        if !command_suggestion_input_can_defer_refresh(self.terminal.editing.assist_input()) {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        if self.active_terminal_uses_alternate_screen() {
            self.hide_command_suggestions_if_present(cx);
            return;
        }

        self.terminal.assist.command_suggestion_search_gen = self
            .terminal
            .assist
            .command_suggestion_search_gen
            .saturating_add(1);
        self.terminal.assist.command_suggestion_refresh_task = None;
        let min_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_min_chars
            .max(1) as usize;
        let max_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_max_chars
            .max(min_chars as u32) as usize;
        let pattern = get_tracked_command(self.terminal.editing.assist_input());
        let pattern_chars = pattern.chars().count();
        let results = if pattern.trim().is_empty() {
            manual_empty_command_suggestions(
                &self.commands.command_history_snapshot(),
                &self.commands.quick_commands_snapshot(),
                12,
                Some(min_chars),
                Some(max_chars),
            )
        } else if can_suggest_from_tracked_command(self.terminal.editing.assist_input(), &pattern) {
            search_command_sources(
                &self.commands.command_history_snapshot(),
                &self.commands.quick_commands_snapshot(),
                &pattern,
                12,
                Some(min_chars),
                Some(max_chars),
            )
        } else {
            Vec::new()
        };
        if results.is_empty() {
            self.hide_command_suggestions_if_present(cx);
            return;
        }

        let (cursor_row, cursor_col) = self.active_terminal_cursor_cell();
        self.terminal.assist.command_suggestions = Some(CommandSuggestionState {
            session_id,
            input_version: self
                .terminal
                .editing
                .state()
                .map_or(0, |state| state.model.version),
            sync_revision: self.sync_input.revision(),
            draft: pattern,
            items: results
                .into_iter()
                .map(|item| CommandSuggestionItem {
                    command: item.command,
                    display: item.display,
                    source: item.source,
                    score: item.score,
                    indices: item.indices,
                })
                .collect(),
            selected_index: None,
            cursor_row,
            cursor_col,
        });
        self.shell
            .set_status(format!("showing {pattern_chars}-char command suggestions"));
        cx.notify();
    }

    fn prepare_command_suggestion_search(
        &mut self,
        request_id: u64,
        cx: &mut Context<Self>,
    ) -> Option<CommandSuggestionSearchRequest> {
        let started_at = Instant::now();
        let popup_visible_at_start = self.terminal.assist.command_suggestions.is_some();
        let mut timing = CommandSuggestionRefreshTiming::default();
        macro_rules! finish_refresh {
            ($outcome:expr, $pattern_chars:expr, $result_count:expr) => {{
                self.log_command_suggestion_refresh_diagnostic(
                    started_at,
                    popup_visible_at_start,
                    $outcome,
                    $pattern_chars,
                    $result_count,
                    timing,
                );
                return None;
            }};
        }

        let Some(session_id) = self
            .session
            .active_id()
            .filter(|session_id| !session_id.is_empty())
            .map(ToOwned::to_owned)
        else {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("no_active_session", 0, 0);
        };
        if self.terminal.assist.credential_suggestions.is_some()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.command_suggestions_suppressed
        {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("suppressed_or_credential", 0, 0);
        }
        // A search scheduled on the shell line can land after a full-screen
        // program has taken the alternate screen, so re-check here too.
        if self.active_terminal_uses_alternate_screen() {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("alternate_screen", 0, 0);
        }
        if !self
            .settings
            .summary()
            .interaction_command_suggestions_enabled
        {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("disabled", 0, 0);
        }
        let min_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_min_chars
            .max(1) as usize;
        let max_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_max_chars
            .max(min_chars as u32) as usize;
        let pattern_started_at = Instant::now();
        let pattern = get_tracked_command(self.terminal.editing.assist_input());
        let pattern_chars = pattern.chars().count();
        timing.pattern = pattern_started_at.elapsed();
        if !can_suggest_from_tracked_command(self.terminal.editing.assist_input(), &pattern) {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("not_eligible", pattern_chars, 0);
        }
        if pattern_chars < min_chars {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("below_min_chars", pattern_chars, 0);
        }
        // Pager/search-like prefixes: hide suggestions.
        if pattern.starts_with('/') || pattern.starts_with('?') || pattern.starts_with(':') {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("pager_prefix", pattern_chars, 0);
        }
        Some(CommandSuggestionSearchRequest {
            request_id,
            session_id,
            input_version: self.terminal.editing.state()?.model.version,
            sync_revision: self.sync_input.revision(),
            pattern,
            pattern_chars,
            min_chars,
            max_chars,
            history: self.commands.command_history_snapshot(),
            quick_commands: self.commands.quick_commands_snapshot(),
            started_at,
            popup_visible_at_start,
            timing,
        })
    }

    fn publish_command_suggestion_search(
        &mut self,
        request: CommandSuggestionSearchRequest,
        results: Vec<nyaterm_core::FuzzyResult>,
        search_duration: Duration,
        cx: &mut Context<Self>,
    ) {
        if self.terminal.assist.command_suggestion_search_gen != request.request_id
            || self.session.active_id() != Some(request.session_id.as_str())
            || self
                .terminal
                .editing
                .state()
                .map(|state| state.model.version)
                != Some(request.input_version)
            || self.sync_input.revision() != request.sync_revision
            || self.terminal.assist.credential_suggestions.is_some()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.command_suggestions_suppressed
            || !self
                .settings
                .summary()
                .interaction_command_suggestions_enabled
            || get_tracked_command(self.terminal.editing.assist_input()) != request.pattern
        {
            return;
        }
        // The search was started on the shell line, but a full-screen program
        // may have taken the alternate screen while it ran.
        if self.active_terminal_uses_alternate_screen() {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        let CommandSuggestionSearchRequest {
            session_id,
            input_version,
            sync_revision,
            pattern,
            pattern_chars,
            started_at,
            popup_visible_at_start,
            mut timing,
            ..
        } = request;
        timing.search = search_duration;
        let result_count = results.len();
        if results.is_empty() {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            self.log_command_suggestion_refresh_diagnostic(
                started_at,
                popup_visible_at_start,
                "search_empty",
                pattern_chars,
                result_count,
                timing,
            );
            return;
        }
        let cursor_started_at = Instant::now();
        let (cursor_row, cursor_col) = self.active_terminal_cursor_cell();
        timing.cursor = cursor_started_at.elapsed();
        let update_started_at = Instant::now();
        let next_state = CommandSuggestionState {
            session_id,
            input_version,
            sync_revision,
            draft: pattern,
            items: results
                .into_iter()
                .map(|item| CommandSuggestionItem {
                    command: item.command,
                    display: item.display,
                    source: item.source,
                    score: item.score,
                    indices: item.indices,
                })
                .collect(),
            selected_index: None,
            cursor_row,
            cursor_col,
        };
        if !command_suggestion_state_changed(
            self.terminal.assist.command_suggestions.as_ref(),
            &next_state,
        ) {
            timing.update_state = update_started_at.elapsed();
            self.log_command_suggestion_refresh_diagnostic(
                started_at,
                popup_visible_at_start,
                "unchanged",
                pattern_chars,
                result_count,
                timing,
            );
            return;
        }
        self.terminal.assist.command_suggestions = Some(next_state);
        timing.update_state = update_started_at.elapsed();
        cx.notify();
        self.log_command_suggestion_refresh_diagnostic(
            started_at,
            popup_visible_at_start,
            "shown",
            pattern_chars,
            result_count,
            timing,
        );
    }

    fn log_command_suggestion_input_diagnostic(
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
            popup_visible = self.terminal.assist.command_suggestions.is_some(),
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

    fn log_command_suggestion_refresh_diagnostic(
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
            popup_visible = self.terminal.assist.command_suggestions.is_some(),
            total_us = total_duration.as_micros(),
            pattern_us = timing.pattern.as_micros(),
            search_us = timing.search.as_micros(),
            cursor_us = timing.cursor.as_micros(),
            update_state_us = timing.update_state.as_micros(),
            hide_popup_us = timing.hide_popup.as_micros(),
            "slow terminal suggestion refresh"
        );
    }

    fn hide_command_suggestions_if_present(&mut self, cx: &mut Context<Self>) {
        if self.terminal.assist.command_suggestions.take().is_some() {
            cx.notify();
        }
    }

    /// Capture a viewport-relative anchor using one snapshot. History padding can
    /// change between frames even when the shell cursor has not moved.
    pub(in crate::features) fn active_terminal_cursor_cell(&self) -> (usize, usize) {
        let session_id = self.session.active_id();
        let offset = self.active_terminal_display_offset();
        let snapshot = self.terminal_snapshot_for_session(session_id, offset);
        let anchor = terminal_snapshot_anchor_row_for_display_offset(
            &snapshot,
            offset,
            self.terminal_viewport_rows_for_session(session_id),
            self.terminal_scrollback_len_for_session(session_id),
        );
        let row = if snapshot.cursor.row == usize::MAX {
            snapshot.row_count().saturating_sub(1)
        } else {
            snapshot.cursor.row
        };
        (
            row.saturating_sub(anchor + snapshot.leading_cleared_rows(anchor)),
            snapshot.cursor.col,
        )
    }

    /// Handle suggestion popup keys. Returns true when the key was consumed.
    pub(in crate::features) fn handle_command_suggestion_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.terminal.assist.command_suggestions.as_ref() else {
            return false;
        };
        if state.items.is_empty() {
            return false;
        }
        let session_id = state.session_id.clone();
        let accepting = command_suggestion_item_for_selection(&state.items, state.selected_index)
            .is_some()
            && matches!(event.keystroke.key.as_str(), "tab" | "enter")
            && !event.keystroke.modifiers.modified();
        if self.session.active_id() != Some(session_id.as_str())
            || self
                .terminal_surface_bounds_for_session(Some(&session_id))
                .is_none()
        {
            if accepting {
                return self
                    .reject_command_suggestion(
                        "Suggestion session or terminal surface changed; select it again",
                        cx,
                    )
                    .handled();
            }
            self.terminal.assist.command_suggestions = None;
            cx.notify();
            return false;
        }
        if self.active_terminal_uses_alternate_screen() {
            // A stale shell suggestion must not submit to the TUI. Other keys,
            // including Escape, still belong to the foreground program.
            if accepting {
                return self
                    .reject_command_suggestion(
                        "Cannot fill a shell suggestion in the alternate screen",
                        cx,
                    )
                    .handled();
            }
            self.terminal.assist.command_suggestions = None;
            cx.notify();
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }
        if accepting {
            // A consumed suggestion key has no remote press owner. Its repeats
            // and Kitty release must not turn into a later command submission.
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                view.pressed_keys.remove(&keystroke.key);
            }
            if event.is_held {
                return true;
            }
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.dismiss_command_suggestions(cx);
                true
            }
            "up" => {
                if let Some(state) = self.terminal.assist.command_suggestions.as_mut() {
                    state.selected_index = command_suggestion_step_selection(
                        state.selected_index,
                        state.items.len(),
                        -1,
                    );
                    cx.notify();
                }
                true
            }
            "down" => {
                if let Some(state) = self.terminal.assist.command_suggestions.as_mut() {
                    state.selected_index = command_suggestion_step_selection(
                        state.selected_index,
                        state.items.len(),
                        1,
                    );
                    cx.notify();
                }
                true
            }
            "tab" => self.apply_selected_command_suggestion(false, cx),
            "enter" => self.apply_selected_command_suggestion(true, cx),
            _ => false,
        }
    }

    pub(in crate::features) fn apply_selected_command_suggestion(
        &mut self,
        execute: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.submit_selected_command_suggestion(execute, cx)
            .handled()
    }

    fn reject_command_suggestion(
        &mut self,
        message: &'static str,
        cx: &mut Context<Self>,
    ) -> SuggestionSubmitResult {
        self.dismiss_command_suggestions(cx);
        self.shell.set_status(message);
        cx.notify();
        SuggestionSubmitResult::Rejected
    }

    fn submit_selected_command_suggestion(
        &mut self,
        execute_requested: bool,
        cx: &mut Context<Self>,
    ) -> SuggestionSubmitResult {
        let Some(popup) = self.terminal.assist.command_suggestions.clone() else {
            return SuggestionSubmitResult::NotHandled;
        };
        let Some(item) = command_suggestion_item_for_selection(&popup.items, popup.selected_index)
        else {
            return SuggestionSubmitResult::NotHandled;
        };
        if self.session.active_id() != Some(popup.session_id.as_str())
            || self.session.is_disconnected(&popup.session_id)
            || self.active_terminal_input_blocked()
            || self.security.screen_locked()
            || self.active_terminal_uses_alternate_screen()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.credential_suggestions.is_some()
            || self.terminal.assist.command_suggestions_suppressed
            || self
                .terminal_protocol_state_for_session(&popup.session_id)
                .mouse_reporting
            || self.active_terminal_display_offset() != 0
            || !self
                .settings
                .summary()
                .interaction_command_suggestions_enabled
            || self.sync_input.may_fan_out_from(&popup.session_id)
            || self.sync_input.revision() != popup.sync_revision
        {
            return self.reject_command_suggestion(
                "Cannot safely fill this suggestion in the current terminal state",
                cx,
            );
        }
        // Reconcile against the latest live frame; a typing prediction or an old
        // matching search pattern is never sufficient evidence for a write.
        self.reconcile_shell_editing(&popup.session_id, cx);
        let snapshot = self.terminal_snapshot_for_session(Some(&popup.session_id), 0);
        let Some(state) = self.terminal.editing.state() else {
            return self
                .reject_command_suggestion("Wait for the terminal to confirm the input", cx);
        };
        if state.awaiting_snapshot
            || state.model.phase != EditPhase::Ready
            || state.model.version != popup.input_version
            || state.mapping.is_none()
            || resolve_input(
                &snapshot,
                &state.model.input.value,
                state.model.input.cursor,
            )
            .as_ref()
                != state.mapping.as_ref()
        {
            return self.reject_command_suggestion(
                "Input changed or is not confirmed; select the suggestion again",
                cx,
            );
        }
        let suffix = match plan_suggestion_fill(&state.model.input, &item.command) {
            SuggestionFill::Rejected(message) => {
                return self.reject_command_suggestion(message, cx);
            }
            SuggestionFill::NoChange => {
                self.dismiss_command_suggestions(cx);
                self.shell.set_status(if execute_requested {
                    "Command already filled; press Enter again to execute"
                } else {
                    "Command already filled"
                });
                cx.notify();
                return SuggestionSubmitResult::NoChange;
            }
            SuggestionFill::Append(suffix) => suffix,
        };
        // Logical input performs charset conversion and advances the shell model
        // exactly once, after the primary write succeeds. The fan-out gate above
        // is checked in the same GPUI update, without an asynchronous gap.
        self.dismiss_command_suggestions(cx);
        self.terminal.editing.state_mut().suggestion_fill = None;
        if !self.send_terminal_input_without_suggestion_track(suffix.as_bytes().to_vec(), cx) {
            self.terminal.editing.state_mut().invalidate();
            return SuggestionSubmitResult::WriteFailed;
        }
        let state = self.terminal.editing.state_mut();
        state.suggestion_fill = Some(SuggestionFillRequest {
            request_id: state.confirmation_id,
            input_version: state.model.version,
            target: item.command.clone(),
            execute_requested,
            deadline: Instant::now() + EDIT_CONFIRMATION_TIMEOUT,
            sync_revision: popup.sync_revision,
        });
        self.shell.set_status(if execute_requested {
            "Suggestion filled; waiting for echo. Press Enter again to execute"
        } else {
            "Suggestion filled; waiting for echo"
        });
        cx.notify();
        SuggestionSubmitResult::Submitted
    }

    pub(in crate::features) fn delete_command_suggestion_history(
        &mut self,
        command: String,
        cx: &mut Context<Self>,
    ) {
        let command = command.trim().to_string();
        if command.is_empty() {
            return;
        }
        let request_command = command.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Commands, move |store| {
                store.delete_command_history(&request_command)?;
                store.list_command_history(64)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(history) => {
                        this.commands.replace_command_history(history);
                        this.session.remove_command_from_all_history(&command);
                        if let Some(state) = this.terminal.assist.command_suggestions.as_mut() {
                            state.items.retain(|item| {
                                !(item.source == "history" && item.command == command)
                            });
                            if state.items.is_empty() {
                                this.terminal.assist.command_suggestions = None;
                            } else {
                                state.selected_index = command_suggestion_clamp_selection(
                                    state.selected_index,
                                    state.items.len(),
                                );
                            }
                        } else {
                            this.refresh_command_suggestions(cx);
                        }
                        this.shell
                            .set_status(format!("deleted history command '{command}'"));
                    }
                    Err(error) => this
                        .shell
                        .set_status(format!("failed to delete history: {error}")),
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn command_suggestions_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let Some(state) = self.terminal.assist.command_suggestions.as_ref() else {
            return div().into_any_element();
        };
        if state.items.is_empty() {
            return div().into_any_element();
        }
        if self.session.active_id() != Some(state.session_id.as_str()) {
            return div().into_any_element();
        }
        if self.active_terminal_uses_alternate_screen() {
            return div().into_any_element();
        }
        let menu_w = 380.0_f32;
        let menu_h = suggestion_overlay_desired_height(state.items.len(), 28.0);
        let title = t!("suggestions.title");
        let match_label = t!(if state.items.len() == 1 {
            "suggestions.match"
        } else {
            "suggestions.matches"
        });
        let footer = format!(
            "↑↓ {} · Tab / Enter {} · Esc {}",
            t!("suggestions.select"),
            t!("suggestions.fill"),
            t!("suggestions.dismiss")
        );
        let Some(placement) = self.suggestion_overlay_position_for_session(
            Some(&state.session_id),
            state.cursor_row,
            state.cursor_col,
            menu_w,
            menu_h,
        ) else {
            return div().into_any_element();
        };
        let selected_preview = state
            .selected_index
            .and_then(|index| state.items.get(index))
            .filter(|item| item.command.chars().count() > 48)
            .map(|item| item.command.clone());

        let mut list = div()
            .id(SharedString::from("command-suggestions-list"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();
        for (index, item) in state.items.iter().enumerate() {
            let selected = state.selected_index == Some(index);
            let source_icon = match item.source.as_str() {
                "history" => "icons/history.svg",
                _ => "icons/commands.svg",
            };
            let label = if item.display.trim().is_empty() {
                item.command.clone()
            } else {
                item.display.clone()
            };
            let show_full_command = label.chars().count() > 48;
            let full_command = item.command.clone();
            let is_history = item.source == "history";
            let delete_command = item.command.clone();
            let mut row = div()
                .id(SharedString::from(format!("command-suggestion-{index}")))
                .h(px(28.))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .border_l_2()
                .border_color(rgb(if selected {
                    palette.primary
                } else {
                    palette.surface
                }))
                .bg(rgb(if selected {
                    palette.hover
                } else {
                    palette.surface
                }))
                .text_size(px(11.))
                .text_color(rgb(palette.text))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(state) = this.terminal.assist.command_suggestions.as_mut() {
                        state.selected_index = Some(index);
                    }
                    let _ = this.apply_selected_command_suggestion(true, cx);
                }))
                .child(
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path(source_icon)
                        .text_color(if selected {
                            rgb(palette.accent)
                        } else {
                            rgb(palette.text_dimmed)
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .children(command_suggestion_highlight_parts(
                            &label,
                            &item.indices,
                            palette,
                            selected,
                        )),
                );
            if show_full_command {
                row = row.tooltip(move |window, cx| {
                    NyaTooltip::new(full_command.clone()).build(window, cx)
                });
            }
            if is_history {
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "command-suggestion-del-{index}"
                        )))
                        .flex_none()
                        .w(px(20.))
                        .h(px(20.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .hover(|this| {
                            this.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.danger))
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_command_suggestion_history(delete_command.clone(), cx);
                        }))
                        .child(
                            svg()
                                .size(px(13.))
                                .path("icons/fe/delete.svg")
                                .text_color(rgb(palette.text_dimmed)),
                        ),
                );
            }
            list = list.child(row);
        }

        div()
            .id(SharedString::from("command-suggestions-overlay"))
            .absolute()
            .occlude()
            .left(px(placement.x))
            .top(px(placement.y))
            .w(px(menu_w))
            .h(px(placement.height))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgba((palette.surface << 8) | 0xf2))
            .shadow_lg()
            .when_some(selected_preview, |this, command| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(placement.height + 6.))
                        .left_0()
                        .w_full()
                        .max_w(px(560.))
                        .p_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.surface_elevated))
                        .shadow_lg()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(11.))
                        .text_color(rgb(palette.text))
                        .whitespace_normal()
                        .child(command),
                )
            })
            .child(
                div()
                    .h(px(SUGGESTION_OVERLAY_HEADER_HEIGHT))
                    .flex_none()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(10.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/commands.svg")
                                    .text_color(rgb(palette.text_dimmed)),
                            )
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(format!("{} {match_label}", state.items.len())),
                    ),
            )
            .child(list)
            .child(
                div()
                    .h(px(SUGGESTION_OVERLAY_FOOTER_HEIGHT))
                    .flex_none()
                    .px_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(footer),
            )
            .into_any_element()
    }

    pub(in crate::features) fn suggestion_overlay_position_for_session(
        &self,
        session_id: Option<&str>,
        cursor_row: usize,
        cursor_col: usize,
        menu_w: f32,
        menu_h: f32,
    ) -> Option<SuggestionOverlayPlacement> {
        let session_id = session_id.or_else(|| self.session.active_id())?;
        // Command and credential popups share cached terminal geometry, which
        // remains available after a different workspace tab or page takes over.
        if !self.terminal_session_has_visible_surface(session_id) {
            return None;
        }
        let bounds = self.terminal_surface_bounds_for_session(Some(session_id))?;
        let insets = self.terminal_content_insets_for_bounds(Some(session_id), bounds);
        suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds,
                cell_size: self.terminal_cell_size(),
                content_origin: (insets.left, insets.top),
                gutter: self.terminal_gutter_width_px_for_session(Some(session_id)),
                viewport_size: self.shell.viewport_size(),
            },
            SuggestionOverlayTarget {
                cursor: (cursor_row, cursor_col),
                menu_size: (menu_w, menu_h),
            },
        )
    }
}

fn command_suggestion_state_changed(
    current: Option<&CommandSuggestionState>,
    next: &CommandSuggestionState,
) -> bool {
    current != Some(next)
}

fn command_suggestion_item_for_selection(
    items: &[CommandSuggestionItem],
    selected_index: Option<usize>,
) -> Option<&CommandSuggestionItem> {
    items.get(selected_index?)
}

fn command_suggestion_step_selection(
    current: Option<usize>,
    len: usize,
    direction: i32,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if direction > 0 {
        return match current {
            None => Some(0),
            Some(index) if index + 1 < len => Some(index + 1),
            Some(_) => None,
        };
    }
    if direction < 0 {
        return match current {
            None => Some(len - 1),
            Some(0) => None,
            Some(index) => Some(index.min(len - 1).saturating_sub(1)),
        };
    }
    command_suggestion_clamp_selection(current, len)
}

fn command_suggestion_clamp_selection(current: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    current.map(|index| index.min(len - 1))
}

pub(in crate::features) fn suggestion_overlay_desired_height(
    item_count: usize,
    row_height: f32,
) -> f32 {
    (item_count as f32 * row_height + SUGGESTION_OVERLAY_CHROME_HEIGHT)
        .min(SUGGESTION_OVERLAY_MAX_HEIGHT)
}

pub(in crate::features) fn suggestion_overlay_position(
    geometry: SuggestionOverlayGeometry,
    target: SuggestionOverlayTarget,
) -> Option<SuggestionOverlayPlacement> {
    let SuggestionOverlayGeometry {
        bounds,
        cell_size,
        content_origin: (pad_left, pad_top),
        gutter,
        viewport_size,
    } = geometry;
    let SuggestionOverlayTarget {
        cursor: (cursor_row, cursor_col),
        menu_size: (menu_w, menu_h),
    } = target;
    let (cell_w, cell_h) = cell_size;
    let bounds_x = f32::from(bounds.origin.x);
    let bounds_y = f32::from(bounds.origin.y);
    let base_x = bounds_x + pad_left + gutter + cursor_col as f32 * cell_w;
    let cursor_top = bounds_y + pad_top + cursor_row as f32 * cell_h;
    let cursor_bottom = cursor_top + cell_h;
    let (viewport_w, viewport_h) = viewport_size;
    let surface_top = bounds_y.max(SUGGESTION_OVERLAY_VIEWPORT_MARGIN);
    // Suggestions live in the window overlay, so panels below the terminal do
    // not limit the available space. Keep the terminal bounds only as the anchor.
    let overlay_bottom = viewport_h - SUGGESTION_OVERLAY_VIEWPORT_MARGIN;
    if overlay_bottom <= surface_top || menu_h <= 0.0 {
        return None;
    }

    let below_y = cursor_bottom + SUGGESTION_OVERLAY_CURSOR_GAP;
    let above_bottom = cursor_top - SUGGESTION_OVERLAY_CURSOR_GAP;
    let space_below = (overlay_bottom - below_y).max(0.0);
    let space_above = (above_bottom - surface_top).max(0.0);
    let (y, height) = if menu_h <= space_below {
        (below_y, menu_h)
    } else if menu_h <= space_above {
        (above_bottom - menu_h, menu_h)
    } else if space_above > space_below {
        (above_bottom - space_above, space_above)
    } else {
        (below_y, space_below)
    };
    if height <= 0.0 {
        return None;
    }

    let max_x = (viewport_w - menu_w - SUGGESTION_OVERLAY_VIEWPORT_MARGIN)
        .max(SUGGESTION_OVERLAY_VIEWPORT_MARGIN);
    Some(SuggestionOverlayPlacement {
        x: base_x.clamp(SUGGESTION_OVERLAY_VIEWPORT_MARGIN, max_x),
        y,
        height,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct SuggestionOverlayPlacement {
    pub x: f32,
    pub y: f32,
    pub height: f32,
}

#[derive(Clone, Copy)]
pub(in crate::features) struct SuggestionOverlayGeometry {
    pub bounds: Bounds<Pixels>,
    pub cell_size: (f32, f32),
    pub content_origin: (f32, f32),
    pub gutter: f32,
    pub viewport_size: (f32, f32),
}

#[derive(Clone, Copy)]
pub(in crate::features) struct SuggestionOverlayTarget {
    pub cursor: (usize, usize),
    pub menu_size: (f32, f32),
}

#[cfg(test)]
mod overlay_position_tests {
    use gpui::{Bounds, point, px, size};

    use super::{
        SuggestionOverlayGeometry, SuggestionOverlayTarget, suggestion_overlay_desired_height,
        suggestion_overlay_position,
    };

    #[test]
    fn suggestion_overlay_position_anchors_below_cursor() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(10.0), px(20.0)), size(px(800.0), px(400.0))),
                cell_size: (8.0, 16.0),
                content_origin: (8.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 768.0),
            },
            SuggestionOverlayTarget {
                cursor: (2, 4),
                menu_size: (300.0, 120.0),
            },
        )
        .expect("surface has room below the cursor");

        assert_eq!(placement.x, 50.0);
        assert_eq!(placement.y, 72.0);
        assert_eq!(placement.height, 120.0);
    }

    #[test]
    fn suggestion_overlay_position_clamps_and_flips_above_cursor() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(700.0), px(100.0)), size(px(200.0), px(500.0))),
                cell_size: (8.0, 16.0),
                content_origin: (8.0, 0.0),
                gutter: 0.0,
                viewport_size: (900.0, 620.0),
            },
            SuggestionOverlayTarget {
                cursor: (24, 30),
                menu_size: (300.0, 140.0),
            },
        )
        .expect("surface has room above the cursor");

        assert_eq!(placement.x, 592.0);
        assert_eq!(placement.y, 340.0);
        assert_eq!(placement.height, 140.0);
        assert_eq!(placement.y + placement.height, 480.0);
    }

    #[test]
    fn suggestion_overlay_position_limits_height_without_covering_cursor() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(180.0))),
                cell_size: (8.0, 16.0),
                content_origin: (0.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 280.0),
            },
            SuggestionOverlayTarget {
                cursor: (6, 4),
                menu_size: (300.0, 140.0),
            },
        )
        .expect("surface has constrained room above the cursor");

        assert_eq!(placement.y, 100.0);
        assert_eq!(placement.height, 92.0);
        assert_eq!(placement.y + placement.height, 192.0);
    }

    #[test]
    fn suggestion_overlay_position_uses_space_below_terminal_surface() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(200.0))),
                cell_size: (8.0, 16.0),
                content_origin: (0.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 768.0),
            },
            SuggestionOverlayTarget {
                cursor: (11, 4),
                menu_size: (380.0, 320.0),
            },
        )
        .expect("window has room below the terminal over the bottom panel");

        assert_eq!(placement.y, 296.0);
        assert_eq!(placement.height, 320.0);
    }

    #[test]
    fn suggestion_overlay_position_limits_height_at_window_bottom() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(120.0))),
                cell_size: (8.0, 16.0),
                content_origin: (0.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 330.0),
            },
            SuggestionOverlayTarget {
                cursor: (6, 4),
                menu_size: (300.0, 140.0),
            },
        )
        .expect("window has more room below the cursor than above");

        assert_eq!(placement.y, 216.0);
        assert_eq!(placement.height, 106.0);
        assert_eq!(placement.y + placement.height, 322.0);
    }

    #[test]
    fn suggestion_overlay_height_is_content_sized_and_capped() {
        assert_eq!(suggestion_overlay_desired_height(1, 28.0), 76.0);
        assert_eq!(suggestion_overlay_desired_height(10, 28.0), 272.0);
        assert_eq!(suggestion_overlay_desired_height(1, 36.0), 84.0);
        assert_eq!(suggestion_overlay_desired_height(8, 36.0), 272.0);
    }
}

fn terminal_line_prefix_for_cell_col(line: &str, cell_col: usize) -> String {
    let end = terminal_byte_index_for_cell_col(line, cell_col);
    line.get(..end).unwrap_or(line).to_string()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, KeyDownEvent, TestAppContext};
    use nyaterm_core::{TerminalInputState, apply_terminal_input_data};

    use crate::features::NyaTermApp;
    use crate::features::terminal::terminal_runtime::TERMINAL_INPUT_LATENCY_WINDOW;
    use crate::models::{CommandSuggestionItem, CommandSuggestionState};
    use crate::test_support::TestConfigDir;

    use super::{
        command_history_input_update, command_suggestion_clamp_selection,
        command_suggestion_input_can_defer_refresh, command_suggestion_input_candidate_chars,
        command_suggestion_input_obvious_pager_prefix, command_suggestion_item_for_selection,
        command_suggestion_refresh_input_delay, command_suggestion_state_changed,
        command_suggestion_step_selection, terminal_line_prefix_for_cell_col,
    };

    #[test]
    fn disabling_credential_autofill_releases_command_suggestions_and_smart_input() {
        let dir = TestConfigDir::new("nyaterm-credential-mode-cancel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"echo abc", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ echo abc".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.prepare_command_suggestion_search(1, cx).is_some());
            assert!(app.can_use_smart_cursor_selection());
            app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
            assert!(app.is_credential_prompt_input_mode());
            assert!(app.prepare_command_suggestion_search(1, cx).is_none());
            assert!(!app.can_use_smart_cursor_selection());

            // The toggle must release these gates without waiting for output or a timeout.
            app.toggle_credential_autofill(cx);
            assert!(!app.is_credential_prompt_input_mode());
            assert!(app.prepare_command_suggestion_search(1, cx).is_some());
            assert!(app.can_use_smart_cursor_selection());
        });
    }

    #[test]
    fn uncertain_input_cannot_start_or_publish_a_suggestion_search() {
        let dir = TestConfigDir::new("nyaterm-suggestions-confidence");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"echo", cx);
            let request = app
                .prepare_command_suggestion_search(1, cx)
                .expect("ordinary typing predictions can request suggestions");
            app.terminal.editing.state_mut().model.invalidate();
            assert!(app.prepare_command_suggestion_search(1, cx).is_none());
            app.publish_command_suggestion_search(
                request,
                vec![nyaterm_core::FuzzyResult {
                    command: "echo example".into(),
                    source: "history".into(),
                    score: 1,
                    indices: vec![],
                    display: "echo example".into(),
                }],
                Duration::ZERO,
                cx,
            );
            assert!(app.terminal.assist.command_suggestions.is_none());
            app.terminal.assist.command_suggestions = Some(open_suggestions_popup("s1"));
            assert!(app.apply_selected_command_suggestion(true, cx));
            assert!(app.terminal.assist.pending_command_history_entry.is_none());
        });
    }

    #[test]
    fn terminal_line_prefix_uses_terminal_cells_for_wide_chars() {
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 0), "");
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 1), "");
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 2), "界");
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 3), "界x");
    }

    #[test]
    fn terminal_line_prefix_keeps_combining_mark_with_base_char() {
        let text = "e\u{301}x";

        assert_eq!(terminal_line_prefix_for_cell_col(text, 0), "");
        assert_eq!(terminal_line_prefix_for_cell_col(text, 1), "e\u{301}");
        assert_eq!(terminal_line_prefix_for_cell_col(text, 2), "e\u{301}x");
    }

    #[test]
    fn command_suggestion_refresh_input_delay_waits_for_terminal_idle_window() {
        let now = Instant::now();
        assert_eq!(
            command_suggestion_refresh_input_delay(Some(now), now),
            Some(TERMINAL_INPUT_LATENCY_WINDOW)
        );
        assert_eq!(
            command_suggestion_refresh_input_delay(
                Some(now - TERMINAL_INPUT_LATENCY_WINDOW - Duration::from_millis(1)),
                now
            ),
            None
        );
        assert_eq!(command_suggestion_refresh_input_delay(None, now), None);
    }

    #[test]
    fn command_suggestion_input_defer_refresh_requires_cursor_at_end() {
        let mut state = TerminalInputState {
            value: "git status".to_string(),
            cursor: 3,
            ..TerminalInputState::default()
        };
        assert!(!command_suggestion_input_can_defer_refresh(&state));

        state.cursor = state.value.len();
        assert!(command_suggestion_input_can_defer_refresh(&state));

        state.desynced = true;
        assert!(!command_suggestion_input_can_defer_refresh(&state));
    }

    #[test]
    fn command_suggestion_input_detects_obvious_pager_prefix_without_sanitizing() {
        let state = TerminalInputState {
            value: "  /search".to_string(),
            cursor: 9,
            ..TerminalInputState::default()
        };
        assert!(command_suggestion_input_obvious_pager_prefix(&state));

        let state = TerminalInputState {
            value: "git status".to_string(),
            cursor: 10,
            ..TerminalInputState::default()
        };
        assert!(!command_suggestion_input_obvious_pager_prefix(&state));
        assert_eq!(command_suggestion_input_candidate_chars(&state), 10);
    }

    #[test]
    fn command_suggestion_state_change_skips_identical_overlay() {
        let state = CommandSuggestionState {
            session_id: "session".to_string(),
            input_version: 0,
            sync_revision: 0,
            draft: "git".to_string(),
            items: vec![CommandSuggestionItem {
                command: "git status".to_string(),
                display: "git status".to_string(),
                source: "history".to_string(),
                score: 42,
                indices: vec![0, 1, 2],
            }],
            selected_index: None,
            cursor_row: 3,
            cursor_col: 4,
        };
        let mut selected = state.clone();
        selected.selected_index = Some(0);

        assert!(!command_suggestion_state_changed(Some(&state), &state));
        assert!(command_suggestion_state_changed(None, &state));
        assert!(command_suggestion_state_changed(Some(&state), &selected));
    }

    #[test]
    fn command_suggestion_step_selection_enters_and_exits_with_down() {
        assert_eq!(command_suggestion_step_selection(None, 3, 1), Some(0));
        assert_eq!(command_suggestion_step_selection(Some(0), 3, 1), Some(1));
        assert_eq!(command_suggestion_step_selection(Some(2), 3, 1), None);
        assert_eq!(command_suggestion_step_selection(None, 0, 1), None);
    }

    #[test]
    fn command_suggestion_step_selection_enters_and_exits_with_up() {
        assert_eq!(command_suggestion_step_selection(None, 3, -1), Some(2));
        assert_eq!(command_suggestion_step_selection(Some(2), 3, -1), Some(1));
        assert_eq!(command_suggestion_step_selection(Some(0), 3, -1), None);
        assert_eq!(command_suggestion_step_selection(None, 0, -1), None);
    }

    #[test]
    fn command_suggestion_clamp_selection_preserves_none_and_clamps_overflow() {
        assert_eq!(command_suggestion_clamp_selection(None, 3), None);
        assert_eq!(command_suggestion_clamp_selection(Some(1), 3), Some(1));
        assert_eq!(command_suggestion_clamp_selection(Some(7), 3), Some(2));
        assert_eq!(command_suggestion_clamp_selection(Some(0), 0), None);
    }

    #[test]
    fn command_suggestion_accept_requires_selected_item() {
        let items = vec![CommandSuggestionItem {
            command: "git status".to_string(),
            display: "git status".to_string(),
            source: "history".to_string(),
            score: 42,
            indices: vec![0, 1, 2],
        }];

        assert!(command_suggestion_item_for_selection(&items, None).is_none());
        assert!(command_suggestion_item_for_selection(&items, Some(1)).is_none());
        assert_eq!(
            command_suggestion_item_for_selection(&items, Some(0))
                .map(|item| item.command.as_str()),
            Some("git status")
        );
    }

    #[test]
    fn command_history_input_update_captures_submission_before_enter_reset() {
        let mut state = apply_terminal_input_data(&TerminalInputState::new(), "git status");

        let submitted = command_history_input_update(&mut state, "\r");

        assert_eq!(submitted.as_deref(), Some("git status"));
        assert!(state.value.is_empty());
    }

    fn escape_key_event() -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers::default(),
                key: "escape".to_string(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    /// A visible surface makes the popup authoritative for the session, so the
    /// key handler can only bail out for the reason under test.
    pub(super) fn show_surface_for_session(app: &mut NyaTermApp, session_id: &str) {
        app.terminal.layout.session_surface_bounds.insert(
            session_id.to_string(),
            gpui::Bounds::new(
                gpui::point(gpui::px(0.0), gpui::px(0.0)),
                gpui::size(gpui::px(800.0), gpui::px(600.0)),
            ),
        );
    }

    fn open_suggestions_popup(session_id: &str) -> CommandSuggestionState {
        CommandSuggestionState {
            session_id: session_id.to_string(),
            input_version: 0,
            sync_revision: 0,
            draft: "1".to_string(),
            items: vec![CommandSuggestionItem {
                command: "1111".to_string(),
                display: "1111".to_string(),
                source: "history".to_string(),
                score: 1,
                indices: Vec::new(),
            }],
            selected_index: Some(0),
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    #[test]
    fn selecting_suggestions_keeps_position_when_snapshot_history_window_changes() {
        let dir = TestConfigDir::new("nyaterm-suggestion-anchor");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );

        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            let view = app.terminal.view.views.get_mut("s1").unwrap();
            view.screen.resize(80, 24);
            view.screen.advance_decoded_text(&"output\r\n".repeat(100));
            view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));

            let (cursor_row, cursor_col) = app.active_terminal_cursor_cell();
            let mut state = open_suggestions_popup("s1");
            state.selected_index = None;
            state.cursor_row = cursor_row;
            state.cursor_col = cursor_col;
            app.terminal.assist.command_suggestions = Some(state);
            let before = app
                .suggestion_overlay_position_for_session(
                    Some("s1"),
                    cursor_row,
                    cursor_col,
                    380.0,
                    272.0,
                )
                .unwrap();

            // Worker frames may grow or shrink history padding without moving the cursor.
            for (older_rows, key) in [(64, "down"), (0, "up"), (96, "down")] {
                let view = app.terminal.view.views.get_mut("s1").unwrap();
                view.frame_snapshot = Some(std::sync::Arc::new(
                    view.screen.viewport_snapshot_with_window(0, older_rows, 0),
                ));
                assert_eq!(app.active_terminal_cursor_cell(), (cursor_row, cursor_col));
                let mut event = escape_key_event();
                event.keystroke.key = key.to_string();
                assert!(app.handle_command_suggestion_key(&event, cx));
                let state = app.terminal.assist.command_suggestions.as_ref().unwrap();
                assert_eq!(state.selected_index, (key == "down").then_some(0));
                let after = app
                    .suggestion_overlay_position_for_session(
                        Some("s1"),
                        state.cursor_row,
                        state.cursor_col,
                        380.0,
                        272.0,
                    )
                    .unwrap();
                assert_eq!(after, before);
            }
        });
    }

    #[test]
    fn suggestion_cursor_anchor_accounts_for_leading_cleared_rows() {
        let dir = TestConfigDir::new("nyaterm-suggestion-cleared-anchor");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, _| {
            let view = app.terminal.view.views.get_mut("s1").unwrap();
            view.screen.resize(80, 24);
            view.screen.advance(b"\x1b[24;1Hroot# draft");
            view.screen.clear_except_input();
            view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));

            assert_eq!(app.active_terminal_cursor_cell(), (0, 11));
        });
    }

    #[test]
    fn alternate_screen_keeps_command_suggestions_from_intercepting_keys() {
        let dir = TestConfigDir::new("nyaterm-alt-screen-suggestions");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );

        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            assert!(!app.active_terminal_uses_alternate_screen());

            app.terminal.assist.command_suggestions = Some(open_suggestions_popup("s1"));
            // Shell line: the popup owns Escape.
            assert!(app.handle_command_suggestion_key(&escape_key_event(), cx));
            assert!(app.terminal.assist.command_suggestions.is_none());
        });

        cx.update_entity(&app, |app, _| {
            // A full-screen program (vim, less, top) takes over the terminal.
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .append_bytes_unprotected(b"\x1b[?1049h");
        });

        cx.update_entity(&app, |app, cx| {
            assert!(app.active_terminal_uses_alternate_screen());

            app.terminal.assist.command_suggestions = Some(open_suggestions_popup("s1"));
            // Escape must reach the program instead of closing a stale popup.
            assert!(!app.handle_command_suggestion_key(&escape_key_event(), cx));
            assert!(app.terminal.assist.command_suggestions.is_none());

            // Typing inside the program neither suggests nor records history.
            app.note_command_suggestion_input(b"11", cx);
            assert!(app.terminal.assist.command_suggestions.is_none());
            assert_eq!(
                app.terminal.editing.predicted_input(),
                &TerminalInputState::new()
            );

            *app.terminal.editing.input_mut() =
                apply_terminal_input_data(&TerminalInputState::new(), "vim ");
            app.note_shell_editing_input(b"a.txt\r", cx);
            assert!(app.terminal.assist.pending_command_history_entry.is_none());
            assert_eq!(
                app.terminal.editing.predicted_input(),
                &TerminalInputState::new()
            );
        });
    }

    #[test]
    fn alternate_screen_blocks_the_manual_trigger_and_the_deferred_publisher() {
        let dir = TestConfigDir::new("nyaterm-alt-screen-manual");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );

        cx.update_entity(&app, |app, _| {
            show_surface_for_session(app, "s1");
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .append_bytes_unprotected(b"\x1b[?1049h");
        });

        cx.update_entity(&app, |app, cx| {
            assert!(app.active_terminal_uses_alternate_screen());
            *app.terminal.editing.input_mut() =
                apply_terminal_input_data(&TerminalInputState::new(), "vim 1");

            // An explicit trigger must not open the popup inside a full-screen
            // program, even with a trackable shell-style line.
            app.show_manual_command_suggestions(cx);
            assert!(app.terminal.assist.command_suggestions.is_none());

            // A search deferred on the shell line would otherwise land after the
            // program has taken the screen; the request must be dropped.
            assert!(app.prepare_command_suggestion_search(1, cx).is_none());
            assert!(app.terminal.assist.command_suggestions.is_none());
        });
    }

    #[test]
    fn alternate_screen_drops_a_search_that_was_already_in_flight() {
        let dir = TestConfigDir::new("nyaterm-alt-screen-inflight");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );

        let request = cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            assert!(!app.active_terminal_uses_alternate_screen());
            *app.terminal.editing.input_mut() =
                apply_terminal_input_data(&TerminalInputState::new(), "vim 1");
            app.terminal.editing.state_mut().model.confirm();
            app.prepare_command_suggestion_search(1, cx)
                .expect("shell line should still build a request")
        });

        // The user starts a full-screen program while the search is running.
        cx.update_entity(&app, |app, _| {
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .append_bytes_unprotected(b"\x1b[?1049h");
        });

        cx.update_entity(&app, |app, cx| {
            assert!(app.active_terminal_uses_alternate_screen());
            let results = vec![nyaterm_core::FuzzyResult {
                command: "vim 1111".to_string(),
                score: 1,
                indices: Vec::new(),
                source: "history".to_string(),
                display: "vim 1111".to_string(),
            }];
            app.publish_command_suggestion_search(request, results, Duration::ZERO, cx);
            assert!(app.terminal.assist.command_suggestions.is_none());
        });
    }

    #[test]
    fn leaving_the_alternate_screen_restores_shell_line_suggestions() {
        let dir = TestConfigDir::new("nyaterm-alt-screen-return");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );

        cx.update_entity(&app, |app, _| {
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .append_bytes_unprotected(b"\x1b[?1049h");
        });
        cx.update_entity(&app, |app, _| {
            assert!(app.active_terminal_uses_alternate_screen());
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .append_bytes_unprotected(b"\x1b[?1049l");
        });

        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            assert!(!app.active_terminal_uses_alternate_screen());
            app.terminal.assist.command_suggestions = Some(open_suggestions_popup("s1"));
            assert!(app.handle_command_suggestion_key(&escape_key_event(), cx));
        });
    }
}
