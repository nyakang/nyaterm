use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, Font, FontFallbacks, FontFeatures,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, ShapedLine,
    SharedString, Style, TextRun, Window, fill, font, point, px, relative, rgb, rgba, size,
};
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_terminal::{
    ShellInputLineKind, TerminalLineId, TerminalSnapshot, terminal_cell_count,
    terminal_char_cell_width, terminal_is_zero_width_mark,
};

use crate::keywords::{
    CompiledKeywordRule, CompiledKeywordRules, TerminalKeywordHighlightSnapshot,
    compile_keyword_rules, terminal_keyword_rules_key,
};
use crate::paint::{
    apply_search_ranges, flush_bg, line_strike_color, push_col_range_bg, terminal_cell_text_at_col,
    terminal_highlight_spans_compiled, terminal_highlight_spans_with_keyword_ranges,
    terminal_keyword_exclusion_ranges, terminal_run_font,
};
use crate::types::{TerminalHighlightSpan, TerminalPaintGeometry};

#[derive(Debug, Clone)]
pub struct TerminalBufferMatch {
    pub line_index: usize,
    /// Half-open character column range on the matched line.
    pub start_col: usize,
    pub end_col: usize,
}

#[derive(Debug, Clone)]
pub struct TerminalSearchFlags {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TerminalLineDecorations {
    pub selected_occurrence_ranges: Vec<(usize, usize)>,
    pub search_ranges: Vec<(usize, usize)>,
    pub active_search_ranges: Vec<(usize, usize)>,
    pub link_ranges: Vec<(usize, usize)>,
}

/// Dynamic absolute-buffer selection. Endpoints are inclusive terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalGridSelection {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub head_line: usize,
    pub head_col: usize,
    pub all_buffer: bool,
}

impl TerminalGridSelection {
    pub fn new(
        anchor_line: usize,
        anchor_col: usize,
        head_line: usize,
        head_col: usize,
        all_buffer: bool,
    ) -> Self {
        Self {
            anchor_line,
            anchor_col,
            head_line,
            head_col,
            all_buffer,
        }
    }

    fn cols_for_absolute_line(self, line: usize) -> Option<(usize, usize)> {
        if self.all_buffer {
            return Some((0, usize::MAX));
        }
        if (self.anchor_line, self.anchor_col) == (self.head_line, self.head_col) {
            return None;
        }
        let (start_line, start_col, end_line, end_col) =
            if (self.anchor_line, self.anchor_col) <= (self.head_line, self.head_col) {
                (
                    self.anchor_line,
                    self.anchor_col,
                    self.head_line,
                    self.head_col,
                )
            } else {
                (
                    self.head_line,
                    self.head_col,
                    self.anchor_line,
                    self.anchor_col,
                )
            };
        if line < start_line || line > end_line {
            return None;
        }
        if start_line == end_line {
            return Some((start_col, end_col.saturating_add(1)));
        }
        if line == start_line {
            return Some((start_col, usize::MAX));
        }
        if line == end_line {
            return Some((0, end_col.saturating_add(1)));
        }
        Some((0, usize::MAX))
    }
}

#[derive(Debug, Clone)]
struct CachedTerminalPaintRow {
    line: Arc<ShapedLine>,
    background_ranges: Vec<TerminalRowBackgroundRange>,
    underline_ranges: Vec<TerminalRowUnderlineRange>,
    text_run_count: usize,
}

#[derive(Debug, Clone)]
struct TerminalRowBackgroundRange {
    bg: u32,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
struct TerminalRowUnderlineRange {
    color: u32,
    start: usize,
    end: usize,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TerminalKeywordLayoutState {
    rules_key: u64,
    spans_present: bool,
    result_known_empty: bool,
}

#[derive(Debug, Default)]
pub struct NyaTerminalLayoutCache {
    rows: HashMap<u64, Arc<CachedTerminalPaintRow>>,
    row_order: VecDeque<u64>,
    cursor_glyphs: HashMap<u64, Arc<ShapedLine>>,
    cursor_glyph_order: VecDeque<u64>,
    keyword_rules_source: Option<Arc<Vec<ResolvedKeywordHighlightRule>>>,
    keyword_rules_key: u64,
    compiled_keyword_key: Option<u64>,
    compiled_keyword_rules: Arc<CompiledKeywordRules>,
    pub hits: u64,
    pub misses: u64,
    pub shape_calls: u64,
    pub shape_duration_us: u64,
}

const TERMINAL_LAYOUT_CACHE_ROW_CAP: usize = 4096;
const TERMINAL_LAYOUT_CACHE_CURSOR_GLYPH_CAP: usize = 256;
const TERMINAL_ELEMENT_PREPAINT_SLOW_MS: u128 = 12;
const TERMINAL_ELEMENT_PAINT_SLOW_MS: u128 = 12;

impl NyaTerminalLayoutCache {
    pub fn clear(&mut self) {
        self.rows.clear();
        self.row_order.clear();
        self.cursor_glyphs.clear();
        self.cursor_glyph_order.clear();
        self.keyword_rules_source = None;
        self.keyword_rules_key = 0;
        self.compiled_keyword_key = None;
        self.compiled_keyword_rules = Arc::default();
        self.hits = 0;
        self.misses = 0;
        self.shape_calls = 0;
        self.shape_duration_us = 0;
    }

    fn keyword_rules_key(&mut self, rules: &Arc<Vec<ResolvedKeywordHighlightRule>>) -> u64 {
        if let Some(cached) = self.keyword_rules_source.as_ref() {
            if Arc::ptr_eq(cached, rules) {
                return self.keyword_rules_key;
            }
            if cached.as_ref() == rules.as_ref() {
                self.keyword_rules_source = Some(Arc::clone(rules));
                return self.keyword_rules_key;
            }
        }
        self.keyword_rules_key = terminal_keyword_rules_key(rules);
        self.keyword_rules_source = Some(Arc::clone(rules));
        self.keyword_rules_key
    }

    fn compiled_keyword_rules(
        &mut self,
        key: u64,
        rules: &[ResolvedKeywordHighlightRule],
    ) -> Arc<CompiledKeywordRules> {
        if self.compiled_keyword_key == Some(key) {
            return Arc::clone(&self.compiled_keyword_rules);
        }
        self.compiled_keyword_key = Some(key);
        self.compiled_keyword_rules = Arc::new(compile_keyword_rules(rules));
        Arc::clone(&self.compiled_keyword_rules)
    }

    #[cfg(test)]
    fn shaped_line(
        &mut self,
        _row: usize,
        key: u64,
        shape: impl FnOnce() -> (Arc<ShapedLine>, std::time::Duration),
    ) -> (Arc<ShapedLine>, bool, std::time::Duration) {
        let (row, did_shape, duration) = self.paint_row(_row, key, || {
            let (line, duration) = shape();
            (line, duration, 0, Vec::new(), Vec::new())
        });
        (Arc::clone(&row.line), did_shape, duration)
    }

    #[cfg(test)]
    fn paint_row(
        &mut self,
        _row: usize,
        key: u64,
        build: impl FnOnce() -> (
            Arc<ShapedLine>,
            std::time::Duration,
            usize,
            Vec<TerminalRowBackgroundRange>,
            Vec<TerminalRowUnderlineRange>,
        ),
    ) -> (Arc<CachedTerminalPaintRow>, bool, std::time::Duration) {
        self.paint_row_reusing(_row, key, None, build)
    }

    /// `reuse_key` must describe paint output equivalent to `key`.
    fn paint_row_reusing(
        &mut self,
        _row: usize,
        key: u64,
        reuse_key: Option<u64>,
        build: impl FnOnce() -> (
            Arc<ShapedLine>,
            std::time::Duration,
            usize,
            Vec<TerminalRowBackgroundRange>,
            Vec<TerminalRowUnderlineRange>,
        ),
    ) -> (Arc<CachedTerminalPaintRow>, bool, std::time::Duration) {
        if let Some(cached) = self.rows.get(&key) {
            self.hits = self.hits.saturating_add(1);
            return (Arc::clone(cached), false, std::time::Duration::ZERO);
        }
        if let Some(reuse_key) = reuse_key.filter(|reuse_key| *reuse_key != key)
            && let Some(cached) = self.rows.remove(&reuse_key)
        {
            self.hits = self.hits.saturating_add(1);
            self.rows.insert(key, Arc::clone(&cached));
            self.row_order.push_back(key);
            return (cached, false, std::time::Duration::ZERO);
        }
        self.misses = self.misses.saturating_add(1);
        if self.rows.len() >= TERMINAL_LAYOUT_CACHE_ROW_CAP {
            self.evict_oldest_row();
        }
        let (line, duration, text_run_count, background_ranges, underline_ranges) = build();
        self.shape_calls = self.shape_calls.saturating_add(1);
        self.shape_duration_us = self
            .shape_duration_us
            .saturating_add(duration.as_micros().min(u128::from(u64::MAX)) as u64);
        let row = Arc::new(CachedTerminalPaintRow {
            line: Arc::clone(&line),
            background_ranges,
            underline_ranges,
            text_run_count,
        });
        self.rows.insert(key, Arc::clone(&row));
        self.row_order.push_back(key);
        (row, true, duration)
    }

    fn contains_paint_row(&self, key: u64, reuse_key: Option<u64>) -> bool {
        self.rows.contains_key(&key)
            || reuse_key
                .filter(|reuse_key| *reuse_key != key)
                .is_some_and(|reuse_key| self.rows.contains_key(&reuse_key))
    }

    fn cursor_glyph(
        &mut self,
        key: u64,
        shape: impl FnOnce() -> (Arc<ShapedLine>, std::time::Duration),
    ) -> (Arc<ShapedLine>, bool, std::time::Duration) {
        if let Some(cached) = self.cursor_glyphs.get(&key) {
            self.hits = self.hits.saturating_add(1);
            return (Arc::clone(cached), false, std::time::Duration::ZERO);
        }
        self.misses = self.misses.saturating_add(1);
        if self.cursor_glyphs.len() >= TERMINAL_LAYOUT_CACHE_CURSOR_GLYPH_CAP
            && let Some(oldest) = self.cursor_glyph_order.pop_front()
        {
            self.cursor_glyphs.remove(&oldest);
        }
        let (line, duration) = shape();
        self.shape_calls = self.shape_calls.saturating_add(1);
        self.shape_duration_us = self
            .shape_duration_us
            .saturating_add(duration.as_micros().min(u128::from(u64::MAX)) as u64);
        self.cursor_glyphs.insert(key, Arc::clone(&line));
        self.cursor_glyph_order.push_back(key);
        (line, true, duration)
    }

    fn evict_oldest_row(&mut self) {
        while self.rows.len() >= TERMINAL_LAYOUT_CACHE_ROW_CAP {
            let Some(key) = self.row_order.pop_front() else {
                self.rows.clear();
                return;
            };
            if self.rows.remove(&key).is_some() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod layout_cache_tests;

pub struct NyaTerminalElement {
    snapshot: Arc<TerminalSnapshot>,
    keyword_rules: Arc<Vec<ResolvedKeywordHighlightRule>>,
    keyword_highlights: Option<Arc<TerminalKeywordHighlightSnapshot>>,
    decorations: Arc<[TerminalLineDecorations]>,
    selection: Option<TerminalGridSelection>,
    layout_cache: Option<Arc<Mutex<NyaTerminalLayoutCache>>>,
    show_cursor: bool,
    cursor_style: String,
    cell_width: f32,
    cell_height: f32,
    palette: nyaterm_ui::ThemePalette,
    font_family: String,
    font_fallbacks: Option<FontFallbacks>,
    font_size: f32,
    normal_weight: f32,
    bold_weight: f32,
    visual_y_offset: f32,
    layout_rows: Option<usize>,
    fill_height: bool,
    zebra_stripes_enabled: bool,
    target_line: Option<TerminalLineId>,
}

struct TerminalPaintRow {
    y: Pixels,
    line: Arc<ShapedLine>,
}

pub struct TerminalImagePaint {
    bounds: Bounds<Pixels>,
    image: std::sync::Arc<gpui::RenderImage>,
}

pub struct TerminalCursorGlyphPaint {
    origin: gpui::Point<Pixels>,
    line: Arc<ShapedLine>,
}

#[derive(Default)]
pub struct NyaTerminalPaintPlan {
    /// Shell-input and click-target row washes, below all terminal content.
    zebra_stripes: Vec<PaintQuad>,
    /// Explicit terminal cell backgrounds (under protocol images).
    backgrounds: Vec<PaintQuad>,
    /// Decoded graphics protocol images painted under terminal text.
    images_under: Vec<TerminalImagePaint>,
    /// Accent placeholders for undecodable under-text images.
    placeholders_under: Vec<PaintQuad>,
    /// Search match + selection washes (over under-text images, under glyphs).
    decoration_backgrounds: Vec<PaintQuad>,
    /// Active-search gutter marks (under glyphs).
    active_markers: Vec<PaintQuad>,
    rows: Vec<TerminalPaintRow>,
    /// Terminal underline decorations painted with current scroll geometry.
    underlines: Vec<PaintQuad>,
    /// Decoded graphics with Kitty z>0, painted above terminal text.
    images_above: Vec<TerminalImagePaint>,
    /// Accent placeholders for undecodable above-text images.
    placeholders_above: Vec<PaintQuad>,
    cursor_background: Option<PaintQuad>,
    cursor_glyph: Option<TerminalCursorGlyphPaint>,
    shape_line_count: usize,
    shape_line_duration: std::time::Duration,
    prefetched_row_count: usize,
    text_run_count: usize,
}

impl NyaTerminalElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        snapshot: Arc<TerminalSnapshot>,
        keyword_rules: Arc<Vec<ResolvedKeywordHighlightRule>>,
        decorations: impl Into<Arc<[TerminalLineDecorations]>>,
        show_cursor: bool,
        cursor_style: impl Into<String>,
        cell_width: f32,
        cell_height: f32,
        palette: nyaterm_ui::ThemePalette,
        font_family: String,
        font_size: f32,
        normal_weight: f32,
        bold_weight: f32,
    ) -> Self {
        Self {
            snapshot,
            keyword_rules,
            keyword_highlights: None,
            decorations: decorations.into(),
            selection: None,
            layout_cache: None,
            show_cursor,
            cursor_style: cursor_style.into(),
            cell_width,
            cell_height,
            palette,
            font_family,
            font_fallbacks: None,
            font_size,
            normal_weight,
            bold_weight,
            visual_y_offset: 0.0,
            layout_rows: None,
            fill_height: false,
            zebra_stripes_enabled: false,
            target_line: None,
        }
    }

    pub fn with_layout_cache(mut self, cache: Arc<Mutex<NyaTerminalLayoutCache>>) -> Self {
        self.layout_cache = Some(cache);
        self
    }

    pub fn with_selection(mut self, selection: Option<TerminalGridSelection>) -> Self {
        self.selection = selection;
        self
    }

    pub fn with_font_fallbacks(mut self, fallbacks: Option<FontFallbacks>) -> Self {
        self.font_fallbacks = fallbacks;
        self
    }

    pub fn with_keyword_highlights(
        mut self,
        highlights: Arc<TerminalKeywordHighlightSnapshot>,
    ) -> Self {
        self.keyword_highlights = Some(highlights);
        self
    }

    pub fn with_visual_y_offset(mut self, offset: f32) -> Self {
        self.visual_y_offset = offset;
        self
    }

    pub fn with_zebra_stripes(
        mut self,
        enabled: bool,
        target_line: Option<TerminalLineId>,
    ) -> Self {
        self.zebra_stripes_enabled = enabled;
        self.target_line = target_line;
        self
    }

    pub fn with_layout_rows(mut self, rows: usize) -> Self {
        self.layout_rows = Some(rows.max(1));
        self
    }

    /// Let the parent viewport own the element height, like an editor viewport.
    /// The snapshot row count still limits which rows are painted.
    pub fn with_fill_height(mut self, fill: bool) -> Self {
        self.fill_height = fill;
        self
    }

    #[cfg(test)]
    fn row_layout_key(
        &self,
        row: usize,
        display_line: &str,
        ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
        decorations: &TerminalLineDecorations,
    ) -> u64 {
        self.row_layout_key_with_keyword_key(
            row,
            display_line,
            ansi_spans,
            decorations,
            self.keyword_rules_key(),
            false,
        )
    }

    #[cfg(test)]
    fn row_layout_key_with_keyword_key(
        &self,
        row: usize,
        display_line: &str,
        ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
        decorations: &TerminalLineDecorations,
        keyword_rules_key: u64,
        keyword_spans_present: bool,
    ) -> u64 {
        self.row_layout_key_with_keyword_state(
            row,
            display_line,
            ansi_spans,
            decorations,
            TerminalKeywordLayoutState {
                rules_key: keyword_rules_key,
                spans_present: keyword_spans_present,
                result_known_empty: false,
            },
        )
    }

    #[cfg(test)]
    fn row_layout_key_with_keyword_state(
        &self,
        row: usize,
        display_line: &str,
        ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
        decorations: &TerminalLineDecorations,
        keyword_state: TerminalKeywordLayoutState,
    ) -> u64 {
        let effective_keyword_rules_key = terminal_effective_keyword_rules_key(
            keyword_state.rules_key,
            keyword_state.result_known_empty,
        );
        let paint_style_key = self.paint_style_key(effective_keyword_rules_key);
        terminal_row_layout_key(
            self.snapshot.row(row).map(|row| row.revision),
            display_line,
            ansi_spans,
            decorations,
            &[],
            keyword_state.spans_present,
            paint_style_key,
        )
    }

    fn paint_style_key(&self, keyword_rules_key: u64) -> u64 {
        let mut hasher = DefaultHasher::new();
        keyword_rules_key.hash(&mut hasher);
        self.palette.bg.hash(&mut hasher);
        self.palette.accent.hash(&mut hasher);
        self.palette.warning.hash(&mut hasher);
        self.palette.terminal_fg.hash(&mut hasher);
        self.palette.terminal_bg.hash(&mut hasher);
        self.palette.terminal_ansi.hash(&mut hasher);
        self.font_family.hash(&mut hasher);
        self.font_fallbacks.hash(&mut hasher);
        self.font_size.to_bits().hash(&mut hasher);
        self.normal_weight.to_bits().hash(&mut hasher);
        self.bold_weight.to_bits().hash(&mut hasher);
        self.cell_width.max(1.0).to_bits().hash(&mut hasher);
        hasher.finish()
    }

    fn cursor_glyph_layout_key(&self, text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        "terminal-cursor-glyph".hash(&mut hasher);
        text.hash(&mut hasher);
        self.paint_style_key(0).hash(&mut hasher);
        hasher.finish()
    }

    fn keyword_rules_key(&self) -> u64 {
        terminal_keyword_rules_key(&self.keyword_rules)
    }

    fn row_layout_cache_keys(
        &self,
        row: usize,
        keyword_paint_style_key: u64,
        empty_keyword_paint_style_key: u64,
    ) -> (u64, Option<u64>) {
        let snapshot_row = self.snapshot.row(row);
        let line = snapshot_row.map(|row| row.text.as_str()).unwrap_or("");
        let display_line = if line.is_empty() { " " } else { line };
        let ansi = snapshot_row.map(|row| row.styled_spans.as_ref());
        let row_revision = snapshot_row.map(|row| row.revision);
        let keyword_lookup = self.keyword_highlights.as_ref().and_then(|highlights| {
            highlights
                .lookup(row, self.snapshot.as_ref())
                .or_else(|| highlights.stale_lookup(row, self.snapshot.as_ref()))
        });
        let keyword_result_known_empty = keyword_lookup
            .as_ref()
            .is_some_and(|lookup| lookup.is_known_empty());
        let keyword_spans_present = keyword_lookup
            .as_ref()
            .and_then(|lookup| lookup.ranges())
            .is_some();
        let paint_style_key = if keyword_result_known_empty {
            empty_keyword_paint_style_key
        } else {
            keyword_paint_style_key
        };
        let default_decorations;
        let decorations = if let Some(decorations) = self.decorations.get(row) {
            decorations
        } else {
            default_decorations = TerminalLineDecorations::default();
            &default_decorations
        };
        let keyword_excluded_ranges =
            terminal_keyword_exclusion_ranges(snapshot_row, &decorations.link_ranges);
        let keyword_exclusions_affect_glyphs = !keyword_result_known_empty
            && (!self.keyword_rules.is_empty() || keyword_spans_present)
            && !keyword_excluded_ranges.is_empty();
        let row_layout_key = |paint_style_key: u64, keyword_spans_present: bool| {
            let keyword_excluded_ranges = if keyword_exclusions_affect_glyphs {
                keyword_excluded_ranges.as_slice()
            } else {
                &[]
            };
            terminal_row_layout_key(
                row_revision,
                display_line,
                ansi,
                decorations,
                keyword_excluded_ranges,
                keyword_spans_present,
                paint_style_key,
            )
        };
        let row_key = row_layout_key(paint_style_key, keyword_spans_present);
        let pending_keyword_row_is_equivalent = keyword_lookup.is_some()
            && (keyword_result_known_empty || !self.keyword_rules.is_empty());
        let pending_keyword_row_key = pending_keyword_row_is_equivalent
            .then(|| row_layout_key(keyword_paint_style_key, false))
            .filter(|pending_key| *pending_key != row_key);
        (row_key, pending_keyword_row_key)
    }
}

fn terminal_layout_prefetch_row(
    visible_rows: std::ops::Range<usize>,
    total_rows: usize,
    mut row_is_cached: impl FnMut(usize) -> bool,
) -> Option<usize> {
    if visible_rows.clone().any(|row| !row_is_cached(row)) {
        return None;
    }
    for distance in 1..=total_rows {
        if let Some(row) = visible_rows.start.checked_sub(distance)
            && !row_is_cached(row)
        {
            return Some(row);
        }
        let row = visible_rows.end.saturating_add(distance - 1);
        if row < total_rows && !row_is_cached(row) {
            return Some(row);
        }
    }
    None
}

fn hash_stable_glyph_decorations<H: Hasher>(decorations: &TerminalLineDecorations, hasher: &mut H) {
    decorations.active_search_ranges.hash(hasher);
}

fn hash_styled_spans<H: Hasher>(
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    hasher: &mut H,
) {
    if let Some(spans) = ansi_spans {
        spans.len().hash(hasher);
        for span in spans {
            span.text.hash(hasher);
            span.style.hash(hasher);
        }
    } else {
        0usize.hash(hasher);
    }
}

fn terminal_cursor_cell_hidden(snapshot: &TerminalSnapshot) -> bool {
    snapshot
        .cell(snapshot.cursor.row, snapshot.cursor.col)
        .is_some_and(|cell| cell.style.hidden)
}

fn terminal_glyph_decorations_needed(decorations: &TerminalLineDecorations) -> bool {
    !decorations.active_search_ranges.is_empty()
}

fn terminal_row_layout_key(
    row_revision: Option<u64>,
    display_line: &str,
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    decorations: &TerminalLineDecorations,
    keyword_excluded_ranges: &[(usize, usize)],
    keyword_spans_present: bool,
    paint_style_key: u64,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    row_revision.hash(&mut hasher);
    if row_revision.is_none() {
        display_line.hash(&mut hasher);
        hash_styled_spans(ansi_spans, &mut hasher);
    }
    hash_stable_glyph_decorations(decorations, &mut hasher);
    keyword_excluded_ranges.hash(&mut hasher);
    keyword_spans_present.hash(&mut hasher);
    paint_style_key.hash(&mut hasher);
    hasher.finish()
}

fn terminal_plain_row_fast_path(
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    keyword_rules: &[ResolvedKeywordHighlightRule],
    decorations: &TerminalLineDecorations,
) -> bool {
    keyword_rules.is_empty()
        && !terminal_glyph_decorations_needed(decorations)
        && terminal_ansi_spans_are_plain(ansi_spans)
}

fn terminal_ansi_spans_are_plain(ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>) -> bool {
    let Some(spans) = ansi_spans else {
        return true;
    };
    let default_style = nyaterm_terminal::CellStyle::default();
    spans
        .iter()
        .all(|span| span.text.is_empty() || span.style == default_style)
}

/// Follow every double-width character with a space, so one glyph is one cell.
///
/// A shaped terminal row is laid out with `force_width`, which puts glyph *n* at
/// `n * cell_width` regardless of how wide the glyph is. A CJK character covers
/// two terminal columns, so without a filler for its second column every
/// character after it is pulled one cell to the left and the row collapses into
/// itself. The filler is a space: it advances the glyph index without painting,
/// and the cell's own background is drawn as a rect underneath either way.
fn pad_wide_cells(text: &str) -> String {
    if !text.chars().any(|ch| terminal_char_cell_width(ch) > 1) {
        return text.to_string();
    }
    let mut padded = String::with_capacity(text.len() + 4);
    for ch in text.chars() {
        padded.push(ch);
        if terminal_is_zero_width_mark(ch) {
            continue;
        }
        for _ in 1..terminal_char_cell_width(ch) {
            padded.push(' ');
        }
    }
    padded
}

fn append_padded_wide_cells(output: &mut String, input: &str) -> usize {
    let start_len = output.len();
    if !input.chars().any(|ch| terminal_char_cell_width(ch) > 1) {
        output.push_str(input);
        return output.len().saturating_sub(start_len);
    }
    for ch in input.chars() {
        output.push(ch);
        if terminal_is_zero_width_mark(ch) {
            continue;
        }
        for _ in 1..terminal_char_cell_width(ch) {
            output.push(' ');
        }
    }
    output.len().saturating_sub(start_len)
}

fn terminal_text_run_for_span(
    span: &TerminalHighlightSpan,
    len: usize,
    base_font: Font,
    normal_weight: f32,
    bold_weight: f32,
    palette: nyaterm_ui::ThemePalette,
) -> TextRun {
    TextRun {
        len,
        font: terminal_run_font(
            base_font,
            span.bold,
            span.italic,
            normal_weight,
            bold_weight,
        ),
        color: span
            .color
            .map(rgb)
            .unwrap_or_else(|| rgb(palette.terminal_fg))
            .into(),
        background_color: None,
        underline: None,
        strikethrough: span.strikeout.then(|| {
            line_strike_color(
                span.color
                    .map(rgb)
                    .unwrap_or_else(|| rgb(palette.terminal_fg))
                    .into(),
            )
        }),
    }
}

#[cfg(test)]
fn terminal_effective_keyword_rules_key(keyword_rules_key: u64, known_empty: bool) -> u64 {
    if known_empty { 0 } else { keyword_rules_key }
}

fn terminal_background_ranges_for_spans(
    spans: &[TerminalHighlightSpan],
) -> Vec<TerminalRowBackgroundRange> {
    let mut out = Vec::new();
    let mut col = 0usize;
    let mut pending_bg: Option<TerminalRowBackgroundRange> = None;
    for span in spans {
        let bg = span.bg;
        let span_cols = terminal_cell_count(&span.text).max(1);
        if let Some(bg) = bg {
            match pending_bg.as_mut() {
                Some(current) if current.bg == bg && current.end == col => {
                    current.end = col + span_cols;
                }
                _ => {
                    if let Some(range) = pending_bg.take() {
                        out.push(range);
                    }
                    pending_bg = Some(TerminalRowBackgroundRange {
                        bg,
                        start: col,
                        end: col + span_cols,
                    });
                }
            }
        } else if let Some(range) = pending_bg.take() {
            out.push(range);
        }
        col += span_cols;
    }
    if let Some(range) = pending_bg.take() {
        out.push(range);
    }
    out
}

fn terminal_underline_ranges_for_spans(
    spans: &[TerminalHighlightSpan],
    palette: nyaterm_ui::ThemePalette,
) -> Vec<TerminalRowUnderlineRange> {
    let mut out = Vec::new();
    let mut pending: Option<TerminalRowUnderlineRange> = None;
    let mut col = 0usize;
    for span in spans {
        let span_cols = terminal_cell_count(&span.text);
        if span_cols == 0 {
            continue;
        }
        if span.underline {
            let color = span.color.unwrap_or(palette.accent);
            match pending.as_mut() {
                Some(current) if current.color == color && current.end == col => {
                    current.end = col + span_cols;
                }
                _ => {
                    if let Some(range) = pending.take() {
                        out.push(range);
                    }
                    pending = Some(TerminalRowUnderlineRange {
                        color,
                        start: col,
                        end: col + span_cols,
                    });
                }
            }
        } else if let Some(range) = pending.take() {
            out.push(range);
        }
        col += span_cols;
    }
    if let Some(range) = pending.take() {
        out.push(range);
    }
    out
}

fn push_terminal_background_ranges(
    row: usize,
    ranges: &[TerminalRowBackgroundRange],
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    for range in ranges {
        flush_bg(
            Some((range.bg, range.start, range.end)),
            row,
            geometry.bounds,
            geometry.visual_y_offset,
            geometry.cell_width,
            geometry.cell_height,
            out,
        );
    }
}

fn push_terminal_zebra_stripes(
    snapshot: &TerminalSnapshot,
    visible_rows: std::ops::Range<usize>,
    target_line: Option<TerminalLineId>,
    palette: nyaterm_ui::ThemePalette,
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    let mut pending: Option<(u32, usize, usize)> = None;
    let flush = |color: u32, start: usize, end: usize, out: &mut Vec<PaintQuad>| {
        let top = (f32::from(geometry.bounds.top())
            + geometry.visual_y_offset
            + start as f32 * geometry.cell_height)
            .floor();
        let bottom = (f32::from(geometry.bounds.top())
            + geometry.visual_y_offset
            + end as f32 * geometry.cell_height)
            .ceil();
        out.push(fill(
            Bounds::new(
                point(geometry.bounds.left(), px(top)),
                size(geometry.bounds.size.width, px((bottom - top).max(0.0))),
            ),
            rgba(color),
        ));
    };

    for row_index in visible_rows {
        let color = snapshot.row(row_index).and_then(|row| {
            if row.line_id.is_some() && row.line_id == target_line {
                Some((palette.accent << 8) | 0x24)
            } else if matches!(
                row.shell_input,
                Some(ShellInputLineKind::Submitted | ShellInputLineKind::Active)
            ) {
                Some((palette.terminal_fg << 8) | 0x0f)
            } else {
                None
            }
        });
        match (pending.as_mut(), color) {
            (Some((pending_color, _, end)), Some(color))
                if *pending_color == color && *end == row_index =>
            {
                *end = row_index + 1;
            }
            (Some(_), color) => {
                let (pending_color, start, end) = pending.take().expect("pending stripe");
                flush(pending_color, start, end, out);
                if let Some(color) = color {
                    pending = Some((color, row_index, row_index + 1));
                }
            }
            (None, Some(color)) => {
                pending = Some((color, row_index, row_index + 1));
            }
            (None, None) => {}
        }
    }
    if let Some((color, start, end)) = pending {
        flush(color, start, end, out);
    }
}

fn terminal_underline_bounds(
    row: usize,
    start: usize,
    end: usize,
    geometry: TerminalPaintGeometry,
) -> Bounds<Pixels> {
    let left = (f32::from(geometry.bounds.left()) + start as f32 * geometry.cell_width).floor();
    let right = (f32::from(geometry.bounds.left()) + end as f32 * geometry.cell_width).ceil();
    let row_top = f32::from(geometry.bounds.top())
        + geometry.visual_y_offset
        + row as f32 * geometry.cell_height;
    let bottom = (row_top + geometry.cell_height).floor();
    let top = (bottom - 2.0).max(row_top);
    Bounds::new(
        point(px(left), px(top)),
        size(px((right - left).max(0.)), px(1.0)),
    )
}

fn push_terminal_underline_ranges(
    row: usize,
    ranges: &[TerminalRowUnderlineRange],
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    for range in ranges {
        if range.end <= range.start {
            continue;
        }
        out.push(fill(
            terminal_underline_bounds(row, range.start, range.end, geometry),
            rgb(range.color),
        ));
    }
}

fn push_dynamic_link_underlines(
    row: usize,
    line: &str,
    decorations: &TerminalLineDecorations,
    palette: nyaterm_ui::ThemePalette,
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    if decorations.link_ranges.is_empty() {
        return;
    }
    let text_cells = terminal_cell_count(line);
    if text_cells == 0 {
        return;
    }
    for &(start, end) in &decorations.link_ranges {
        let start = start.min(text_cells);
        let end = end.min(text_cells);
        if end <= start {
            continue;
        }
        out.push(fill(
            terminal_underline_bounds(row, start, end, geometry),
            rgb(terminal_link_underline_color(palette)),
        ));
    }
}

fn terminal_link_underline_color(palette: nyaterm_ui::ThemePalette) -> u32 {
    palette.text_muted
}

fn push_dynamic_decoration_backgrounds(
    row: usize,
    decorations: &TerminalLineDecorations,
    palette: nyaterm_ui::ThemePalette,
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    for &(start, end) in &decorations.selected_occurrence_ranges {
        push_selected_occurrence_bg(row, start, end, palette, geometry, out);
    }
    for &(start, end) in &decorations.search_ranges {
        push_col_range_bg(row, start, end, palette.terminal_selection, geometry, out);
    }
    for &(start, end) in &decorations.active_search_ranges {
        push_col_range_bg(row, start, end, palette.warning, geometry, out);
    }
}

fn terminal_selection_cols_for_snapshot_row(
    snapshot: &TerminalSnapshot,
    row: usize,
    selection: Option<TerminalGridSelection>,
) -> Option<(usize, usize)> {
    if row >= snapshot.row_count() {
        return None;
    }
    let absolute_end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let absolute_start = absolute_end.saturating_sub(snapshot.row_count());
    let absolute_line = absolute_start.saturating_add(row);
    let (start, end) = selection?.cols_for_absolute_line(absolute_line)?;
    let start = start.min(snapshot.cols);
    let end = end.min(snapshot.cols);
    (end > start).then_some((start, end))
}

fn push_dynamic_selection_background(
    snapshot: &TerminalSnapshot,
    row: usize,
    selection: Option<TerminalGridSelection>,
    palette: nyaterm_ui::ThemePalette,
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    let Some((start, end)) = terminal_selection_cols_for_snapshot_row(snapshot, row, selection)
    else {
        return;
    };
    push_col_range_bg(row, start, end, palette.terminal_selection, geometry, out);
}

fn push_selected_occurrence_bg(
    row: usize,
    start: usize,
    end: usize,
    palette: nyaterm_ui::ThemePalette,
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    if end <= start {
        return;
    }
    let left = (f32::from(geometry.bounds.left()) + start as f32 * geometry.cell_width).floor();
    let top = (f32::from(geometry.bounds.top())
        + geometry.visual_y_offset
        + row as f32 * geometry.cell_height)
        .floor();
    let right = (f32::from(geometry.bounds.left()) + end as f32 * geometry.cell_width).ceil();
    let bottom = (f32::from(geometry.bounds.top())
        + geometry.visual_y_offset
        + (row + 1) as f32 * geometry.cell_height)
        .ceil();
    out.push(fill(
        Bounds::new(
            point(px(left), px(top)),
            size(px((right - left).max(0.0)), px((bottom - top).max(0.0))),
        ),
        rgba((palette.text_muted << 8) | 0x58),
    ));
}

fn push_terminal_image_placeholder(
    rect: Bounds<Pixels>,
    x: Pixels,
    y: Pixels,
    w: Pixels,
    above_text: bool,
    palette: nyaterm_ui::ThemePalette,
    plan: &mut NyaTerminalPaintPlan,
) {
    let mut wash = rgb(palette.accent);
    wash.a = 0.18;
    let bar = Bounds::new(point(x, y), size(w, px(2.)));
    let mut bar_color = rgb(palette.accent);
    bar_color.a = 0.55;
    if above_text {
        plan.placeholders_above.push(fill(rect, wash));
        plan.placeholders_above.push(fill(bar, bar_color));
    } else {
        plan.placeholders_under.push(fill(rect, wash));
        plan.placeholders_under.push(fill(bar, bar_color));
    }
}

impl IntoElement for NyaTerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for NyaTerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = NyaTerminalPaintPlan;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = if self.fill_height {
            relative(1.).into()
        } else {
            px(terminal_layout_height_px(
                self.cell_height,
                self.snapshot.row_count(),
                self.layout_rows,
            ))
            .into()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let started_at = Instant::now();
        let visible_bounds = window.content_mask().bounds.intersect(&bounds);
        if visible_bounds.size.width <= px(0.) || visible_bounds.size.height <= px(0.) {
            return NyaTerminalPaintPlan::default();
        }
        let layout_cache = self.layout_cache.clone();
        let mut layout_cache = layout_cache.as_ref().and_then(|cache| cache.lock().ok());
        let cache_stats_before = layout_cache
            .as_ref()
            .map(|cache| (cache.hits, cache.misses));
        let mut plan = NyaTerminalPaintPlan::default();
        let cell_w = self.cell_width.max(1.);
        let scale_factor = window.scale_factor();
        let cell_h = nyaterm_core::terminal_snapped_cell_height(self.cell_height, scale_factor);
        let font_size = px(self.font_size.max(8.));
        let mut base_font = font(SharedString::from(self.font_family.clone()));
        base_font.fallbacks = self.font_fallbacks.clone();
        // Terminal cells advance by character columns, while font ligatures can
        // collapse several characters into one glyph. Disable contextual
        // ligatures so GPUI's fixed-width shaping keeps one glyph position per
        // terminal cell and does not shift the remainder of a row.
        base_font.features = FontFeatures::disable_ligatures();
        let keyword_rules_key = if let Some(highlights) = self.keyword_highlights.as_ref() {
            highlights.rules_key()
        } else if self.keyword_rules.is_empty() {
            0
        } else if let Some(cache) = layout_cache.as_deref_mut() {
            cache.keyword_rules_key(&self.keyword_rules)
        } else {
            self.keyword_rules_key()
        };
        let compiled_keyword_rules = if self.keyword_rules.is_empty() {
            Arc::default()
        } else if let Some(cache) = layout_cache.as_deref_mut() {
            cache.compiled_keyword_rules(keyword_rules_key, self.keyword_rules.as_slice())
        } else {
            Arc::new(compile_keyword_rules(self.keyword_rules.as_slice()))
        };
        let keyword_paint_style_key = self.paint_style_key(keyword_rules_key);
        let empty_keyword_paint_style_key = if keyword_rules_key == 0 {
            keyword_paint_style_key
        } else {
            self.paint_style_key(0)
        };

        let visual_y_offset = self.visual_y_offset;
        let paint_geometry = TerminalPaintGeometry {
            bounds,
            visual_y_offset,
            cell_width: cell_w,
            cell_height: cell_h,
        };
        let visible_rows = terminal_visible_rows_for_clipped_bounds(
            bounds,
            visible_bounds,
            cell_h,
            self.snapshot.row_count(),
            visual_y_offset,
        );
        let visible_row_start = visible_rows.start;
        let visible_row_end = visible_rows.end;
        let visible_row_count = visible_rows.len();
        if self.zebra_stripes_enabled {
            push_terminal_zebra_stripes(
                self.snapshot.as_ref(),
                visible_rows.clone(),
                self.target_line,
                self.palette,
                paint_geometry,
                &mut plan.zebra_stripes,
            );
        }
        // Follow the editor model: once the visible viewport is entirely hot,
        // spend at most one subsequent frame shaping the nearest retained row.
        // Any changed visible row suppresses this work, keeping input/output
        // latency ahead of speculative scroll preparation.
        let prefetch_row = layout_cache.as_deref().and_then(|cache| {
            terminal_layout_prefetch_row(visible_rows.clone(), self.snapshot.row_count(), |row| {
                let (key, reuse_key) = self.row_layout_cache_keys(
                    row,
                    keyword_paint_style_key,
                    empty_keyword_paint_style_key,
                );
                cache.contains_paint_row(key, reuse_key)
            })
        });
        let mut rows_to_prepare = Vec::with_capacity(visible_row_count.saturating_add(1));
        rows_to_prepare.extend(visible_rows.clone());
        if let Some(row) = prefetch_row {
            rows_to_prepare.push(row);
        }
        for row in rows_to_prepare {
            let row_is_visible = row >= visible_row_start && row < visible_row_end;
            let snapshot_row = self.snapshot.row(row);
            let line = snapshot_row.map(|row| row.text.as_str()).unwrap_or("");
            let display_line = if line.is_empty() { " " } else { line };
            let ansi = snapshot_row.map(|row| row.styled_spans.as_ref());
            let row_revision = snapshot_row.map(|row| row.revision);
            let keyword_lookup = self.keyword_highlights.as_ref().and_then(|highlights| {
                highlights
                    .lookup(row, self.snapshot.as_ref())
                    .or_else(|| highlights.stale_lookup(row, self.snapshot.as_ref()))
            });
            let keyword_result_known_empty = keyword_lookup
                .as_ref()
                .is_some_and(|lookup| lookup.is_known_empty());
            let keyword_ranges = keyword_lookup.as_ref().and_then(|lookup| lookup.ranges());
            let keyword_spans_present = keyword_ranges.is_some();
            let row_paint_style_key = if keyword_result_known_empty {
                empty_keyword_paint_style_key
            } else {
                keyword_paint_style_key
            };
            let default_decorations;
            let decorations = if let Some(decorations) = self.decorations.get(row) {
                decorations
            } else {
                default_decorations = TerminalLineDecorations::default();
                &default_decorations
            };
            let keyword_excluded_ranges =
                terminal_keyword_exclusion_ranges(snapshot_row, &decorations.link_ranges);
            let keyword_exclusions_affect_glyphs = !keyword_result_known_empty
                && keyword_rules_key != 0
                && !keyword_excluded_ranges.is_empty();
            let y = px(f32::from(bounds.top()) + visual_y_offset + row as f32 * cell_h);

            if row_is_visible {
                if !decorations.active_search_ranges.is_empty() {
                    plan.active_markers.push(fill(
                        Bounds::new(point(bounds.left(), y), size(px(2.), px(cell_h))),
                        rgb(self.palette.warning),
                    ));
                }
                push_dynamic_decoration_backgrounds(
                    row,
                    decorations,
                    self.palette,
                    paint_geometry,
                    &mut plan.decoration_backgrounds,
                );
                push_dynamic_selection_background(
                    self.snapshot.as_ref(),
                    row,
                    self.selection,
                    self.palette,
                    paint_geometry,
                    &mut plan.decoration_backgrounds,
                );
                push_dynamic_link_underlines(
                    row,
                    line,
                    decorations,
                    self.palette,
                    paint_geometry,
                    &mut plan.underlines,
                );
            }

            let row_layout_key = |paint_style_key: u64, keyword_spans_present: bool| {
                let keyword_excluded_ranges = if keyword_exclusions_affect_glyphs {
                    keyword_excluded_ranges.as_slice()
                } else {
                    &[]
                };
                terminal_row_layout_key(
                    row_revision,
                    display_line,
                    ansi,
                    decorations,
                    keyword_excluded_ranges,
                    keyword_spans_present,
                    paint_style_key,
                )
            };
            let row_key = row_layout_key(row_paint_style_key, keyword_spans_present);
            // Reuse a pending row only when its paint is equivalent to the parsed result.
            // TerminalSurface intentionally omits synchronous rules, so a matching result
            // there must rebuild instead of promoting the cached plain row as highlighted.
            let pending_keyword_row_is_equivalent = keyword_lookup.is_some()
                && (keyword_result_known_empty || !self.keyword_rules.is_empty());
            let pending_keyword_row_key = pending_keyword_row_is_equivalent
                .then(|| row_layout_key(keyword_paint_style_key, false))
                .filter(|pending_key| *pending_key != row_key);
            let build_row = |window: &mut Window| {
                let row_keyword_rules: &[ResolvedKeywordHighlightRule] =
                    if keyword_result_known_empty {
                        &[]
                    } else {
                        self.keyword_rules.as_slice()
                    };
                if keyword_ranges.is_none()
                    && terminal_plain_row_fast_path(ansi, row_keyword_rules, decorations)
                {
                    let text = pad_wide_cells(display_line);
                    let text_runs = vec![TextRun {
                        len: text.len().max(1),
                        font: terminal_run_font(
                            base_font.clone(),
                            false,
                            false,
                            self.normal_weight,
                            self.bold_weight,
                        ),
                        color: rgb(self.palette.terminal_fg).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }];
                    let line_started_at = Instant::now();
                    let line = Arc::new(window.text_system().shape_line(
                        SharedString::from(text),
                        font_size,
                        &text_runs,
                        Some(px(cell_w)),
                    ));
                    return (
                        line,
                        line_started_at.elapsed(),
                        text_runs.len(),
                        Vec::new(),
                        Vec::new(),
                    );
                }

                // Base spans drive explicit terminal cell backgrounds only (under images).
                let background_spans = keyword_ranges
                    .map(|ranges| {
                        terminal_highlight_spans_with_keyword_ranges(
                            display_line,
                            ansi,
                            Some(ranges.as_ref()),
                            &keyword_excluded_ranges,
                            self.palette,
                        )
                    })
                    .unwrap_or_else(|| {
                        let row_compiled_keyword_rules: &[CompiledKeywordRule] =
                            if keyword_result_known_empty {
                                &[]
                            } else {
                                compiled_keyword_rules.as_slice()
                            };
                        terminal_highlight_spans_compiled(
                            display_line,
                            ansi,
                            row_compiled_keyword_rules,
                            &[],
                            &[],
                            &[],
                            &[],
                            &keyword_excluded_ranges,
                            self.palette,
                        )
                    });
                // Glyph spans intentionally exclude search/selection/cursor state so
                // dynamic overlays do not invalidate shaped base rows.
                let glyph_spans_storage;
                let glyph_spans = if decorations.active_search_ranges.is_empty() {
                    background_spans.as_slice()
                } else {
                    glyph_spans_storage = apply_search_ranges(
                        background_spans.clone(),
                        &decorations.active_search_ranges,
                        true,
                        self.palette,
                    );
                    glyph_spans_storage.as_slice()
                };
                let background_ranges = terminal_background_ranges_for_spans(&background_spans);
                let underline_ranges =
                    terminal_underline_ranges_for_spans(glyph_spans, self.palette);

                let mut text =
                    String::with_capacity(display_line.len().saturating_add(glyph_spans.len()));
                let mut text_runs = Vec::with_capacity(glyph_spans.len());
                for span in glyph_spans {
                    let run_len = append_padded_wide_cells(&mut text, &span.text);
                    if run_len > 0 {
                        text_runs.push(terminal_text_run_for_span(
                            span,
                            run_len,
                            base_font.clone(),
                            self.normal_weight,
                            self.bold_weight,
                            self.palette,
                        ));
                    }
                }

                if text.is_empty() {
                    text.push(' ');
                    text_runs.push(TextRun {
                        len: 1,
                        font: terminal_run_font(
                            base_font.clone(),
                            false,
                            false,
                            self.normal_weight,
                            self.bold_weight,
                        ),
                        color: rgb(self.palette.terminal_fg).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
                let line_started_at = Instant::now();
                let line = Arc::new(window.text_system().shape_line(
                    SharedString::from(text),
                    font_size,
                    &text_runs,
                    Some(px(cell_w)),
                ));
                (
                    line,
                    line_started_at.elapsed(),
                    text_runs.len(),
                    background_ranges,
                    underline_ranges,
                )
            };
            let (painted_row, did_shape, shape_duration) = if let Some(cache) =
                layout_cache.as_deref_mut()
            {
                cache.paint_row_reusing(row, row_key, pending_keyword_row_key, || build_row(window))
            } else {
                let (line, duration, text_run_count, background_ranges, underline_ranges) =
                    build_row(window);
                (
                    Arc::new(CachedTerminalPaintRow {
                        line,
                        background_ranges,
                        underline_ranges,
                        text_run_count,
                    }),
                    true,
                    duration,
                )
            };
            if did_shape {
                plan.shape_line_count = plan.shape_line_count.saturating_add(1);
                plan.shape_line_duration += shape_duration;
            }
            if row_is_visible {
                push_terminal_background_ranges(
                    row,
                    &painted_row.background_ranges,
                    paint_geometry,
                    &mut plan.backgrounds,
                );
                plan.text_run_count = plan
                    .text_run_count
                    .saturating_add(painted_row.text_run_count);
                plan.rows.push(TerminalPaintRow {
                    y,
                    line: Arc::clone(&painted_row.line),
                });
                push_terminal_underline_ranges(
                    row,
                    &painted_row.underline_ranges,
                    paint_geometry,
                    &mut plan.underlines,
                );
            } else {
                plan.prefetched_row_count = plan.prefetched_row_count.saturating_add(1);
            }
        }
        drop(layout_cache);

        // Graphics protocol placements (Kitty / iTerm2 / Sixel).
        // Kitty z>0 places above text; everything else stays under the glyph layer.
        for image in &self.snapshot.images {
            if image.width_cells == 0 || image.height_cells == 0 {
                continue;
            }
            let image_row_end = image.row.saturating_add(image.height_cells);
            if image_row_end <= visible_row_start || image.row >= visible_row_end {
                continue;
            }
            let x = px(f32::from(bounds.left())
                + (image.col as f32 - image.source_col_cells as f32) * cell_w);
            let y = px(f32::from(bounds.top())
                + visual_y_offset
                + (image.row as f32 - image.source_row_cells as f32) * cell_h);
            let w = px(image.image_width_cells as f32 * cell_w);
            let h = px(image.image_height_cells as f32 * cell_h);
            let rect = Bounds::new(point(x, y), size(w, h));
            match crate::images::cached_render_image(
                image.id,
                image.content_id,
                Arc::clone(&image.data),
            ) {
                crate::images::CachedRenderImage::Ready(decoded) => {
                    let paint = TerminalImagePaint {
                        bounds: rect,
                        image: decoded,
                    };
                    if image.above_text {
                        plan.images_above.push(paint);
                    } else {
                        plan.images_under.push(paint);
                    }
                }
                crate::images::CachedRenderImage::Pending => {
                    window.refresh();
                    push_terminal_image_placeholder(
                        rect,
                        x,
                        y,
                        w,
                        image.above_text,
                        self.palette,
                        &mut plan,
                    );
                }
                crate::images::CachedRenderImage::Failed => {
                    push_terminal_image_placeholder(
                        rect,
                        x,
                        y,
                        w,
                        image.above_text,
                        self.palette,
                        &mut plan,
                    );
                }
            }
        }

        if self.show_cursor
            && self.snapshot.cursor.row < self.snapshot.row_count()
            && self.snapshot.cursor.col < self.snapshot.cols.max(1)
            && self.snapshot.cursor.row >= visible_row_start
            && self.snapshot.cursor.row < visible_row_end
        {
            let left =
                (f32::from(bounds.left()) + self.snapshot.cursor.col as f32 * cell_w).floor();
            let top = (f32::from(bounds.top())
                + visual_y_offset
                + self.snapshot.cursor.row as f32 * cell_h)
                .floor();
            let right =
                (f32::from(bounds.left()) + (self.snapshot.cursor.col + 1) as f32 * cell_w).ceil();
            let bottom = (f32::from(bounds.top())
                + visual_y_offset
                + (self.snapshot.cursor.row + 1) as f32 * cell_h)
                .ceil();
            let x = px(left);
            let y = px(top);
            let width = (right - left).max(1.);
            let height = (bottom - top).max(1.);
            let cursor_bounds = match self.cursor_style.as_str() {
                "bar" => Bounds::new(point(x, y), size(px(2.), px(height))),
                "underline" => {
                    Bounds::new(point(x, px((bottom - 2.).floor())), size(px(width), px(2.)))
                }
                _ => Bounds::new(point(x, y), size(px(width), px(height))),
            };
            plan.cursor_background = Some(fill(cursor_bounds, rgb(self.palette.terminal_cursor)));
            if self.cursor_style.as_str() != "bar"
                && self.cursor_style.as_str() != "underline"
                && !terminal_cursor_cell_hidden(&self.snapshot)
            {
                let cursor_line = self.snapshot.line(self.snapshot.cursor.row).unwrap_or("");
                let cursor_text = terminal_cell_text_at_col(cursor_line, self.snapshot.cursor.col);
                let cursor_runs = vec![TextRun {
                    len: cursor_text.len().max(1),
                    font: terminal_run_font(
                        base_font,
                        false,
                        false,
                        self.normal_weight,
                        self.bold_weight,
                    ),
                    color: rgb(self.palette.terminal_bg).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }];
                let cursor_key = self.cursor_glyph_layout_key(&cursor_text);
                let build_cursor_glyph = |window: &mut Window| {
                    let started_at = Instant::now();
                    let line = Arc::new(window.text_system().shape_line(
                        SharedString::from(cursor_text),
                        font_size,
                        &cursor_runs,
                        Some(px(cell_w)),
                    ));
                    (line, started_at.elapsed())
                };
                let cursor_layout_cache = self.layout_cache.clone();
                let mut cursor_layout_cache = cursor_layout_cache
                    .as_ref()
                    .and_then(|cache| cache.lock().ok());
                let (line, did_shape, shape_duration) =
                    if let Some(cache) = cursor_layout_cache.as_deref_mut() {
                        cache.cursor_glyph(cursor_key, || build_cursor_glyph(window))
                    } else {
                        let (line, duration) = build_cursor_glyph(window);
                        (line, true, duration)
                    };
                if did_shape {
                    plan.shape_line_count = plan.shape_line_count.saturating_add(1);
                    plan.shape_line_duration += shape_duration;
                }
                plan.text_run_count = plan.text_run_count.saturating_add(cursor_runs.len());
                plan.cursor_glyph = Some(TerminalCursorGlyphPaint {
                    origin: point(x, y),
                    line,
                });
            }
        }

        let cache_stats_after = self
            .layout_cache
            .as_ref()
            .and_then(|cache| cache.lock().ok())
            .map(|cache| (cache.hits, cache.misses));
        let elapsed = started_at.elapsed();
        if elapsed.as_millis() >= TERMINAL_ELEMENT_PREPAINT_SLOW_MS {
            let (cache_hits, cache_misses) = cache_stats_after.unwrap_or((0, 0));
            let (cache_hit_delta, cache_miss_delta) =
                cache_stats_before.map_or((0, 0), |(before_hits, before_misses)| {
                    (
                        cache_hits.saturating_sub(before_hits),
                        cache_misses.saturating_sub(before_misses),
                    )
                });
            tracing::warn!(
                diagnostic = "terminal_element_prepaint",
                total_ms = elapsed.as_millis(),
                visible_row_start,
                visible_row_end,
                visible_row_count,
                snapshot_rows = self.snapshot.row_count(),
                snapshot_cols = self.snapshot.cols,
                styled_lines = self.snapshot.row_count(),
                decorations = self.decorations.len(),
                keyword_rules = self.keyword_rules.len(),
                images = self.snapshot.images.len(),
                backgrounds = plan.backgrounds.len(),
                underlines = plan.underlines.len(),
                decoration_backgrounds = plan.decoration_backgrounds.len(),
                active_markers = plan.active_markers.len(),
                shaped_rows = plan.rows.len(),
                prefetched_rows = plan.prefetched_row_count,
                shape_line_count = plan.shape_line_count,
                shape_line_ms = plan.shape_line_duration.as_millis(),
                text_run_count = plan.text_run_count,
                images_under = plan.images_under.len(),
                images_above = plan.images_above.len(),
                placeholders_under = plan.placeholders_under.len(),
                placeholders_above = plan.placeholders_above.len(),
                cache_hit_delta,
                cache_miss_delta,
                cache_hits,
                cache_misses,
                "slow terminal element prepaint"
            );
        }

        plan
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started_at = Instant::now();
        let cell_height =
            nyaterm_core::terminal_snapped_cell_height(self.cell_height, window.scale_factor());
        let zebra_stripes = prepaint.zebra_stripes.len();
        let backgrounds = prepaint.backgrounds.len();
        let images_under = prepaint.images_under.len();
        let placeholders_under = prepaint.placeholders_under.len();
        let decoration_backgrounds = prepaint.decoration_backgrounds.len();
        let active_markers = prepaint.active_markers.len();
        let shaped_rows = prepaint.rows.len();
        let underlines = prepaint.underlines.len();
        let images_above = prepaint.images_above.len();
        let placeholders_above = prepaint.placeholders_above.len();
        let cursor = prepaint.cursor_background.is_some();
        let cursor_glyph = prepaint.cursor_glyph.is_some();
        let shape_line_count = prepaint.shape_line_count;
        let shape_line_ms = prepaint.shape_line_duration.as_millis();
        let text_run_count = prepaint.text_run_count;
        let viewport_mask = ContentMask {
            bounds: window.content_mask().bounds.intersect(&bounds),
        };
        window.with_content_mask(Some(viewport_mask), |window| {
            // zebra → cell bg → under images → search/selection → marks → text → images → cursor
            for quad in prepaint.zebra_stripes.drain(..) {
                window.paint_quad(quad);
            }
            for quad in prepaint.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            for image in prepaint.images_under.drain(..) {
                let _ = window.paint_image(
                    image.bounds,
                    image.bounds,
                    gpui::Corners::default(),
                    image.image,
                    0,
                    false,
                );
            }
            for quad in prepaint.placeholders_under.drain(..) {
                window.paint_quad(quad);
            }
            for quad in prepaint.decoration_backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            for quad in prepaint.active_markers.drain(..) {
                window.paint_quad(quad);
            }
            for row in prepaint.rows.drain(..) {
                let _ = row.line.paint(
                    point(bounds.left(), row.y),
                    px(cell_height),
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            for quad in prepaint.underlines.drain(..) {
                window.paint_quad(quad);
            }
            for image in prepaint.images_above.drain(..) {
                let _ = window.paint_image(
                    image.bounds,
                    image.bounds,
                    gpui::Corners::default(),
                    image.image,
                    0,
                    false,
                );
            }
            for quad in prepaint.placeholders_above.drain(..) {
                window.paint_quad(quad);
            }
            if let Some(cursor) = prepaint.cursor_background.take() {
                window.paint_quad(cursor);
            }
            if let Some(cursor_glyph) = prepaint.cursor_glyph.take() {
                let _ = cursor_glyph.line.paint(
                    cursor_glyph.origin,
                    px(cell_height),
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        });
        let elapsed = started_at.elapsed();
        if elapsed.as_millis() >= TERMINAL_ELEMENT_PAINT_SLOW_MS {
            tracing::warn!(
                diagnostic = "terminal_element_paint",
                total_ms = elapsed.as_millis(),
                snapshot_rows = self.snapshot.row_count(),
                snapshot_cols = self.snapshot.cols,
                zebra_stripes,
                backgrounds,
                decoration_backgrounds,
                active_markers,
                shaped_rows,
                underlines,
                images_under,
                images_above,
                placeholders_under,
                placeholders_above,
                cursor,
                cursor_glyph,
                shape_line_count,
                shape_line_ms,
                text_run_count,
                "slow terminal element paint"
            );
        }
    }
}

#[cfg(test)]
fn terminal_visible_rows_for_bounds(
    bounds: Bounds<Pixels>,
    cell_h: f32,
    row_limit: usize,
    visual_y_offset: f32,
) -> std::ops::Range<usize> {
    terminal_visible_rows_for_clipped_bounds(bounds, bounds, cell_h, row_limit, visual_y_offset)
}

fn terminal_visible_rows_for_clipped_bounds(
    bounds: Bounds<Pixels>,
    visible_bounds: Bounds<Pixels>,
    cell_h: f32,
    row_limit: usize,
    visual_y_offset: f32,
) -> std::ops::Range<usize> {
    if row_limit == 0 {
        return 0..0;
    }
    let cell_h = cell_h.max(1.);
    let visible_top = f32::from(visible_bounds.top() - bounds.top());
    let visible_bottom = f32::from(visible_bounds.bottom() - bounds.top());
    let overscan_rows = 1usize;
    let visible_start = ((visible_top - visual_y_offset) / cell_h).floor().max(0.0) as usize;
    let visible_end = ((visible_bottom - visual_y_offset) / cell_h)
        .ceil()
        .max(0.0) as usize;
    let start = visible_start.saturating_sub(overscan_rows).min(row_limit);
    let end = visible_end.saturating_add(overscan_rows).min(row_limit);
    if end < start {
        return start..start;
    }
    start..end
}

fn terminal_layout_rows(snapshot_rows: usize, override_rows: Option<usize>) -> usize {
    override_rows.unwrap_or(snapshot_rows).max(1)
}

fn terminal_layout_height_px(
    cell_height: f32,
    snapshot_rows: usize,
    override_rows: Option<usize>,
) -> f32 {
    cell_height.max(1.0) * terminal_layout_rows(snapshot_rows, override_rows) as f32
}
