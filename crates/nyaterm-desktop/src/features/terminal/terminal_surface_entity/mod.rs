use gpui::{
    Context, Entity, FontFallbacks, IntoElement, MouseButton, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, WeakEntity, Window, div, font, prelude::*, px, rgb, rgba,
};
use nyaterm_terminal::{TerminalLineId, TerminalScreen, TerminalSnapshot};

use crate::features::NyaTermApp;
use crate::features::formatting::{
    TerminalTimestampFormatter, terminal_gutter_labels, terminal_timestamp_format_width_chars,
};
use crate::features::terminal::terminal_runtime::{
    TerminalScrollVisualState, TerminalScrollWheelStateResult, terminal_display_offset_from_state,
    terminal_local_scroll_delta_lines_from_state, terminal_scroll_needs_text_first_repaint,
    terminal_visual_scroll_active_for_state,
};
use crate::features::terminal::terminal_selection_runtime::{
    terminal_bounds_tracker, terminal_gutter_metrics, terminal_line_number_digits,
};
use crate::features::terminal::terminal_surface::{
    TERMINAL_SCROLLBAR_COLUMN_WIDTH, TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
    TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT, TERMINAL_SCROLLBAR_TRACK_PADDING_Y,
    TerminalScrollbarInput, terminal_overview_marker_buckets, terminal_overview_marker_canvas,
    terminal_scroll_offset_from_pointer, terminal_scrollbar_grab_offset_for_pointer,
    terminal_scrollbar_metrics, terminal_scrollbar_thumb_element,
    terminal_scrollbar_track_bounds_tracker, terminal_scrollbar_track_color, track_height,
};
use crate::models::{TerminalPerformanceOverlay, TerminalProtocolState, TerminalSelection};
use crate::terminal::{
    NyaTerminalElement, NyaTerminalLayoutCache, TerminalGridSelection,
    TerminalKeywordHighlightSnapshot, TerminalKeywordHighlighter, TerminalLineDecorations,
    compile_terminal_keyword_highlighter,
    precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel, terminal_font_features,
    terminal_keyword_highlight_expanded_rows, terminal_keyword_rules_key,
};
use crate::theme::ThemePalette;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Process-wide surface paint counter (Phase 0 isolation diagnostics).
pub(in crate::features) static TERMINAL_SURFACE_PAINT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(in crate::features) static FULL_SHELL_PAINT_COUNT: AtomicU64 = AtomicU64::new(0);
const TERMINAL_SURFACE_RETAINED_SNAPSHOT_LIMIT: usize = 12;
const TERMINAL_SURFACE_RETAINED_ROW_LIMIT: usize = 4096;
const TERMINAL_SURFACE_RETAINED_ROW_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const TERMINAL_SURFACE_SYNTHESIZED_WINDOW_MIN_EXTRA_ROWS: usize = 32;
const TERMINAL_SURFACE_SYNTHESIZED_WINDOW_MAX_EXTRA_ROWS: usize = 192;
const TERMINAL_SURFACE_SCROLL_PENDING_WARN_AFTER: Duration = Duration::from_millis(48);
const TERMINAL_SURFACE_SCROLL_PENDING_WARN_INTERVAL: Duration = Duration::from_millis(500);
const TERMINAL_SURFACE_LOCAL_SCROLL_SYNC_DELAY: Duration = Duration::from_millis(16);
const TERMINAL_KEYWORD_HIGHLIGHT_PREFETCH_VIEWPORTS: usize = 2;
const TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_PREFETCH_VIEWPORTS: usize = 0;
const TERMINAL_KEYWORD_HIGHLIGHT_SLOW: Duration = Duration::from_millis(8);
const TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE: Duration = Duration::from_millis(50);

#[derive(Clone)]
struct TerminalSurfacePendingScrollSync {
    state: TerminalScrollVisualState,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSurfaceLocalScrollResult {
    generation: u64,
    visual_changed: bool,
    needs_text_snapshot: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalKeywordHighlightRequestKey {
    rules_key: u64,
    display_offset: usize,
    row_start: usize,
    row_end: usize,
    line_signatures_key: u64,
}

fn terminal_keyword_highlight_request_key(
    snapshot: &TerminalSnapshot,
    rules_key: u64,
    rows: Range<usize>,
) -> TerminalKeywordHighlightRequestKey {
    let rows = terminal_keyword_highlight_expanded_rows(snapshot, rows);
    let row_start = rows.start.min(snapshot.row_count());
    let row_end = rows.end.min(snapshot.row_count()).max(row_start);
    let mut hasher = DefaultHasher::new();
    let rows = snapshot.rows().get(row_start..row_end).unwrap_or_default();
    rows.len().hash(&mut hasher);
    for row in rows {
        (row.signature, row.wrapped).hash(&mut hasher);
    }
    TerminalKeywordHighlightRequestKey {
        rules_key,
        display_offset: snapshot.display_offset,
        row_start,
        row_end,
        line_signatures_key: hasher.finish(),
    }
}

fn terminal_snapshot_row_estimated_bytes(row: &nyaterm_terminal::TerminalSnapshotRow) -> usize {
    let mut bytes = std::mem::size_of_val(row)
        .saturating_add(std::mem::size_of_val(row.cells.as_ref()))
        .saturating_add(std::mem::size_of_val(row.styled_spans.as_ref()))
        .saturating_add(std::mem::size_of_val(row.hyperlinks.as_ref()))
        .saturating_add(row.text.capacity());
    for cell in &row.cells {
        bytes = bytes.saturating_add(cell.text.len());
        if let Some(uri) = cell.hyperlink.as_deref() {
            bytes = bytes.saturating_add(uri.len());
        }
    }
    for span in &row.styled_spans {
        bytes = bytes.saturating_add(span.text.capacity());
    }
    for hyperlink in &row.hyperlinks {
        bytes = bytes.saturating_add(hyperlink.uri.capacity());
    }
    bytes
}

fn terminal_keyword_highlight_prefetch_viewports(output_pressure: bool) -> usize {
    if output_pressure {
        TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_PREFETCH_VIEWPORTS
    } else {
        TERMINAL_KEYWORD_HIGHLIGHT_PREFETCH_VIEWPORTS
    }
}

fn terminal_keyword_highlight_pressure_delay(
    output_pressure: bool,
    elapsed_since_last_start: Option<Duration>,
) -> Option<Duration> {
    if !output_pressure {
        return None;
    }
    let elapsed = elapsed_since_last_start?;
    if elapsed < TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE {
        Some(TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE - elapsed)
    } else {
        None
    }
}

fn terminal_keyword_rule_sets_equal(
    left: &Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
    right: &Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
) -> bool {
    Arc::ptr_eq(left, right) || left.as_ref() == right.as_ref()
}

fn empty_terminal_keyword_rules() -> Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>> {
    static RULES: OnceLock<Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>> = OnceLock::new();
    Arc::clone(RULES.get_or_init(|| Arc::new(Vec::new())))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct TerminalPaintedHitTestGeometry {
    pub(in crate::features) grid_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(in crate::features) display_offset: usize,
    pub(in crate::features) viewport_anchor_row: usize,
    pub(in crate::features) snapshot_rows: usize,
    pub(in crate::features) viewport_rows: usize,
    pub(in crate::features) visual_y_offset: f32,
    pub(in crate::features) cell_width: f32,
    pub(in crate::features) cell_height: f32,
    pub(in crate::features) revision: u64,
}

fn terminal_surface_grid_bounds_tracker(surface: Entity<TerminalSurface>) -> impl IntoElement {
    let tracked_surface = surface.clone();
    gpui::canvas(
        move |bounds, _window, cx| {
            let unchanged = tracked_surface
                .read(cx)
                .painted_hit_test_geometry
                .is_some_and(|geometry| geometry.grid_bounds == Some(bounds));
            if unchanged {
                return;
            }
            let surface = surface.clone();
            cx.defer(move |cx| {
                surface.update(cx, |surface, _cx| {
                    surface.set_painted_hit_test_grid_bounds(bounds);
                });
            });
        },
        |_bounds, _state, _window, _cx| {},
    )
    .absolute()
    .inset_0()
    .size_full()
}

pub(in crate::features) struct TerminalSurfacePaintChrome {
    pub palette: ThemePalette,
    pub font_family: String,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_size: f32,
    pub normal_weight: f32,
    pub bold_weight: f32,
    pub bold_default_foreground: bool,
    pub cell_width: f32,
    pub cell_height: f32,
    pub show_line_numbers: bool,
    pub show_timestamps: bool,
    pub timestamp_format: String,
    pub is_active: bool,
}

pub(in crate::features) struct TerminalSurfaceFrameSnapshot {
    pub snapshot: Arc<TerminalSnapshot>,
    pub scroll: TerminalScrollVisualState,
    pub has_action_link_decorations: bool,
    pub show_cursor: bool,
    pub cursor_style: String,
}

impl TerminalSurfaceFrameSnapshot {
    pub(in crate::features) fn new(
        snapshot: Arc<TerminalSnapshot>,
        scroll: TerminalScrollVisualState,
    ) -> Self {
        Self {
            snapshot,
            scroll,
            has_action_link_decorations: false,
            show_cursor: false,
            cursor_style: "block".to_string(),
        }
    }

    #[cfg(test)]
    fn with_output_state(
        mut self,
        has_new_while_scrolled: bool,
        performance_overlay: Option<TerminalPerformanceOverlay>,
        skipped_output_chars: u64,
    ) -> Self {
        self.scroll.has_new_while_scrolled = has_new_while_scrolled;
        self.scroll.performance_overlay = performance_overlay;
        self.scroll.skipped_output_chars = skipped_output_chars;
        self
    }

    pub(in crate::features) fn with_presentation(
        mut self,
        has_action_link_decorations: bool,
        show_cursor: bool,
        cursor_style: impl Into<String>,
    ) -> Self {
        self.has_action_link_decorations = has_action_link_decorations;
        self.show_cursor = show_cursor;
        self.cursor_style = cursor_style.into();
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct TerminalVisualScrollGeometry {
    pub snapshot_pending: bool,
    pub target_offset: usize,
    pub displayed_offset: usize,
    pub residual_lines: f32,
    pub viewport_anchor_row: usize,
    pub snapshot_rows: usize,
    pub viewport_rows: usize,
    pub cell_height: f32,
}

pub(in crate::features) fn terminal_surface_paint_count() -> u64 {
    TERMINAL_SURFACE_PAINT_COUNT.load(Ordering::Relaxed)
}

/// Per-session GPUI entity that owns terminal grid paint state.
///
/// Output frames notify this entity only; chrome (tabs/sidebars/status) stays
/// on `NyaTermApp` and is notified only for unread/effects/layout changes.
pub(in crate::features) struct TerminalSurface {
    session_id: String,
    /// Parent app for scroll/selection actions that still live on NyaTermApp.
    app: Option<WeakEntity<NyaTermApp>>,
    snapshot: Option<Arc<TerminalSnapshot>>,
    retained_snapshots: Vec<Arc<TerminalSnapshot>>,
    retained_rows: BTreeMap<usize, Arc<nyaterm_terminal::TerminalSnapshotRow>>,
    retained_row_estimated_bytes: usize,
    keyword_rules: Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
    keyword_highlights: Option<Arc<TerminalKeywordHighlightSnapshot>>,
    keyword_highlight_generation: u64,
    keyword_highlight_cancel_epoch: Arc<AtomicU64>,
    keyword_highlight_task: Option<gpui::Task<()>>,
    keyword_highlight_deferred_task: Option<gpui::Task<()>>,
    keyword_highlight_pending_key: Option<TerminalKeywordHighlightRequestKey>,
    keyword_highlight_last_started_at: Option<Instant>,
    keyword_highlight_output_pressure: bool,
    keyword_highlighter_rules: Option<Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>>,
    keyword_highlighter: Option<Arc<TerminalKeywordHighlighter>>,
    decorations: Arc<[TerminalLineDecorations]>,
    selection_visual: Option<TerminalSelection>,
    palette: ThemePalette,
    font_family: String,
    font_fallbacks: Option<FontFallbacks>,
    font_size: f32,
    normal_weight: f32,
    bold_weight: f32,
    bold_default_foreground: bool,
    cell_width: f32,
    cell_height: f32,
    show_cursor: bool,
    cursor_style: String,
    layout_cache: Arc<Mutex<NyaTerminalLayoutCache>>,
    show_line_numbers: bool,
    show_timestamps: bool,
    timestamp_format: String,
    scroll_offset: usize,
    scroll_residual_lines: f32,
    display_offset: usize,
    scroll_snapshot_pending: bool,
    scrollback_len: usize,
    viewport_rows: usize,
    has_new_while_scrolled: bool,
    has_action_link_decorations: bool,
    performance_overlay: Option<TerminalPerformanceOverlay>,
    skipped_output_chars: u64,
    transparent_background: bool,
    is_active: bool,
    protocol_state: TerminalProtocolState,
    zebra_stripes_enabled: bool,
    target_line: Option<TerminalLineId>,
    scroll_interaction_generation: u64,
    pending_local_scroll_sync: Option<TerminalSurfacePendingScrollSync>,
    local_scroll_sync_armed: bool,
    pending_scroll_snapshot_offsets: BTreeSet<usize>,
    revision: u64,
    scroll_snapshot_pending_since: Option<Instant>,
    last_scroll_snapshot_pending_warn_at: Option<Instant>,
    painted_hit_test_geometry: Option<TerminalPaintedHitTestGeometry>,
    painted_hit_test_snapshot: Option<Arc<TerminalSnapshot>>,
    overview_markers: Arc<[crate::features::terminal::terminal_surface::TerminalOverviewMarker]>,
    overview_total_rows: usize,
    overview_marker_key: u64,
    overview_marker_bucket_key: Option<(u64, usize, usize)>,
    overview_marker_buckets:
        Arc<[crate::features::terminal::terminal_surface::TerminalOverviewMarkerBucket]>,
}

impl TerminalSurface {
    pub(in crate::features) fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            app: None,
            snapshot: None,
            retained_snapshots: Vec::new(),
            retained_rows: BTreeMap::new(),
            retained_row_estimated_bytes: 0,
            keyword_rules: Arc::new(Vec::new()),
            keyword_highlights: None,
            keyword_highlight_generation: 0,
            keyword_highlight_cancel_epoch: Arc::new(AtomicU64::new(0)),
            keyword_highlight_task: None,
            keyword_highlight_deferred_task: None,
            keyword_highlight_pending_key: None,
            keyword_highlight_last_started_at: None,
            keyword_highlight_output_pressure: false,
            keyword_highlighter_rules: None,
            keyword_highlighter: None,
            decorations: Arc::from(Vec::<TerminalLineDecorations>::new()),
            selection_visual: None,
            palette: crate::theme::theme_palette("github-dark"),
            font_family: "monospace".to_string(),
            font_fallbacks: None,
            font_size: 14.0,
            normal_weight: 400.0,
            bold_weight: 700.0,
            bold_default_foreground: false,
            cell_width: 8.0,
            cell_height: 16.0,
            show_cursor: false,
            cursor_style: "block".to_string(),
            layout_cache: Arc::new(Mutex::new(NyaTerminalLayoutCache::default())),
            show_line_numbers: false,
            show_timestamps: false,
            timestamp_format: nyaterm_core::DEFAULT_TERMINAL_TIMESTAMP_FORMAT.to_string(),
            scroll_offset: 0,
            scroll_residual_lines: 0.0,
            display_offset: 0,
            scroll_snapshot_pending: false,
            scrollback_len: 0,
            viewport_rows: 1,
            has_new_while_scrolled: false,
            has_action_link_decorations: false,
            performance_overlay: None,
            skipped_output_chars: 0,
            transparent_background: false,
            is_active: false,
            protocol_state: TerminalProtocolState::default(),
            zebra_stripes_enabled: false,
            target_line: None,
            scroll_interaction_generation: 0,
            pending_local_scroll_sync: None,
            local_scroll_sync_armed: false,
            pending_scroll_snapshot_offsets: BTreeSet::new(),
            revision: 0,
            scroll_snapshot_pending_since: None,
            last_scroll_snapshot_pending_warn_at: None,
            painted_hit_test_geometry: None,
            painted_hit_test_snapshot: None,
            overview_markers: Arc::from([]),
            overview_total_rows: 1,
            overview_marker_key: 0,
            overview_marker_bucket_key: None,
            overview_marker_buckets: Arc::from([]),
        }
    }

    pub(in crate::features) fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Return the surface's committed live cursor position; scrolled snapshots
    /// do not have a paintable cursor.
    pub(in crate::features) fn cursor_position(&self) -> Option<(usize, usize)> {
        self.snapshot.as_ref().and_then(|snapshot| {
            (snapshot.display_offset == 0 && snapshot.cursor.row != usize::MAX)
                .then_some((snapshot.cursor.row, snapshot.cursor.col))
        })
    }

    #[cfg(test)]
    pub(in crate::features) fn cursor_is_shown(&self) -> bool {
        self.show_cursor
    }

    pub(in crate::features) fn painted_hit_test_state(
        &self,
    ) -> Option<(TerminalPaintedHitTestGeometry, Arc<TerminalSnapshot>)> {
        Some((
            self.painted_hit_test_geometry?,
            self.painted_hit_test_snapshot.clone()?,
        ))
    }

    fn set_painted_hit_test_grid_bounds(&mut self, bounds: gpui::Bounds<gpui::Pixels>) -> bool {
        let Some(geometry) = self.painted_hit_test_geometry.as_mut() else {
            return false;
        };
        if geometry.grid_bounds == Some(bounds) {
            return false;
        }
        geometry.grid_bounds = Some(bounds);
        true
    }

    pub(in crate::features) fn snapshot_covering_display_offset(
        &self,
        display_offset: usize,
        viewport_rows: usize,
        scrollback_len: usize,
    ) -> Option<Arc<TerminalSnapshot>> {
        self.snapshot
            .as_ref()
            .filter(|snapshot| {
                terminal_snapshot_covers_display_offset(
                    snapshot,
                    display_offset,
                    viewport_rows,
                    scrollback_len,
                )
            })
            .cloned()
            .or_else(|| {
                self.retained_snapshots
                    .iter()
                    .filter(|snapshot| {
                        terminal_snapshot_covers_display_offset(
                            snapshot,
                            display_offset,
                            viewport_rows,
                            scrollback_len,
                        )
                    })
                    .min_by_key(|snapshot| snapshot.display_offset.abs_diff(display_offset))
                    .cloned()
            })
            .or_else(|| {
                self.synthesize_snapshot_covering_display_offset(
                    display_offset,
                    viewport_rows,
                    scrollback_len,
                )
            })
    }

    pub(in crate::features) fn retained_snapshot_covering_display_offset(
        &self,
        display_offset: usize,
        viewport_rows: usize,
        scrollback_len: usize,
    ) -> Option<Arc<TerminalSnapshot>> {
        self.snapshot
            .as_ref()
            .filter(|snapshot| {
                terminal_snapshot_covers_display_offset(
                    snapshot,
                    display_offset,
                    viewport_rows,
                    scrollback_len,
                )
            })
            .cloned()
            .or_else(|| {
                self.retained_snapshots
                    .iter()
                    .filter(|snapshot| {
                        terminal_snapshot_covers_display_offset(
                            snapshot,
                            display_offset,
                            viewport_rows,
                            scrollback_len,
                        )
                    })
                    .min_by_key(|snapshot| snapshot.display_offset.abs_diff(display_offset))
                    .cloned()
            })
    }

    pub(in crate::features) fn has_snapshot_covering_display_offset(
        &self,
        display_offset: usize,
        viewport_rows: usize,
        scrollback_len: usize,
    ) -> bool {
        if self
            .retained_snapshot_covering_display_offset(
                display_offset,
                viewport_rows,
                scrollback_len,
            )
            .is_some()
        {
            return true;
        }
        self.can_synthesize_snapshot_covering_display_offset(
            display_offset,
            viewport_rows,
            scrollback_len,
        )
    }

    pub(in crate::features) fn retain_prefetched_snapshot(
        &mut self,
        snapshot: Arc<TerminalSnapshot>,
    ) -> bool {
        if snapshot.row_count() == 0
            || self.snapshot.as_ref().is_some_and(|current| {
                current.cols != snapshot.cols
                    || current.viewport_rows != snapshot.viewport_rows
                    || snapshot.scrollback_len < self.scrollback_len
            })
            || self
                .retained_snapshots
                .iter()
                .any(|retained| Arc::ptr_eq(retained, &snapshot))
        {
            return false;
        }
        self.remember_retained_snapshot(snapshot);
        self.prune_pending_scroll_snapshot_offsets();
        true
    }

    pub(in crate::features) fn set_app(&mut self, app: Entity<NyaTermApp>) {
        self.app = Some(app.downgrade());
    }

    pub(in crate::features) fn apply_frame_snapshot(
        &mut self,
        frame: TerminalSurfaceFrameSnapshot,
    ) -> bool {
        // Decorations/keywords are pushed separately so frame notifies can keep
        // selection/search highlights until the next decoration rebuild.
        let TerminalSurfaceFrameSnapshot {
            snapshot,
            mut scroll,
            has_action_link_decorations,
            show_cursor,
            cursor_style,
        } = frame;
        scroll.session_id.clone_from(&self.session_id);
        scroll.viewport_rows = scroll.viewport_rows.max(1);
        let TerminalScrollVisualState {
            session_id: _,
            scroll_offset,
            scroll_residual_lines,
            display_offset,
            scrollback_len,
            viewport_rows,
            has_new_while_scrolled,
            performance_overlay,
            skipped_output_chars,
        } = scroll.clone();
        let desired_show_cursor = show_cursor
            && !terminal_visual_scroll_active_for_state(scroll_offset, scroll_residual_lines);
        let retained_rows_should_reset =
            self.retained_rows_should_reset(snapshot.as_ref(), scrollback_len, viewport_rows);
        let pending_local_scroll_state = (!retained_rows_should_reset)
            .then(|| self.pending_local_scroll_state_reanchored_for_frame(&scroll))
            .flatten();
        if !retained_rows_should_reset
            && pending_local_scroll_state.is_none()
            && !self.has_pending_local_scroll_sync()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &snapshot))
            && self.scroll_offset == scroll_offset
            && (self.scroll_residual_lines - scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.display_offset == display_offset
            && self.scrollback_len == scrollback_len
            && self.viewport_rows == viewport_rows
            && self.has_new_while_scrolled == has_new_while_scrolled
            && self.has_action_link_decorations == has_action_link_decorations
            && self.performance_overlay == performance_overlay
            && self.skipped_output_chars == skipped_output_chars
            && self.show_cursor == desired_show_cursor
            && self.cursor_style == cursor_style
        {
            return false;
        }
        if retained_rows_should_reset {
            self.clear_retained_scroll_state();
        }
        self.remember_retained_snapshot(snapshot.clone());
        if let Some(state) = pending_local_scroll_state {
            let text_updated = self.apply_scroll_visual_state(state);
            if text_updated {
                self.show_cursor = false;
            }
            return false;
        }
        self.snapshot = Some(snapshot);
        self.scroll_offset = scroll_offset;
        self.scroll_residual_lines = scroll_residual_lines;
        self.display_offset = display_offset;
        self.scroll_snapshot_pending = false;
        self.scroll_snapshot_pending_since = None;
        self.clear_pending_scroll_snapshot_offsets_if_scrollback_changed(scrollback_len);
        self.scrollback_len = scrollback_len;
        self.viewport_rows = viewport_rows;
        self.has_new_while_scrolled = has_new_while_scrolled;
        self.has_action_link_decorations = has_action_link_decorations;
        self.performance_overlay = performance_overlay;
        self.skipped_output_chars = skipped_output_chars;
        self.show_cursor = desired_show_cursor;
        self.cursor_style = cursor_style;
        self.prune_pending_scroll_snapshot_offsets();
        self.revision = self.revision.saturating_add(1);
        true
    }

    fn pending_local_scroll_state_reanchored_for_frame(
        &self,
        frame: &TerminalScrollVisualState,
    ) -> Option<TerminalScrollVisualState> {
        let pending = self.pending_local_scroll_sync.as_ref()?;
        let mut state = pending.state.clone();
        if state.scroll_offset == frame.scroll_offset
            && state.display_offset == frame.display_offset
            && (state.scroll_residual_lines - frame.scroll_residual_lines).abs()
                < f32::EPSILON * 8.0
        {
            return None;
        }
        if state.scroll_offset == 0 {
            state.scroll_residual_lines = 0.0;
        } else {
            state.scroll_offset = state
                .scroll_offset
                .saturating_add(frame.scrollback_len.saturating_sub(state.scrollback_len))
                .min(frame.scrollback_len);
            if state.scroll_offset >= frame.scrollback_len && state.scroll_residual_lines > 0.0 {
                state.scroll_residual_lines = 0.0;
            }
        }
        state.scrollback_len = frame.scrollback_len;
        state.viewport_rows = frame.viewport_rows.max(1);
        state.display_offset = terminal_display_offset_from_state(
            state.scroll_offset,
            state.scroll_residual_lines,
            state.scrollback_len,
        );
        state.has_new_while_scrolled = frame.has_new_while_scrolled
            || terminal_visual_scroll_active_for_state(
                state.scroll_offset,
                state.scroll_residual_lines,
            );
        state.performance_overlay = frame.performance_overlay;
        state.skipped_output_chars = frame.skipped_output_chars;
        Some(state)
    }

    fn retained_rows_should_reset(
        &self,
        snapshot: &TerminalSnapshot,
        scrollback_len: usize,
        viewport_rows: usize,
    ) -> bool {
        let viewport_rows = viewport_rows.max(1);
        let Some(previous) = self.snapshot.as_ref() else {
            return false;
        };
        previous.cols != snapshot.cols
            || self.viewport_rows != viewport_rows
            || scrollback_len < self.scrollback_len
            || snapshot.total_rows < previous.total_rows
    }

    fn clear_retained_scroll_state(&mut self) {
        self.retained_snapshots.clear();
        self.retained_rows.clear();
        self.retained_row_estimated_bytes = 0;
        self.decorations = Arc::from(Vec::<TerminalLineDecorations>::new());
        self.selection_visual = None;
        self.has_action_link_decorations = false;
        self.scroll_snapshot_pending = false;
        self.scroll_snapshot_pending_since = None;
        self.pending_scroll_snapshot_offsets.clear();
    }

    pub(in crate::features) fn update_scroll_chrome_without_snapshot(
        &mut self,
        state: &TerminalScrollVisualState,
    ) -> bool {
        let scroll_offset = state.scroll_offset;
        let scroll_residual_lines = state.scroll_residual_lines;
        let display_offset = state.display_offset;
        let scrollback_len = state.scrollback_len;
        let viewport_rows = state.viewport_rows;
        let has_new_while_scrolled = state.has_new_while_scrolled;
        let performance_overlay = state.performance_overlay;
        let skipped_output_chars = state.skipped_output_chars;
        let state_matches = self.scroll_offset == scroll_offset
            && (self.scroll_residual_lines - scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.scrollback_len == scrollback_len
            && self.viewport_rows == viewport_rows.max(1)
            && self.has_new_while_scrolled == has_new_while_scrolled
            && self.performance_overlay == performance_overlay
            && self.skipped_output_chars == skipped_output_chars
            && !self.show_cursor
            && if self.snapshot.is_some() {
                self.scroll_snapshot_pending == (self.display_offset != display_offset)
            } else {
                self.display_offset == display_offset && !self.scroll_snapshot_pending
            };
        if state_matches {
            return false;
        }
        self.scroll_offset = scroll_offset;
        self.scroll_residual_lines = scroll_residual_lines;
        if self.snapshot.is_none() {
            self.display_offset = display_offset;
        }
        self.set_scroll_snapshot_pending(
            self.snapshot.is_some() && self.display_offset != display_offset,
        );
        self.clear_pending_scroll_snapshot_offsets_if_scrollback_changed(scrollback_len);
        self.scrollback_len = scrollback_len;
        self.viewport_rows = viewport_rows.max(1);
        self.has_new_while_scrolled = has_new_while_scrolled;
        self.performance_overlay = performance_overlay;
        self.skipped_output_chars = skipped_output_chars;
        // Keep decorations tied to the retained snapshot while the target
        // scrollback snapshot is still loading. The editor follows the same
        // stale-until-recomputed rule for highlights: old adornments are better
        // than a visible flash to an undecorated terminal surface.
        self.show_cursor = false;
        self.revision = self.revision.saturating_add(1);
        true
    }

    pub(in crate::features) fn update_scroll_position_without_snapshot(
        &mut self,
        state: &TerminalScrollVisualState,
    ) -> bool {
        let scroll_offset = state.scroll_offset;
        let scroll_residual_lines = state.scroll_residual_lines;
        let display_offset = state.display_offset;
        let scrollback_len = state.scrollback_len;
        let viewport_rows = state.viewport_rows;
        let has_new_while_scrolled = state.has_new_while_scrolled;
        let performance_overlay = state.performance_overlay;
        let skipped_output_chars = state.skipped_output_chars;
        let viewport_rows = viewport_rows.max(1);
        let snapshot_covers_display_offset = self.snapshot.as_ref().is_some_and(|snapshot| {
            terminal_snapshot_covers_display_offset(
                snapshot.as_ref(),
                display_offset,
                viewport_rows,
                scrollback_len,
            )
        });
        let state_matches = snapshot_covers_display_offset
            && self.scroll_offset == scroll_offset
            && (self.scroll_residual_lines - scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.display_offset == display_offset
            && self.scrollback_len == scrollback_len
            && self.viewport_rows == viewport_rows
            && self.has_new_while_scrolled == has_new_while_scrolled
            && self.performance_overlay == performance_overlay
            && self.skipped_output_chars == skipped_output_chars
            && (!self.visual_scroll_active() || !self.show_cursor);
        if state_matches {
            return false;
        }
        self.promote_snapshot_covering_display_offset(
            display_offset,
            viewport_rows,
            scrollback_len,
        );
        // This is the scroll-only path. Keep stale decorations attached until a
        // full surface sync computes replacements, matching Zed/editor style
        // stale-until-recomputed highlights and avoiding a flash to plain text.
        self.scroll_offset = scroll_offset;
        self.scroll_residual_lines = scroll_residual_lines;
        self.display_offset = display_offset;
        self.scroll_snapshot_pending = false;
        self.scroll_snapshot_pending_since = None;
        self.clear_pending_scroll_snapshot_offsets_if_scrollback_changed(scrollback_len);
        self.scrollback_len = scrollback_len;
        self.viewport_rows = viewport_rows.max(1);
        self.has_new_while_scrolled = has_new_while_scrolled;
        self.performance_overlay = performance_overlay;
        self.skipped_output_chars = skipped_output_chars;
        if self.visual_scroll_active() {
            self.show_cursor = false;
        }
        self.prune_pending_scroll_snapshot_offsets();
        self.revision = self.revision.saturating_add(1);
        true
    }

    fn scroll_snapshot_request_offsets_to_enqueue(&mut self, offsets: Vec<usize>) -> Vec<usize> {
        let mut offsets = offsets
            .into_iter()
            .filter(|offset| *offset > 0)
            .collect::<Vec<_>>();
        offsets.sort_unstable();
        offsets.dedup();
        offsets
            .into_iter()
            .filter(|offset| {
                !self.has_snapshot_covering_display_offset(
                    *offset,
                    self.viewport_rows,
                    self.scrollback_len,
                ) && self.pending_scroll_snapshot_offsets.insert(*offset)
            })
            .collect()
    }

    fn clear_pending_scroll_snapshot_offsets_if_scrollback_changed(
        &mut self,
        next_scrollback_len: usize,
    ) {
        if next_scrollback_len != self.scrollback_len {
            self.pending_scroll_snapshot_offsets.clear();
        }
    }

    fn prune_pending_scroll_snapshot_offsets(&mut self) {
        if self.pending_scroll_snapshot_offsets.is_empty() {
            return;
        }
        let viewport_rows = self.viewport_rows;
        let scrollback_len = self.scrollback_len;
        let resolved = self
            .pending_scroll_snapshot_offsets
            .iter()
            .copied()
            .filter(|offset| {
                self.has_snapshot_covering_display_offset(*offset, viewport_rows, scrollback_len)
            })
            .collect::<Vec<_>>();
        for offset in resolved {
            self.pending_scroll_snapshot_offsets.remove(&offset);
        }
    }

    fn set_scroll_snapshot_pending(&mut self, pending: bool) {
        if pending {
            if !self.scroll_snapshot_pending {
                self.scroll_snapshot_pending_since = Some(Instant::now());
            }
        } else {
            self.scroll_snapshot_pending_since = None;
        }
        self.scroll_snapshot_pending = pending;
    }

    fn maybe_log_scroll_snapshot_pending(&mut self, snapshot: &TerminalSnapshot) {
        if !self.scroll_snapshot_pending {
            return;
        }
        let Some(pending_since) = self.scroll_snapshot_pending_since else {
            return;
        };
        let now = Instant::now();
        let pending_for = now.saturating_duration_since(pending_since);
        if pending_for < TERMINAL_SURFACE_SCROLL_PENDING_WARN_AFTER {
            return;
        }
        if self
            .last_scroll_snapshot_pending_warn_at
            .is_some_and(|last| {
                now.saturating_duration_since(last) < TERMINAL_SURFACE_SCROLL_PENDING_WARN_INTERVAL
            })
        {
            return;
        }
        self.last_scroll_snapshot_pending_warn_at = Some(now);
        tracing::warn!(
            diagnostic = "terminal_surface_scroll_snapshot_pending",
            session_id = %self.session_id,
            scroll_offset = self.scroll_offset,
            display_offset = self.display_offset,
            residual_lines = self.scroll_residual_lines,
            scrollback_len = self.scrollback_len,
            viewport_rows = self.viewport_rows,
            snapshot_display_offset = snapshot.display_offset,
            snapshot_rows = snapshot.row_count(),
            snapshot_total_rows = snapshot.total_rows,
            retained_snapshots = self.retained_snapshots.len(),
            retained_rows = self.retained_rows.len(),
            retained_row_bytes = self.retained_row_estimated_bytes,
            pending_ms = pending_for.as_millis(),
            "terminal surface retained text while waiting for target scroll snapshot"
        );
    }

    fn remember_retained_snapshot(&mut self, snapshot: Arc<TerminalSnapshot>) {
        if snapshot.row_count() == 0 {
            return;
        }
        self.remember_retained_snapshot_rows(snapshot.as_ref());
        self.retained_snapshots.retain(|retained| {
            !(retained.display_offset == snapshot.display_offset
                && retained.total_rows == snapshot.total_rows
                && retained.row_count() == snapshot.row_count())
        });
        self.retained_snapshots.push(snapshot);
        let excess = self
            .retained_snapshots
            .len()
            .saturating_sub(TERMINAL_SURFACE_RETAINED_SNAPSHOT_LIMIT);
        if excess > 0 {
            self.retained_snapshots.drain(0..excess);
        }
    }

    fn remember_retained_snapshot_rows(&mut self, snapshot: &TerminalSnapshot) -> usize {
        self.remember_retained_snapshot_rows_with_limits(
            snapshot,
            TERMINAL_SURFACE_RETAINED_ROW_LIMIT,
            TERMINAL_SURFACE_RETAINED_ROW_BYTE_LIMIT,
        )
    }

    fn remember_retained_snapshot_rows_with_limits(
        &mut self,
        snapshot: &TerminalSnapshot,
        max_rows: usize,
        max_bytes: usize,
    ) -> usize {
        let Some((start, _)) = terminal_snapshot_absolute_window(snapshot) else {
            return 0;
        };
        let mut refreshed_rows = 0usize;
        for row in 0..snapshot.row_count() {
            let Some(abs_row) = start.checked_add(row) else {
                continue;
            };
            let Some(snapshot_row) = snapshot.rows().get(row) else {
                continue;
            };
            if self.retained_rows.get(&abs_row).is_some_and(|retained| {
                Arc::ptr_eq(retained, snapshot_row) || retained.as_ref() == snapshot_row.as_ref()
            }) {
                continue;
            }
            if let Some(replaced) = self.retained_rows.insert(abs_row, Arc::clone(snapshot_row)) {
                self.retained_row_estimated_bytes = self
                    .retained_row_estimated_bytes
                    .saturating_sub(terminal_snapshot_row_estimated_bytes(&replaced));
            }
            self.retained_row_estimated_bytes = self
                .retained_row_estimated_bytes
                .saturating_add(terminal_snapshot_row_estimated_bytes(snapshot_row));
            refreshed_rows = refreshed_rows.saturating_add(1);
        }
        while self.retained_rows.len() > max_rows || self.retained_row_estimated_bytes > max_bytes {
            let Some(drop_key) = self.retained_rows.keys().next().copied() else {
                break;
            };
            if let Some(removed) = self.retained_rows.remove(&drop_key) {
                self.retained_row_estimated_bytes = self
                    .retained_row_estimated_bytes
                    .saturating_sub(terminal_snapshot_row_estimated_bytes(&removed));
            }
        }
        refreshed_rows
    }

    fn promote_snapshot_covering_display_offset(
        &mut self,
        display_offset: usize,
        viewport_rows: usize,
        scrollback_len: usize,
    ) -> bool {
        // Scrolling usually stays inside the retained window already assigned
        // to the surface. Keep that Arc in place: rebuilding retained rows here
        // clones the entire viewport on every pixel-wheel event.
        if self.snapshot.as_ref().is_some_and(|snapshot| {
            terminal_snapshot_covers_display_offset(
                snapshot,
                display_offset,
                viewport_rows,
                scrollback_len,
            )
        }) {
            return true;
        }
        let Some(snapshot) =
            self.snapshot_covering_display_offset(display_offset, viewport_rows, scrollback_len)
        else {
            return false;
        };
        let already_retained = self
            .retained_snapshots
            .iter()
            .any(|retained| Arc::ptr_eq(retained, &snapshot));
        if !already_retained {
            self.remember_retained_snapshot(snapshot.clone());
        }
        self.snapshot = Some(snapshot);
        true
    }

    fn can_synthesize_snapshot_covering_display_offset(
        &self,
        display_offset: usize,
        viewport_rows: usize,
        scrollback_len: usize,
    ) -> bool {
        let viewport_rows = viewport_rows.max(1);
        let real_total_rows = scrollback_len.saturating_add(viewport_rows);
        let desired_end = real_total_rows.saturating_sub(display_offset);
        let desired_start = desired_end.saturating_sub(viewport_rows);
        self.retained_rows_cover_absolute_range(desired_start, desired_end)
            || self.snapshot_sources_cover_absolute_range(desired_start, desired_end)
    }

    fn retained_rows_cover_absolute_range(&self, desired_start: usize, desired_end: usize) -> bool {
        let Some(first_row) = self.retained_rows.get(&desired_start) else {
            return false;
        };
        let cols = first_row.cells.len();
        if cols == 0 {
            return false;
        }
        (desired_start..desired_end).all(|abs_row| {
            self.retained_rows
                .get(&abs_row)
                .is_some_and(|row| row.cells.len() == cols)
        })
    }

    fn snapshot_sources_cover_absolute_range(
        &self,
        desired_start: usize,
        desired_end: usize,
    ) -> bool {
        let source_cols = self
            .snapshot
            .iter()
            .chain(self.retained_snapshots.iter())
            .filter(|snapshot| {
                terminal_snapshot_absolute_window(snapshot)
                    .is_some_and(|(start, end)| start < desired_end && desired_start < end)
            })
            .map(|snapshot| snapshot.cols)
            .find(|cols| *cols > 0);
        let Some(cols) = source_cols else {
            return false;
        };
        if self
            .snapshot
            .iter()
            .chain(self.retained_snapshots.iter())
            .any(|snapshot| snapshot.cols > 0 && snapshot.cols != cols)
        {
            return false;
        }
        (desired_start..desired_end).all(|abs_row| {
            self.snapshot
                .iter()
                .chain(self.retained_snapshots.iter())
                .any(|snapshot| {
                    terminal_snapshot_row_for_absolute_row(snapshot, abs_row).is_some_and(|row| {
                        snapshot
                            .row(row)
                            .is_some_and(|snapshot_row| snapshot_row.cells.len() == cols)
                    })
                })
        })
    }

    fn synthesize_snapshot_covering_display_offset(
        &self,
        display_offset: usize,
        viewport_rows: usize,
        scrollback_len: usize,
    ) -> Option<Arc<TerminalSnapshot>> {
        let viewport_rows = viewport_rows.max(1);
        let real_total_rows = scrollback_len.saturating_add(viewport_rows);
        let desired_end = real_total_rows.checked_sub(display_offset)?;
        let desired_start = desired_end.checked_sub(viewport_rows)?;
        let extra_rows = terminal_surface_synthesized_window_extra_rows(viewport_rows);
        let retained_window = self.retained_rows_window_around(
            desired_start,
            desired_end,
            real_total_rows,
            extra_rows,
        );
        if let Some((window_start, window_end)) = retained_window
            && let Some(snapshot) = self.synthesize_snapshot_from_retained_rows(
                viewport_rows,
                scrollback_len,
                window_start,
                window_end,
            )
        {
            return Some(snapshot);
        }
        let mut sources: Vec<&Arc<TerminalSnapshot>> = Vec::new();
        if let Some(snapshot) = self.snapshot.as_ref() {
            sources.push(snapshot);
        }
        sources.extend(self.retained_snapshots.iter());

        let cols = sources
            .iter()
            .filter(|snapshot| {
                terminal_snapshot_absolute_window(snapshot)
                    .is_some_and(|(start, end)| start < desired_end && desired_start < end)
            })
            .map(|snapshot| snapshot.cols)
            .find(|cols| *cols > 0)?;
        if sources
            .iter()
            .any(|snapshot| snapshot.cols > 0 && snapshot.cols != cols)
        {
            return None;
        }

        let mut rows = Vec::with_capacity(viewport_rows);

        for abs_row in desired_start..desired_end {
            let (snapshot, row) = sources
                .iter()
                .filter_map(|snapshot| {
                    terminal_snapshot_row_for_absolute_row(snapshot, abs_row)
                        .map(|row| (*snapshot, row))
                })
                .min_by_key(|(snapshot, _)| snapshot.display_offset.abs_diff(display_offset))?;
            rows.push(snapshot.rows().get(row)?.clone());
        }

        Some(Arc::new(TerminalSnapshot::from_rows(
            nyaterm_terminal::TerminalSnapshotMeta {
                cols,
                viewport_rows,
                cursor: hidden_terminal_cursor_snapshot(),
                selection: None,
                scrollback_len,
                total_rows: real_total_rows,
                display_offset,
                images: Vec::new(),
            },
            rows,
        )))
    }

    fn synthesize_snapshot_from_retained_rows(
        &self,
        viewport_rows: usize,
        scrollback_len: usize,
        window_start: usize,
        window_end: usize,
    ) -> Option<Arc<TerminalSnapshot>> {
        let first_row = self.retained_rows.get(&window_start)?;
        let cols = first_row.cells.len();
        let rows = window_end.checked_sub(window_start)?;
        if cols == 0 || rows < viewport_rows.max(1) {
            return None;
        }
        let real_total_rows = scrollback_len.saturating_add(viewport_rows);
        let display_offset = real_total_rows.checked_sub(window_end)?;
        let mut row_data = Vec::with_capacity(rows);

        for abs_row in window_start..window_end {
            let row = self.retained_rows.get(&abs_row)?;
            if row.cells.len() != cols {
                return None;
            }
            row_data.push(Arc::clone(row));
        }
        Some(Arc::new(TerminalSnapshot::from_rows(
            nyaterm_terminal::TerminalSnapshotMeta {
                cols,
                viewport_rows,
                cursor: hidden_terminal_cursor_snapshot(),
                selection: None,
                scrollback_len,
                total_rows: real_total_rows,
                display_offset,
                images: Vec::new(),
            },
            row_data,
        )))
    }

    fn retained_rows_window_around(
        &self,
        desired_start: usize,
        desired_end: usize,
        real_total_rows: usize,
        extra_rows: usize,
    ) -> Option<(usize, usize)> {
        if !self.retained_rows_cover_absolute_range(desired_start, desired_end) {
            return None;
        }
        let cols = self.retained_rows.get(&desired_start)?.cells.len();
        let row_is_compatible = |absolute_row: usize| {
            self.retained_rows
                .get(&absolute_row)
                .is_some_and(|row| row.cells.len() == cols)
        };

        let mut window_start = desired_start;
        while desired_start.saturating_sub(window_start) < extra_rows && window_start > 0 {
            let previous = window_start - 1;
            if !row_is_compatible(previous) {
                break;
            }
            window_start = previous;
        }

        let mut window_end = desired_end;
        while window_end.saturating_sub(desired_end) < extra_rows
            && window_end < real_total_rows
            && row_is_compatible(window_end)
        {
            window_end = window_end.saturating_add(1);
        }
        Some((window_start, window_end))
    }

    pub(in crate::features) fn set_paint_chrome(
        &mut self,
        chrome: TerminalSurfacePaintChrome,
    ) -> bool {
        let TerminalSurfacePaintChrome {
            palette,
            font_family,
            font_fallbacks,
            font_size,
            normal_weight,
            bold_weight,
            bold_default_foreground,
            cell_width,
            cell_height,
            show_line_numbers,
            show_timestamps,
            timestamp_format,
            is_active,
        } = chrome;
        let cell_width = cell_width.max(1.0);
        let cell_height = cell_height.max(1.0);
        let state_matches = self.palette == palette
            && self.font_family == font_family
            && self.font_fallbacks == font_fallbacks
            && (self.font_size - font_size).abs() < f32::EPSILON * 8.0
            && (self.normal_weight - normal_weight).abs() < f32::EPSILON * 8.0
            && (self.bold_weight - bold_weight).abs() < f32::EPSILON * 8.0
            && self.bold_default_foreground == bold_default_foreground
            && (self.cell_width - cell_width).abs() < f32::EPSILON * 8.0
            && (self.cell_height - cell_height).abs() < f32::EPSILON * 8.0
            && self.show_line_numbers == show_line_numbers
            && self.show_timestamps == show_timestamps
            && self.timestamp_format == timestamp_format
            && self.is_active == is_active;
        if state_matches {
            return false;
        }
        self.palette = palette;
        self.font_family = font_family;
        self.font_fallbacks = font_fallbacks;
        self.font_size = font_size;
        self.normal_weight = normal_weight;
        self.bold_weight = bold_weight;
        self.bold_default_foreground = bold_default_foreground;
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.show_line_numbers = show_line_numbers;
        self.show_timestamps = show_timestamps;
        self.timestamp_format = timestamp_format;
        self.is_active = is_active;
        true
    }

    pub(in crate::features) fn set_background_transparent(&mut self, transparent: bool) -> bool {
        if self.transparent_background == transparent {
            return false;
        }
        self.transparent_background = transparent;
        true
    }

    pub(in crate::features) fn set_protocol_state(
        &mut self,
        protocol_state: TerminalProtocolState,
    ) -> bool {
        if self.protocol_state == protocol_state {
            return false;
        }
        self.protocol_state = protocol_state;
        true
    }

    pub(in crate::features) fn set_zebra_stripes(
        &mut self,
        enabled: bool,
        target_line: Option<TerminalLineId>,
    ) -> bool {
        if self.zebra_stripes_enabled == enabled && self.target_line == target_line {
            return false;
        }
        self.zebra_stripes_enabled = enabled;
        self.target_line = target_line;
        true
    }

    pub(in crate::features) fn set_overview_markers(
        &mut self,
        markers: Arc<[crate::features::terminal::terminal_surface::TerminalOverviewMarker]>,
        total_rows: usize,
        key: u64,
    ) -> bool {
        let total_rows = total_rows.max(1);
        if self.overview_marker_key == key
            && self.overview_total_rows == total_rows
            && self.overview_markers.as_ref() == markers.as_ref()
        {
            return false;
        }
        self.overview_markers = markers;
        self.overview_total_rows = total_rows;
        self.overview_marker_key = key;
        self.overview_marker_bucket_key = None;
        self.overview_marker_buckets = Arc::from([]);
        true
    }

    pub(in crate::features) fn overview_marker_key(&self) -> u64 {
        self.overview_marker_key
    }

    fn overview_marker_buckets_for_track_height(
        &mut self,
        track_height_px: usize,
    ) -> Arc<[crate::features::terminal::terminal_surface::TerminalOverviewMarkerBucket]> {
        let bucket_key = (
            self.overview_marker_key,
            self.overview_total_rows,
            track_height_px,
        );
        if self.overview_marker_bucket_key != Some(bucket_key) {
            self.overview_marker_buckets = terminal_overview_marker_buckets(
                &self.overview_markers,
                self.overview_total_rows,
                track_height_px,
            )
            .into();
            self.overview_marker_bucket_key = Some(bucket_key);
        }
        self.overview_marker_buckets.clone()
    }

    pub(in crate::features) fn set_layout_cache(
        &mut self,
        layout_cache: Arc<Mutex<NyaTerminalLayoutCache>>,
    ) -> bool {
        if Arc::ptr_eq(&self.layout_cache, &layout_cache) {
            return false;
        }
        self.layout_cache = layout_cache;
        true
    }

    pub(in crate::features) fn set_decorations_and_keywords(
        &mut self,
        decorations: impl Into<Arc<[TerminalLineDecorations]>>,
        keyword_rules: Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
        show_cursor: bool,
        cursor_style: impl Into<String>,
    ) -> bool {
        let decorations = decorations.into();
        self.apply_decorations_and_keywords(decorations, keyword_rules, show_cursor, cursor_style)
    }

    pub(in crate::features) fn set_decorations_and_keywords_preserving_stale(
        &mut self,
        decorations: impl Into<Arc<[TerminalLineDecorations]>>,
        keyword_rules: Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
        show_cursor: bool,
        cursor_style: impl Into<String>,
        allow_empty_stale: bool,
    ) -> bool {
        let decorations = decorations.into();
        if allow_empty_stale && decorations.is_empty() && !self.decorations.is_empty() {
            let show_cursor = show_cursor && !self.visual_scroll_active();
            let cursor_style = cursor_style.into();
            let rules_changed =
                !terminal_keyword_rule_sets_equal(&self.keyword_rules, &keyword_rules);
            let changed = rules_changed
                || self.show_cursor != show_cursor
                || self.cursor_style != cursor_style;
            if rules_changed {
                self.keyword_highlight_cancel_epoch
                    .fetch_add(1, Ordering::AcqRel);
            }
            self.keyword_rules = keyword_rules;
            self.show_cursor = show_cursor;
            self.cursor_style = cursor_style;
            return changed;
        }
        self.apply_decorations_and_keywords(decorations, keyword_rules, show_cursor, cursor_style)
    }

    fn apply_decorations_and_keywords(
        &mut self,
        decorations: Arc<[TerminalLineDecorations]>,
        keyword_rules: Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
        show_cursor: bool,
        cursor_style: impl Into<String>,
    ) -> bool {
        let show_cursor = show_cursor && !self.visual_scroll_active();
        let cursor_style = cursor_style.into();
        let rules_changed = !terminal_keyword_rule_sets_equal(&self.keyword_rules, &keyword_rules);
        let changed = self.decorations.as_ref() != decorations.as_ref()
            || rules_changed
            || self.show_cursor != show_cursor
            || self.cursor_style != cursor_style;
        if !changed {
            return false;
        }
        if rules_changed {
            self.keyword_highlight_cancel_epoch
                .fetch_add(1, Ordering::AcqRel);
        }
        self.decorations = decorations;
        self.keyword_rules = keyword_rules;
        self.show_cursor = show_cursor;
        self.cursor_style = cursor_style;
        true
    }

    pub(in crate::features) fn schedule_keyword_highlights(
        &mut self,
        clear_if_empty: bool,
        output_pressure: bool,
        cx: &mut Context<Self>,
    ) {
        self.keyword_highlight_output_pressure = output_pressure;
        if !output_pressure {
            self.keyword_highlight_deferred_task = None;
        }
        if self.handle_empty_keyword_rules_for_highlights(clear_if_empty) {
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let visible_rows = terminal_keyword_highlight_visible_rows(
            snapshot.as_ref(),
            self.display_offset,
            self.viewport_rows,
            self.scrollback_len,
        );
        let visible_rows =
            terminal_keyword_highlight_expanded_rows(snapshot.as_ref(), visible_rows);
        let rules = self.keyword_rules.clone();
        let rules_key = terminal_keyword_rules_key(rules.as_slice());
        if self.keyword_highlights.as_ref().is_some_and(|highlights| {
            highlights.rules_key() == rules_key
                && highlights.matches_snapshot_rows(
                    snapshot.as_ref(),
                    self.palette,
                    visible_rows.clone(),
                )
        }) {
            if self.keyword_highlight_task.is_none() {
                self.keyword_highlight_pending_key = None;
            }
            return;
        }
        let prefetch_viewports = terminal_keyword_highlight_prefetch_viewports(output_pressure);
        let requested_rows = terminal_keyword_highlight_prefetch_rows(
            snapshot.as_ref(),
            self.display_offset,
            self.viewport_rows,
            self.scrollback_len,
            prefetch_viewports,
        );
        let requested_row_count = requested_rows.len();
        let requested_rows =
            terminal_keyword_highlight_expanded_rows(snapshot.as_ref(), requested_rows);
        let expanded_requested_row_count = requested_rows.len();
        let request_key = terminal_keyword_highlight_request_key(
            snapshot.as_ref(),
            rules_key,
            requested_rows.clone(),
        );
        if self.keyword_highlight_pending_key == Some(request_key)
            && (self.keyword_highlight_task.is_some()
                || self.keyword_highlight_deferred_task.is_some())
        {
            return;
        }
        self.keyword_highlight_pending_key = Some(request_key);
        // Let one parse finish, then have its completion schedule the newest
        // surface snapshot. Replacing the task on every output frame can starve
        // highlighting indefinitely during continuous input.
        if self.keyword_highlight_task.is_some() {
            return;
        }
        if let Some(delay) = terminal_keyword_highlight_pressure_delay(
            output_pressure,
            self.keyword_highlight_last_started_at
                .map(|last_started_at| last_started_at.elapsed()),
        ) {
            self.schedule_deferred_keyword_highlights(clear_if_empty, delay, cx);
            return;
        }
        self.keyword_highlight_generation = self.keyword_highlight_generation.saturating_add(1);
        let generation = self.keyword_highlight_generation;
        let cancel_epoch = self.keyword_highlight_cancel_epoch.clone();
        let request_cancel_epoch = cancel_epoch.load(Ordering::Acquire);
        let now = Instant::now();
        self.keyword_highlight_last_started_at = Some(now);
        // Keep the last published snapshot drawable while the replacement is parsed in the
        // background, matching the editor's stale-until-reparsed behavior.
        let highlighter = self.cached_keyword_highlighter_for_rules(&rules);
        let palette = self.palette;
        let previous_highlights = self.keyword_highlights.clone();
        self.keyword_highlight_task = Some(cx.spawn(async move |this, cx| {
            let (rules, highlighter, result, highlight_duration) = cx
                .background_spawn(async move {
                    let highlighter = highlighter.unwrap_or_else(|| {
                        Arc::new(compile_terminal_keyword_highlighter(rules.as_ref()))
                    });
                    let highlight_started_at = Instant::now();
                    let result =
                        precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel(
                            snapshot.as_ref(),
                            highlighter.as_ref(),
                            palette,
                            previous_highlights.as_deref(),
                            requested_rows,
                            || cancel_epoch.load(Ordering::Acquire) != request_cancel_epoch,
                        );
                    let highlight_duration = highlight_started_at.elapsed();
                    (rules, highlighter, result, highlight_duration)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.keyword_highlight_generation != generation {
                    return;
                }
                this.keyword_highlight_task = None;
                this.keyword_highlight_pending_key = None;
                let Some((highlights, stats)) = result else {
                    this.schedule_keyword_highlights(
                        clear_if_empty,
                        this.keyword_highlight_output_pressure,
                        cx,
                    );
                    return;
                };
                let publish = terminal_keyword_rule_sets_equal(&this.keyword_rules, &rules);
                if publish {
                    if highlight_duration >= TERMINAL_KEYWORD_HIGHLIGHT_SLOW {
                        tracing::warn!(
                            diagnostic = "terminal_keyword_highlight_slow",
                            session_id = %this.session_id,
                            rules = rules.len(),
                            requested_rows = stats.requested_rows,
                            original_requested_rows = requested_row_count,
                            expanded_rows = expanded_requested_row_count,
                            known_rows = stats.known_rows,
                            range_count = stats.range_count,
                            reused_rows = stats.reused_rows,
                            processed_bytes = stats.processed_bytes,
                            oversized_wrapped_groups = stats.oversized_wrapped_groups,
                            degraded_rows = stats.degraded_rows,
                            match_duration_us = stats.match_duration_us,
                            range_build_duration_us = stats.range_build_duration_us,
                            duration_us = highlight_duration.as_micros(),
                            "slow terminal keyword highlight precompute"
                        );
                    }
                    this.keyword_highlighter_rules = Some(rules);
                    this.keyword_highlighter = Some(highlighter);
                    this.keyword_highlights = Some(Arc::new(highlights));
                    cx.notify();
                }
                // The surface may have advanced while this task was running.
                // Schedule exactly one follow-up for its latest snapshot.
                this.schedule_keyword_highlights(
                    clear_if_empty,
                    this.keyword_highlight_output_pressure,
                    cx,
                );
            });
        }));
    }

    fn schedule_deferred_keyword_highlights(
        &mut self,
        clear_if_empty: bool,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        if self.keyword_highlight_deferred_task.is_some() {
            return;
        }
        self.keyword_highlight_deferred_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                this.keyword_highlight_deferred_task = None;
                this.schedule_keyword_highlights(
                    clear_if_empty,
                    this.keyword_highlight_output_pressure,
                    cx,
                );
            });
        }));
    }

    fn handle_empty_keyword_rules_for_highlights(&mut self, clear_if_empty: bool) -> bool {
        if !self.keyword_rules.is_empty() {
            return false;
        }
        if clear_if_empty {
            self.cancel_pending_keyword_highlights();
            self.keyword_highlight_last_started_at = None;
            self.keyword_highlight_output_pressure = false;
            self.keyword_highlighter_rules = None;
            self.keyword_highlighter = None;
            self.keyword_highlights = None;
        }
        true
    }

    fn cached_keyword_highlighter_for_rules(
        &self,
        rules: &Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
    ) -> Option<Arc<TerminalKeywordHighlighter>> {
        self.keyword_highlighter_rules
            .as_ref()
            .filter(|cached_rules| terminal_keyword_rule_sets_equal(cached_rules, rules))
            .and(self.keyword_highlighter.clone())
    }

    fn cancel_pending_keyword_highlights(&mut self) {
        if self.keyword_highlight_task.is_none()
            && self.keyword_highlight_deferred_task.is_none()
            && self.keyword_highlight_pending_key.is_none()
        {
            return;
        }
        self.keyword_highlight_generation = self.keyword_highlight_generation.saturating_add(1);
        self.keyword_highlight_cancel_epoch
            .fetch_add(1, Ordering::AcqRel);
        self.keyword_highlight_task = None;
        self.keyword_highlight_deferred_task = None;
        self.keyword_highlight_pending_key = None;
    }

    pub(in crate::features) fn set_selection_visual(
        &mut self,
        selection: Option<TerminalSelection>,
    ) -> bool {
        if self.selection_visual == selection {
            return false;
        }
        self.selection_visual = selection;
        self.revision = self.revision.saturating_add(1);
        true
    }

    fn visual_scroll_active(&self) -> bool {
        terminal_visual_scroll_active_for_state(self.scroll_offset, self.scroll_residual_lines)
    }

    pub(in crate::features) fn scroll_visual_state_matches(
        &self,
        state: &TerminalScrollVisualState,
    ) -> bool {
        self.scroll_offset == state.scroll_offset
            && (self.scroll_residual_lines - state.scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.display_offset == state.display_offset
            && self.scrollback_len == state.scrollback_len
            && self.viewport_rows == state.viewport_rows
            && self.has_new_while_scrolled == state.has_new_while_scrolled
            && self.performance_overlay == state.performance_overlay
            && self.skipped_output_chars == state.skipped_output_chars
    }

    fn scroll_position_state_matches(&self, state: &TerminalScrollVisualState) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            terminal_snapshot_covers_display_offset(
                snapshot.as_ref(),
                state.display_offset,
                state.viewport_rows,
                state.scrollback_len,
            )
        }) && self.scroll_visual_state_matches(state)
            && self.display_offset == state.display_offset
            && !self.scroll_snapshot_pending
            && (!self.visual_scroll_active() || !self.show_cursor)
    }

    fn scroll_chrome_state_matches(&self, state: &TerminalScrollVisualState) -> bool {
        self.scroll_offset == state.scroll_offset
            && (self.scroll_residual_lines - state.scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.scrollback_len == state.scrollback_len
            && self.viewport_rows == state.viewport_rows.max(1)
            && self.has_new_while_scrolled == state.has_new_while_scrolled
            && self.performance_overlay == state.performance_overlay
            && self.skipped_output_chars == state.skipped_output_chars
            && if self.snapshot.is_some() {
                self.scroll_snapshot_pending == (self.display_offset != state.display_offset)
            } else {
                self.display_offset == state.display_offset && !self.scroll_snapshot_pending
            }
            && !self.show_cursor
    }

    pub(in crate::features) fn has_pending_local_scroll_sync(&self) -> bool {
        self.pending_local_scroll_sync.is_some()
    }

    fn defer_surface_repaint(
        app: Entity<NyaTermApp>,
        session_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.defer(move |cx| {
            app.update(cx, |this, cx| {
                this.notify_terminal_scroll_after_state_change(session_id.as_deref(), cx);
            });
        });
    }

    fn defer_local_scroll_snapshot_requests(
        app: Entity<NyaTermApp>,
        session_id: String,
        request_offsets: Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() || request_offsets.is_empty() {
            return;
        }
        cx.defer(move |cx| {
            app.update(cx, |this, _cx| {
                for request_offset in request_offsets {
                    this.request_terminal_frame_snapshot_for_user_scroll(
                        session_id.as_str(),
                        request_offset,
                    );
                }
            });
        });
    }

    pub(in crate::features) fn apply_scroll_visual_state(
        &mut self,
        state: TerminalScrollVisualState,
    ) -> bool {
        if self.has_snapshot_covering_display_offset(
            state.display_offset,
            state.viewport_rows,
            state.scrollback_len,
        ) {
            if self.scroll_position_state_matches(&state) {
                return false;
            }
            self.update_scroll_position_without_snapshot(&state);
            true
        } else {
            if self.scroll_chrome_state_matches(&state) {
                return false;
            }
            self.update_scroll_chrome_without_snapshot(&state);
            false
        }
    }

    fn scroll_visual_state_needs_repaint(&self, state: &TerminalScrollVisualState) -> bool {
        if self.has_snapshot_covering_display_offset(
            state.display_offset,
            state.viewport_rows,
            state.scrollback_len,
        ) {
            !self.scroll_position_state_matches(state)
        } else {
            !self.scroll_chrome_state_matches(state)
        }
    }

    fn handle_scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(app) = self.app.as_ref().and_then(WeakEntity::upgrade) else {
            return;
        };
        let session_id = self.session_id.clone();
        if session_id.is_empty() {
            return;
        }
        let raw_lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / self.cell_height.max(1.0),
        };
        if !app
            .read(cx)
            .terminal_selection_dragging_for_session(&session_id)
            && self.can_handle_scroll_wheel_locally()
            && let Some(result) = self.apply_local_scroll_wheel_visual_state(raw_lines)
        {
            if result.visual_changed {
                let state = self.current_scroll_visual_state();
                self.schedule_keyword_highlights(false, false, cx);
                cx.notify();
                let mut request_offsets = Vec::new();
                if result.needs_text_snapshot && state.display_offset > 0 {
                    request_offsets.push(state.display_offset);
                }
                if let Some(prefetch_offset) = terminal_surface_fractional_prefetch_offset(
                    state.scroll_offset,
                    state.scroll_residual_lines,
                    state.scrollback_len,
                ) {
                    request_offsets.push(prefetch_offset);
                }
                request_offsets.sort_unstable();
                request_offsets.dedup();
                let request_offsets =
                    self.scroll_snapshot_request_offsets_to_enqueue(request_offsets);
                if !request_offsets.is_empty() {
                    Self::defer_local_scroll_snapshot_requests(
                        app.clone(),
                        state.session_id.clone(),
                        request_offsets,
                        cx,
                    );
                }
                self.queue_local_scroll_app_sync(app, state, result.generation, cx);
            }
            cx.stop_propagation();
            return;
        }

        // Protocol-owned wheel handling needs the surface's hit-test geometry. The
        // current callback still holds the TerminalSurface update lease, so the
        // parent app must not be entered until that lease is released; otherwise
        // GPUI detects entity reentrancy at `surface.read(cx)` and panics.
        if !raw_lines.is_finite() || raw_lines == 0.0 {
            return;
        }
        let surface = cx.entity();
        let position = event.position;
        let modifiers = event.modifiers;
        cx.stop_propagation();
        cx.defer(move |cx| {
            let result = app.update(cx, |this, cx| {
                this.terminal_scroll_wheel_state_for_session(
                    session_id.as_str(),
                    raw_lines,
                    position,
                    modifiers,
                    cx,
                )
            });
            surface.update(cx, |surface, cx| {
                surface.apply_scroll_wheel_result(app, session_id, result, cx);
            });
        });
    }

    fn apply_scroll_wheel_result(
        &mut self,
        app: Entity<NyaTermApp>,
        session_id: String,
        result: TerminalScrollWheelStateResult,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = result.visual_state {
            self.pending_local_scroll_sync = None;
            self.scroll_interaction_generation =
                self.scroll_interaction_generation.saturating_add(1);
            let repaint_needed = self.scroll_visual_state_needs_repaint(&state);
            let text_updated = self.apply_scroll_visual_state(state.clone());
            let needs_text_first_repaint =
                terminal_scroll_needs_text_first_repaint(&state, text_updated);
            if repaint_needed {
                cx.notify();
            }
            if needs_text_first_repaint {
                let session_id = session_id.clone();
                let app = app.clone();
                cx.defer(move |cx| {
                    app.update(cx, |this, cx| {
                        this.notify_terminal_scroll_position_only(session_id.as_str(), cx);
                    });
                });
            }
        }
        if result.defer_repaint {
            Self::defer_surface_repaint(app, Some(session_id), cx);
        }
    }

    fn can_handle_scroll_wheel_locally(&self) -> bool {
        !self.protocol_state.mouse_reporting
            && self.protocol_state.alternate_scroll_payload(1).is_none()
    }

    fn apply_local_scroll_wheel_visual_state(
        &mut self,
        raw_lines: f32,
    ) -> Option<TerminalSurfaceLocalScrollResult> {
        if raw_lines == 0.0 || !raw_lines.is_finite() {
            return None;
        }
        let (delta_lines, next_residual) = terminal_local_scroll_delta_lines_from_state(
            self.scroll_offset,
            self.scroll_residual_lines,
            self.scrollback_len,
            raw_lines,
        );
        if delta_lines == 0 && next_residual == self.scroll_residual_lines {
            return Some(TerminalSurfaceLocalScrollResult {
                generation: self.scroll_interaction_generation,
                visual_changed: false,
                needs_text_snapshot: false,
            });
        }

        let next_offset = if delta_lines > 0 {
            self.scroll_offset.saturating_add(delta_lines as usize)
        } else {
            self.scroll_offset.saturating_sub((-delta_lines) as usize)
        }
        .min(self.scrollback_len);
        let display_offset =
            terminal_display_offset_from_state(next_offset, next_residual, self.scrollback_len);
        let state = TerminalScrollVisualState {
            session_id: self.session_id.clone(),
            scroll_offset: next_offset,
            scroll_residual_lines: next_residual,
            display_offset,
            scrollback_len: self.scrollback_len,
            viewport_rows: self.viewport_rows,
            has_new_while_scrolled: if next_offset == 0 {
                false
            } else {
                self.has_new_while_scrolled
            },
            performance_overlay: self.performance_overlay,
            skipped_output_chars: self.skipped_output_chars,
        };
        let text_updated = self.apply_scroll_visual_state(state);
        self.scroll_interaction_generation = self.scroll_interaction_generation.saturating_add(1);
        Some(TerminalSurfaceLocalScrollResult {
            generation: self.scroll_interaction_generation,
            visual_changed: true,
            needs_text_snapshot: !text_updated
                && self.scroll_snapshot_pending
                && display_offset > 0,
        })
    }

    fn current_scroll_visual_state(&self) -> TerminalScrollVisualState {
        TerminalScrollVisualState {
            session_id: self.session_id.clone(),
            scroll_offset: self.scroll_offset,
            scroll_residual_lines: self.scroll_residual_lines,
            display_offset: self.display_offset,
            scrollback_len: self.scrollback_len,
            viewport_rows: self.viewport_rows,
            has_new_while_scrolled: self.has_new_while_scrolled,
            performance_overlay: self.performance_overlay,
            skipped_output_chars: self.skipped_output_chars,
        }
    }

    pub(in crate::features) fn take_scroll_state_for_selection(
        &mut self,
    ) -> Option<TerminalScrollVisualState> {
        self.pending_local_scroll_sync.take()?;
        self.scroll_interaction_generation = self.scroll_interaction_generation.saturating_add(1);
        Some(self.current_scroll_visual_state())
    }

    fn queue_local_scroll_app_sync(
        &mut self,
        app: Entity<NyaTermApp>,
        state: TerminalScrollVisualState,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.remember_pending_local_scroll_sync(state, generation) {
            return;
        }
        let surface = cx.entity();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(TERMINAL_SURFACE_LOCAL_SCROLL_SYNC_DELAY)
                .await;
            surface.update(cx, |surface, cx| {
                surface.flush_local_scroll_app_sync(app, cx);
            });
        })
        .detach();
    }

    fn remember_pending_local_scroll_sync(
        &mut self,
        state: TerminalScrollVisualState,
        generation: u64,
    ) -> bool {
        self.pending_local_scroll_sync =
            Some(TerminalSurfacePendingScrollSync { state, generation });
        if self.local_scroll_sync_armed {
            return false;
        }
        self.local_scroll_sync_armed = true;
        true
    }

    fn flush_local_scroll_app_sync(&mut self, app: Entity<NyaTermApp>, cx: &mut Context<Self>) {
        self.local_scroll_sync_armed = false;
        let Some(pending) = self.pending_local_scroll_sync.take() else {
            return;
        };
        let state = app.update(cx, |this, cx| {
            this.sync_terminal_local_scroll_visual_state_from_surface(pending.state, cx)
        });
        if self.scroll_interaction_generation != pending.generation {
            return;
        }
        if let Some(state) = state {
            let repaint_needed = self.scroll_visual_state_needs_repaint(&state);
            let text_updated = self.apply_scroll_visual_state(state.clone());
            let text_first_repaint_ready = terminal_surface_text_first_repaint_ready(
                &state,
                text_updated,
                app.update(cx, |this, _cx| {
                    this.terminal_scroll_text_cached_for_session(
                        state.session_id.as_str(),
                        state.display_offset,
                    )
                }),
            );
            if text_first_repaint_ready {
                let session_id = state.session_id.clone();
                // The app notification may read/update this surface. Wait until
                // the current entity update has released its GPUI lease.
                cx.defer(move |cx| {
                    app.update(cx, |this, cx| {
                        this.notify_terminal_scroll_position_only(session_id.as_str(), cx);
                    });
                });
            }
            if repaint_needed {
                cx.notify();
            }
        }
    }

    fn scrollbar_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let session_id = self.session_id.clone();
        let is_active = self.is_active;
        let scroll_offset = self.scroll_offset;
        let max = self.scrollback_len;
        let viewport_rows = self.viewport_rows.max(1);
        let show = max > 0;
        let app = self.app.as_ref().and_then(WeakEntity::upgrade);
        let track_bounds = app.as_ref().and_then(|app| {
            app.read(cx)
                .terminal_scrollbar_track_bounds_for_session(Some(session_id.as_str()))
        });
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows,
            scrollback_rows: max,
            scroll_offset,
            track_height: track_bounds.map(track_height).unwrap_or(1.0),
            min_thumb_height: TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
        });
        let track_id = format!("terminal-scrollbar-track-{session_id}");
        let thumb_id = format!("terminal-scrollbar-thumb-{session_id}");
        let drag_active = app.as_ref().is_some_and(|app| {
            app.read(cx)
                .terminal
                .view
                .scrollbar_drag
                .as_ref()
                .is_some_and(|drag| drag.session_id.as_deref() == Some(session_id.as_str()))
        });
        let overview_markers = self.overview_markers.clone();
        let overview_total_rows = self.overview_total_rows;
        let overview_track_height_px =
            track_bounds.map(track_height).unwrap_or(1.0).round() as usize;
        let overview_marker_buckets =
            self.overview_marker_buckets_for_track_height(overview_track_height_px);

        div()
            .id(SharedString::from(format!(
                "terminal-scrollbar-{session_id}"
            )))
            .w(px(TERMINAL_SCROLLBAR_COLUMN_WIDTH))
            .flex_none()
            .h_full()
            .py(px(TERMINAL_SCROLLBAR_TRACK_PADDING_Y))
            .pr(px(TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT))
            .opacity(if show { 1.0 } else { 0.35 })
            .child(
                div()
                    .id(SharedString::from(track_id))
                    .relative()
                    .size_full()
                    .border_l_1()
                    .border_color(rgb(palette.border))
                    .bg(rgba(terminal_scrollbar_track_color(palette)))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, {
                        let session_id = session_id.clone();
                        let app = app.clone();
                        cx.listener(move |_this, event: &gpui::MouseDownEvent, _window, cx| {
                            let Some(app) = app.clone() else {
                                return;
                            };
                            let repaint_session_id = app.update(cx, |this, cx| {
                                if !session_id.is_empty() {
                                    this.activate_workspace_pane(session_id.clone(), cx);
                                }
                                let drag_session_id =
                                    (!session_id.is_empty()).then_some(session_id.clone());
                                let bounds = this.terminal_scrollbar_track_bounds_for_session(
                                    drag_session_id.as_deref(),
                                )?;
                                let metrics = this.terminal_scrollbar_metrics_for_session(
                                    drag_session_id.as_deref(),
                                    bounds,
                                );
                                let grab_offset_y = terminal_scrollbar_grab_offset_for_pointer(
                                    f32::from(event.position.y),
                                    f32::from(bounds.origin.y),
                                    metrics,
                                );
                                let mut repaint_session_id = this
                                    .begin_terminal_scrollbar_drag_state_only(
                                        drag_session_id.clone(),
                                        grab_offset_y,
                                    );
                                let offset = terminal_scroll_offset_from_pointer(
                                    f32::from(event.position.y),
                                    f32::from(bounds.origin.y),
                                    metrics,
                                    grab_offset_y,
                                    this.terminal_scroll_max_for_session(
                                        drag_session_id.as_deref(),
                                    ),
                                );
                                repaint_session_id = this
                                    .set_terminal_scroll_offset_for_session_state_only(
                                        drag_session_id.as_deref(),
                                        offset,
                                    )
                                    .or(repaint_session_id);
                                repaint_session_id
                            });
                            Self::defer_surface_repaint(app, repaint_session_id, cx);
                            cx.stop_propagation();
                        })
                    })
                    .when_some(app.clone(), |this, app| {
                        this.child(terminal_scrollbar_track_bounds_tracker(
                            app,
                            Some(session_id.clone()),
                        ))
                    })
                    .child(terminal_overview_marker_canvas(
                        overview_markers,
                        overview_total_rows,
                        overview_track_height_px,
                        overview_marker_buckets,
                        palette,
                    ))
                    .when(show, |this| {
                        this.child(terminal_scrollbar_thumb_element(
                            SharedString::from(thumb_id),
                            metrics,
                            palette,
                            is_active,
                            drag_active,
                        ))
                    }),
            )
    }
}

pub(in crate::features) fn terminal_visual_scroll_offset_px(
    target_offset: usize,
    displayed_offset: usize,
    residual_lines: f32,
    cell_height: f32,
) -> f32 {
    terminal_visual_scroll_line_delta(target_offset, displayed_offset, residual_lines)
        * cell_height.max(1.0)
}

fn terminal_visual_scroll_line_delta(
    target_offset: usize,
    displayed_offset: usize,
    residual_lines: f32,
) -> f32 {
    let line_delta = target_offset as isize - displayed_offset as isize;
    line_delta as f32 + residual_lines
}

fn terminal_surface_text_first_repaint_ready(
    state: &TerminalScrollVisualState,
    text_updated: bool,
    text_snapshot_cached: bool,
) -> bool {
    terminal_scroll_needs_text_first_repaint(state, text_updated) && text_snapshot_cached
}

fn terminal_retained_visual_scroll_line_bounds(
    viewport_anchor_row: usize,
    snapshot_rows: usize,
    viewport_rows: usize,
) -> (f32, f32) {
    let viewport_rows = viewport_rows.max(1);
    let older_rows = viewport_anchor_row.min(snapshot_rows);
    let newer_rows = snapshot_rows.saturating_sub(
        viewport_anchor_row
            .saturating_add(viewport_rows)
            .min(snapshot_rows),
    );
    (-(newer_rows as f32), older_rows as f32)
}

pub(in crate::features) fn terminal_effective_visual_scroll_offset_px(
    geometry: TerminalVisualScrollGeometry,
) -> f32 {
    let TerminalVisualScrollGeometry {
        snapshot_pending,
        target_offset,
        displayed_offset,
        residual_lines,
        viewport_anchor_row,
        snapshot_rows,
        viewport_rows,
        cell_height,
    } = geometry;
    if !snapshot_pending {
        return terminal_visual_scroll_offset_px(
            target_offset,
            displayed_offset,
            residual_lines,
            cell_height,
        );
    }
    let (min_lines, max_lines) = terminal_retained_visual_scroll_line_bounds(
        viewport_anchor_row,
        snapshot_rows,
        viewport_rows,
    );
    terminal_visual_scroll_line_delta(target_offset, displayed_offset, residual_lines)
        .clamp(min_lines, max_lines)
        * cell_height.max(1.0)
}

pub(in crate::features) fn terminal_snapshot_covers_display_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> bool {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = scrollback_len.saturating_add(viewport_rows);
    let Some((snapshot_start, snapshot_end)) = terminal_snapshot_absolute_window(snapshot) else {
        return false;
    };
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    snapshot_start <= desired_start && desired_end <= snapshot_end
}

pub(in crate::features) fn terminal_snapshot_anchor_row_for_display_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> usize {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = scrollback_len.saturating_add(viewport_rows);
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    terminal_snapshot_absolute_window(snapshot)
        .map(|(snapshot_start, _)| desired_start.saturating_sub(snapshot_start))
        .unwrap_or(0)
}

fn terminal_keyword_highlight_visible_rows(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> Range<usize> {
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        snapshot,
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    let start = anchor.saturating_sub(1).min(snapshot.row_count());
    let end = anchor
        .saturating_add(viewport_rows.max(1))
        .saturating_add(1)
        .min(snapshot.row_count())
        .max(start);
    start..end
}

fn terminal_keyword_highlight_prefetch_rows(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
    prefetch_viewports: usize,
) -> Range<usize> {
    let viewport_rows = viewport_rows.max(1);
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        snapshot,
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    let extra_rows = viewport_rows.saturating_mul(prefetch_viewports);
    let start = anchor
        .saturating_sub(extra_rows)
        .saturating_sub(1)
        .min(snapshot.row_count());
    let end = anchor
        .saturating_add(viewport_rows)
        .saturating_add(extra_rows)
        .saturating_add(1)
        .min(snapshot.row_count())
        .max(start);
    start..end
}

fn hidden_terminal_cursor_snapshot() -> nyaterm_terminal::CursorSnapshot {
    nyaterm_terminal::CursorSnapshot {
        row: usize::MAX,
        col: 0,
        shape: nyaterm_terminal::CursorShape::Hidden,
        visible: false,
        blinking: false,
    }
}

fn terminal_surface_synthesized_window_extra_rows(viewport_rows: usize) -> usize {
    viewport_rows.max(1).saturating_mul(2).clamp(
        TERMINAL_SURFACE_SYNTHESIZED_WINDOW_MIN_EXTRA_ROWS,
        TERMINAL_SURFACE_SYNTHESIZED_WINDOW_MAX_EXTRA_ROWS,
    )
}

fn terminal_snapshot_row_for_absolute_row(
    snapshot: &TerminalSnapshot,
    absolute_row: usize,
) -> Option<usize> {
    let (start, end) = terminal_snapshot_absolute_window(snapshot)?;
    if absolute_row < start || absolute_row >= end {
        return None;
    }
    Some(absolute_row - start)
}

fn terminal_snapshot_absolute_window(snapshot: &TerminalSnapshot) -> Option<(usize, usize)> {
    if snapshot.row_count() == 0 {
        return None;
    }
    let end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let start = end.saturating_sub(snapshot.row_count());
    Some((start, end))
}

fn terminal_surface_visible_rows_for_viewport(
    viewport_rows: usize,
    snapshot_rows: usize,
    visual_y_offset: f32,
    cell_height: f32,
) -> Range<usize> {
    if snapshot_rows == 0 {
        return 0..0;
    }
    let viewport_height = viewport_rows.max(1) as f32 * cell_height.max(1.0);
    let cell_height = cell_height.max(1.0);
    let overscan_rows = 1usize;
    let visible_start = ((-visual_y_offset) / cell_height).floor().max(0.0) as usize;
    let visible_end = ((viewport_height - visual_y_offset) / cell_height)
        .ceil()
        .max(0.0) as usize;
    let start = visible_start
        .saturating_sub(overscan_rows)
        .min(snapshot_rows);
    let end = visible_end.saturating_add(overscan_rows).min(snapshot_rows);
    if end < start {
        return start..start;
    }
    start..end
}

fn terminal_surface_fractional_prefetch_offset(
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
) -> Option<usize> {
    if scrollback_len == 0 || residual_lines == 0.0 || !residual_lines.is_finite() {
        return None;
    }
    if residual_lines > 0.0 {
        return scroll_offset
            .checked_add(1)
            .filter(|offset| *offset <= scrollback_len && *offset > 0);
    }
    scroll_offset.checked_sub(1).filter(|offset| *offset > 0)
}

impl Render for TerminalSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        TERMINAL_SURFACE_PAINT_COUNT.fetch_add(1, Ordering::Relaxed);
        let palette = self.palette;
        let cell_w = self.cell_width.max(1.0);
        let cell_h = self.cell_height.max(1.0);
        let snapshot = self
            .snapshot
            .clone()
            .unwrap_or_else(|| Arc::new(TerminalScreen::default().viewport_snapshot(0)));
        self.maybe_log_scroll_snapshot_pending(snapshot.as_ref());
        let viewport_anchor_row = terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            self.display_offset,
            self.viewport_rows,
            self.scrollback_len,
        );
        let visual_y_offset =
            terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
                snapshot_pending: self.scroll_snapshot_pending,
                target_offset: self.scroll_offset,
                displayed_offset: self.display_offset,
                residual_lines: self.scroll_residual_lines,
                viewport_anchor_row,
                snapshot_rows: snapshot.row_count(),
                viewport_rows: self.viewport_rows,
                cell_height: cell_h,
            }) - viewport_anchor_row as f32 * cell_h;
        let gutter_enabled = self.show_line_numbers || self.show_timestamps;
        let previous_grid_bounds = self
            .painted_hit_test_geometry
            .and_then(|geometry| geometry.grid_bounds);
        self.painted_hit_test_geometry = Some(TerminalPaintedHitTestGeometry {
            grid_bounds: previous_grid_bounds,
            display_offset: self.display_offset,
            viewport_anchor_row,
            snapshot_rows: snapshot.row_count(),
            viewport_rows: self.viewport_rows,
            visual_y_offset,
            cell_width: cell_w,
            cell_height: cell_h,
            revision: self.revision,
        });
        self.painted_hit_test_snapshot = Some(snapshot.clone());
        let line_count = snapshot.row_count();
        let visible_gutter_rows = terminal_surface_visible_rows_for_viewport(
            self.viewport_rows,
            line_count,
            visual_y_offset,
            cell_h,
        );
        let performance_overlay = self.performance_overlay;
        let skipped_output_chars = self.skipped_output_chars;
        let mut surface_font = font(SharedString::from(self.font_family.clone()));
        surface_font.fallbacks = self.font_fallbacks.clone();
        surface_font.features = terminal_font_features();
        let app = self.app.as_ref().and_then(WeakEntity::upgrade);
        let surface = cx.entity();
        let session_id = self.session_id.clone();
        let is_active = self.is_active;
        let zebra_stripes_visible = self.zebra_stripes_enabled
            && !self.protocol_state.alternate_screen
            && !self.protocol_state.mouse_reporting;
        let target_line = self.target_line;
        let mut grid = NyaTerminalElement::new(
            snapshot.clone(),
            empty_terminal_keyword_rules(),
            self.decorations.clone(),
            self.show_cursor,
            self.cursor_style.clone(),
            cell_w,
            cell_h,
            palette,
            self.font_family.clone(),
            self.font_size,
            self.normal_weight,
            self.bold_weight,
        );
        grid = grid
            .with_bold_default_foreground(self.bold_default_foreground)
            .with_selection(self.selection_visual.map(|selection| {
                TerminalGridSelection::new(
                    selection.anchor.line,
                    selection.anchor.col,
                    selection.head.line,
                    selection.head.col,
                    selection.all_buffer,
                )
            }))
            .with_font_fallbacks(self.font_fallbacks.clone())
            .with_layout_cache(self.layout_cache.clone())
            .with_layout_rows(self.viewport_rows)
            .with_fill_height(true)
            .with_visual_y_offset(visual_y_offset)
            .with_zebra_stripes(zebra_stripes_visible, target_line);
        if let Some(highlights) = self.keyword_highlights.clone() {
            grid = grid.with_keyword_highlights(highlights);
        }

        let gutter = if gutter_enabled {
            let gutter_metrics = terminal_gutter_metrics(
                cell_w,
                self.show_timestamps,
                terminal_timestamp_format_width_chars(&self.timestamp_format),
                self.show_line_numbers,
                terminal_line_number_digits(snapshot.as_ref()),
            );
            let timestamp_formatter = TerminalTimestampFormatter::new(&self.timestamp_format);
            let line_number_digits = terminal_line_number_digits(snapshot.as_ref());
            let ts_w = gutter_metrics.timestamp_width;
            let ln_w = gutter_metrics.line_number_width;
            let gutter_viewport_width = (gutter_metrics.total_width() - 10.0).max(1.0);
            let abs_start = snapshot
                .total_rows
                .saturating_sub(snapshot.display_offset)
                .saturating_sub(snapshot.row_count());
            let gutter_y_offset = visual_y_offset + visible_gutter_rows.start as f32 * cell_h;
            let mut gutter_rows = div()
                .absolute()
                .left_0()
                .top(px(gutter_y_offset))
                .flex()
                .flex_col();
            for line_index in visible_gutter_rows {
                let snapshot_row = snapshot.row(line_index);
                let zebra_color = zebra_stripes_visible
                    .then(|| {
                        snapshot_row.and_then(|row| {
                            if row.line_id.is_some() && row.line_id == target_line {
                                Some(rgba((palette.accent << 8) | 0x24))
                            } else if row.shell_input.is_some() {
                                Some(rgba((palette.terminal_fg << 8) | 0x0f))
                            } else {
                                None
                            }
                        })
                    })
                    .flatten();
                let labels = terminal_gutter_labels(
                    snapshot_row,
                    abs_start + line_index + 1,
                    self.show_timestamps,
                    self.show_line_numbers,
                    line_number_digits,
                    &timestamp_formatter,
                );
                gutter_rows = gutter_rows.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .min_h(px(cell_h))
                        .gap(px(gutter_metrics.gap_width))
                        .flex_none()
                        .pr(px(8.))
                        .text_color(rgb(palette.text_dimmed))
                        .when_some(zebra_color, |this, color| this.bg(color))
                        .font(surface_font.clone())
                        .text_size(px(self.font_size))
                        .when(self.show_timestamps, |this| {
                            this.child(div().w(px(ts_w)).flex_none().child(labels.timestamp))
                        })
                        .when(self.show_line_numbers, |this| {
                            this.child(div().w(px(ln_w)).flex_none().child(labels.line_number))
                        }),
                );
            }
            Some(
                div()
                    .relative()
                    .h_full()
                    .min_h_0()
                    .w(px(gutter_viewport_width))
                    .flex_none()
                    .mr(px(10.))
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(rgb(palette.border))
                    .child(gutter_rows),
            )
        } else {
            None
        };

        let body = if let Some(gutter) = gutter {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(gutter)
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(grid)
                        .child(terminal_surface_grid_bounds_tracker(surface.clone())),
                )
        } else {
            div().flex().flex_row().flex_1().min_w_0().min_h_0().child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(grid)
                    .child(terminal_surface_grid_bounds_tracker(surface)),
            )
        };

        let scrollbar = self.scrollbar_element(cx).into_any_element();

        div()
            .id(SharedString::from(format!(
                "terminal-surface-{}",
                self.session_id
            )))
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_row()
            .relative()
            .bg(if self.transparent_background {
                rgba(palette.terminal_bg << 8)
            } else {
                rgb(palette.terminal_bg)
            })
            .text_color(rgb(palette.terminal_fg))
            .font(surface_font)
            .text_size(px(self.font_size))
            .when(!self.protocol_state.mouse_reporting, |this| {
                this.cursor_text()
            })
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                this.handle_scroll_wheel(event, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .relative()
                    .overflow_hidden()
                    .child(body)
                    .when_some(app, |this, app| {
                        this.child(terminal_bounds_tracker(
                            app,
                            Some(session_id.clone()),
                            is_active,
                        ))
                    })
                    .when_some(performance_overlay, |this, overlay| {
                        this.child(
                            div()
                                .absolute()
                                .left_2()
                                .top_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(palette.surface_elevated))
                                .border_1()
                                .border_color(rgb(palette.border))
                                .text_xs()
                                .text_color(rgb(palette.text_muted))
                                .child(match overlay {
                                    TerminalPerformanceOverlay::Overloaded => {
                                        format!("protecting output… skipped={skipped_output_chars}")
                                    }
                                    TerminalPerformanceOverlay::Recovered => {
                                        "render recovered".to_string()
                                    }
                                }),
                        )
                    }),
            )
            .child(scrollbar)
    }
}

#[cfg(test)]
mod tests;
