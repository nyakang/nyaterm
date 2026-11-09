//! Terminal frame processing and ordered recording coordination.

use super::frame_commands::{
    TerminalFrameCommand, TerminalFrameCommandReceiver, TerminalSelectionTextRequest,
    TerminalSelectionTextResult,
};
use super::frame_events::{
    TerminalFrameEvent, TerminalFrameEventQueue, TerminalFrameOutputEvent,
    TerminalFrameSearchEvent, TerminalFrameSnapshotEvent, TerminalFrameSnapshotPurpose,
    push_terminal_frame_worker_event,
};
use super::frame_search::{
    FindSearchJob, SelectedOccurrenceSearchJob, TerminalFrameSearchKey, TerminalFrameSearchPurpose,
    TerminalFrameSearchResult, cancel_find_search_job_for_session,
    cancel_selected_occurrence_search_job_for_session, process_next_find_search_chunk,
    process_next_selected_occurrence_search_chunk, replace_selected_occurrence_search_job,
};
use super::{
    TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP, TerminalFrameActionLinks, TerminalPresentation,
    TerminalProtocolState, TerminalSelectionTextError, TerminalWorkPolicy,
    append_terminal_frame_visible_tail, merge_terminal_effects,
    prepare_terminal_frame_action_links_reusing, protect_terminal_output_burst,
    terminal_frame_live_snapshot_with_stats, terminal_frame_snapshot_with_scroll_window_and_stats,
};
use crate::models::recording::RecordingWriteHandle;
use crate::terminal::{TerminalBufferMatch, terminal_screen_from_output};
use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_terminal::action_links::{
    TerminalActionLinkBuildStats, prepare_terminal_frame_action_links,
};
use nyaterm_terminal::{
    TerminalEffects, TerminalOutputDecoder, TerminalScreen, TerminalSearchDirection,
    TerminalSearchQuery, TerminalSearchRangeMode, TerminalSnapshotBuildStats,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

pub(crate) struct TerminalFrameSession {
    pub(super) screen: TerminalScreen,
    pub(super) output_decoder: TerminalOutputDecoder,
    pub(super) recording_decoder: TerminalOutputDecoder,
    pub(super) revision: u64,
    /// When false, output frames omit full viewport_snapshot (hidden tabs).
    pub(super) include_live_snapshot: bool,
    pub(super) action_link_cache: Option<TerminalFrameActionLinks>,
}

impl std::fmt::Debug for TerminalFrameSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalFrameSession")
            .field("revision", &self.revision)
            .field("include_live_snapshot", &self.include_live_snapshot)
            .finish_non_exhaustive()
    }
}

impl TerminalFrameSession {
    pub(super) fn new(encoding: &str, scrollback_limit: usize) -> Self {
        let mut screen = TerminalScreen::default();
        if let Err(error) = screen.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        screen.set_scrollback_limit(scrollback_limit);
        let mut output_decoder = TerminalOutputDecoder::default();
        if let Err(error) = output_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        let mut recording_decoder = TerminalOutputDecoder::default();
        if let Err(error) = recording_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        Self {
            screen,
            output_decoder,
            recording_decoder,
            revision: 0,
            // New sessions start high-priority until UI reports visibility.
            include_live_snapshot: true,
            action_link_cache: None,
        }
    }

    pub(super) fn set_encoding_and_limit(&mut self, encoding: &str, scrollback_limit: usize) {
        if let Err(error) = self.screen.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        self.screen.set_scrollback_limit(scrollback_limit);
        if let Err(error) = self.output_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        if let Err(error) = self.recording_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
    }

    pub(super) fn reset_reconnect_stream(&mut self, encoding: &str, scrollback_limit: usize) {
        self.screen.reset_stream_state();
        self.screen.reset_session_input_protocols();
        self.output_decoder.reset_decoder();
        self.recording_decoder.reset_decoder();
        self.set_encoding_and_limit(encoding, scrollback_limit);
        self.revision = self.revision.saturating_add(1);
        self.action_link_cache = None;
    }

    pub(super) fn seed(&mut self, output: String, encoding: &str, scrollback_limit: usize) {
        self.screen = terminal_screen_from_output(&output);
        if let Err(error) = self.screen.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        self.screen.set_scrollback_limit(scrollback_limit);
        self.output_decoder = TerminalOutputDecoder::default();
        if let Err(error) = self.output_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        self.recording_decoder = TerminalOutputDecoder::default();
        if let Err(error) = self.recording_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        self.revision = self.revision.saturating_add(1);
        self.action_link_cache = None;
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) {
        if self.screen.cols() as u16 != cols || self.screen.rows() as u16 != rows {
            self.screen.resize(cols, rows);
            self.revision = self.revision.saturating_add(1);
            self.action_link_cache = None;
        }
    }

    pub(super) fn resized_live_snapshot_event(
        &self,
        session_id: String,
        started_at: Instant,
    ) -> TerminalFrameSnapshotEvent {
        let (snapshot, snapshot_duration, snapshot_stats) =
            terminal_frame_snapshot_with_scroll_window_and_stats(&self.screen, 0, false);
        TerminalFrameSnapshotEvent {
            session_id,
            offset: 0,
            snapshot,
            action_links: None,
            revision: self.revision,
            purpose: TerminalFrameSnapshotPurpose::Paint,
            snapshot_duration,
            snapshot_stats,
            action_link_stats: TerminalActionLinkBuildStats::default(),
            process_duration: started_at.elapsed(),
        }
    }

    #[cfg(test)]
    pub(super) fn process_output(
        &mut self,
        session_id: String,
        data: Vec<u8>,
        encoding: String,
        scrollback_limit: usize,
        recording_writer: &RecordingWriteHandle,
    ) -> TerminalFrameOutputEvent {
        let started_at = Instant::now();
        let mut batch = TerminalFrameOutputBatch::default();
        batch.absorb(self.process_output_chunk(
            &session_id,
            &data,
            &encoding,
            scrollback_limit,
            recording_writer,
        ));
        self.output_event_from_batch(session_id, batch, started_at)
    }

    pub(super) fn process_output_chunk(
        &mut self,
        session_id: &str,
        data: &[u8],
        encoding: &str,
        scrollback_limit: usize,
        recording_writer: &RecordingWriteHandle,
    ) -> TerminalAdvanceResult {
        self.set_encoding_and_limit(encoding, scrollback_limit);
        let result = terminal_advance_result(
            &mut self.screen,
            &mut self.output_decoder,
            &mut self.recording_decoder,
            session_id,
            data,
            recording_writer,
        );
        self.revision = self.revision.saturating_add(1);
        result
    }

    pub(super) fn output_event_from_batch(
        &self,
        session_id: String,
        batch: TerminalFrameOutputBatch,
        started_at: Instant,
    ) -> TerminalFrameOutputEvent {
        let command_running = self.screen.command_running();
        let protocol_state = TerminalProtocolState::from_screen(&self.screen);
        // Hidden/low-priority sessions keep protocol/effects without paying for a
        // full grid snapshot every output frame.
        let (snapshot, snapshot_duration, snapshot_stats) = if self.include_live_snapshot {
            let (snapshot, duration, stats) = terminal_frame_live_snapshot_with_stats(&self.screen);
            (Some(snapshot), duration, stats)
        } else {
            (None, Duration::ZERO, TerminalSnapshotBuildStats::default())
        };
        TerminalFrameOutputEvent {
            session_id,
            visible_text: batch.visible_text,
            recording_text_bytes: batch.recording_text_bytes,
            snapshot,
            action_links: None,
            protocol_state,
            effects: batch.effects,
            command_running,
            accepted_bytes: batch.accepted_bytes,
            skipped_output_bytes: batch.skipped_output_bytes,
            revision: self.revision,
            snapshot_duration,
            snapshot_stats,
            process_duration: started_at.elapsed(),
        }
    }

    pub(super) fn snapshot_event(
        &mut self,
        session_id: String,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
        priority: bool,
        purpose: TerminalFrameSnapshotPurpose,
    ) -> TerminalFrameSnapshotEvent {
        let started_at = Instant::now();
        let (snapshot, snapshot_duration, snapshot_stats) = if priority {
            terminal_frame_snapshot_with_scroll_window_and_stats(&self.screen, offset, true)
        } else {
            terminal_frame_snapshot_with_scroll_window_and_stats(&self.screen, offset, false)
        };
        let (action_links, action_link_stats) = if action_links_enabled {
            if let Some(previous) = self.action_link_cache.as_ref() {
                prepare_terminal_frame_action_links_reusing(
                    &snapshot,
                    true,
                    &action_link_matchers,
                    Some(previous),
                )
            } else {
                (
                    prepare_terminal_frame_action_links(&snapshot, true, &action_link_matchers),
                    TerminalActionLinkBuildStats {
                        reused_rows: 0,
                        rebuilt_rows: snapshot.row_count(),
                    },
                )
            }
        } else {
            (
                prepare_terminal_frame_action_links(&snapshot, false, &action_link_matchers),
                TerminalActionLinkBuildStats::default(),
            )
        };
        if action_links_enabled {
            self.action_link_cache.clone_from(&action_links);
        }
        TerminalFrameSnapshotEvent {
            session_id,
            offset: snapshot.display_offset,
            snapshot,
            action_links,
            revision: self.revision,
            purpose,
            snapshot_duration,
            snapshot_stats,
            action_link_stats,
            process_duration: started_at.elapsed(),
        }
    }

    pub(super) fn search_event(
        &mut self,
        session_id: String,
        purpose: TerminalFrameSearchPurpose,
        key: TerminalFrameSearchKey,
    ) -> TerminalFrameSearchEvent {
        let started_at = Instant::now();
        let query = TerminalSearchQuery {
            pattern: key.query.clone(),
            regex: key.regex,
            case_sensitive: key.case_sensitive,
            whole_word: key.whole_word,
            direction: TerminalSearchDirection::Forward,
            limit: key.limit,
        };
        if let TerminalFrameSearchPurpose::FindVisible {
            absolute_start,
            absolute_end,
        } = purpose
        {
            let visible_query = TerminalSearchQuery {
                limit: key.limit.saturating_add(1),
                ..query
            };
            let occurrences = self
                .screen
                .search_grid_occurrences_in_absolute_range(
                    &visible_query,
                    absolute_start..absolute_end,
                    TerminalSearchRangeMode::Intersects,
                )
                .map_err(|error| error.to_string());
            return TerminalFrameSearchEvent {
                session_id,
                purpose,
                result: TerminalFrameSearchResult::new_find(key, self.revision, occurrences),
                process_duration: started_at.elapsed(),
            };
        }
        let matches = match purpose {
            TerminalFrameSearchPurpose::SelectedOccurrenceVisible {
                absolute_start,
                absolute_end,
            } => self
                .screen
                .search_grid_in_absolute_range(&query, absolute_start..absolute_end),
            TerminalFrameSearchPurpose::Find | TerminalFrameSearchPurpose::SelectedOccurrence => {
                self.screen.search_grid(&query)
            }
            TerminalFrameSearchPurpose::FindVisible { .. } => unreachable!(),
        }
        .map(|matches| {
            matches
                .into_iter()
                .map(|m| TerminalBufferMatch {
                    line_index: m.line_index,
                    start_col: m.start_col,
                    end_col: m.end_col,
                })
                .collect()
        })
        .map_err(|error| error.to_string());
        TerminalFrameSearchEvent {
            session_id,
            purpose,
            result: TerminalFrameSearchResult::new(key, self.revision, matches),
            process_duration: started_at.elapsed(),
        }
    }
}

#[derive(Debug)]
pub(super) struct TerminalAdvanceResult {
    pub(super) visible_text: String,
    pub(super) recording_text_bytes: usize,
    pub(super) effects: TerminalEffects,
    pub(super) accepted_bytes: usize,
    pub(super) skipped_output_bytes: usize,
}

#[derive(Debug, Default)]
pub(super) struct TerminalFrameOutputBatch {
    pub(super) visible_text: String,
    pub(super) recording_text_bytes: usize,
    pub(super) effects: TerminalEffects,
    pub(super) accepted_bytes: usize,
    pub(super) skipped_output_bytes: usize,
}

impl TerminalFrameOutputBatch {
    pub(super) fn absorb(&mut self, chunk: TerminalAdvanceResult) {
        append_terminal_frame_visible_tail(&mut self.visible_text, &chunk.visible_text);
        self.recording_text_bytes = self
            .recording_text_bytes
            .saturating_add(chunk.recording_text_bytes);
        self.accepted_bytes = self.accepted_bytes.saturating_add(chunk.accepted_bytes);
        self.skipped_output_bytes = self
            .skipped_output_bytes
            .saturating_add(chunk.skipped_output_bytes);
        merge_terminal_effects(&mut self.effects, chunk.effects);
    }
}

pub(super) fn terminal_advance_decoded_result(
    screen: &mut TerminalScreen,
    session_id: &str,
    text: &str,
    recording_writer: &RecordingWriteHandle,
) -> TerminalAdvanceResult {
    recording_writer.write_output(session_id.to_string(), text.to_string());
    screen.advance_decoded_text(text);
    let mut visible_text = String::new();
    append_terminal_frame_visible_tail(&mut visible_text, text);
    TerminalAdvanceResult {
        visible_text,
        recording_text_bytes: text.len(),
        effects: screen.take_effects(),
        accepted_bytes: text.len(),
        skipped_output_bytes: 0,
    }
}

pub(super) fn terminal_advance_result(
    screen: &mut TerminalScreen,
    output_decoder: &mut TerminalOutputDecoder,
    recording_decoder: &mut TerminalOutputDecoder,
    session_id: &str,
    data: &[u8],
    recording_writer: &RecordingWriteHandle,
) -> TerminalAdvanceResult {
    let recording_text = recording_decoder.decode_output_text(data);
    let recording_text_bytes = recording_text.len();
    recording_writer.write_output(session_id.to_string(), recording_text);
    let (feed, skipped_output_bytes) = protect_terminal_output_burst(screen, output_decoder, data);
    screen.advance(feed);
    // Only the tail is ever kept, so cap inside the decoder rather than
    // building a whole burst's worth of text and draining it back down.
    let visible_text =
        output_decoder.decode_output_text_tail(feed, TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP);
    let effects = screen.take_effects();
    TerminalAdvanceResult {
        visible_text,
        recording_text_bytes,
        effects,
        accepted_bytes: feed.len(),
        skipped_output_bytes,
    }
}

pub(super) fn terminal_frame_live_snapshot_enabled(is_visible: bool) -> bool {
    let presentation = if is_visible {
        // The worker only needs visibility; active and inactive visible sessions
        // intentionally share live snapshot policy.
        TerminalPresentation::VisibleInactive
    } else {
        TerminalPresentation::Background
    };
    let policy = TerminalWorkPolicy::for_presentation(presentation);
    debug_assert!(policy.parse_output);
    policy.live_snapshot
}

pub(super) fn run_terminal_frame_processor(
    command_rx: TerminalFrameCommandReceiver,
    event_queue: TerminalFrameEventQueue,
    recording_writer: RecordingWriteHandle,
) {
    let mut sessions: HashMap<String, TerminalFrameSession> = HashMap::new();
    let mut logical_line_snapshots = false;
    let mut rekeyed_old_ids: HashSet<String> = HashSet::new();
    let mut rekeyed_old_order = VecDeque::new();
    // Sessions that should include full live viewport snapshots on every output.
    // Default (empty) keeps include_live_snapshot as-is for existing sessions and
    // true for newly created ones until the first priority update arrives.
    let mut snapshot_priority: HashSet<String> = HashSet::new();
    let mut priority_initialized = false;
    let mut pending_commands = VecDeque::new();
    let mut find_search_jobs = VecDeque::new();
    let mut selected_occurrence_search_jobs = VecDeque::new();
    loop {
        let mut command = try_next_terminal_frame_command(&command_rx, &mut pending_commands);
        if command.is_none()
            && find_search_jobs.is_empty()
            && selected_occurrence_search_jobs.is_empty()
        {
            command = command_rx.recv();
            if command.is_none() {
                break;
            }
        }
        let Some(command) = command else {
            if let Some(event) = process_next_find_search_chunk(&mut find_search_jobs, &sessions) {
                push_terminal_frame_worker_event(&event_queue, TerminalFrameEvent::Search(event));
            }
            if let Some(event) = process_next_selected_occurrence_search_chunk(
                &mut selected_occurrence_search_jobs,
                &sessions,
            ) {
                push_terminal_frame_worker_event(&event_queue, TerminalFrameEvent::Search(event));
            }
            continue;
        };
        let push_event = |queue: &TerminalFrameEventQueue, event: TerminalFrameEvent| {
            push_terminal_frame_worker_event(queue, event);
        };
        match command {
            TerminalFrameCommand::SetLogicalLineSnapshots { enabled } => {
                logical_line_snapshots = enabled;
                for (session_id, session) in &mut sessions {
                    session.screen.set_logical_line_snapshots(enabled);
                    session.revision = session.revision.saturating_add(1);
                    session.action_link_cache = None;
                    if session.include_live_snapshot {
                        push_event(
                            &event_queue,
                            TerminalFrameEvent::Snapshot(
                                session.resized_live_snapshot_event(
                                    session_id.clone(),
                                    Instant::now(),
                                ),
                            ),
                        );
                    }
                }
            }
            // Keyword rules only feed the surface's own highlighter now; the frame
            // worker no longer pre-parses input rows.
            TerminalFrameCommand::EnsureSession {
                session_id,
                encoding,
                scrollback_limit,
            } => {
                let include = terminal_frame_live_snapshot_enabled(
                    !priority_initialized || snapshot_priority.contains(&session_id),
                );
                let session = sessions
                    .entry(session_id)
                    .or_insert_with(|| TerminalFrameSession::new(&encoding, scrollback_limit));
                session.set_encoding_and_limit(&encoding, scrollback_limit);
                session
                    .screen
                    .set_logical_line_snapshots(logical_line_snapshots);
                session.include_live_snapshot = include;
            }
            TerminalFrameCommand::SeedSession {
                session_id,
                output,
                encoding,
                scrollback_limit,
            } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence search was cancelled by session reset",
                ) {
                    push_event(&event_queue, TerminalFrameEvent::Search(stale));
                }
                let include = terminal_frame_live_snapshot_enabled(
                    !priority_initialized || snapshot_priority.contains(&session_id),
                );
                let session = sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| TerminalFrameSession::new(&encoding, scrollback_limit));
                session.seed(output, &encoding, scrollback_limit);
                session
                    .screen
                    .set_logical_line_snapshots(logical_line_snapshots);
                session.include_live_snapshot = include;
            }
            TerminalFrameCommand::RemoveSession { session_id } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence session was removed",
                ) {
                    push_event(&event_queue, TerminalFrameEvent::Search(stale));
                }
                sessions.remove(&session_id);
                snapshot_priority.remove(&session_id);
            }
            TerminalFrameCommand::DecodedOutput { session_id, text } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    let started_at = Instant::now();
                    let mut batch = TerminalFrameOutputBatch::default();
                    batch.absorb(terminal_advance_decoded_result(
                        &mut session.screen,
                        &session_id,
                        &text,
                        &recording_writer,
                    ));
                    session.revision = session.revision.saturating_add(1);
                    let event = session.output_event_from_batch(session_id, batch, started_at);
                    push_event(&event_queue, TerminalFrameEvent::Output(event));
                }
            }
            TerminalFrameCommand::ExecutionRecording { session_id, event } => {
                recording_writer.write_execution_event(&session_id, event);
            }
            TerminalFrameCommand::SyntheticOutput { session_id, text } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    let started_at = Instant::now();
                    // Application presentation is never source output: no recording,
                    // prompt observation, credentials, or transport side effects.
                    session.screen.reset_stream_state();
                    session.screen.advance_decoded_text(&text);
                    session.screen.take_effects();
                    session.revision = session.revision.saturating_add(1);
                    session.action_link_cache = None;
                    let event = session.resized_live_snapshot_event(session_id, started_at);
                    push_event(&event_queue, TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::SystemRecording { session_id, text } => {
                recording_writer.write_local_message(session_id, text);
            }
            TerminalFrameCommand::AppendLocalText { session_id, text } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    let started_at = Instant::now();
                    session.screen.reset_stream_state();
                    session.output_decoder.reset_decoder();
                    session.recording_decoder.reset_decoder();
                    session.screen.advance_decoded_text(&text);
                    session.revision = session.revision.saturating_add(1);
                    session.action_link_cache = None;
                    let event = session.resized_live_snapshot_event(session_id, started_at);
                    push_event(&event_queue, TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::RekeySession {
                old_id,
                new_id,
                encoding,
                scrollback_limit,
            } => {
                for id in [&old_id, &new_id] {
                    if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                        &mut selected_occurrence_search_jobs,
                        id,
                        "selected occurrence search was cancelled by reconnect",
                    ) {
                        push_event(&event_queue, TerminalFrameEvent::Search(stale));
                    }
                }
                let success = if let Some(mut session) = sessions.remove(&old_id) {
                    session.reset_reconnect_stream(&encoding, scrollback_limit);
                    session.include_live_snapshot = terminal_frame_live_snapshot_enabled(
                        !priority_initialized || snapshot_priority.contains(&new_id),
                    );
                    sessions.insert(new_id.clone(), session);
                    snapshot_priority.remove(&old_id);
                    if rekeyed_old_ids.insert(old_id.clone()) {
                        rekeyed_old_order.push_back(old_id.clone());
                        if rekeyed_old_order.len() > 256
                            && let Some(expired) = rekeyed_old_order.pop_front()
                        {
                            rekeyed_old_ids.remove(&expired);
                        }
                    }
                    true
                } else {
                    false
                };
                push_event(
                    &event_queue,
                    TerminalFrameEvent::Rekeyed {
                        old_id,
                        new_id: new_id.clone(),
                        success,
                    },
                );
                if success {
                    let session = sessions.get(&new_id).expect("rekeyed session exists");
                    let event = session.resized_live_snapshot_event(new_id, Instant::now());
                    push_event(&event_queue, TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::CommandInput {
                session_id,
                fallback,
            } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    let changed = session.screen.reset_command_navigation();
                    if let Some(anchor) = fallback {
                        session.screen.record_fallback_command_at(anchor);
                    }
                    if changed {
                        session.revision = session.revision.saturating_add(1);
                        let event = session.resized_live_snapshot_event(session_id, Instant::now());
                        push_event(&event_queue, TerminalFrameEvent::Snapshot(event));
                    }
                }
            }
            TerminalFrameCommand::NavigateCommand {
                session_id,
                action,
                offset,
                source_history,
            } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    let offset = session
                        .screen
                        .history_state()
                        .reanchor_offset(source_history, offset);
                    if let Some(result) = session.screen.navigate_command_from_view(action, offset)
                    {
                        session.revision = session.revision.saturating_add(1);
                        if result.display_offset > 0 {
                            let live = session
                                .resized_live_snapshot_event(session_id.clone(), Instant::now());
                            push_event(&event_queue, TerminalFrameEvent::Snapshot(live));
                        }
                        let frame = session.snapshot_event(
                            session_id.clone(),
                            result.display_offset,
                            false,
                            ActionLinksMatcherSettings::default(),
                            true,
                            TerminalFrameSnapshotPurpose::Paint,
                        );
                        push_event(
                            &event_queue,
                            TerminalFrameEvent::CommandNavigation {
                                session_id,
                                result,
                                frame,
                            },
                        );
                    }
                }
            }
            TerminalFrameCommand::ClearExceptInput { session_id } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence search was cancelled by terminal clear",
                ) {
                    push_event(&event_queue, TerminalFrameEvent::Search(stale));
                }
                if let Some(session) = sessions.get_mut(&session_id) {
                    let started_at = Instant::now();
                    session.screen.clear_except_input();
                    session.revision = session.revision.saturating_add(1);
                    session.action_link_cache = None;
                    let event = session.resized_live_snapshot_event(session_id, started_at);
                    push_event(&event_queue, TerminalFrameEvent::ClearExceptInput(event));
                }
            }
            TerminalFrameCommand::TakeSession {
                session_id,
                response_tx,
            } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence session was moved to another workspace",
                ) {
                    push_event(&event_queue, TerminalFrameEvent::Search(stale));
                }
                snapshot_priority.remove(&session_id);
                if let Err(std::sync::mpsc::SendError(Some(session))) =
                    response_tx.send(sessions.remove(&session_id))
                {
                    sessions.insert(session_id, session);
                }
            }
            TerminalFrameCommand::InsertSessions {
                sessions: transferred,
            } => {
                for (session_id, mut session) in transferred {
                    session
                        .screen
                        .set_logical_line_snapshots(logical_line_snapshots);
                    session.include_live_snapshot = terminal_frame_live_snapshot_enabled(
                        !priority_initialized || snapshot_priority.contains(&session_id),
                    );
                    sessions.insert(session_id, session);
                }
            }
            TerminalFrameCommand::ResizeSession {
                session_id,
                cols,
                rows,
            } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence search was cancelled by terminal resize",
                ) {
                    push_event(&event_queue, TerminalFrameEvent::Search(stale));
                }
                if let Some(session) = sessions.get_mut(&session_id) {
                    let started_at = Instant::now();
                    session.resize(cols, rows);
                    let event = session.resized_live_snapshot_event(session_id, started_at);
                    push_event(&event_queue, TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::Output {
                session_id,
                data,
                encoding,
                scrollback_limit,
            } => {
                if rekeyed_old_ids.contains(&session_id) {
                    continue;
                }
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence search was cancelled by terminal output",
                ) {
                    push_event(&event_queue, TerminalFrameEvent::Search(stale));
                }
                let include_live_snapshot = terminal_frame_live_snapshot_enabled(
                    !priority_initialized || snapshot_priority.contains(&session_id),
                );
                let session = sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| TerminalFrameSession::new(&encoding, scrollback_limit));
                session
                    .screen
                    .set_logical_line_snapshots(logical_line_snapshots);
                session.include_live_snapshot = include_live_snapshot;
                process_terminal_frame_output_burst(
                    &command_rx,
                    &mut pending_commands,
                    &mut sessions,
                    &recording_writer,
                    session_id,
                    data,
                    encoding,
                    scrollback_limit,
                    |event| {
                        push_event(&event_queue, TerminalFrameEvent::Output(event));
                    },
                );
            }
            TerminalFrameCommand::RequestSnapshot {
                session_id,
                offset,
                source_history,
                action_links_enabled,
                action_link_matchers,
                priority,
                purpose,
            } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    if offset > 0
                        && source_history.is_some_and(|history| {
                            session
                                .screen
                                .history_state()
                                .changes_since(history)
                                .is_none()
                        })
                    {
                        continue;
                    }
                    let offset = source_history.map_or(offset, |history| {
                        session
                            .screen
                            .history_state()
                            .reanchor_offset(history, offset)
                    });
                    let event = session.snapshot_event(
                        session_id,
                        offset,
                        action_links_enabled,
                        action_link_matchers,
                        priority,
                        purpose,
                    );
                    push_event(&event_queue, TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::RequestSearch {
                session_id,
                purpose,
                key,
            } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    if purpose == TerminalFrameSearchPurpose::Find {
                        find_search_jobs.clear();
                        find_search_jobs.push_back(FindSearchJob::new(session_id, key, session));
                    } else if purpose == TerminalFrameSearchPurpose::SelectedOccurrence {
                        let job = SelectedOccurrenceSearchJob::new(session_id, key, session);
                        if let Some(stale) = replace_selected_occurrence_search_job(
                            &mut selected_occurrence_search_jobs,
                            job,
                        ) {
                            push_event(&event_queue, TerminalFrameEvent::Search(stale));
                        }
                    } else {
                        if matches!(purpose, TerminalFrameSearchPurpose::FindVisible { .. }) {
                            find_search_jobs
                                .retain(|job| job.session_id == session_id && job.key == key);
                        }
                        let event = session.search_event(session_id, purpose, key);
                        push_event(&event_queue, TerminalFrameEvent::Search(event));
                    }
                }
            }
            TerminalFrameCommand::RequestSelectionText {
                session_id,
                request,
                response_tx,
            } => {
                if !response_tx.is_canceled() {
                    let TerminalSelectionTextRequest { selection, history } = *request;
                    let text = sessions
                        .get(&session_id)
                        .ok_or(TerminalSelectionTextError::MissingSession)
                        .and_then(|session| {
                            let current = session.screen.history_state();
                            let selection = if selection.all_buffer {
                                selection
                            } else {
                                current
                                    .changes_since(history)
                                    .ok_or(TerminalSelectionTextError::StaleHistory)?;
                                selection
                                    .reanchored_history(history, current)
                                    .ok_or(TerminalSelectionTextError::InvalidRange)?
                            };
                            let text = session
                                .screen
                                .text_for_selection(selection)
                                .map_err(|_| TerminalSelectionTextError::InvalidRange)?;
                            Ok(TerminalSelectionTextResult {
                                text,
                                selection,
                                history: current,
                            })
                        });
                    let _ = response_tx.send(text);
                }
            }
            TerminalFrameCommand::RequestAllText {
                session_id,
                response_tx,
            } => {
                if !response_tx.is_canceled() {
                    let text = sessions
                        .get(&session_id)
                        .and_then(|session| session.screen.all_text());
                    let _ = response_tx.send(text);
                }
            }
            TerminalFrameCommand::CancelFindSearch { session_id } => {
                cancel_find_search_job_for_session(&mut find_search_jobs, &session_id);
            }
            TerminalFrameCommand::SetSnapshotPriority { session_ids } => {
                priority_initialized = true;
                snapshot_priority.clear();
                snapshot_priority.extend(session_ids);
                for (session_id, session) in sessions.iter_mut() {
                    session.include_live_snapshot = terminal_frame_live_snapshot_enabled(
                        snapshot_priority.contains(session_id),
                    );
                }
            }
            TerminalFrameCommand::Fence { complete_tx } => {
                let _ = complete_tx.send(());
            }
        }
    }
}

pub(super) fn try_next_terminal_frame_command(
    command_rx: &TerminalFrameCommandReceiver,
    pending_commands: &mut VecDeque<TerminalFrameCommand>,
) -> Option<TerminalFrameCommand> {
    pending_commands
        .pop_front()
        .or_else(|| command_rx.try_recv())
}

#[cfg(test)]
pub(super) fn next_terminal_frame_command(
    command_rx: &TerminalFrameCommandReceiver,
    pending_commands: &mut VecDeque<TerminalFrameCommand>,
) -> Option<TerminalFrameCommand> {
    pending_commands.pop_front().or_else(|| command_rx.recv())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_terminal_frame_output_burst(
    command_rx: &TerminalFrameCommandReceiver,
    pending_commands: &mut VecDeque<TerminalFrameCommand>,
    sessions: &mut HashMap<String, TerminalFrameSession>,
    recording_writer: &RecordingWriteHandle,
    session_id: String,
    data: Vec<u8>,
    encoding: String,
    scrollback_limit: usize,
    mut emit: impl FnMut(TerminalFrameOutputEvent),
) {
    let started_at = Instant::now();
    let mut batch = TerminalFrameOutputBatch::default();
    let mut processed_bytes = data.len();
    let session = sessions
        .entry(session_id.clone())
        .or_insert_with(|| TerminalFrameSession::new(&encoding, scrollback_limit));
    batch.absorb(session.process_output_chunk(
        &session_id,
        &data,
        &encoding,
        scrollback_limit,
        recording_writer,
    ));

    let mut trailing_data = Vec::new();
    loop {
        if processed_bytes >= TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT {
            break;
        }
        // Coalesce only what has already arrived. Blocking here to see whether
        // more output shows up would buy larger batches at the cost of holding
        // finished bytes off the screen — the wait landed on the keystroke-echo
        // path, and at the tail of a flood it delayed the last chunk too.
        let next = pending_commands
            .pop_front()
            .or_else(|| command_rx.try_recv());
        let Some(next) = next else {
            break;
        };
        match next {
            TerminalFrameCommand::Output {
                session_id: next_session_id,
                data: next_data,
                encoding: next_encoding,
                scrollback_limit: next_scrollback_limit,
            } if terminal_frame_output_commands_can_merge(
                TerminalFrameOutputShape {
                    session_id: &session_id,
                    encoding: &encoding,
                    scrollback_limit,
                    bytes: processed_bytes,
                },
                TerminalFrameOutputShape {
                    session_id: &next_session_id,
                    encoding: &next_encoding,
                    scrollback_limit: next_scrollback_limit,
                    bytes: next_data.len(),
                },
                TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT,
            ) =>
            {
                processed_bytes = processed_bytes.saturating_add(next_data.len());
                trailing_data.extend(next_data);
            }
            other => {
                pending_commands.push_front(other);
                break;
            }
        }
    }

    if !trailing_data.is_empty() {
        batch.absorb(session.process_output_chunk(
            &session_id,
            &trailing_data,
            &encoding,
            scrollback_limit,
            recording_writer,
        ));
    }
    emit(session.output_event_from_batch(session_id, batch, started_at));
}

#[cfg(test)]
pub(super) fn coalesce_terminal_frame_output_command(
    command_rx: &TerminalFrameCommandReceiver,
    pending_commands: &mut VecDeque<TerminalFrameCommand>,
    session_id: String,
    mut data: Vec<u8>,
    encoding: String,
    scrollback_limit: usize,
    byte_limit: usize,
) -> (String, Vec<u8>, String, usize) {
    loop {
        let next = pending_commands
            .pop_front()
            .or_else(|| command_rx.try_recv());
        let Some(next) = next else {
            break;
        };
        match next {
            TerminalFrameCommand::Output {
                session_id: next_session_id,
                data: next_data,
                encoding: next_encoding,
                scrollback_limit: next_scrollback_limit,
            } if terminal_frame_output_commands_can_merge(
                TerminalFrameOutputShape {
                    session_id: &session_id,
                    encoding: &encoding,
                    scrollback_limit,
                    bytes: data.len(),
                },
                TerminalFrameOutputShape {
                    session_id: &next_session_id,
                    encoding: &next_encoding,
                    scrollback_limit: next_scrollback_limit,
                    bytes: next_data.len(),
                },
                byte_limit,
            ) =>
            {
                data.extend(next_data);
            }
            other => {
                pending_commands.push_front(other);
                break;
            }
        }
    }

    (session_id, data, encoding, scrollback_limit)
}

#[derive(Clone, Copy)]
pub(super) struct TerminalFrameOutputShape<'a> {
    pub(super) session_id: &'a str,
    pub(super) encoding: &'a str,
    pub(super) scrollback_limit: usize,
    pub(super) bytes: usize,
}

pub(super) fn terminal_frame_output_commands_can_merge(
    current: TerminalFrameOutputShape<'_>,
    next: TerminalFrameOutputShape<'_>,
    byte_limit: usize,
) -> bool {
    current.session_id == next.session_id
        && current.encoding == next.encoding
        && current.scrollback_limit == next.scrollback_limit
        && current.bytes.saturating_add(next.bytes) <= byte_limit
}

#[cfg(test)]
pub(super) const TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT: usize = 32 * 1024;

/// Parse at most one bulk output frame before materializing a snapshot.
pub(super) const TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT: usize = 128 * 1024;
