use rust_i18n::t;

use nyaterm_ui::{NyaScrollable, NyaTooltip};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::features::commands::completion_runtime::{CompletionQuery, CompletionRuntime};
use gpui::{
    Bounds, Context, FontWeight, IntoElement, KeyDownEvent, Pixels, SharedString, div, prelude::*,
    px, rgb, rgba, svg,
};
use nyaterm_core::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionFillPolicy, CompletionMode,
    CompletionResult, CompletionScope, ShellCommandContext, ShellDialect,
};
use nyaterm_core::terminal::editing::{
    EDIT_CONFIRMATION_TIMEOUT, EditIntent, ShellLineEditing, plan_edit,
};
use nyaterm_core::{QuickCommand, TerminalInputState, terminal_input_tracker_below_min_chars};

use crate::features::NyaTermApp;
use crate::features::terminal::editing_payload::shell_edit_payload_with_backend;
use crate::features::terminal::editing_state::{SnapshotEvidence, SuggestionFillRequest};
use crate::features::terminal::terminal_runtime::TERMINAL_INPUT_LATENCY_WINDOW;
use crate::models::{CommandSuggestionItem, CommandSuggestionState};
use crate::terminal::terminal_byte_index_for_cell_col;

mod fill;
#[cfg(test)]
mod fill_tests;
mod helpers;
pub(super) mod paste;
pub(super) mod path_source;
mod progressive;

use self::fill::{SuggestionSubmitResult, suggestion_fill_rejection_message};
use self::helpers::command_suggestion_highlight_parts;
use self::paste::SuggestionPasteContext;
use self::path_source::{PATH_CANDIDATE_LIMIT, PathSource};
use nyaterm_core::completion::fill::{SuggestionFill, plan_suggestion_fill};

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
const COMMAND_SUGGESTION_ROW_HEIGHT: f32 = 28.0;
const COMMAND_SUGGESTION_BORDER_HEIGHT: f32 = 2.0;
const SUGGESTION_PREVIEW_GAP: f32 = 6.0;
const SUGGESTION_PREVIEW_MIN_HEIGHT: f32 = 32.0;
const SUGGESTION_PREVIEW_MAX_HEIGHT: f32 = 120.0;

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
    update_state: Duration,
    hide_popup: Duration,
}

#[derive(Clone)]
struct CommandSuggestionSearchRequest {
    request_id: u64,
    session_id: String,
    input_version: u64,
    sync_revision: u64,
    pattern: String,
    pattern_chars: usize,
    min_chars: usize,
    max_chars: usize,
    mode: CompletionMode,
    context: ShellCommandContext,
    completion: CompletionRuntime,
    quick_commands: Arc<[QuickCommand]>,
    started_at: Instant,
    popup_visible_at_start: bool,
    timing: CommandSuggestionRefreshTiming,
    fill_policy: CompletionFillPolicy,
    path_source: Option<PathSource>,
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

fn command_suggestion_refresh_input_delay(
    last_terminal_input_at: Option<Instant>,
    now: Instant,
) -> Option<Duration> {
    last_terminal_input_at.and_then(|last| {
        let elapsed = now.saturating_duration_since(last);
        (elapsed < TERMINAL_INPUT_LATENCY_WINDOW).then(|| TERMINAL_INPUT_LATENCY_WINDOW - elapsed)
    })
}

/// Path rows lead once the user typed a directory part (`cd /va`); a bare
/// name (`cd pr`) keeps history and catalog rows first.
fn merge_path_candidates(
    mut result: CompletionResult,
    paths: Vec<CompletionCandidate>,
    explicit_directory: bool,
    input: &str,
) -> CompletionResult {
    if paths.is_empty() {
        return result;
    }
    let mut seen = std::collections::HashSet::new();
    let mut existing = std::mem::take(&mut result.candidates);
    let reserved: Vec<_> = existing
        .iter()
        .filter(|item| matches!(item.kind, CandidateKind::Option | CandidateKind::Subcommand))
        .take(3)
        .cloned()
        .collect();
    let mut merged = Vec::new();
    let mut add = |item: CompletionCandidate| {
        if let Some(target) = item.target_line(input)
            && seen.insert(target)
        {
            merged.push(item);
        }
    };
    for item in reserved {
        add(item);
    }
    existing.truncate(6);
    let paths = paths.into_iter().take(6);
    if !explicit_directory {
        for item in existing {
            add(item);
        }
        for item in paths {
            add(item);
        }
    } else {
        for item in paths {
            add(item);
        }
        for item in existing {
            add(item);
        }
    }
    merged.truncate(PATH_CANDIDATE_LIMIT);
    result.candidates = merged;
    result
}

fn preserve_suggestion_selection(
    previous: Option<&CommandSuggestionState>,
    next: &mut CommandSuggestionState,
) {
    if let Some(previous) = previous
        && previous.session_id == next.session_id
        && previous.mode == next.mode
        && previous.input_version == next.input_version
        && previous.draft == next.draft
    {
        next.selected_index = previous
            .selected_index
            .and_then(|i| previous.items.get(i))
            .and_then(|selected| {
                next.items
                    .iter()
                    .position(|item| item.command == selected.command)
            });
    }
}

/// True when `next` is `previous` plus characters typed at the end.
fn command_suggestion_draft_only_appended(previous: &str, next: &str) -> bool {
    !previous.is_empty() && next.len() > previous.len() && next.starts_with(previous)
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

/// Whether the tracked line holds any actual command text.
///
/// Remote pagers on switches consume keys such as Space without echoing them,
/// so the local tracker can hold nothing but separators while the remote screen
/// owns the cursor. That is not a command context: the token completer reads a
/// trailing separator as an empty first token and answers with the whole
/// catalog, and the whole-line search treats it as the manual empty query and
/// answers with recent history. Requiring real text keeps this structural
/// instead of matching any vendor's pager prompt.
fn command_suggestion_input_has_command_text(state: &TerminalInputState) -> bool {
    !state.value.trim().is_empty()
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
    pub(in crate::features::terminal) fn command_suggestion_fill_policy(
        &self,
        session_id: &str,
    ) -> CompletionFillPolicy {
        let shell_session = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Local(_)
                    | crate::models::SessionLaunchConfig::Ssh(_)
            )
        });
        if shell_session
            && self
                .terminal
                .editing
                .sessions
                .get(session_id)
                .is_some_and(|state| {
                    state.confirmed_input_at_end()
                        && state.allows_editing()
                        && state.editor_evidence.is_some()
                })
        {
            CompletionFillPolicy::ReplaceAllowed
        } else {
            CompletionFillPolicy::AppendOnly
        }
    }

    pub(in crate::features::terminal) fn command_suggestions_allowed_for_session(
        &self,
        session_id: &str,
    ) -> bool {
        self.session.active_id() == Some(session_id)
            && !self
                .terminal
                .editing
                .sessions
                .get(session_id)
                .is_some_and(|state| state.browsing_history)
            && !self.session.is_disconnected(session_id)
            && !self.active_terminal_input_blocked()
            && !self.security.screen_locked()
            && !self.active_terminal_uses_alternate_screen()
            && !self.is_credential_prompt_input_mode()
            && self.terminal.assist.credential_suggestions.is_none()
            && !self
                .terminal
                .assist
                .command_suggestion_suppression
                .is_suppressed()
            && !self
                .terminal_protocol_state_for_session(session_id)
                .mouse_reporting
            && self.active_terminal_display_offset() == 0
            && self
                .settings
                .summary()
                .interaction_command_suggestions_enabled
            && !self.sync_input.may_fan_out_from(session_id)
    }

    pub(in crate::features::terminal) fn command_suggestion_input_is_confirmed(&self) -> bool {
        if self
            .session
            .active_id()
            .is_none_or(|id| !self.terminal.editing.is_active(id))
        {
            return false;
        }
        self.terminal
            .editing
            .state()
            .is_some_and(|state| state.confirmed_input_at_end())
    }

    pub(in crate::features::terminal) fn retain_command_suggestions_during_edit(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(popup) = self.terminal.assist.command_suggestions.as_mut()
            && popup.begin_input_refresh(text)
        {
            cx.notify();
        }
    }

    fn command_suggestion_refresh_can_remain_visible(&self) -> bool {
        let Some(popup) = self.terminal.assist.command_suggestions.as_ref() else {
            return false;
        };
        let input = self.terminal.editing.assist_input();
        popup.refreshing
            && self.command_suggestions_allowed_for_session(&popup.session_id)
            && self.sync_input.revision() == popup.sync_revision
            && command_suggestion_input_can_defer_refresh(input)
            && !input.line_rewrite_required
            && !input.paste_mode
            && command_suggestion_input_has_command_text(input)
            && !command_suggestion_input_obvious_pager_prefix(input)
            && (popup.mode == CompletionMode::History
                || !terminal_input_tracker_below_min_chars(
                    input,
                    self.settings
                        .summary()
                        .interaction_command_suggestion_min_chars
                        .max(1) as usize,
                )
                || input.value.contains(char::is_whitespace))
    }

    /// Called from state/terminal events, never from element construction.
    pub(in crate::features) fn reconcile_command_suggestion_availability(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.command_suggestion_refresh_can_remain_visible() {
            return;
        }
        let Some(popup) = self.terminal.assist.command_suggestions.as_ref() else {
            return;
        };
        if popup.refreshing
            || !self.command_suggestions_allowed_for_session(&popup.session_id)
            || !self.command_suggestion_input_is_confirmed()
            || self.sync_input.revision() != popup.sync_revision
        {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        let draft = self.terminal.editing.assist_input().value.clone();
        let version = self.terminal.editing.state().unwrap().model.version;
        if draft != popup.draft || version != popup.input_version {
            // Typing forward only narrows the visible rows, so keep the popup
            // instead of flickering until the debounced search lands. Any
            // other edit (backspace, mid-line, paste) still hides it.
            if !command_suggestion_draft_only_appended(&popup.draft, &draft) {
                self.hide_command_suggestions_if_present(cx);
                return;
            }
            let popup = self.terminal.assist.command_suggestions.as_mut().unwrap();
            let selected = popup
                .selected_index
                .and_then(|index| popup.items.get(index))
                .map(|item| item.command.clone());
            popup.items.retain(|item| item.command.starts_with(&draft));
            popup.draft = draft;
            popup.input_version = version;
            popup.selected_index = selected
                .and_then(|command| popup.items.iter().position(|item| item.command == command));
            if popup.items.is_empty() {
                self.hide_command_suggestions_if_present(cx);
                return;
            }
            cx.notify();
        }
        let Some(popup) = self.terminal.assist.command_suggestions.as_ref() else {
            return;
        };
        let policy = self.command_suggestion_fill_policy(&popup.session_id);
        let popup = self.terminal.assist.command_suggestions.as_mut().unwrap();
        let selected_command = popup
            .selected_index
            .and_then(|index| popup.items.get(index))
            .map(|item| item.command.clone());
        let old_len = popup.items.len();
        popup
            .items
            .retain(|item| item.fillable && policy.allows(&popup.draft, &item.command));
        if popup.items.len() != old_len {
            popup.selected_index = selected_command
                .and_then(|command| popup.items.iter().position(|item| item.command == command));
            if popup.items.is_empty() {
                self.hide_command_suggestions_if_present(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(in crate::features) fn completion_has_marked_shell(&self, session_id: &str) -> bool {
        self.terminal
            .editing
            .sessions
            .get(session_id)
            .is_some_and(|state| state.observed_anchor.flatten().is_some())
    }

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
        self.terminal.assist.command_suggestion_mode = CompletionMode::Token;
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
        self.terminal.assist.command_suggestion_mode = CompletionMode::Token;
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

        // Typing invalidates an explicit choice before the debounced search can
        // publish its replacement. Enter must not accept the old selection.
        if let Some(popup) = self.terminal.assist.command_suggestions.as_mut()
            && popup.selected_index.take().is_some()
        {
            cx.notify();
        }

        if self
            .terminal
            .assist
            .command_suggestion_suppression
            .is_suppressed()
        {
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
        let below_min_chars = self.terminal.assist.command_suggestion_mode == CompletionMode::Token
            && terminal_input_tracker_below_min_chars(
                self.terminal.editing.assist_input(),
                min_chars,
            )
            && !self
                .terminal
                .editing
                .assist_input()
                .value
                .contains(char::is_whitespace);
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

        // Automatic suggestions need real command text. A pager keypress (for
        // example Space on a switch's `--More--` prompt) reaches the tracker as
        // ordinary input, and only whitespace means no local command line grew.
        if self.terminal.assist.command_suggestion_mode == CompletionMode::Token
            && !command_suggestion_input_has_command_text(self.terminal.editing.assist_input())
        {
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
            finish!("blank_input", 0);
        }

        let pattern_started_at = Instant::now();
        let pattern_chars =
            command_suggestion_input_candidate_chars(self.terminal.editing.assist_input());
        timing.pattern = pattern_started_at.elapsed();

        let pager_started_at = Instant::now();
        let pager_input =
            command_suggestion_input_obvious_pager_prefix(self.terminal.editing.assist_input());
        timing.pager = pager_started_at.elapsed();
        if pager_input && self.terminal.assist.command_suggestion_mode == CompletionMode::Token {
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
        let mode = self.terminal.assist.command_suggestion_mode;
        self.schedule_command_suggestion_search_mode(delay, mode, cx);
    }

    fn schedule_command_suggestion_search_mode(
        &mut self,
        delay: Duration,
        mode: CompletionMode,
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
                        // The visible rows were already narrowed to the current
                        // draft, so keep them; echo of typed keys is also output.
                        this.schedule_command_suggestion_refresh_after(
                            COMMAND_SUGGESTION_REFRESH_PRESSURE_RETRY,
                            cx,
                        );
                        return None;
                    }
                    this.prepare_command_suggestion_search(request_id, mode, cx)
                });
                let Ok(Some(request)) = request else {
                    return;
                };
                let search_started_at = Instant::now();
                let path_task = request.path_source.clone().map(|source| {
                    let input = request.pattern.clone();
                    cx.background_executor()
                        .spawn(async move { source.candidates(&input).await })
                });
                let completion = request.completion.clone();
                let ordinary = completion.search(CompletionQuery {
                    fill_policy: Some(request.fill_policy),
                    context: request.context.clone(),
                    input: request.pattern.clone(),
                    mode: request.mode,
                    quick: request.quick_commands.clone(),
                    min: request.min_chars,
                    max: request.max_chars,
                });
                let paths = async move {
                    match path_task {
                        Some(task) => task.await,
                        None => Vec::new(),
                    }
                };
                progressive::publish_progressively(ordinary, paths, |result, paths| {
                    let mut publication = request.clone();
                    let result = match (paths, publication.path_source.as_ref()) {
                        (Some(paths), Some(source)) => merge_path_candidates(
                            result,
                            paths,
                            !source.query().dir_text.is_empty(),
                            &publication.pattern,
                        ),
                        _ => {
                            publication.path_source = None;
                            result
                        }
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.publish_command_suggestion_search(
                            publication,
                            result,
                            search_started_at.elapsed(),
                            cx,
                        );
                    });
                })
                .await;
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
            || self
                .terminal
                .assist
                .command_suggestion_suppression
                .is_suppressed()
            || !self
                .settings
                .summary()
                .interaction_command_suggestions_enabled
        {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        let Some(_session_id) = self
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

        self.terminal.editing.state_mut().browsing_history = false;
        self.terminal.assist.command_suggestion_mode = CompletionMode::History;
        self.schedule_command_suggestion_search_mode(
            COMMAND_SUGGESTION_REFRESH_DEBOUNCE,
            CompletionMode::History,
            cx,
        );
    }

    fn command_suggestion_context(&self, session: &str) -> ShellCommandContext {
        let mut context =
            self.commands
                .command_context(session)
                .cloned()
                .unwrap_or(ShellCommandContext {
                    scope: CompletionScope::legacy(),
                    dialect: ShellDialect::Unknown,
                    command_events: false,
                });
        if let Some(evidence) = self
            .terminal
            .editing
            .sessions
            .get(session)
            .and_then(|state| state.editor_evidence)
        {
            if let Some(dialect) = evidence.dialect {
                context.dialect = dialect;
            } else if evidence.platform
                == Some(nyaterm_core::action_links::ActionLinkPlatform::Posix)
                && matches!(
                    context.dialect,
                    ShellDialect::Cmd | ShellDialect::PowerShell
                )
            {
                // A POSIX prompt cannot identify bash vs zsh, but it disproves
                // a stale Windows parsing hint after leaving a nested shell.
                context.dialect = ShellDialect::Unknown;
            }
        }
        context
    }

    /// Line-editing backend of the active session's shell.
    ///
    /// Only current snapshot evidence selects a destructive edit's encoding.
    fn shell_line_editing(&self, session_id: &str) -> Option<ShellLineEditing> {
        self.terminal
            .editing
            .sessions
            .get(session_id)?
            .editor_evidence
            .map(|evidence| evidence.line_editing)
    }

    fn prepare_command_suggestion_search(
        &mut self,
        request_id: u64,
        mode: CompletionMode,
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
        if !self.command_suggestions_allowed_for_session(&session_id)
            || !self.command_suggestion_input_is_confirmed()
        {
            self.log_command_suggestions_unavailable(&session_id);
            if !self.command_suggestion_refresh_can_remain_visible() {
                self.hide_command_suggestions_if_present(cx);
            }
            finish_refresh!("not_confirmed_or_restricted", 0, 0);
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
        let pattern = self.terminal.editing.assist_input().value.clone();
        let pattern_chars = pattern.chars().count();
        timing.pattern = pattern_started_at.elapsed();
        if !command_suggestion_input_can_defer_refresh(self.terminal.editing.assist_input())
            || (mode == CompletionMode::Token
                && !command_suggestion_input_has_command_text(self.terminal.editing.assist_input()))
        {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("not_eligible", pattern_chars, 0);
        }
        if mode == CompletionMode::Token
            && pattern_chars < min_chars
            && !pattern.contains(char::is_whitespace)
        {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("below_min_chars", pattern_chars, 0);
        }
        // Pager/search-like prefixes: hide suggestions.
        if mode == CompletionMode::Token
            && (pattern.starts_with('/') || pattern.starts_with('?') || pattern.starts_with(':'))
        {
            let hide_started_at = Instant::now();
            self.hide_command_suggestions_if_present(cx);
            timing.hide_popup = hide_started_at.elapsed();
            finish_refresh!("pager_prefix", pattern_chars, 0);
        }
        let context = self.command_suggestion_context(&session_id);
        let path_source = (mode == CompletionMode::Token)
            .then(|| self.path_source_for_session(&session_id, &pattern, context.dialect))
            .flatten();
        Some(CommandSuggestionSearchRequest {
            request_id,
            session_id: session_id.clone(),
            input_version: self.terminal.editing.state()?.model.version,
            sync_revision: self.sync_input.revision(),
            pattern,
            pattern_chars,
            min_chars,
            max_chars,
            mode,
            context,
            completion: self.commands.completion_runtime(),
            quick_commands: self.commands.quick_commands_snapshot(),
            started_at,
            popup_visible_at_start,
            timing,
            fill_policy: self.command_suggestion_fill_policy(&session_id),
            path_source,
        })
    }

    fn publish_command_suggestion_search(
        &mut self,
        request: CommandSuggestionSearchRequest,
        result: CompletionResult,
        search_duration: Duration,
        cx: &mut Context<Self>,
    ) {
        if request.path_source.as_ref().is_some_and(|source| {
            self.terminal
                .editing
                .sessions
                .get(&request.session_id)
                .and_then(|state| state.file_context.current())
                .is_none_or(|context| context.cache_key() != source.context())
        }) || self.commands.completion_runtime().revision() != result.revision
            || self.command_suggestion_context(&request.session_id).scope != request.context.scope
            || self.terminal.assist.command_suggestion_search_gen != request.request_id
            || self.session.active_id() != Some(request.session_id.as_str())
            || self
                .terminal
                .editing
                .state()
                .map(|state| state.model.version)
                != Some(request.input_version)
            || self.sync_input.revision() != request.sync_revision
            || self.terminal.editing.assist_input().value != request.pattern
        {
            return;
        }
        self.reconcile_command_suggestion_availability(cx);
        if !self.command_suggestions_allowed_for_session(&request.session_id)
            || !self.command_suggestion_input_is_confirmed()
        {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        if self.command_suggestion_fill_policy(&request.session_id) != request.fill_policy {
            self.refresh_command_suggestions(cx);
            return;
        }
        let results: Vec<_> = result
            .candidates
            .into_iter()
            .filter(|item| {
                item.fillable
                    && item
                        .target_line(&request.pattern)
                        .is_some_and(|target| request.fill_policy.allows(&request.pattern, &target))
            })
            .collect();
        let completion_revision = result.revision;
        let incremental = request.path_source.is_some();
        let scope = Some(request.context.scope.clone());
        let mode = request.mode;
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
        let update_started_at = Instant::now();
        let mut next_state = CommandSuggestionState {
            refreshing: false,
            mode,
            completion_revision,
            scope,
            session_id,
            input_version,
            sync_revision,
            draft: pattern.clone(),
            items: results
                .into_iter()
                .map(|item| completion_suggestion_item(item, &pattern))
                .collect(),
            selected_index: None,
        };
        if incremental {
            preserve_suggestion_selection(
                self.terminal.assist.command_suggestions.as_ref(),
                &mut next_state,
            );
        }
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
        self.terminal
            .assist
            .command_suggestion_scroll
            .set_offset(gpui::point(px(0.), px(0.)));
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

    /// Explains why no popup was offered. Flags only, never input text.
    fn log_command_suggestions_unavailable(&mut self, session_id: &str) {
        if !self.should_log_slow_diagnostic("terminal_suggestion_unavailable", Instant::now()) {
            return;
        }
        let allowed = self.command_suggestions_allowed_for_session(session_id);
        let state = self.terminal.editing.state();
        tracing::info!(
            diagnostic = "terminal_suggestion_unavailable",
            allowed,
            editing_active = self.terminal.editing.is_active(session_id),
            suppression = ?self.terminal.assist.command_suggestion_suppression,
            phase = ?state.map(|s| s.model.phase),
            has_mapping = state.is_some_and(|s| s.mapping.is_some()),
            awaiting_snapshot = state.is_some_and(|s| s.awaiting_snapshot),
            desynced = state.is_some_and(|s| s.model.input.desynced),
            multiline = state.is_some_and(|s| s.model.input.multiline),
            line_rewrite = state.is_some_and(|s| s.model.input.line_rewrite_required),
            "command suggestions unavailable"
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

    pub(in crate::features) fn suggestion_cursor_cell_for_session(
        &self,
        session_id: &str,
        cx: &gpui::App,
    ) -> Option<(usize, usize)> {
        if self.session.active_id() != Some(session_id) {
            return None;
        }
        if let Some(surface) = self.terminal.view.surfaces.get(session_id)
            && surface.read(cx).has_snapshot()
        {
            return surface.read(cx).suggestion_cursor_cell();
        }
        // Only read a committed frame: rendering must not synthesize history.
        let view = self.terminal.view.views.get(session_id)?;
        let snapshot = view.frame_snapshot.as_ref()?;
        nyaterm_terminal_gpui::geometry::terminal_cursor_display_cell(
            snapshot,
            self.active_terminal_display_offset(),
            view.viewport_rows_for_ui(),
            0.0,
        )
    }

    #[cfg(test)]
    pub(in crate::features) fn active_terminal_cursor_cell(&self) -> (usize, usize) {
        let session_id = self.session.active_id();
        let offset = self.active_terminal_display_offset();
        let snapshot = self.terminal_snapshot_for_session(session_id, offset);
        nyaterm_terminal_gpui::geometry::terminal_cursor_display_cell(
            &snapshot,
            offset,
            self.terminal_viewport_rows_for_session(session_id),
            0.0,
        )
        .unwrap_or((usize::MAX, snapshot.cursor.col))
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
        if state.refreshing {
            if !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.control
                && matches!(
                    event.keystroke.key.as_str(),
                    "escape" | "up" | "down" | "tab" | "enter"
                )
            {
                self.dismiss_command_suggestions(cx);
                return event.keystroke.key == "escape";
            }
            return false;
        }
        let session_id = state.session_id.clone();
        let selected =
            command_suggestion_item_for_selection(&state.items, state.selected_index).is_some();
        let accepting =
            selected && event.keystroke.key == "enter" && !event.keystroke.modifiers.modified();
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
        if keystroke.key == "enter" && keystroke.modifiers.modified() {
            return false;
        }
        if !selected && matches!(keystroke.key.as_str(), "tab" | "enter") {
            self.dismiss_command_suggestions(cx);
            return false;
        }
        if accepting || (selected && keystroke.key == "tab") {
            // A consumed suggestion key has no remote press owner. Its repeats
            // and Kitty release must not turn into a later command submission.
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                view.pressed_keys.remove(&keystroke.key);
            }
            if accepting && event.is_held {
                return true;
            }
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.dismiss_command_suggestions(cx);
                true
            }
            "up" | "down" | "tab" => {
                let direction = if keystroke.key == "up"
                    || (keystroke.key == "tab" && keystroke.modifiers.shift)
                {
                    -1
                } else {
                    1
                };
                if let Some(state) = self.terminal.assist.command_suggestions.as_mut() {
                    state.selected_index = command_suggestion_step_selection(
                        state.selected_index,
                        state.items.len(),
                        direction,
                    );
                    if let Some(index) = state.selected_index {
                        self.terminal
                            .assist
                            .command_suggestion_scroll
                            .scroll_to_item(index);
                    }
                    cx.notify();
                }
                true
            }
            "enter" => self.apply_selected_command_suggestion(false, cx),
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
        if popup.refreshing {
            return SuggestionSubmitResult::NotHandled;
        }
        let execute_requested = execute_requested && popup.mode == CompletionMode::History;
        let Some(item) = command_suggestion_item_for_selection(&popup.items, popup.selected_index)
        else {
            return SuggestionSubmitResult::NotHandled;
        };
        if !item.fillable {
            return self.reject_command_suggestion(
                "Cannot safely fill this match; edit the command manually",
                cx,
            );
        }
        let context = SuggestionPasteContext::from_popup(&popup);
        let snapshot = match self.validate_command_suggestion_input(&context, cx) {
            Ok(snapshot) => snapshot,
            Err(message) => return self.reject_command_suggestion(message, cx),
        };
        let state = self
            .terminal
            .editing
            .state()
            .expect("validated editing state");
        match plan_suggestion_fill(&state.model.input, &item.command) {
            SuggestionFill::Rejected(reason) => {
                return self
                    .reject_command_suggestion(suggestion_fill_rejection_message(reason), cx);
            }
            SuggestionFill::PasteReview(command) => {
                self.terminal
                    .paste
                    .queue_suggestion(context, command.to_owned());
                self.dismiss_command_suggestions(cx);
                cx.notify();
                return SuggestionSubmitResult::PasteReviewRequested;
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
            SuggestionFill::Append(suffix) => {
                // Logical input converts the charset and advances the shell
                // model once, after the primary write succeeds.
                self.dismiss_command_suggestions(cx);
                if !self
                    .send_terminal_input_without_suggestion_track(suffix.as_bytes().to_vec(), cx)
                {
                    self.terminal.editing.state_mut().invalidate();
                    return SuggestionSubmitResult::WriteFailed;
                }
            }
            SuggestionFill::Replace(command) => {
                let Some(backend) = self.shell_line_editing(&popup.session_id) else {
                    return self.reject_command_suggestion("Shell editor is not confirmed", cx);
                };
                if self.command_suggestion_fill_policy(&popup.session_id)
                    != CompletionFillPolicy::ReplaceAllowed
                {
                    return self.reject_command_suggestion(
                        "This terminal has no confirmed shell editing capability",
                        cx,
                    );
                }
                let Some(plan) = plan_edit(
                    &state.model.input,
                    state.model.version,
                    EditIntent::Replace {
                        range: 0..state.model.input.value.len(),
                        text: command,
                    },
                ) else {
                    return self
                        .reject_command_suggestion("Cannot plan this shell replacement", cx);
                };
                if !backend.allows_cursor_motion() && plan.move_steps != 0 {
                    return self.reject_command_suggestion(
                        "Console replacement requires the cursor at the end",
                        cx,
                    );
                }
                let insertion_wire = match self
                    .encode_session_outgoing(&popup.session_id, plan.insert_text.as_bytes())
                {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.dismiss_command_suggestions(cx);
                        self.terminal.editing.state_mut().invalidate();
                        self.shell.set_status(format!("input failed: {error}"));
                        cx.notify();
                        return SuggestionSubmitResult::WriteFailed;
                    }
                };
                let mode = self.terminal_key_mode_for_session(Some(&popup.session_id));
                let (wire, logical) =
                    shell_edit_payload_with_backend(&plan, backend, mode, insertion_wire);
                self.dismiss_command_suggestions(cx);
                self.terminal.view.frame_pipeline.arm_output_event_wake();
                if let Err(error) =
                    self.write_session_wire_input_recorded_as(&popup.session_id, &wire, &logical)
                {
                    self.terminal.editing.state_mut().invalidate();
                    self.shell.set_status(format!("input failed: {error}"));
                    cx.notify();
                    return SuggestionSubmitResult::WriteFailed;
                }
                // The framed writer records logical input but does not update
                // the editing model. Commit the prediction only after success.
                let state = self.terminal.editing.state_mut();
                state.model.accept_plan(plan, Instant::now());
                state.pending_evidence = Some(SnapshotEvidence::from_snapshot(&snapshot));
                state.mapping = None;
                state.selection = None;
                state.selection_origin_version = None;
                self.clear_terminal_selection(cx);
                self.arm_terminal_input_wake(cx);
                self.schedule_shell_edit_confirmation_timeout(cx);
            }
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
        self.dismiss_command_suggestions(cx);
        let runtime = self.commands.completion_runtime();
        cx.spawn(async move |this, cx| {
            let outcome = runtime.delete(command.clone()).await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(history) => {
                        this.commands.replace_command_history(history);
                        this.session.remove_command_from_all_history(&command);
                        this.shell.set_status("History command deleted");
                        this.refresh_command_suggestions(cx);
                    }
                    Err(error) => this
                        .shell
                        .set_status(format!("failed to delete history: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
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
        if !self.command_suggestions_allowed_for_session(&state.session_id)
            || (if state.refreshing {
                !self.command_suggestion_refresh_can_remain_visible()
            } else {
                !self.command_suggestion_input_is_confirmed()
            })
        {
            return div().into_any_element();
        }
        let Some(cursor) = self.suggestion_cursor_cell_for_session(&state.session_id, cx) else {
            return div().into_any_element();
        };
        let menu_w = 380.0_f32;
        let menu_h = command_suggestion_desired_height(state.items.len());
        let title = t!("suggestions.title");
        let match_label = t!(if state.items.len() == 1 {
            "suggestions.match"
        } else {
            "suggestions.matches"
        });
        let footer = format!(
            "{} {} · Enter {} · Esc {}",
            if state.selected_index.is_some() {
                "↑↓ / Tab / Shift+Tab"
            } else {
                "↑↓"
            },
            t!("suggestions.select"),
            t!("suggestions.acceptSelected"),
            t!("suggestions.dismiss")
        );
        let Some(placement) = self.suggestion_overlay_position_for_session_with_policy(
            Some(&state.session_id),
            SuggestionOverlayTarget {
                cursor,
                menu_size: (menu_w, menu_h),
            },
            SuggestionOverlayPolicy::BelowFirst {
                minimum_height: command_suggestion_desired_height(state.items.len().min(2)),
            },
        ) else {
            return div().into_any_element();
        };
        let selected_preview = state
            .selected_index
            .and_then(|index| state.items.get(index))
            .filter(|item| item.command.chars().count() > 48)
            .and_then(|item| {
                suggestion_preview_position(placement, self.shell.viewport_size().1)
                    .map(|preview| (item.command.clone(), preview))
            });
        let scroll = &self.terminal.assist.command_suggestion_scroll;
        let list_height =
            placement.height - SUGGESTION_OVERLAY_CHROME_HEIGHT - COMMAND_SUGGESTION_BORDER_HEIGHT;
        if let Some(index) = state.selected_index
            && scroll.bounds().size.height != px(list_height)
        {
            // scroll_to_item uses the previous viewport during prepaint. Resolve
            // a resize with the new height; ordinary redraws preserve wheel scrolling.
            let mut offset = scroll.offset();
            let row_top = px(index as f32 * COMMAND_SUGGESTION_ROW_HEIGHT);
            let row_bottom = row_top + px(COMMAND_SUGGESTION_ROW_HEIGHT);
            if row_top + offset.y < px(0.) {
                offset.y = -row_top;
            } else if row_bottom + offset.y > px(list_height) {
                offset.y = px(list_height) - row_bottom;
            }
            scroll.set_offset(offset);
        }
        let mut list = div()
            .id(SharedString::from("command-suggestions-list"))
            .flex()
            .flex_col()
            .size_full()
            .track_scroll(scroll)
            .overflow_y_scroll();
        for (index, item) in state.items.iter().enumerate() {
            let input_version = state.input_version;
            let selected = state.selected_index == Some(index);
            let source_icon = match item.source.as_str() {
                "history" | "legacyHistory" | "tokenHistory" => "icons/history.svg",
                _ => "icons/commands.svg",
            };
            let label = if item.display.trim().is_empty() {
                item.command.clone()
            } else {
                item.display.clone()
            };
            let show_full_command = label.chars().count() > 48;
            let full_command = item.command.clone();
            let is_history = matches!(item.source.as_str(), "history" | "legacyHistory");
            let description = item.description.clone();
            let show_description = state.mode == CompletionMode::Token && !description.is_empty();
            let multiline = item.command.contains(['\r', '\n']);
            let delete_command = item.command.clone();
            let mut row = div()
                .id(SharedString::from(format!("command-suggestion-{index}")))
                .h(px(COMMAND_SUGGESTION_ROW_HEIGHT))
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
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this
                        .terminal
                        .assist
                        .command_suggestions
                        .as_ref()
                        .is_none_or(|popup| {
                            popup.refreshing || popup.input_version != input_version
                        })
                    {
                        return;
                    }
                    if let Some(state) = this.terminal.assist.command_suggestions.as_mut() {
                        state.selected_index = Some(index);
                    }
                    let _ = this.apply_selected_command_suggestion(true, cx);
                    this.open_pending_suggestion_paste(window, cx);
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
                        .when(show_description, |this| this.min_w(px(140.)))
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
            if multiline {
                row = row.child(
                    div()
                        .flex_none()
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("suggestions.openPasteReview")),
                );
            }
            if show_description {
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "command-suggestion-description-{index}"
                        )))
                        .min_w_0()
                        .max_w(px(180.))
                        .flex_initial()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_right()
                        .text_color(rgb(palette.text_dimmed))
                        .child(description.clone()),
                );
            }
            if show_full_command || !description.is_empty() || multiline {
                row = row.tooltip(move |window, cx| {
                    let label = if multiline {
                        t!("suggestions.openPasteReview").to_string()
                    } else if !description.is_empty() {
                        description.clone()
                    } else {
                        full_command.clone()
                    };
                    NyaTooltip::new(label).build(window, cx)
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
                            if this
                                .terminal
                                .assist
                                .command_suggestions
                                .as_ref()
                                .is_none_or(|popup| {
                                    popup.refreshing || popup.input_version != input_version
                                })
                            {
                                return;
                            }
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
            .debug_selector(|| "command-suggestions-overlay".into())
            .absolute()
            .occlude()
            .left(px(placement.x))
            .top(px(placement.y))
            .w(px(placement.width))
            .h(px(placement.height))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgba((palette.surface << 8) | 0xf2))
            .shadow_lg()
            .when_some(selected_preview, |this, (command, preview)| {
                this.child(
                    div()
                        .id("command-suggestion-preview")
                        .debug_selector(|| "command-suggestion-preview".into())
                        .absolute()
                        .map(|this| match placement.direction {
                            SuggestionOverlayDirection::Below => this
                                .top(px(placement.height + SUGGESTION_PREVIEW_GAP
                                    - COMMAND_SUGGESTION_BORDER_HEIGHT / 2.0)),
                            SuggestionOverlayDirection::Above => this
                                .bottom(px(placement.height + SUGGESTION_PREVIEW_GAP
                                    - COMMAND_SUGGESTION_BORDER_HEIGHT / 2.0)),
                        })
                        .left(px(-COMMAND_SUGGESTION_BORDER_HEIGHT / 2.0))
                        .w(px(placement.width))
                        .max_h(px(preview.height))
                        .flex()
                        .flex_col()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.surface_elevated))
                        .shadow_lg()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(11.))
                        .text_color(rgb(palette.text))
                        .whitespace_normal()
                        .child(
                            div()
                                .id("command-suggestion-preview-content")
                                .min_h_0()
                                .max_h(px(preview.height - COMMAND_SUGGESTION_BORDER_HEIGHT))
                                .p_2()
                                .overflow_y_scrollbar()
                                .child(command),
                        ),
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
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(list)
                    .vertical_scrollbar(scroll),
            )
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
        self.suggestion_overlay_position_for_session_with_policy(
            session_id,
            SuggestionOverlayTarget {
                cursor: (cursor_row, cursor_col),
                menu_size: (menu_w, menu_h),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
    }

    fn suggestion_overlay_position_for_session_with_policy(
        &self,
        session_id: Option<&str>,
        target: SuggestionOverlayTarget,
        policy: SuggestionOverlayPolicy,
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
            target,
            policy,
        )
    }
}

fn completion_suggestion_item(item: CompletionCandidate, input: &str) -> CommandSuggestionItem {
    let source = if item.kind == CandidateKind::QuickCommand {
        "quickCommand"
    } else if item.kind == CandidateKind::History {
        if item.sources.contains(&CandidateSource::History) {
            "history"
        } else {
            "legacyHistory"
        }
    } else if item.sources.contains(&CandidateSource::Catalog) {
        "static"
    } else if item.sources.contains(&CandidateSource::History) {
        "tokenHistory"
    } else {
        "quickToken"
    };
    CommandSuggestionItem {
        command: item.target_line(input).unwrap_or_default(),
        display: item.display,
        source: source.into(),
        score: item.score,
        indices: item.indices,
        fillable: item.fillable,
        description: item.description,
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

fn command_suggestion_desired_height(item_count: usize) -> f32 {
    (item_count as f32 * COMMAND_SUGGESTION_ROW_HEIGHT
        + SUGGESTION_OVERLAY_CHROME_HEIGHT
        + COMMAND_SUGGESTION_BORDER_HEIGHT)
        .min(SUGGESTION_OVERLAY_MAX_HEIGHT)
}

pub(in crate::features) fn suggestion_overlay_position(
    geometry: SuggestionOverlayGeometry,
    target: SuggestionOverlayTarget,
    policy: SuggestionOverlayPolicy,
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
    let (height, direction) = match policy {
        SuggestionOverlayPolicy::BelowFirst { minimum_height } => {
            let minimum_height = minimum_height.min(menu_h);
            if space_below >= minimum_height {
                (menu_h.min(space_below), SuggestionOverlayDirection::Below)
            } else if space_above >= minimum_height {
                (menu_h.min(space_above), SuggestionOverlayDirection::Above)
            } else {
                return None;
            }
        }
        SuggestionOverlayPolicy::Adaptive => {
            if menu_h <= space_below {
                (menu_h, SuggestionOverlayDirection::Below)
            } else if menu_h <= space_above {
                (menu_h, SuggestionOverlayDirection::Above)
            } else if space_above > space_below {
                (space_above, SuggestionOverlayDirection::Above)
            } else {
                (space_below, SuggestionOverlayDirection::Below)
            }
        }
    };
    if height <= 0.0 {
        return None;
    }

    let width = match policy {
        SuggestionOverlayPolicy::Adaptive => menu_w,
        SuggestionOverlayPolicy::BelowFirst { .. } => {
            menu_w.min(viewport_w - 2.0 * SUGGESTION_OVERLAY_VIEWPORT_MARGIN)
        }
    };
    if width <= 0.0 {
        return None;
    }
    let max_x = (viewport_w - width - SUGGESTION_OVERLAY_VIEWPORT_MARGIN)
        .max(SUGGESTION_OVERLAY_VIEWPORT_MARGIN);
    Some(SuggestionOverlayPlacement {
        x: base_x.clamp(SUGGESTION_OVERLAY_VIEWPORT_MARGIN, max_x),
        y: match direction {
            SuggestionOverlayDirection::Below => below_y,
            SuggestionOverlayDirection::Above => above_bottom - height,
        },
        width,
        height,
        direction,
    })
}

#[derive(Clone, Copy)]
pub(in crate::features) enum SuggestionOverlayPolicy {
    Adaptive,
    BelowFirst { minimum_height: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) enum SuggestionOverlayDirection {
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct SuggestionOverlayPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub direction: SuggestionOverlayDirection,
}

struct SuggestionPreviewPlacement {
    height: f32,
}

fn suggestion_preview_position(
    placement: SuggestionOverlayPlacement,
    viewport_height: f32,
) -> Option<SuggestionPreviewPlacement> {
    // Stay on the far side of the popup so the preview never crosses the input row.
    let available = match placement.direction {
        SuggestionOverlayDirection::Below => {
            viewport_height
                - SUGGESTION_OVERLAY_VIEWPORT_MARGIN
                - placement.y
                - placement.height
                - SUGGESTION_PREVIEW_GAP
        }
        SuggestionOverlayDirection::Above => {
            placement.y - SUGGESTION_PREVIEW_GAP - SUGGESTION_OVERLAY_VIEWPORT_MARGIN
        }
    };
    (available >= SUGGESTION_PREVIEW_MIN_HEIGHT).then_some(SuggestionPreviewPlacement {
        height: available.min(SUGGESTION_PREVIEW_MAX_HEIGHT),
    })
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
        SuggestionOverlayDirection, SuggestionOverlayGeometry, SuggestionOverlayPlacement,
        SuggestionOverlayPolicy, SuggestionOverlayTarget, command_suggestion_desired_height,
        suggestion_overlay_desired_height, suggestion_overlay_position,
        suggestion_preview_position,
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
            SuggestionOverlayPolicy::Adaptive,
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
            SuggestionOverlayPolicy::Adaptive,
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
            SuggestionOverlayPolicy::Adaptive,
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
            SuggestionOverlayPolicy::Adaptive,
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
            SuggestionOverlayPolicy::Adaptive,
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

    fn command_placement(
        viewport_size: (f32, f32),
        cursor: (usize, usize),
        item_count: usize,
    ) -> Option<SuggestionOverlayPlacement> {
        suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(300.0))),
                cell_size: (8.0, 16.0),
                content_origin: (8.0, 0.0),
                gutter: 12.0,
                viewport_size,
            },
            SuggestionOverlayTarget {
                cursor,
                menu_size: (380.0, command_suggestion_desired_height(item_count)),
            },
            SuggestionOverlayPolicy::BelowFirst {
                minimum_height: command_suggestion_desired_height(item_count.min(2)),
            },
        )
    }

    #[test]
    fn command_overlay_anchors_at_cursor_with_content_insets_and_gutter() {
        let placement = command_placement((1024.0, 768.0), (2, 4), 3).unwrap();
        assert_eq!(placement.x, 72.0);
        assert_eq!(placement.y, 152.0);
        assert_eq!(placement.width, 380.0);
        assert_eq!(placement.height, 134.0);
        assert_eq!(placement.direction, SuggestionOverlayDirection::Below);
    }

    #[test]
    fn command_overlay_shrinks_below_even_when_full_menu_fits_above() {
        // Screenshot layout: bottom commands panel is available to the overlay.
        let placement = command_placement((1024.0, 650.0), (17, 4), 12).unwrap();
        assert_eq!(placement.y, 392.0);
        assert_eq!(placement.height, 250.0);
        assert_eq!(placement.direction, SuggestionOverlayDirection::Below);
    }

    #[test]
    fn command_overlay_flips_only_below_minimum_height() {
        let at_minimum = command_placement((1024.0, 650.0), (26, 4), 12).unwrap();
        assert_eq!(at_minimum.height, 106.0);
        assert_eq!(at_minimum.direction, SuggestionOverlayDirection::Below);

        let below_minimum = command_placement((1024.0, 649.0), (26, 4), 12).unwrap();
        assert_eq!(below_minimum.direction, SuggestionOverlayDirection::Above);
        assert_eq!(below_minimum.y + below_minimum.height, 512.0);
    }

    #[test]
    fn command_overlay_accepts_one_candidate_below_at_78_pixels() {
        let placement = command_placement((1024.0, 622.0), (26, 4), 1).unwrap();
        assert_eq!(placement.height, 78.0);
        assert_eq!(placement.direction, SuggestionOverlayDirection::Below);
        let placement = command_placement((1024.0, 621.0), (26, 4), 1).unwrap();
        assert_eq!(placement.direction, SuggestionOverlayDirection::Above);
    }

    #[test]
    fn command_overlay_shrinks_above_and_hides_when_neither_side_is_usable() {
        let placement = command_placement((1024.0, 330.0), (10, 4), 12).unwrap();
        assert_eq!(placement.direction, SuggestionOverlayDirection::Above);
        assert_eq!(placement.y, 100.0);
        assert_eq!(placement.height, 156.0);
        assert!(command_placement((1024.0, 300.0), (6, 4), 12).is_none());
    }

    #[test]
    fn command_overlay_clamps_right_edge_and_reduces_width_in_narrow_windows() {
        let placement = command_placement((800.0, 768.0), (2, 95), 3).unwrap();
        assert_eq!(placement.x, 412.0);
        assert_eq!(placement.x + placement.width, 792.0);
        let placement = command_placement((300.0, 768.0), (2, 4), 3).unwrap();
        assert_eq!(placement.x, 8.0);
        assert_eq!(placement.width, 284.0);
        assert!(command_placement((16.0, 768.0), (2, 4), 3).is_none());
    }

    #[test]
    fn command_overlay_height_includes_border_and_stays_capped() {
        assert_eq!(command_suggestion_desired_height(1), 78.0);
        assert_eq!(command_suggestion_desired_height(2), 106.0);
        assert_eq!(command_suggestion_desired_height(12), 272.0);
    }

    #[test]
    fn command_preview_uses_only_space_beyond_the_popup_and_caps_height() {
        let mut placement = command_placement((1024.0, 768.0), (2, 4), 3).unwrap();
        assert_eq!(
            suggestion_preview_position(placement, 768.0)
                .unwrap()
                .height,
            120.0
        );
        // 32px after the 6px gap is the minimum useful preview size.
        let bottom = placement.y + placement.height;
        assert_eq!(
            suggestion_preview_position(placement, bottom + 46.0)
                .unwrap()
                .height,
            32.0
        );
        assert!(suggestion_preview_position(placement, bottom + 45.0).is_none());
        placement.direction = SuggestionOverlayDirection::Above;
        placement.y = 46.0;
        assert_eq!(
            suggestion_preview_position(placement, 768.0)
                .unwrap()
                .height,
            32.0
        );
        placement.y = 45.0;
        assert!(suggestion_preview_position(placement, 768.0).is_none());
    }
}

fn terminal_line_prefix_for_cell_col(line: &str, cell_col: usize) -> String {
    let end = terminal_byte_index_for_cell_col(line, cell_col);
    line.get(..end).unwrap_or(line).to_string()
}

#[cfg(test)]
mod tests {
    use nyaterm_core::command_suggestion_suppression::CommandSuggestionSuppression;
    pub(super) fn history_search_fixture(
        command: &str,
        input: &str,
        revision: u64,
    ) -> nyaterm_core::completion::types::CompletionResult {
        use nyaterm_core::completion::types::{
            CandidateKind, CandidateSource, CompletionCandidate, CompletionResult,
        };
        CompletionResult {
            revision,
            candidates: vec![CompletionCandidate {
                display: command.into(),
                insert_text: command.into(),
                description: String::new(),
                range: 0..input.len(),
                kind: CandidateKind::History,
                sources: vec![CandidateSource::History],
                use_count: 1,
                last_used_at_ms: 1,
                score: 1,
                indices: vec![],
                fillable: nyaterm_core::completion::types::supports_full_command_fill(command),
                context_priority: 1,
            }],
        }
    }
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, KeyDownEvent, TestAppContext};
    use nyaterm_core::{TerminalInputState, apply_terminal_input_data};

    use crate::features::NyaTermApp;
    use crate::features::terminal::terminal_runtime::TERMINAL_INPUT_LATENCY_WINDOW;
    use crate::models::{CommandSuggestionItem, CommandSuggestionState};
    use crate::test_support::TestConfigDir;

    use super::{
        command_history_input_update, command_suggestion_clamp_selection,
        command_suggestion_draft_only_appended, command_suggestion_input_can_defer_refresh,
        command_suggestion_input_candidate_chars, command_suggestion_input_obvious_pager_prefix,
        command_suggestion_item_for_selection, command_suggestion_refresh_input_delay,
        command_suggestion_state_changed, command_suggestion_step_selection,
        terminal_line_prefix_for_cell_col,
    };

    #[test]
    fn whole_candidate_types_keep_their_presentation_without_legacy_descriptions() {
        use crate::features::terminal::command_suggestions::completion_suggestion_item;
        use nyaterm_core::completion::types::{
            CandidateKind, CandidateSource, CompletionCandidate,
        };

        for (kind, sources, expected) in [
            (
                CandidateKind::History,
                vec![CandidateSource::LegacyHistory],
                "legacyHistory",
            ),
            (
                CandidateKind::History,
                vec![CandidateSource::Catalog, CandidateSource::History],
                "history",
            ),
            (
                CandidateKind::QuickCommand,
                vec![CandidateSource::Catalog, CandidateSource::QuickCommand],
                "quickCommand",
            ),
        ] {
            let item = completion_suggestion_item(
                CompletionCandidate {
                    display: "重启代理".into(),
                    insert_text: "systemctl restart example.service".into(),
                    description: String::new(),
                    range: 0.."重启代理".len(),
                    kind,
                    sources,
                    use_count: 1,
                    last_used_at_ms: 1,
                    score: 1,
                    indices: vec![0, 1],
                    fillable: true,
                    context_priority: 0,
                },
                "重启代理",
            );
            assert_eq!(item.source, expected);
            assert_eq!(item.display, "重启代理");
            assert_eq!(item.command, "systemctl restart example.service");
            assert!(item.description.is_empty());
        }
    }

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
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_some()
            );
            assert!(app.can_use_smart_cursor_selection());
            app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
            assert!(app.is_credential_prompt_input_mode());
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
            assert!(!app.can_use_smart_cursor_selection());

            // The toggle must release these gates without waiting for output or a timeout.
            app.toggle_credential_autofill(cx);
            assert!(!app.is_credential_prompt_input_mode());
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_some()
            );
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
            assert!(
                app.prepare_command_suggestion_search(
                    app.terminal.assist.command_suggestion_search_gen,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx,
                )
                .is_none()
            );
            app.terminal
                .seed_session_view("s1".into(), "$ echo".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            let request = app
                .prepare_command_suggestion_search(
                    app.terminal.assist.command_suggestion_search_gen,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx,
                )
                .expect("confirmed echoed input can request suggestions");
            app.terminal.editing.state_mut().model.invalidate();
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
            app.publish_command_suggestion_search(
                request,
                history_search_fixture(
                    "echo example",
                    "echo ex",
                    app.commands.completion_runtime().revision(),
                ),
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
    fn startup_layout_resize_keeps_first_line_input_eligible_for_suggestions() {
        let dir = TestConfigDir::new("nyaterm-suggestions-first-line-resize");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            // Selecting the new session activates editing state, then the first
            // paint fits the pane to its real bounds before the user can type.
            app.terminal.activate_shell_editing_session("s1");
            app.terminal.invalidate_shell_editing_session("s1");
            app.note_shell_editing_input(b"git", cx);
            assert!(!app.terminal.editing.predicted_input().desynced);
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
            app.terminal
                .seed_session_view("s1".into(), "$ git".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.command_suggestion_input_is_confirmed());
            assert!(
                app.prepare_command_suggestion_search(
                    2,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_some()
            );
        });
    }

    #[test]
    fn layout_resize_with_tracked_text_still_desyncs_the_line() {
        let dir = TestConfigDir::new("nyaterm-suggestions-resize-tracked");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            app.note_shell_editing_input(b"git", cx);
            app.terminal.invalidate_shell_editing_session("s1");
            app.note_shell_editing_input(b"x", cx);
            assert!(app.terminal.editing.predicted_input().desynced);
            assert!(
                app.prepare_command_suggestion_search(
                    2,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
        });
    }

    #[test]
    fn pager_space_without_command_text_cannot_start_suggestions() {
        let dir = TestConfigDir::new("nyaterm-suggestions-pager-space");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            // A remote pager consumes Space itself and echoes nothing, so the
            // local tracker holds only a separator while the screen repaints.
            app.note_shell_editing_input(b" ", cx);
            assert_eq!(app.terminal.editing.assist_input().value, " ");
            assert!(app.terminal.assist.command_suggestions.is_none());
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );

            // The same signal hides a popup left over from an earlier line.
            app.terminal.assist.command_suggestions = Some(open_suggestions_popup("s1"));
            app.note_command_suggestion_input(b" ", cx);
            assert!(app.terminal.assist.command_suggestions.is_none());

            // A separator after real command text still offers suggestions.
            app.note_shell_editing_input(b"git ", cx);
            assert!(!app.command_suggestion_input_is_confirmed());
            app.terminal
                .seed_session_view("s1".into(), "$   git ".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(
                app.prepare_command_suggestion_search(
                    2,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_some()
            );
        });
    }

    /// Deterministic replay of a real `su` round trip. Every step feeds the
    /// screen text the remote would produce and drives the same gates the live
    /// input path drives, so a rejection is localized to the step that causes it.
    #[test]
    fn su_round_trip_keeps_suggestions_available_across_prompts() {
        use nyaterm_core::completion::types::CompletionMode;

        let dir = TestConfigDir::new("nyaterm-suggestions-su-round-trip");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            macro_rules! output {
                ($screen:expr) => {{
                    app.terminal
                        .seed_session_view("s1".into(), $screen.to_string(), "UTF-8");
                    if let Some(view) = app.terminal.view.views.get_mut("s1") {
                        let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
                        view.frame_snapshot = Some(snapshot);
                    }
                    app.drain_pending_credential_autofill_detection(cx);
                    app.reconcile_shell_editing("s1", cx);
                }};
            }
            let mut screen = String::from("user@host:~$ ");
            output!(screen);

            // 1. Login shell: type and submit `su - root`.
            app.note_shell_editing_input(b"su - root", cx);
            app.note_command_suggestion_input(b"su - root", cx);
            assert!(!app.command_suggestion_input_is_confirmed());
            screen.push_str("su - root");
            output!(screen);
            assert!(
                app.prepare_command_suggestion_search(1, CompletionMode::Token, cx)
                    .is_some(),
                "suggestions must work before su"
            );
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);
            // bash preexec emits OSC 133 C for the foreground `su`.
            app.apply_shell_integration_edges("s1", true, false, true);

            // 2. su writes a no-echo password prompt; the password is never echoed.
            screen.push_str("\r\nPassword: ");
            output!(screen);
            app.note_shell_editing_input(b"password", cx);
            app.note_command_suggestion_input(b"password", cx);
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);

            // 3. Authentication succeeds; the child root shell owns the prompt.
            screen.push_str("\r\nroot@host:~# ");
            output!(screen);
            app.note_shell_editing_input(b"ls", cx);
            app.note_command_suggestion_input(b"ls", cx);
            screen.push_str("ls");
            output!(screen);
            assert!(
                app.prepare_command_suggestion_search(2, CompletionMode::Token, cx)
                    .is_some(),
                "suggestions must work inside the child shell"
            );

            // 4. `ls` runs and the child prompt returns without any OSC 133 marks.
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);
            screen.push_str("\r\nfile1 file2\r\nroot@host:~# ");
            output!(screen);
            app.note_shell_editing_input(b"ls", cx);
            app.note_command_suggestion_input(b"ls", cx);
            screen.push_str("ls");
            output!(screen);
            assert!(
                app.prepare_command_suggestion_search(3, CompletionMode::Token, cx)
                    .is_some(),
                "a second command inside the child shell must still suggest"
            );

            // 5. `exit` returns to the outer shell, which emits OSC 133 D.
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);
            app.note_shell_editing_input(b"exit", cx);
            app.note_command_suggestion_input(b"exit", cx);
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);
            screen.push_str("\r\nlogout\r\nuser@host:~$ ");
            app.terminal
                .seed_session_view("s1".into(), screen.clone(), "UTF-8");
            app.apply_shell_integration_edges("s1", false, true, false);
            app.drain_pending_credential_autofill_detection(cx);
            app.reconcile_shell_editing("s1", cx);
            app.note_shell_editing_input(b"ls", cx);
            app.note_command_suggestion_input(b"ls", cx);
            screen.push_str("ls");
            output!(screen);
            assert!(
                app.prepare_command_suggestion_search(4, CompletionMode::Token, cx)
                    .is_some(),
                "suggestions must work after exiting the child shell"
            );
        });
    }

    /// Reproduces the reported `su` failure without an SSH server: a saved
    /// credential makes the credential tick arm the no-echo input window at
    /// `su`'s password prompt. The window must end when the child prompt is
    /// visible, otherwise every later keystroke is discarded as password input.
    #[test]
    fn su_password_prompt_input_window_ends_at_the_child_prompt() {
        use nyaterm_core::completion::types::CompletionMode;

        let dir = TestConfigDir::new("nyaterm-suggestions-su-credential-window");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");

            // The credential tick ran at `su`'s no-echo password prompt and
            // armed the no-echo input window, remembering that prompt instance.
            app.terminal.seed_session_view(
                "s1".into(),
                "user@host:~$ su - root\r\nPassword: ".into(),
                "UTF-8",
            );
            if let Some(view) = app.terminal.view.views.get_mut("s1") {
                let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
                view.frame_snapshot = Some(snapshot);
            }
            app.sync_credential_autofill_for_test(cx);
            assert!(app.is_credential_prompt_input_mode());

            // Authentication succeeded; the prompt line changed and is no longer
            // a credential prompt. `observe_credential_prompt_instance` clears
            // the buffer during this tick.
            app.terminal.seed_session_view(
                "s1".into(),
                "user@host:~$ su - root\r\nPassword: \r\nroot@host:~# ".into(),
                "UTF-8",
            );
            if let Some(view) = app.terminal.view.views.get_mut("s1") {
                let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
                view.frame_snapshot = Some(snapshot);
            }
            app.sync_credential_autofill_for_test(cx);
            assert!(
                !app.is_credential_prompt_input_mode(),
                "the no-echo input window must end once the visible prompt is not a credential prompt"
            );

            // Typing at the child prompt must reach the suggestion gates.
            app.note_shell_editing_input(b"ls", cx);
            app.note_command_suggestion_input(b"ls", cx);
            app.terminal
                .seed_session_view("s1".into(), "root@host:~# ls".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(
                app.prepare_command_suggestion_search(1, CompletionMode::Token, cx)
                    .is_some(),
                "suggestions must work in the child shell"
            );

            // `exit` returns to the outer shell. The window must stay closed:
            // this is the reported "still no suggestions after leaving su"
            // symptom, which a per-prompt re-arm would bring back.
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);
            app.note_shell_editing_input(b"exit", cx);
            app.note_command_suggestion_input(b"exit", cx);
            app.note_shell_editing_input(b"\r", cx);
            app.note_command_suggestion_input(b"\r", cx);
            app.terminal
                .seed_session_view("s1".into(), "logout\r\nuser@host:~$ ".into(), "UTF-8");
            if let Some(view) = app.terminal.view.views.get_mut("s1") {
                let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
                view.frame_snapshot = Some(snapshot);
            }
            app.sync_credential_autofill_for_test(cx);
            app.reconcile_shell_editing("s1", cx);
            assert!(
                !app.is_credential_prompt_input_mode(),
                "the window must not reopen at the outer prompt after exit"
            );
            app.note_shell_editing_input(b"ls", cx);
            app.note_command_suggestion_input(b"ls", cx);
            app.terminal
                .seed_session_view("s1".into(), "logout\r\nuser@host:~$ ls".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(
                app.prepare_command_suggestion_search(2, CompletionMode::Token, cx)
                    .is_some(),
                "suggestions must work after exiting the child shell"
            );
        });
    }

    #[test]
    fn confirmed_prompt_echo_in_a_child_shell_releases_suppression() {
        let dir = TestConfigDir::new("nyaterm-suggestions-child-shell-resync");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            // `su - root` started. The outer shell sent OSC 133 C, while its D
            // can only arrive after the child exits, so the latch outlives it.
            app.terminal.assist.command_suggestion_suppression =
                CommandSuggestionSuppression::CommandRunning;
            app.note_shell_editing_input(b"git", cx);
            app.note_command_suggestion_input(b"git", cx);
            assert!(app.terminal.assist.command_suggestions.is_none());

            // The root prompt echoes the locally predicted line back.
            app.terminal
                .seed_session_view("s1".into(), "root@host:~# git".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(
                !app.terminal
                    .assist
                    .command_suggestion_suppression
                    .is_suppressed()
            );
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_some()
            );
        });
    }

    #[test]
    fn echo_without_a_shell_prompt_prefix_keeps_suppression() {
        let dir = TestConfigDir::new("nyaterm-suggestions-pager-prompt-resync");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            app.terminal.assist.command_suggestion_suppression =
                CommandSuggestionSuppression::CommandRunning;
            app.note_shell_editing_input(b"git", cx);

            // A pager status line and a child REPL echo the text, but neither
            // prefix is a shell prompt, so the suppression must survive.
            for seeded in ["/git", ">>> git"] {
                app.terminal
                    .seed_session_view("s1".into(), seeded.into(), "UTF-8");
                app.reconcile_shell_editing("s1", cx);
                assert!(
                    app.terminal
                        .assist
                        .command_suggestion_suppression
                        .is_suppressed(),
                    "{seeded}"
                );
            }
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
        });
    }

    #[test]
    fn shell_like_pager_echo_cannot_release_explicit_interactive_suppression() {
        let dir = TestConfigDir::new("nyaterm-explicit-pager-suppression");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            app.terminal.assist.pending_command_history_entry = Some("su - root".into());
            app.note_shell_editing_input(b"less", cx);
            app.note_shell_editing_input(b"\r", cx);
            app.apply_shell_integration_edges("s1", true, false, true);
            app.note_shell_editing_input(b"git", cx);
            app.terminal
                .seed_session_view("s1".into(), "root@host:~# git".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.command_suggestion_input_is_confirmed());
            assert_eq!(
                app.terminal.assist.command_suggestion_suppression,
                CommandSuggestionSuppression::InteractiveProgram
            );
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
        });
    }

    #[test]
    fn published_candidates_require_explicit_selection_even_for_a_single_match() {
        let dir = TestConfigDir::new("nyaterm-suggestions-explicit-selection");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_local_session(
            &mut cx,
            dir.path(),
            "s1",
        );
        cx.update_entity(&app, |app, cx| {
            show_surface_for_session(app, "s1");
            app.note_shell_editing_input(b"docker r", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ docker r".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            for mode in [
                nyaterm_core::completion::types::CompletionMode::Token,
                nyaterm_core::completion::types::CompletionMode::History,
            ] {
                for count in [1, 3] {
                    let request_id = app.terminal.assist.command_suggestion_search_gen;
                    let mut result = history_search_fixture(
                        "docker run",
                        "docker r",
                        app.commands.completion_runtime().revision(),
                    );
                    result
                        .candidates
                        .resize(count, result.candidates[0].clone());
                    for _ in 0..2 {
                        let request = app
                            .prepare_command_suggestion_search(request_id, mode, cx)
                            .unwrap();
                        app.publish_command_suggestion_search(
                            request,
                            nyaterm_core::completion::types::CompletionResult {
                                candidates: result.candidates.clone(),
                                revision: result.revision,
                            },
                            Duration::ZERO,
                            cx,
                        );
                        let popup = app.terminal.assist.command_suggestions.as_ref().unwrap();
                        assert_eq!(popup.items.len(), count);
                        assert_eq!(popup.selected_index, None);
                        let mut down = escape_key_event();
                        down.keystroke.key = "down".into();
                        assert!(app.handle_command_suggestion_key(&down, cx));
                        assert_eq!(
                            app.terminal
                                .assist
                                .command_suggestions
                                .as_ref()
                                .unwrap()
                                .selected_index,
                            Some(0),
                        );
                    }
                }
            }
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
    fn only_forward_typing_keeps_the_popup_alive() {
        assert!(command_suggestion_draft_only_appended("git c", "git ch"));
        assert!(!command_suggestion_draft_only_appended("git ch", "git c"));
        assert!(!command_suggestion_draft_only_appended("git c", "git c"));
        assert!(!command_suggestion_draft_only_appended("", "g"));
        assert!(!command_suggestion_draft_only_appended("git c", "ls"));
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
            refreshing: false,
            mode: nyaterm_core::completion::types::CompletionMode::History,
            completion_revision: 0,
            scope: None,
            session_id: "session".to_string(),
            input_version: 0,
            sync_revision: 0,
            draft: "git".to_string(),
            items: vec![CommandSuggestionItem {
                fillable: true,
                description: String::new(),
                command: "git status".to_string(),
                display: "git status".to_string(),
                source: "history".to_string(),
                score: 42,
                indices: vec![0, 1, 2],
            }],
            selected_index: None,
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
            fillable: true,
            description: String::new(),
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
            refreshing: false,
            mode: nyaterm_core::completion::types::CompletionMode::History,
            completion_revision: 0,
            scope: None,
            session_id: session_id.to_string(),
            input_version: 0,
            sync_revision: 0,
            draft: "1".to_string(),
            items: vec![CommandSuggestionItem {
                fillable: true,
                description: String::new(),
                command: "1111".to_string(),
                display: "1111".to_string(),
                source: "history".to_string(),
                score: 1,
                indices: Vec::new(),
            }],
            selected_index: Some(0),
        }
    }

    #[test]
    fn incremental_selection_follows_the_command_and_never_a_reused_row_index() {
        let previous = open_suggestions_popup("s1");
        let mut next = open_suggestions_popup("s1");
        let mut added = next.items[0].clone();
        added.command = "new-path".into();
        next.items.insert(0, added);
        next.selected_index = None;
        super::preserve_suggestion_selection(Some(&previous), &mut next);
        assert_eq!(next.selected_index, Some(1));
        next.items.pop();
        super::preserve_suggestion_selection(Some(&previous), &mut next);
        assert_eq!(next.selected_index, None);
        next.input_version += 1;
        next.items = previous.items.clone();
        super::preserve_suggestion_selection(Some(&previous), &mut next);
        assert_eq!(next.selected_index, None);
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
                        app.suggestion_cursor_cell_for_session("s1", cx).unwrap().0,
                        app.suggestion_cursor_cell_for_session("s1", cx).unwrap().1,
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
            assert!(
                app.prepare_command_suggestion_search(
                    1,
                    nyaterm_core::completion::types::CompletionMode::Token,
                    cx
                )
                .is_none()
            );
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
            app.note_shell_editing_input(b"vim 1", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ vim 1".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            app.prepare_command_suggestion_search(
                app.terminal.assist.command_suggestion_search_gen,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx,
            )
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
            let results = history_search_fixture(
                "vim 1111",
                "vim 1",
                app.commands.completion_runtime().revision(),
            );
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
