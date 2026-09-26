use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::models::{
    EffectiveTerminalPaintPolicy, TerminalFrameActionLinks, TerminalPerformanceMode,
    TerminalPresentation, TerminalProtocolState, TerminalSearchMode, TerminalSelection,
    TerminalViewState, TerminalWorkPolicy, terminal_action_link_matcher_key,
    terminal_snapshot_matches_grid_geometry,
};
use gpui::{AppContext, ClipboardItem, Context, Entity, Window};
#[cfg(test)]
use nyaterm_terminal::TerminalScreen;
use nyaterm_terminal::TerminalSnapshot;

use crate::features::NyaTermApp;
use crate::features::terminal::terminal_surface_entity::{
    TerminalSurface, TerminalSurfaceFrameSnapshot, TerminalSurfacePaintChrome,
    terminal_snapshot_covers_display_offset,
};
use crate::terminal::{NyaTerminalLayoutCache, TerminalBufferMatch, TerminalLineDecorations};

use super::{
    TERMINAL_INPUT_LATENCY_WINDOW, TERMINAL_USER_SCROLL_ACTIVE_WINDOW, TerminalScrollVisualState,
};

mod input;

pub(in crate::features) use input::TerminalMouseReportRequest;
#[cfg(test)]
use input::{
    lost_mouse_report_release_button, terminal_key_bytes_for_mode_and_settings,
    terminal_mouse_report_button, terminal_session_write_failure_log,
    terminal_should_defer_key_text_to_input_handler_for_state,
    terminal_should_track_command_suggestion_input, terminal_status_changed,
};

const TERMINAL_SCROLL_PAINT_SLOW_TOTAL: Duration = Duration::from_millis(16);
const TERMINAL_SCROLL_PAINT_SLOW_STAGE: Duration = Duration::from_millis(8);
const TERMINAL_SCROLL_POSITION_NOTIFY_SLOW: Duration = Duration::from_millis(8);
#[cfg(test)]
const TERMINAL_SCROLL_RETAINED_WINDOW_MIN_EXTRA_ROWS: usize = 32;
#[cfg(test)]
const TERMINAL_SCROLL_RETAINED_WINDOW_MAX_EXTRA_ROWS: usize = 192;

#[cfg(test)]
type TerminalSnapshotRowSlice = std::sync::Arc<nyaterm_terminal::TerminalSnapshotRow>;

#[cfg(test)]
fn terminal_paint_snapshot_for_view(
    view: Option<&TerminalViewState>,
    offset: usize,
    retained_surface_snapshot: Option<std::sync::Arc<TerminalSnapshot>>,
) -> Option<std::sync::Arc<TerminalSnapshot>> {
    let Some(view) = view else {
        return if offset == 0 {
            retained_surface_snapshot
        } else {
            None
        };
    };
    if offset == 0 {
        return view.frame_snapshot.clone();
    }
    view.scrollback_snapshots
        .get(&offset)
        .cloned()
        .or(retained_surface_snapshot)
}

fn terminal_cached_scrollback_snapshot_covering_display_offset(
    view: &TerminalViewState,
    display_offset: usize,
    viewport_rows: usize,
) -> Option<std::sync::Arc<TerminalSnapshot>> {
    if display_offset == 0 {
        return None;
    }
    if let Some(snapshot) = view.scrollback_snapshots.get(&display_offset) {
        return Some(snapshot.clone());
    }
    let scrollback_len = view.scrollback_len_for_ui();
    view.scrollback_snapshots
        .values()
        .filter(|snapshot| {
            terminal_snapshot_covers_display_offset(
                snapshot.as_ref(),
                display_offset,
                viewport_rows,
                scrollback_len,
            )
        })
        .min_by_key(|snapshot| snapshot.display_offset.abs_diff(display_offset))
        .cloned()
}

fn terminal_retained_snapshot_matches_view(
    snapshot: &TerminalSnapshot,
    view: &TerminalViewState,
    display_offset: usize,
    viewport_rows: usize,
) -> bool {
    let scrollback_len = view.scrollback_len_for_ui();
    terminal_snapshot_matches_grid_geometry(snapshot, view.screen.cols(), view.screen.rows())
        && snapshot.scrollback_len == scrollback_len
        && terminal_snapshot_covers_display_offset(
            snapshot,
            display_offset,
            viewport_rows,
            scrollback_len,
        )
}

fn terminal_paint_window_snapshot_for_view(
    view: Option<&TerminalViewState>,
    display_offset: usize,
    viewport_rows: usize,
    retained_surface_snapshot: Option<std::sync::Arc<TerminalSnapshot>>,
) -> Option<std::sync::Arc<TerminalSnapshot>> {
    if view.is_some_and(|view| view.grid_resize_pending) {
        return retained_surface_snapshot;
    }
    if display_offset == 0 {
        let Some(view) = view else {
            return retained_surface_snapshot;
        };
        let scrollback_len = view.scrollback_len_for_ui();
        if let Some(snapshot) = view.frame_snapshot.as_ref()
            && terminal_snapshot_matches_grid_geometry(
                snapshot.as_ref(),
                view.screen.cols(),
                view.screen.rows(),
            )
            && terminal_snapshot_covers_display_offset(
                snapshot.as_ref(),
                display_offset,
                viewport_rows,
                scrollback_len,
            )
        {
            return Some(snapshot.clone());
        }
        return retained_surface_snapshot.filter(|snapshot| {
            terminal_snapshot_matches_grid_geometry(
                snapshot.as_ref(),
                view.screen.cols(),
                view.screen.rows(),
            ) && terminal_snapshot_covers_display_offset(
                snapshot.as_ref(),
                display_offset,
                viewport_rows,
                scrollback_len,
            )
        });
    }
    if let Some(snapshot) = retained_surface_snapshot {
        return Some(snapshot);
    }
    let view = view?;
    terminal_cached_scrollback_snapshot_covering_display_offset(view, display_offset, viewport_rows)
}

#[cfg(test)]
fn terminal_scroll_retained_window_extra_rows(viewport_rows: usize) -> usize {
    viewport_rows.saturating_mul(2).clamp(
        TERMINAL_SCROLL_RETAINED_WINDOW_MIN_EXTRA_ROWS,
        TERMINAL_SCROLL_RETAINED_WINDOW_MAX_EXTRA_ROWS,
    )
}

pub(in crate::features) fn terminal_visual_display_offset(
    target_offset: usize,
    _residual_lines: f32,
    max_offset: usize,
) -> usize {
    // Keep fractional wheel/trackpad movement as a visual transform only.
    // Switching the text snapshot window at half-line boundaries causes the
    // surface to wait on a different scrollback snapshot and reads as flicker.
    if max_offset == 0 {
        return 0;
    }
    target_offset.min(max_offset)
}

fn terminal_scroll_snapshot_request_offset(
    target_offset: usize,
    residual_lines: f32,
    max_offset: usize,
) -> Option<usize> {
    let display_offset = terminal_visual_display_offset(target_offset, residual_lines, max_offset);
    (display_offset > 0).then_some(display_offset)
}

fn terminal_cursor_visible_for_display_offset(
    is_active: bool,
    is_disconnected: bool,
    display_offset: usize,
    remote_cursor_visible: bool,
    blink_enabled: bool,
    cursor_blink_on: bool,
) -> bool {
    is_active
        && !is_disconnected
        && display_offset == 0
        && remote_cursor_visible
        && (!blink_enabled || cursor_blink_on)
}

fn terminal_cursor_position_changed(
    previous: Option<(usize, usize)>,
    current: Option<(usize, usize)>,
) -> bool {
    current.is_some() && previous != current
}

#[cfg(test)]
fn terminal_snapshot_with_newer_edge_row(
    base: std::sync::Arc<TerminalSnapshot>,
    newer: std::sync::Arc<TerminalSnapshot>,
) -> std::sync::Arc<TerminalSnapshot> {
    if base.cols == 0 || base.row_count() == 0 || base.cols != newer.cols || newer.row_count() == 0
    {
        return base;
    }
    let mut rows = base.rows().to_vec();
    rows.push(newer.rows().last().unwrap().clone());
    std::sync::Arc::new(TerminalSnapshot::from_rows(
        nyaterm_terminal::TerminalSnapshotMeta {
            cols: base.cols,
            viewport_rows: base.viewport_rows,
            cursor: base.cursor,
            selection: base.selection.clone(),
            scrollback_len: base.scrollback_len,
            total_rows: base.total_rows.saturating_add(1),
            display_offset: base.display_offset,
            images: base.images.clone(),
        },
        rows,
    ))
}

#[cfg(test)]
fn terminal_snapshot_with_retained_scroll_window(
    view: &TerminalViewState,
    base: std::sync::Arc<TerminalSnapshot>,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> std::sync::Arc<TerminalSnapshot> {
    if base.cols == 0 || base.row_count() == 0 {
        return base;
    }
    let extra = terminal_scroll_retained_window_extra_rows(viewport_rows);
    let older_count = scrollback_len.saturating_sub(display_offset).min(extra);
    let newer_count = display_offset.min(extra);
    if older_count == 0 && newer_count == 0 {
        return base;
    }
    let older_rows = if older_count == 0 {
        Vec::new()
    } else {
        terminal_snapshot_older_row_slices(&view.screen, display_offset, older_count)
    };
    let older_count = older_rows.len();
    let mut rows = Vec::with_capacity(older_count + base.row_count() + newer_count);
    rows.extend(older_rows);
    rows.extend(base.rows().iter().cloned());
    if newer_count > 0 {
        rows.extend(terminal_snapshot_newer_row_slices(
            &view.screen,
            display_offset,
            newer_count,
        ));
    }
    let images = view
        .screen
        .viewport_snapshot(display_offset)
        .images
        .into_iter()
        .filter(|image| image.row < viewport_rows)
        .map(|mut image| {
            image.row = image.row.saturating_add(older_count);
            image
        })
        .collect();
    std::sync::Arc::new(TerminalSnapshot::from_rows(
        nyaterm_terminal::TerminalSnapshotMeta {
            cols: base.cols,
            viewport_rows: base.viewport_rows,
            cursor: base.cursor,
            selection: base.selection.clone(),
            scrollback_len: base.scrollback_len,
            total_rows: base.total_rows.saturating_add(newer_count),
            display_offset: base.display_offset,
            images,
        },
        rows,
    ))
}

#[cfg(test)]
fn terminal_snapshot_older_row_slices(
    screen: &TerminalScreen,
    display_offset: usize,
    row_count: usize,
) -> Vec<TerminalSnapshotRowSlice> {
    let mut rows = Vec::new();
    let mut remaining = row_count;
    while remaining > 0 {
        let snapshot = screen.viewport_snapshot(display_offset.saturating_add(remaining));
        if snapshot.row_count() == 0 {
            break;
        }
        let take = remaining.min(snapshot.row_count());
        rows.extend(terminal_snapshot_row_slices(&snapshot, 0, take));
        remaining = remaining.saturating_sub(take);
    }
    rows
}

#[cfg(test)]
fn terminal_snapshot_newer_row_slices(
    screen: &TerminalScreen,
    display_offset: usize,
    row_count: usize,
) -> Vec<TerminalSnapshotRowSlice> {
    let mut rows = Vec::new();
    let mut consumed = 0usize;
    let viewport_rows = screen.viewport_snapshot(display_offset).row_count().max(1);
    while consumed < row_count {
        let remaining = row_count - consumed;
        let take = remaining.min(viewport_rows);
        let offset_delta = consumed.saturating_add(take);
        let snapshot = screen.viewport_snapshot(display_offset.saturating_sub(offset_delta));
        if snapshot.row_count() == 0 {
            break;
        }
        let take = take.min(snapshot.row_count());
        rows.extend(terminal_snapshot_row_slices(
            &snapshot,
            snapshot.row_count().saturating_sub(take),
            take,
        ));
        consumed = consumed.saturating_add(take);
    }
    rows
}

#[cfg(test)]
fn terminal_snapshot_row_slices(
    snapshot: &TerminalSnapshot,
    start_row: usize,
    row_count: usize,
) -> Vec<TerminalSnapshotRowSlice> {
    if row_count == 0 || start_row >= snapshot.row_count() {
        return Vec::new();
    }
    let end_row = start_row
        .saturating_add(row_count)
        .min(snapshot.row_count());
    (start_row..end_row)
        .filter_map(|row| terminal_snapshot_row_slice(snapshot, row))
        .collect()
}

#[cfg(test)]
fn terminal_snapshot_row_slice(
    snapshot: &TerminalSnapshot,
    row: usize,
) -> Option<TerminalSnapshotRowSlice> {
    snapshot.rows().get(row).cloned()
}

fn terminal_scroll_text_first_decorations(
    snapshot: &TerminalSnapshot,
    search_matches: Option<&[TerminalBufferMatch]>,
    frame_action_links: &[TerminalFrameActionLinks],
    include_action_links: bool,
    include_hyperlinks: bool,
) -> Vec<TerminalLineDecorations> {
    let mut search_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    if let Some(search_matches) = search_matches {
        let (abs_start, abs_end) =
            crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(snapshot);
        for search_match in search_matches {
            let abs = search_match.line_index;
            if abs < abs_start || abs >= abs_end {
                continue;
            }
            search_ranges_by_line
                .entry(abs - abs_start)
                .or_default()
                .push((search_match.start_col, search_match.end_col));
        }
    }
    let has_search_decorations = !search_ranges_by_line.is_empty();
    let has_frame_action_links = include_action_links
        && crate::features::terminal::terminal_surface::terminal_action_links_have_ranges_for_snapshot(
            snapshot,
            frame_action_links,
        );
    let has_hyperlinks =
        include_hyperlinks && snapshot.rows().iter().any(|row| !row.hyperlinks.is_empty());
    if !crate::features::terminal::terminal_surface::terminal_line_decorations_needed(
        false,
        has_search_decorations,
        has_frame_action_links,
        has_hyperlinks,
    ) {
        return Vec::new();
    }

    let active_search_ranges_by_line = HashMap::new();
    crate::features::terminal::terminal_surface::build_terminal_line_decorations(
        snapshot,
        &crate::features::terminal::terminal_surface::TerminalDecorationSources {
            selected_occurrence_ranges_by_line: &HashMap::new(),
            search_ranges_by_line: &search_ranges_by_line,
            active_search_ranges_by_line: &active_search_ranges_by_line,
            frame_action_links,
            include_action_links,
            include_hyperlinks,
        },
    )
}

fn terminal_keyword_highlight_updates_allowed(is_active: bool, input_latency_active: bool) -> bool {
    // TerminalSurface keeps the last precomputed spans while rules are withheld;
    // the input-idle repaint restores rules and schedules one catch-up task.
    is_active && !input_latency_active
}

fn terminal_live_action_link_enrichment_allowed(
    display_offset: usize,
    action_links_enabled: bool,
    input_latency_active: bool,
    output_or_scroll_pressure: bool,
) -> bool {
    display_offset == 0
        && action_links_enabled
        && !input_latency_active
        && !output_or_scroll_pressure
}

fn terminal_selection_for_session(
    selection: Option<TerminalSelection>,
    selection_session_id: Option<&str>,
    active_session_id: Option<&str>,
    session_id: &str,
) -> Option<TerminalSelection> {
    let owner = selection_session_id.or(active_session_id);
    (owner == Some(session_id)).then_some(selection).flatten()
}

fn terminal_user_scroll_active(
    display_offset: usize,
    session_has_recent_user_scroll: bool,
    last_user_scroll_at: Option<Instant>,
    now: Instant,
) -> bool {
    display_offset > 0
        && session_has_recent_user_scroll
        && last_user_scroll_at.is_some_and(|last| {
            now.saturating_duration_since(last) < TERMINAL_USER_SCROLL_ACTIVE_WINDOW
        })
}

fn terminal_input_latency_active(last_input_at: Option<Instant>, now: Instant) -> bool {
    last_input_at
        .is_some_and(|last| now.saturating_duration_since(last) < TERMINAL_INPUT_LATENCY_WINDOW)
}

impl NyaTermApp {
    pub(in crate::features) fn terminal_protocol_state_for_session(
        &self,
        session_id: &str,
    ) -> TerminalProtocolState {
        self.terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.protocol_state)
            .unwrap_or_else(|| TerminalProtocolState::from_screen(&self.terminal.view.screen))
    }

    pub(in crate::features) fn open_terminal_actions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.menus.actions_open = true;
        self.shell.set_status("terminal actions opened".to_string());
        window.focus(&self.terminal.menus.actions_focus, cx);
        cx.notify();
    }

    pub(in crate::features) fn close_terminal_actions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.menus.actions_open = false;
        self.shell.set_status("terminal actions closed".to_string());
        window.focus(&self.terminal.input.focus, cx);
        cx.notify();
    }

    pub(in crate::features) fn focus_terminal_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.terminal.input.focus, cx);
        cx.notify();
    }

    pub(in crate::features) fn active_terminal_visible_text(&self) -> String {
        self.active_terminal_snapshot()
            .rows()
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(in crate::features) fn active_terminal_buffer_text(&self) -> String {
        self.session
            .active_id()
            .map(|session_id| self.terminal_buffer_text_for_session(session_id))
            .unwrap_or_else(|| self.terminal.view.output.clone())
    }

    pub(in crate::features) fn terminal_buffer_text_for_session(&self, session_id: &str) -> String {
        self.terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.output.clone())
            .unwrap_or_else(|| self.terminal.view.output.clone())
    }

    pub(in crate::features) fn active_terminal_buffer_tail(&self) -> &str {
        self.session
            .active_id()
            .and_then(|session_id| self.terminal.view.views.get(session_id))
            .map(|view| view.output.as_str())
            .unwrap_or(self.terminal.view.output.as_str())
    }

    pub(in crate::features) fn terminal_buffer_tail_for_session(&self, session_id: &str) -> &str {
        self.terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.output.as_str())
            .unwrap_or(self.terminal.view.output.as_str())
    }

    pub(in crate::features) fn terminal_snapshot_for_session(
        &self,
        session_id: Option<&str>,
        offset: usize,
    ) -> std::sync::Arc<TerminalSnapshot> {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty())
            && let Some(view) = self.terminal.view.views.get(session_id)
        {
            if offset == 0 {
                return view
                    .frame_snapshot
                    .clone()
                    .unwrap_or_else(|| view.live_snapshot_with_scroll_window());
            }
            if let Some(snapshot) = view.scrollback_snapshots.get(&offset).cloned() {
                return snapshot;
            }
            if let Some(snapshot) = view.frame_snapshot.as_ref()
                && terminal_snapshot_covers_display_offset(
                    snapshot.as_ref(),
                    offset,
                    view.viewport_rows_for_ui(),
                    view.scrollback_len_for_ui(),
                )
            {
                return snapshot.clone();
            }
            // A live session's UI screen is only a geometry/encoding mirror;
            // it may not contain the worker's latest output. Use the nearest
            // authoritative frame when the requested history window is still
            // being fetched instead of painting or hit-testing stale content.
            return view
                .frame_snapshot
                .clone()
                .unwrap_or_else(|| std::sync::Arc::new(view.screen.viewport_snapshot(offset)));
        }
        std::sync::Arc::new(self.terminal.view.screen.viewport_snapshot(offset))
    }

    pub(in crate::features) fn active_terminal_snapshot(&self) -> std::sync::Arc<TerminalSnapshot> {
        self.terminal_snapshot_for_session(
            self.session.active_id(),
            self.active_terminal_display_offset(),
        )
    }

    pub(in crate::features) fn copy_terminal_visible_text(&mut self, cx: &mut Context<Self>) {
        let text = self.active_terminal_visible_text();
        if text.trim().is_empty() {
            self.shell
                .set_status("visible terminal text is empty".to_string());
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.shell
                .set_status("copied visible terminal text".to_string());
        }
        self.terminal.menus.actions_open = false;
        cx.notify();
    }

    pub(in crate::features) fn send_terminal_clear_screen(&mut self, cx: &mut Context<Self>) {
        self.terminal.menus.actions_open = false;
        self.clear_terminal_selection(cx);
        if self.send_terminal_input(vec![0x0c], cx) {
            self.shell
                .set_status("clear screen command sent".to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn ensure_terminal_surface(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalSurface> {
        if let Some(surface) = self.terminal.view.surfaces.get(session_id) {
            return surface.clone();
        }
        let layout_cache = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.render_cache.layout_cache.clone())
            .unwrap_or_else(|| {
                std::sync::Arc::new(std::sync::Mutex::new(NyaTerminalLayoutCache::default()))
            });
        let session_id_owned = session_id.to_string();
        let app = cx.entity();
        let surface = cx.new(|_| {
            let mut surface = TerminalSurface::new(session_id_owned);
            surface.set_layout_cache(layout_cache);
            surface.set_app(app);
            surface
        });
        self.terminal
            .view
            .surfaces
            .insert(session_id.to_string(), surface.clone());
        surface
    }

    pub(in crate::features) fn remove_terminal_surface(&mut self, session_id: &str) {
        self.terminal.view.surfaces.remove(session_id);
    }

    fn remember_terminal_scroll_window_snapshot(
        &mut self,
        session_id: &str,
        display_offset: usize,
        snapshot: &std::sync::Arc<TerminalSnapshot>,
    ) {
        if display_offset == 0 {
            return;
        }
        if let Some(view) = self.terminal.view.views.get_mut(session_id) {
            view.remember_scrollback_snapshot(display_offset, snapshot.clone());
        }
    }

    fn sync_terminal_scroll_text_first_surface_paint(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        let surface = self.ensure_terminal_surface(session_id, cx);
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return false;
        };
        let scroll_offset = view.scroll_offset;
        let scroll_residual_lines = self.terminal_scroll_residual_for_session(Some(session_id));
        let scrollback_len = view.scrollback_len_for_ui();
        let viewport_rows = view.viewport_rows_for_ui();
        let display_offset =
            terminal_visual_display_offset(scroll_offset, scroll_residual_lines, scrollback_len);
        let now = Instant::now();
        let user_scroll_active = terminal_user_scroll_active(
            display_offset,
            self.shell.terminal_user_scroll_idle_pending(session_id),
            self.shell.last_terminal_user_scroll_at(),
            now,
        );
        let input_latency_active =
            terminal_input_latency_active(self.shell.last_terminal_input_at(), now);
        let Some(snapshot) = terminal_paint_window_snapshot_for_view(
            Some(view),
            display_offset,
            viewport_rows,
            None,
        ) else {
            return false;
        };
        let action_links_enabled = self.settings.summary().terminal_action_links_enabled
            && !self.settings.summary().terminal_low_latency_mode;
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return false;
        };
        let palette = self.terminal_theme_palette();
        let transparent_background = self.wallpaper_enabled();
        let font = self.gpui_terminal_font();
        let font_family = font.family;
        let font_fallbacks = font.fallbacks;
        let font_size = self.settings.summary().terminal_font_size as f32;
        let normal_weight = self.settings.summary().terminal_font_weight as f32;
        let bold_weight = self.settings.summary().terminal_font_weight_bold as f32;
        let bold_default_foreground = self.settings.summary().bold_default_foreground;
        let show_line_numbers = self.settings.summary().terminal_show_line_numbers;
        let max_line_number_digits = view.max_line_number_digits;
        let show_timestamps = self.settings.summary().terminal_show_timestamps;
        let timestamp_format = self.settings.summary().terminal_timestamp_format.clone();
        let (cell_w, cell_h) = self
            .terminal
            .layout
            .cell_metrics
            .unwrap_or(((font_size * 0.6).max(6.0), (font_size * 1.35).max(12.0)));
        let is_active = self.session.active_id() == Some(session_id);
        let terminal_selection = terminal_selection_for_session(
            self.terminal.selection.selection,
            self.terminal.selection.session_id.as_deref(),
            self.session.active_id(),
            session_id,
        );
        let layout_cache = view.render_cache.layout_cache.clone();
        let render_degraded =
            view.render_degraded || self.settings.summary().terminal_low_latency_mode;
        let has_new = view.has_new_while_scrolled;
        let performance_overlay = view.performance_overlay;
        let skipped = view.skipped_output_chars;
        let protocol_state = view.protocol_state;
        let search_matches = if is_active
            && !input_latency_active
            && !self.settings.summary().terminal_low_latency_mode
            && self.terminal.search.open
            && self.terminal.search.mode == TerminalSearchMode::Buffer
        {
            self.terminal_buffer_matches().unwrap_or_default()
        } else {
            std::sync::Arc::from([])
        };
        let action_link_matcher_key = terminal_action_link_matcher_key(
            action_links_enabled,
            &self.settings.summary().terminal_action_links_matchers,
        );
        let frame_action_links = if action_links_enabled {
            crate::features::terminal::terminal_surface::terminal_action_links_for_paint_snapshot(
                Some(view),
                display_offset,
                snapshot.as_ref(),
                action_link_matcher_key,
            )
        } else {
            Default::default()
        };
        let needs_scroll_enrichment = super::buffer::terminal_scroll_enrichment_should_request(
            display_offset > 0,
            display_offset,
            action_links_enabled.then_some(action_link_matcher_key),
            Some(view),
            Some(snapshot.as_ref()),
        );
        let decorations = terminal_scroll_text_first_decorations(
            snapshot.as_ref(),
            (!search_matches.is_empty()).then_some(search_matches.as_ref()),
            &frame_action_links,
            action_links_enabled,
            action_links_enabled,
        );
        let has_action_link_decorations =
            crate::features::terminal::terminal_surface::terminal_action_links_have_ranges_for_snapshot(
                snapshot.as_ref(),
                &frame_action_links,
            );
        let configured_keyword_rules = self.resolved_keyword_highlight_rules();
        let clear_keyword_highlights = configured_keyword_rules.is_empty();
        let keyword_rules =
            if terminal_keyword_highlight_updates_allowed(is_active, input_latency_active) {
                configured_keyword_rules.clone()
            } else {
                std::sync::Arc::new(Vec::new())
            };
        let keyword_output_pressure = self.runtime_output_pressure_active();
        if needs_scroll_enrichment {
            let _ = self.request_terminal_frame_snapshot_for_scroll_enrichment(
                session_id,
                display_offset,
                Some(snapshot.as_ref()),
            );
        }
        if terminal_live_action_link_enrichment_allowed(
            display_offset,
            action_links_enabled,
            input_latency_active,
            render_degraded || keyword_output_pressure || user_scroll_active,
        ) {
            let _ =
                self.request_terminal_live_action_link_enrichment(session_id, snapshot.as_ref());
        }
        self.remember_terminal_scroll_window_snapshot(session_id, display_offset, &snapshot);
        surface.update(cx, |surface, cx| {
            let mut changed = false;
            changed |= surface.set_layout_cache(layout_cache);
            changed |= surface.set_background_transparent(transparent_background);
            changed |= surface.set_paint_chrome(TerminalSurfacePaintChrome {
                palette,
                font_family,
                font_fallbacks,
                font_size,
                normal_weight,
                bold_weight,
                bold_default_foreground,
                cell_width: cell_w,
                cell_height: cell_h,
                show_line_numbers,
                max_line_number_digits,
                show_timestamps,
                timestamp_format: timestamp_format.clone(),
                is_active,
            });
            changed |= surface.set_protocol_state(protocol_state);
            let had_pending_local_scroll_sync = surface.has_pending_local_scroll_sync();
            let frame_applied = surface.apply_frame_snapshot(
                TerminalSurfaceFrameSnapshot::new(
                    snapshot,
                    TerminalScrollVisualState {
                        session_id: session_id.to_string(),
                        scroll_offset,
                        scroll_residual_lines,
                        display_offset,
                        scrollback_len,
                        viewport_rows,
                        has_new_while_scrolled: has_new,
                        performance_overlay,
                        skipped_output_chars: skipped,
                    },
                )
                .with_presentation(has_action_link_decorations, false, "block"),
            );
            let paint_details_changed = if frame_applied
                || (!render_degraded && !user_scroll_active && !input_latency_active)
            {
                surface.set_decorations_and_keywords(decorations, keyword_rules, false, "block")
            } else {
                surface.set_decorations_and_keywords_preserving_stale(
                    decorations,
                    keyword_rules,
                    false,
                    "block",
                    true,
                )
            };
            let selection_changed = surface.set_selection_visual(terminal_selection);
            if frame_applied || paint_details_changed {
                surface.schedule_keyword_highlights(
                    clear_keyword_highlights,
                    keyword_output_pressure,
                    cx,
                );
            }
            if changed
                || frame_applied
                || paint_details_changed
                || selection_changed
                || had_pending_local_scroll_sync
            {
                cx.notify();
            }
        });
        true
    }

    /// Push the current view/frame paint state into the session surface and notify it.
    pub(in crate::features) fn sync_terminal_surface_paint(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        let is_active = self.session.active_id() == Some(session_id);
        let paint_started_at = Instant::now();
        self.ensure_paint_theme_caches();
        let surface = self.ensure_terminal_surface(session_id, cx);
        // Read only the lightweight row/column pair for the active surface; no
        // snapshot is cloned, and inactive surfaces do not need this check.
        let previous_cursor_position = if is_active {
            surface.read(cx).cursor_position()
        } else {
            None
        };
        let is_visible = self.terminal_session_has_visible_surface(session_id);
        let presentation = TerminalPresentation::resolve(is_active, is_visible);
        let work_policy = TerminalWorkPolicy::for_presentation(presentation);
        let is_disconnected = self.session.is_disconnected(session_id);
        let render_output_pressure = self.runtime_output_pressure_active();
        let view = self.terminal.view.views.get(session_id);
        let scroll_offset = view.map(|v| v.scroll_offset).unwrap_or(0);
        let scroll_residual_lines = self.terminal_scroll_residual_for_session(Some(session_id));
        let has_new = view.map(|v| v.has_new_while_scrolled).unwrap_or(false);
        let performance_overlay = view.and_then(|v| v.performance_overlay);
        let skipped = view.map(|v| v.skipped_output_chars).unwrap_or(0);
        let protocol_state = view.map(|v| v.protocol_state).unwrap_or_default();
        let target_line = view.and_then(|v| v.target_line);
        let zebra_stripes_enabled = self.settings.summary().terminal_zebra_stripes_enabled;
        let layout_cache = view
            .map(|v| v.render_cache.layout_cache.clone())
            .unwrap_or_else(|| {
                std::sync::Arc::new(std::sync::Mutex::new(NyaTerminalLayoutCache::default()))
            });
        let render_degraded_view = view.map(|v| v.render_degraded).unwrap_or(false);
        let burst = view.map(|v| v.output_burst_bytes).unwrap_or(0);
        let mode = view
            .map(|v| v.performance_mode)
            .unwrap_or(TerminalPerformanceMode::Normal);
        let scrollback_len = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.scrollback_len_for_ui())
            .unwrap_or(0);
        let viewport_rows = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.viewport_rows_for_ui())
            .unwrap_or(1);
        let display_offset =
            terminal_visual_display_offset(scroll_offset, scroll_residual_lines, scrollback_len);
        let user_scroll_active = terminal_user_scroll_active(
            display_offset,
            self.shell.terminal_user_scroll_idle_pending(session_id),
            self.shell.last_terminal_user_scroll_at(),
            paint_started_at,
        );
        let input_latency_active =
            terminal_input_latency_active(self.shell.last_terminal_input_at(), paint_started_at);
        let render_pressure = render_output_pressure
            || burst > 0
            || mode == TerminalPerformanceMode::Overloaded
            || user_scroll_active
            || input_latency_active
            || self.settings.summary().terminal_low_latency_mode;
        let render_degraded = render_degraded_view || render_pressure;
        let configured_keyword_rules = self.resolved_keyword_highlight_rules();
        let clear_keyword_highlights = configured_keyword_rules.is_empty();
        let keyword_rules =
            if !terminal_keyword_highlight_updates_allowed(is_active, input_latency_active) {
                std::sync::Arc::new(Vec::new())
            } else {
                configured_keyword_rules.clone()
            };
        let retained_lookup_started_at = Instant::now();
        let retained_surface_snapshot = if display_offset > 0 {
            surface
                .read(cx)
                .snapshot_covering_display_offset(display_offset, viewport_rows, scrollback_len)
                .filter(|snapshot| {
                    terminal_snapshot_covers_display_offset(
                        snapshot.as_ref(),
                        display_offset,
                        viewport_rows,
                        scrollback_len,
                    )
                })
                .filter(|snapshot| {
                    view.is_some_and(|view| {
                        terminal_retained_snapshot_matches_view(
                            snapshot.as_ref(),
                            view,
                            display_offset,
                            viewport_rows,
                        )
                    })
                })
        } else {
            None
        };
        let retained_lookup_duration = retained_lookup_started_at.elapsed();
        let retained_snapshot_reused = retained_surface_snapshot.is_some();
        let snapshot_started_at = Instant::now();
        let snapshot = terminal_paint_window_snapshot_for_view(
            view,
            display_offset,
            viewport_rows,
            retained_surface_snapshot,
        );
        let snapshot_duration = snapshot_started_at.elapsed();
        let action_links_enabled = self.settings.summary().terminal_action_links_enabled
            && !self.settings.summary().terminal_low_latency_mode;
        if let Some(snapshot) = snapshot.as_ref()
            && terminal_live_action_link_enrichment_allowed(
                display_offset,
                action_links_enabled,
                input_latency_active,
                render_degraded_view
                    || render_output_pressure
                    || burst > 0
                    || mode == TerminalPerformanceMode::Overloaded
                    || user_scroll_active,
            )
        {
            let _ =
                self.request_terminal_live_action_link_enrichment(session_id, snapshot.as_ref());
        }
        let view = self.terminal.view.views.get(session_id);
        let palette = self.terminal_theme_palette();
        let transparent_background = self.wallpaper_enabled();
        let font = self.gpui_terminal_font();
        let font_family = font.family;
        let font_fallbacks = font.fallbacks;
        let font_size = self.settings.summary().terminal_font_size as f32;
        let normal_weight = self.settings.summary().terminal_font_weight as f32;
        let bold_weight = self.settings.summary().terminal_font_weight_bold as f32;
        let bold_default_foreground = self.settings.summary().bold_default_foreground;
        let show_line_numbers = self.settings.summary().terminal_show_line_numbers;
        let max_line_number_digits = view.map_or(1, |view| view.max_line_number_digits);
        let show_timestamps = self.settings.summary().terminal_show_timestamps;
        let timestamp_format = self.settings.summary().terminal_timestamp_format.clone();
        let (cell_w, cell_h) = self
            .terminal
            .layout
            .cell_metrics
            .unwrap_or(((font_size * 0.6).max(6.0), (font_size * 1.35).max(12.0)));
        let Some(snapshot) = snapshot else {
            if let Some(request_offset) = terminal_scroll_snapshot_request_offset(
                scroll_offset,
                scroll_residual_lines,
                scrollback_len,
            ) {
                self.request_terminal_frame_snapshot_for_user_scroll(session_id, request_offset);
                if user_scroll_active {
                    if self.sync_terminal_scroll_text_first_surface_paint(session_id, cx) {
                        return;
                    }
                } else if self
                    .terminal
                    .view
                    .views
                    .get(session_id)
                    .is_some_and(|view| view.scrollback_snapshots.contains_key(&request_offset))
                    && self.sync_terminal_scroll_text_first_surface_paint(session_id, cx)
                {
                    return;
                }
                if self
                    .should_log_slow_diagnostic("terminal_scroll_snapshot_missing", Instant::now())
                {
                    tracing::warn!(
                        diagnostic = "terminal_scroll_snapshot_missing",
                        session_id = %session_id,
                        offset = request_offset,
                        "terminal scrolled paint retained current surface while waiting for snapshot"
                    );
                }
            }
            let scroll_state = TerminalScrollVisualState {
                session_id: session_id.to_string(),
                scroll_offset,
                scroll_residual_lines,
                display_offset,
                scrollback_len,
                viewport_rows,
                has_new_while_scrolled: has_new,
                performance_overlay,
                skipped_output_chars: skipped,
            };
            surface.update(cx, |surface, cx| {
                let mut changed = false;
                changed |= surface.set_layout_cache(layout_cache);
                changed |= surface.set_background_transparent(transparent_background);
                changed |= surface.set_paint_chrome(TerminalSurfacePaintChrome {
                    palette,
                    font_family,
                    font_fallbacks,
                    font_size,
                    normal_weight,
                    bold_weight,
                    bold_default_foreground,
                    cell_width: cell_w,
                    cell_height: cell_h,
                    show_line_numbers,
                    max_line_number_digits,
                    show_timestamps,
                    timestamp_format: timestamp_format.clone(),
                    is_active,
                });
                changed |= surface.set_protocol_state(protocol_state);
                changed |= surface.set_zebra_stripes(zebra_stripes_enabled, target_line);
                changed |= surface.update_scroll_chrome_without_snapshot(&scroll_state);
                if changed {
                    cx.notify();
                }
            });
            return;
        };
        let current_cursor_position = (snapshot.display_offset == 0
            && snapshot.cursor.row != usize::MAX)
            .then_some((snapshot.cursor.row, snapshot.cursor.col));
        if is_active
            && display_offset == 0
            && terminal_cursor_position_changed(previous_cursor_position, current_cursor_position)
        {
            // Full-screen programs such as Vim must show the new position even
            // when the previous frame was in the hidden blink phase.
            self.shell.reset_cursor_blink_phase();
        }
        let cursor_row = snapshot.cursor.row;
        let remote_cursor_visible = snapshot.cursor.visible
            && snapshot.cursor.shape != nyaterm_terminal::CursorShape::Hidden
            && cursor_row != usize::MAX;
        let blink_enabled = self.settings.summary().cursor_blink || snapshot.cursor.blinking;
        let show_cursor = terminal_cursor_visible_for_display_offset(
            is_active,
            is_disconnected,
            display_offset,
            remote_cursor_visible,
            blink_enabled,
            self.shell.cursor_blink_on(),
        );
        let cursor_style = match snapshot.cursor.shape {
            nyaterm_terminal::CursorShape::Underline => "underline".to_string(),
            nyaterm_terminal::CursorShape::Beam => "bar".to_string(),
            nyaterm_terminal::CursorShape::Hidden => self.settings.summary().cursor_style.clone(),
            nyaterm_terminal::CursorShape::Block => self.settings.summary().cursor_style.clone(),
        };

        let search_mapping_started_at = Instant::now();
        let paint_policy = EffectiveTerminalPaintPolicy::resolve(
            presentation,
            render_degraded,
            render_output_pressure,
            burst,
            mode,
            action_links_enabled,
        );
        let enhanced = paint_policy.enhanced_decorations;
        let action_link_matcher_key = terminal_action_link_matcher_key(
            action_links_enabled,
            &self.settings.summary().terminal_action_links_matchers,
        );
        let frame_action_links = if action_links_enabled {
            crate::features::terminal::terminal_surface::terminal_action_links_for_paint_snapshot(
                view,
                display_offset,
                snapshot.as_ref(),
                action_link_matcher_key,
            )
        } else {
            Default::default()
        };
        let needs_scroll_enrichment = super::buffer::terminal_scroll_enrichment_should_request(
            display_offset > 0,
            display_offset,
            action_links_enabled.then_some(action_link_matcher_key),
            view,
            Some(snapshot.as_ref()),
        );

        let mut search_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        let mut active_search_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        let mut selected_occurrence_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> =
            HashMap::new();
        let selected_occurrence_matches = if work_policy.active_decorations {
            self.terminal_selected_occurrence_matches_for_session(session_id)
                .unwrap_or_default()
        } else {
            Default::default()
        };
        // Selected occurrences are explicit user feedback. Keep them visible
        // while optional decorations are degraded under output pressure.
        if work_policy.active_decorations {
            let (abs_start, abs_end) =
                crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
                    &snapshot,
                );
            for search_match in
                selected_occurrence_matches.iter_in_absolute_range(abs_start..abs_end)
            {
                let abs = search_match.line_index;
                selected_occurrence_ranges_by_line
                    .entry(abs - abs_start)
                    .or_default()
                    .push((search_match.start_col, search_match.end_col));
            }
        }
        if enhanced
            && is_active
            && self.terminal.search.open
            && self.terminal.search.mode == TerminalSearchMode::Buffer
        {
            let search_matches = self.terminal_buffer_matches().unwrap_or_default();
            let (abs_start, abs_end) =
                crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
                    &snapshot,
                );
            let active_match_abs = search_matches
                .get(
                    self.terminal
                        .search
                        .active_index
                        .min(search_matches.len().saturating_sub(1)),
                )
                .map(|search_match| search_match.line_index);
            let visible_matches = crate::features::terminal::terminal_search_runtime::terminal_matches_in_absolute_range(
                &search_matches,
                abs_start..abs_end,
            );
            let first_visible_index =
                search_matches.partition_point(|search_match| search_match.line_index < abs_start);
            for (visible_index, search_match) in visible_matches.iter().enumerate() {
                let match_index = first_visible_index + visible_index;
                let abs = search_match.line_index;
                let view_row = abs - abs_start;
                let range = (search_match.start_col, search_match.end_col);
                search_ranges_by_line
                    .entry(view_row)
                    .or_default()
                    .push(range);
                if Some(abs) == active_match_abs
                    && match_index
                        == self
                            .terminal
                            .search
                            .active_index
                            .min(search_matches.len().saturating_sub(1))
                {
                    active_search_ranges_by_line
                        .entry(view_row)
                        .or_default()
                        .push(range);
                }
            }
        }
        let search_mapping_duration = search_mapping_started_at.elapsed();

        // The actual selection is always painted, even while degraded. Only
        // weak occurrence/search overlays may be omitted under output pressure.
        let terminal_selection = terminal_selection_for_session(
            self.terminal.selection.selection,
            self.terminal.selection.session_id.as_deref(),
            self.session.active_id(),
            session_id,
        );
        let has_search_decorations =
            !search_ranges_by_line.is_empty() || !active_search_ranges_by_line.is_empty();
        let has_frame_action_links = action_links_enabled
            && crate::features::terminal::terminal_surface::terminal_action_links_have_ranges_for_snapshot(
                &snapshot,
                &frame_action_links,
            );
        let has_hyperlinks =
            action_links_enabled && snapshot.rows().iter().any(|row| !row.hyperlinks.is_empty());
        let decorations_started_at = Instant::now();
        let decorations =
            if crate::features::terminal::terminal_surface::terminal_line_decorations_needed(
                !selected_occurrence_ranges_by_line.is_empty(),
                has_search_decorations,
                has_frame_action_links,
                has_hyperlinks,
            ) {
                let include_action_links = action_links_enabled;
                let include_hyperlinks = action_links_enabled;
                let decoration_sources =
                    crate::features::terminal::terminal_surface::TerminalDecorationSources {
                        selected_occurrence_ranges_by_line: &selected_occurrence_ranges_by_line,
                        search_ranges_by_line: &search_ranges_by_line,
                        active_search_ranges_by_line: &active_search_ranges_by_line,
                        frame_action_links: &frame_action_links,
                        include_action_links,
                        include_hyperlinks,
                    };
                let decoration_cache_key = crate::features::terminal::terminal_surface::terminal_line_decorations_cache_key(
                    &snapshot,
                    &decoration_sources,
                );
                let build = || {
                    crate::features::terminal::terminal_surface::build_terminal_line_decorations(
                        &snapshot,
                        &decoration_sources,
                    )
                };
                if let Some(view) = self.terminal.view.views.get(session_id) {
                    view.render_cache
                        .line_decorations(decoration_cache_key, build)
                } else {
                    build().into()
                }
            } else {
                std::sync::Arc::from(Vec::<TerminalLineDecorations>::new())
            };
        let decorations_duration = decorations_started_at.elapsed();

        if needs_scroll_enrichment {
            let _ = self.request_terminal_frame_snapshot_for_scroll_enrichment(
                session_id,
                display_offset,
                Some(snapshot.as_ref()),
            );
        }

        let prep_duration = paint_started_at.elapsed();
        if display_offset > 0
            && (prep_duration >= TERMINAL_SCROLL_PAINT_SLOW_TOTAL
                || snapshot_duration >= TERMINAL_SCROLL_PAINT_SLOW_STAGE
                || search_mapping_duration >= TERMINAL_SCROLL_PAINT_SLOW_STAGE
                || decorations_duration >= TERMINAL_SCROLL_PAINT_SLOW_STAGE)
            && self.should_log_slow_diagnostic("terminal_scroll_paint_prepare", Instant::now())
        {
            tracing::warn!(
                diagnostic = "terminal_scroll_paint_prepare",
                session_id = %session_id,
                scroll_offset,
                display_offset,
                residual_lines = scroll_residual_lines,
                viewport_rows,
                scrollback_len,
                snapshot_rows = snapshot.row_count(),
                retained_snapshot_reused,
                retained_lookup_us = retained_lookup_duration.as_micros(),
                snapshot_us = snapshot_duration.as_micros(),
                search_mapping_us = search_mapping_duration.as_micros(),
                decorations_us = decorations_duration.as_micros(),
                total_us = prep_duration.as_micros(),
                "slow terminal scroll paint preparation"
            );
        }

        let overview_marker_key = self
            .terminal_overview_marker_key_for_session_with_selected_matches(
                session_id,
                &selected_occurrence_matches,
            );
        let overview_markers_dirty = surface.read(cx).overview_marker_key() != overview_marker_key;
        let (overview_markers, overview_total_rows) = if overview_markers_dirty {
            let (markers, total_rows) = self
                .terminal_overview_markers_for_session_with_selected_matches(
                    session_id,
                    &selected_occurrence_matches,
                );
            (Some(markers.into()), total_rows)
        } else {
            (None, 1)
        };
        self.remember_terminal_scroll_window_snapshot(session_id, display_offset, &snapshot);
        surface.update(cx, |surface, cx| {
            let mut changed = false;
            if let Some(overview_markers) = overview_markers {
                changed |= surface.set_overview_markers(
                    overview_markers,
                    overview_total_rows,
                    overview_marker_key,
                );
            }
            changed |= surface.set_layout_cache(layout_cache);
            changed |= surface.set_background_transparent(transparent_background);
            changed |= surface.set_paint_chrome(TerminalSurfacePaintChrome {
                palette,
                font_family,
                font_fallbacks,
                font_size,
                normal_weight,
                bold_weight,
                bold_default_foreground,
                cell_width: cell_w,
                cell_height: cell_h,
                show_line_numbers,
                max_line_number_digits,
                show_timestamps,
                timestamp_format,
                is_active,
            });
            changed |= surface.set_protocol_state(protocol_state);
            changed |= surface.set_zebra_stripes(zebra_stripes_enabled, target_line);
            let had_pending_local_scroll_sync = surface.has_pending_local_scroll_sync();
            let frame_applied = surface.apply_frame_snapshot(
                TerminalSurfaceFrameSnapshot::new(
                    snapshot,
                    TerminalScrollVisualState {
                        session_id: session_id.to_string(),
                        scroll_offset,
                        scroll_residual_lines,
                        display_offset,
                        scrollback_len,
                        viewport_rows,
                        has_new_while_scrolled: has_new,
                        performance_overlay,
                        skipped_output_chars: skipped,
                    },
                )
                .with_presentation(
                    has_frame_action_links,
                    show_cursor,
                    cursor_style.clone(),
                ),
            );
            let paint_details_changed = if frame_applied || !render_degraded {
                surface.set_decorations_and_keywords(
                    decorations,
                    keyword_rules,
                    show_cursor,
                    cursor_style,
                )
            } else {
                surface.set_decorations_and_keywords_preserving_stale(
                    decorations,
                    keyword_rules,
                    show_cursor,
                    cursor_style,
                    true,
                )
            };
            let selection_changed = surface.set_selection_visual(terminal_selection);
            if frame_applied || paint_details_changed {
                surface.schedule_keyword_highlights(
                    clear_keyword_highlights,
                    render_output_pressure,
                    cx,
                );
            }
            if changed
                || frame_applied
                || paint_details_changed
                || selection_changed
                || had_pending_local_scroll_sync
            {
                cx.notify();
            }
        });
    }

    pub(in crate::features) fn notify_terminal_scroll_position_only(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        let notify_started_at = Instant::now();
        let surface = self.ensure_terminal_surface(session_id, cx);
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return;
        };
        let scroll_offset = view.scroll_offset;
        let scroll_residual_lines = self.terminal_scroll_residual_for_session(Some(session_id));
        let scrollback_len = view.scrollback_len_for_ui();
        let viewport_rows = view.viewport_rows_for_ui();
        let display_offset =
            terminal_visual_display_offset(scroll_offset, scroll_residual_lines, scrollback_len);
        let has_new = view.has_new_while_scrolled;
        let performance_overlay = view.performance_overlay;
        let skipped = view.skipped_output_chars;
        let desired_scroll_state = TerminalScrollVisualState {
            session_id: session_id.to_string(),
            scroll_offset,
            scroll_residual_lines,
            display_offset,
            scrollback_len,
            viewport_rows,
            has_new_while_scrolled: has_new,
            performance_overlay,
            skipped_output_chars: skipped,
        };
        let (can_reuse_snapshot, scroll_state_current) = {
            let surface = surface.read(cx);
            (
                surface.has_snapshot_covering_display_offset(
                    display_offset,
                    viewport_rows,
                    scrollback_len,
                ),
                surface.scroll_visual_state_matches(&desired_scroll_state),
            )
        };
        if can_reuse_snapshot && scroll_state_current {
            return;
        }
        if !can_reuse_snapshot {
            if display_offset > 0 {
                self.request_terminal_frame_snapshot_for_user_scroll(session_id, display_offset);
                if self.sync_terminal_scroll_text_first_surface_paint(session_id, cx) {
                    let elapsed = notify_started_at.elapsed();
                    if elapsed >= TERMINAL_SCROLL_POSITION_NOTIFY_SLOW
                        && self.should_log_slow_diagnostic(
                            "terminal_scroll_position_notify",
                            Instant::now(),
                        )
                    {
                        tracing::warn!(
                            diagnostic = "terminal_scroll_position_notify",
                            session_id = %session_id,
                            scroll_offset,
                            display_offset,
                            residual_lines = scroll_residual_lines,
                            viewport_rows,
                            scrollback_len,
                            can_reuse_snapshot = false,
                            text_first = true,
                            elapsed_us = elapsed.as_micros(),
                            "slow terminal scroll position notify"
                        );
                    }
                    return;
                }
                if self.should_log_slow_diagnostic("terminal_scroll_snapshot_wait", Instant::now())
                {
                    tracing::warn!(
                        diagnostic = "terminal_scroll_snapshot_wait",
                        session_id = %session_id,
                        offset = display_offset,
                        "terminal scroll retained current surface while waiting for target snapshot"
                    );
                }
            }
            surface.update(cx, |surface, cx| {
                let changed = surface.update_scroll_chrome_without_snapshot(&desired_scroll_state);
                if changed {
                    cx.notify();
                }
            });
            let elapsed = notify_started_at.elapsed();
            if elapsed >= TERMINAL_SCROLL_POSITION_NOTIFY_SLOW
                && self
                    .should_log_slow_diagnostic("terminal_scroll_position_notify", Instant::now())
            {
                tracing::warn!(
                    diagnostic = "terminal_scroll_position_notify",
                    session_id = %session_id,
                    scroll_offset,
                    display_offset,
                    residual_lines = scroll_residual_lines,
                    viewport_rows,
                    scrollback_len,
                    can_reuse_snapshot = false,
                    elapsed_us = elapsed.as_micros(),
                    "slow terminal scroll position notify"
                );
            }
            return;
        }
        surface.update(cx, |surface, cx| {
            let changed = surface.update_scroll_position_without_snapshot(&desired_scroll_state);
            if changed {
                cx.notify();
            }
        });
        let elapsed = notify_started_at.elapsed();
        if elapsed >= TERMINAL_SCROLL_POSITION_NOTIFY_SLOW
            && self.should_log_slow_diagnostic("terminal_scroll_position_notify", Instant::now())
        {
            tracing::warn!(
                diagnostic = "terminal_scroll_position_notify",
                session_id = %session_id,
                scroll_offset,
                display_offset,
                residual_lines = scroll_residual_lines,
                viewport_rows,
                scrollback_len,
                can_reuse_snapshot = true,
                elapsed_us = elapsed.as_micros(),
                "slow terminal scroll position notify"
            );
        }
    }

    pub(in crate::features) fn notify_terminal_scroll_visual_only(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        let notify_started_at = Instant::now();
        let surface = self.ensure_terminal_surface(session_id, cx);
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return;
        };
        let scroll_offset = view.scroll_offset;
        let scroll_residual_lines = self.terminal_scroll_residual_for_session(Some(session_id));
        let scrollback_len = view.scrollback_len_for_ui();
        let viewport_rows = view.viewport_rows_for_ui();
        let display_offset =
            terminal_visual_display_offset(scroll_offset, scroll_residual_lines, scrollback_len);
        let has_new = view.has_new_while_scrolled;
        let performance_overlay = view.performance_overlay;
        let skipped = view.skipped_output_chars;
        let desired_scroll_state = TerminalScrollVisualState {
            session_id: session_id.to_string(),
            scroll_offset,
            scroll_residual_lines,
            display_offset,
            scrollback_len,
            viewport_rows,
            has_new_while_scrolled: has_new,
            performance_overlay,
            skipped_output_chars: skipped,
        };
        let (can_reuse_snapshot, scroll_state_current) = {
            let surface = surface.read(cx);
            (
                surface.has_snapshot_covering_display_offset(
                    display_offset,
                    viewport_rows,
                    scrollback_len,
                ),
                surface.scroll_visual_state_matches(&desired_scroll_state),
            )
        };
        if can_reuse_snapshot && scroll_state_current {
            return;
        }
        if !can_reuse_snapshot && display_offset > 0 {
            self.request_terminal_frame_snapshot_for_user_scroll(session_id, display_offset);
        }
        surface.update(cx, |surface, cx| {
            let changed = if can_reuse_snapshot {
                surface.update_scroll_position_without_snapshot(&desired_scroll_state)
            } else {
                surface.update_scroll_chrome_without_snapshot(&desired_scroll_state)
            };
            if changed {
                cx.notify();
            }
        });
        let elapsed = notify_started_at.elapsed();
        if elapsed >= TERMINAL_SCROLL_POSITION_NOTIFY_SLOW
            && self.should_log_slow_diagnostic("terminal_scroll_visual_notify", Instant::now())
        {
            tracing::warn!(
                diagnostic = "terminal_scroll_visual_notify",
                session_id = %session_id,
                scroll_offset,
                display_offset,
                residual_lines = scroll_residual_lines,
                viewport_rows,
                scrollback_len,
                can_reuse_snapshot,
                elapsed_us = elapsed.as_micros(),
                "slow terminal scroll visual notify"
            );
        }
    }

    /// Notify surface only (no full shell). Used for cursor blink.
    pub(in crate::features) fn notify_active_terminal_surface(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        self.sync_terminal_surface_paint(&session_id, cx);
    }

    /// Surface-only repaint for the given session (scroll / selection / frame).
    pub(in crate::features) fn notify_terminal_surface_only(
        &mut self,
        session_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let session_id = session_id
            .map(str::to_string)
            .or_else(|| self.session.active_id_owned());
        let Some(session_id) = session_id else {
            return;
        };
        if session_id.is_empty() {
            return;
        }
        self.sync_terminal_surface_paint(&session_id, cx);
    }

    pub(in crate::features) fn notify_terminal_selection_visual_only(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        let Some(surface) = self.terminal.view.surfaces.get(session_id).cloned() else {
            self.sync_terminal_surface_paint(session_id, cx);
            return;
        };
        let selection = self.terminal.selection.selection;
        let visual_state_ready = surface.update(cx, |surface, cx| {
            if surface.set_selection_visual(selection) {
                cx.notify();
                true
            } else {
                surface.has_snapshot()
            }
        });
        if !visual_state_ready {
            self.sync_terminal_surface_paint(session_id, cx);
        }
    }
}

#[cfg(test)]
mod tests;
