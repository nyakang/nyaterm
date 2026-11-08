use nyaterm_terminal_gpui::geometry::terminal_hit_test_visual_y_offset_px;
pub(in crate::features) use nyaterm_terminal_gpui::geometry::{
    TerminalHitTestGeometry, terminal_cell_for_visual_geometry, terminal_gutter_metrics,
    terminal_line_number_digits, terminal_snapshot_row_for_visual_geometry,
};
use std::collections::HashSet;

use gpui::{App, Bounds, Context, FontWeight, Pixels, Point, px};

use nyaterm_core::TerminalViewportInsets;

use crate::features::NyaTermApp;
use crate::features::formatting::terminal_timestamp_format_width_chars;
use crate::features::shell::configured_appearance_font_stack;
use crate::features::terminal::state::TerminalFontMetricsCache;
use crate::features::terminal::terminal_surface_entity::{
    TerminalVisualScrollGeometry, terminal_snapshot_anchor_row_for_display_offset,
};
use crate::models::TerminalCellPos;

use super::{CELL_WIDTH_RATIO, LINE_HEIGHT_RATIO};
use crate::features::terminal::{
    ResolvedAppearanceFont, TerminalFontMeasurement, is_generic_terminal_font_family,
};
use crate::features::{FontResolutionSource, FontResolutionStatus, normalize_font_family};

pub(in crate::features) use nyaterm_terminal_gpui::fonts::{
    measure_terminal_font, terminal_font_with_features,
};

fn effective_terminal_font_descriptor(
    descriptor: &ResolvedAppearanceFont,
    measurement: &TerminalFontMeasurement,
) -> ResolvedAppearanceFont {
    if is_generic_terminal_font_family(&descriptor.family)
        && !descriptor
            .family
            .eq_ignore_ascii_case(measurement.resolved_family.as_str())
    {
        descriptor.with_primary_family(&measurement.resolved_family)
    } else {
        descriptor.clone()
    }
}

struct TerminalFontResolution {
    font: gpui::Font,
    cell_width: f32,
    runtime_override: Option<ResolvedAppearanceFont>,
    configured_font_valid: bool,
    resolution: FontResolutionStatus,
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

    /// Refreshes fixed-width cell metrics for the configured terminal font through GPUI TextSystem.
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
                    && cache.catalog_generation == self.settings.font_catalog_generation()
            })
            .cloned();
        let (terminal_font, measured_w, override_font, resolution) = if let Some(cache) = cached {
            let descriptor = cache
                .resolved_font
                .clone()
                .unwrap_or_else(|| configured_font.clone());
            let font = terminal_font_with_features(&descriptor, weight);
            (
                font,
                cache.cell_width,
                cache.resolved_font,
                cache.resolution,
            )
        } else {
            let result =
                self.resolve_terminal_font_metrics(text_system, configured_font, size, weight);
            self.terminal.layout.font_metrics_cache = Some(TerminalFontMetricsCache {
                configured_family: settings.terminal_font_family.clone(),
                font_size: settings.terminal_font_size,
                font_weight: settings.terminal_font_weight,
                catalog_generation: self.settings.font_catalog_generation(),
                configured_font_valid: result.configured_font_valid,
                resolved_font: result.runtime_override.clone(),
                resolution: result.resolution.clone(),
                cell_width: result.cell_width,
            });
            (
                result.font,
                result.cell_width,
                result.runtime_override,
                result.resolution,
            )
        };
        self.terminal.set_terminal_font_override(override_font);
        self.terminal.set_terminal_font_resolution(resolution);
        let font_id = text_system.resolve_font(&terminal_font);
        let ascent = f32::from(text_system.ascent(font_id, size));
        let descent = f32::from(text_system.descent(font_id, size)).abs();
        let font_line = (ascent + descent).max(font_size + 2.);
        // Preserve the painter's convention: the default 14px font uses an 18px line height.
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
    ) -> TerminalFontResolution {
        let user_stack = configured_appearance_font_stack(
            &self.settings.summary().terminal_font_family,
            "JetBrains Mono",
        );
        let configured_failure =
            match measure_terminal_font(text_system, &configured_font, size, weight) {
                Ok(measurement) => {
                    let effective =
                        effective_terminal_font_descriptor(&configured_font, &measurement);
                    let runtime_override = (!effective
                        .family
                        .eq_ignore_ascii_case(configured_font.family.as_str()))
                    .then_some(effective.clone());
                    return TerminalFontResolution {
                        font: terminal_font_with_features(&effective, weight),
                        cell_width: measurement.cell_width,
                        runtime_override,
                        configured_font_valid: true,
                        resolution: FontResolutionStatus {
                            configured_family: configured_font.family.clone(),
                            effective_family: measurement.resolved_family,
                            source: if user_stack.is_empty() {
                                FontResolutionSource::PlatformDefault
                            } else {
                                FontResolutionSource::Configured
                            },
                            reason: None,
                        },
                    };
                }
                Err(failure) => failure,
            };

        let configured_family = configured_font.family.clone();
        tracing::debug!(
            configured_font = %configured_family,
            reason = ?configured_failure,
            "configured terminal font failed fixed-width validation"
        );

        let mut candidates = Vec::<ResolvedAppearanceFont>::new();
        let mut seen_families = HashSet::new();
        let mut add_candidate = |candidate: ResolvedAppearanceFont| {
            let family = candidate.family.trim().to_ascii_lowercase();
            if !family.is_empty() && seen_families.insert(family) {
                candidates.push(candidate);
            }
        };
        // Preserve the user's fallback order before trying platform defaults.
        for family in &configured_font.fallback_families {
            add_candidate(configured_font.with_primary_family(family));
        }
        // The catalog is an option source, not an implicit fallback chain. Only the
        // configured stack may be used before the platform fallback ladder below;
        // otherwise a missing primary could silently select an unrelated font just
        // because it sorts first in the system catalog.
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
        for candidate in candidates {
            if let Ok(measurement) = measure_terminal_font(text_system, &candidate, size, weight) {
                let effective = effective_terminal_font_descriptor(&candidate, &measurement);
                let font = terminal_font_with_features(&effective, weight);
                let source = user_stack
                    .iter()
                    .position(|family| {
                        normalize_font_family(family) == normalize_font_family(&candidate.family)
                    })
                    .map(|index| {
                        if index == 0 {
                            FontResolutionSource::Configured
                        } else {
                            FontResolutionSource::UserFallback(index)
                        }
                    })
                    .unwrap_or(FontResolutionSource::PlatformDefault);
                tracing::debug!(
                    configured_font = %configured_family,
                    fallback_font = %effective.family,
                    reason = ?configured_failure,
                    "configured terminal font failed fixed-width validation; using a local fallback"
                );
                return TerminalFontResolution {
                    font,
                    cell_width: measurement.cell_width,
                    runtime_override: Some(effective),
                    configured_font_valid: false,
                    resolution: FontResolutionStatus {
                        configured_family: configured_family.clone(),
                        effective_family: measurement.resolved_family,
                        source,
                        reason: Some(configured_failure.into()),
                    },
                };
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
        tracing::warn!(
            configured_font = %configured_family,
            reason = ?configured_failure,
            "configured terminal font could not be validated; using a metrics fallback"
        );
        TerminalFontResolution {
            font,
            cell_width: width,
            runtime_override: None,
            configured_font_valid: false,
            resolution: FontResolutionStatus {
                configured_family,
                effective_family: configured_font.family,
                source: FontResolutionSource::EmergencyMetricsFallback,
                reason: Some(configured_failure.into()),
            },
        }
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
            self.terminal_line_number_digits_for_session(session_id, snapshot.as_ref()),
        )
        .total_width()
    }

    pub(in crate::features) fn terminal_wrap_marker_width_px(&self) -> f32 {
        if self.settings.summary().terminal_show_wrap_markers {
            self.terminal_cell_size().0
        } else {
            0.0
        }
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
                unwrapped: session_id
                    .and_then(|id| self.terminal.view.surfaces.get(id))
                    .and_then(|surface| surface.read(cx).painted_unwrapped_layout()),
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
        }) - snapshot.leading_cleared_rows(fallback_viewport_anchor_row)
            as f32
            * fallback_cell_h;
        let gutter = terminal_gutter_metrics(
            fallback_cell_w,
            self.settings.summary().terminal_show_timestamps,
            terminal_timestamp_format_width_chars(
                &self.settings.summary().terminal_timestamp_format,
            ),
            self.settings.summary().terminal_show_line_numbers,
            self.terminal_line_number_digits_for_session(session_id, snapshot.as_ref()),
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
            unwrapped: None,
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

impl NyaTermApp {
    pub(in crate::features) fn terminal_line_number_digits_for_session(
        &self,
        session_id: Option<&str>,
        snapshot: &nyaterm_terminal::TerminalSnapshot,
    ) -> usize {
        let current = terminal_line_number_digits(snapshot);
        session_id
            .or_else(|| self.session.active_id())
            .and_then(|id| self.terminal.view.views.get(id))
            .map_or(current, |view| current.max(view.max_line_number_digits))
    }
}

#[cfg(test)]
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
    use crate::features::terminal::{ResolvedAppearanceFont, TerminalFontMeasurement};
    use crate::models::TerminalCellPos;

    use super::{
        TerminalHitTestGeometry, TerminalVisualScrollGeometry, effective_terminal_font_descriptor,
        is_generic_terminal_font_family, terminal_cell_for_visual_geometry,
        terminal_hit_test_visual_y_offset_px, terminal_snapshot_row_for_viewport_row,
        terminal_snapshot_row_for_visual_geometry,
    };

    #[test]
    fn generic_terminal_family_is_promoted_to_the_measured_family() {
        assert!(is_generic_terminal_font_family("system-monospace"));
        assert!(is_generic_terminal_font_family(" UI-MONOSPACE "));
        assert!(!is_generic_terminal_font_family("JetBrains Mono"));

        let descriptor = ResolvedAppearanceFont {
            family: "monospace".to_string(),
            fallbacks: None,
            fallback_families: vec!["Menlo".to_string(), "Monaco".to_string()],
        };
        let measurement = TerminalFontMeasurement {
            cell_width: 8.0,
            resolved_family: "Menlo".to_string(),
        };

        let promoted = effective_terminal_font_descriptor(&descriptor, &measurement);
        assert_eq!(promoted.family, "Menlo");
        assert_eq!(promoted.fallback_families, vec!["Monaco".to_string()]);
    }

    #[test]
    fn named_terminal_family_is_not_silently_replaced() {
        let descriptor = ResolvedAppearanceFont {
            family: "JetBrains Mono".to_string(),
            fallbacks: None,
            fallback_families: vec!["Menlo".to_string()],
        };
        let measurement = TerminalFontMeasurement {
            cell_width: 8.0,
            resolved_family: "Menlo".to_string(),
        };

        assert_eq!(
            effective_terminal_font_descriptor(&descriptor, &measurement),
            descriptor
        );
    }

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
    fn gutter_line_number_digits_match_actual_max_display_line_number() {
        let mut screen = nyaterm_terminal::TerminalScreen::new(80, 24);
        screen.set_scrollback_limit(500_000);
        for _ in 0..20_000 {
            screen.advance(b"x\r\n");
        }
        // Live viewport snapshot at offset 0
        let snapshot = screen.snapshot();
        let max_ln = snapshot
            .rows()
            .iter()
            .filter_map(|r| r.display_line_number)
            .max()
            .unwrap_or(1);
        let digits = super::terminal_line_number_digits(&snapshot);
        let actual_digits = max_ln.to_string().len();
        eprintln!(
            "offset=0: max_ln={}, total_rows={}, digits={}, actual_digits={}",
            max_ln, snapshot.total_rows, digits, actual_digits
        );
        assert!(
            digits >= actual_digits,
            "offset=0: computed digits {} should be >= actual {}",
            digits,
            actual_digits
        );

        // Now test a snapshot scrolled back into history.
        let offset = 2_000;
        let scrolled = screen.viewport_snapshot_with_window(offset, 32, 32);
        let scrolled_max = scrolled
            .rows()
            .iter()
            .filter_map(|r| r.display_line_number)
            .max()
            .unwrap_or(1);
        let scrolled_min = scrolled
            .rows()
            .iter()
            .filter_map(|r| r.display_line_number)
            .min()
            .unwrap_or(1);
        let scrolled_digits = super::terminal_line_number_digits(&scrolled);
        let scrolled_actual_digits = scrolled_max.to_string().len();
        eprintln!(
            "offset={}: display_range=[{}, {}], total_rows={}, display_offset_in_snap={}, digits={}, actual_digits={}",
            offset,
            scrolled_min,
            scrolled_max,
            scrolled.total_rows,
            scrolled.display_offset,
            scrolled_digits,
            scrolled_actual_digits
        );
        assert!(
            scrolled_digits >= scrolled_actual_digits,
            "offset={}: computed digits {} should be >= actual {}",
            offset,
            scrolled_digits,
            scrolled_actual_digits
        );

        // Test after clear_except_input - line numbers should keep their origin
        screen.clear_except_input();
        for _ in 0..10 {
            screen.advance(b"x\r\n");
        }
        let after_clear = screen.snapshot();
        let clear_max = after_clear
            .rows()
            .iter()
            .filter_map(|r| r.display_line_number)
            .max()
            .unwrap_or(1);
        let clear_min = after_clear
            .rows()
            .iter()
            .filter_map(|r| r.display_line_number)
            .min()
            .unwrap_or(1);
        let clear_digits = super::terminal_line_number_digits(&after_clear);
        let clear_actual_digits = clear_max.to_string().len();
        eprintln!(
            "after clear: display_range=[{}, {}], total_rows={}, digits={}, actual_digits={}",
            clear_min, clear_max, after_clear.total_rows, clear_digits, clear_actual_digits
        );
        assert!(
            clear_digits >= clear_actual_digits,
            "after clear: computed digits {} should be >= actual {}",
            clear_digits,
            clear_actual_digits
        );

        // Reproduce the scenario from logical_line_numbers_keep_local_clear_origin_when_wrapped_history_is_pruned:
        // display_line_number can exceed total_rows after wrap + scrollback prune + clear
        let mut small = nyaterm_terminal::TerminalScreen::new(4, 2);
        small.set_scrollback_limit(2);
        small.clear_except_input();
        for _ in 0..10 {
            small.advance(b"abcdefghijkl\r\n");
        }
        let snap = small.snapshot();
        let ln = snap.row(1).and_then(|r| r.display_line_number).unwrap_or(0);
        let snap_digits = super::terminal_line_number_digits(&snap);
        let snap_actual = ln.to_string().len();
        eprintln!(
            "wrap+prune: row[1].display_line_number={}, total_rows={}, digits={}, actual_digits={}",
            ln, snap.total_rows, snap_digits, snap_actual
        );
        assert!(
            snap_digits >= snap_actual,
            "wrap+prune: computed digits {} should be >= actual {}",
            snap_digits,
            snap_actual
        );
    }

    #[test]
    fn clear_all_hit_testing_maps_the_top_prompt_to_its_host_row() {
        let mut terminal = TerminalScreen::new(40, 12);
        terminal.advance(b"\x1b[12;1Hroot# draft");
        terminal.clear_except_input();
        let snapshot = std::sync::Arc::new(terminal.snapshot());
        let geometry = TerminalHitTestGeometry {
            unwrapped: None,
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(320.0),
                    height: px(192.0),
                },
            ),
            visual_y_offset: -(snapshot.leading_cleared_rows(0) as f32) * 16.0,
            snapshot,
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 12,
            cols: 40,
            display_offset: 0,
            viewport_anchor_row: 0,
            snapshot_rows: 12,
            viewport_rows: 12,
        };
        let position = Point {
            x: px(28.0),
            y: px(8.0),
        };
        assert_eq!(
            terminal_cell_for_visual_geometry(position, &geometry),
            TerminalCellPos::new(11, 3)
        );
        assert_eq!(
            terminal_snapshot_row_for_visual_geometry(position, &geometry),
            11
        );
    }

    #[test]
    fn terminal_visual_hit_test_maps_through_snapshot_anchor() {
        let geometry = TerminalHitTestGeometry {
            unwrapped: None,
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
            unwrapped: None,
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
            unwrapped: None,
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
            unwrapped: None,
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
            unwrapped: None,
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
            unwrapped: None,
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
                unwrapped: None,
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
