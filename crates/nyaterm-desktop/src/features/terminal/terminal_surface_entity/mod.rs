use gpui::{
    Context, Entity, FontFallbacks, IntoElement, MouseButton, Render, ScrollDelta, ScrollEvent,
    SharedString, WeakEntity, Window, div, font, prelude::*, px, rgb, rgba,
};
use nyaterm_terminal::{TerminalLineId, TerminalScreen, TerminalSnapshot};
pub(in crate::features) use nyaterm_terminal_gpui::geometry::{
    TerminalVisualScrollGeometry, terminal_effective_visual_scroll_offset_px,
    terminal_snapshot_anchor_row_for_display_offset, terminal_snapshot_covers_display_offset,
};
use nyaterm_terminal_gpui::keywords::{
    precompute_terminal_keyword_highlights_for_ranges_with_stats_and_cancel,
    terminal_keyword_highlight_expanded_ranges,
};
use nyaterm_terminal_gpui::stripes::{TerminalStripeMode, terminal_row_stripe_color};

use crate::features::NyaTermApp;
use crate::features::formatting::{
    TerminalTimestampFormatter, terminal_gutter_labels, terminal_gutter_row_count,
    terminal_timestamp_format_width_chars,
};
use crate::features::terminal::terminal_runtime::{
    TerminalScrollVisualState, TerminalScrollWheelStateResult, terminal_display_offset_from_state,
    terminal_local_scroll_delta_lines_from_state, terminal_scroll_needs_text_first_repaint,
    terminal_visual_scroll_active_for_state,
};
use crate::features::terminal::terminal_selection_runtime::{
    terminal_bounds_tracker_with_exclusion, terminal_gutter_metrics, terminal_line_number_digits,
};
use crate::features::terminal::terminal_surface::{
    TERMINAL_SCROLLBAR_COLUMN_WIDTH, TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
    TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT, TERMINAL_SCROLLBAR_TRACK_PADDING_Y,
    TerminalScrollbarInput, terminal_overview_marker_buckets, terminal_overview_marker_canvas,
    terminal_scroll_offset_from_pointer, terminal_scrollbar_grab_offset_for_pointer,
    terminal_scrollbar_metrics, terminal_scrollbar_thumb_element,
    terminal_scrollbar_track_bounds_tracker, terminal_scrollbar_track_color, track_height,
};
use crate::models::{
    TerminalPerformanceOverlay, TerminalProtocolState, TerminalSelection,
    terminal_geometry_diagnostics_enabled,
};
use crate::terminal::{
    NyaTerminalElement, NyaTerminalLayoutCache, TerminalGridSelection,
    TerminalKeywordHighlightConfig, TerminalKeywordHighlightSnapshot, TerminalKeywordHighlighter,
    TerminalLineDecorations, compile_terminal_keyword_highlighter, terminal_font_features,
    terminal_keyword_highlight_config_key,
};
use crate::theme::ThemePalette;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nyaterm_terminal::snapshot_window::terminal_snapshot_absolute_window;

fn terminal_surface_text_first_repaint_ready(
    state: &TerminalScrollVisualState,
    text_updated: bool,
    text_snapshot_cached: bool,
) -> bool {
    terminal_scroll_needs_text_first_repaint(state, text_updated) && text_snapshot_cached
}

mod command_blocks;
mod wheel_zoom;
mod wrapping;

use self::wheel_zoom::TerminalWheelZoom;

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
    let ranges = terminal_keyword_highlight_expanded_ranges(snapshot, &[rows]);
    let rows =
        ranges.first().map_or(0, |range| range.start)..ranges.last().map_or(0, |range| range.end);
    let row_start = rows.start.min(snapshot.row_count());
    let row_end = rows.end.min(snapshot.row_count()).max(row_start);
    let mut hasher = DefaultHasher::new();
    ranges.hash(&mut hasher);
    snapshot.history.alternate_screen.hash(&mut hasher);
    if rows.contains(&snapshot.cursor.row) {
        snapshot.cursor.row.hash(&mut hasher);
    }
    if snapshot.logical_lines.is_some() || snapshot.display_rows.is_some() {
        // The worker owns the full source range. Frame identity is sufficient
        // here and keeps preparing a huge logical line off the UI thread.
        Arc::as_ptr(&snapshot.row_data)
            .cast::<()>()
            .hash(&mut hasher);
    } else {
        for row in ranges
            .iter()
            .flat_map(|range| &snapshot.rows()[range.clone()])
        {
            (
                row.signature,
                row.wrapped,
                row.command_mark,
                row.shell_input,
                row.shell_input_columns,
                row.shell_integration,
            )
                .hash(&mut hasher);
        }
    }
    TerminalKeywordHighlightRequestKey {
        rules_key,
        display_offset: snapshot.display_offset,
        row_start,
        row_end,
        line_signatures_key: hasher.finish(),
    }
}

/// Distance from an absolute row to the retained snapshot's row window.
///
/// Eviction must drop the rows farthest from the text the surface is about to
/// paint, not always the smallest absolute row. Scrolling upward inserts rows
/// below the previous window; dropping the smallest key would evict exactly the
/// rows being fetched and pin the cache to the bottom of the scrollback.
fn terminal_retained_row_window_distance(
    abs_row: usize,
    window_start: usize,
    window_end: usize,
) -> usize {
    let last_row = window_end.saturating_sub(1);
    if abs_row < window_start {
        window_start - abs_row
    } else {
        abs_row.saturating_sub(last_row)
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
    pub minimum_contrast_ratio: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub show_line_numbers: bool,
    pub max_line_number_digits: usize,
    pub show_timestamps: bool,
    pub wrap_mode: nyaterm_core::models::settings::TerminalWrapMode,
    pub show_wrap_markers: bool,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TerminalCursorBlinkBasis {
    pub is_active: bool,
    pub is_disconnected: bool,
    /// Cursor visibility evaluated as if the blink phase were lit.
    pub cursor_visible_when_lit: bool,
    pub blink_enabled: bool,
}

pub(in crate::features) fn terminal_surface_paint_count() -> u64 {
    TERMINAL_SURFACE_PAINT_COUNT.load(Ordering::Relaxed)
}

/// Estimate once when retaining a row; eviction never scans its cells.
#[derive(Clone)]
struct TerminalRetainedRow {
    row: Arc<nyaterm_terminal::TerminalSnapshotRow>,
    estimated_bytes: usize,
}

impl std::ops::Deref for TerminalRetainedRow {
    type Target = Arc<nyaterm_terminal::TerminalSnapshotRow>;
    fn deref(&self) -> &Self::Target {
        &self.row
    }
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
    history: nyaterm_terminal::TerminalHistoryState,
    retained_row_origin: usize,
    retained_rows: BTreeMap<usize, TerminalRetainedRow>,
    retained_row_estimated_bytes: usize,
    keyword_rules: Arc<Vec<nyaterm_core::ResolvedKeywordHighlightRule>>,
    /// Cached `ui_highlight_line_roles`; the highlight path runs inside
    /// `app.update`, where reading the app entity would borrow it twice.
    line_roles: bool,
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
    selection_visual_history: nyaterm_terminal::TerminalHistoryState,
    palette: ThemePalette,
    font_family: String,
    font_fallbacks: Option<FontFallbacks>,
    font_size: f32,
    normal_weight: f32,
    bold_weight: f32,
    bold_default_foreground: bool,
    minimum_contrast_ratio: f32,
    cell_width: f32,
    cell_height: f32,
    show_cursor: bool,
    /// Blink-independent cursor inputs from the last full paint sync, so a blink
    /// half-period can flip the caret without re-running paint preparation.
    cursor_blink_basis: Option<TerminalCursorBlinkBasis>,
    cursor_style: String,
    layout_cache: Arc<Mutex<NyaTerminalLayoutCache>>,
    show_line_numbers: bool,
    max_line_number_digits: usize,
    show_timestamps: bool,
    wrap_mode: nyaterm_core::models::settings::TerminalWrapMode,
    show_wrap_markers: bool,
    horizontal_scroll: gpui::ScrollHandle,
    horizontal_reveal: Option<(TerminalLineId, usize)>,
    logical_scroll_residual_lines: f32,
    last_horizontal_cursor: Option<(TerminalLineId, usize)>,
    last_horizontal_input: Option<Instant>,
    last_horizontal_search: Option<(TerminalLineId, usize)>,
    command_blocks: crate::models::terminal::command_blocks::TerminalCommandBlockState,
    painted_unwrapped: Option<Arc<nyaterm_terminal_gpui::unwrapped::TerminalUnwrappedLayout>>,
    timestamp_format: String,
    scroll_offset: usize,
    scroll_residual_lines: f32,
    wheel_zoom: TerminalWheelZoom,
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
    zebra_stripes_by_row: bool,
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
            history: Default::default(),
            retained_row_origin: 0,
            retained_rows: BTreeMap::new(),
            retained_row_estimated_bytes: 0,
            keyword_rules: Arc::new(Vec::new()),
            line_roles: true,
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
            selection_visual_history: Default::default(),
            palette: crate::theme::theme_palette("github-dark"),
            font_family: "monospace".to_string(),
            font_fallbacks: None,
            font_size: 14.0,
            normal_weight: 400.0,
            bold_weight: 700.0,
            bold_default_foreground: false,
            minimum_contrast_ratio: 1.0,
            cell_width: 8.0,
            cell_height: 16.0,
            show_cursor: false,
            cursor_blink_basis: None,
            cursor_style: "block".to_string(),
            layout_cache: Arc::new(Mutex::new(NyaTerminalLayoutCache::default())),
            show_line_numbers: false,
            max_line_number_digits: 1,
            show_timestamps: false,
            wrap_mode: nyaterm_core::models::settings::TerminalWrapMode::Character,
            show_wrap_markers: false,
            horizontal_scroll: gpui::ScrollHandle::default(),
            horizontal_reveal: None,
            logical_scroll_residual_lines: 0.0,
            last_horizontal_cursor: None,
            last_horizontal_input: None,
            last_horizontal_search: None,
            command_blocks: Default::default(),
            painted_unwrapped: None,
            timestamp_format: nyaterm_core::DEFAULT_TERMINAL_TIMESTAMP_FORMAT.to_string(),
            scroll_offset: 0,
            scroll_residual_lines: 0.0,
            wheel_zoom: TerminalWheelZoom::default(),
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
            zebra_stripes_by_row: false,
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

    pub(in crate::features) fn set_cursor_blink_basis(&mut self, basis: TerminalCursorBlinkBasis) {
        self.cursor_blink_basis = Some(basis);
    }

    /// Apply only a blink phase change. `None` means the recorded basis no longer
    /// describes the surface and the caller must run a full paint sync instead.
    pub(in crate::features) fn apply_cursor_blink_phase(
        &mut self,
        is_active: bool,
        is_disconnected: bool,
        cursor_blink_on: bool,
    ) -> Option<bool> {
        let basis = self.cursor_blink_basis?;
        if basis.is_active != is_active
            || basis.is_disconnected != is_disconnected
            || self.visual_scroll_active()
            || self.has_pending_local_scroll_sync()
        {
            return None;
        }
        let show_cursor =
            basis.cursor_visible_when_lit && (!basis.blink_enabled || cursor_blink_on);
        if self.show_cursor == show_cursor {
            return Some(false);
        }
        self.show_cursor = show_cursor;
        Some(true)
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

    fn snapshot_has_current_coordinates(&self, snapshot: &TerminalSnapshot) -> bool {
        self.history == snapshot.history
            || (self.history.preserves_coordinates_from(snapshot.history)
                && snapshot.total_rows.saturating_sub(snapshot.display_offset)
                    <= snapshot.history.scrollback_len)
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
                self.snapshot_has_current_coordinates(snapshot)
                    && terminal_snapshot_covers_display_offset(
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
                        self.snapshot_has_current_coordinates(snapshot)
                            && terminal_snapshot_covers_display_offset(
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
                self.snapshot_has_current_coordinates(snapshot)
                    && terminal_snapshot_covers_display_offset(
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
                        self.snapshot_has_current_coordinates(snapshot)
                            && terminal_snapshot_covers_display_offset(
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

    pub(in crate::features) fn history_coordinates_match(
        &self,
        history: nyaterm_terminal::TerminalHistoryState,
    ) -> bool {
        history.preserves_coordinates_from(self.history)
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
        let snapshot = if self.history.cols > 0 && snapshot.history != self.history {
            let Some(rebased) = snapshot.reanchored_history(self.history) else {
                return false;
            };
            Arc::new(rebased)
        } else {
            snapshot
        };
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
        if self.history.cols == 0 {
            self.history = snapshot.history;
        }
        self.remember_retained_snapshot(snapshot);
        self.prune_pending_scroll_snapshot_offsets();
        true
    }

    /// Re-apply the line-role setting without recreating the surface.
    pub(in crate::features) fn set_line_roles(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.line_roles == enabled {
            return;
        }
        self.line_roles = enabled;
        self.invalidate_keyword_highlight_config();
        self.schedule_keyword_highlights(true, self.keyword_highlight_output_pressure, cx);
        cx.notify();
    }

    pub(in crate::features) fn set_app(&mut self, app: Entity<NyaTermApp>, line_roles: bool) {
        self.app = Some(app.downgrade());
        // Cached here because the highlight path runs inside `app.update`, where
        // reading the app entity again would trip `RefCell already borrowed`.
        self.line_roles = line_roles;
    }

    pub(in crate::features) fn retain_surviving_prefetched_rows(
        &mut self,
        snapshot: &TerminalSnapshot,
    ) -> bool {
        let Some(range) = snapshot.retained_history_row_range(self.history) else {
            return false;
        };
        let Some(change) = self.history.changes_since(snapshot.history) else {
            return false;
        };
        let Some((start, _)) = terminal_snapshot_absolute_window(snapshot) else {
            return false;
        };
        let start = start + range.start - change.evicted;
        self.remember_retained_rows_with_limits(
            &snapshot.rows()[range],
            start.saturating_add(self.retained_row_origin),
            TERMINAL_SURFACE_RETAINED_ROW_LIMIT,
            TERMINAL_SURFACE_RETAINED_ROW_BYTE_LIMIT,
        ) > 0
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
            history,
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
        self.advance_history(history);
        let retained_rows_should_reset = self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.display_rows != snapshot.display_rows)
            || self.retained_rows_should_reset(
                snapshot.as_ref(),
                history,
                scrollback_len,
                viewport_rows,
            );
        let pending_local_scroll_state =
            self.pending_local_scroll_state_reanchored_for_frame(&scroll);
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
        self.history = history;
        if let Some(state) = pending_local_scroll_state {
            if retained_rows_should_reset {
                self.display_offset = snapshot.display_offset;
                self.snapshot = Some(snapshot.clone());
            }
            let text_updated = self.apply_scroll_visual_state(state);
            if text_updated {
                self.show_cursor = false;
            }
            return false;
        }
        self.snapshot = Some(snapshot);
        self.history = history;
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
        // Frames publish text immediately. Keyword compilation and parsing are
        // scheduled separately, so input never waits for a highlight cache miss.
        true
    }

    pub(in crate::features) fn pending_local_scroll_state_reanchored_for_frame(
        &self,
        frame: &TerminalScrollVisualState,
    ) -> Option<TerminalScrollVisualState> {
        let pending = self.pending_local_scroll_sync.as_ref()?;
        let mut state = pending.state.clone();
        if state.history == frame.history
            && state.scroll_offset == frame.scroll_offset
            && state.display_offset == frame.display_offset
            && (state.scroll_residual_lines - frame.scroll_residual_lines).abs()
                < f32::EPSILON * 8.0
        {
            return None;
        }
        frame.history.scrolled_since(state.history)?;
        if state.scroll_offset > 0 {
            state.scroll_offset = frame
                .history
                .reanchor_offset(state.history, state.scroll_offset);
            if state.scroll_offset >= frame.scrollback_len && state.scroll_residual_lines > 0.0 {
                state.scroll_residual_lines = 0.0;
            }
        }
        state.history = frame.history;
        state.scrollback_len = frame.scrollback_len;
        state.viewport_rows = frame.viewport_rows.max(1);
        state.display_offset = terminal_display_offset_from_state(
            state.scroll_offset,
            state.scroll_residual_lines,
            state.scrollback_len,
        );
        state.has_new_while_scrolled = terminal_visual_scroll_active_for_state(
            state.scroll_offset,
            state.scroll_residual_lines,
        ) && (frame.has_new_while_scrolled
            || state.has_new_while_scrolled);
        state.performance_overlay = frame.performance_overlay;
        state.skipped_output_chars = frame.skipped_output_chars;
        Some(state)
    }

    /// Rebase the small snapshot catalog and lazily translate retained row keys.
    /// The last paint remains intact if its window/index needs a worker refresh.
    pub(in crate::features) fn advance_history(
        &mut self,
        history: nyaterm_terminal::TerminalHistoryState,
    ) {
        if history == self.history {
            return;
        }
        if self.history.cols == 0 {
            self.history = history;
            self.selection_visual_history = history;
            return;
        }
        let Some(change) = history.changes_since(self.history) else {
            self.clear_retained_scroll_state();
            self.snapshot = None;
            self.history = history;
            return;
        };
        if change.evicted > 0
            && !self.visual_scroll_active()
            && !self.has_pending_local_scroll_sync()
        {
            // Following live output does not need a growing history-row cache.
            // Keep paint/selection while dropping query state without rescanning cells.
            self.retained_snapshots.clear();
            self.retained_rows.clear();
            self.retained_row_origin = 0;
            self.retained_row_estimated_bytes = 0;
            self.pending_scroll_snapshot_offsets.clear();
        } else if change.scrolled > 0 {
            let Some(origin) = self.retained_row_origin.checked_add(change.evicted) else {
                self.clear_retained_scroll_state();
                self.history = history;
                return;
            };
            // Old live rows are mutable and cannot be promoted into immutable history.
            let live_start = self
                .retained_row_origin
                .saturating_add(self.history.scrollback_len);
            let live = self.retained_rows.split_off(&live_start);
            for row in live.values() {
                self.retained_row_estimated_bytes = self
                    .retained_row_estimated_bytes
                    .saturating_sub(row.estimated_bytes);
            }
            self.retained_row_origin = origin;
            while self
                .retained_rows
                .first_key_value()
                .is_some_and(|(key, _)| *key < origin)
            {
                if let Some((_, row)) = self.retained_rows.pop_first() {
                    self.retained_row_estimated_bytes = self
                        .retained_row_estimated_bytes
                        .saturating_sub(row.estimated_bytes);
                }
            }
            self.retained_snapshots = self
                .retained_snapshots
                .drain(..)
                .filter_map(|snapshot| snapshot.reanchored_history(history).map(Arc::new))
                .collect();
            if let Some(snapshot) = self.snapshot.as_ref()
                && let Some(rebased) = snapshot.reanchored_history(history)
            {
                self.display_offset =
                    history.reanchor_offset(snapshot.history, self.display_offset);
                self.snapshot = Some(Arc::new(rebased));
            }
            self.pending_scroll_snapshot_offsets.clear();
        }
        if let Some(change) = history.changes_since(self.selection_visual_history) {
            self.selection_visual = self
                .selection_visual
                .and_then(|selection| selection.after_eviction(change.evicted));
            self.selection_visual_history = history;
        }
        self.history = history;
    }

    pub(in crate::features) fn apply_retained_logical_frame(
        &mut self,
        snapshot: Arc<TerminalSnapshot>,
        state: &TerminalScrollVisualState,
    ) -> bool {
        let Some(change) = state.history.changes_since(snapshot.history) else {
            return false;
        };
        let Some(offset) = state.display_offset.checked_sub(change.scrolled) else {
            return false;
        };
        let Some(index) = snapshot.logical_lines.as_ref() else {
            return false;
        };
        let top = index
            .logical_row(snapshot.history.scrollback_len.saturating_sub(offset))
            .min(index.logical_rows().saturating_sub(state.viewport_rows));
        let start = index.physical_row(top);
        let end = index.physical_row((top + state.viewport_rows).min(index.logical_rows()));
        if start < change.evicted
            || end > snapshot.history.scrollback_len
            || !terminal_snapshot_covers_display_offset(
                &snapshot,
                offset,
                state.viewport_rows,
                snapshot.scrollback_len,
            )
        {
            return false;
        }
        self.update_scroll_chrome_without_snapshot(state);
        // Keep the complete source index and source offset together. It is paint
        // state only: current-history lookups still request a fresh worker index.
        self.snapshot = Some(snapshot);
        self.display_offset = offset;
        self.show_cursor = false;
        self.set_scroll_snapshot_pending(true);
        self.revision = self.revision.saturating_add(1);
        true
    }

    fn retained_rows_should_reset(
        &self,
        snapshot: &TerminalSnapshot,
        history: nyaterm_terminal::TerminalHistoryState,
        scrollback_len: usize,
        viewport_rows: usize,
    ) -> bool {
        let viewport_rows = viewport_rows.max(1);
        if self.history.cols > 0 && history.changes_since(self.history).is_none() {
            return true;
        }
        let Some(previous) = self.snapshot.as_ref() else {
            return false;
        };
        previous.cols != snapshot.cols
            || self.viewport_rows != viewport_rows
            || scrollback_len < self.scrollback_len
            || snapshot.total_rows < previous.total_rows
            // A rebased numbering origin makes cached `display_line_number`s
            // incomparable with new rows; never stitch the two bases together.
            || previous.line_number_base != snapshot.line_number_base
    }

    fn clear_retained_scroll_state(&mut self) {
        self.retained_snapshots.clear();
        self.retained_rows.clear();
        self.retained_row_origin = 0;
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
        self.advance_history(state.history);
        let scroll_offset = state.scroll_offset;
        let scroll_residual_lines = state.scroll_residual_lines;
        let display_offset = state.display_offset;
        let scrollback_len = state.scrollback_len;
        let viewport_rows = state.viewport_rows;
        let has_new_while_scrolled = state.has_new_while_scrolled;
        let performance_overlay = state.performance_overlay;
        let skipped_output_chars = state.skipped_output_chars;
        let snapshot_pending = self.snapshot.is_some()
            && (self.display_offset != display_offset
                || !terminal_visual_scroll_active_for_state(scroll_offset, scroll_residual_lines));
        let state_matches = self.scroll_offset == scroll_offset
            && (self.scroll_residual_lines - scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.scrollback_len == scrollback_len
            && self.viewport_rows == viewport_rows.max(1)
            && self.has_new_while_scrolled == has_new_while_scrolled
            && self.performance_overlay == performance_overlay
            && self.skipped_output_chars == skipped_output_chars
            && !self.show_cursor
            && if self.snapshot.is_some() {
                self.scroll_snapshot_pending == snapshot_pending
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
        self.set_scroll_snapshot_pending(snapshot_pending);
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
        self.advance_history(state.history);
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
            self.snapshot_has_current_coordinates(snapshot)
                && terminal_snapshot_covers_display_offset(
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
        if !self.promote_snapshot_covering_display_offset(
            display_offset,
            viewport_rows,
            scrollback_len,
        ) {
            return self.update_scroll_chrome_without_snapshot(state);
        }
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
        // Rich logical frames already share their source row Arcs. Retaining
        // and estimating every cell here would scan a long line on the UI
        // thread, even though physical-row synthesis cannot serve this mode.
        if snapshot.logical_lines.is_none() && snapshot.display_rows.is_none() {
            self.remember_retained_snapshot_rows(snapshot.as_ref());
        }
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
        let Some((window_start, _)) = terminal_snapshot_absolute_window(snapshot) else {
            return 0;
        };
        let window_start = window_start.saturating_add(self.retained_row_origin);
        self.remember_retained_rows_with_limits(snapshot.rows(), window_start, max_rows, max_bytes)
    }

    fn remember_retained_rows_with_limits(
        &mut self,
        rows: &[Arc<nyaterm_terminal::TerminalSnapshotRow>],
        window_start: usize,
        max_rows: usize,
        max_bytes: usize,
    ) -> usize {
        let window_end = window_start.saturating_add(rows.len());
        let mut refreshed_rows = 0usize;
        for (row, snapshot_row) in rows.iter().enumerate() {
            let Some(abs_row) = window_start.checked_add(row) else {
                continue;
            };
            if self.retained_rows.get(&abs_row).is_some_and(|retained| {
                Arc::ptr_eq(retained, snapshot_row) || retained.as_ref() == snapshot_row.as_ref()
            }) {
                continue;
            }
            let estimated_bytes = terminal_snapshot_row_estimated_bytes(snapshot_row);
            if let Some(replaced) = self.retained_rows.insert(
                abs_row,
                TerminalRetainedRow {
                    row: Arc::clone(snapshot_row),
                    estimated_bytes,
                },
            ) {
                self.retained_row_estimated_bytes = self
                    .retained_row_estimated_bytes
                    .saturating_sub(replaced.estimated_bytes);
            }
            self.retained_row_estimated_bytes = self
                .retained_row_estimated_bytes
                .saturating_add(estimated_bytes);
            refreshed_rows = refreshed_rows.saturating_add(1);
        }
        while self.retained_rows.len() > max_rows || self.retained_row_estimated_bytes > max_bytes {
            let Some(first_key) = self.retained_rows.keys().next().copied() else {
                break;
            };
            let Some(last_key) = self.retained_rows.keys().next_back().copied() else {
                break;
            };
            // Equal keys and equal distances both keep the oldest key, so a
            // single-entry cache still makes progress.
            let drop_key =
                if terminal_retained_row_window_distance(first_key, window_start, window_end)
                    >= terminal_retained_row_window_distance(last_key, window_start, window_end)
                {
                    first_key
                } else {
                    last_key
                };
            if let Some(removed) = self.retained_rows.remove(&drop_key) {
                self.retained_row_estimated_bytes = self
                    .retained_row_estimated_bytes
                    .saturating_sub(removed.estimated_bytes);
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
            self.snapshot_has_current_coordinates(snapshot)
                && terminal_snapshot_covers_display_offset(
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
        if self.wrap_mode == nyaterm_core::models::settings::TerminalWrapMode::None
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.display_rows.is_some())
        {
            return false;
        }
        let viewport_rows = viewport_rows.max(1);
        let real_total_rows = scrollback_len.saturating_add(viewport_rows);
        let desired_end = real_total_rows.saturating_sub(display_offset);
        let desired_start = desired_end.saturating_sub(viewport_rows);
        self.retained_rows_cover_absolute_range(desired_start, desired_end)
            || self.snapshot_sources_cover_absolute_range(desired_start, desired_end)
    }

    fn retained_row(&self, row: usize) -> Option<&Arc<nyaterm_terminal::TerminalSnapshotRow>> {
        self.retained_rows
            .get(&row.checked_add(self.retained_row_origin)?)
            .map(|retained| &retained.row)
    }

    fn retained_rows_cover_absolute_range(&self, desired_start: usize, desired_end: usize) -> bool {
        let Some(first_row) = self.retained_row(desired_start) else {
            return false;
        };
        let cols = first_row.cells.len();
        if cols == 0 {
            return false;
        }
        (desired_start..desired_end).all(|abs_row| {
            self.retained_row(abs_row)
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
            .filter(|snapshot| self.snapshot_has_current_coordinates(snapshot))
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
            .filter(|snapshot| self.snapshot_has_current_coordinates(snapshot))
            .any(|snapshot| snapshot.cols > 0 && snapshot.cols != cols)
        {
            return false;
        }
        (desired_start..desired_end).all(|abs_row| {
            self.snapshot
                .iter()
                .chain(self.retained_snapshots.iter())
                .filter(|snapshot| self.snapshot_has_current_coordinates(snapshot))
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
        // Physical-row cache synthesis can split a logical line. Reuse a complete
        // committed frame or request one from the worker instead.
        if self.wrap_mode == nyaterm_core::models::settings::TerminalWrapMode::None
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.display_rows.is_some())
        {
            return None;
        }

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
        if let Some(snapshot) = self.snapshot.as_ref()
            && self.snapshot_has_current_coordinates(snapshot)
        {
            sources.push(snapshot);
        }
        sources.extend(self.retained_snapshots.iter());
        // Only rows numbered against the live origin may be stitched; a stale
        // snapshot from before a rebase would restart the gutter mid-window.
        let line_number_base = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.line_number_base);
        sources.retain(|snapshot| snapshot.line_number_base == line_number_base);

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
                command_blocks: self
                    .snapshot
                    .as_ref()
                    .map(|s| s.command_blocks.clone())
                    .unwrap_or_default(),
                display_rows: self.snapshot.as_ref().and_then(|s| s.display_rows.clone()),
                history: self.history,
                file_context: None,
                shell_input_anchor: None,
                command_navigation_target: None,
                command_navigation_revision: 0,
                shell_command_prompt: None,
                cols,
                viewport_rows,
                cursor: hidden_terminal_cursor_snapshot(),
                selection: None,
                scrollback_len,
                total_rows: real_total_rows,
                display_offset,
                images: Vec::new(),
                line_number_base,
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
        let first_row = self.retained_row(window_start)?;
        let cols = first_row.cells.len();
        let rows = window_end.checked_sub(window_start)?;
        if cols == 0 || rows < viewport_rows.max(1) {
            return None;
        }
        let real_total_rows = scrollback_len.saturating_add(viewport_rows);
        let display_offset = real_total_rows.checked_sub(window_end)?;
        let mut row_data = Vec::with_capacity(rows);

        for abs_row in window_start..window_end {
            let row = self.retained_row(abs_row)?;
            if row.cells.len() != cols {
                return None;
            }
            row_data.push(Arc::clone(row));
        }
        Some(Arc::new(TerminalSnapshot::from_rows(
            nyaterm_terminal::TerminalSnapshotMeta {
                command_blocks: self
                    .snapshot
                    .as_ref()
                    .map(|s| s.command_blocks.clone())
                    .unwrap_or_default(),
                display_rows: self.snapshot.as_ref().and_then(|s| s.display_rows.clone()),
                history: self.history,
                file_context: None,
                shell_input_anchor: None,
                command_navigation_target: None,
                command_navigation_revision: 0,
                shell_command_prompt: None,
                cols,
                viewport_rows,
                cursor: hidden_terminal_cursor_snapshot(),
                selection: None,
                scrollback_len,
                total_rows: real_total_rows,
                display_offset,
                images: Vec::new(),
                line_number_base: self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.line_number_base),
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
        let cols = self.retained_row(desired_start)?.cells.len();
        let row_is_compatible = |absolute_row: usize| {
            self.retained_row(absolute_row)
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
            minimum_contrast_ratio,
            cell_width,
            cell_height,
            show_line_numbers,
            max_line_number_digits,
            show_timestamps,
            wrap_mode,
            show_wrap_markers,
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
            && self.minimum_contrast_ratio == minimum_contrast_ratio
            && (self.cell_width - cell_width).abs() < f32::EPSILON * 8.0
            && (self.cell_height - cell_height).abs() < f32::EPSILON * 8.0
            && self.show_line_numbers == show_line_numbers
            && self.max_line_number_digits == max_line_number_digits
            && self.wrap_mode == wrap_mode
            && self.show_wrap_markers == show_wrap_markers
            && self.show_timestamps == show_timestamps
            && self.timestamp_format == timestamp_format
            && self.is_active == is_active;
        if state_matches {
            return false;
        }
        if self.palette.terminal_ansi != palette.terminal_ansi {
            self.invalidate_keyword_highlight_config();
        }
        self.palette = palette;
        self.font_family = font_family;
        self.font_fallbacks = font_fallbacks;
        self.font_size = font_size;
        self.normal_weight = normal_weight;
        self.bold_weight = bold_weight;
        self.bold_default_foreground = bold_default_foreground;
        self.minimum_contrast_ratio = minimum_contrast_ratio;
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.show_line_numbers = show_line_numbers;
        self.max_line_number_digits = max_line_number_digits;
        if self.wrap_mode != wrap_mode {
            self.horizontal_scroll
                .set_offset(gpui::point(px(0.), px(0.)));
            self.horizontal_reveal = None;
            self.painted_unwrapped = None;
            self.logical_scroll_residual_lines = 0.0;
        }
        self.wrap_mode = wrap_mode;
        self.show_wrap_markers = show_wrap_markers;
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
        by_row: bool,
        target_line: Option<TerminalLineId>,
    ) -> bool {
        if self.zebra_stripes_enabled == enabled
            && self.zebra_stripes_by_row == by_row
            && self.target_line == target_line
        {
            return false;
        }
        self.zebra_stripes_enabled = enabled;
        self.zebra_stripes_by_row = by_row;
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
                self.invalidate_keyword_highlight_config();
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
        let decorations_changed = !Arc::ptr_eq(&self.decorations, &decorations)
            && self.decorations.as_ref() != decorations.as_ref();
        if decorations_changed
            && self.wrap_mode == nyaterm_core::models::settings::TerminalWrapMode::None
        {
            let target = self.snapshot.as_ref().and_then(|snapshot| {
                decorations
                    .iter()
                    .enumerate()
                    .find_map(|(row, decoration)| {
                        let &(col, _) = decoration.active_search_ranges.first()?;
                        Some((snapshot.row(row)?.line_id?, col))
                    })
            });
            if target != self.last_horizontal_search {
                self.last_horizontal_search = target;
                if target.is_some() {
                    self.horizontal_reveal = target;
                }
            }
        }
        let changed = decorations_changed
            || rules_changed
            || self.show_cursor != show_cursor
            || self.cursor_style != cursor_style;
        if !changed {
            return false;
        }
        if rules_changed {
            self.invalidate_keyword_highlight_config();
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
        let visible_ranges = terminal_keyword_highlight_expanded_ranges(
            snapshot.as_ref(),
            std::slice::from_ref(&visible_rows),
        );

        let rules = self.keyword_rules.clone();
        let config = self.keyword_highlight_config();
        let rules_key = terminal_keyword_highlight_config_key(rules.as_slice(), config);
        let visible_ready = self.keyword_highlights.as_ref().is_some_and(|highlights| {
            highlights.rules_key() == rules_key
                && highlights.matches_snapshot_ranges(
                    snapshot.as_ref(),
                    self.palette,
                    &visible_ranges,
                )
        });
        if visible_ready && output_pressure {
            if self.keyword_highlight_task.is_none() {
                self.keyword_highlight_pending_key = None;
            }
            return;
        }
        let prefetch_viewports = terminal_keyword_highlight_prefetch_viewports(output_pressure);
        // First publish the viewport; only its follow-up fills idle prefetch.
        let requested_rows = if visible_ready {
            terminal_keyword_highlight_prefetch_rows(
                snapshot.as_ref(),
                self.display_offset,
                self.viewport_rows,
                self.scrollback_len,
                prefetch_viewports,
            )
        } else {
            visible_rows
        };
        let requested_ranges = terminal_keyword_highlight_expanded_ranges(
            snapshot.as_ref(),
            std::slice::from_ref(&requested_rows),
        );
        if visible_ready
            && self.keyword_highlights.as_ref().is_some_and(|highlights| {
                highlights.matches_snapshot_ranges(
                    snapshot.as_ref(),
                    self.palette,
                    &requested_ranges,
                )
            })
        {
            self.keyword_highlight_pending_key = None;
            return;
        }
        let requested_row_count = requested_rows.len();
        let expanded_requested_row_count: usize = requested_ranges.iter().map(Range::len).sum();
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
        let previous_highlights = self.keyword_highlights.clone();
        self.keyword_highlight_task = Some(cx.spawn(async move |this, cx| {
            let (rules, highlighter, result, highlight_duration) = cx
                .background_spawn(async move {
                    let highlighter = highlighter.unwrap_or_else(|| {
                        Arc::new(compile_terminal_keyword_highlighter(rules.as_ref(), config))
                    });
                    let highlight_started_at = Instant::now();
                    let result =
                        precompute_terminal_keyword_highlights_for_ranges_with_stats_and_cancel(
                            snapshot.as_ref(),
                            highlighter.as_ref(),
                            previous_highlights.as_deref(),
                            &requested_ranges,
                            || cancel_epoch.load(Ordering::Acquire) != request_cancel_epoch,
                        );
                    let highlight_duration = highlight_started_at.elapsed();
                    (rules, highlighter, result, highlight_duration)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // A configuration change cancels the worker cooperatively but
                // keeps this task as the single in-flight owner until it returns.
                this.keyword_highlight_task = None;
                this.keyword_highlight_pending_key = None;
                if this.keyword_highlight_generation != generation
                    || this.keyword_highlight_cancel_epoch.load(Ordering::Acquire)
                        != request_cancel_epoch
                    || this.keyword_highlight_config() != config
                    || !terminal_keyword_rule_sets_equal(&this.keyword_rules, &rules)
                {
                    this.schedule_keyword_highlights(
                        clear_if_empty,
                        this.keyword_highlight_output_pressure,
                        cx,
                    );
                    return;
                }
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
            .and(self.keyword_highlighter.as_ref())
            .filter(|highlighter| highlighter.config() == self.keyword_highlight_config())
            .cloned()
    }

    fn keyword_highlight_config(&self) -> TerminalKeywordHighlightConfig {
        TerminalKeywordHighlightConfig {
            line_roles: self.line_roles,
            ansi: self.palette.terminal_ansi,
        }
    }

    fn invalidate_keyword_highlight_config(&mut self) {
        self.keyword_highlight_generation = self.keyword_highlight_generation.saturating_add(1);
        self.keyword_highlight_cancel_epoch
            .fetch_add(1, Ordering::AcqRel);
        self.keyword_highlight_deferred_task = None;
        self.keyword_highlight_pending_key = None;
        self.keyword_highlight_last_started_at = None;
        self.keyword_highlighter_rules = None;
        self.keyword_highlighter = None;
        self.keyword_highlights = None;
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
        // Keep the in-flight owner until its cooperatively cancelled worker returns.
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
        self.selection_visual_history = self.history;
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
        self.history == state.history
            && self.scroll_offset == state.scroll_offset
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
            self.snapshot_has_current_coordinates(snapshot)
                && terminal_snapshot_covers_display_offset(
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
        self.history == state.history
            && self.scroll_offset == state.scroll_offset
            && (self.scroll_residual_lines - state.scroll_residual_lines).abs() < f32::EPSILON * 8.0
            && self.scrollback_len == state.scrollback_len
            && self.viewport_rows == state.viewport_rows.max(1)
            && self.has_new_while_scrolled == state.has_new_while_scrolled
            && self.performance_overlay == state.performance_overlay
            && self.skipped_output_chars == state.skipped_output_chars
            && if self.snapshot.is_some() {
                self.scroll_snapshot_pending == (self.display_offset != state.display_offset)
                    || (self.scroll_snapshot_pending
                        && !terminal_visual_scroll_active_for_state(
                            state.scroll_offset,
                            state.scroll_residual_lines,
                        ))
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
        self.advance_history(state.history);
        if state.history.preserves_coordinates_from(self.history)
            && self.has_snapshot_covering_display_offset(
                state.display_offset,
                state.viewport_rows,
                state.scrollback_len,
            )
        {
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

    /// Commit live text and its geometry together. Range coverage of a history
    /// window says nothing about the freshness of its mutable bottom rows.
    pub(in crate::features) fn apply_live_scroll_frame(
        &mut self,
        state: &TerminalScrollVisualState,
        live: Option<(TerminalSurfaceFrameSnapshot, TerminalCursorBlinkBasis)>,
    ) -> bool {
        if let Some((mut frame, basis)) = live {
            frame.has_action_link_decorations = self.has_action_link_decorations;
            if let Some(pending) = self.pending_local_scroll_sync.as_mut() {
                pending.state = frame.scroll.clone();
            }
            self.set_cursor_blink_basis(basis);
            self.apply_frame_snapshot(frame)
        } else {
            let changed = self.update_scroll_chrome_without_snapshot(state);
            let was_pending = self.scroll_snapshot_pending;
            self.set_scroll_snapshot_pending(self.snapshot.is_some());
            changed || was_pending != self.scroll_snapshot_pending
        }
    }

    fn scroll_visual_state_needs_repaint(&self, state: &TerminalScrollVisualState) -> bool {
        if state.history.preserves_coordinates_from(self.history)
            && self.has_snapshot_covering_display_offset(
                state.display_offset,
                state.viewport_rows,
                state.scrollback_len,
            )
        {
            !self.scroll_position_state_matches(state)
        } else {
            !self.scroll_chrome_state_matches(state)
        }
    }

    fn handle_scroll_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(app) = self.app.as_ref().and_then(WeakEntity::upgrade) else {
            self.wheel_zoom = TerminalWheelZoom::default();
            return;
        };
        let session_id = self.session_id.clone();
        if session_id.is_empty() {
            self.wheel_zoom = TerminalWheelZoom::default();
            return;
        }
        if let Some(delta) = self.wheel_zoom.handle_event(
            event,
            app.read(cx)
                .settings
                .summary()
                .interaction_terminal_zoom_enabled,
            cfg!(target_os = "macos"),
        ) {
            cx.stop_propagation();
            if delta != 0 {
                // Resizing reads and updates terminal surfaces. Release this surface's
                // update lease before entering the parent, as for protocol scrolling.
                cx.defer(move |cx| {
                    app.update(cx, |app, cx| {
                        if app.settings.summary().interaction_terminal_zoom_enabled {
                            app.adjust_terminal_font_size(delta, cx);
                        }
                    });
                });
            }
            return;
        }
        if self.handle_unwrapped_scroll(event, cx) {
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
            // The gesture ends navigation even at a clamped edge. Defer the
            // app adapter until this surface lease is released, rather than
            // resetting from the later coalesced visual sync (which could
            // otherwise cancel a newer navigation action).
            let reset_app = app.clone();
            let reset_session = session_id.clone();
            cx.defer(move |cx| {
                reset_app.update(cx, |app, _| {
                    app.reset_terminal_command_navigation_for_session(&reset_session);
                });
            });
            if result.visual_changed {
                let mut state = self.current_scroll_visual_state();
                if !self.visual_scroll_active() {
                    let live = app.read(cx).terminal_live_surface_frame(&session_id);
                    self.apply_live_scroll_frame(&state, live);
                    state = self.current_scroll_visual_state();
                    if self.scroll_snapshot_pending || self.snapshot.is_none() {
                        let app = app.clone();
                        let session_id = session_id.clone();
                        cx.defer(move |cx| {
                            app.update(cx, |app, _| {
                                app.request_terminal_live_snapshot(&session_id);
                            });
                        });
                    }
                }
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
            history: self.history,
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
        let text_updated = if terminal_visual_scroll_active_for_state(next_offset, next_residual) {
            self.apply_scroll_visual_state(state)
        } else {
            // The event adapter supplies the authoritative live frame before
            // notifying. Never promote a covering history window in between.
            self.update_scroll_chrome_without_snapshot(&state);
            false
        };
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
            history: self.history,
            session_id: self.session_id.clone(),
            scroll_offset: self.scroll_offset,
            scroll_residual_lines: self.scroll_residual_lines,
            display_offset: terminal_display_offset_from_state(
                self.scroll_offset,
                self.scroll_residual_lines,
                self.scrollback_len,
            ),
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
        if self.scroll_interaction_generation != pending.generation {
            return;
        }
        let state = app.update(cx, |this, cx| {
            this.sync_terminal_local_scroll_visual_state_from_surface(pending.state, cx)
        });
        if self.scroll_interaction_generation != pending.generation {
            return;
        }
        if let Some(state) = state {
            if !terminal_visual_scroll_active_for_state(
                state.scroll_offset,
                state.scroll_residual_lines,
            ) {
                let live = app.read(cx).terminal_live_surface_frame(&state.session_id);
                if self.apply_live_scroll_frame(&state, live) {
                    self.schedule_keyword_highlights(false, false, cx);
                    cx.notify();
                }
                return;
            }
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
        let (scroll_offset, max) = self
            .painted_unwrapped
            .as_ref()
            .and_then(|_| self.snapshot.as_ref()?.logical_lines.as_ref())
            .map_or((self.scroll_offset, self.scrollback_len), |index| {
                index.scroll_metrics(self.scroll_offset, self.viewport_rows)
            });
        let (scroll_offset, max) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.display_rows.as_ref())
            .map_or((scroll_offset, max), |index| {
                index.scroll_metrics(self.scroll_offset, self.viewport_rows)
            });
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
        let mut overview_markers = self.overview_markers.clone();
        let mut overview_total_rows = self.overview_total_rows;
        if let Some(index) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.display_rows.as_ref())
        {
            overview_markers = overview_markers
                .iter()
                .map(|marker| {
                    let mut marker = *marker;
                    marker.absolute_line = index.display_row(marker.absolute_line);
                    marker
                })
                .collect::<Vec<_>>()
                .into();
            overview_total_rows = index.rows();
        }
        let overview_track_height_px =
            track_bounds.map(track_height).unwrap_or(1.0).round() as usize;
        let overview_marker_buckets = if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.display_rows.is_some())
        {
            terminal_overview_marker_buckets(
                &overview_markers,
                overview_total_rows,
                overview_track_height_px,
            )
            .into()
        } else {
            self.overview_marker_buckets_for_track_height(overview_track_height_px)
        };

        div()
            .id(SharedString::from(format!(
                "terminal-scrollbar-{session_id}"
            )))
            .w(px(TERMINAL_SCROLLBAR_COLUMN_WIDTH))
            .flex_none()
            .h_full()
            .py(px(TERMINAL_SCROLLBAR_TRACK_PADDING_Y))
            .pr(px(TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT))
            .when(!show, |this| this.invisible())
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
                            // Pane activation walks into
                            // `notify_terminal_surface_only`, which reads this
                            // surface — defer until this entity update drops
                            // its lease, the same escape hatch
                            // `flush_local_scroll_app_sync` uses above.
                            if !session_id.is_empty() {
                                let activate_id = session_id.clone();
                                let activate_app = app.clone();
                                cx.defer(move |cx| {
                                    activate_app.update(cx, |this, cx| {
                                        this.activate_workspace_pane(activate_id, cx);
                                    });
                                });
                            }
                            let repaint_session_id = app.update(cx, |this, _cx| {
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
                                    this.terminal_scrollbar_range_for_session(
                                        drag_session_id.as_deref(),
                                    )
                                    .0,
                                );
                                let offset = this.terminal_grid_offset_for_scrollbar(
                                    drag_session_id.as_deref(),
                                    offset,
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

fn terminal_keyword_highlight_visible_rows(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> Range<usize> {
    if let Some(rows) =
        terminal_projected_highlight_rows(snapshot, display_offset, viewport_rows, 0)
    {
        return rows;
    }
    if let Some(rows) = terminal_logical_highlight_rows(snapshot, display_offset, viewport_rows, 0)
    {
        return rows;
    }
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

/// Highlight rows for a folded viewport. Painting walks the fold projection,
/// so the scheduled window must cover the source rows it actually paints; the
/// unfolded index misses the rows a collapse pulls into the viewport (#1259).
fn terminal_projected_highlight_rows(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    prefetch_viewports: usize,
) -> Option<Range<usize>> {
    let projection = snapshot.display_rows.as_ref()?;
    let viewport_rows = viewport_rows.max(1);
    let extra_rows = viewport_rows.saturating_mul(prefetch_viewports);
    let top = projection.top(display_offset, viewport_rows);
    let start_display = top.saturating_sub(extra_rows);
    let end_display = (top + viewport_rows + extra_rows).min(projection.rows());
    let (source_start, _) = terminal_snapshot_absolute_window(snapshot)?;
    let start = projection
        .source_row(start_display)
        .saturating_sub(source_start)
        .min(snapshot.row_count());
    let end = projection
        .source_row(end_display)
        .saturating_sub(source_start)
        .min(snapshot.row_count())
        .max(start);
    Some(start..end)
}

fn terminal_keyword_highlight_prefetch_rows(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
    prefetch_viewports: usize,
) -> Range<usize> {
    if let Some(rows) = terminal_projected_highlight_rows(
        snapshot,
        display_offset,
        viewport_rows,
        prefetch_viewports,
    ) {
        return rows;
    }
    if let Some(rows) =
        terminal_logical_highlight_rows(snapshot, display_offset, viewport_rows, prefetch_viewports)
    {
        return rows;
    }
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

fn terminal_logical_highlight_rows(
    snapshot: &TerminalSnapshot,
    offset: usize,
    viewport: usize,
    prefetch: usize,
) -> Option<Range<usize>> {
    let index = snapshot.logical_lines.as_ref()?;
    let max = index.logical_rows().saturating_sub(viewport);
    let top = if offset == 0 {
        max
    } else {
        index
            .logical_row(snapshot.history.scrollback_len.saturating_sub(offset))
            .min(max)
    };
    let source_start = snapshot
        .total_rows
        .saturating_sub(snapshot.display_offset)
        .saturating_sub(snapshot.row_count());
    let extra = viewport.saturating_mul(prefetch);
    let start = index
        .physical_row(top.saturating_sub(extra))
        .saturating_sub(source_start)
        .min(snapshot.row_count());
    let end = index
        .physical_row((top + viewport + extra).min(index.logical_rows()))
        .saturating_sub(source_start)
        .min(snapshot.row_count());
    Some(start..end.max(start))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        TERMINAL_SURFACE_PAINT_COUNT.fetch_add(1, Ordering::Relaxed);
        let palette = self.palette;
        let cell_w = self.cell_width.max(1.0);
        let cell_h =
            nyaterm_core::terminal_snapped_cell_height(self.cell_height, window.scale_factor());
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
            }) - (viewport_anchor_row + snapshot.leading_cleared_rows(viewport_anchor_row)) as f32
                * cell_h;
        self.painted_unwrapped = self.unwrapped_layout(&snapshot);
        let visual_y_offset = if self.painted_unwrapped.is_some() {
            0.0
        } else {
            visual_y_offset
        };
        let visual_y_offset = if self.scroll_snapshot_pending && !self.visual_scroll_active() {
            // A missing live frame must not move old bottom rows into view.
            // Keep the last painted geometry until live text can be committed.
            self.painted_hit_test_geometry
                .filter(|geometry| {
                    geometry.viewport_rows == self.viewport_rows
                        && geometry.cell_height == cell_h
                        && self
                            .painted_hit_test_snapshot
                            .as_ref()
                            .is_some_and(|painted| Arc::ptr_eq(painted, &snapshot))
                })
                .map_or(visual_y_offset, |geometry| geometry.visual_y_offset)
        } else {
            visual_y_offset
        };
        if terminal_geometry_diagnostics_enabled() {
            tracing::info!(
                diagnostic = "terminal_surface_geometry",
                session_id = %self.session_id,
                surface_revision = self.revision,
                snapshot_rows = snapshot.row_count(),
                viewport_rows = self.viewport_rows,
                scrollback_len = self.scrollback_len,
                total_rows = snapshot.total_rows,
                viewport_anchor_row,
                visual_y_offset,
                cell_height = cell_h,
                scroll_offset = self.scroll_offset,
                display_offset = self.display_offset,
                scroll_residual_lines = self.scroll_residual_lines,
                snapshot_pending = self.scroll_snapshot_pending,
                has_action_link_decorations = self.has_action_link_decorations,
                "terminal surface geometry"
            );
        }
        let gutter_enabled = true;
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
        let line_count = if self.painted_unwrapped.is_some() {
            self.viewport_rows
        } else {
            snapshot.row_count()
        };
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
        let composition_exclusion = std::rc::Rc::default();
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
            .with_paint_exclusion(std::rc::Rc::clone(&composition_exclusion))
            .with_bold_default_foreground(self.bold_default_foreground)
            .with_minimum_contrast_ratio(self.minimum_contrast_ratio)
            .with_selection(self.selection_visual.and_then(|mut selection| {
                if let Some(change) = self
                    .selection_visual_history
                    .changes_since(snapshot.history)
                {
                    if !selection.all_buffer {
                        selection.anchor.position.line =
                            selection.anchor.position.line.checked_add(change.evicted)?;
                        selection.head.position.line =
                            selection.head.position.line.checked_add(change.evicted)?;
                    }
                    Some(TerminalGridSelection::from_selection(selection))
                } else {
                    selection
                        .reanchored_history(self.selection_visual_history, snapshot.history)
                        .map(TerminalGridSelection::from_selection)
                }
            }))
            .with_font_fallbacks(self.font_fallbacks.clone())
            .with_layout_cache(self.layout_cache.clone())
            .with_layout_rows(self.viewport_rows)
            .with_fill_height(true)
            .with_unwrapped_layout(self.painted_unwrapped.clone())
            .with_visual_y_offset(visual_y_offset)
            .with_zebra_stripes(zebra_stripes_visible, target_line)
            .with_zebra_stripes_by_row(self.zebra_stripes_by_row);
        if let Some(highlights) = self.keyword_highlights.clone() {
            grid = grid.with_keyword_highlights(highlights);
        }

        let gutter = if gutter_enabled {
            let gutter_metrics = terminal_gutter_metrics(
                cell_w,
                self.show_timestamps,
                terminal_timestamp_format_width_chars(&self.timestamp_format),
                self.show_line_numbers,
                self.max_line_number_digits
                    .max(terminal_line_number_digits(snapshot.as_ref())),
            );
            let timestamp_formatter = TerminalTimestampFormatter::new(&self.timestamp_format);
            let line_number_digits = self
                .max_line_number_digits
                .max(terminal_line_number_digits(snapshot.as_ref()));
            let ts_w = gutter_metrics.timestamp_width;
            let ln_w = gutter_metrics.line_number_width;
            let gutter_viewport_width = gutter_metrics.total_width();
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
            let labeled_row_count = terminal_gutter_row_count(snapshot.as_ref());
            let gutter_row_count = if zebra_stripes_visible && self.zebra_stripes_by_row {
                self.painted_unwrapped
                    .as_ref()
                    .map_or(snapshot.row_count(), |layout| {
                        layout
                            .viewport_rows
                            .min(layout.logical_rows().saturating_sub(layout.logical_top))
                    })
            } else {
                labeled_row_count
            };
            for display_row in visible_gutter_rows
                .clone()
                .take_while(|row| *row < gutter_row_count)
            {
                let line_index = self
                    .painted_unwrapped
                    .as_ref()
                    .map_or(display_row, |layout| layout.first_source_row(display_row));
                let snapshot_row = snapshot.row(line_index);
                let row_target = self
                    .painted_unwrapped
                    .as_ref()
                    .map_or(target_line, |layout| {
                        layout.target_line_for_display_row(&snapshot, display_row, target_line)
                    });
                let zebra_color = zebra_stripes_visible
                    .then(|| {
                        snapshot_row.and_then(|row| {
                            terminal_row_stripe_color(
                                row,
                                TerminalStripeMode::new(
                                    self.zebra_stripes_by_row,
                                    self.painted_unwrapped
                                        .as_ref()
                                        .is_some_and(|layout| layout.unwrapped),
                                ),
                                row_target,
                                palette,
                            )
                            .map(rgba)
                        })
                    })
                    .flatten();
                let labels = terminal_gutter_labels(
                    snapshot_row,
                    abs_start + line_index + 1,
                    self.show_timestamps
                        && line_index < labeled_row_count
                        && self
                            .painted_unwrapped
                            .as_ref()
                            .is_none_or(|layout| layout.summary(display_row).is_none()),
                    self.show_line_numbers
                        && line_index < labeled_row_count
                        && self
                            .painted_unwrapped
                            .as_ref()
                            .is_none_or(|layout| layout.summary(display_row).is_none()),
                    line_number_digits,
                    &timestamp_formatter,
                );
                gutter_rows = gutter_rows.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(cell_h))
                        .line_height(px(cell_h))
                        .gap(px(gutter_metrics.gap_width))
                        .flex_none()
                        .pr(px(19.))
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
                    .overflow_hidden()
                    .child(gutter_rows)
                    .child(self.command_rail(&snapshot, visual_y_offset, cell_h, cx)),
            )
        } else {
            None
        };

        let wrap_markers = self.wrap_marker_gutter(&snapshot, visual_y_offset, cell_h);
        let (horizontal, horizontal_bar) = self.horizontal_scroll_layer();
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
                        .children(horizontal)
                        .overflow_hidden()
                        .child(grid)
                        .children(self.command_summaries(&snapshot, cell_h))
                        .children(horizontal_bar)
                        .child(terminal_surface_grid_bounds_tracker(surface.clone())),
                )
                .children(wrap_markers)
        } else {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .children(horizontal)
                        .overflow_hidden()
                        .child(grid)
                        .children(self.command_summaries(&snapshot, cell_h))
                        .children(horizontal_bar)
                        .child(terminal_surface_grid_bounds_tracker(surface)),
                )
                .children(wrap_markers)
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
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
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
                    .when(
                        self.wrap_mode == nyaterm_core::models::settings::TerminalWrapMode::None
                            && self.painted_unwrapped.is_none()
                            && snapshot.logical_lines.is_none(),
                        |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .right_2()
                                    .top_1()
                                    .text_xs()
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(
                                        rust_i18n::t!("settings.terminalWrapGridFallback")
                                            .to_string(),
                                    ),
                            )
                        },
                    )
                    .when_some(app, |this, app| {
                        this.child(terminal_bounds_tracker_with_exclusion(
                            app,
                            Some(session_id.clone()),
                            is_active,
                            composition_exclusion,
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
mod history_rotation_bench;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod wheel_zoom_tests;

#[cfg(test)]
mod native_probe;
