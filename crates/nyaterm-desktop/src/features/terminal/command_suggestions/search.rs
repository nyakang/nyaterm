//! Debounced searches and ordered, revalidated publication into GPUI state.
use super::diagnostics::CommandSuggestionRefreshTiming;
use super::input::{
    command_suggestion_input_can_defer_refresh, command_suggestion_input_has_command_text,
};
use super::path_source::PathSource;
use super::state::CommandSuggestionState;
use super::view::completion_suggestion_item;
use crate::features::NyaTermApp;
use crate::features::commands::completion_runtime::{
    CompletionQuery, CompletionRuntime, CompletionSearchOutcome,
};
use crate::features::terminal::terminal_runtime::TERMINAL_INPUT_LATENCY_WINDOW;
use gpui::Context;
use nyaterm_completion_runtime::progressive::publish_progressively;
use nyaterm_core::QuickCommand;
use nyaterm_core::completion::merge::merge_path_candidates;
use nyaterm_core::completion::types::{
    CompletionFillPolicy, CompletionMode, CompletionResult, ShellCommandContext,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
const COMMAND_SUGGESTION_REFRESH_DEBOUNCE: Duration = Duration::from_millis(80);
const COMMAND_SUGGESTION_REFRESH_PRESSURE_RETRY: Duration = Duration::from_millis(120);
#[derive(Clone)]
pub(super) struct CommandSuggestionSearchRequest {
    request_id: u64,
    session_id: String,
    input_version: u64,
    sync_revision: u64,
    pattern: String,
    pattern_chars: usize,
    min_chars: usize,
    max_chars: usize,
    mode: CompletionMode,
    pub(super) context: ShellCommandContext,
    completion: CompletionRuntime,
    quick_commands: Arc<[QuickCommand]>,
    started_at: Instant,
    popup_visible_at_start: bool,
    timing: CommandSuggestionRefreshTiming,
    pub(super) fill_policy: CompletionFillPolicy,
    path_source: Option<PathSource>,
}

pub(super) fn command_suggestion_refresh_input_delay(
    last_terminal_input_at: Option<Instant>,
    now: Instant,
) -> Option<Duration> {
    last_terminal_input_at.and_then(|last| {
        let elapsed = now.saturating_duration_since(last);
        (elapsed < TERMINAL_INPUT_LATENCY_WINDOW).then(|| TERMINAL_INPUT_LATENCY_WINDOW - elapsed)
    })
}

impl NyaTermApp {
    pub(in crate::features) fn schedule_command_suggestion_refresh(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        // Tauri useCommandHistory: 80ms debounce before fuzzy search.
        self.schedule_command_suggestion_refresh_after(COMMAND_SUGGESTION_REFRESH_DEBOUNCE, cx);
    }

    pub(super) fn schedule_command_suggestion_refresh_after(
        &mut self,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let mode = self.terminal.assist.commands.mode;
        self.schedule_command_suggestion_search_mode(delay, mode, cx);
    }

    pub(super) fn schedule_command_suggestion_search_mode(
        &mut self,
        delay: Duration,
        mode: CompletionMode,
        cx: &mut Context<Self>,
    ) {
        let request_id = self.terminal.assist.commands.invalidate_search();
        self.terminal.assist.commands.refresh_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let request = this.update(cx, |this, cx| {
                if this.terminal.assist.commands.search_generation != request_id {
                    return None;
                }
                let now = Instant::now();
                if let Some(delay) =
                    command_suggestion_refresh_input_delay(this.shell.last_terminal_input_at(), now)
                {
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
                let policy = request.fill_policy;
                cx.background_executor()
                    .spawn(async move { source.candidates_with_policy(&input, Some(policy)).await })
            });
            let completion = request.completion.clone();
            let ordinary = completion.search_outcome(CompletionQuery {
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
            let result = match ordinary.await {
                CompletionSearchOutcome::Ready(result) => result,
                CompletionSearchOutcome::Busy => {
                    let _ = this.update(cx, |this, cx| {
                        if this.terminal.assist.commands.search_generation == request.request_id {
                            this.schedule_command_suggestion_search_mode(
                                COMMAND_SUGGESTION_REFRESH_PRESSURE_RETRY,
                                request.mode,
                                cx,
                            );
                        }
                    });
                    return;
                }
                CompletionSearchOutcome::Superseded => return,
                CompletionSearchOutcome::Unavailable => {
                    let _ = this.update(cx, |this, cx| {
                        if this.terminal.assist.commands.search_generation == request.request_id {
                            this.hide_command_suggestions_if_present(cx);
                            this.settings
                                .update_store_status("completion worker is unavailable", false);
                        }
                    });
                    return;
                }
            };
            publish_progressively(std::future::ready(result), paths, |result, paths| {
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

    pub(in crate::features) fn refresh_command_suggestions(&mut self, cx: &mut Context<Self>) {
        self.schedule_command_suggestion_refresh_after(Duration::ZERO, cx);
    }

    pub(in crate::features) fn show_manual_command_suggestions(&mut self, cx: &mut Context<Self>) {
        if self.terminal.assist.credential_suggestions.is_some()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.commands.suppression.is_suppressed()
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
        self.terminal.assist.commands.mode = CompletionMode::History;
        self.schedule_command_suggestion_search_mode(
            COMMAND_SUGGESTION_REFRESH_DEBOUNCE,
            CompletionMode::History,
            cx,
        );
    }

    pub(super) fn prepare_command_suggestion_search(
        &mut self,
        request_id: u64,
        mode: CompletionMode,
        cx: &mut Context<Self>,
    ) -> Option<CommandSuggestionSearchRequest> {
        let started_at = Instant::now();
        let popup_visible_at_start = self.terminal.assist.commands.popup.is_some();
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

    pub(super) fn publish_command_suggestion_search(
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
        }) || self.command_suggestion_context(&request.session_id) != request.context
            || self.terminal.assist.commands.search_generation != request.request_id
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
        if self.commands.completion_runtime().revision() != result.revision {
            self.schedule_command_suggestion_search_mode(
                COMMAND_SUGGESTION_REFRESH_PRESSURE_RETRY,
                request.mode,
                cx,
            );
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
        let next_state = CommandSuggestionState {
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
        if !self
            .terminal
            .assist
            .commands
            .replace_results(next_state, incremental)
        {
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
}

#[cfg(test)]
mod tests {
    use super::command_suggestion_refresh_input_delay;
    use crate::features::terminal::terminal_runtime::TERMINAL_INPUT_LATENCY_WINDOW;
    use std::time::{Duration, Instant};
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
}
