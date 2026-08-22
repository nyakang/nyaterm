use std::collections::HashSet;
use std::sync::Arc;

use gpui::{App, Bounds, Context, FontFeatures, FontWeight, Pixels, Point, px};

use nyaterm_core::TerminalViewportInsets;
use nyaterm_terminal::TerminalSnapshot;

use crate::features::NyaTermApp;
use crate::features::formatting::terminal_timestamp_format_width_chars;
use crate::features::shell::ResolvedAppearanceFont;
use crate::features::terminal::terminal_surface_entity::{
    TerminalVisualScrollGeometry, terminal_effective_visual_scroll_offset_px,
    terminal_snapshot_anchor_row_for_display_offset,
};
use crate::models::TerminalCellPos;

use super::{CELL_WIDTH_RATIO, LINE_HEIGHT_RATIO};
use crate::features::terminal::state::TerminalFontMetricsCache;

fn terminal_font_with_features(
    descriptor: &ResolvedAppearanceFont,
    weight: FontWeight,
) -> gpui::Font {
    let mut font = descriptor.font();
    font.features = FontFeatures::disable_ligatures();
    font.weight = weight;
    font
}

fn measure_terminal_font(
    text_system: &gpui::TextSystem,
    descriptor: &ResolvedAppearanceFont,
    size: Pixels,
    weight: FontWeight,
) -> Option<f32> {
    let font = terminal_font_with_features(descriptor, weight);
    let font_id = text_system.resolve_font(&font);
    let resolved = text_system.get_font_for_id(font_id)?;

    // TextSystem silently falls back to the global UI font when the requested family is missing.
    // Reject that result before a proportional font reaches the fixed-width terminal painter.
    if !resolved
        .family
        .as_str()
        .eq_ignore_ascii_case(font.family.as_str())
    {
        return None;
    }

    let widths = ['i', 'W', '0', 'm']
        .into_iter()
        .filter_map(|ch| {
            text_system
                .advance(font_id, size, ch)
                .ok()
                .map(|size| size.width)
        })
        .map(f32::from)
        .collect::<Vec<_>>();
    if widths.len() != 4
        || widths
            .iter()
            .any(|width| !width.is_finite() || *width <= 1.0)
    {
        return None;
    }

    let min_width = widths.iter().copied().fold(f32::INFINITY, f32::min);
    let max_width = widths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if max_width - min_width > (max_width * 0.02).max(0.25) {
        return None;
    }

    // The forced-width layout must agree with the raw glyph advance. A large discrepancy is
    // another sign that this font's platform shaping path cannot be used for terminal cells.
    let shaped_zero_width = f32::from(text_system.layout_width(font_id, size, '0'));
    if !shaped_zero_width.is_finite()
        || (shaped_zero_width - widths[2]).abs() > (widths[2] * 0.05).max(0.5)
    {
        return None;
    }

    Some(widths[2])
}

impl NyaTermApp {
    pub(in crate::features) fn terminal_cell_size(&self) -> (f32, f32) {
        let (cell_w, cell_h) = self
            .terminal
            .layout
            .cell_metrics
            .unwrap_or_else(|| self.fallback_terminal_cell_size());
        (
            cell_w,
            nyaterm_core::terminal_snapped_cell_height(cell_h, self.terminal.layout.scale_factor),
        )
    }

    pub(in crate::features) fn fallback_terminal_cell_size(&self) -> (f32, f32) {
        let font_size = self.settings.summary().terminal_font_size.max(8) as f32;
        // Prefer painted fixed 18px when font is near default; scale with font otherwise.
        let cell_h = if (font_size - 14.).abs() < 0.5 {
            18.
        } else {
            (font_size * LINE_HEIGHT_RATIO).max(font_size + 2.)
        };
        // Tauri gutter fallback uses fontSize * 0.62 when measured cell is unavailable.
        let cell_w = (font_size * CELL_WIDTH_RATIO).max(4.);
        (cell_w, cell_h)
    }

    /// Refresh monospaced cell metrics from GPUI TextSystem for the configured terminal font.
    pub(in crate::features) fn refresh_terminal_cell_metrics(&mut self, cx: &App) {
        let settings = self.settings.summary();
        let font_size = settings.terminal_font_size.max(8) as f32;
        let text_system = cx.text_system();
        let size = px(font_size);
        let configured_font = self.gpui_configured_terminal_font();
        let weight = FontWeight(settings.terminal_font_weight as f32);
        let cached = self
            .terminal
            .layout
            .font_metrics_cache
            .as_ref()
            .filter(|cache| {
                cache.configured_family == settings.terminal_font_family
                    && cache.font_size == settings.terminal_font_size
                    && cache.font_weight == settings.terminal_font_weight
            })
            .cloned();
        let (terminal_font, measured_w, override_font) = if let Some(cache) = cached {
            let descriptor = cache
                .resolved_font
                .clone()
                .unwrap_or_else(|| configured_font.clone());
            let font = terminal_font_with_features(&descriptor, weight);
            (font, cache.cell_width, cache.resolved_font)
        } else {
            let result =
                self.resolve_terminal_font_metrics(text_system, configured_font, size, weight);
            self.terminal.layout.font_metrics_cache = Some(TerminalFontMetricsCache {
                configured_family: settings.terminal_font_family.clone(),
                font_size: settings.terminal_font_size,
                font_weight: settings.terminal_font_weight,
                resolved_font: result.2.clone(),
                cell_width: result.1,
            });
            result
        };
        self.terminal.set_terminal_font_override(override_font);
        let font_id = text_system.resolve_font(&terminal_font);
        let ascent = f32::from(text_system.ascent(font_id, size));
        let descent = f32::from(text_system.descent(font_id, size)).abs();
        let font_line = (ascent + descent).max(font_size + 2.);
        // Keep painter contract: default 14px font paints ~18px rows.
        let cell_h = if (font_size - 14.).abs() < 0.5 {
            18.
        } else {
            (font_size * LINE_HEIGHT_RATIO).max(font_line)
        };
        let cell_w = measured_w;
        let next = (cell_w, cell_h);
        if self.terminal.layout.cell_metrics != Some(next) {
            self.terminal.layout.cell_metrics = Some(next);
        }
    }

    fn resolve_terminal_font_metrics(
        &self,
        text_system: &gpui::TextSystem,
        configured_font: ResolvedAppearanceFont,
        size: Pixels,
        weight: FontWeight,
    ) -> (gpui::Font, f32, Option<ResolvedAppearanceFont>) {
        if let Some(width) = measure_terminal_font(text_system, &configured_font, size, weight) {
            let font = terminal_font_with_features(&configured_font, weight);
            return (font, width, None);
        }

        let mut candidates = Vec::<ResolvedAppearanceFont>::new();
        let mut seen_families = HashSet::new();
        let mut add_candidate = |candidate: ResolvedAppearanceFont| {
            let family = candidate.family.trim().to_ascii_lowercase();
            if !family.is_empty() && seen_families.insert(family) {
                candidates.push(candidate);
            }
        };
        // Preserve the user's fallback order before applying platform defaults.
        for family in &configured_font.fallback_families {
            add_candidate(configured_font.with_primary_family(family));
        }
        for family in self.settings.terminal_font_options() {
            add_candidate(self.gpui_terminal_font_for_family(family));
        }
        #[cfg(target_os = "macos")]
        for family in ["Menlo", "Monaco", "SF Mono"] {
            add_candidate(self.gpui_terminal_font_for_family(family));
        }
        #[cfg(target_os = "windows")]
        for family in ["Consolas", "Cascadia Mono"] {
            add_candidate(self.gpui_terminal_font_for_family(family));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        for family in ["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono"] {
            add_candidate(self.gpui_terminal_font_for_family(family));
        }
        // Enumerate the system only after preferred candidates. The result is
        // still evaluated once and then retained by font_metrics_cache.
        for family in text_system.all_font_names() {
            add_candidate(self.gpui_terminal_font_for_family(&family));
        }

        for candidate in candidates {
            if let Some(width) = measure_terminal_font(text_system, &candidate, size, weight) {
                let font = terminal_font_with_features(&candidate, weight);
                tracing::warn!(
                    configured_font = %configured_font.family,
                    fallback_font = %candidate.family,
                    "configured terminal font is unavailable or not monospaced; using a local fallback"
                );
                return (font, width, Some(candidate));
            }
        }

        let font = terminal_font_with_features(&configured_font, weight);
        let font_id = text_system.resolve_font(&font);
        let width = text_system
            .ch_advance(font_id, size)
            .or_else(|_| text_system.em_advance(font_id, size))
            .ok()
            .map(f32::from)
            .filter(|width| width.is_finite() && *width > 1.0)
            .unwrap_or(8.0);
        (font, width, None)
    }

    pub(in crate::features) fn terminal_content_insets(&self) -> TerminalViewportInsets {
        if self.settings.summary().terminal_show_workspace_padding
            && !self.settings.summary().terminal_show_line_numbers
            && !self.settings.summary().terminal_show_timestamps
        {
            // Tauri applies workspace padding as `pl-2`; it does not add a
            // default margin or vertical padding around the terminal grid.
            TerminalViewportInsets {
                left: 8.,
                right: 0.,
                top: 0.,
                bottom: 0.,
            }
        } else {
            TerminalViewportInsets::symmetric(0.)
        }
    }

    pub(in crate::features) fn terminal_gutter_width_px(&self) -> f32 {
        self.terminal_gutter_width_px_for_session(self.session.active_id())
    }

    pub(in crate::features) fn terminal_gutter_width_px_for_session(
        &self,
        session_id: Option<&str>,
    ) -> f32 {
        let (cell_w, _) = self.terminal_cell_size();
        let display_offset = self.terminal_display_offset_for_session(session_id);
        let snapshot = self.terminal_snapshot_for_session(session_id, display_offset);
        terminal_gutter_metrics(
            cell_w,
            self.settings.summary().terminal_show_timestamps,
            terminal_timestamp_format_width_chars(
                &self.settings.summary().terminal_timestamp_format,
            ),
            self.settings.summary().terminal_show_line_numbers,
            terminal_line_number_digits(snapshot.as_ref()),
        )
        .total_width()
    }

    pub(in crate::features) fn active_terminal_grid_size(&self) -> (usize, usize) {
        self.terminal_grid_size_for_session(self.session.active_id())
    }

    pub(in crate::features) fn terminal_grid_size_for_session(
        &self,
        session_id: Option<&str>,
    ) -> (usize, usize) {
        let rows = self.terminal_viewport_rows_for_session(session_id);
        let cols = session_id
            .filter(|id| !id.is_empty())
            .and_then(|id| self.terminal.view.views.get(id))
            .map(|view| view.screen.cols())
            .unwrap_or_else(|| {
                let offset = self.terminal_display_offset_for_session(session_id);
                self.terminal_snapshot_for_session(session_id, offset).cols
            })
            .max(1);
        (rows, cols)
    }

    pub(in crate::features) fn terminal_content_insets_for_bounds(
        &self,
        session_id: Option<&str>,
        bounds: Bounds<Pixels>,
    ) -> TerminalViewportInsets {
        let session_id = session_id.filter(|id| !id.is_empty());
        if session_id.is_some_and(|id| {
            self.terminal
                .layout
                .session_surface_bounds
                .get(id)
                .is_some_and(|tracked| *tracked == bounds)
        }) {
            // TerminalSurface is already laid out inside the shell's padding.
            // Its bounds are the content box, so applying the shell inset again
            // shifts resize and pointer geometry by one padding width.
            TerminalViewportInsets::symmetric(0.)
        } else {
            self.terminal_content_insets()
        }
    }

    pub(in crate::features) fn terminal_viewport_rows_for_session(
        &self,
        session_id: Option<&str>,
    ) -> usize {
        let session_id = session_id.filter(|id| !id.is_empty());
        if let Some(session_id) = session_id
            && let Some(view) = self.terminal.view.views.get(session_id)
        {
            return view.viewport_rows_for_ui();
        }
        self.terminal.view.screen.rows().max(1)
    }

    pub(in crate::features) fn terminal_scrollback_len_for_session(
        &self,
        session_id: Option<&str>,
    ) -> usize {
        let session_id = session_id.filter(|id| !id.is_empty());
        if let Some(session_id) = session_id
            && let Some(view) = self.terminal.view.views.get(session_id)
        {
            return view.scrollback_len_for_ui();
        }
        self.terminal.view.screen.scrollback_len()
    }

    pub(in crate::features) fn terminal_snapshot_row_for_session_viewport_row(
        &self,
        session_id: Option<&str>,
        snapshot: &nyaterm_terminal::TerminalSnapshot,
        display_offset: usize,
        viewport_row: usize,
    ) -> Option<usize> {
        terminal_snapshot_row_for_viewport_row(
            snapshot,
            display_offset,
            self.terminal_viewport_rows_for_session(session_id),
            self.terminal_scrollback_len_for_session(session_id),
            viewport_row,
        )
    }

    pub(in crate::features) fn point_to_terminal_cell(
        &self,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<TerminalCellPos> {
        self.point_to_terminal_cell_for_session(self.session.active_id(), position, cx)
    }

    pub(in crate::features) fn point_to_terminal_cell_for_session(
        &self,
        session_id: Option<&str>,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<TerminalCellPos> {
        let geometry = self.terminal_hit_test_geometry_for_session(session_id, cx)?;
        Some(terminal_cell_for_visual_geometry(position, &geometry))
    }

    pub(in crate::features) fn terminal_hit_test_geometry_for_session(
        &self,
        session_id: Option<&str>,
        cx: &App,
    ) -> Option<TerminalHitTestGeometry> {
        let session_id = session_id.filter(|id| !id.is_empty());
        let bounds = session_id
            .and_then(|id| self.terminal.layout.session_surface_bounds.get(id).copied())
            .or(self.terminal.layout.surface_bounds)?;
        let (fallback_cell_w, fallback_cell_h) = self.terminal_cell_size();
        let insets = self.terminal_content_insets_for_bounds(session_id, bounds);
        if let Some((painted, snapshot, grid_bounds)) = session_id
            .and_then(|session_id| self.terminal.view.surfaces.get(session_id))
            .and_then(|surface| surface.read(cx).painted_hit_test_state())
            .and_then(|(painted, snapshot)| {
                painted
                    .grid_bounds
                    .map(|grid_bounds| (painted, snapshot, grid_bounds))
            })
        {
            let cols = snapshot.cols.max(1);
            return Some(TerminalHitTestGeometry {
                bounds: grid_bounds,
                snapshot: snapshot.clone(),
                cell_w: painted.cell_width,
                cell_h: painted.cell_height,
                padding_left: 0.0,
                padding_top: 0.0,
                gutter: 0.0,
                rows: painted.viewport_rows,
                cols,
                display_offset: painted.display_offset,
                viewport_anchor_row: painted.viewport_anchor_row,
                snapshot_rows: painted.snapshot_rows,
                viewport_rows: painted.viewport_rows,
                visual_y_offset: painted.visual_y_offset,
            });
        }
        let target_display_offset = self.terminal_display_offset_for_session(session_id);
        let snapshot = self.terminal_snapshot_for_session(session_id, target_display_offset);
        let viewport_rows = self.terminal_viewport_rows_for_session(session_id);
        let scrollback_len = self.terminal_scrollback_len_for_session(session_id);
        let fallback_viewport_anchor_row = terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            target_display_offset,
            viewport_rows,
            scrollback_len,
        );
        let visual_y_offset = terminal_hit_test_visual_y_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: false,
            target_offset: target_display_offset,
            displayed_offset: target_display_offset,
            residual_lines: 0.0,
            viewport_anchor_row: fallback_viewport_anchor_row,
            snapshot_rows: snapshot.row_count(),
            viewport_rows,
            cell_height: fallback_cell_h,
        });
        let gutter = terminal_gutter_metrics(
            fallback_cell_w,
            self.settings.summary().terminal_show_timestamps,
            terminal_timestamp_format_width_chars(
                &self.settings.summary().terminal_timestamp_format,
            ),
            self.settings.summary().terminal_show_line_numbers,
            terminal_line_number_digits(snapshot.as_ref()),
        )
        .total_width();
        Some(TerminalHitTestGeometry {
            bounds,
            snapshot: snapshot.clone(),
            cell_w: fallback_cell_w,
            cell_h: fallback_cell_h,
            padding_left: insets.left,
            padding_top: insets.top,
            gutter,
            rows: viewport_rows,
            cols: snapshot.cols.max(1),
            display_offset: target_display_offset,
            viewport_anchor_row: fallback_viewport_anchor_row,
            snapshot_rows: snapshot.row_count(),
            viewport_rows,
            visual_y_offset,
        })
    }

    /// Capture painted bounds for hit-testing; called from a canvas prepaint under the output area.
    pub(in crate::features) fn remember_terminal_surface_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.terminal.layout.surface_bounds = Some(bounds);
    }

    /// Capture painted bounds for a specific terminal pane and keep that pane's
    /// terminal model/backend PTY sized to its own viewport.
    pub(in crate::features) fn remember_terminal_surface_bounds_for_session(
        &mut self,
        session_id: Option<&str>,
        bounds: Bounds<Pixels>,
    ) -> bool {
        let session_id = session_id.filter(|id| !id.is_empty());
        if session_id.is_none() || session_id == self.session.active_id() {
            self.remember_terminal_surface_bounds(bounds);
        }
        if let Some(session_id) = session_id {
            self.terminal
                .layout
                .session_surface_bounds
                .insert(session_id.to_string(), bounds);
            self.resize_terminal_to_bounds_for_session(Some(session_id), bounds)
        } else {
            self.resize_terminal_to_bounds_for_session(None, bounds)
        }
    }

    pub(in crate::features) fn remember_terminal_surface_bounds_for_session_and_sync(
        &mut self,
        session_id: Option<&str>,
        bounds: Bounds<Pixels>,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) {
        self.terminal.layout.scale_factor = if scale_factor.is_finite() {
            scale_factor.max(1e-3)
        } else {
            1.0
        };
        let resized = self.remember_terminal_surface_bounds_for_session(session_id, bounds);
        if !resized {
            return;
        }
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()).map(str::to_string) {
            self.sync_terminal_surface_paint(&session_id, cx);
        } else {
            cx.notify();
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::features) struct TerminalHitTestGeometry {
    pub(in crate::features) bounds: Bounds<Pixels>,
    pub(in crate::features) snapshot: Arc<TerminalSnapshot>,
    pub(in crate::features) cell_w: f32,
    pub(in crate::features) cell_h: f32,
    pub(in crate::features) padding_left: f32,
    pub(in crate::features) padding_top: f32,
    pub(in crate::features) gutter: f32,
    pub(in crate::features) rows: usize,
    pub(in crate::features) cols: usize,
    pub(in crate::features) display_offset: usize,
    pub(in crate::features) viewport_anchor_row: usize,
    pub(in crate::features) snapshot_rows: usize,
    pub(in crate::features) viewport_rows: usize,
    pub(in crate::features) visual_y_offset: f32,
}

fn terminal_hit_test_visual_y_offset_px(geometry: TerminalVisualScrollGeometry) -> f32 {
    terminal_effective_visual_scroll_offset_px(geometry)
        - geometry.viewport_anchor_row as f32 * geometry.cell_height.max(1.0)
}

pub(in crate::features) fn terminal_cell_for_visual_geometry(
    position: Point<Pixels>,
    geometry: &TerminalHitTestGeometry,
) -> TerminalCellPos {
    let cell_w = geometry.cell_w.max(1.0);
    let cell_h = geometry.cell_h.max(1.0);
    let local_x =
        f32::from(position.x - geometry.bounds.origin.x) - geometry.padding_left - geometry.gutter;
    let local_y = f32::from(position.y - geometry.bounds.origin.y) - geometry.padding_top;
    let snapshot_row = ((local_y - geometry.visual_y_offset) / cell_h)
        .floor()
        .max(0.0) as usize;
    let row = snapshot_row.saturating_sub(geometry.viewport_anchor_row);
    let col = (local_x / cell_w).floor().max(0.0) as usize;
    TerminalCellPos::new(
        row.min(geometry.rows.saturating_sub(1)),
        col.min(geometry.cols.saturating_sub(1)),
    )
}

pub(in crate::features) fn terminal_snapshot_row_for_visual_geometry(
    position: Point<Pixels>,
    geometry: &TerminalHitTestGeometry,
) -> usize {
    let cell_h = geometry.cell_h.max(1.0);
    let local_y = f32::from(position.y - geometry.bounds.origin.y) - geometry.padding_top;
    ((local_y - geometry.visual_y_offset) / cell_h)
        .floor()
        .max(0.0) as usize
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(in crate::features) struct TerminalGutterMetrics {
    pub timestamp_width: f32,
    pub line_number_width: f32,
    pub gap_width: f32,
    pub trailing_padding_width: f32,
}

impl TerminalGutterMetrics {
    pub(in crate::features) fn total_width(self) -> f32 {
        self.timestamp_width + self.line_number_width + self.gap_width + self.trailing_padding_width
    }
}

pub(in crate::features) fn terminal_gutter_metrics(
    cell_width: f32,
    show_timestamps: bool,
    timestamp_width_chars: usize,
    show_line_numbers: bool,
    line_number_digits: usize,
) -> TerminalGutterMetrics {
    let cell_width = cell_width.max(1.0);
    let timestamp_width = if show_timestamps {
        (cell_width * timestamp_width_chars.clamp(1, 64) as f32).ceil() + 2.0
    } else {
        0.0
    };
    let line_number_width = if show_line_numbers {
        (cell_width * line_number_digits.max(1) as f32)
            .ceil()
            .max(22.0)
            + 2.0
    } else {
        0.0
    };
    let gap_width = if show_timestamps && show_line_numbers {
        8.0
    } else {
        0.0
    };
    let trailing_padding_width = if timestamp_width > 0.0 || line_number_width > 0.0 {
        // Tauri: 8px right padding, a 1px border, and a 10px separator gap.
        19.0
    } else {
        0.0
    };

    TerminalGutterMetrics {
        timestamp_width,
        line_number_width,
        gap_width,
        trailing_padding_width,
    }
}

pub(in crate::features) fn terminal_line_number_digits(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
) -> usize {
    let visible_end = snapshot
        .total_rows
        .saturating_sub(snapshot.display_offset)
        .max(1);
    terminal_line_number_digits_for_end(visible_end)
}

fn terminal_line_number_digits_for_end(visible_end: usize) -> usize {
    visible_end.max(1).to_string().len()
}

pub(in crate::features) fn terminal_snapshot_row_for_viewport_row(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
    viewport_row: usize,
) -> Option<usize> {
    if viewport_row >= viewport_rows.max(1) {
        return None;
    }
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        snapshot,
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    anchor
        .checked_add(viewport_row)
        .filter(|row| *row < snapshot.row_count())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{Point, Size, px};
    use nyaterm_terminal::TerminalScreen;

    use crate::features::terminal::terminal_surface::terminal_absolute_line_for_snapshot_row;
    use crate::models::TerminalCellPos;

    use super::{
        TerminalHitTestGeometry, TerminalVisualScrollGeometry, terminal_cell_for_visual_geometry,
        terminal_gutter_metrics, terminal_hit_test_visual_y_offset_px,
        terminal_line_number_digits_for_end, terminal_snapshot_row_for_viewport_row,
        terminal_snapshot_row_for_visual_geometry,
    };

    fn test_hit_test_snapshot() -> Arc<nyaterm_terminal::TerminalSnapshot> {
        Arc::new(TerminalScreen::default().viewport_snapshot(0))
    }

    fn terminal_output_lines(count: usize) -> String {
        (0..count)
            .map(|index| format!("line {index:03}\n"))
            .collect::<String>()
    }

    #[test]
    fn snapshot_row_mapping_anchors_viewport_rows_inside_retained_snapshot() {
        let mut screen = nyaterm_terminal::TerminalScreen::default();
        screen.advance_decoded_text(&terminal_output_lines(80));
        let base = screen.viewport_snapshot(0);
        let viewport_rows = base.row_count().max(1);
        let older = screen.viewport_snapshot(viewport_rows);
        let retained_older_rows = older.row_count().min(viewport_rows);
        assert!(retained_older_rows > 0);

        let mut snapshot = base.clone();
        let mut rows = older.rows()[..retained_older_rows].to_vec();
        rows.extend(snapshot.rows().iter().cloned());
        snapshot.row_data = rows.into();

        let first_visible_row = terminal_snapshot_row_for_viewport_row(
            &snapshot,
            0,
            viewport_rows,
            snapshot.scrollback_len,
            0,
        );
        let last_visible_row = terminal_snapshot_row_for_viewport_row(
            &snapshot,
            0,
            viewport_rows,
            snapshot.scrollback_len,
            viewport_rows.saturating_sub(1),
        );

        assert_eq!(first_visible_row, Some(retained_older_rows));
        assert_eq!(snapshot.line(first_visible_row.unwrap()), base.line(0),);
        assert_eq!(
            snapshot.line(last_visible_row.unwrap()),
            base.line(viewport_rows.saturating_sub(1)),
        );
    }
    #[test]
    fn gutter_metrics_use_same_widths_for_ms_timestamps_and_total_hit_area() {
        let metrics = terminal_gutter_metrics(8.0, true, 14, true, 5);

        assert_eq!(metrics.timestamp_width, 114.0);
        assert_eq!(metrics.line_number_width, 42.0);
        assert_eq!(metrics.gap_width, 8.0);
        assert_eq!(metrics.trailing_padding_width, 19.0);
        assert_eq!(metrics.total_width(), 183.0);
    }

    #[test]
    fn gutter_metrics_expand_with_large_terminal_font() {
        let metrics = terminal_gutter_metrics(18.0, true, 10, true, 5);

        assert!(metrics.timestamp_width > 120.0);
        assert!(metrics.line_number_width > 70.0);
        assert_eq!(
            metrics.total_width(),
            metrics.timestamp_width
                + metrics.line_number_width
                + metrics.gap_width
                + metrics.trailing_padding_width
        );
    }

    #[test]
    fn gutter_line_number_digits_follow_visible_absolute_end() {
        assert_eq!(terminal_line_number_digits_for_end(0), 1);
        assert_eq!(terminal_line_number_digits_for_end(9), 1);
        assert_eq!(terminal_line_number_digits_for_end(10), 2);
        assert_eq!(terminal_line_number_digits_for_end(99_999), 5);
        assert_eq!(terminal_line_number_digits_for_end(100_000), 6);
    }

    #[test]
    fn terminal_visual_hit_test_maps_through_snapshot_anchor() {
        let geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                Point {
                    x: px(10.0),
                    y: px(20.0),
                },
                Size {
                    width: px(400.0),
                    height: px(300.0),
                },
            ),
            snapshot: test_hit_test_snapshot(),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 4.0,
            padding_top: 4.0,
            gutter: 12.0,
            rows: 24,
            cols: 80,
            display_offset: 10,
            viewport_anchor_row: 5,
            snapshot_rows: 40,
            viewport_rows: 24,
            visual_y_offset: -80.0,
        };

        let cell = terminal_cell_for_visual_geometry(
            Point {
                x: px(10.0 + 4.0 + 12.0 + 3.0 * 8.0),
                y: px(20.0 + 4.0 + 10.0 * 16.0),
            },
            &geometry,
        );

        assert_eq!(cell, TerminalCellPos::new(10, 3));
    }

    #[test]
    fn terminal_visual_hit_test_maps_address_from_the_painted_grid_origin() {
        let geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                Point {
                    x: px(103.0),
                    y: px(40.0),
                },
                Size {
                    width: px(320.0),
                    height: px(96.0),
                },
            ),
            snapshot: test_hit_test_snapshot(),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 6,
            cols: 40,
            display_offset: 0,
            viewport_anchor_row: 0,
            snapshot_rows: 6,
            viewport_rows: 6,
            visual_y_offset: 0.0,
        };

        let address_start = terminal_cell_for_visual_geometry(
            Point {
                x: px(105.0),
                y: px(48.0),
            },
            &geometry,
        );
        let first_d = terminal_cell_for_visual_geometry(
            Point {
                x: px(113.0),
                y: px(48.0),
            },
            &geometry,
        );

        assert_eq!(address_start.col, 0);
        assert_eq!(first_d.col, 1);
    }

    #[test]
    fn terminal_visual_hit_test_follows_fractional_visual_offset() {
        let geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(400.0),
                    height: px(300.0),
                },
            ),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 24,
            cols: 80,
            snapshot: test_hit_test_snapshot(),
            display_offset: 3,
            viewport_anchor_row: 5,
            snapshot_rows: 40,
            viewport_rows: 24,
            visual_y_offset: -72.0,
        };

        let cell = terminal_cell_for_visual_geometry(
            Point {
                x: px(4.0 * 8.0),
                y: px(10.0 * 16.0),
            },
            &geometry,
        );

        assert_eq!(cell, TerminalCellPos::new(9, 4));
    }

    #[test]
    fn terminal_scrolled_hit_test_maps_viewport_row_to_absolute_buffer_line() {
        let mut screen = TerminalScreen::new(40, 6);
        screen.advance_decoded_text(&terminal_output_lines(180));
        let snapshot = Arc::new(screen.viewport_snapshot(100));
        let geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(400.0),
                    height: px(96.0),
                },
            ),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 6,
            cols: snapshot.cols,
            display_offset: 100,
            viewport_anchor_row: 0,
            snapshot_rows: snapshot.row_count(),
            viewport_rows: 6,
            visual_y_offset: 0.0,
            snapshot: snapshot.clone(),
        };
        let viewport_row = 3;
        let position = Point {
            x: px(4.0),
            y: px(viewport_row as f32 * geometry.cell_h + geometry.cell_h * 0.5),
        };

        let snapshot_row = terminal_snapshot_row_for_visual_geometry(position, &geometry);
        let absolute_line =
            terminal_absolute_line_for_snapshot_row(snapshot.as_ref(), snapshot_row).unwrap();
        let expected = snapshot
            .total_rows
            .saturating_sub(snapshot.display_offset)
            .saturating_sub(snapshot.row_count())
            + viewport_row;

        assert_eq!(snapshot_row, viewport_row);
        assert_eq!(absolute_line, expected);
    }

    #[test]
    fn terminal_hit_test_uses_painted_geometry_when_model_offset_advances() {
        let mut geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(400.0),
                    height: px(300.0),
                },
            ),
            snapshot: test_hit_test_snapshot(),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 24,
            cols: 80,
            display_offset: 100,
            viewport_anchor_row: 4,
            snapshot_rows: 40,
            viewport_rows: 24,
            visual_y_offset: -64.0,
        };
        let position = Point {
            x: px(20.0),
            y: px(40.0),
        };
        let painted_cell = terminal_cell_for_visual_geometry(position, &geometry);

        geometry.display_offset = 4;

        assert_eq!(
            terminal_cell_for_visual_geometry(position, &geometry),
            painted_cell
        );
    }

    #[test]
    fn terminal_visual_hit_test_clamps_pending_scroll_to_retained_rows() {
        let cell_h = 16.0;
        let viewport_anchor_row = 12;
        let visual_y_offset = terminal_hit_test_visual_y_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: true,
            target_offset: 40,
            displayed_offset: 0,
            residual_lines: 0.0,
            viewport_anchor_row,
            snapshot_rows: 40,
            viewport_rows: 20,
            cell_height: cell_h,
        });
        let geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(640.0),
                    height: px(320.0),
                },
            ),
            cell_w: 8.0,
            cell_h,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 20,
            cols: 80,
            snapshot: test_hit_test_snapshot(),
            display_offset: 0,
            viewport_anchor_row,
            snapshot_rows: 40,
            viewport_rows: 20,
            visual_y_offset,
        };
        let viewport_row = 3;
        let cell = terminal_cell_for_visual_geometry(
            Point {
                x: px(4.5 * geometry.cell_w),
                y: px(visual_y_offset
                    + (viewport_anchor_row + viewport_row) as f32 * cell_h
                    + cell_h * 0.5),
            },
            &geometry,
        );

        assert_eq!(visual_y_offset, 0.0);
        assert_eq!(cell, TerminalCellPos::new(viewport_row, 4));
    }

    #[test]
    fn terminal_visual_hit_test_is_stable_across_cell_metric_scaling() {
        for (cell_w, cell_h) in [(8.0, 16.0), (12.0, 24.0), (16.0, 32.0)] {
            let viewport_anchor_row = 5;
            let viewport_row = 7;
            let snapshot_row = viewport_anchor_row + viewport_row;
            let visual_y_offset = -3.25 * cell_h;
            let geometry = TerminalHitTestGeometry {
                bounds: gpui::bounds(
                    Point {
                        x: px(10.0),
                        y: px(20.0),
                    },
                    Size {
                        width: px(800.0),
                        height: px(600.0),
                    },
                ),
                snapshot: test_hit_test_snapshot(),
                cell_w,
                cell_h,
                padding_left: 0.0,
                padding_top: 0.0,
                gutter: 0.0,
                rows: 24,
                cols: 80,
                display_offset: 4,
                viewport_anchor_row,
                snapshot_rows: 40,
                viewport_rows: 24,
                visual_y_offset,
            };
            let cell = terminal_cell_for_visual_geometry(
                Point {
                    x: px(10.0 + 9.5 * cell_w),
                    y: px(20.0 + visual_y_offset + (snapshot_row as f32 + 0.5) * cell_h),
                },
                &geometry,
            );

            assert_eq!(cell, TerminalCellPos::new(viewport_row, 9));
        }
    }
}
