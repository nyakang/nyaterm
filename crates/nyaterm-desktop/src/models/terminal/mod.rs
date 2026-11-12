//! Application terminal state and frame-worker facade.
pub(crate) mod command_blocks;

use crate::terminal::terminal_screen_from_output;
use nyaterm_core::{TerminalBackendResize, terminal_backend_resize_changed};
use nyaterm_terminal::{
    TerminalEffects, TerminalLineId, TerminalOutputDecoder, TerminalScreen, TerminalSnapshot,
};
use nyaterm_terminal_gpui::render_cache::TerminalRenderCache;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

mod frame_commands;
mod frame_events;
mod frame_pipeline;
mod frame_search;
mod frame_worker;

pub(crate) use frame_commands::TerminalSelectionTextResult;
pub(crate) use frame_events::{
    TerminalFrameEvent, TerminalFrameOutputEvent, TerminalFrameSearchEvent,
    TerminalFrameSnapshotEvent, TerminalFrameSnapshotPurpose,
};
pub(crate) use frame_pipeline::{TerminalFrameOutputSubmission, TerminalFramePipeline};
pub(crate) use frame_search::{
    TerminalFrameSearchKey, TerminalFrameSearchPurpose, TerminalFrameSearchResult,
    terminal_frame_search_result_is_current,
};
pub(crate) use frame_worker::TerminalFrameSession;

pub(crate) use nyaterm_terminal::action_links::{
    TerminalFrameActionLinks, prepare_terminal_frame_action_links_reusing,
    terminal_action_link_matcher_key,
};

mod selection;
#[cfg(test)]
mod selection_text_tests;
pub(crate) use selection::{
    TerminalBufferCellPos, TerminalCellPos, TerminalSelection, TerminalSelectionTextError,
};
mod presentation;
pub(crate) use presentation::{
    TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP, TERMINAL_OUTPUT_VISIBLE_BURST_OVERLOAD,
    TERMINAL_PERFORMANCE_RECOVERY_NOTICE, TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM,
    TERMINAL_UI_OUTPUT_TAIL_CAP, TerminalPerformanceMode, TerminalPerformanceOverlay,
    TerminalPresentation, TerminalWorkPolicy,
};

const TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP: usize = 16 * 1024;
const TERMINAL_SCROLLBACK_SNAPSHOT_CACHE_LIMIT: usize = 16;

pub(crate) fn terminal_geometry_diagnostics_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("NYATERM_TERMINAL_GEOMETRY_DIAGNOSTICS").is_some_and(|value| value == "1")
    })
}

pub(crate) fn terminal_expensive_interactions_enabled(
    action_links_enabled: bool,
    is_active: bool,
    render_degraded: bool,
    runtime_output_pressure: bool,
    output_burst_bytes: usize,
    performance_mode: TerminalPerformanceMode,
) -> bool {
    action_links_enabled
        && is_active
        && !render_degraded
        && !runtime_output_pressure
        && output_burst_bytes == 0
        && performance_mode != TerminalPerformanceMode::Overloaded
}

/// Unified paint policy for terminal surfaces (single decision point for
/// decorations and action links under pressure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectiveTerminalPaintPolicy {
    pub enhanced_decorations: bool,
    pub expensive_interactions: bool,
}

impl EffectiveTerminalPaintPolicy {
    pub(crate) fn resolve(
        presentation: TerminalPresentation,
        render_degraded: bool,
        runtime_output_pressure: bool,
        output_burst_bytes: usize,
        performance_mode: TerminalPerformanceMode,
        action_links_enabled: bool,
    ) -> Self {
        let work_policy = TerminalWorkPolicy::for_presentation(presentation);
        let enhanced_decorations = work_policy.active_decorations && !render_degraded;
        let expensive_interactions = terminal_expensive_interactions_enabled(
            action_links_enabled,
            work_policy.active_decorations,
            render_degraded,
            runtime_output_pressure,
            output_burst_bytes,
            performance_mode,
        );
        Self {
            enhanced_decorations,
            expensive_interactions,
        }
    }
}

pub(crate) use nyaterm_terminal::protocol::TerminalProtocolState;

pub(crate) fn protect_terminal_output_burst<'a>(
    screen: &mut TerminalScreen,
    output_decoder: &mut TerminalOutputDecoder,
    data: &'a [u8],
) -> (&'a [u8], usize) {
    if data.len() <= TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP {
        return (data, 0);
    }
    let skip = data.len() - TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP;
    screen.reset_stream_state();
    output_decoder.reset_decoder();
    (&data[skip..], skip)
}

fn trim_string_to_tail(output: &mut String, max_bytes: usize) {
    if max_bytes == 0 {
        output.clear();
        return;
    }
    if output.len() <= max_bytes {
        return;
    }
    let min_start = output.len() - max_bytes;
    let drain_to = output
        .char_indices()
        .find_map(|(index, _)| (index >= min_start).then_some(index))
        .unwrap_or(output.len());
    output.drain(..drain_to);
}

pub(crate) fn append_terminal_ui_output_tail(output: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    output.push_str(text);
    trim_string_to_tail(output, TERMINAL_UI_OUTPUT_TAIL_CAP);
}

fn append_terminal_frame_visible_tail(output: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    output.push_str(text);
    trim_string_to_tail(output, TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP);
}

fn merge_terminal_effects(target: &mut TerminalEffects, mut incoming: TerminalEffects) {
    if incoming.title.is_some() {
        target.title = incoming.title.take();
    }
    target.reset_title |= incoming.reset_title;
    target.bell |= incoming.bell;
    if incoming.cwd.is_some() {
        target.cwd = incoming.cwd.take();
    }
    target.shell_command_started |= incoming.shell_command_started;
    target.shell_command_finished |= incoming.shell_command_finished;
    target.pty_write.append(&mut incoming.pty_write);
    if incoming.clipboard_store.is_some() {
        target.clipboard_store = incoming.clipboard_store.take();
    }
    target.clipboard_loads.append(&mut incoming.clipboard_loads);
}

pub(crate) struct TerminalViewState {
    pub(crate) command_blocks: command_blocks::TerminalCommandBlockState,
    /// Bounded tail of decoded session output, control sequences included.
    /// Kept current for every session whatever its visibility or render mode:
    /// AI context, MCP recent output, and CLI prompt detection read it, and
    /// none of them can wait on the frame worker's grid.
    pub(crate) output: String,
    pub(crate) screen: TerminalScreen,
    pub(crate) max_line_number_digits: usize,
    /// Latest live viewport prepared by the background terminal frame processor.
    pub(crate) frame_snapshot: Option<Arc<TerminalSnapshot>>,
    /// Anchor baseline while a hidden session releases its heavy frame snapshot.
    detached_history: Option<nyaterm_terminal::TerminalHistoryState>,
    /// Immutable input colors paired with frame_snapshot, independent of the
    /// input-idle output batch scheduler. Cleared whenever that snapshot changes.
    pub(crate) frame_action_links: Option<TerminalFrameActionLinks>,
    pub(crate) scrollback_snapshots: HashMap<usize, Arc<TerminalSnapshot>>,
    pub(crate) scrollback_action_links: HashMap<usize, TerminalFrameActionLinks>,
    pub(crate) pending_snapshot_offsets: HashSet<usize>,
    pub(crate) priority_pending_snapshot_offsets: HashSet<usize>,
    pub(crate) search_result: Option<TerminalFrameSearchResult>,
    pub(crate) search_visible_result: Option<TerminalFrameSearchResult>,
    pub(crate) pending_search_key: Option<TerminalFrameSearchKey>,
    pub(crate) pending_search_revision: u64,
    pub(crate) pending_search_at: Option<Instant>,
    pub(crate) pending_search_visible: Option<(TerminalFrameSearchKey, usize, usize, u64, Instant)>,
    pub(crate) search_visible_range: Option<(usize, usize)>,
    pub(crate) last_screen_change_at: Instant,
    pub(crate) selected_occurrence_result: Option<TerminalFrameSearchResult>,
    pub(crate) pending_selected_occurrence_key: Option<TerminalFrameSearchKey>,
    pub(crate) selected_occurrence_visible_result: Option<TerminalFrameSearchResult>,
    pub(crate) pending_selected_occurrence_visible_key: Option<TerminalFrameSearchKey>,
    pub(crate) protocol_state: TerminalProtocolState,
    /// Physical keys routed to this connection; never carried across reconnect.
    pub(crate) pressed_keys: HashSet<String>,
    /// Tab actions omit the platform repeat flag, including locally consumed presses.
    pub(crate) tab_action_pressed: bool,
    pub(crate) output_decoder: TerminalOutputDecoder,
    pub(crate) recording_decoder: TerminalOutputDecoder,
    pub(crate) screen_revision: u64,
    /// True while the frame worker is rebuilding content for a new grid size.
    pub(crate) grid_resize_pending: bool,
    pub(crate) render_cache: TerminalRenderCache,
    pub(crate) has_unread: bool,
    /// Viewport offset from the live bottom (0 = follow output).
    pub(crate) scroll_offset: usize,
    /// True when output arrived while scrolled into history (FAB "New" affordance).
    pub(crate) has_new_while_scrolled: bool,
    pub(crate) performance_mode: TerminalPerformanceMode,
    pub(crate) performance_overlay: Option<TerminalPerformanceOverlay>,
    /// Deadline for the recovered banner's auto-dismiss (`None` = nothing to dismiss).
    pub(crate) performance_overlay_until: Option<Instant>,
    /// Characters dropped while protecting responsiveness (Tauri skippedOutputChars).
    pub(crate) skipped_output_chars: u64,
    /// Bytes accepted in the current calm window (reset each event-pump tick).
    pub(crate) output_burst_bytes: usize,
    /// True while expensive render decorations are temporarily skipped.
    pub(crate) render_degraded: bool,
    /// Start of the current uninterrupted calm window (`None` = not calm yet).
    pub(crate) render_degraded_calm_since: Option<Instant>,
    /// Last size sent to the PTY/backend for this session.
    pub(crate) last_backend_resize: Option<TerminalBackendResize>,
    /// Stable logical row selected by an ordinary left click.
    pub(crate) target_line: Option<TerminalLineId>,
}

pub(crate) struct TerminalFrameParts<'a> {
    pub visible_text: &'a str,
    pub snapshot: Arc<TerminalSnapshot>,
    pub action_links: Option<TerminalFrameActionLinks>,
    pub protocol_state: TerminalProtocolState,
    pub accepted_bytes: usize,
    pub skipped_output_bytes: usize,
    pub revision: u64,
}

impl TerminalViewState {
    pub(crate) fn new() -> Self {
        Self {
            command_blocks: Default::default(),
            output: String::new(),
            screen: TerminalScreen::default(),
            max_line_number_digits: 1,
            frame_snapshot: None,
            detached_history: None,
            frame_action_links: None,
            scrollback_snapshots: HashMap::new(),
            scrollback_action_links: HashMap::new(),
            pending_snapshot_offsets: HashSet::new(),
            priority_pending_snapshot_offsets: HashSet::new(),
            search_result: None,
            search_visible_result: None,
            pending_search_key: None,
            pending_search_revision: 0,
            pending_search_at: None,
            pending_search_visible: None,
            search_visible_range: None,
            last_screen_change_at: Instant::now(),
            selected_occurrence_result: None,
            pending_selected_occurrence_key: None,
            selected_occurrence_visible_result: None,
            pending_selected_occurrence_visible_key: None,
            protocol_state: TerminalProtocolState::default(),
            pressed_keys: HashSet::new(),
            tab_action_pressed: false,
            output_decoder: TerminalOutputDecoder::default(),
            recording_decoder: TerminalOutputDecoder::default(),
            screen_revision: 0,
            grid_resize_pending: false,
            render_cache: TerminalRenderCache::default(),
            has_unread: false,
            scroll_offset: 0,
            has_new_while_scrolled: false,
            performance_mode: TerminalPerformanceMode::Normal,
            performance_overlay: None,
            performance_overlay_until: None,
            skipped_output_chars: 0,
            output_burst_bytes: 0,
            render_degraded: true,
            render_degraded_calm_since: None,
            last_backend_resize: None,
            target_line: None,
        }
    }

    pub(crate) fn complete_snapshot_request(&mut self, offset: usize) {
        self.pending_snapshot_offsets.remove(&offset);
        self.priority_pending_snapshot_offsets.remove(&offset);
    }

    pub(crate) fn from_output(output: String) -> Self {
        let screen = terminal_screen_from_output(&output);
        let protocol_state = TerminalProtocolState::from_screen(&screen);
        Self {
            command_blocks: Default::default(),
            output,
            screen,
            max_line_number_digits: 1,
            frame_snapshot: None,
            detached_history: None,
            frame_action_links: None,
            scrollback_snapshots: HashMap::new(),
            scrollback_action_links: HashMap::new(),
            pending_snapshot_offsets: HashSet::new(),
            priority_pending_snapshot_offsets: HashSet::new(),
            search_result: None,
            search_visible_result: None,
            pending_search_key: None,
            pending_search_revision: 0,
            pending_search_at: None,
            pending_search_visible: None,
            search_visible_range: None,
            last_screen_change_at: Instant::now(),
            selected_occurrence_result: None,
            pending_selected_occurrence_key: None,
            selected_occurrence_visible_result: None,
            pending_selected_occurrence_visible_key: None,
            protocol_state,
            pressed_keys: HashSet::new(),
            tab_action_pressed: false,
            output_decoder: TerminalOutputDecoder::default(),
            recording_decoder: TerminalOutputDecoder::default(),
            screen_revision: 0,
            grid_resize_pending: false,
            render_cache: TerminalRenderCache::default(),
            has_unread: false,
            scroll_offset: 0,
            has_new_while_scrolled: false,
            performance_mode: TerminalPerformanceMode::Normal,
            performance_overlay: None,
            performance_overlay_until: None,
            skipped_output_chars: 0,
            output_burst_bytes: 0,
            render_degraded: true,
            render_degraded_calm_since: None,
            last_backend_resize: None,
            target_line: None,
        }
    }

    pub(crate) fn history_state(&self) -> nyaterm_terminal::TerminalHistoryState {
        self.frame_snapshot
            .as_ref()
            .map(|snapshot| snapshot.history)
            .or(self.detached_history)
            .unwrap_or_else(|| self.screen.history_state())
    }

    pub(crate) fn live_snapshot_with_scroll_window(&self) -> Arc<TerminalSnapshot> {
        terminal_frame_snapshot_with_scroll_window(&self.screen, 0, false)
    }

    #[cfg(test)]
    pub(crate) fn snapshot_with_scroll_window(&self, offset: usize) -> Arc<TerminalSnapshot> {
        terminal_frame_snapshot_with_scroll_window(
            &self.screen,
            offset.min(self.screen.scrollback_len()),
            true,
        )
    }

    fn rekey_scrollback_query_caches_after_output(
        &mut self,
        current: nyaterm_terminal::TerminalHistoryState,
    ) {
        let mut links = std::mem::take(&mut self.scrollback_action_links);
        self.scrollback_snapshots = self
            .scrollback_snapshots
            .drain()
            .filter_map(|(offset, snapshot)| {
                let change = current.changes_since(snapshot.history)?;
                let rebased = snapshot.reanchored_history(current)?;
                let next_offset = current.reanchor_offset(snapshot.history, offset);
                if let Some(mut decorations) = links.remove(&offset)
                    && let (Some(start), Some(end)) = (
                        decorations.absolute_start_row.checked_sub(change.evicted),
                        decorations.absolute_end_row.checked_sub(change.evicted),
                    )
                {
                    decorations.absolute_start_row = start;
                    decorations.absolute_end_row = end;
                    self.scrollback_action_links
                        .insert(next_offset, decorations);
                }
                Some((next_offset, Arc::new(rebased)))
            })
            .collect();
        self.pending_snapshot_offsets.clear();
        self.priority_pending_snapshot_offsets.clear();
    }

    fn anchor_scrollback_after_history_change(
        &mut self,
        previous: nyaterm_terminal::TerminalHistoryState,
    ) {
        let current = self.history_state();
        let scrolled = current.scrolled_since(previous);
        if current.changes_since(previous).is_none() {
            self.clear_scrollback_query_caches();
        } else if scrolled.is_some_and(|rows| rows > 0) {
            if self.scroll_offset > 0 {
                self.rekey_scrollback_query_caches_after_output(current);
            } else {
                self.clear_scrollback_query_caches();
            }
        }
        self.scroll_offset = current.reanchor_offset(previous, self.scroll_offset);
        if self.scroll_offset > 0 && scrolled.is_some_and(|rows| rows > 0) {
            self.has_new_while_scrolled = true;
        }
        self.clamp_scroll_offset();
    }

    pub(crate) fn from_output_with_encoding(output: String, encoding: &str) -> Self {
        let mut view = Self::from_output(output);
        view.set_encoding(encoding);
        view
    }

    pub(crate) fn set_encoding(&mut self, encoding: &str) {
        if let Err(error) = self.screen.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        if let Err(error) = self.output_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
        if let Err(error) = self.recording_decoder.set_encoding(encoding) {
            tracing::warn!(%error, "terminal encoding configuration rejected");
        }
    }

    pub(crate) fn reset_reconnect_stream(&mut self, encoding: &str) {
        self.screen.reset_stream_state();
        self.screen.reset_session_input_protocols();
        // The UI screen mirrors geometry; only the worker knows the active
        // screen. Keep that presentation bit while resetting input protocols.
        self.protocol_state = TerminalProtocolState {
            alternate_screen: self.protocol_state.alternate_screen,
            alternate_scroll: self.screen.alternate_scroll(),
            ..TerminalProtocolState::default()
        };
        self.pressed_keys.clear();
        self.tab_action_pressed = false;
        self.output_decoder.reset_decoder();
        self.recording_decoder.reset_decoder();
        self.set_encoding(encoding);
        self.pending_snapshot_offsets.clear();
        self.priority_pending_snapshot_offsets.clear();
        self.clear_terminal_query_caches();
        self.render_cache.clear();
    }

    #[cfg(test)]
    pub(crate) fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let previous_history = self.history_state();
        self.screen.advance_decoded_text(text);
        self.screen_revision = self.screen_revision.saturating_add(1);
        self.last_screen_change_at = Instant::now();
        self.detached_history = None;
        self.frame_snapshot = Some(self.live_snapshot_with_scroll_window());
        self.grid_resize_pending = false;
        self.frame_action_links = None;
        self.enter_render_degraded_mode();
        self.protocol_state = TerminalProtocolState::from_screen(&self.screen);
        append_terminal_ui_output_tail(&mut self.output, text);
        self.anchor_scrollback_after_history_change(previous_history);
    }

    /// Feed already-protected bytes into the view (used when the caller applies
    /// the same feed to the mirrored active screen).
    #[cfg(test)]
    pub(crate) fn append_bytes_unprotected(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let previous_history = self.history_state();
        self.screen.advance(data);
        self.screen_revision = self.screen_revision.saturating_add(1);
        self.last_screen_change_at = Instant::now();
        self.detached_history = None;
        self.frame_snapshot = Some(self.live_snapshot_with_scroll_window());
        self.grid_resize_pending = false;
        self.frame_action_links = None;
        self.enter_render_degraded_mode();
        self.protocol_state = TerminalProtocolState::from_screen(&self.screen);
        append_terminal_ui_output_tail(
            &mut self.output,
            &self.output_decoder.decode_output_text(data),
        );
        self.anchor_scrollback_after_history_change(previous_history);
    }

    /// Drop the oldest part of an oversized burst so the latest screen state wins
    /// (Tauri backlog trim + large-output protection).
    #[cfg(test)]
    pub(crate) fn protect_output_burst<'a>(&mut self, data: &'a [u8]) -> &'a [u8] {
        if data.is_empty() {
            return data;
        }
        let (feed, skip) =
            protect_terminal_output_burst(&mut self.screen, &mut self.output_decoder, data);
        if skip > 0 {
            self.note_skipped_output(skip);
        }
        self.output_burst_bytes = self.output_burst_bytes.saturating_add(feed.len());
        if self.output_burst_bytes > TERMINAL_OUTPUT_VISIBLE_BURST_OVERLOAD
            || feed.len() > 32 * 1024
        {
            self.enter_overloaded_mode();
        } else if !feed.is_empty() {
            self.enter_render_degraded_mode();
        }
        feed
    }

    pub(crate) fn note_skipped_output(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.skipped_output_chars = self.skipped_output_chars.saturating_add(count as u64);
        self.enter_overloaded_mode();
    }

    pub(crate) fn note_output_discontinuity(&mut self, count: usize) {
        self.note_skipped_output(count);
        self.screen.reset_stream_state();
        self.output_decoder.reset_decoder();
        self.recording_decoder.reset_decoder();
    }

    pub(crate) fn enter_overloaded_mode(&mut self) {
        self.performance_mode = TerminalPerformanceMode::Overloaded;
        self.performance_overlay = Some(TerminalPerformanceOverlay::Overloaded);
        self.performance_overlay_until = None;
        self.enter_render_degraded_mode();
    }

    pub(crate) fn maybe_exit_overloaded_mode(&mut self, now: Instant) {
        if self.performance_mode != TerminalPerformanceMode::Overloaded {
            return;
        }
        // Calm window: no large burst this tick.
        if self.output_burst_bytes > TERMINAL_OUTPUT_VISIBLE_BURST_OVERLOAD / 4 {
            return;
        }
        self.performance_mode = TerminalPerformanceMode::Normal;
        self.performance_overlay = Some(TerminalPerformanceOverlay::Recovered);
        self.performance_overlay_until = Some(now + TERMINAL_PERFORMANCE_RECOVERY_NOTICE);
    }

    pub(crate) fn enter_render_degraded_mode(&mut self) {
        self.render_degraded = true;
        self.render_degraded_calm_since = None;
    }

    fn tick_render_degradation(&mut self, output_pressure: bool, now: Instant) {
        if output_pressure || self.output_burst_bytes > 0 {
            self.enter_render_degraded_mode();
            return;
        }
        if !self.render_degraded {
            return;
        }
        // The first calm observation opens the window rather than closing it: output
        // between two ticks is accounted for by `output_burst_bytes` above, so calm
        // can only be claimed for a span this actually observed.
        let calm_since = *self.render_degraded_calm_since.get_or_insert(now);
        if now.saturating_duration_since(calm_since) >= TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM {
            self.render_degraded = false;
            self.render_degraded_calm_since = None;
        }
    }

    pub(crate) fn tick_performance_overlay(&mut self, output_pressure: bool, now: Instant) {
        // End-of-tick calm accounting for recovery.
        self.maybe_exit_overloaded_mode(now);
        self.tick_render_degradation(output_pressure, now);
        self.output_burst_bytes = 0;
        if self
            .performance_overlay_until
            .is_some_and(|until| now >= until)
        {
            self.performance_overlay_until = None;
            if self.performance_overlay == Some(TerminalPerformanceOverlay::Recovered) {
                self.performance_overlay = None;
            }
        }
    }

    pub(crate) fn clear_presentation_except_input(
        &mut self,
        revision: u64,
        snapshot: &TerminalSnapshot,
    ) {
        self.output.clear();
        for row in snapshot
            .rows()
            .iter()
            .filter(|row| !row.text.trim().is_empty())
        {
            if !self.output.is_empty() && !row.wrapped {
                self.output.push('\n');
            }
            append_terminal_ui_output_tail(&mut self.output, &row.text);
        }
        self.frame_snapshot = None;
        self.frame_action_links = None;
        self.clear_terminal_query_caches();
        self.render_cache.clear();
        // A local clear restarts gutter numbering at 1 and drops scrollback, so
        // the high-water digit count retained from before the clear no longer
        // applies. Re-observe from the post-clear snapshot.
        self.max_line_number_digits = 1;
        self.observe_line_number_digits(snapshot);
        self.screen_revision = revision;
        self.last_screen_change_at = Instant::now();
        self.has_unread = false;
        self.scroll_offset = 0;
        self.has_new_while_scrolled = false;
    }

    pub(crate) fn clamp_scroll_offset(&mut self) {
        let max = self.scrollback_len_for_ui();
        if self.scroll_offset > max {
            self.scroll_offset = max;
        }
    }

    pub(crate) fn apply_terminal_frame_parts(&mut self, parts: TerminalFrameParts<'_>) {
        let TerminalFrameParts {
            visible_text,
            snapshot,
            action_links,
            protocol_state,
            accepted_bytes,
            skipped_output_bytes,
            revision,
        } = parts;
        append_terminal_ui_output_tail(&mut self.output, visible_text);
        let previous_history = self.history_state();
        self.observe_line_number_digits(&snapshot);
        if !snapshot.history.alternate_screen {
            self.command_blocks.retain(&snapshot.command_blocks);
        }
        let preserved_action_links = action_links.or_else(|| {
            self.frame_action_links
                .take()
                .filter(|links| links.has_matching_decorated_snapshot_rows(snapshot.as_ref()))
        });
        self.detached_history = None;
        self.frame_snapshot = Some(snapshot);
        self.frame_action_links = preserved_action_links;
        self.protocol_state = protocol_state;
        self.screen_revision = revision;
        self.last_screen_change_at = Instant::now();
        self.grid_resize_pending = false;
        self.output_burst_bytes = self.output_burst_bytes.saturating_add(accepted_bytes);
        if accepted_bytes > 0 {
            self.enter_render_degraded_mode();
        }
        if skipped_output_bytes > 0 {
            self.note_skipped_output(skipped_output_bytes);
        }
        self.anchor_scrollback_after_history_change(previous_history);
    }

    pub(crate) fn apply_terminal_live_snapshot_frame(
        &mut self,
        snapshot: Arc<TerminalSnapshot>,
        action_links: Option<TerminalFrameActionLinks>,
        revision: u64,
    ) {
        let previous_history = self.history_state();
        self.observe_line_number_digits(&snapshot);
        if !snapshot.history.alternate_screen {
            self.command_blocks.retain(&snapshot.command_blocks);
        }
        let preserved_action_links = action_links.or_else(|| {
            self.frame_action_links
                .take()
                .filter(|links| links.has_matching_decorated_snapshot_rows(snapshot.as_ref()))
        });
        self.detached_history = None;
        self.frame_snapshot = Some(snapshot);
        self.frame_action_links = preserved_action_links;
        self.grid_resize_pending = false;
        if revision > self.screen_revision {
            self.screen_revision = revision;
            self.last_screen_change_at = Instant::now();
        }
        self.anchor_scrollback_after_history_change(previous_history);
    }

    pub(crate) fn apply_terminal_background_frame_parts(
        &mut self,
        visible_text: &str,
        snapshot: Option<Arc<TerminalSnapshot>>,
        action_links: Option<TerminalFrameActionLinks>,
        protocol_state: TerminalProtocolState,
        skipped_output_bytes: usize,
        revision: u64,
    ) {
        // Paint work can wait for visibility; the output tail cannot, because
        // AI context and MCP read it for sessions that are not on screen.
        append_terminal_ui_output_tail(&mut self.output, visible_text);
        let previous_history = self.history_state();
        let has_snapshot = snapshot.is_some();
        // Hidden sessions keep protocol/revision current without retaining a full
        // viewport snapshot until the surface becomes visible again.
        if let Some(snapshot) = snapshot {
            self.observe_line_number_digits(&snapshot);
            if !snapshot.history.alternate_screen {
                self.command_blocks.retain(&snapshot.command_blocks);
            }
            self.detached_history = None;
            self.frame_snapshot = Some(snapshot);
            self.frame_action_links = action_links;
            self.grid_resize_pending = false;
        } else {
            // Drop heavy paint state while backgrounded under pressure.
            self.detached_history = Some(previous_history);
            self.frame_snapshot = None;
            self.clear_scrollback_query_caches();
            self.frame_action_links = None;
        }
        self.protocol_state = protocol_state;
        self.screen_revision = revision;
        self.last_screen_change_at = Instant::now();
        if skipped_output_bytes > 0 {
            self.note_skipped_output(skipped_output_bytes);
        }
        if has_snapshot {
            self.anchor_scrollback_after_history_change(previous_history);
        }
    }

    pub(crate) fn backend_resize_changed(
        &self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> bool {
        terminal_backend_resize_changed(
            self.last_backend_resize,
            TerminalBackendResize::new(cols, rows, pixel_width, pixel_height),
        )
    }

    pub(crate) fn remember_backend_resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) {
        self.last_backend_resize = Some(TerminalBackendResize::new(
            cols,
            rows,
            pixel_width,
            pixel_height,
        ));
    }

    pub(crate) fn clear_scrollback_query_caches(&mut self) {
        self.scrollback_snapshots.clear();
        self.scrollback_action_links.clear();
        self.pending_snapshot_offsets.clear();
        self.priority_pending_snapshot_offsets.clear();
    }

    pub(crate) fn remember_scrollback_snapshot(
        &mut self,
        offset: usize,
        snapshot: Arc<TerminalSnapshot>,
    ) {
        if !self
            .history_state()
            .preserves_coordinates_from(snapshot.history)
        {
            return;
        }
        self.observe_line_number_digits(&snapshot);
        if offset == 0 && !snapshot.history.alternate_screen {
            self.command_blocks.retain(&snapshot.command_blocks);
        }
        if offset == 0 {
            self.detached_history = None;
            self.frame_snapshot = Some(snapshot);
            return;
        }
        self.scrollback_snapshots.insert(offset, snapshot);
        self.prune_scrollback_snapshot_cache(offset);
    }

    fn observe_line_number_digits(&mut self, snapshot: &TerminalSnapshot) {
        self.max_line_number_digits = self
            .max_line_number_digits
            .max(snapshot.total_rows.max(1).to_string().len());
    }

    pub(crate) fn prune_scrollback_snapshot_cache(&mut self, keep_offset: usize) {
        while self.scrollback_snapshots.len() > TERMINAL_SCROLLBACK_SNAPSHOT_CACHE_LIMIT {
            let Some(drop_offset) = self
                .scrollback_snapshots
                .keys()
                .copied()
                .filter(|offset| *offset != keep_offset)
                .max_by_key(|offset| {
                    (
                        offset.abs_diff(keep_offset),
                        // Prefer dropping the older/farther side on ties. Newer
                        // offsets are more likely to be reached while returning
                        // to live output.
                        *offset > keep_offset,
                    )
                })
            else {
                break;
            };
            self.scrollback_snapshots.remove(&drop_offset);
            self.scrollback_action_links.remove(&drop_offset);
        }
    }

    fn clear_terminal_query_caches(&mut self) {
        self.clear_scrollback_query_caches();
        self.search_result = None;
        self.search_visible_result = None;
        self.pending_search_key = None;
        self.pending_search_at = None;
        self.pending_search_visible = None;
        self.search_visible_range = None;
        self.selected_occurrence_result = None;
        self.pending_selected_occurrence_key = None;
        self.selected_occurrence_visible_result = None;
        self.pending_selected_occurrence_visible_key = None;
    }

    pub(crate) fn scrollback_len_for_ui(&self) -> usize {
        self.history_state().scrollback_len
    }

    pub(crate) fn viewport_rows_for_ui(&self) -> usize {
        if self.grid_resize_pending {
            return self.screen.rows().max(1);
        }
        self.frame_snapshot
            .as_ref()
            .map_or_else(|| self.screen.rows(), |snapshot| snapshot.viewport_rows)
            .max(1)
    }

    pub(crate) fn total_rows_for_ui(&self) -> usize {
        self.frame_snapshot
            .as_ref()
            .map(|snapshot| snapshot.total_rows.max(1))
            .unwrap_or_else(|| self.screen.total_rows().max(1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeywordHighlightEditorField {
    Name,
    Patterns,
    ColorDark,
    ColorLight,
}

impl KeywordHighlightEditorField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Name => Self::Patterns,
            Self::Patterns => Self::ColorDark,
            Self::ColorDark => Self::ColorLight,
            Self::ColorLight => Self::Name,
        }
    }

    pub(crate) fn input_key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Patterns => "patterns",
            Self::ColorDark => "color-dark",
            Self::ColorLight => "color-light",
        }
    }

    pub(crate) fn from_input_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(Self::Name),
            "patterns" => Some(Self::Patterns),
            "color-dark" => Some(Self::ColorDark),
            "color-light" => Some(Self::ColorLight),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;

pub(crate) use nyaterm_terminal::snapshot_window::{
    terminal_frame_live_snapshot_with_stats, terminal_frame_scroll_window_extra_rows,
    terminal_frame_snapshot_with_scroll_window,
    terminal_frame_snapshot_with_scroll_window_and_stats, terminal_snapshot_matches_grid_geometry,
};
