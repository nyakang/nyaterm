use gpui::{
    Bounds, Font, FontStyle, FontWeight, Hsla, PaintQuad, Pixels, StrikethroughStyle, fill, point,
    px, rgb, size,
};
use nyaterm_terminal::{
    terminal_byte_index_for_cell_col, terminal_cell_col_for_byte_index, terminal_cell_count,
    terminal_char_cell_width, terminal_is_zero_width_mark,
};

use crate::ansi::ansi_to_highlight_spans_compiled;
use crate::keywords::{CompiledKeywordRule, keyword_matches_compiled, quick_highlight_background};
use crate::types::{TerminalHighlightSpan, TerminalKeywordRange, TerminalPaintGeometry};

pub(super) fn flush_bg(
    pending: Option<(u32, usize, usize)>,
    row: usize,
    bounds: Bounds<Pixels>,
    visual_y_offset: f32,
    cell_w: f32,
    cell_h: f32,
    out: &mut Vec<PaintQuad>,
) {
    let Some((bg, start, end)) = pending else {
        return;
    };
    if end <= start {
        return;
    }
    let left = (f32::from(bounds.left()) + start as f32 * cell_w).floor();
    let top = (f32::from(bounds.top()) + visual_y_offset + row as f32 * cell_h).floor();
    let right = (f32::from(bounds.left()) + end as f32 * cell_w).ceil();
    let bottom = (f32::from(bounds.top()) + visual_y_offset + (row + 1) as f32 * cell_h).ceil();
    out.push(fill(
        Bounds::new(
            point(px(left), px(top)),
            size(px((right - left).max(0.)), px((bottom - top).max(0.))),
        ),
        rgb(bg),
    ));
}
/// Solid background over a half-open [start, end) column range on a viewport row.
pub(super) fn push_col_range_bg(
    row: usize,
    start: usize,
    end: usize,
    color: u32,
    geometry: TerminalPaintGeometry,
    out: &mut Vec<PaintQuad>,
) {
    if end <= start {
        return;
    }
    flush_bg(
        Some((color, start, end)),
        row,
        geometry.bounds,
        geometry.visual_y_offset,
        geometry.cell_width,
        geometry.cell_height,
        out,
    );
}

pub(super) fn terminal_run_font(
    mut font: Font,
    bold: bool,
    italic: bool,
    normal_weight: f32,
    bold_weight: f32,
) -> Font {
    font.weight = FontWeight(if bold { bold_weight } else { normal_weight });
    font.style = if italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    font
}
pub(super) fn line_strike_color(color: Hsla) -> StrikethroughStyle {
    StrikethroughStyle {
        color: Some(color),
        thickness: px(1.0),
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn terminal_highlight_spans_compiled(
    line: &str,
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    compiled_keyword_rules: &[CompiledKeywordRule],
    selected_occurrence_ranges: &[(usize, usize)],
    search_ranges: &[(usize, usize)],
    active_search_ranges: &[(usize, usize)],
    link_ranges: &[(usize, usize)],
    keyword_excluded_ranges: &[(usize, usize)],
    palette: nyaterm_ui::ThemePalette,
    bold_default_foreground: bool,
) -> Vec<TerminalHighlightSpan> {
    let keyword_ranges = (!compiled_keyword_rules.is_empty() && !line.is_empty()).then(|| {
        keyword_matches_compiled(line, compiled_keyword_rules)
            .into_iter()
            .map(|(start, end, color)| TerminalKeywordRange {
                start_col: terminal_cell_col_for_byte_index(line, start),
                end_col: terminal_cell_col_for_byte_index(line, end),
                color,
                background: compiled_keyword_rules
                    .iter()
                    .find(|rule| rule.color == color)
                    .and_then(|rule| rule.background),
            })
            .collect::<Vec<_>>()
    });
    let mut spans = if keyword_ranges
        .as_ref()
        .is_some_and(|ranges| !ranges.is_empty())
    {
        terminal_highlight_spans_with_keyword_ranges_and_options(
            line,
            ansi_spans,
            keyword_ranges.as_deref(),
            keyword_excluded_ranges,
            palette,
            bold_default_foreground,
        )
    } else if let Some(ansi) = ansi_spans
        && !(ansi.is_empty() || (ansi.len() == 1 && ansi[0].text.is_empty()))
    {
        ansi_to_highlight_spans_compiled(ansi, palette, &[], bold_default_foreground)
    } else {
        terminal_highlight_spans_with_keyword_ranges_and_options(
            line,
            ansi_spans,
            None,
            keyword_excluded_ranges,
            palette,
            bold_default_foreground,
        )
    };
    if !link_ranges.is_empty() {
        spans = apply_action_link_ranges(spans, link_ranges, palette);
    }
    if !selected_occurrence_ranges.is_empty() {
        spans = apply_selected_occurrence_ranges(spans, selected_occurrence_ranges, palette);
    }
    if !search_ranges.is_empty() {
        spans = apply_search_ranges(spans, search_ranges, false, palette);
    }
    if !active_search_ranges.is_empty() {
        spans = apply_search_ranges(spans, active_search_ranges, true, palette);
    }
    spans
}
/// Underline action-link ranges with the accent color (Tauri decoration look).
pub(super) fn apply_action_link_ranges(
    spans: Vec<TerminalHighlightSpan>,
    ranges: &[(usize, usize)],
    palette: nyaterm_ui::ThemePalette,
) -> Vec<TerminalHighlightSpan> {
    if ranges.is_empty() {
        return spans;
    }
    let mut flat = flatten_highlight_spans(spans);
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let end = end.min(flat.len());
        let start = start.min(end);
        for idx in start..end {
            if let Some(cell) = flat.get_mut(idx) {
                cell.underline = true;
                if cell.color.is_none() {
                    cell.color = Some(palette.accent);
                }
            }
        }
    }
    compress_flat_cells(flat)
}
/// Highlight a half-open [start, end) column range with the theme selection background.
#[cfg(test)]
pub(super) fn apply_selection_range(
    spans: Vec<TerminalHighlightSpan>,
    start: usize,
    end: usize,
    palette: nyaterm_ui::ThemePalette,
) -> Vec<TerminalHighlightSpan> {
    if start >= end {
        return spans;
    }
    let mut flat = flatten_highlight_spans(spans);
    while flat.len() < end {
        flat.push(FlatTerminalCell::blank());
    }
    let end = end.min(flat.len());
    for idx in start..end {
        if let Some(cell) = flat.get_mut(idx) {
            cell.bg = Some(palette.terminal_selection);
            cell.keyword = false;
        }
    }
    compress_flat_cells(flat)
}
pub(super) fn apply_search_ranges(
    spans: Vec<TerminalHighlightSpan>,
    ranges: &[(usize, usize)],
    _active: bool,
    palette: nyaterm_ui::ThemePalette,
) -> Vec<TerminalHighlightSpan> {
    if ranges.is_empty() {
        return spans;
    }
    let mut flat = flatten_highlight_spans(spans);
    let max_end = ranges.iter().map(|(_, end)| *end).max().unwrap_or(0);
    while flat.len() < max_end {
        flat.push(FlatTerminalCell::blank());
    }
    // Active results use the same fill; the element paints their outline separately.
    let bg = palette.terminal_find_bg;
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let end = end.min(flat.len());
        for idx in start..end {
            if let Some(cell) = flat.get_mut(idx) {
                cell.bg = Some(bg);
                cell.keyword = false;
            }
        }
    }
    compress_flat_cells(flat)
}

pub(super) fn apply_selected_occurrence_ranges(
    spans: Vec<TerminalHighlightSpan>,
    ranges: &[(usize, usize)],
    palette: nyaterm_ui::ThemePalette,
) -> Vec<TerminalHighlightSpan> {
    if ranges.is_empty() {
        return spans;
    }
    let mut flat = flatten_highlight_spans(spans);
    let max_end = ranges.iter().map(|(_, end)| *end).max().unwrap_or(0);
    while flat.len() < max_end {
        flat.push(FlatTerminalCell::blank());
    }
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let end = end.min(flat.len());
        let start = start.min(end);
        for idx in start..end {
            if let Some(cell) = flat.get_mut(idx)
                && cell.bg.is_none()
            {
                cell.bg = Some(palette.hover);
            }
        }
    }
    compress_flat_cells(flat)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn terminal_highlight_spans_with_keyword_ranges(
    line: &str,
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    keyword_ranges: Option<&[TerminalKeywordRange]>,
    keyword_excluded_ranges: &[(usize, usize)],
    palette: nyaterm_ui::ThemePalette,
) -> Vec<TerminalHighlightSpan> {
    terminal_highlight_spans_with_keyword_ranges_and_options(
        line,
        ansi_spans,
        keyword_ranges,
        keyword_excluded_ranges,
        palette,
        false,
    )
}

pub(super) fn terminal_highlight_spans_with_keyword_ranges_and_options(
    line: &str,
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    keyword_ranges: Option<&[TerminalKeywordRange]>,
    keyword_excluded_ranges: &[(usize, usize)],
    palette: nyaterm_ui::ThemePalette,
    bold_default_foreground: bool,
) -> Vec<TerminalHighlightSpan> {
    let text = if line.is_empty() { " " } else { line };
    let total_cols = terminal_cell_count(text);
    if total_cols == 0 {
        return vec![plain_terminal_span(" ")];
    }

    let base_runs = terminal_base_style_runs(text, ansi_spans, palette, bold_default_foreground);
    let mut boundaries = Vec::with_capacity(
        2 + base_runs.len().saturating_mul(2)
            + keyword_ranges.map_or(0, |ranges| ranges.len().saturating_mul(2))
            + keyword_excluded_ranges.len().saturating_mul(2),
    );
    boundaries.push(0);
    boundaries.push(total_cols);
    for run in &base_runs {
        push_terminal_span_boundary(&mut boundaries, run.start_col, total_cols);
        push_terminal_span_boundary(&mut boundaries, run.end_col, total_cols);
    }
    if let Some(ranges) = keyword_ranges {
        for range in ranges {
            push_terminal_span_boundary(&mut boundaries, range.start_col, total_cols);
            push_terminal_span_boundary(&mut boundaries, range.end_col, total_cols);
        }
    }
    for &(start, end) in keyword_excluded_ranges {
        push_terminal_span_boundary(&mut boundaries, start, total_cols);
        push_terminal_span_boundary(&mut boundaries, end, total_cols);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let keyword_ranges = keyword_ranges.unwrap_or(&[]);
    let mut keyword_cursor = TerminalRangeCursor::new(keyword_ranges);
    let mut exclusion_cursor = TerminalRangeCursor::new(keyword_excluded_ranges);
    let mut byte_cursor = TerminalByteIndexCursor::default();
    let mut spans = Vec::new();
    let mut base_idx = 0usize;
    for window in boundaries.windows(2) {
        let start_col = window[0];
        let end_col = window[1];
        if start_col >= end_col {
            continue;
        }
        while base_idx + 1 < base_runs.len() && base_runs[base_idx].end_col <= start_col {
            base_idx += 1;
        }
        let Some(base) = base_runs.get(base_idx) else {
            continue;
        };
        if base.end_col <= start_col || base.start_col >= end_col {
            continue;
        }
        let start_byte = byte_cursor.byte_index_for_cell_col(text, start_col);
        let end_byte = byte_cursor.byte_index_for_cell_col(text, end_col);
        if end_byte <= start_byte {
            continue;
        }

        let mut style = base.style;
        if style.allow_keyword
            && !terminal_range_is_excluded(
                start_col,
                end_col,
                keyword_excluded_ranges,
                &mut exclusion_cursor,
            )
            && let Some((color, background)) = terminal_keyword_color_for_range(
                start_col,
                end_col,
                keyword_ranges,
                &mut keyword_cursor,
            )
        {
            style.color = Some(color);
            style.bg = background
                .or_else(|| quick_highlight_background(color))
                .or(style.bg);
            style.keyword = true;
        }
        push_terminal_highlight_span(&mut spans, &text[start_byte..end_byte], style);
    }

    if spans.is_empty() {
        spans.push(plain_terminal_span(" "));
    }
    debug_assert!(terminal_highlight_spans_have_distinct_neighbors(&spans));
    spans
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalSpanStyle {
    color: Option<u32>,
    bg: Option<u32>,
    keyword: bool,
    underline: bool,
    strikeout: bool,
    bold: bool,
    italic: bool,
    hidden: bool,
    allow_keyword: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalStyleRun {
    start_col: usize,
    end_col: usize,
    style: TerminalSpanStyle,
}

fn terminal_base_style_runs(
    line: &str,
    ansi_spans: Option<&[nyaterm_terminal::StyledSpan]>,
    palette: nyaterm_ui::ThemePalette,
    bold_default_foreground: bool,
) -> Vec<TerminalStyleRun> {
    let default_style = TerminalSpanStyle {
        color: None,
        bg: None,
        keyword: false,
        underline: false,
        strikeout: false,
        bold: false,
        italic: false,
        hidden: false,
        allow_keyword: true,
    };
    let total_cols = terminal_cell_count(line);
    let Some(ansi) = ansi_spans else {
        return vec![TerminalStyleRun {
            start_col: 0,
            end_col: total_cols,
            style: default_style,
        }];
    };
    if ansi.is_empty() || (ansi.len() == 1 && ansi[0].text.is_empty()) {
        return vec![TerminalStyleRun {
            start_col: 0,
            end_col: total_cols,
            style: default_style,
        }];
    }

    let mut runs = Vec::new();
    let mut cursor = 0usize;
    for span in ansi {
        if span.text.is_empty() {
            continue;
        }
        let start = cursor.min(line.len());
        let end = cursor.saturating_add(span.text.len()).min(line.len());
        cursor = end;
        if end <= start {
            continue;
        }
        let bg = crate::resolve_cell_bg(palette, span.style, bold_default_foreground);
        let color = if span.style.hidden {
            bg.unwrap_or(palette.terminal_bg)
        } else {
            crate::resolve_cell_fg(palette, span.style, bold_default_foreground)
        };
        let style = TerminalSpanStyle {
            color: Some(color),
            bg,
            keyword: false,
            underline: span.style.underline,
            strikeout: span.style.strikeout,
            bold: span.style.bold,
            italic: span.style.italic,
            hidden: span.style.hidden,
            allow_keyword: !span.style.hidden
                && !span.style.reverse
                && span.style.fg.is_none()
                && span.style.fg_rgb.is_none(),
        };
        runs.push(TerminalStyleRun {
            start_col: terminal_cell_col_for_byte_index(line, start),
            end_col: terminal_cell_col_for_byte_index(line, end),
            style,
        });
    }
    if runs.is_empty() {
        runs.push(TerminalStyleRun {
            start_col: 0,
            end_col: total_cols,
            style: default_style,
        });
    }
    runs
}

fn push_terminal_span_boundary(boundaries: &mut Vec<usize>, boundary: usize, max: usize) {
    boundaries.push(boundary.min(max));
}

#[derive(Default)]
struct TerminalByteIndexCursor {
    col: usize,
    byte: usize,
}

impl TerminalByteIndexCursor {
    fn byte_index_for_cell_col(&mut self, text: &str, target_col: usize) -> usize {
        if target_col == 0 {
            return 0;
        }
        if target_col < self.col {
            return terminal_byte_index_for_cell_col(text, target_col);
        }
        if target_col == self.col {
            return self.byte;
        }

        let mut cells = self.col;
        let base_byte = self.byte;
        for (offset, ch) in text[base_byte..].char_indices() {
            let idx = base_byte + offset;
            if terminal_is_zero_width_mark(ch) && cells > 0 {
                self.byte = idx + ch.len_utf8();
                continue;
            }
            if target_col == cells {
                self.col = cells;
                self.byte = idx;
                return idx;
            }
            let width = terminal_char_cell_width(ch);
            if target_col < cells.saturating_add(width) {
                return idx;
            }
            cells = cells.saturating_add(width);
            self.col = cells;
            self.byte = idx + ch.len_utf8();
        }

        self.col = target_col;
        self.byte = text.len();
        text.len()
    }
}

#[derive(Debug)]
struct TerminalRangeCursor {
    idx: usize,
    ordered_non_overlapping: bool,
}

impl TerminalRangeCursor {
    fn new<T>(ranges: &[T]) -> Self
    where
        T: TerminalRangeBounds,
    {
        Self {
            idx: 0,
            ordered_non_overlapping: terminal_ranges_are_ordered_non_overlapping(ranges),
        }
    }
}

trait TerminalRangeBounds {
    fn start_col(&self) -> usize;
    fn end_col(&self) -> usize;
}

impl TerminalRangeBounds for TerminalKeywordRange {
    fn start_col(&self) -> usize {
        self.start_col
    }

    fn end_col(&self) -> usize {
        self.end_col
    }
}

impl TerminalRangeBounds for (usize, usize) {
    fn start_col(&self) -> usize {
        self.0
    }

    fn end_col(&self) -> usize {
        self.1
    }
}

fn terminal_ranges_are_ordered_non_overlapping<T>(ranges: &[T]) -> bool
where
    T: TerminalRangeBounds,
{
    ranges
        .windows(2)
        .all(|pair| pair[0].end_col() <= pair[1].start_col())
}

fn terminal_keyword_color_for_range(
    start_col: usize,
    end_col: usize,
    keyword_ranges: &[TerminalKeywordRange],
    cursor: &mut TerminalRangeCursor,
) -> Option<(u32, Option<u32>)> {
    if !cursor.ordered_non_overlapping {
        return keyword_ranges.iter().rev().find_map(|range| {
            (range.start_col <= start_col && range.end_col >= end_col)
                .then_some((range.color, range.background))
        });
    }
    while cursor.idx < keyword_ranges.len() && keyword_ranges[cursor.idx].end_col <= start_col {
        cursor.idx += 1;
    }
    keyword_ranges.get(cursor.idx).and_then(|range| {
        (range.start_col <= start_col && range.end_col >= end_col)
            .then_some((range.color, range.background))
    })
}

fn terminal_range_is_excluded(
    start_col: usize,
    end_col: usize,
    keyword_excluded_ranges: &[(usize, usize)],
    cursor: &mut TerminalRangeCursor,
) -> bool {
    if !cursor.ordered_non_overlapping {
        return keyword_excluded_ranges
            .iter()
            .any(|&(start, end)| start <= start_col && end >= end_col);
    }
    while cursor.idx < keyword_excluded_ranges.len()
        && keyword_excluded_ranges[cursor.idx].1 <= start_col
    {
        cursor.idx += 1;
    }
    keyword_excluded_ranges
        .get(cursor.idx)
        .is_some_and(|&(start, end)| start <= start_col && end >= end_col)
}

fn push_terminal_highlight_span(
    out: &mut Vec<TerminalHighlightSpan>,
    text: &str,
    mut style: TerminalSpanStyle,
) {
    if text.is_empty() {
        return;
    }
    style.allow_keyword = false;
    if let Some(previous) = out.last_mut()
        && terminal_span_style(previous) == style
    {
        previous.text.push_str(text);
        return;
    }
    out.push(TerminalHighlightSpan {
        text: text.to_string(),
        color: style.color,
        bg: style.bg,
        keyword: style.keyword,
        underline: style.underline,
        strikeout: style.strikeout,
        bold: style.bold,
        italic: style.italic,
        hidden: style.hidden,
    });
}

fn plain_terminal_span(text: &str) -> TerminalHighlightSpan {
    TerminalHighlightSpan {
        text: text.to_string(),
        color: None,
        bg: None,
        keyword: false,
        underline: false,
        strikeout: false,
        bold: false,
        italic: false,
        hidden: false,
    }
}

fn terminal_span_style(span: &TerminalHighlightSpan) -> TerminalSpanStyle {
    TerminalSpanStyle {
        color: span.color,
        bg: span.bg,
        keyword: span.keyword,
        underline: span.underline,
        strikeout: span.strikeout,
        bold: span.bold,
        italic: span.italic,
        hidden: span.hidden,
        allow_keyword: false,
    }
}

fn terminal_highlight_spans_have_distinct_neighbors(spans: &[TerminalHighlightSpan]) -> bool {
    spans
        .windows(2)
        .all(|pair| terminal_span_style(&pair[0]) != terminal_span_style(&pair[1]))
}

pub(super) fn terminal_keyword_exclusion_ranges(
    _row: Option<&nyaterm_terminal::TerminalSnapshotRow>,
    _link_ranges: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    // Link underlines are independent decorations, so links do not need to suppress keyword color.
    Vec::new()
}

#[derive(Debug, Clone)]
pub(super) struct FlatTerminalCell {
    pub(super) text: String,
    pub(super) color: Option<u32>,
    pub(super) bg: Option<u32>,
    pub(super) keyword: bool,
    pub(super) underline: bool,
    pub(super) strikeout: bool,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) hidden: bool,
}

impl FlatTerminalCell {
    fn blank() -> Self {
        Self {
            text: " ".to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        }
    }

    fn from_span_char(span: &TerminalHighlightSpan, ch: char) -> Self {
        Self {
            text: ch.to_string(),
            color: span.color,
            bg: span.bg,
            keyword: span.keyword,
            underline: span.underline,
            strikeout: span.strikeout,
            bold: span.bold,
            italic: span.italic,
            hidden: span.hidden,
        }
    }
}

pub(super) fn flatten_highlight_spans(spans: Vec<TerminalHighlightSpan>) -> Vec<FlatTerminalCell> {
    let mut flat: Vec<FlatTerminalCell> = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        for ch in span.text.chars() {
            if terminal_is_zero_width_mark(ch)
                && let Some(previous) = flat.last_mut()
            {
                previous.text.push(ch);
                continue;
            }
            flat.push(FlatTerminalCell::from_span_char(&span, ch));
            for _ in 1..terminal_char_cell_width(ch) {
                let mut spacer = FlatTerminalCell::from_span_char(&span, ' ');
                spacer.text.clear();
                flat.push(spacer);
            }
        }
    }
    flat
}

pub(super) fn compress_flat_cells(flat: Vec<FlatTerminalCell>) -> Vec<TerminalHighlightSpan> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < flat.len() {
        let cell = flat[i].clone();
        let mut text = cell.text.clone();
        let mut j = i + 1;
        while j < flat.len() {
            let next = &flat[j];
            if next.color == cell.color
                && next.bg == cell.bg
                && next.keyword == cell.keyword
                && next.underline == cell.underline
                && next.strikeout == cell.strikeout
                && next.bold == cell.bold
                && next.italic == cell.italic
                && next.hidden == cell.hidden
            {
                text.push_str(&next.text);
                j += 1;
            } else {
                break;
            }
        }
        out.push(TerminalHighlightSpan {
            text,
            color: cell.color,
            bg: cell.bg,
            keyword: cell.keyword,
            underline: cell.underline,
            strikeout: cell.strikeout,
            bold: cell.bold,
            italic: cell.italic,
            hidden: cell.hidden,
        });
        i = j;
    }
    out
}

pub(super) fn terminal_cell_text_at_col(line: &str, col: usize) -> String {
    let mut cell_col = 0usize;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if terminal_is_zero_width_mark(ch) {
            if col == 0 {
                let mut text = String::new();
                text.push(ch);
                return text;
            }
            continue;
        }

        let width = terminal_char_cell_width(ch).max(1);
        let mut text = String::new();
        text.push(ch);
        while let Some(next) = chars.peek().copied() {
            if !terminal_is_zero_width_mark(next) {
                break;
            }
            text.push(next);
            chars.next();
        }

        if col >= cell_col && col < cell_col + width {
            return text;
        }
        cell_col += width;
    }

    " ".to_string()
}

#[cfg(test)]
mod tests {
    use nyaterm_terminal::{
        terminal_byte_index_for_cell_col, terminal_cell_col_for_byte_index, terminal_cell_count,
        terminal_is_zero_width_mark,
    };

    use super::{
        apply_action_link_ranges, apply_selection_range, flatten_highlight_spans,
        terminal_cell_text_at_col, terminal_highlight_spans_compiled,
        terminal_highlight_spans_have_distinct_neighbors,
        terminal_highlight_spans_with_keyword_ranges, terminal_keyword_exclusion_ranges,
    };
    use crate::keywords::{CompiledKeywordRule, compile_keyword_rules, quick_highlight_background};
    use crate::types::{TerminalHighlightSpan, TerminalKeywordRange};

    fn plain_span(text: &str) -> TerminalHighlightSpan {
        TerminalHighlightSpan {
            text: text.to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        }
    }

    fn styled_span(
        text: &str,
        style: nyaterm_terminal::CellStyle,
    ) -> Vec<nyaterm_terminal::StyledSpan> {
        vec![nyaterm_terminal::StyledSpan {
            text: text.to_string(),
            style,
        }]
    }

    fn compiled(pattern: &str, color: u32) -> Vec<CompiledKeywordRule> {
        vec![CompiledKeywordRule {
            regex: regex::Regex::new(pattern).unwrap(),
            color,
            background: None,
            priority: 0,
        }]
    }

    #[test]
    fn quick_keyword_ranges_paint_dark_background_with_light_text() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let text_color = 0xfff0ee;
        let expected_bg = quick_highlight_background(text_color).expect("quick background");
        let spans = terminal_highlight_spans_with_keyword_ranges(
            "deny",
            None,
            Some(&[TerminalKeywordRange {
                start_col: 0,
                end_col: 4,
                color: text_color,
                background: None,
            }]),
            &[],
            palette,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].color, Some(text_color));
        assert_eq!(spans[0].bg, Some(expected_bg));
        assert!(spans[0].keyword);
    }

    #[test]
    fn reverse_keyword_ranges_preserve_selected_colors_and_highlight_normal_text() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let mut screen = nyaterm_terminal::TerminalScreen::new(24, 2);
        screen.advance(b"SELECTED\x1b[7mSELECTED\x1b[27mSELECTED");
        let snapshot = screen.snapshot();
        let row = snapshot.row(0).unwrap();
        let ranges = [TerminalKeywordRange {
            start_col: 0,
            end_col: 24,
            color: 0xff2244,
            background: None,
        }];
        let spans = terminal_highlight_spans_with_keyword_ranges(
            &row.text,
            Some(&row.styled_spans),
            Some(&ranges),
            &[],
            palette,
        );
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].color, Some(0xff2244));
        assert!(spans[0].keyword);
        assert_eq!(spans[1].color, Some(palette.terminal_bg));
        assert_eq!(spans[1].bg, Some(palette.terminal_fg));
        assert!(!spans[1].keyword);
        assert_eq!(spans[2].color, Some(0xff2244));
        assert!(spans[2].keyword);
    }

    #[test]
    fn keyword_ranges_apply_to_columns_after_wide_chars() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans = terminal_highlight_spans_with_keyword_ranges(
            "界ERROR",
            None,
            Some(&[TerminalKeywordRange {
                start_col: 2,
                end_col: 7,
                color: 0xff2244,
                background: None,
            }]),
            &[],
            palette,
        );

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "界");
        assert!(!spans[0].keyword);
        assert_eq!(spans[1].text, "ERROR");
        assert!(spans[1].keyword);
        assert_eq!(spans[1].color, Some(0xff2244));
    }

    #[test]
    fn keyword_range_composer_merges_adjacent_equal_styles() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans =
            terminal_highlight_spans_with_keyword_ranges("prefix suffix", None, None, &[], palette);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "prefix suffix");
        assert!(terminal_highlight_spans_have_distinct_neighbors(&spans));
    }

    #[test]
    fn keyword_ranges_preserve_ansi_underline() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let style = nyaterm_terminal::CellStyle {
            underline: true,
            ..nyaterm_terminal::CellStyle::default()
        };
        let spans = terminal_highlight_spans_with_keyword_ranges(
            "ERROR",
            Some(&styled_span("ERROR", style)),
            Some(&[TerminalKeywordRange {
                start_col: 0,
                end_col: 5,
                color: 0xff2244,
                background: None,
            }]),
            &[],
            palette,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "ERROR");
        assert!(spans[0].keyword);
        assert!(spans[0].underline);
        assert_eq!(spans[0].color, Some(0xff2244));
    }

    #[test]
    fn keyword_ranges_respect_explicit_exclusion_ranges() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans = terminal_highlight_spans_with_keyword_ranges(
            "ERROR ERROR",
            None,
            Some(&[
                TerminalKeywordRange {
                    start_col: 0,
                    end_col: 5,
                    color: 0xff2244,
                    background: None,
                },
                TerminalKeywordRange {
                    start_col: 6,
                    end_col: 11,
                    color: 0xff2244,
                    background: None,
                },
            ]),
            &[(6, 11)],
            palette,
        );

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "ERROR");
        assert!(spans[0].keyword);
        assert_eq!(spans[1].text, " ERROR");
        assert!(!spans[1].keyword);
        assert_eq!(spans[1].color, None);
    }

    #[test]
    fn keyword_ranges_keep_later_overlapping_range_priority() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans = terminal_highlight_spans_with_keyword_ranges(
            "ABCDE",
            None,
            Some(&[
                TerminalKeywordRange {
                    start_col: 0,
                    end_col: 5,
                    color: 0x111111,
                    background: None,
                },
                TerminalKeywordRange {
                    start_col: 1,
                    end_col: 4,
                    color: 0x222222,
                    background: None,
                },
            ]),
            &[],
            palette,
        );

        assert_eq!(
            spans
                .iter()
                .map(|span| (span.text.as_str(), span.color, span.keyword))
                .collect::<Vec<_>>(),
            vec![
                ("A", Some(0x111111), true),
                ("BCD", Some(0x222222), true),
                ("E", Some(0x111111), true),
            ]
        );
        assert!(terminal_highlight_spans_have_distinct_neighbors(&spans));
    }

    #[test]
    fn dense_keyword_ranges_compose_without_adjacent_equal_styles() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let line = "a b c d e f g h";
        let ranges = (0..line.len())
            .step_by(2)
            .map(|start| TerminalKeywordRange {
                start_col: start,
                end_col: start + 1,
                color: 0xff2244,
                background: None,
            })
            .collect::<Vec<_>>();

        let spans =
            terminal_highlight_spans_with_keyword_ranges(line, None, Some(&ranges), &[], palette);

        assert_eq!(spans.len(), line.len());
        assert!(spans.iter().step_by(2).all(|span| span.keyword));
        assert!(terminal_highlight_spans_have_distinct_neighbors(&spans));
    }

    #[test]
    fn compiled_keyword_highlights_respect_explicit_exclusion_ranges() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let compiled = compiled("ERROR", 0xff2244);
        let spans = terminal_highlight_spans_compiled(
            "ERROR ERROR",
            None,
            &compiled,
            &[],
            &[],
            &[],
            &[],
            &[(6, 11)],
            palette,
            false,
        );

        assert_eq!(spans.len(), 2);
        assert!(spans[0].keyword);
        assert_eq!(spans[1].text, " ERROR");
        assert!(!spans[1].keyword);
        assert_eq!(spans[1].color, None);
    }

    #[test]
    fn action_link_range_keeps_keyword_and_underline() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let compiled = compiled("ERROR", 0xff2244);
        let link_ranges = [(6, 11)];
        let keyword_excluded_ranges = terminal_keyword_exclusion_ranges(None, &link_ranges);
        let spans = terminal_highlight_spans_compiled(
            "ERROR ERROR",
            None,
            &compiled,
            &[],
            &[],
            &[],
            &link_ranges,
            &keyword_excluded_ranges,
            palette,
            false,
        );

        let flat = flatten_highlight_spans(spans);

        assert!(flat[0..5].iter().all(|cell| cell.keyword));
        assert!(flat[0..5].iter().all(|cell| !cell.underline));
        assert!(!flat[5].keyword);
        assert!(!flat[5].underline);
        assert!(flat[6..11].iter().all(|cell| cell.keyword));
        assert!(flat[6..11].iter().all(|cell| cell.underline));
        assert!(flat[6..11].iter().all(|cell| cell.color == Some(0xff2244)));
    }

    #[test]
    fn motd_action_link_urls_keep_keyword_and_underline() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let line = "Welcome to Ubuntu 24.04.4 LTS\n * Documentation: https://help.ubuntu.com\n * Management: https://landscape.canonical.com\n * Support: https://ubuntu.com/pro";
        let rules = nyaterm_core::get_builtin_keyword_rules(true);
        let compiled = compile_keyword_rules(&rules);
        let urls = [
            "https://help.ubuntu.com",
            "https://landscape.canonical.com",
            "https://ubuntu.com/pro",
        ];
        let link_ranges = urls
            .iter()
            .map(|url| {
                let start = line.find(url).expect("motd url present");
                (start, start + url.len())
            })
            .collect::<Vec<_>>();
        let keyword_excluded_ranges = terminal_keyword_exclusion_ranges(None, &link_ranges);
        let spans = terminal_highlight_spans_compiled(
            line,
            None,
            &compiled,
            &[],
            &[],
            &[],
            &link_ranges,
            &keyword_excluded_ranges,
            palette,
            false,
        );

        let version = spans
            .iter()
            .find(|span| span.text == "24.04.4")
            .expect("version keyword span");
        assert!(version.keyword);
        assert_eq!(version.color, Some(0xff9e64));

        let flat = flatten_highlight_spans(spans);
        for (idx, url) in urls.iter().enumerate() {
            let (start, end) = link_ranges[idx];
            assert!(
                flat[start..end].iter().all(|cell| cell.keyword),
                "url should keep keyword spans: {url}"
            );
            assert!(
                flat[start..end].iter().all(|cell| cell.underline),
                "url should stay underlined: {url}"
            );
            assert!(
                flat[start..end]
                    .iter()
                    .all(|cell| cell.color == Some(0x8be9fd)),
                "url should keep URL keyword color: {url}"
            );
        }
    }

    #[test]
    fn keyword_ranges_do_not_override_explicit_or_hidden_ansi_text() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let explicit = nyaterm_terminal::CellStyle {
            fg_rgb: Some(0x112233),
            ..nyaterm_terminal::CellStyle::default()
        };
        let explicit_spans = terminal_highlight_spans_with_keyword_ranges(
            "ERROR",
            Some(&styled_span("ERROR", explicit)),
            Some(&[TerminalKeywordRange {
                start_col: 0,
                end_col: 5,
                color: 0xff2244,
                background: None,
            }]),
            &[],
            palette,
        );
        assert_eq!(explicit_spans[0].color, Some(0x112233));
        assert!(!explicit_spans[0].keyword);

        let hidden = nyaterm_terminal::CellStyle {
            bg_rgb: Some(0x445566),
            hidden: true,
            ..nyaterm_terminal::CellStyle::default()
        };
        let hidden_spans = terminal_highlight_spans_with_keyword_ranges(
            "secret",
            Some(&styled_span("secret", hidden)),
            Some(&[TerminalKeywordRange {
                start_col: 0,
                end_col: 6,
                color: 0xff2244,
                background: None,
            }]),
            &[],
            palette,
        );
        assert_eq!(hidden_spans[0].color, Some(0x445566));
        assert_eq!(hidden_spans[0].bg, Some(0x445566));
        assert!(!hidden_spans[0].keyword);
    }

    #[test]
    fn selection_columns_treat_combining_mark_as_same_cell() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans = apply_selection_range(vec![plain_span("e\u{301}x")], 1, 2, palette);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "e\u{301}");
        assert_eq!(spans[0].bg, None);
        assert_eq!(spans[1].text, "x");
        assert_eq!(spans[1].bg, Some(palette.terminal_selection));
    }

    #[test]
    fn action_link_columns_treat_combining_mark_as_same_cell() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans = apply_action_link_ranges(vec![plain_span("e\u{301}x")], &[(1, 2)], palette);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "e\u{301}");
        assert!(!spans[0].underline);
        assert_eq!(spans[1].text, "x");
        assert!(spans[1].underline);
    }

    #[test]
    fn action_link_columns_count_wide_chars_as_two_cells() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let spans = apply_action_link_ranges(vec![plain_span("界x")], &[(2, 3)], palette);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "界");
        assert!(!spans[0].underline);
        assert_eq!(spans[1].text, "x");
        assert!(spans[1].underline);
    }

    #[test]
    fn terminal_cell_count_keeps_combining_mark_with_previous_cell() {
        assert_eq!(terminal_cell_count("e\u{301}x"), 2);
        assert_eq!(terminal_cell_count("\u{301}x"), 2);
        assert_eq!(terminal_cell_count(""), 0);
    }

    #[test]
    fn terminal_cell_count_treats_wide_char_as_two_cells() {
        assert_eq!(terminal_cell_count("界x"), 3);
        assert_eq!(terminal_cell_count("界\u{301}x"), 3);
    }

    #[test]
    fn terminal_cell_count_keeps_variation_selector_with_previous_cell() {
        assert!(terminal_is_zero_width_mark('\u{fe0f}'));
        assert_eq!(terminal_cell_count("a\u{fe0f}x"), 2);
    }

    #[test]
    fn terminal_cell_col_for_byte_index_keeps_combining_mark_with_previous_cell() {
        let text = "e\u{301}x";

        assert_eq!(terminal_cell_col_for_byte_index(text, 0), 0);
        assert_eq!(terminal_cell_col_for_byte_index(text, "e".len()), 1);
        assert_eq!(terminal_cell_col_for_byte_index(text, "e\u{301}".len()), 1);
        assert_eq!(terminal_cell_col_for_byte_index(text, text.len()), 2);
    }

    #[test]
    fn terminal_cell_col_for_byte_index_counts_wide_char_columns() {
        let text = "界x";

        assert_eq!(terminal_cell_col_for_byte_index(text, 0), 0);
        assert_eq!(terminal_cell_col_for_byte_index(text, "界".len()), 2);
        assert_eq!(terminal_cell_col_for_byte_index(text, text.len()), 3);
    }

    #[test]
    fn terminal_cell_col_for_byte_index_keeps_variation_selector_with_previous_cell() {
        let text = "a\u{fe0f}x";

        assert_eq!(terminal_cell_col_for_byte_index(text, 0), 0);
        assert_eq!(terminal_cell_col_for_byte_index(text, "a".len()), 1);
        assert_eq!(terminal_cell_col_for_byte_index(text, "a\u{fe0f}".len()), 1);
        assert_eq!(terminal_cell_col_for_byte_index(text, text.len()), 2);
    }

    #[test]
    fn terminal_byte_index_for_cell_col_skips_attached_combining_marks() {
        let text = "e\u{301}x";

        assert_eq!(terminal_byte_index_for_cell_col(text, 0), 0);
        assert_eq!(terminal_byte_index_for_cell_col(text, 1), "e\u{301}".len());
        assert_eq!(terminal_byte_index_for_cell_col(text, 2), text.len());
        assert_eq!(terminal_byte_index_for_cell_col(text, 99), text.len());
    }

    #[test]
    fn terminal_byte_index_for_cell_col_maps_wide_char_spacer_to_base() {
        let text = "界x";

        assert_eq!(terminal_byte_index_for_cell_col(text, 0), 0);
        assert_eq!(terminal_byte_index_for_cell_col(text, 1), 0);
        assert_eq!(terminal_byte_index_for_cell_col(text, 2), "界".len());
        assert_eq!(terminal_byte_index_for_cell_col(text, 3), text.len());
    }

    #[test]
    fn terminal_byte_index_for_cell_col_skips_attached_variation_selector() {
        let text = "a\u{fe0f}x";

        assert_eq!(terminal_byte_index_for_cell_col(text, 0), 0);
        assert_eq!(terminal_byte_index_for_cell_col(text, 1), "a\u{fe0f}".len());
        assert_eq!(terminal_byte_index_for_cell_col(text, 2), text.len());
    }

    #[test]
    fn terminal_cell_text_at_col_keeps_combining_marks() {
        assert_eq!(terminal_cell_text_at_col("e\u{301}x", 0), "e\u{301}");
        assert_eq!(terminal_cell_text_at_col("e\u{301}x", 1), "x");
    }

    #[test]
    fn terminal_cell_text_at_col_maps_wide_spacer_to_base_glyph() {
        assert_eq!(terminal_cell_text_at_col("界x", 0), "界");
        assert_eq!(terminal_cell_text_at_col("界x", 1), "界");
        assert_eq!(terminal_cell_text_at_col("界x", 2), "x");
        assert_eq!(terminal_cell_text_at_col("界x", 3), " ");
    }
}
