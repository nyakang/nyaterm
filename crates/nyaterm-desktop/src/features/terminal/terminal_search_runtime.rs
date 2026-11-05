use crate::features::terminal::terminal_surface::{
    TerminalOverviewMarker, TerminalOverviewMarkerKind,
};
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::{
    RecordingHistorySearchKey, RecordingWriteEvent, TerminalFrameSearchKey,
    TerminalFrameSearchPurpose, TerminalFrameSearchResult, TerminalSearchMode, TerminalSelection,
    terminal_frame_search_result_is_current,
};
use crate::terminal::TerminalBufferMatch;
use futures::StreamExt as _;
use gpui::{Context, KeyDownEvent, Window};
use rust_i18n::t;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub(in crate::features) struct TerminalSelectedOccurrenceMatches {
    matches: Arc<[TerminalBufferMatch]>,
    selection: Option<TerminalSelection>,
    position_fingerprint: u64,
    revision: u64,
}

impl TerminalSelectedOccurrenceMatches {
    fn iter(&self) -> impl Iterator<Item = &TerminalBufferMatch> {
        self.matches
            .iter()
            .filter(|search_match| !selected_occurrence_is_original(search_match, self.selection))
    }

    pub(in crate::features) fn iter_in_absolute_range(
        &self,
        range: Range<usize>,
    ) -> impl Iterator<Item = &TerminalBufferMatch> {
        let start = self
            .matches
            .partition_point(|search_match| search_match.line_index < range.start);
        let end = self
            .matches
            .partition_point(|search_match| search_match.line_index < range.end);
        self.matches[start..end]
            .iter()
            .filter(|search_match| !selected_occurrence_is_original(search_match, self.selection))
    }

    fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

pub(in crate::features) fn terminal_matches_in_absolute_range(
    matches: &[TerminalBufferMatch],
    range: Range<usize>,
) -> &[TerminalBufferMatch] {
    let start = matches.partition_point(|search_match| search_match.line_index < range.start);
    let end = matches.partition_point(|search_match| search_match.line_index < range.end);
    &matches[start..end]
}

pub(in crate::features) fn terminal_search_index_for_position(
    matches: &[TerminalBufferMatch],
    occurrences: &[Range<usize>],
    position: (usize, usize),
) -> usize {
    occurrences
        .partition_point(|range| {
            matches
                .get(range.start)
                .is_some_and(|m| (m.line_index, m.start_col) < position)
        })
        .min(occurrences.len().saturating_sub(1))
}

impl NyaTermApp {
    pub(in crate::features) fn open_terminal_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.menus.actions_open = false;
        self.terminal.search.open = true;
        self.terminal.search.active_index = 0;
        self.shell.set_status("terminal search opened".to_string());
        self.forget_text_inputs("terminal.search.");
        let query = self.terminal.search.query.clone();
        let field = self.text_input(
            "terminal.search.query",
            &query,
            TextInputSetup::placeholder("Find"),
            cx,
        );
        self.refresh_terminal_search_state(cx);
        window.focus(&field.read(cx).focus_handle(), cx);
    }

    pub(in crate::features) fn close_terminal_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session_id) = self.session.active_id_owned() {
            self.terminal
                .view
                .frame_pipeline
                .cancel_find_search(session_id.clone());
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                view.pending_search_key = None;
                view.pending_search_at = None;
                view.pending_search_visible = None;
            }
        }
        self.terminal.search.open = false;
        self.terminal.search.active_index = 0;
        self.forget_text_inputs("terminal.search.");
        self.shell.set_status("terminal search closed".to_string());
        window.focus(&self.terminal.input.focus, cx);
        self.notify_active_terminal_surface(cx);
        cx.notify();
    }

    pub(in crate::features) fn refresh_terminal_search_state(&mut self, cx: &mut Context<Self>) {
        self.terminal.search.full_search_ready_at = Instant::now() + Duration::from_millis(100);
        if (self.terminal.search.mode != TerminalSearchMode::Buffer
            || self.terminal_search_key().is_none())
            && let Some(session_id) = self.session.active_id_owned()
        {
            self.terminal
                .view
                .frame_pipeline
                .cancel_find_search(session_id);
        }
        self.request_active_terminal_search();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            let _ = this.update(cx, |this, cx| {
                let _ = this.request_active_terminal_buffer_search();
                cx.notify();
            });
        })
        .detach();
        self.notify_active_terminal_surface(cx);
        cx.notify();
    }

    pub(in crate::features) fn apply_terminal_search_query(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        self.terminal.search.query = text;
        self.terminal.search.active_index = 0;
        self.refresh_terminal_search_state(cx);
    }

    pub(in crate::features) fn terminal_search_key(&self) -> Option<TerminalFrameSearchKey> {
        let query = self.terminal.search.query.trim();
        if query.is_empty() {
            return None;
        }
        Some(TerminalFrameSearchKey {
            query: query.to_string(),
            case_sensitive: self.terminal.search.case_sensitive,
            regex: self.terminal.search.regex,
            whole_word: self.terminal.search.whole_word,
            limit: 1000,
            request_generation: 0,
        })
    }

    pub(in crate::features) fn request_active_terminal_buffer_search(&mut self) -> bool {
        if !self.terminal.search.open || self.terminal.search.mode != TerminalSearchMode::Buffer {
            return false;
        }
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        let Some(key) = self.terminal_search_key() else {
            return false;
        };
        let Some(view) = self.terminal.view.views.get(&session_id) else {
            return false;
        };
        if view.search_result.as_ref().is_some_and(|result| {
            terminal_frame_search_result_is_current(result, &key, view.screen_revision)
        }) {
            return false;
        }
        let total = view.total_rows_for_ui();
        let end = total.saturating_sub(view.scroll_offset);
        let start = end.saturating_sub(view.viewport_rows_for_ui());
        let mut requested = self.request_terminal_frame_search(
            &session_id,
            TerminalFrameSearchPurpose::FindVisible {
                absolute_start: start,
                absolute_end: end,
            },
            key.clone(),
        );
        let ready = Instant::now() >= self.terminal.search.full_search_ready_at
            && self
                .terminal
                .view
                .views
                .get(&session_id)
                .is_some_and(|view| {
                    view.last_screen_change_at.elapsed() >= Duration::from_millis(100)
                });
        if ready {
            requested |= self.request_terminal_frame_search(
                &session_id,
                TerminalFrameSearchPurpose::Find,
                key,
            );
        }
        requested
    }

    pub(in crate::features) fn request_active_terminal_search(&mut self) {
        let _ = self.request_active_terminal_buffer_search();
        self.request_active_terminal_history_search();
    }

    fn terminal_buffer_search_result(&self) -> Option<&TerminalFrameSearchResult> {
        let key = self.terminal_search_key()?;
        let view = self
            .session
            .active_id()
            .and_then(|session_id| self.terminal.view.views.get(session_id))?;
        view.search_result
            .as_ref()
            .filter(|result| {
                terminal_frame_search_result_is_current(result, &key, view.screen_revision)
            })
            .or_else(|| {
                view.search_visible_result.as_ref().filter(|result| {
                    let end = view.total_rows_for_ui().saturating_sub(view.scroll_offset);
                    let start = end.saturating_sub(view.viewport_rows_for_ui());
                    terminal_frame_search_result_is_current(result, &key, view.screen_revision)
                        && view.search_visible_range == Some((start, end))
                })
            })
    }

    pub(in crate::features) fn terminal_buffer_matches(
        &self,
    ) -> Result<Arc<[TerminalBufferMatch]>, String> {
        self.terminal_buffer_search_result()
            .map(|result| result.matches.clone())
            .unwrap_or_else(|| Ok(Arc::from([])))
    }

    pub(in crate::features) fn terminal_buffer_occurrence_ranges(&self) -> Arc<[Range<usize>]> {
        self.terminal_buffer_search_result()
            .map(|result| result.occurrences.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    pub(in crate::features) fn terminal_buffer_search_complete(&self) -> bool {
        let Some(key) = self.terminal_search_key() else {
            return true;
        };
        self.session
            .active_id()
            .and_then(|session_id| self.terminal.view.views.get(session_id))
            .is_some_and(|view| {
                view.search_result.as_ref().is_some_and(|result| {
                    terminal_frame_search_result_is_current(result, &key, view.screen_revision)
                })
            })
    }

    pub(in crate::features) fn terminal_buffer_search_needs_wake(&self) -> bool {
        self.terminal.search.open
            && self.terminal.search.mode == TerminalSearchMode::Buffer
            && self.terminal_search_key().is_some()
            && self
                .session
                .active_id()
                .is_some_and(|session_id| self.terminal.view.views.contains_key(session_id))
            && !self.terminal_buffer_search_complete()
    }

    pub(in crate::features) fn terminal_buffer_search_truncated(&self) -> bool {
        self.terminal_buffer_search_result()
            .is_some_and(|result| result.truncated)
    }

    pub(in crate::features) fn terminal_selected_occurrence_matches_for_session(
        &self,
        session_id: &str,
    ) -> Result<TerminalSelectedOccurrenceMatches, String> {
        let Some(query) = self
            .terminal
            .selection
            .selected_occurrence
            .query
            .as_ref()
            .filter(|_| {
                self.terminal
                    .selection
                    .selected_occurrence
                    .session_id
                    .as_deref()
                    == Some(session_id)
            })
        else {
            return Ok(TerminalSelectedOccurrenceMatches::default());
        };
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return Ok(TerminalSelectedOccurrenceMatches::default());
        };
        let key = TerminalFrameSearchKey {
            query: query.clone(),
            case_sensitive: true,
            regex: false,
            whole_word: false,
            limit: 2000,
            request_generation: self.terminal.selection.selected_occurrence.generation,
        };
        let selection = self
            .terminal
            .selection
            .selection
            .filter(|_| self.terminal.selection.session_id.as_deref() == Some(session_id));
        current_selected_occurrence_matches(view, &key, selection)
    }

    pub(in crate::features) fn terminal_overview_markers_for_session(
        &self,
        session_id: &str,
    ) -> (Vec<TerminalOverviewMarker>, usize) {
        let selected_matches = self
            .terminal_selected_occurrence_matches_for_session(session_id)
            .unwrap_or_default();
        self.terminal_overview_markers_for_session_with_selected_matches(
            session_id,
            &selected_matches,
        )
    }

    pub(in crate::features) fn terminal_overview_markers_for_session_with_selected_matches(
        &self,
        session_id: &str,
        selected_matches: &TerminalSelectedOccurrenceMatches,
    ) -> (Vec<TerminalOverviewMarker>, usize) {
        let total_rows = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.total_rows_for_ui())
            .unwrap_or_else(|| self.terminal.view.screen.total_rows())
            .max(1);
        let mut markers = Vec::with_capacity(selected_matches.matches.len());
        markers.extend(
            selected_matches
                .iter()
                .map(|search_match| TerminalOverviewMarker {
                    absolute_line: search_match.line_index,
                    kind: TerminalOverviewMarkerKind::SelectedOccurrence,
                }),
        );
        if self.session.active_id() == Some(session_id)
            && self.terminal.search.open
            && self.terminal.search.mode == TerminalSearchMode::Buffer
            && let Ok(matches) = self.terminal_buffer_matches()
        {
            let occurrences = self.terminal_buffer_occurrence_ranges();
            let active_index = self
                .terminal
                .search
                .active_index
                .min(occurrences.len().saturating_sub(1));
            let active_range = occurrences.get(active_index).cloned();
            for (index, m) in matches.iter().enumerate() {
                markers.push(TerminalOverviewMarker {
                    absolute_line: m.line_index,
                    kind: if active_range
                        .as_ref()
                        .is_some_and(|range| range.contains(&index))
                    {
                        TerminalOverviewMarkerKind::ActiveSearchMatch
                    } else {
                        TerminalOverviewMarkerKind::SearchMatch
                    },
                });
            }
        }
        (markers, total_rows)
    }

    pub(in crate::features) fn terminal_overview_marker_key_for_session_with_selected_matches(
        &self,
        session_id: &str,
        selected_matches: &TerminalSelectedOccurrenceMatches,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        session_id.hash(&mut hasher);
        self.terminal
            .selection
            .selected_occurrence
            .generation
            .hash(&mut hasher);
        self.terminal
            .selection
            .selected_occurrence
            .session_id
            .as_deref()
            .hash(&mut hasher);
        self.terminal
            .selection
            .selected_occurrence
            .query
            .as_deref()
            .hash(&mut hasher);
        self.terminal.search.active_index.hash(&mut hasher);
        self.terminal.search.open.hash(&mut hasher);
        self.terminal.search.mode.hash(&mut hasher);
        selected_matches.position_fingerprint.hash(&mut hasher);
        selected_matches.revision.hash(&mut hasher);
        selected_matches.selection.hash(&mut hasher);
        let mut has_markers = !selected_matches.is_empty();
        if let Some(view) = self.terminal.view.views.get(session_id) {
            let active_buffer_search = self.session.active_id() == Some(session_id)
                && self.terminal.search.open
                && self.terminal.search.mode == TerminalSearchMode::Buffer;
            active_buffer_search.hash(&mut hasher);
            if active_buffer_search {
                let search_key = self.terminal_search_key();
                search_key.hash(&mut hasher);
                let search_result = search_key.as_ref().and_then(|key| {
                    let complete = view.search_result.as_ref().filter(|result| {
                        terminal_frame_search_result_is_current(result, key, view.screen_revision)
                    });
                    complete.or_else(|| {
                        let end = view.total_rows_for_ui().saturating_sub(view.scroll_offset);
                        let start = end.saturating_sub(view.viewport_rows_for_ui());
                        (start, end).hash(&mut hasher);
                        view.search_visible_result.as_ref().filter(|result| {
                            terminal_frame_search_result_is_current(
                                result,
                                key,
                                view.screen_revision,
                            ) && view.search_visible_range == Some((start, end))
                        })
                    })
                });
                let search_is_current = search_result.is_some();
                search_is_current.hash(&mut hasher);
                if let Some(result) = search_result {
                    result.key.hash(&mut hasher);
                    result.revision.hash(&mut hasher);
                    result.position_fingerprint.hash(&mut hasher);
                    let non_empty = result
                        .matches
                        .as_ref()
                        .is_ok_and(|matches| !matches.is_empty());
                    non_empty.hash(&mut hasher);
                    has_markers |= non_empty;
                }
            }
            if has_markers {
                view.total_rows_for_ui().hash(&mut hasher);
            } else {
                0usize.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Ensure the absolute buffer line is visible by adjusting scroll_offset.
    pub(in crate::features) fn reveal_terminal_absolute_line(
        &mut self,
        abs_line: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(session_id) = self.session.active_id_owned() {
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                let total = view.total_rows_for_ui();
                let rows = view.viewport_rows_for_ui();
                let max_start = total.saturating_sub(rows);
                let start = abs_line.min(max_start);
                let offset = total.saturating_sub(start + rows);
                view.scroll_offset = offset.min(view.scrollback_len_for_ui());
                self.clear_terminal_scroll_residual_for_session(Some(&session_id));
            }
            self.notify_terminal_scroll_after_state_change(Some(session_id.as_str()), cx);
        } else {
            let total = self.terminal.view.screen.total_rows().max(1);
            let rows = self
                .terminal_snapshot_for_session(None, 0)
                .row_count()
                .max(1);
            let max_start = total.saturating_sub(rows);
            let start = abs_line.min(max_start);
            let offset = total.saturating_sub(start + rows);
            self.terminal.view.scroll_offset =
                offset.min(self.terminal.view.screen.scrollback_len());
            self.clear_terminal_scroll_residual_for_session(None);
            cx.notify();
        }
    }

    pub(in crate::features) fn terminal_history_search_key(
        &self,
    ) -> Option<RecordingHistorySearchKey> {
        let session_id = self.session.active_id_owned()?;
        let query = self.terminal.search.query.trim();
        if query.is_empty() {
            return None;
        }
        Some(RecordingHistorySearchKey {
            session_id,
            query: query.to_string(),
            case_sensitive: self.terminal.search.case_sensitive,
            regex: self.terminal.search.regex,
            whole_word: self.terminal.search.whole_word,
            limit: Some(8),
            context_before: Some(1),
            context_after: Some(1),
            max_lines: Some(30_000),
        })
    }

    pub(in crate::features) fn request_active_terminal_history_search(&mut self) {
        if !self.terminal.search.open || self.terminal.search.mode != TerminalSearchMode::History {
            return;
        }
        let Some(key) = self.terminal_history_search_key() else {
            self.terminal.search.history_pending_key = None;
            self.terminal.search.history_result = None;
            return;
        };
        if self.terminal.search.history_pending_key.as_ref() == Some(&key)
            || self
                .terminal
                .search
                .history_result
                .as_ref()
                .is_some_and(|result| result.key == key)
        {
            return;
        }
        self.terminal.search.history_pending_key = Some(key.clone());
        self.recording.request_history_search(key);
    }

    /// Deliver recording-writer replies as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `try_recv_event`, and the only thing keeping that wait short was the
    /// `history_search_is_pending` term in `runtime_quiet_tick_allowed` -- which
    /// was missing until it was added as a bug fix, so a recording-history search
    /// result could sit for a full quiet interval on an otherwise idle app, which
    /// is exactly the state someone using a search box is in.
    pub(in crate::features) fn start_recording_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.recording.take_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.apply_recording_write_event(event) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Apply one writer reply, reporting whether the UI needs a repaint.
    pub(in crate::features) fn apply_recording_write_event(
        &mut self,
        event: RecordingWriteEvent,
    ) -> bool {
        match event {
            RecordingWriteEvent::Status(status) => {
                let state = status.state;
                let previous_state = self
                    .recording
                    .status(&status.session_id)
                    .map(|status| status.state);
                let last_error = status.last_error.clone();
                self.recording.apply_status(status);
                match state {
                    nyaterm_recording::recording::RecordingStatusState::Degraded
                        if previous_state != Some(state) =>
                    {
                        self.shell.set_status(format!(
                            "{}: {}",
                            t!("recording.degraded"),
                            last_error.unwrap_or_else(|| t!("recording.degraded").to_string())
                        ));
                    }
                    nyaterm_recording::recording::RecordingStatusState::Failed
                        if previous_state != Some(state) =>
                    {
                        self.shell.set_status(format!(
                            "{}: {}",
                            t!("recording.failed"),
                            last_error.unwrap_or_else(|| t!("recording.failed").to_string())
                        ));
                    }
                    nyaterm_recording::recording::RecordingStatusState::Starting
                    | nyaterm_recording::recording::RecordingStatusState::Recording
                    | nyaterm_recording::recording::RecordingStatusState::Stopping
                    | nyaterm_recording::recording::RecordingStatusState::Degraded
                    | nyaterm_recording::recording::RecordingStatusState::Failed => {}
                }
                true
            }
            RecordingWriteEvent::PendingStatus { .. }
            | RecordingWriteEvent::PendingHistorySearch { .. } => false,
            RecordingWriteEvent::ShutdownError(error) => {
                self.shell
                    .set_status(format!("{}: {error}", t!("recording.failed")));
                true
            }
            RecordingWriteEvent::StatusRemoved { session_id } => {
                self.recording.remove_status(&session_id);
                true
            }
            RecordingWriteEvent::HistorySearch(event) => {
                // A reply for a query the user has already moved on from.
                if self.terminal.search.history_pending_key.as_ref() != Some(&event.key) {
                    return false;
                }
                self.terminal.search.history_pending_key = None;
                self.terminal.search.history_result = Some(event);
                true
            }
        }
    }

    pub(in crate::features) fn terminal_history_search_pending_for_current_query(&self) -> bool {
        let Some(key) = self.terminal_history_search_key() else {
            return false;
        };
        self.terminal.search.history_pending_key.as_ref() == Some(&key)
    }

    pub(in crate::features) fn terminal_history_search_results(
        &self,
    ) -> Result<nyaterm_recording::recording::TerminalHistorySearchResponse, String> {
        let Some(key) = self.terminal_history_search_key() else {
            return Ok(empty_terminal_history_search_response());
        };
        if let Some(result) = self
            .terminal
            .search
            .history_result
            .as_ref()
            .filter(|result| result.key == key)
        {
            return result.result.clone();
        }
        Ok(empty_terminal_history_search_response())
    }

    pub(in crate::features) fn navigate_terminal_search(
        &mut self,
        direction: isize,
        cx: &mut Context<Self>,
    ) {
        let count = match self.terminal.search.mode {
            TerminalSearchMode::Buffer => self.terminal_buffer_occurrence_ranges().len(),
            TerminalSearchMode::History => self
                .terminal_history_search_results()
                .map(|response| response.results.len())
                .unwrap_or(0),
        };
        if count == 0 {
            self.terminal.search.active_index = 0;
            self.shell
                .set_status("terminal search has no matches".to_string());
            self.notify_active_terminal_surface(cx);
            cx.notify();
            return;
        }
        let wrap_around = self.terminal.search_wrap_around(self.session.active_id());
        let (next_index, hit_boundary) = terminal_search_next_index(
            self.terminal.search.active_index,
            direction,
            count,
            wrap_around,
        );
        self.terminal.search.active_index = next_index;
        if self.terminal.search.mode == TerminalSearchMode::Buffer
            && let Ok(matches) = self.terminal_buffer_matches()
            && let Some(range) = self
                .terminal_buffer_occurrence_ranges()
                .get(self.terminal.search.active_index)
            && let Some(m) = matches.get(range.start)
        {
            self.reveal_terminal_absolute_line(m.line_index, cx);
        }
        self.shell.set_status(if hit_boundary {
            if direction < 0 {
                t!(
                    "terminalCtx.searchReachedFirst",
                    current = self.terminal.search.active_index + 1,
                    total = count
                )
                .to_string()
            } else {
                t!(
                    "terminalCtx.searchReachedLast",
                    current = self.terminal.search.active_index + 1,
                    total = count
                )
                .to_string()
            }
        } else {
            t!(
                "terminalCtx.searchMatchStatus",
                current = self.terminal.search.active_index + 1,
                total = count
            )
            .to_string()
        });
        self.notify_active_terminal_surface(cx);
        cx.notify();
    }

    pub(in crate::features) fn handle_terminal_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        match keystroke.key.as_str() {
            "escape" => self.close_terminal_search(window, cx),
            "enter" => {
                if keystroke.modifiers.shift {
                    self.navigate_terminal_search(-1, cx);
                } else {
                    self.navigate_terminal_search(1, cx);
                }
            }
            "tab" => {
                self.terminal.search.mode =
                    if self.terminal.search.mode == TerminalSearchMode::Buffer {
                        TerminalSearchMode::History
                    } else {
                        TerminalSearchMode::Buffer
                    };
                self.terminal.search.active_index = 0;
                self.refresh_terminal_search_state(cx);
            }
            _ => return false,
        }
        true
    }

    /// Apply an edit from the active sessions filter box.
    pub(in crate::features) fn apply_active_sessions_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.session.set_active_search_draft(text);
        cx.notify();
    }
}

fn terminal_search_next_index(
    current: usize,
    direction: isize,
    count: usize,
    wrap_around: bool,
) -> (usize, bool) {
    if count == 0 {
        return (0, false);
    }
    let candidate = current.min(count - 1) as isize + direction;
    if wrap_around {
        (candidate.rem_euclid(count as isize) as usize, false)
    } else if candidate < 0 {
        (0, true)
    } else if candidate >= count as isize {
        (count - 1, true)
    } else {
        (candidate as usize, false)
    }
}

fn empty_terminal_history_search_response()
-> nyaterm_recording::recording::TerminalHistorySearchResponse {
    nyaterm_recording::recording::TerminalHistorySearchResponse {
        total: 0,
        elapsed_ms: 0,
        truncated: false,
        results: Vec::new(),
    }
}

fn selected_occurrence_is_original(
    search_match: &TerminalBufferMatch,
    selection: Option<TerminalSelection>,
) -> bool {
    selection.is_some_and(|selection| {
        selection
            .cols_for_absolute_line(search_match.line_index)
            .is_some_and(|(start, end)| {
                search_match.start_col >= start && search_match.end_col <= end
            })
    })
}

fn current_selected_occurrence_matches(
    view: &crate::models::TerminalViewState,
    key: &TerminalFrameSearchKey,
    selection: Option<TerminalSelection>,
) -> Result<TerminalSelectedOccurrenceMatches, String> {
    let selection = selection.map(|selection| {
        view.frame_snapshot
            .iter()
            .chain(view.scrollback_snapshots.values())
            .find_map(|snapshot| selection.expanded_for_snapshot(snapshot))
            .unwrap_or(selection)
    });
    // The visible result provides immediate feedback. Once the full-buffer
    // result arrives it is authoritative for both decorations and the ruler.
    let result = view
        .selected_occurrence_result
        .as_ref()
        .filter(|result| terminal_frame_search_result_is_current(result, key, view.screen_revision))
        .or_else(|| {
            view.selected_occurrence_visible_result
                .as_ref()
                .filter(|result| {
                    terminal_frame_search_result_is_current(result, key, view.screen_revision)
                })
        });
    result
        .map(|result| {
            result
                .matches
                .clone()
                .map(|matches| TerminalSelectedOccurrenceMatches {
                    matches,
                    selection,
                    position_fingerprint: result.position_fingerprint,
                    revision: result.revision,
                })
        })
        .unwrap_or_else(|| Ok(TerminalSelectedOccurrenceMatches::default()))
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalSelectedOccurrenceMatches, current_selected_occurrence_matches,
        terminal_matches_in_absolute_range, terminal_search_index_for_position,
        terminal_search_next_index,
    };
    use crate::features::terminal::terminal_surface::{
        TerminalDecorationSources, TerminalOverviewMarker, TerminalOverviewMarkerKind,
        build_terminal_line_decorations, terminal_snapshot_absolute_range,
    };
    use crate::models::{
        TerminalBufferCellPos, TerminalFrameSearchKey, TerminalFrameSearchResult,
        TerminalSelection, TerminalViewState,
    };
    use crate::terminal::TerminalBufferMatch;
    use std::collections::HashMap;

    #[test]
    fn search_navigation_wraps_by_default_and_stops_when_disabled() {
        assert_eq!(terminal_search_next_index(2, 1, 3, true), (0, false));
        assert_eq!(terminal_search_next_index(0, -1, 3, true), (2, false));
        assert_eq!(terminal_search_next_index(2, 1, 3, false), (2, true));
        assert_eq!(terminal_search_next_index(0, -1, 3, false), (0, true));
        assert_eq!(terminal_search_next_index(1, 1, 3, false), (2, false));
        assert_eq!(terminal_search_next_index(8, -1, 3, false), (1, false));
    }

    #[test]
    fn full_search_preserves_visible_active_match_position() {
        let matches = vec![
            TerminalBufferMatch {
                line_index: 1,
                start_col: 0,
                end_col: 3,
            },
            TerminalBufferMatch {
                line_index: 5,
                start_col: 3,
                end_col: 5,
            },
            TerminalBufferMatch {
                line_index: 6,
                start_col: 0,
                end_col: 2,
            },
            TerminalBufferMatch {
                line_index: 9,
                start_col: 0,
                end_col: 3,
            },
        ];
        let occurrences = vec![0..1, 1..3, 3..4];
        assert_eq!(
            terminal_search_index_for_position(&matches, &occurrences, (5, 3)),
            1
        );
        assert_eq!(
            terminal_search_index_for_position(&matches, &occurrences, (7, 0)),
            2
        );
    }

    #[test]
    fn selected_occurrence_filter_excludes_the_original_selection_only() {
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(4, 2),
            TerminalBufferCellPos::new(4, 8),
        );
        let matches = vec![
            TerminalBufferMatch {
                line_index: 4,
                start_col: 2,
                end_col: 9,
            },
            TerminalBufferMatch {
                line_index: 4,
                start_col: 20,
                end_col: 27,
            },
            TerminalBufferMatch {
                line_index: 5,
                start_col: 2,
                end_col: 9,
            },
        ];

        let filtered = TerminalSelectedOccurrenceMatches {
            matches: matches.into(),
            selection: Some(selection),
            position_fingerprint: 0,
            revision: 0,
        }
        .iter()
        .cloned()
        .collect::<Vec<_>>();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].line_index, 4);
        assert_eq!(filtered[1].line_index, 5);
    }

    #[test]
    fn selected_occurrence_promotes_visible_result_to_full_buffer_result() {
        let key = TerminalFrameSearchKey {
            query: "address".to_string(),
            case_sensitive: true,
            regex: false,
            whole_word: false,
            limit: 2000,
            request_generation: 3,
        };
        let mut view = TerminalViewState::new();
        view.screen_revision = 7;
        view.selected_occurrence_visible_result = Some(TerminalFrameSearchResult::new(
            key.clone(),
            7,
            Ok(vec![TerminalBufferMatch {
                line_index: 2,
                start_col: 5,
                end_col: 12,
            }]),
        ));

        assert_eq!(
            current_selected_occurrence_matches(&view, &key, None)
                .unwrap()
                .iter()
                .count(),
            1
        );

        view.selected_occurrence_result = Some(TerminalFrameSearchResult::new(
            key.clone(),
            7,
            Ok((0..3)
                .map(|line_index| TerminalBufferMatch {
                    line_index,
                    start_col: 5,
                    end_col: 12,
                })
                .collect()),
        ));

        let full = current_selected_occurrence_matches(&view, &key, None).unwrap();
        assert_eq!(full.iter().count(), 3);
        assert!(std::sync::Arc::ptr_eq(
            &full.matches,
            view.selected_occurrence_result
                .as_ref()
                .unwrap()
                .matches
                .as_ref()
                .unwrap()
        ));
    }

    #[test]
    fn absolute_range_slice_uses_sorted_match_boundaries() {
        let matches = (0..100)
            .map(|line_index| TerminalBufferMatch {
                line_index,
                start_col: 1,
                end_col: 3,
            })
            .collect::<Vec<_>>();

        let visible = terminal_matches_in_absolute_range(&matches, 40..44);

        assert_eq!(
            visible
                .iter()
                .map(|search_match| search_match.line_index)
                .collect::<Vec<_>>(),
            vec![40, 41, 42, 43]
        );
    }

    #[test]
    fn selected_address_pipeline_builds_two_backgrounds_and_two_markers() {
        use nyaterm_terminal::{TerminalSearchDirection, TerminalSearchQuery};

        let mut screen = nyaterm_terminal::TerminalScreen::new(40, 3);
        screen.advance(b"IPv4 address for br0\r\nIPv6 address for br0\r\nIPv6 address for br1");
        let snapshot = screen.viewport_snapshot(0);
        let (absolute_start, _) = terminal_snapshot_absolute_range(&snapshot);
        let matches = screen
            .search_grid(&TerminalSearchQuery {
                pattern: "address".to_string(),
                regex: false,
                case_sensitive: true,
                whole_word: false,
                direction: TerminalSearchDirection::Forward,
                limit: 2000,
            })
            .unwrap()
            .into_iter()
            .map(|item| TerminalBufferMatch {
                line_index: item.line_index,
                start_col: item.start_col,
                end_col: item.end_col,
            })
            .collect::<Vec<_>>();
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(absolute_start + 2, 5),
            TerminalBufferCellPos::new(absolute_start + 2, 11),
        );
        let filtered = TerminalSelectedOccurrenceMatches {
            matches: matches.into(),
            selection: Some(selection),
            position_fingerprint: 0,
            revision: 0,
        }
        .iter()
        .cloned()
        .collect::<Vec<_>>();
        let ranges = filtered.iter().fold(HashMap::new(), |mut ranges, item| {
            ranges
                .entry(item.line_index - absolute_start)
                .or_insert_with(Vec::new)
                .push((item.start_col, item.end_col));
            ranges
        });
        let decorations = build_terminal_line_decorations(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &ranges,
                search_ranges_by_line: &HashMap::new(),
                active_search_ranges_by_line: &HashMap::new(),
                frame_action_links: &[],
                include_action_links: false,
                include_hyperlinks: false,
            },
        );
        let markers = filtered
            .iter()
            .map(|item| TerminalOverviewMarker {
                absolute_line: item.line_index,
                kind: TerminalOverviewMarkerKind::SelectedOccurrence,
            })
            .collect::<Vec<_>>();

        assert_eq!(filtered.len(), 2);
        assert_eq!(decorations[0].selected_occurrence_ranges, vec![(5, 12)]);
        assert_eq!(decorations[1].selected_occurrence_ranges, vec![(5, 12)]);
        assert!(decorations[2].selected_occurrence_ranges.is_empty());
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].absolute_line, absolute_start);
        assert_eq!(markers[1].absolute_line, absolute_start + 1);
    }
}
