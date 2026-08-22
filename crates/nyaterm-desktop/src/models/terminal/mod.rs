use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nyaterm_core::{
    ActionLinksMatcherSettings, TerminalBackendResize, terminal_backend_resize_changed,
};
use nyaterm_terminal::{
    TerminalEffects, TerminalLineId, TerminalOutputDecoder, TerminalScreen,
    TerminalSearchDirection, TerminalSearchQuery, TerminalSnapshot, TerminalSnapshotBuildStats,
    terminal_cell_col_for_byte_index, terminal_cell_count,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    action_links::{ActionLinkMatch, find_action_links},
    terminal::{
        NyaTerminalLayoutCache, TerminalBufferMatch, TerminalLineDecorations,
        terminal_screen_from_output,
    },
};

use super::RecordingWriteHandle;

mod selection;
pub(crate) use selection::{TerminalBufferCellPos, TerminalCellPos, TerminalSelection};

/// Large-output protection modes (Tauri XTerminal performanceMode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TerminalPerformanceMode {
    #[default]
    Normal,
    Overloaded,
}

/// In-pane large-output protection banner (Tauri PerformanceOverlayState).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalPerformanceOverlay {
    Overloaded,
    Recovered,
}

pub(crate) const TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP: usize = 1_000_000;
pub(crate) const TERMINAL_OUTPUT_VISIBLE_BURST_OVERLOAD: usize = 256 * 1024;
/// UI-only text mirror cap. The authoritative terminal screen/scrollback lives
/// in the frame worker; the GPUI thread keeps only a recent tail for prompts,
/// AI context snippets, reconnect seed text, and compact tab actions.
pub(crate) const TERMINAL_UI_OUTPUT_TAIL_CAP: usize = 128 * 1024;
const TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP: usize = 16 * 1024;
const TERMINAL_FRAME_SCROLL_WINDOW_MIN_EXTRA_ROWS: usize = 32;
const TERMINAL_FRAME_SCROLL_WINDOW_MAX_EXTRA_ROWS: usize = 192;
const TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MIN_EXTRA_ROWS: usize = 64;
const TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MAX_EXTRA_ROWS: usize = 256;
const TERMINAL_SCROLLBACK_SNAPSHOT_CACHE_LIMIT: usize = 16;
/// How long the "recovered" notice stays up after leaving overloaded mode.
///
/// Was `TERMINAL_PERFORMANCE_RECOVERY_TICKS: u8 = 60`, documented as "~3s recovery
/// notice at the 50ms event-pump cadence" -- but the pump has three cadences, so 60
/// ticks was 0.96s under pressure and **30s on the 500ms quiet interval**, which is
/// exactly the state an app falls into once a flood ends and the user is left
/// looking at the notice.
pub(crate) const TERMINAL_PERFORMANCE_RECOVERY_NOTICE: Duration = Duration::from_secs(3);
/// How long output must stay calm before expensive render decorations come back.
///
/// Was 8 ticks, which is 0.13s under pressure and 4s on the quiet interval.
pub(crate) const TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Default)]
pub(crate) struct TerminalFrameActionLinks {
    pub(crate) matcher_key: u64,
    pub(crate) absolute_start_row: usize,
    pub(crate) absolute_end_row: usize,
    pub(crate) row_signatures: Vec<u64>,
    pub(crate) matches_by_line: Vec<Vec<ActionLinkMatch>>,
    pub(crate) cell_ranges_by_line: Vec<Vec<(usize, usize)>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TerminalActionLinkBuildStats {
    pub(crate) reused_rows: usize,
    pub(crate) rebuilt_rows: usize,
}

impl TerminalFrameActionLinks {
    pub(crate) fn source_index_for_snapshot_row(
        &self,
        snapshot: &TerminalSnapshot,
        line_index: usize,
    ) -> Option<usize> {
        let row = snapshot.row(line_index)?;
        let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
        let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());
        let absolute_row = absolute_start_row.checked_add(line_index)?;
        if absolute_row >= absolute_end_row
            || absolute_row < self.absolute_start_row
            || absolute_row >= self.absolute_end_row
        {
            return None;
        }
        let source_index = absolute_row - self.absolute_start_row;
        if self.row_signatures.get(source_index).copied()? != row.signature {
            return None;
        }
        Some(source_index)
    }

    pub(crate) fn overlaps_snapshot(&self, snapshot: &TerminalSnapshot) -> bool {
        let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
        let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());
        self.absolute_start_row < absolute_end_row && absolute_start_row < self.absolute_end_row
    }

    pub(crate) fn covers_all_snapshot_rows(&self, snapshot: &TerminalSnapshot) -> bool {
        (0..snapshot.row_count()).all(|line_index| {
            self.source_index_for_snapshot_row(snapshot, line_index)
                .is_some()
        })
    }

    pub(crate) fn has_matching_decorated_snapshot_rows(&self, snapshot: &TerminalSnapshot) -> bool {
        (0..snapshot.row_count()).any(|line_index| {
            self.source_index_for_snapshot_row(snapshot, line_index)
                .is_some_and(|source_index| {
                    self.matches_by_line
                        .get(source_index)
                        .is_some_and(|matches| !matches.is_empty())
                        || self
                            .cell_ranges_by_line
                            .get(source_index)
                            .is_some_and(|ranges| !ranges.is_empty())
                })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TerminalFrameSearchKey {
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) regex: bool,
    pub(crate) whole_word: bool,
    pub(crate) limit: usize,
    pub(crate) request_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TerminalFrameSearchPurpose {
    Find,
    SelectedOccurrenceVisible {
        absolute_start: usize,
        absolute_end: usize,
    },
    SelectedOccurrence,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameSearchResult {
    pub(crate) key: TerminalFrameSearchKey,
    pub(crate) revision: u64,
    pub(crate) matches: Result<Arc<[TerminalBufferMatch]>, String>,
    pub(crate) position_fingerprint: u64,
}

impl TerminalFrameSearchResult {
    pub(crate) fn new(
        key: TerminalFrameSearchKey,
        revision: u64,
        matches: Result<Vec<TerminalBufferMatch>, String>,
    ) -> Self {
        let position_fingerprint = matches
            .as_deref()
            .map(terminal_buffer_matches_position_fingerprint)
            .unwrap_or_default();
        Self {
            key,
            revision,
            matches: matches.map(Arc::from),
            position_fingerprint,
        }
    }
}

pub(crate) fn terminal_buffer_matches_position_fingerprint(matches: &[TerminalBufferMatch]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    matches.len().hash(&mut hasher);
    for search_match in matches {
        search_match.line_index.hash(&mut hasher);
        search_match.start_col.hash(&mut hasher);
        search_match.end_col.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn terminal_frame_search_result_is_current(
    result: &TerminalFrameSearchResult,
    key: &TerminalFrameSearchKey,
    revision: u64,
) -> bool {
    result.key == *key && result.revision == revision
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
        is_active: bool,
        render_degraded: bool,
        runtime_output_pressure: bool,
        output_burst_bytes: usize,
        performance_mode: TerminalPerformanceMode,
        action_links_enabled: bool,
    ) -> Self {
        let enhanced_decorations = !render_degraded;
        let expensive_interactions = terminal_expensive_interactions_enabled(
            action_links_enabled,
            is_active,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalProtocolState {
    pub(crate) focus_reporting: bool,
    pub(crate) bracketed_paste: bool,
    pub(crate) mouse_reporting: bool,
    pub(crate) mouse_sgr: bool,
    pub(crate) mouse_drag_reporting: bool,
    pub(crate) mouse_motion_reporting: bool,
    pub(crate) application_cursor_keys: bool,
    pub(crate) application_keypad: bool,
    pub(crate) kitty_keyboard_disambiguate: bool,
    pub(crate) kitty_keyboard_report_event_types: bool,
    pub(crate) kitty_keyboard_report_alternate_keys: bool,
    pub(crate) kitty_keyboard_report_all_keys_as_esc: bool,
    pub(crate) kitty_keyboard_report_associated_text: bool,
    pub(crate) alternate_scroll: bool,
    pub(crate) alternate_screen: bool,
}

impl TerminalProtocolState {
    pub(crate) fn from_screen(screen: &TerminalScreen) -> Self {
        Self {
            focus_reporting: screen.focus_reporting(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_reporting: screen.mouse_reporting(),
            mouse_sgr: screen.mouse_sgr(),
            mouse_drag_reporting: screen.mouse_drag_reporting(),
            mouse_motion_reporting: screen.mouse_motion_reporting(),
            application_cursor_keys: screen.application_cursor_keys(),
            application_keypad: screen.application_keypad(),
            kitty_keyboard_disambiguate: screen.kitty_keyboard_disambiguate(),
            kitty_keyboard_report_event_types: screen.kitty_keyboard_report_event_types(),
            kitty_keyboard_report_alternate_keys: screen.kitty_keyboard_report_alternate_keys(),
            kitty_keyboard_report_all_keys_as_esc: screen.kitty_keyboard_report_all_keys_as_esc(),
            kitty_keyboard_report_associated_text: screen.kitty_keyboard_report_associated_text(),
            alternate_scroll: screen.alternate_scroll(),
            alternate_screen: screen.alternate_screen(),
        }
    }

    pub(crate) fn alternate_scroll_payload(self, delta_lines: i32) -> Option<Vec<u8>> {
        if delta_lines == 0
            || !self.alternate_screen
            || !self.alternate_scroll
            || self.mouse_reporting
        {
            return None;
        }
        let up = delta_lines > 0;
        let unit = nyaterm_terminal::alternate_scroll_key_bytes(up, self.application_cursor_keys);
        let steps = delta_lines.unsigned_abs().min(8) as usize;
        let mut payload = Vec::with_capacity(unit.len() * steps);
        for _ in 0..steps {
            payload.extend_from_slice(&unit);
        }
        Some(payload)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode_mouse_report(
        self,
        button: u8,
        col: u16,
        row: u16,
        press: bool,
        motion: bool,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Vec<u8> {
        if !self.mouse_reporting {
            return Vec::new();
        }
        let x = col.saturating_add(1);
        let y = row.saturating_add(1);
        let mut code = if press || self.mouse_sgr { button } else { 3 };
        if motion {
            code = code.saturating_add(32);
        }
        if shift {
            code = code.saturating_add(4);
        }
        if alt {
            code = code.saturating_add(8);
        }
        if ctrl {
            code = code.saturating_add(16);
        }
        if self.mouse_sgr {
            let suffix = if press { 'M' } else { 'm' };
            format!("\x1b[<{code};{x};{y}{suffix}").into_bytes()
        } else {
            let cb = 32u16.saturating_add(u16::from(code)).min(255) as u8;
            let cx = 32u16.saturating_add(x).min(255) as u8;
            let cy = 32u16.saturating_add(y).min(255) as u8;
            vec![0x1b, b'[', b'M', cb, cx, cy]
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TerminalRenderCache {
    pub(crate) layout_cache: Arc<Mutex<NyaTerminalLayoutCache>>,
    decoration_cache: Arc<Mutex<TerminalDecorationCache>>,
}

#[derive(Debug, Default)]
struct TerminalDecorationCache {
    decoration_lines: HashMap<u64, Arc<[TerminalLineDecorations]>>,
    hits: u64,
    misses: u64,
}

const TERMINAL_DECORATION_CACHE_CAP: usize = 4096;

impl TerminalRenderCache {
    pub(crate) fn clear(&mut self) {
        if let Ok(mut cache) = self.layout_cache.lock() {
            cache.clear();
        }
        if let Ok(mut cache) = self.decoration_cache.lock() {
            cache.clear();
        }
    }

    pub(crate) fn line_decorations(
        &self,
        key: u64,
        build: impl FnOnce() -> Vec<TerminalLineDecorations>,
    ) -> Arc<[TerminalLineDecorations]> {
        let Ok(mut cache) = self.decoration_cache.lock() else {
            return build().into();
        };
        cache.line_decorations(key, build)
    }

    pub(crate) fn decoration_stats(&self) -> (u64, u64) {
        self.decoration_cache
            .lock()
            .map(|cache| (cache.hits, cache.misses))
            .unwrap_or((0, 0))
    }
}

impl TerminalDecorationCache {
    fn clear(&mut self) {
        self.decoration_lines.clear();
        self.hits = 0;
        self.misses = 0;
    }

    fn line_decorations(
        &mut self,
        key: u64,
        build: impl FnOnce() -> Vec<TerminalLineDecorations>,
    ) -> Arc<[TerminalLineDecorations]> {
        if let Some(decorations) = self.decoration_lines.get(&key) {
            self.hits = self.hits.saturating_add(1);
            return Arc::clone(decorations);
        }
        self.misses = self.misses.saturating_add(1);
        if self.decoration_lines.len() >= TERMINAL_DECORATION_CACHE_CAP {
            self.decoration_lines.clear();
        }
        let decorations: Arc<[TerminalLineDecorations]> = build().into();
        self.decoration_lines.insert(key, Arc::clone(&decorations));
        decorations
    }
}

pub(crate) fn terminal_action_link_matcher_key(
    enabled: bool,
    matchers: &ActionLinksMatcherSettings,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    enabled.hash(&mut hasher);
    matchers.ipv4.hash(&mut hasher);
    matchers.archive.hash(&mut hasher);
    matchers.host_port.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn prepare_terminal_frame_action_links(
    snapshot: &TerminalSnapshot,
    enabled: bool,
    matchers: &ActionLinksMatcherSettings,
) -> Option<TerminalFrameActionLinks> {
    prepare_terminal_frame_action_links_reusing(snapshot, enabled, matchers, None).0
}

pub(crate) fn prepare_terminal_frame_action_links_reusing(
    snapshot: &TerminalSnapshot,
    enabled: bool,
    matchers: &ActionLinksMatcherSettings,
    previous: Option<&TerminalFrameActionLinks>,
) -> (
    Option<TerminalFrameActionLinks>,
    TerminalActionLinkBuildStats,
) {
    let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());
    let row_signatures = snapshot
        .rows()
        .iter()
        .map(|row| row.signature)
        .collect::<Vec<_>>();
    if !enabled {
        return (
            Some(TerminalFrameActionLinks {
                matcher_key: terminal_action_link_matcher_key(false, matchers),
                absolute_start_row,
                absolute_end_row,
                row_signatures,
                matches_by_line: vec![Vec::new(); snapshot.row_count()],
                cell_ranges_by_line: vec![Vec::new(); snapshot.row_count()],
            }),
            TerminalActionLinkBuildStats::default(),
        );
    }
    let matcher_key = terminal_action_link_matcher_key(true, matchers);
    let reusable = previous.filter(|links| links.matcher_key == matcher_key);
    let mut matches_by_line = Vec::with_capacity(snapshot.row_count());
    let mut cell_ranges_by_line = Vec::with_capacity(snapshot.row_count());
    let mut stats = TerminalActionLinkBuildStats::default();
    for (line_index, row) in snapshot.rows().iter().enumerate() {
        let absolute_row = absolute_start_row.saturating_add(line_index);
        let reused = reusable.and_then(|links| {
            if absolute_row < links.absolute_start_row || absolute_row >= links.absolute_end_row {
                return None;
            }
            let source_index = absolute_row - links.absolute_start_row;
            if links.row_signatures.get(source_index) != Some(&row.signature) {
                return None;
            }
            Some((
                links.matches_by_line.get(source_index)?.clone(),
                links.cell_ranges_by_line.get(source_index)?.clone(),
            ))
        });
        if let Some((matches, cell_ranges)) = reused {
            stats.reused_rows += 1;
            matches_by_line.push(matches);
            cell_ranges_by_line.push(cell_ranges);
            continue;
        }
        stats.rebuilt_rows += 1;
        let matches = if row.text.is_empty() {
            Vec::new()
        } else {
            find_action_links(&row.text, matchers, true)
        };
        let cell_ranges = matches
            .iter()
            .map(|item| {
                (
                    terminal_cell_col_for_byte_index(&row.text, item.start),
                    terminal_cell_col_for_byte_index(&row.text, item.end),
                )
            })
            .collect();
        matches_by_line.push(matches);
        cell_ranges_by_line.push(cell_ranges);
    }
    (
        Some(TerminalFrameActionLinks {
            matcher_key,
            absolute_start_row,
            absolute_end_row,
            row_signatures,
            matches_by_line,
            cell_ranges_by_line,
        }),
        stats,
    )
}

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
    pub(crate) output: String,
    pub(crate) screen: TerminalScreen,
    /// Latest live viewport prepared by the background terminal frame processor.
    pub(crate) frame_snapshot: Option<Arc<TerminalSnapshot>>,
    pub(crate) frame_action_links: Option<TerminalFrameActionLinks>,
    pub(crate) scrollback_snapshots: HashMap<usize, Arc<TerminalSnapshot>>,
    pub(crate) scrollback_action_links: HashMap<usize, TerminalFrameActionLinks>,
    pub(crate) pending_snapshot_offsets: HashSet<usize>,
    pub(crate) priority_pending_snapshot_offsets: HashSet<usize>,
    pub(crate) search_result: Option<TerminalFrameSearchResult>,
    pub(crate) pending_search_key: Option<TerminalFrameSearchKey>,
    pub(crate) selected_occurrence_result: Option<TerminalFrameSearchResult>,
    pub(crate) pending_selected_occurrence_key: Option<TerminalFrameSearchKey>,
    pub(crate) selected_occurrence_visible_result: Option<TerminalFrameSearchResult>,
    pub(crate) pending_selected_occurrence_visible_key: Option<TerminalFrameSearchKey>,
    pub(crate) protocol_state: TerminalProtocolState,
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
            output: String::new(),
            screen: TerminalScreen::default(),
            frame_snapshot: None,
            frame_action_links: None,
            scrollback_snapshots: HashMap::new(),
            scrollback_action_links: HashMap::new(),
            pending_snapshot_offsets: HashSet::new(),
            priority_pending_snapshot_offsets: HashSet::new(),
            search_result: None,
            pending_search_key: None,
            selected_occurrence_result: None,
            pending_selected_occurrence_key: None,
            selected_occurrence_visible_result: None,
            pending_selected_occurrence_visible_key: None,
            protocol_state: TerminalProtocolState::default(),
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

    pub(crate) fn from_output(output: String) -> Self {
        let screen = terminal_screen_from_output(&output);
        let protocol_state = TerminalProtocolState::from_screen(&screen);
        Self {
            output,
            screen,
            frame_snapshot: None,
            frame_action_links: None,
            scrollback_snapshots: HashMap::new(),
            scrollback_action_links: HashMap::new(),
            pending_snapshot_offsets: HashSet::new(),
            priority_pending_snapshot_offsets: HashSet::new(),
            search_result: None,
            pending_search_key: None,
            selected_occurrence_result: None,
            pending_selected_occurrence_key: None,
            selected_occurrence_visible_result: None,
            pending_selected_occurrence_visible_key: None,
            protocol_state,
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

    fn scrollback_len_for_anchor(&self) -> usize {
        self.frame_snapshot
            .as_ref()
            .map(|snapshot| snapshot.scrollback_len)
            .unwrap_or_else(|| self.screen.scrollback_len())
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

    fn clone_snapshot_with_scrollback_delta(
        snapshot: &Arc<TerminalSnapshot>,
        delta: usize,
    ) -> Arc<TerminalSnapshot> {
        let mut snapshot = (**snapshot).clone();
        snapshot.scrollback_len = snapshot.scrollback_len.saturating_add(delta);
        snapshot.total_rows = snapshot.total_rows.saturating_add(delta);
        snapshot.display_offset = snapshot.display_offset.saturating_add(delta);
        Arc::new(snapshot)
    }

    fn rekey_scrollback_query_caches_for_growth(&mut self, delta: usize) {
        if delta == 0 {
            return;
        }
        self.scrollback_snapshots = self
            .scrollback_snapshots
            .drain()
            .map(|(offset, snapshot)| {
                (
                    offset.saturating_add(delta),
                    Self::clone_snapshot_with_scrollback_delta(&snapshot, delta),
                )
            })
            .collect();
        self.scrollback_action_links = self
            .scrollback_action_links
            .drain()
            .map(|(offset, links)| (offset.saturating_add(delta), links))
            .collect();
        // In-flight worker requests carry the old literal offset. Once output
        // grows scrollback, the anchored target needs a new offset request.
        self.pending_snapshot_offsets.clear();
        self.priority_pending_snapshot_offsets.clear();
    }

    fn anchor_scrollback_after_len_change(&mut self, old_len: usize, new_len: usize) {
        if new_len < old_len {
            self.clear_scrollback_query_caches();
            self.clamp_scroll_offset();
            return;
        }
        let delta = new_len.saturating_sub(old_len);
        if self.scroll_offset == 0 {
            if delta > 0 {
                self.clear_scrollback_query_caches();
            }
            return;
        }
        if delta > 0 {
            self.scroll_offset = self.scroll_offset.saturating_add(delta).min(new_len);
            self.rekey_scrollback_query_caches_for_growth(delta);
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
        self.screen.set_encoding(encoding);
        self.output_decoder.set_encoding(encoding);
        self.recording_decoder.set_encoding(encoding);
    }

    pub(crate) fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let old_scrollback_len = self.scrollback_len_for_anchor();
        self.screen.advance_decoded_text(text);
        self.screen_revision = self.screen_revision.saturating_add(1);
        self.frame_snapshot = Some(self.live_snapshot_with_scroll_window());
        self.grid_resize_pending = false;
        self.frame_action_links = None;
        self.enter_render_degraded_mode();
        self.protocol_state = TerminalProtocolState::from_screen(&self.screen);
        append_terminal_ui_output_tail(&mut self.output, text);
        self.anchor_scrollback_after_len_change(
            old_scrollback_len,
            self.scrollback_len_for_anchor(),
        );
    }

    /// Feed already-protected bytes into the view (used when the caller applies
    /// the same feed to the mirrored active screen).
    #[cfg(test)]
    pub(crate) fn append_bytes_unprotected(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let old_scrollback_len = self.scrollback_len_for_anchor();
        self.screen.advance(data);
        self.screen_revision = self.screen_revision.saturating_add(1);
        self.frame_snapshot = Some(self.live_snapshot_with_scroll_window());
        self.grid_resize_pending = false;
        self.frame_action_links = None;
        self.enter_render_degraded_mode();
        self.protocol_state = TerminalProtocolState::from_screen(&self.screen);
        append_terminal_ui_output_tail(
            &mut self.output,
            &self.output_decoder.decode_output_text(data),
        );
        self.anchor_scrollback_after_len_change(
            old_scrollback_len,
            self.scrollback_len_for_anchor(),
        );
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

    pub(crate) fn clear(&mut self) {
        self.output.clear();
        self.screen.clear();
        self.frame_snapshot = None;
        self.frame_action_links = None;
        self.clear_terminal_query_caches();
        self.protocol_state = TerminalProtocolState::default();
        self.output_decoder.reset_decoder();
        self.recording_decoder.reset_decoder();
        self.screen_revision = self.screen_revision.saturating_add(1);
        self.grid_resize_pending = false;
        self.render_cache.clear();
        self.has_unread = false;
        self.scroll_offset = 0;
        self.has_new_while_scrolled = false;
        self.performance_mode = TerminalPerformanceMode::Normal;
        self.performance_overlay = None;
        self.performance_overlay_until = None;
        self.skipped_output_chars = 0;
        self.output_burst_bytes = 0;
        self.render_degraded = true;
        self.render_degraded_calm_since = None;
    }

    pub(crate) fn clamp_scroll_offset(&mut self) {
        let max = self
            .frame_snapshot
            .as_ref()
            .map(|snapshot| snapshot.scrollback_len)
            .unwrap_or_else(|| self.screen.scrollback_len());
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
        // Keep the bounded UI output tail even while paint is degraded. The tail
        // is used as reconnect seed/copy input, and append_terminal_ui_output_tail
        // trims it to a fixed size, so retaining it does not grow without bound.
        if !visible_text.is_empty() {
            append_terminal_ui_output_tail(&mut self.output, visible_text);
        }
        let old_scrollback_len = self.scrollback_len_for_anchor();
        let new_scrollback_len = snapshot.scrollback_len;
        let preserved_action_links = action_links.or_else(|| {
            self.frame_action_links
                .take()
                .filter(|links| links.has_matching_decorated_snapshot_rows(snapshot.as_ref()))
        });
        self.frame_snapshot = Some(snapshot);
        self.frame_action_links = preserved_action_links;
        self.protocol_state = protocol_state;
        self.screen_revision = revision;
        self.grid_resize_pending = false;
        self.output_burst_bytes = self.output_burst_bytes.saturating_add(accepted_bytes);
        if accepted_bytes > 0 {
            self.enter_render_degraded_mode();
        }
        if skipped_output_bytes > 0 {
            self.note_skipped_output(skipped_output_bytes);
        }
        self.anchor_scrollback_after_len_change(old_scrollback_len, new_scrollback_len);
    }

    pub(crate) fn apply_terminal_live_snapshot_frame(
        &mut self,
        snapshot: Arc<TerminalSnapshot>,
        action_links: Option<TerminalFrameActionLinks>,
        revision: u64,
    ) {
        let old_scrollback_len = self.scrollback_len_for_anchor();
        let new_scrollback_len = snapshot.scrollback_len;
        let preserved_action_links = action_links.or_else(|| {
            self.frame_action_links
                .take()
                .filter(|links| links.has_matching_decorated_snapshot_rows(snapshot.as_ref()))
        });
        self.frame_snapshot = Some(snapshot);
        self.frame_action_links = preserved_action_links;
        self.grid_resize_pending = false;
        if revision > self.screen_revision {
            self.screen_revision = revision;
        }
        self.anchor_scrollback_after_len_change(old_scrollback_len, new_scrollback_len);
    }

    pub(crate) fn apply_terminal_background_frame_parts(
        &mut self,
        snapshot: Option<Arc<TerminalSnapshot>>,
        action_links: Option<TerminalFrameActionLinks>,
        visible_text: &str,
        protocol_state: TerminalProtocolState,
        skipped_output_bytes: usize,
        revision: u64,
    ) {
        // Hidden sessions keep protocol/revision current without retaining a full
        // viewport snapshot until the surface becomes visible again. Keep the
        // bounded text tail so reconnects do not lose a banner that arrived before
        // the replacement terminal surface was attached.
        append_terminal_ui_output_tail(&mut self.output, visible_text);
        if let Some(snapshot) = snapshot {
            self.frame_snapshot = Some(snapshot);
            self.frame_action_links = action_links;
            self.grid_resize_pending = false;
        } else {
            // Drop heavy paint state while backgrounded under pressure.
            self.frame_snapshot = None;
            self.frame_action_links = None;
        }
        self.protocol_state = protocol_state;
        self.screen_revision = revision;
        if skipped_output_bytes > 0 {
            self.note_skipped_output(skipped_output_bytes);
        }
        self.clamp_scroll_offset();
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
        if offset == 0 {
            self.frame_snapshot = Some(snapshot);
            return;
        }
        self.scrollback_snapshots.insert(offset, snapshot);
        self.prune_scrollback_snapshot_cache(offset);
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
        self.pending_search_key = None;
        self.selected_occurrence_result = None;
        self.pending_selected_occurrence_key = None;
        self.selected_occurrence_visible_result = None;
        self.pending_selected_occurrence_visible_key = None;
    }

    pub(crate) fn scrollback_len_for_ui(&self) -> usize {
        self.frame_snapshot
            .as_ref()
            .map(|snapshot| snapshot.scrollback_len)
            .unwrap_or_else(|| self.screen.scrollback_len())
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

#[derive(Clone, Debug)]
pub(crate) struct TerminalFramePipeline {
    command_tx: TerminalFrameCommandSender,
    event_queue: TerminalFrameEventQueue,
    event_wake_rx: Arc<Mutex<Option<UnboundedReceiver<()>>>>,
}

pub(crate) struct TerminalFrameOutputSubmission {
    pub(crate) session_id: String,
    pub(crate) data: Vec<u8>,
    pub(crate) encoding: String,
    pub(crate) scrollback_limit: usize,
}

/// Hand a submission to the frame processor as one command.
///
/// Splitting it here would only be undone downstream — `push_terminal_frame_command`
/// re-merges adjacent same-session output on enqueue, and the worker coalesces
/// again up to [`TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT`] — while costing a copy
/// of the payload plus two `String` clones per slice.
fn terminal_frame_output_commands(
    output: TerminalFrameOutputSubmission,
) -> Option<TerminalFrameCommand> {
    if output.data.is_empty() {
        return None;
    }
    Some(TerminalFrameCommand::Output {
        session_id: output.session_id,
        data: output.data,
        encoding: output.encoding,
        scrollback_limit: output.scrollback_limit,
    })
}

impl TerminalFramePipeline {
    pub(crate) fn spawn(recording_writer: RecordingWriteHandle) -> Self {
        let (command_tx, command_rx) = terminal_frame_command_channel();
        let (event_queue, event_wake_rx) =
            TerminalFrameEventQueue::new_with_wake(TERMINAL_FRAME_EVENT_QUEUE_CAP);
        let event_queue_for_worker = event_queue.clone();
        thread::Builder::new()
            .name("nyaterm-terminal-frame-processor".to_string())
            .spawn(move || {
                run_terminal_frame_processor(command_rx, event_queue_for_worker, recording_writer)
            })
            .expect("failed to spawn terminal frame processor");
        Self {
            command_tx,
            event_queue,
            event_wake_rx: Arc::new(Mutex::new(Some(event_wake_rx))),
        }
    }

    pub(crate) fn arm_output_event_wake(&self) {
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_OUTPUT);
    }

    /// Declare interest in every kind of frame event.
    ///
    /// The drain task calls this before each check, so a snapshot or search reply
    /// cannot be missed by a consumer that only armed for output. The individual
    /// arms at the request sites and the input-echo accelerator stay: `arm_wake`
    /// is a `fetch_or`, so they compose.
    pub(crate) fn arm_event_wakes(&self) {
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_ALL);
    }

    pub(crate) fn take_event_wake_receiver(&self) -> Option<UnboundedReceiver<()>> {
        self.event_wake_rx.lock().ok()?.take()
    }

    pub(crate) fn event_wake_count(&self) -> u64 {
        self.event_queue.wake_count()
    }

    pub(crate) fn ensure_session(
        &self,
        session_id: impl Into<String>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::EnsureSession {
            session_id: session_id.into(),
            encoding: encoding.into(),
            scrollback_limit,
        });
    }

    pub(crate) fn seed_session(
        &self,
        session_id: impl Into<String>,
        output: impl Into<String>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::SeedSession {
            session_id: session_id.into(),
            output: output.into(),
            encoding: encoding.into(),
            scrollback_limit,
        });
    }

    pub(crate) fn remove_session(&self, session_id: impl Into<String>) {
        let _ = self.command_tx.send(TerminalFrameCommand::RemoveSession {
            session_id: session_id.into(),
        });
    }

    pub(crate) fn resize_session(&self, session_id: impl Into<String>, cols: u16, rows: u16) {
        let _ = self.command_tx.send(TerminalFrameCommand::ResizeSession {
            session_id: session_id.into(),
            cols,
            rows,
        });
    }

    pub(crate) fn submit_output(
        &self,
        session_id: impl Into<String>,
        data: Vec<u8>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        if data.is_empty() {
            return;
        }
        // Wake the GPUI frame consumer as soon as the processor publishes the
        // resulting event; otherwise the first login burst waits for the
        // runtime tick instead of taking the existing event-wake path.
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_OUTPUT);
        let _ = self.command_tx.send_many(terminal_frame_output_commands(
            TerminalFrameOutputSubmission {
                session_id: session_id.into(),
                data,
                encoding: encoding.into(),
                scrollback_limit,
            },
        ));
    }

    pub(crate) fn submit_outputs(&self, outputs: Vec<TerminalFrameOutputSubmission>) {
        if outputs.is_empty() {
            return;
        }
        // Keep batched output on the same low-latency wake path as a single
        // submission. The session bridge normally uses this batch method.
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_OUTPUT);
        let commands = outputs.into_iter().flat_map(terminal_frame_output_commands);
        let _ = self.command_tx.send_many(commands);
    }

    pub(crate) fn request_snapshot(
        &self,
        session_id: impl Into<String>,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
    ) {
        self.request_snapshot_with_priority(
            session_id,
            offset,
            action_links_enabled,
            action_link_matchers,
            false,
            TerminalFrameSnapshotPurpose::Paint,
        );
    }

    pub(crate) fn request_action_link_enrichment(
        &self,
        session_id: impl Into<String>,
        action_link_matchers: ActionLinksMatcherSettings,
    ) {
        self.request_snapshot_with_priority(
            session_id,
            0,
            true,
            action_link_matchers,
            false,
            TerminalFrameSnapshotPurpose::ActionLinkEnrichment,
        );
    }

    pub(crate) fn request_priority_snapshot(
        &self,
        session_id: impl Into<String>,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
    ) {
        self.event_queue
            .arm_wake(TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT);
        self.request_snapshot_with_priority(
            session_id,
            offset,
            action_links_enabled,
            action_link_matchers,
            true,
            TerminalFrameSnapshotPurpose::Paint,
        );
    }

    fn request_snapshot_with_priority(
        &self,
        session_id: impl Into<String>,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
        priority: bool,
        purpose: TerminalFrameSnapshotPurpose,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::RequestSnapshot {
            session_id: session_id.into(),
            offset,
            action_links_enabled,
            action_link_matchers,
            priority,
            purpose,
        });
    }

    pub(crate) fn request_search(
        &self,
        session_id: impl Into<String>,
        purpose: TerminalFrameSearchPurpose,
        key: TerminalFrameSearchKey,
    ) {
        if key.query.trim().is_empty() || key.limit == 0 {
            return;
        }
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_SEARCH);
        let _ = self.command_tx.send(TerminalFrameCommand::RequestSearch {
            session_id: session_id.into(),
            purpose,
            key,
        });
    }

    pub(crate) fn set_snapshot_priority(&self, session_ids: Vec<String>) {
        let _ = self
            .command_tx
            .send(TerminalFrameCommand::SetSnapshotPriority { session_ids });
    }

    pub(crate) fn drain_events_into(
        &self,
        events: &mut VecDeque<TerminalFrameEvent>,
        limit: usize,
    ) -> usize {
        self.event_queue.drain_into(events, limit)
    }

    pub(crate) fn queued_event_count(&self) -> usize {
        self.event_queue.len()
    }

    pub(crate) fn queued_command_count(&self) -> usize {
        self.command_tx.len()
    }

    pub(crate) fn queued_output_bytes(&self) -> usize {
        self.command_tx.queued_output_bytes()
    }
}

impl Default for TerminalFramePipeline {
    fn default() -> Self {
        let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
        let recording_writer = super::RecordingWritePipeline::spawn(recording_manager).writer();
        Self::spawn(recording_writer)
    }
}

#[derive(Debug)]
enum TerminalFrameCommand {
    EnsureSession {
        session_id: String,
        encoding: String,
        scrollback_limit: usize,
    },
    SeedSession {
        session_id: String,
        output: String,
        encoding: String,
        scrollback_limit: usize,
    },
    RemoveSession {
        session_id: String,
    },
    ResizeSession {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    Output {
        session_id: String,
        data: Vec<u8>,
        encoding: String,
        scrollback_limit: usize,
    },
    RequestSnapshot {
        session_id: String,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
        priority: bool,
        purpose: TerminalFrameSnapshotPurpose,
    },
    RequestSearch {
        session_id: String,
        purpose: TerminalFrameSearchPurpose,
        key: TerminalFrameSearchKey,
    },
    /// Prefer building full viewport snapshots for these sessions (visible tabs).
    /// Empty list means no session is paint-priority (all background).
    SetSnapshotPriority {
        session_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalFrameSnapshotPurpose {
    Paint,
    ActionLinkEnrichment,
}

#[derive(Clone, Debug)]
pub(crate) enum TerminalFrameEvent {
    Output(TerminalFrameOutputEvent),
    Snapshot(TerminalFrameSnapshotEvent),
    Search(TerminalFrameSearchEvent),
}

#[derive(Clone, Debug)]
struct TerminalFrameEventQueue {
    inner: Arc<Mutex<VecDeque<TerminalFrameEvent>>>,
    cap: usize,
    wake_tx: Option<UnboundedSender<()>>,
    wake_interests: Arc<AtomicU8>,
    wake_count: Arc<AtomicU64>,
}

const TERMINAL_FRAME_EVENT_WAKE_OUTPUT: u8 = 1 << 0;
const TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT: u8 = 1 << 1;
const TERMINAL_FRAME_EVENT_WAKE_SEARCH: u8 = 1 << 2;
/// What the data-plane drain task declares interest in: every kind, because it
/// applies every kind. Narrowing this strands whichever reply is dropped from it.
const TERMINAL_FRAME_EVENT_WAKE_ALL: u8 = TERMINAL_FRAME_EVENT_WAKE_OUTPUT
    | TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT
    | TERMINAL_FRAME_EVENT_WAKE_SEARCH;

impl TerminalFrameEventQueue {
    #[cfg(test)]
    fn new(cap: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(1024)))),
            cap,
            wake_tx: None,
            wake_interests: Arc::new(AtomicU8::new(0)),
            wake_count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn new_with_wake(cap: usize) -> (Self, UnboundedReceiver<()>) {
        let (wake_tx, wake_rx) = unbounded();
        (
            Self {
                inner: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(1024)))),
                cap,
                wake_tx: Some(wake_tx),
                wake_interests: Arc::new(AtomicU8::new(0)),
                wake_count: Arc::new(AtomicU64::new(0)),
            },
            wake_rx,
        )
    }

    fn arm_wake(&self, interest: u8) {
        if self.wake_tx.is_some() && interest != 0 {
            self.wake_interests.fetch_or(interest, Ordering::Release);
        }
    }

    fn push(&self, mut event: TerminalFrameEvent) {
        let wake_interest = terminal_frame_event_wake_interest(&event);
        let Ok(mut queue) = self.inner.lock() else {
            return;
        };
        compact_terminal_frame_event_queue(&mut queue, &mut event);
        while queue.len() >= self.cap.max(1) {
            let drop_index = queue
                .iter()
                .position(terminal_frame_event_can_drop_under_pressure)
                .unwrap_or(0);
            queue.remove(drop_index);
        }
        queue.push_back(event);
        drop(queue);
        if wake_interest != 0
            && self
                .wake_interests
                .fetch_and(!wake_interest, Ordering::AcqRel)
                & wake_interest
                != 0
            && let Some(wake_tx) = &self.wake_tx
            && wake_tx.unbounded_send(()).is_ok()
        {
            self.wake_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    fn try_recv(&self) -> Option<TerminalFrameEvent> {
        self.inner.lock().ok()?.pop_front()
    }

    fn drain_into(&self, events: &mut VecDeque<TerminalFrameEvent>, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        let Ok(mut queue) = self.inner.lock() else {
            return 0;
        };
        let mut drained = 0usize;
        while drained < limit {
            let Some(event) = queue.pop_front() else {
                break;
            };
            events.push_back(event);
            drained += 1;
        }
        drained
    }

    fn len(&self) -> usize {
        self.inner.lock().map(|queue| queue.len()).unwrap_or(0)
    }

    fn wake_count(&self) -> u64 {
        self.wake_count.load(Ordering::Relaxed)
    }
}

fn terminal_frame_event_wake_interest(event: &TerminalFrameEvent) -> u8 {
    match event {
        TerminalFrameEvent::Output(_) => TERMINAL_FRAME_EVENT_WAKE_OUTPUT,
        TerminalFrameEvent::Snapshot(_) => TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT,
        TerminalFrameEvent::Search(_) => TERMINAL_FRAME_EVENT_WAKE_SEARCH,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameOutputEvent {
    pub(crate) session_id: String,
    pub(crate) visible_text: String,
    pub(crate) recording_text_bytes: usize,
    /// Full viewport grid for paint. Worker omits this for low-priority
    /// (hidden) sessions to avoid per-frame snapshot CPU/memory.
    pub(crate) snapshot: Option<Arc<TerminalSnapshot>>,
    pub(crate) action_links: Option<TerminalFrameActionLinks>,
    pub(crate) protocol_state: TerminalProtocolState,
    pub(crate) effects: TerminalEffects,
    pub(crate) command_running: bool,
    pub(crate) accepted_bytes: usize,
    pub(crate) skipped_output_bytes: usize,
    pub(crate) revision: u64,
    pub(crate) snapshot_duration: Duration,
    pub(crate) snapshot_stats: TerminalSnapshotBuildStats,
    pub(crate) process_duration: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameSnapshotEvent {
    pub(crate) session_id: String,
    pub(crate) offset: usize,
    pub(crate) snapshot: Arc<TerminalSnapshot>,
    pub(crate) action_links: Option<TerminalFrameActionLinks>,
    pub(crate) revision: u64,
    pub(crate) snapshot_duration: Duration,
    pub(crate) snapshot_stats: TerminalSnapshotBuildStats,
    pub(crate) action_link_stats: TerminalActionLinkBuildStats,
    pub(crate) process_duration: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameSearchEvent {
    pub(crate) session_id: String,
    pub(crate) purpose: TerminalFrameSearchPurpose,
    pub(crate) result: TerminalFrameSearchResult,
    pub(crate) process_duration: Duration,
}

pub(crate) fn terminal_frame_scroll_window_extra_rows(
    viewport_rows: usize,
    priority: bool,
) -> usize {
    let viewport_rows = viewport_rows.max(1);
    if priority {
        return viewport_rows.saturating_mul(3).clamp(
            TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MIN_EXTRA_ROWS,
            TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MAX_EXTRA_ROWS,
        );
    }
    viewport_rows.saturating_mul(2).clamp(
        TERMINAL_FRAME_SCROLL_WINDOW_MIN_EXTRA_ROWS,
        TERMINAL_FRAME_SCROLL_WINDOW_MAX_EXTRA_ROWS,
    )
}

fn terminal_frame_snapshot_with_scroll_window(
    screen: &TerminalScreen,
    offset: usize,
    priority: bool,
) -> Arc<TerminalSnapshot> {
    terminal_frame_snapshot_with_scroll_window_and_stats(screen, offset, priority).0
}

fn terminal_frame_snapshot_with_scroll_window_and_stats(
    screen: &TerminalScreen,
    offset: usize,
    priority: bool,
) -> (Arc<TerminalSnapshot>, Duration, TerminalSnapshotBuildStats) {
    let extra_rows = terminal_frame_scroll_window_extra_rows(screen.rows(), priority);
    terminal_frame_snapshot_with_extra_rows_and_stats(screen, offset, extra_rows)
}

fn terminal_frame_live_snapshot_with_stats(
    screen: &TerminalScreen,
) -> (Arc<TerminalSnapshot>, Duration, TerminalSnapshotBuildStats) {
    let started_at = Instant::now();
    let (snapshot, stats) = screen.viewport_snapshot_with_stats(0);
    (Arc::new(snapshot), started_at.elapsed(), stats)
}

fn terminal_frame_snapshot_with_extra_rows_and_stats(
    screen: &TerminalScreen,
    offset: usize,
    extra_rows: usize,
) -> (Arc<TerminalSnapshot>, Duration, TerminalSnapshotBuildStats) {
    let started_at = Instant::now();
    let (snapshot, stats) =
        screen.viewport_snapshot_with_window_and_stats(offset, extra_rows, extra_rows);
    (Arc::new(snapshot), started_at.elapsed(), stats)
}

pub(crate) fn terminal_snapshot_matches_grid_geometry(
    snapshot: &TerminalSnapshot,
    cols: usize,
    viewport_rows: usize,
) -> bool {
    snapshot.cols == cols.max(1) && snapshot.viewport_rows == viewport_rows.max(1)
}

fn compact_terminal_frame_event_queue(
    queue: &mut VecDeque<TerminalFrameEvent>,
    incoming: &mut TerminalFrameEvent,
) {
    let TerminalFrameEvent::Output(incoming) = incoming else {
        return;
    };
    if !terminal_frame_output_event_can_drop_under_pressure(incoming) {
        return;
    }
    let mut replaced = Vec::new();
    let mut index = 0usize;
    while index < queue.len() {
        let replace = matches!(queue.get(index), Some(TerminalFrameEvent::Output(queued))
            if queued.session_id == incoming.session_id
                && terminal_frame_output_event_can_drop_under_pressure(queued));
        if replace {
            if let Some(TerminalFrameEvent::Output(queued)) = queue.remove(index) {
                replaced.push(queued);
            }
        } else {
            index += 1;
        }
    }
    for queued in replaced.into_iter().rev() {
        merge_terminal_frame_output_event_into_newer(&queued, incoming);
    }
}

fn merge_terminal_frame_output_event_into_newer(
    older: &TerminalFrameOutputEvent,
    newer: &mut TerminalFrameOutputEvent,
) {
    newer.recording_text_bytes = older
        .recording_text_bytes
        .saturating_add(newer.recording_text_bytes);
    newer.accepted_bytes = older.accepted_bytes.saturating_add(newer.accepted_bytes);
    newer.skipped_output_bytes = older
        .skipped_output_bytes
        .saturating_add(newer.skipped_output_bytes);
    let mut visible_text = older.visible_text.clone();
    append_terminal_frame_visible_tail(&mut visible_text, &newer.visible_text);
    newer.visible_text = visible_text;
    newer.process_duration = older
        .process_duration
        .saturating_add(newer.process_duration);
}

fn terminal_frame_event_can_drop_under_pressure(event: &TerminalFrameEvent) -> bool {
    match event {
        TerminalFrameEvent::Output(frame) => {
            terminal_frame_output_event_can_drop_under_pressure(frame)
        }
        TerminalFrameEvent::Snapshot(_) | TerminalFrameEvent::Search(_) => false,
    }
}

fn terminal_frame_output_event_can_drop_under_pressure(frame: &TerminalFrameOutputEvent) -> bool {
    !frame.effects.bell
        && frame.effects.title.is_none()
        && !frame.effects.reset_title
        && frame.effects.cwd.is_none()
        && !frame.effects.shell_command_started
        && !frame.effects.shell_command_finished
        && frame.effects.pty_write.is_empty()
        && frame.effects.clipboard_store.is_none()
        && frame.effects.clipboard_loads.is_empty()
}

struct TerminalFrameSession {
    screen: TerminalScreen,
    output_decoder: TerminalOutputDecoder,
    recording_decoder: TerminalOutputDecoder,
    visible_output_filter: TerminalVisibleOutputFilter,
    revision: u64,
    /// True after live backend output has produced visible terminal text.
    output_seen: bool,
    /// When false, output frames omit full viewport_snapshot (hidden tabs).
    include_live_snapshot: bool,
    action_link_cache: Option<TerminalFrameActionLinks>,
}

impl TerminalFrameSession {
    fn new(encoding: &str, scrollback_limit: usize) -> Self {
        let mut screen = TerminalScreen::default();
        screen.set_encoding(encoding);
        screen.set_scrollback_limit(scrollback_limit);
        let mut output_decoder = TerminalOutputDecoder::default();
        output_decoder.set_encoding(encoding);
        let mut recording_decoder = TerminalOutputDecoder::default();
        recording_decoder.set_encoding(encoding);
        Self {
            screen,
            output_decoder,
            recording_decoder,
            visible_output_filter: TerminalVisibleOutputFilter::default(),
            revision: 0,
            output_seen: false,
            // New sessions start high-priority until UI reports visibility.
            include_live_snapshot: true,
            action_link_cache: None,
        }
    }

    fn set_encoding_and_limit(&mut self, encoding: &str, scrollback_limit: usize) {
        self.screen.set_encoding(encoding);
        self.screen.set_scrollback_limit(scrollback_limit);
        self.output_decoder.set_encoding(encoding);
        self.recording_decoder.set_encoding(encoding);
    }

    fn seed(&mut self, output: String, encoding: &str, scrollback_limit: usize) {
        // A deferred SSH worker can emit its first banner before the UI drains
        // the start result. Do not let the later reconnect seed reset that live
        // screen and erase the banner that is already visible in the pipeline.
        if self.output_seen {
            return;
        }
        self.screen = terminal_screen_from_output(&output);
        self.screen.set_encoding(encoding);
        self.screen.set_scrollback_limit(scrollback_limit);
        self.output_decoder = TerminalOutputDecoder::default();
        self.output_decoder.set_encoding(encoding);
        self.recording_decoder = TerminalOutputDecoder::default();
        self.recording_decoder.set_encoding(encoding);
        self.visible_output_filter.reset();
        self.revision = self.revision.saturating_add(1);
        self.output_seen = false;
        self.action_link_cache = None;
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if self.screen.cols() as u16 != cols || self.screen.rows() as u16 != rows {
            self.screen.resize(cols, rows);
            self.revision = self.revision.saturating_add(1);
            self.action_link_cache = None;
        }
    }

    fn resized_live_snapshot_event(
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
            snapshot_duration,
            snapshot_stats,
            action_link_stats: TerminalActionLinkBuildStats::default(),
            process_duration: started_at.elapsed(),
        }
    }

    #[cfg(test)]
    fn process_output(
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

    fn process_output_chunk(
        &mut self,
        session_id: &str,
        data: &[u8],
        encoding: &str,
        scrollback_limit: usize,
        recording_writer: &RecordingWriteHandle,
    ) -> TerminalAdvanceResult {
        self.set_encoding_and_limit(encoding, scrollback_limit);
        if data.len() > TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP {
            self.visible_output_filter.reset();
        }
        // Once a visible chunk has arrived, the seed guard is permanently
        // decided for this session; avoid a second byte scan on every later
        // output frame.
        let visible_output_filter = (!self.output_seen).then_some(&mut self.visible_output_filter);
        let result = terminal_advance_result(
            &mut self.screen,
            &mut self.output_decoder,
            &mut self.recording_decoder,
            visible_output_filter,
            session_id,
            data,
            recording_writer,
        );
        let first_live_output = !self.output_seen && result.visible_content_changed;
        if result.visible_content_changed {
            self.output_seen = true;
        }
        if first_live_output {
            tracing::info!(
                diagnostic = "terminal_first_live_output",
                session_id,
                raw_bytes = data.len(),
                accepted_bytes = result.accepted_bytes,
                visible_text_bytes = result.visible_text.len(),
                skipped_output_bytes = result.skipped_output_bytes,
                "first live terminal output reached the frame processor"
            );
        }
        self.revision = self.revision.saturating_add(1);
        result
    }

    fn output_event_from_batch(
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

    fn snapshot_event(
        &mut self,
        session_id: String,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
        priority: bool,
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
            snapshot_duration,
            snapshot_stats,
            action_link_stats,
            process_duration: started_at.elapsed(),
        }
    }

    fn search_event(
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

const SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS: usize = 256;

#[derive(Debug)]
struct SelectedOccurrenceSearchJob {
    session_id: String,
    key: TerminalFrameSearchKey,
    query: TerminalSearchQuery,
    revision: u64,
    total_rows: usize,
    overlap_rows: usize,
    next_absolute_row: usize,
    matches: Vec<TerminalBufferMatch>,
    started_at: Instant,
}

impl SelectedOccurrenceSearchJob {
    fn new(
        session_id: String,
        key: TerminalFrameSearchKey,
        session: &TerminalFrameSession,
    ) -> Self {
        let cols = session.screen.cols().max(1);
        let query_cell_width = terminal_cell_count(&key.query);
        let overlap_rows = query_cell_width.saturating_add(cols.saturating_sub(1)) / cols + 1;
        let query = TerminalSearchQuery {
            pattern: key.query.clone(),
            regex: false,
            case_sensitive: true,
            whole_word: false,
            direction: TerminalSearchDirection::Forward,
            limit: key.limit,
        };
        Self {
            session_id,
            key,
            query,
            revision: session.revision,
            total_rows: session.screen.total_rows(),
            overlap_rows,
            next_absolute_row: 0,
            matches: Vec::new(),
            started_at: Instant::now(),
        }
    }

    fn cancellation_event(self, message: &str) -> TerminalFrameSearchEvent {
        let result =
            TerminalFrameSearchResult::new(self.key, self.revision, Err(message.to_string()));
        TerminalFrameSearchEvent {
            session_id: self.session_id,
            purpose: TerminalFrameSearchPurpose::SelectedOccurrence,
            result,
            process_duration: self.started_at.elapsed(),
        }
    }

    fn completion_event(mut self) -> TerminalFrameSearchEvent {
        self.matches
            .sort_unstable_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.matches
            .dedup_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.matches.truncate(self.key.limit);
        let result = TerminalFrameSearchResult::new(self.key, self.revision, Ok(self.matches));
        TerminalFrameSearchEvent {
            session_id: self.session_id,
            purpose: TerminalFrameSearchPurpose::SelectedOccurrence,
            result,
            process_duration: self.started_at.elapsed(),
        }
    }

    fn process_chunk(
        &mut self,
        session: &TerminalFrameSession,
    ) -> Result<bool, nyaterm_terminal::TerminalSearchError> {
        if self.next_absolute_row >= self.total_rows || self.matches.len() >= self.key.limit {
            return Ok(true);
        }
        let chunk_start = self.next_absolute_row;
        let chunk_end = chunk_start
            .saturating_add(SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS)
            .min(self.total_rows);
        let search_start = chunk_start.saturating_sub(self.overlap_rows);
        let chunk_matches = session
            .screen
            .search_grid_in_absolute_range(&self.query, search_start..chunk_end)?;
        self.matches.extend(
            chunk_matches
                .into_iter()
                .map(|search_match| TerminalBufferMatch {
                    line_index: search_match.line_index,
                    start_col: search_match.start_col,
                    end_col: search_match.end_col,
                }),
        );
        self.matches
            .sort_unstable_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.matches
            .dedup_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.next_absolute_row = chunk_end;
        Ok(self.next_absolute_row >= self.total_rows || self.matches.len() >= self.key.limit)
    }
}

fn process_next_selected_occurrence_search_chunk(
    jobs: &mut VecDeque<SelectedOccurrenceSearchJob>,
    sessions: &HashMap<String, TerminalFrameSession>,
) -> Option<TerminalFrameSearchEvent> {
    let mut job = jobs.pop_front()?;
    let Some(session) = sessions.get(&job.session_id) else {
        return Some(job.cancellation_event("selected occurrence session was removed"));
    };
    if session.revision != job.revision {
        return Some(
            job.cancellation_event("selected occurrence search was cancelled by terminal output"),
        );
    }
    match job.process_chunk(session) {
        Ok(true) => Some(job.completion_event()),
        Ok(false) => {
            jobs.push_back(job);
            None
        }
        Err(error) => Some(job.cancellation_event(&error.to_string())),
    }
}

fn replace_selected_occurrence_search_job(
    jobs: &mut VecDeque<SelectedOccurrenceSearchJob>,
    job: SelectedOccurrenceSearchJob,
) -> Option<TerminalFrameSearchEvent> {
    let replaced = jobs
        .iter()
        .position(|pending| pending.session_id == job.session_id)
        .and_then(|index| jobs.remove(index));
    jobs.push_back(job);
    replaced.map(|stale| {
        stale.cancellation_event("selected occurrence search was replaced by a newer request")
    })
}

fn cancel_selected_occurrence_search_job_for_session(
    jobs: &mut VecDeque<SelectedOccurrenceSearchJob>,
    session_id: &str,
    message: &str,
) -> Option<TerminalFrameSearchEvent> {
    jobs.iter()
        .position(|job| job.session_id == session_id)
        .and_then(|index| jobs.remove(index))
        .map(|job| job.cancellation_event(message))
}

#[derive(Debug)]
struct TerminalAdvanceResult {
    visible_text: String,
    recording_text_bytes: usize,
    effects: TerminalEffects,
    accepted_bytes: usize,
    skipped_output_bytes: usize,
    visible_content_changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum TerminalVisibleOutputState {
    #[default]
    Ground,
    Escape,
    Csi,
    ControlString,
}

#[derive(Debug, Default)]
struct TerminalVisibleOutputFilter {
    state: TerminalVisibleOutputState,
}

impl TerminalVisibleOutputFilter {
    fn reset(&mut self) {
        self.state = TerminalVisibleOutputState::Ground;
    }

    /// Tracks ANSI control strings across PTY chunks without allocating text.
    fn contains_visible_text(&mut self, bytes: &[u8]) -> bool {
        let mut visible = false;
        for &byte in bytes {
            match self.state {
                TerminalVisibleOutputState::Ground => match byte {
                    0x1b => self.state = TerminalVisibleOutputState::Escape,
                    0x9b => self.state = TerminalVisibleOutputState::Csi,
                    0x90 | 0x98 | 0x9d | 0x9e | 0x9f => {
                        self.state = TerminalVisibleOutputState::ControlString;
                    }
                    0x20..=0x7e | 0xa0..=0xff => visible = true,
                    _ => {}
                },
                TerminalVisibleOutputState::Escape => {
                    self.state = match byte {
                        b'[' => TerminalVisibleOutputState::Csi,
                        b']' | b'P' | b'^' | b'_' | b'X' => {
                            TerminalVisibleOutputState::ControlString
                        }
                        0x1b => TerminalVisibleOutputState::Escape,
                        _ => TerminalVisibleOutputState::Ground,
                    };
                }
                TerminalVisibleOutputState::Csi => {
                    if byte == 0x1b {
                        self.state = TerminalVisibleOutputState::Escape;
                    } else if (0x40..=0x7e).contains(&byte) {
                        self.state = TerminalVisibleOutputState::Ground;
                    }
                }
                TerminalVisibleOutputState::ControlString => match byte {
                    0x07 | 0x9c => self.state = TerminalVisibleOutputState::Ground,
                    0x1b => self.state = TerminalVisibleOutputState::Escape,
                    _ => {}
                },
            }
        }
        visible
    }
}

#[derive(Debug, Default)]
struct TerminalFrameOutputBatch {
    visible_text: String,
    recording_text_bytes: usize,
    effects: TerminalEffects,
    accepted_bytes: usize,
    skipped_output_bytes: usize,
}

impl TerminalFrameOutputBatch {
    fn absorb(&mut self, chunk: TerminalAdvanceResult) {
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

fn terminal_advance_result(
    screen: &mut TerminalScreen,
    output_decoder: &mut TerminalOutputDecoder,
    recording_decoder: &mut TerminalOutputDecoder,
    visible_output_filter: Option<&mut TerminalVisibleOutputFilter>,
    session_id: &str,
    data: &[u8],
    recording_writer: &RecordingWriteHandle,
) -> TerminalAdvanceResult {
    let recording_text = recording_decoder.decode_output_text(data);
    let recording_text_bytes = recording_text.len();
    recording_writer.write_output(session_id.to_string(), recording_text);
    let (feed, skipped_output_bytes) = protect_terminal_output_burst(screen, output_decoder, data);
    let visible_content_changed = visible_output_filter
        .map(|filter| filter.contains_visible_text(feed))
        .unwrap_or(false);
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
        visible_content_changed,
    }
}

#[derive(Debug)]
struct TerminalFrameCommandSender {
    shared: Arc<TerminalFrameCommandQueueShared>,
}

#[derive(Debug)]
struct TerminalFrameCommandReceiver {
    shared: Arc<TerminalFrameCommandQueueShared>,
}

#[derive(Debug)]
struct TerminalFrameCommandQueueShared {
    inner: Mutex<TerminalFrameCommandQueueInner>,
    ready: Condvar,
    // Approximate backpressure gauge; command ordering remains protected by `inner`.
    queued_output_bytes: AtomicUsize,
}

#[derive(Debug)]
struct TerminalFrameCommandQueueInner {
    commands: VecDeque<TerminalFrameCommand>,
    sender_count: usize,
}

fn terminal_frame_command_channel() -> (TerminalFrameCommandSender, TerminalFrameCommandReceiver) {
    let shared = Arc::new(TerminalFrameCommandQueueShared {
        inner: Mutex::new(TerminalFrameCommandQueueInner {
            commands: VecDeque::new(),
            sender_count: 1,
        }),
        ready: Condvar::new(),
        queued_output_bytes: AtomicUsize::new(0),
    });
    (
        TerminalFrameCommandSender {
            shared: shared.clone(),
        },
        TerminalFrameCommandReceiver { shared },
    )
}

impl TerminalFrameCommandSender {
    fn send(&self, command: TerminalFrameCommand) -> bool {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return false;
        };
        let output_bytes = terminal_frame_command_output_bytes(&command);
        push_terminal_frame_command(&mut inner.commands, command);
        self.shared
            .queued_output_bytes
            .fetch_add(output_bytes, Ordering::Relaxed);
        self.shared.ready.notify_one();
        true
    }

    fn send_many<I>(&self, commands: I) -> bool
    where
        I: IntoIterator<Item = TerminalFrameCommand>,
    {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return false;
        };
        let mut sent = false;
        for command in commands {
            let output_bytes = terminal_frame_command_output_bytes(&command);
            push_terminal_frame_command(&mut inner.commands, command);
            self.shared
                .queued_output_bytes
                .fetch_add(output_bytes, Ordering::Relaxed);
            sent = true;
        }
        if sent {
            self.shared.ready.notify_one();
        }
        sent
    }

    fn len(&self) -> usize {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.commands.len())
            .unwrap_or(0)
    }

    fn queued_output_bytes(&self) -> usize {
        self.shared.queued_output_bytes.load(Ordering::Relaxed)
    }
}

impl Clone for TerminalFrameCommandSender {
    fn clone(&self) -> Self {
        if let Ok(mut inner) = self.shared.inner.lock() {
            inner.sender_count = inner.sender_count.saturating_add(1);
        }
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for TerminalFrameCommandSender {
    fn drop(&mut self) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.sender_count = inner.sender_count.saturating_sub(1);
        self.shared.ready.notify_all();
    }
}

impl TerminalFrameCommandReceiver {
    fn recv(&self) -> Option<TerminalFrameCommand> {
        let mut inner = self.shared.inner.lock().ok()?;
        loop {
            if let Some(command) = pop_terminal_frame_command(&self.shared, &mut inner) {
                return Some(command);
            }
            if inner.sender_count == 0 {
                return None;
            }
            inner = self.shared.ready.wait(inner).ok()?;
        }
    }

    fn try_recv(&self) -> Option<TerminalFrameCommand> {
        let mut inner = self.shared.inner.lock().ok()?;
        pop_terminal_frame_command(&self.shared, &mut inner)
    }
}

fn pop_terminal_frame_command(
    shared: &TerminalFrameCommandQueueShared,
    inner: &mut TerminalFrameCommandQueueInner,
) -> Option<TerminalFrameCommand> {
    let command = inner.commands.pop_front()?;
    shared.queued_output_bytes.fetch_sub(
        terminal_frame_command_output_bytes(&command),
        Ordering::Relaxed,
    );
    Some(command)
}

fn push_terminal_frame_command(
    commands: &mut VecDeque<TerminalFrameCommand>,
    command: TerminalFrameCommand,
) {
    match command {
        TerminalFrameCommand::Output {
            session_id,
            data,
            encoding,
            scrollback_limit,
        } => {
            let insert_at = commands
                .iter()
                .rposition(|queued| !terminal_frame_command_is_low_priority_derived(queued))
                .map_or(0, |index| index + 1);
            if let Some(TerminalFrameCommand::Output {
                session_id: queued_session_id,
                data: queued_data,
                encoding: queued_encoding,
                scrollback_limit: queued_scrollback_limit,
            }) = insert_at
                .checked_sub(1)
                .and_then(|index| commands.get_mut(index))
                && queued_session_id == &session_id
                && queued_encoding == &encoding
                && *queued_scrollback_limit == scrollback_limit
                && queued_data.len().saturating_add(data.len()) <= TERMINAL_FRAME_OUTPUT_CHUNK_SIZE
            {
                queued_data.extend(data);
                return;
            }
            commands.insert(
                insert_at,
                TerminalFrameCommand::Output {
                    session_id,
                    data,
                    encoding,
                    scrollback_limit,
                },
            );
        }
        TerminalFrameCommand::ResizeSession {
            session_id,
            cols,
            rows,
        } => {
            if let Some(TerminalFrameCommand::ResizeSession {
                session_id: last_session_id,
                cols: last_cols,
                rows: last_rows,
            }) = commands.back_mut()
                && *last_session_id == session_id
            {
                *last_cols = cols;
                *last_rows = rows;
                return;
            }
            commands.push_back(TerminalFrameCommand::ResizeSession {
                session_id,
                cols,
                rows,
            });
        }
        TerminalFrameCommand::RequestSnapshot {
            session_id,
            offset,
            action_links_enabled,
            action_link_matchers,
            priority: true,
            purpose,
        } => {
            commands.retain(|queued| {
                !matches!(
                    queued,
                    TerminalFrameCommand::RequestSnapshot {
                        session_id: queued_session_id,
                        priority: true,
                        ..
                    } if queued_session_id == &session_id
                )
            });
            let insert_at = commands
                .iter()
                .position(terminal_frame_command_priority_snapshot_insert_before)
                .unwrap_or(commands.len());
            commands.insert(
                insert_at,
                TerminalFrameCommand::RequestSnapshot {
                    session_id,
                    offset,
                    action_links_enabled,
                    action_link_matchers,
                    priority: true,
                    purpose,
                },
            );
        }
        other => commands.push_back(other),
    }
    compact_terminal_frame_command_queue(commands, TERMINAL_FRAME_COMMAND_QUEUE_CAP);
}

fn compact_terminal_frame_command_queue(commands: &mut VecDeque<TerminalFrameCommand>, cap: usize) {
    compact_stale_terminal_frame_commands(commands);
    while commands.len() > cap {
        let Some(drop_index) = commands
            .iter()
            .position(terminal_frame_command_can_drop_under_pressure)
        else {
            break;
        };
        commands.remove(drop_index);
    }
}

fn compact_stale_terminal_frame_commands(commands: &mut VecDeque<TerminalFrameCommand>) {
    if commands.len() <= 1 {
        return;
    }
    let mut seen_snapshots: HashSet<(String, usize)> = HashSet::new();
    let mut seen_priority_snapshots: HashSet<String> = HashSet::new();
    let mut seen_searches: HashSet<(String, TerminalFrameSearchPurpose)> = HashSet::new();
    let mut kept_snapshot_priority = false;
    let mut compacted = VecDeque::with_capacity(commands.len());

    for command in commands.drain(..).rev() {
        let keep = match &command {
            TerminalFrameCommand::RequestSnapshot {
                session_id,
                priority: true,
                ..
            } => seen_priority_snapshots.insert(session_id.clone()),
            TerminalFrameCommand::RequestSnapshot {
                session_id, offset, ..
            } => seen_snapshots.insert((session_id.clone(), *offset)),
            TerminalFrameCommand::RequestSearch {
                session_id,
                purpose,
                ..
            } => seen_searches.insert((session_id.clone(), *purpose)),
            TerminalFrameCommand::SetSnapshotPriority { .. } => {
                if kept_snapshot_priority {
                    false
                } else {
                    kept_snapshot_priority = true;
                    true
                }
            }
            _ => true,
        };
        if keep {
            compacted.push_front(command);
        }
    }

    *commands = compacted;
}

fn terminal_frame_command_can_drop_under_pressure(command: &TerminalFrameCommand) -> bool {
    matches!(
        command,
        TerminalFrameCommand::RequestSnapshot {
            priority: false,
            ..
        } | TerminalFrameCommand::RequestSearch { .. }
    )
}

fn terminal_frame_command_is_low_priority_derived(command: &TerminalFrameCommand) -> bool {
    matches!(
        command,
        TerminalFrameCommand::RequestSnapshot {
            priority: false,
            ..
        } | TerminalFrameCommand::RequestSearch { .. }
    )
}

fn terminal_frame_command_priority_snapshot_insert_before(command: &TerminalFrameCommand) -> bool {
    matches!(
        command,
        TerminalFrameCommand::Output { .. }
            | TerminalFrameCommand::RequestSnapshot { .. }
            | TerminalFrameCommand::RequestSearch { .. }
    )
}

fn terminal_frame_command_output_bytes(command: &TerminalFrameCommand) -> usize {
    match command {
        TerminalFrameCommand::Output { data, .. } => data.len(),
        _ => 0,
    }
}

fn run_terminal_frame_processor(
    command_rx: TerminalFrameCommandReceiver,
    event_queue: TerminalFrameEventQueue,
    recording_writer: RecordingWriteHandle,
) {
    let mut sessions: HashMap<String, TerminalFrameSession> = HashMap::new();
    // Sessions that should include full live viewport snapshots on every output.
    // Default (empty) keeps include_live_snapshot as-is for existing sessions and
    // true for newly created ones until the first priority update arrives.
    let mut snapshot_priority: HashSet<String> = HashSet::new();
    let mut priority_initialized = false;
    let mut pending_commands = VecDeque::new();
    let mut selected_occurrence_search_jobs = VecDeque::new();
    loop {
        let mut command = try_next_terminal_frame_command(&command_rx, &mut pending_commands);
        if command.is_none() && selected_occurrence_search_jobs.is_empty() {
            command = command_rx.recv();
            if command.is_none() {
                break;
            }
        }
        let Some(command) = command else {
            if let Some(event) = process_next_selected_occurrence_search_chunk(
                &mut selected_occurrence_search_jobs,
                &sessions,
            ) {
                event_queue.push(TerminalFrameEvent::Search(event));
            }
            continue;
        };
        match command {
            TerminalFrameCommand::EnsureSession {
                session_id,
                encoding,
                scrollback_limit,
            } => {
                let include = !priority_initialized || snapshot_priority.contains(&session_id);
                let session = sessions
                    .entry(session_id)
                    .or_insert_with(|| TerminalFrameSession::new(&encoding, scrollback_limit));
                session.set_encoding_and_limit(&encoding, scrollback_limit);
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
                    event_queue.push(TerminalFrameEvent::Search(stale));
                }
                let include = !priority_initialized || snapshot_priority.contains(&session_id);
                let session = sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| TerminalFrameSession::new(&encoding, scrollback_limit));
                session.seed(output, &encoding, scrollback_limit);
                session.include_live_snapshot = include;
            }
            TerminalFrameCommand::RemoveSession { session_id } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence session was removed",
                ) {
                    event_queue.push(TerminalFrameEvent::Search(stale));
                }
                sessions.remove(&session_id);
                snapshot_priority.remove(&session_id);
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
                    event_queue.push(TerminalFrameEvent::Search(stale));
                }
                if let Some(session) = sessions.get_mut(&session_id) {
                    let started_at = Instant::now();
                    session.resize(cols, rows);
                    let event = session.resized_live_snapshot_event(session_id, started_at);
                    event_queue.push(TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::Output {
                session_id,
                data,
                encoding,
                scrollback_limit,
            } => {
                if let Some(stale) = cancel_selected_occurrence_search_job_for_session(
                    &mut selected_occurrence_search_jobs,
                    &session_id,
                    "selected occurrence search was cancelled by terminal output",
                ) {
                    event_queue.push(TerminalFrameEvent::Search(stale));
                }
                process_terminal_frame_output_burst(
                    &command_rx,
                    &mut pending_commands,
                    &mut sessions,
                    &recording_writer,
                    session_id,
                    data,
                    encoding,
                    scrollback_limit,
                    |event| event_queue.push(TerminalFrameEvent::Output(event)),
                );
            }
            TerminalFrameCommand::RequestSnapshot {
                session_id,
                offset,
                action_links_enabled,
                action_link_matchers,
                priority,
                purpose: _,
            } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    let event = session.snapshot_event(
                        session_id,
                        offset,
                        action_links_enabled,
                        action_link_matchers,
                        priority,
                    );
                    event_queue.push(TerminalFrameEvent::Snapshot(event));
                }
            }
            TerminalFrameCommand::RequestSearch {
                session_id,
                purpose,
                key,
            } => {
                if let Some(session) = sessions.get_mut(&session_id) {
                    if purpose == TerminalFrameSearchPurpose::SelectedOccurrence {
                        let job = SelectedOccurrenceSearchJob::new(session_id, key, session);
                        if let Some(stale) = replace_selected_occurrence_search_job(
                            &mut selected_occurrence_search_jobs,
                            job,
                        ) {
                            event_queue.push(TerminalFrameEvent::Search(stale));
                        }
                    } else {
                        let event = session.search_event(session_id, purpose, key);
                        event_queue.push(TerminalFrameEvent::Search(event));
                    }
                }
            }
            TerminalFrameCommand::SetSnapshotPriority { session_ids } => {
                priority_initialized = true;
                snapshot_priority.clear();
                snapshot_priority.extend(session_ids);
                for (session_id, session) in sessions.iter_mut() {
                    session.include_live_snapshot = snapshot_priority.contains(session_id);
                }
            }
        }
    }
}

fn try_next_terminal_frame_command(
    command_rx: &TerminalFrameCommandReceiver,
    pending_commands: &mut VecDeque<TerminalFrameCommand>,
) -> Option<TerminalFrameCommand> {
    pending_commands
        .pop_front()
        .or_else(|| command_rx.try_recv())
}

#[cfg(test)]
fn next_terminal_frame_command(
    command_rx: &TerminalFrameCommandReceiver,
    pending_commands: &mut VecDeque<TerminalFrameCommand>,
) -> Option<TerminalFrameCommand> {
    pending_commands.pop_front().or_else(|| command_rx.recv())
}

#[allow(clippy::too_many_arguments)]
fn process_terminal_frame_output_burst(
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
fn coalesce_terminal_frame_output_command(
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
struct TerminalFrameOutputShape<'a> {
    session_id: &'a str,
    encoding: &'a str,
    scrollback_limit: usize,
    bytes: usize,
}

fn terminal_frame_output_commands_can_merge(
    current: TerminalFrameOutputShape<'_>,
    next: TerminalFrameOutputShape<'_>,
    byte_limit: usize,
) -> bool {
    current.session_id == next.session_id
        && current.encoding == next.encoding
        && current.scrollback_limit == next.scrollback_limit
        && current.bytes.saturating_add(next.bytes) <= byte_limit
}

const TERMINAL_FRAME_EVENT_QUEUE_CAP: usize = 1024;
const TERMINAL_FRAME_COMMAND_QUEUE_CAP: usize = 512;
/// Ceiling on how far `push_terminal_frame_command` grows a queued `Output` by
/// absorbing the next one. Also the size of a representative PTY read.
const TERMINAL_FRAME_OUTPUT_CHUNK_SIZE: usize = 8 * 1024;
#[cfg(test)]
const TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT: usize = 32 * 1024;
// Parse roughly one display frame of bulk PTY output before materializing the
// next owned viewport. Zed renders from the terminal grid directly, so it does
// not pay this snapshot cost for every 8 KiB event-loop chunk.
const TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT: usize = 128 * 1024;

#[cfg(test)]
mod tests;
