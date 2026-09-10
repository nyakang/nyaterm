use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_terminal::{TerminalSnapshot, terminal_cell_col_for_byte_index};

use crate::element::{TerminalBufferMatch, TerminalSearchFlags};
use crate::types::{TerminalHighlightSpan, TerminalKeywordRange};

pub(super) type CompiledKeywordRules = Vec<CompiledKeywordRule>;

#[derive(Debug)]
pub(super) struct CompiledKeywordRule {
    pub(super) regex: regex::Regex,
    pub(super) color: u32,
    pub(super) priority: usize,
}

#[derive(Debug, Clone)]
struct CompiledLiteralKeywordRule {
    pattern: String,
    color: u32,
    priority: usize,
}

pub struct TerminalKeywordHighlighter {
    rules_key: u64,
    regex_rules: CompiledKeywordRules,
    literal_rules: Vec<CompiledLiteralKeywordRule>,
    literal_automaton: Option<AhoCorasick>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TerminalKeywordHighlightPrecomputeStats {
    pub match_duration_us: u64,
    pub range_build_duration_us: u64,
    pub known_rows: usize,
    pub range_count: usize,
    pub requested_rows: usize,
    pub reused_rows: usize,
    pub processed_bytes: usize,
    pub oversized_wrapped_groups: usize,
    pub degraded_rows: usize,
}

const MAX_KEYWORD_WRAPPED_GROUP_ROWS: usize = 512;
const MAX_KEYWORD_WRAPPED_GROUP_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TerminalKeywordRowReuseKey {
    Single { revision: u64 },
    Wrapped { group_key: u64, row_offset: usize },
}

/// Immutable keyword data prepared away from GPUI's paint path.
pub struct TerminalKeywordHighlightSnapshot {
    rules_key: u64,
    display_offset: usize,
    row_revisions: Vec<u64>,
    wrapped_flags: Vec<bool>,
    row_reuse_keys: Vec<Option<TerminalKeywordRowReuseKey>>,
    known_rows: Vec<bool>,
    rows: Vec<Option<Arc<Vec<TerminalKeywordRange>>>>,
    rows_by_reuse_key: HashMap<TerminalKeywordRowReuseKey, Option<Arc<Vec<TerminalKeywordRange>>>>,
}

pub(super) enum TerminalKeywordHighlightLookup<'a> {
    Current(Option<&'a Arc<Vec<TerminalKeywordRange>>>),
    Reused(Option<&'a Arc<Vec<TerminalKeywordRange>>>),
}

impl<'a> TerminalKeywordHighlightLookup<'a> {
    pub(super) fn ranges(&self) -> Option<&'a Arc<Vec<TerminalKeywordRange>>> {
        match self {
            Self::Current(ranges) | Self::Reused(ranges) => *ranges,
        }
    }

    pub(super) fn is_known_empty(&self) -> bool {
        self.ranges().is_none()
    }
}

impl TerminalKeywordHighlightSnapshot {
    pub fn rules_key(&self) -> u64 {
        self.rules_key
    }

    pub fn known_row_count(&self) -> usize {
        self.known_rows.iter().filter(|known| **known).count()
    }

    pub fn range_count(&self) -> usize {
        self.rows
            .iter()
            .filter_map(|ranges| ranges.as_ref())
            .map(|ranges| ranges.len())
            .sum()
    }

    pub fn matches_snapshot(
        &self,
        snapshot: &TerminalSnapshot,
        _palette: nyaterm_ui::ThemePalette,
    ) -> bool {
        self.matches_snapshot_rows(snapshot, _palette, 0..snapshot.row_count())
    }

    pub fn matches_snapshot_rows(
        &self,
        snapshot: &TerminalSnapshot,
        _palette: nyaterm_ui::ThemePalette,
        rows: Range<usize>,
    ) -> bool {
        if self.display_offset != snapshot.display_offset {
            return false;
        }
        let start = rows.start.min(snapshot.row_count());
        let end = rows.end.min(snapshot.row_count()).max(start);
        (start..end).all(|row| self.has_row_at(row, snapshot))
    }

    pub(super) fn lookup(
        &self,
        row: usize,
        snapshot: &TerminalSnapshot,
    ) -> Option<TerminalKeywordHighlightLookup<'_>> {
        snapshot.row(row)?;
        if self.has_row_at(row, snapshot) {
            return Some(TerminalKeywordHighlightLookup::Current(
                self.rows.get(row)?.as_ref(),
            ));
        }
        let reuse_key = terminal_keyword_row_reuse_key(snapshot, row)?;
        self.rows_by_reuse_key
            .get(&reuse_key)
            .map(|ranges| TerminalKeywordHighlightLookup::Reused(ranges.as_ref()))
    }

    pub(super) fn stale_lookup(
        &self,
        row: usize,
        snapshot: &TerminalSnapshot,
    ) -> Option<TerminalKeywordHighlightLookup<'_>> {
        if self.display_offset != snapshot.display_offset
            || self.row_revisions.len() != snapshot.row_count()
        {
            return None;
        }
        snapshot.row(row)?;
        if !self.has_row_at(row, snapshot) {
            return None;
        }
        Some(TerminalKeywordHighlightLookup::Current(
            self.rows.get(row)?.as_ref(),
        ))
    }

    fn has_row_at(&self, row: usize, snapshot: &TerminalSnapshot) -> bool {
        let Some(snapshot_row) = snapshot.row(row) else {
            return false;
        };
        self.known_rows.get(row).copied().unwrap_or(false)
            && self.row_revisions.get(row).copied() == Some(snapshot_row.revision)
            && self.wrapped_flags.get(row).copied() == Some(snapshot_row.wrapped)
            && self.row_reuse_keys.get(row).and_then(|key| *key)
                == terminal_keyword_row_reuse_key(snapshot, row)
    }
}

pub fn terminal_keyword_highlight_expanded_rows(
    snapshot: &TerminalSnapshot,
    rows: Range<usize>,
) -> Range<usize> {
    let start = rows.start.min(snapshot.row_count());
    let end = rows.end.min(snapshot.row_count()).max(start);
    if start == end {
        return start..end;
    }
    let first = terminal_keyword_wrapped_group_bounds(snapshot, start);
    let last = terminal_keyword_wrapped_group_bounds(snapshot, end.saturating_sub(1));
    first.start..last.end
}

pub fn precompute_terminal_keyword_highlights(
    snapshot: &TerminalSnapshot,
    highlighter: &TerminalKeywordHighlighter,
    palette: nyaterm_ui::ThemePalette,
    previous: Option<&TerminalKeywordHighlightSnapshot>,
) -> TerminalKeywordHighlightSnapshot {
    precompute_terminal_keyword_highlights_for_rows(
        snapshot,
        highlighter,
        palette,
        previous,
        0..snapshot.row_count(),
    )
}

pub fn precompute_terminal_keyword_highlights_for_rows(
    snapshot: &TerminalSnapshot,
    highlighter: &TerminalKeywordHighlighter,
    palette: nyaterm_ui::ThemePalette,
    previous: Option<&TerminalKeywordHighlightSnapshot>,
    requested_rows: Range<usize>,
) -> TerminalKeywordHighlightSnapshot {
    precompute_terminal_keyword_highlights_for_rows_with_stats(
        snapshot,
        highlighter,
        palette,
        previous,
        requested_rows,
    )
    .0
}

pub fn precompute_terminal_keyword_highlights_for_rows_with_stats(
    snapshot: &TerminalSnapshot,
    highlighter: &TerminalKeywordHighlighter,
    palette: nyaterm_ui::ThemePalette,
    previous: Option<&TerminalKeywordHighlightSnapshot>,
    requested_rows: Range<usize>,
) -> (
    TerminalKeywordHighlightSnapshot,
    TerminalKeywordHighlightPrecomputeStats,
) {
    precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel(
        snapshot,
        highlighter,
        palette,
        previous,
        requested_rows,
        || false,
    )
    .expect("non-cancelling keyword precompute")
}

pub fn precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel(
    snapshot: &TerminalSnapshot,
    highlighter: &TerminalKeywordHighlighter,
    palette: nyaterm_ui::ThemePalette,
    previous: Option<&TerminalKeywordHighlightSnapshot>,
    requested_rows: Range<usize>,
    mut cancelled: impl FnMut() -> bool,
) -> Option<(
    TerminalKeywordHighlightSnapshot,
    TerminalKeywordHighlightPrecomputeStats,
)> {
    let _ = palette;
    let previous = previous.filter(|previous| previous.rules_key == highlighter.rules_key);
    let requested_rows = terminal_keyword_highlight_expanded_rows(snapshot, requested_rows);
    let mut stats = TerminalKeywordHighlightPrecomputeStats {
        requested_rows: requested_rows.len(),
        ..TerminalKeywordHighlightPrecomputeStats::default()
    };
    let mut known_rows = vec![false; snapshot.row_count()];
    let mut rows: Vec<Option<Arc<Vec<TerminalKeywordRange>>>> = vec![None; snapshot.row_count()];
    let row_reuse_keys = terminal_keyword_row_reuse_keys(snapshot);

    let mut row = 0;
    while row < snapshot.row_count() {
        if cancelled() {
            return None;
        }
        // Soft-wrapped screen rows are one logical line; hard breaks must never
        // join unrelated commands or log records. Only pathological lines degrade.
        let group = terminal_keyword_wrapped_group_bounds(snapshot, row);
        let group_range = group.start..group.end;
        let requested = group.start < requested_rows.end && group.end > requested_rows.start;
        let group_keys = row_reuse_keys
            .get(group_range.clone())
            .and_then(|keys| keys.iter().copied().collect::<Option<Vec<_>>>());
        if let Some(keys) = group_keys.as_ref()
            && let Some(previous) = previous
            && keys
                .iter()
                .all(|key| previous.rows_by_reuse_key.contains_key(key))
        {
            for (idx, key) in group_range.clone().zip(keys.iter()) {
                known_rows[idx] = true;
                rows[idx] = previous.rows_by_reuse_key.get(key).cloned().flatten();
                stats.reused_rows = stats.reused_rows.saturating_add(1);
            }
            row = group.end;
            continue;
        }
        if !requested {
            row = group.end;
            continue;
        }

        let group_rows = if group.oversized {
            stats.oversized_wrapped_groups = stats.oversized_wrapped_groups.saturating_add(1);
            stats.degraded_rows = stats.degraded_rows.saturating_add(group_range.len());
            group_range
                .clone()
                .map(|row| {
                    terminal_keyword_ranges_for_wrapped_group(
                        snapshot,
                        row..row.saturating_add(1),
                        highlighter,
                        Some(&mut stats),
                    )
                    .into_iter()
                    .next()
                    .unwrap_or_default()
                })
                .collect()
        } else {
            terminal_keyword_ranges_for_wrapped_group(
                snapshot,
                group_range.clone(),
                highlighter,
                Some(&mut stats),
            )
        };
        for (idx, ranges) in group_range.zip(group_rows) {
            known_rows[idx] = true;
            rows[idx] = (!ranges.is_empty()).then(|| Arc::new(ranges));
        }
        row = group.end;
    }

    let rows_by_reuse_key = (0..snapshot.row_count())
        .filter(|row| known_rows[*row])
        .filter_map(|row| row_reuse_keys[row].map(|key| (key, rows[row].clone())))
        .collect();
    let snapshot = TerminalKeywordHighlightSnapshot {
        rules_key: highlighter.rules_key,
        display_offset: snapshot.display_offset,
        row_revisions: snapshot.rows().iter().map(|row| row.revision).collect(),
        wrapped_flags: snapshot.rows().iter().map(|row| row.wrapped).collect(),
        row_reuse_keys,
        known_rows,
        rows,
        rows_by_reuse_key,
    };
    stats.known_rows = snapshot.known_row_count();
    stats.range_count = snapshot.range_count();
    Some((snapshot, stats))
}

#[derive(Clone, Copy)]
struct TerminalKeywordWrappedGroup {
    start: usize,
    end: usize,
    oversized: bool,
}

#[derive(Clone, Copy)]
struct TerminalKeywordByteCell {
    row: usize,
    start_col: usize,
    end_col: usize,
    start_byte: usize,
    end_byte: usize,
}

fn terminal_keyword_wrapped_group_bounds(
    snapshot: &TerminalSnapshot,
    row: usize,
) -> TerminalKeywordWrappedGroup {
    let mut start = row.min(snapshot.row_count());
    let mut rows = 1usize;
    let mut bytes = snapshot
        .row(start)
        .map(terminal_keyword_row_bytes)
        .unwrap_or(0);
    while start > 0
        && snapshot.row(start).is_some_and(|row| row.wrapped)
        && rows < MAX_KEYWORD_WRAPPED_GROUP_ROWS
        && bytes < MAX_KEYWORD_WRAPPED_GROUP_BYTES
    {
        start -= 1;
        rows = rows.saturating_add(1);
        bytes = bytes.saturating_add(
            snapshot
                .row(start)
                .map(terminal_keyword_row_bytes)
                .unwrap_or(0),
        );
    }
    let truncated_before = start > 0 && snapshot.row(start).is_some_and(|row| row.wrapped);
    if truncated_before {
        start = row.min(snapshot.row_count());
        rows = 1;
        bytes = snapshot
            .row(start)
            .map(terminal_keyword_row_bytes)
            .unwrap_or(0);
    }
    let mut end = start.saturating_add(1).min(snapshot.row_count());
    while end < snapshot.row_count()
        && snapshot.row(end).is_some_and(|row| row.wrapped)
        && rows < MAX_KEYWORD_WRAPPED_GROUP_ROWS
        && bytes < MAX_KEYWORD_WRAPPED_GROUP_BYTES
    {
        let row_bytes = snapshot
            .row(end)
            .map(terminal_keyword_row_bytes)
            .unwrap_or(0);
        if bytes.saturating_add(row_bytes) > MAX_KEYWORD_WRAPPED_GROUP_BYTES {
            break;
        }
        bytes = bytes.saturating_add(row_bytes);
        rows = rows.saturating_add(1);
        end += 1;
    }
    let truncated_after =
        end < snapshot.row_count() && snapshot.row(end).is_some_and(|row| row.wrapped);
    TerminalKeywordWrappedGroup {
        start,
        end,
        oversized: truncated_before || truncated_after,
    }
}

fn terminal_keyword_row_bytes(row: &nyaterm_terminal::TerminalSnapshotRow) -> usize {
    row.cells
        .iter()
        .filter(|cell| cell.width != 0)
        .map(|cell| cell.text.len().max(1))
        .sum()
}

fn terminal_keyword_row_reuse_key(
    snapshot: &TerminalSnapshot,
    row: usize,
) -> Option<TerminalKeywordRowReuseKey> {
    let group = terminal_keyword_wrapped_group_bounds(snapshot, row);
    if group.oversized || group.end == group.start.saturating_add(1) {
        return snapshot
            .row(row)
            .map(|row| TerminalKeywordRowReuseKey::Single {
                revision: row.revision,
            });
    }

    let mut hasher = DefaultHasher::new();
    (group.start..group.end).for_each(|idx| {
        if let Some(row) = snapshot.row(idx) {
            row.signature.hash(&mut hasher);
            row.revision.hash(&mut hasher);
            row.wrapped.hash(&mut hasher);
        }
    });
    Some(TerminalKeywordRowReuseKey::Wrapped {
        group_key: hasher.finish(),
        row_offset: row.saturating_sub(group.start),
    })
}

fn terminal_keyword_row_reuse_keys(
    snapshot: &TerminalSnapshot,
) -> Vec<Option<TerminalKeywordRowReuseKey>> {
    let mut keys = vec![None; snapshot.row_count()];
    let mut row = 0usize;
    while row < snapshot.row_count() {
        let group = terminal_keyword_wrapped_group_bounds(snapshot, row);
        if group.oversized || group.end == group.start.saturating_add(1) {
            keys[row] = snapshot
                .row(row)
                .map(|row| TerminalKeywordRowReuseKey::Single {
                    revision: row.revision,
                });
            row = group.end;
            continue;
        }

        let mut hasher = DefaultHasher::new();
        for idx in group.start..group.end {
            if let Some(row) = snapshot.row(idx) {
                row.signature.hash(&mut hasher);
                row.revision.hash(&mut hasher);
                row.wrapped.hash(&mut hasher);
            }
        }
        let group_key = hasher.finish();
        for (idx, key) in keys
            .iter_mut()
            .enumerate()
            .take(group.end)
            .skip(group.start)
        {
            *key = Some(TerminalKeywordRowReuseKey::Wrapped {
                group_key,
                row_offset: idx.saturating_sub(group.start),
            });
        }
        row = group.end;
    }
    keys
}

fn terminal_keyword_ranges_for_wrapped_group(
    snapshot: &TerminalSnapshot,
    rows: Range<usize>,
    highlighter: &TerminalKeywordHighlighter,
    mut stats: Option<&mut TerminalKeywordHighlightPrecomputeStats>,
) -> Vec<Vec<TerminalKeywordRange>> {
    let range_build_started = Instant::now();
    let row_count = rows.end.saturating_sub(rows.start);
    let mut row_ranges = vec![Vec::new(); row_count];
    if highlighter.regex_rules.is_empty() && highlighter.literal_rules.is_empty() || row_count == 0
    {
        return row_ranges;
    }

    let mut line = String::new();
    let mut byte_cells = Vec::new();
    for row in rows.clone() {
        let Some(snapshot_row) = snapshot.row(row) else {
            continue;
        };
        for (col, cell) in snapshot_row.cells.iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            let text = if cell.text.is_empty() {
                " "
            } else {
                cell.text()
            };
            let width = usize::from(cell.width).max(1);
            let start_col = col;
            let end_col = col.saturating_add(width);
            let start_byte = line.len();
            line.push_str(text);
            let end_byte = line.len();
            byte_cells.push(TerminalKeywordByteCell {
                row,
                start_col,
                end_col,
                start_byte,
                end_byte,
            });
        }
    }
    let line_build_duration = range_build_started.elapsed();
    if let Some(stats) = stats.as_mut() {
        stats.processed_bytes = stats.processed_bytes.saturating_add(line.len());
    }
    if line.is_empty() || byte_cells.is_empty() {
        if let Some(stats) = stats.as_mut() {
            stats.range_build_duration_us = stats
                .range_build_duration_us
                .saturating_add(duration_micros_u64(line_build_duration));
        }
        return row_ranges;
    }

    let match_started = Instant::now();
    let matches = keyword_matches_highlighter(&line, highlighter);
    let match_duration = match_started.elapsed();
    let range_map_started = Instant::now();
    for (start, end, color) in matches {
        if start >= end || end > line.len() {
            continue;
        }
        let start_idx = byte_cells.partition_point(|cell| cell.end_byte <= start);
        for cell in byte_cells[start_idx..]
            .iter()
            .take_while(|cell| cell.start_byte < end)
        {
            let row_idx = cell.row.saturating_sub(rows.start);
            if let Some(ranges) = row_ranges.get_mut(row_idx) {
                if let Some(previous) = ranges.last_mut()
                    && previous.color == color
                    && previous.end_col >= cell.start_col
                {
                    previous.end_col = previous.end_col.max(cell.end_col);
                    continue;
                }
                ranges.push(TerminalKeywordRange {
                    start_col: cell.start_col,
                    end_col: cell.end_col,
                    color,
                });
            }
        }
    }
    if let Some(stats) = stats.as_mut() {
        stats.match_duration_us = stats
            .match_duration_us
            .saturating_add(duration_micros_u64(match_duration));
        stats.range_build_duration_us = stats
            .range_build_duration_us
            .saturating_add(duration_micros_u64(line_build_duration))
            .saturating_add(duration_micros_u64(range_map_started.elapsed()));
    }

    row_ranges
}

fn duration_micros_u64(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn keyword_matches_highlighter(
    line: &str,
    highlighter: &TerminalKeywordHighlighter,
) -> Vec<(usize, usize, u32)> {
    if line.is_empty()
        || (highlighter.regex_rules.is_empty() && highlighter.literal_rules.is_empty())
    {
        return Vec::new();
    }

    let mut regex_pending = highlighter
        .regex_rules
        .iter()
        .map(|rule| pending_regex_match(rule, line, 0))
        .collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut cursor = 0usize;
    loop {
        for (rule, pending) in highlighter.regex_rules.iter().zip(&mut regex_pending) {
            if pending.is_some_and(|candidate| candidate.start < cursor) {
                *pending = pending_regex_match(rule, line, cursor);
            }
        }
        let literal = highlighter
            .literal_automaton
            .as_ref()
            .and_then(|automaton| automaton.find(&line[cursor..]))
            .and_then(|found| {
                let rule = highlighter.literal_rules.get(found.pattern().as_usize())?;
                Some(PendingKeywordMatch {
                    start: cursor.saturating_add(found.start()),
                    end: cursor.saturating_add(found.end()),
                    color: rule.color,
                    priority: rule.priority,
                })
            });
        let best = regex_pending
            .iter()
            .flatten()
            .copied()
            .chain(literal)
            .reduce(|current, candidate| {
                if keyword_match_is_better(candidate, current) {
                    candidate
                } else {
                    current
                }
            });
        let Some(best) = best else {
            break;
        };
        matches.push((best.start, best.end, best.color));
        cursor = best.end;
        if cursor >= line.len() {
            break;
        }
    }
    matches
}

fn pending_regex_match(
    rule: &CompiledKeywordRule,
    line: &str,
    cursor: usize,
) -> Option<PendingKeywordMatch> {
    find_next_non_empty_match(&rule.regex, line, cursor).map(|(start, end)| PendingKeywordMatch {
        start,
        end,
        color: rule.color,
        priority: rule.priority,
    })
}

pub(super) fn keyword_matches_compiled(
    line: &str,
    compiled: &[CompiledKeywordRule],
) -> Vec<(usize, usize, u32)> {
    let mut pending = compiled
        .iter()
        .map(|rule| {
            find_next_non_empty_match(&rule.regex, line, 0).map(|(start, end)| {
                PendingKeywordMatch {
                    start,
                    end,
                    color: rule.color,
                    priority: rule.priority,
                }
            })
        })
        .collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut cursor = 0;

    loop {
        for (rule, candidate) in compiled.iter().zip(pending.iter_mut()) {
            if candidate.is_some_and(|candidate| candidate.start < cursor) {
                *candidate =
                    find_next_non_empty_match(&rule.regex, line, cursor).map(|(start, end)| {
                        PendingKeywordMatch {
                            start,
                            end,
                            color: rule.color,
                            priority: rule.priority,
                        }
                    });
            }
        }

        let Some(best) = pending
            .iter()
            .flatten()
            .copied()
            .reduce(|current, candidate| {
                if keyword_match_is_better(candidate, current) {
                    candidate
                } else {
                    current
                }
            })
        else {
            break;
        };

        debug_assert!(best.start >= cursor);
        debug_assert!(best.end > best.start);
        matches.push((best.start, best.end, best.color));
        cursor = best.end;
        if cursor >= line.len() {
            break;
        }
    }

    matches
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingKeywordMatch {
    start: usize,
    end: usize,
    color: u32,
    priority: usize,
}

fn keyword_match_is_better(candidate: PendingKeywordMatch, current: PendingKeywordMatch) -> bool {
    candidate.start < current.start
        || (candidate.start == current.start && candidate.end > current.end)
        || (candidate.start == current.start
            && candidate.end == current.end
            && candidate.priority < current.priority)
}

fn find_next_non_empty_match(
    regex: &regex::Regex,
    line: &str,
    start: usize,
) -> Option<(usize, usize)> {
    let mut cursor = start;
    while cursor < line.len() {
        let found = regex.find_at(line, cursor)?;
        let start = found.start();
        let end = found.end();
        if end > start {
            return Some((start, end));
        }
        cursor = next_char_boundary(line, start)?;
    }
    None
}

fn next_char_boundary(line: &str, start: usize) -> Option<usize> {
    if start >= line.len() {
        return None;
    }
    line[start..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| start + offset)
        .or(Some(line.len()))
}

pub fn compile_terminal_keyword_highlighter(
    rules: &[ResolvedKeywordHighlightRule],
) -> TerminalKeywordHighlighter {
    let (regex_rules, literal_rules, literal_automaton) = compile_keyword_rule_sets(rules);
    TerminalKeywordHighlighter {
        rules_key: terminal_keyword_rules_key(rules),
        regex_rules,
        literal_rules,
        literal_automaton,
    }
}

pub fn terminal_keyword_rules_key(rules: &[ResolvedKeywordHighlightRule]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for rule in rules {
        rule.id.hash(&mut hasher);
        rule.name.hash(&mut hasher);
        rule.patterns.hash(&mut hasher);
        rule.color.hash(&mut hasher);
        rule.enabled.hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn keyword_highlight_spans_compiled(
    line: &str,
    compiled: &[CompiledKeywordRule],
) -> Vec<TerminalHighlightSpan> {
    if compiled.is_empty() || line.is_empty() {
        return vec![TerminalHighlightSpan {
            text: line.to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
        }];
    }

    let matches = keyword_matches_compiled(line, compiled);
    if matches.is_empty() {
        return vec![TerminalHighlightSpan {
            text: line.to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
        }];
    }

    let mut spans = Vec::new();
    let mut cursor = 0;
    for (start, end, color) in matches {
        if start > cursor {
            spans.push(TerminalHighlightSpan {
                text: line[cursor..start].to_string(),
                color: None,
                bg: None,
                keyword: false,
                underline: false,
                strikeout: false,
                bold: false,
                italic: false,
            });
        }
        spans.push(TerminalHighlightSpan {
            text: line[start..end].to_string(),
            color: Some(color),
            bg: None,
            keyword: true,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
        });
        cursor = end;
    }

    if cursor < line.len() {
        spans.push(TerminalHighlightSpan {
            text: line[cursor..].to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
        });
    }
    spans
}
pub(super) fn compile_keyword_rules(
    rules: &[ResolvedKeywordHighlightRule],
) -> CompiledKeywordRules {
    compile_keyword_rules_with_filter(rules, |_| true)
}

fn compile_keyword_rule_sets(
    rules: &[ResolvedKeywordHighlightRule],
) -> (
    CompiledKeywordRules,
    Vec<CompiledLiteralKeywordRule>,
    Option<AhoCorasick>,
) {
    let mut regex_rules = Vec::new();
    let mut literal_rules = Vec::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        let color = parse_hex_rgb(&rule.color).unwrap_or(0x79c0ff);
        let alts = rule
            .patterns
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if alts.is_empty() {
            continue;
        }
        let priority = regex_rules.len().saturating_add(literal_rules.len());
        if alts
            .iter()
            .all(|pattern| is_literal_keyword_pattern(pattern))
        {
            literal_rules.extend(alts.into_iter().map(|pattern| CompiledLiteralKeywordRule {
                pattern,
                color,
                priority,
            }));
            continue;
        }
        compile_keyword_regex_alternatives(alts, color, priority, &mut regex_rules);
    }
    literal_rules.sort_by(|left, right| {
        right
            .pattern
            .len()
            .cmp(&left.pattern.len())
            .then_with(|| left.priority.cmp(&right.priority))
    });
    let patterns = literal_rules
        .iter()
        .map(|rule| rule.pattern.as_str())
        .collect::<Vec<_>>();
    let literal_automaton = (!patterns.is_empty())
        .then(|| {
            AhoCorasickBuilder::new()
                .ascii_case_insensitive(true)
                .match_kind(MatchKind::LeftmostFirst)
                .build(patterns)
                .ok()
        })
        .flatten();
    (regex_rules, literal_rules, literal_automaton)
}

fn compile_keyword_rules_with_filter(
    rules: &[ResolvedKeywordHighlightRule],
    include: impl Fn(&[String]) -> bool,
) -> CompiledKeywordRules {
    let mut compiled = Vec::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        let color = parse_hex_rgb(&rule.color).unwrap_or(0x79c0ff);
        let alts = rule
            .patterns
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if alts.is_empty() {
            continue;
        }
        if !include(&alts) {
            continue;
        }
        let priority = compiled.len();
        compile_keyword_regex_alternatives(alts, color, priority, &mut compiled);
    }
    compiled
}

fn compile_keyword_regex_alternatives(
    alts: Vec<String>,
    color: u32,
    priority: usize,
    out: &mut CompiledKeywordRules,
) {
    let combined = if alts.len() == 1 {
        alts[0].clone()
    } else {
        alts.iter()
            .map(|p| format!("(?:{p})"))
            .collect::<Vec<_>>()
            .join("|")
    };
    let pattern = keyword_pattern_with_default_case(&combined);
    match regex::Regex::new(&pattern) {
        Ok(regex) => out.push(CompiledKeywordRule {
            regex,
            color,
            priority,
        }),
        Err(_) => {
            for alt in alts {
                let pattern = keyword_pattern_with_default_case(&alt);
                if let Ok(regex) = regex::Regex::new(&pattern) {
                    let priority = out.len();
                    out.push(CompiledKeywordRule {
                        regex,
                        color,
                        priority,
                    });
                }
            }
        }
    }
}

fn is_literal_keyword_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.chars().any(|ch| {
            matches!(
                ch,
                '\\' | '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        })
}

fn keyword_pattern_with_default_case(pattern: &str) -> String {
    if keyword_pattern_has_inline_case_flag(pattern) {
        pattern.to_string()
    } else {
        format!("(?i){pattern}")
    }
}

fn keyword_pattern_has_inline_case_flag(pattern: &str) -> bool {
    pattern.contains("(?i)") || pattern.contains("(?-i)")
}

pub fn terminal_buffer_matches(
    output: &str,
    query: &str,
    flags: &TerminalSearchFlags,
    limit: usize,
) -> Result<Vec<TerminalBufferMatch>, String> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let mut matches = Vec::new();
    if flags.regex {
        let pattern = if flags.case_sensitive {
            query.to_string()
        } else {
            format!("(?i){query}")
        };
        let regex = regex::Regex::new(&pattern).map_err(|error| error.to_string())?;
        for (line_index, line) in output.lines().enumerate() {
            for found in regex.find_iter(line) {
                if flags.whole_word && !is_whole_word_match(line, found.start(), found.end()) {
                    continue;
                }
                let start_col = terminal_cell_col_for_byte_index(line, found.start());
                let end_col = terminal_cell_col_for_byte_index(line, found.end());
                matches.push(TerminalBufferMatch {
                    line_index,
                    start_col,
                    end_col,
                });
                if matches.len() >= limit {
                    return Ok(matches);
                }
            }
        }
        return Ok(matches);
    }

    let needle = if flags.case_sensitive {
        query.to_string()
    } else {
        query.to_ascii_lowercase()
    };
    for (line_index, line) in output.lines().enumerate() {
        let haystack = if flags.case_sensitive {
            line.to_string()
        } else {
            line.to_ascii_lowercase()
        };
        let mut cursor = 0;
        while cursor <= haystack.len() {
            let Some(relative_start) = haystack[cursor..].find(&needle) else {
                break;
            };
            let start = cursor + relative_start;
            let end = start + needle.len();
            if !flags.whole_word || is_whole_word_match(line, start, end) {
                let start_col = terminal_cell_col_for_byte_index(line, start);
                let end_col = terminal_cell_col_for_byte_index(line, end);
                matches.push(TerminalBufferMatch {
                    line_index,
                    start_col,
                    end_col,
                });
                if matches.len() >= limit {
                    return Ok(matches);
                }
            }
            cursor = end.max(cursor + 1);
        }
    }
    Ok(matches)
}
pub(super) fn is_whole_word_match(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}
pub(super) fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}
pub(super) fn parse_hex_rgb(value: &str) -> Option<u32> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nyaterm_core::ResolvedKeywordHighlightRule;
    use nyaterm_terminal::{
        TerminalScreen, TerminalSnapshot, terminal_char_cell_width, terminal_is_zero_width_mark,
    };

    use super::{
        CompiledKeywordRule, MAX_KEYWORD_WRAPPED_GROUP_ROWS, TerminalKeywordHighlightLookup,
        compile_keyword_rules, compile_terminal_keyword_highlighter,
        keyword_highlight_spans_compiled, keyword_matches_compiled, keyword_matches_highlighter,
        precompute_terminal_keyword_highlights, precompute_terminal_keyword_highlights_for_rows,
        precompute_terminal_keyword_highlights_for_rows_with_stats, terminal_buffer_matches,
        terminal_keyword_row_reuse_keys,
    };
    use crate::element::TerminalSearchFlags;
    use crate::types::TerminalKeywordRange;

    fn set_snapshot_row(
        snapshot: &mut TerminalSnapshot,
        row: usize,
        text: impl Into<String>,
        signature: u64,
    ) {
        let text = text.into();
        let cells = test_render_cells(&text, snapshot.cols);
        let rows = Arc::make_mut(&mut snapshot.row_data);
        let row = Arc::make_mut(&mut rows[row]);
        row.text = text.clone();
        row.styled_spans = vec![nyaterm_terminal::StyledSpan {
            text,
            style: nyaterm_terminal::CellStyle::default(),
        }]
        .into_boxed_slice();
        row.cells = cells.into_boxed_slice();
        row.signature = signature;
        row.revision = signature;
    }

    fn set_snapshot_row_wrapped(
        snapshot: &mut TerminalSnapshot,
        row: usize,
        text: impl Into<String>,
        signature: u64,
        wrapped: bool,
    ) {
        set_snapshot_row(snapshot, row, text, signature);
        let rows = Arc::make_mut(&mut snapshot.row_data);
        Arc::make_mut(&mut rows[row]).wrapped = wrapped;
    }

    fn set_snapshot_row_hyperlink(
        snapshot: &mut TerminalSnapshot,
        row: usize,
        start_col: usize,
        end_col: usize,
        uri: &str,
    ) {
        let rows = Arc::make_mut(&mut snapshot.row_data);
        let row = Arc::make_mut(&mut rows[row]);
        row.hyperlinks = vec![nyaterm_terminal::HyperlinkSpan {
            start_col,
            end_col: end_col.saturating_sub(1),
            uri: uri.to_string(),
        }]
        .into_boxed_slice();
        for col in start_col..end_col.min(row.cells.len()) {
            row.cells[col].hyperlink = Some(Arc::from(uri));
        }
    }

    fn test_render_cells(text: &str, cols: usize) -> Vec<nyaterm_terminal::RenderCell> {
        let mut cells: Vec<nyaterm_terminal::RenderCell> = Vec::new();
        for ch in text.chars() {
            if terminal_is_zero_width_mark(ch)
                && let Some(previous) = cells.last_mut()
            {
                let mut text = previous.text.to_string();
                text.push(ch);
                previous.text = text.into();
                continue;
            }
            let width = terminal_char_cell_width(ch);
            cells.push(nyaterm_terminal::RenderCell {
                text: ch.to_string().into(),
                style: nyaterm_terminal::CellStyle::default(),
                width: width as u8,
                hyperlink: None,
            });
            for _ in 1..width {
                cells.push(nyaterm_terminal::RenderCell {
                    text: Arc::from(""),
                    style: nyaterm_terminal::CellStyle::default(),
                    width: 0,
                    hyperlink: None,
                });
            }
        }
        while cells.len() < cols {
            cells.push(nyaterm_terminal::RenderCell {
                text: Arc::from(""),
                style: nyaterm_terminal::CellStyle::default(),
                width: 1,
                hyperlink: None,
            });
        }
        cells.truncate(cols);
        cells
    }

    fn shifted_display_offset(snapshot: &TerminalSnapshot, amount: usize) -> TerminalSnapshot {
        let mut shifted = snapshot.clone();
        shifted.display_offset = shifted.display_offset.saturating_add(amount);
        shifted
    }

    fn keyword_matches_reference(
        line: &str,
        compiled: &[CompiledKeywordRule],
    ) -> Vec<(usize, usize, u32)> {
        let mut matches = Vec::new();
        let mut cursor = 0;
        while cursor < line.len() {
            let mut best: Option<(usize, usize, u32)> = None;
            for rule in compiled {
                if let Some(found) = rule.regex.find_at(line, cursor) {
                    let start = found.start();
                    let end = found.end();
                    if end <= start {
                        continue;
                    }
                    let replace = best
                        .map(|(best_start, best_end, _)| {
                            start < best_start || (start == best_start && end > best_end)
                        })
                        .unwrap_or(true);
                    if replace {
                        best = Some((start, end, rule.color));
                    }
                }
            }

            let Some((start, end, color)) = best else {
                break;
            };
            matches.push((start, end, color));
            cursor = end;
        }
        matches
    }

    fn compiled_rules(patterns: &[(&str, u32)]) -> Vec<CompiledKeywordRule> {
        patterns
            .iter()
            .enumerate()
            .map(|(priority, (pattern, color))| CompiledKeywordRule {
                regex: regex::Regex::new(pattern).unwrap(),
                color: *color,
                priority,
            })
            .collect()
    }

    #[test]
    fn keyword_highlights_keep_earliest_longest_and_rule_priority() {
        let compiled = compiled_rules(&[("ERR", 1), ("ERROR", 2), ("ERROR", 3)]);

        assert_eq!(
            keyword_matches_compiled("x ERROR ERR", &compiled),
            vec![(2, 7, 2), (8, 11, 1)]
        );

        let spans = keyword_highlight_spans_compiled("x ERROR ERR", &compiled);

        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<Vec<_>>(),
            vec!["x ", "ERROR", " ", "ERR"]
        );
        assert_eq!(spans[1].color, Some(2));
        assert_eq!(spans[3].color, Some(1));
    }

    #[test]
    fn keyword_matches_return_contiguous_matches_for_one_rule() {
        let compiled = compiled_rules(&[("ERROR", 0xff2244)]);

        assert_eq!(
            keyword_matches_compiled("ERROR ERROR ERROR", &compiled),
            vec![(0, 5, 0xff2244), (6, 11, 0xff2244), (12, 17, 0xff2244)]
        );
    }

    #[test]
    fn keyword_matches_choose_longest_match_at_same_start() {
        let compiled = compiled_rules(&[("ERR", 1), ("ERROR", 2)]);

        assert_eq!(
            keyword_matches_compiled("ERROR", &compiled),
            vec![(0, 5, 2)]
        );
    }

    #[test]
    fn keyword_matches_keep_first_rule_for_identical_range() {
        let compiled = compiled_rules(&[("ERROR", 1), ("ERROR", 2)]);

        assert_eq!(
            keyword_matches_compiled("ERROR", &compiled),
            vec![(0, 5, 1)]
        );
    }

    #[test]
    fn keyword_matches_refresh_overlapped_candidate_after_cursor_moves() {
        let compiled = compiled_rules(&[("XXa12", 1), ("a.*?b", 2)]);

        assert_eq!(
            keyword_matches_compiled("XXa12a3b", &compiled),
            vec![(0, 5, 1), (5, 8, 2)]
        );
    }

    #[test]
    fn keyword_matches_preserve_unicode_byte_boundaries() {
        let line = "前ERROR后";
        let compiled = compiled_rules(&[("ERROR", 0xff2244)]);
        let matches = keyword_matches_compiled(line, &compiled);

        assert_eq!(matches, vec![(3, 8, 0xff2244)]);
        for (start, end, _) in matches {
            assert_eq!(&line[start..end], "ERROR");
        }
    }

    #[test]
    fn keyword_matches_skip_zero_length_rules_without_losing_later_matches() {
        let compiled = compiled_rules(&[("^", 1), (r"\b", 2), ("a*", 3), ("ERROR", 4)]);

        assert_eq!(
            keyword_matches_compiled("b ERROR", &compiled),
            vec![(2, 7, 4)]
        );
    }

    #[test]
    fn keyword_highlight_spans_follow_compiled_matches() {
        let line = "pre ERROR mid WARN end";
        let compiled = compiled_rules(&[("ERROR", 0xff2244), ("WARN", 0xffcc00)]);
        let matches = keyword_matches_compiled(line, &compiled);
        let spans = keyword_highlight_spans_compiled(line, &compiled);

        assert_eq!(matches, vec![(4, 9, 0xff2244), (14, 18, 0xffcc00)]);
        assert_eq!(
            spans
                .iter()
                .map(|span| (span.text.as_str(), span.color, span.keyword))
                .collect::<Vec<_>>(),
            vec![
                ("pre ", None, false),
                ("ERROR", Some(0xff2244), true),
                (" mid ", None, false),
                ("WARN", Some(0xffcc00), true),
                (" end", None, false),
            ]
        );
    }

    #[test]
    fn keyword_matches_match_reference_for_non_empty_regexes() {
        let cases = vec![
            ("plain text", vec![("ERROR", 1)]),
            ("ERROR ERROR", vec![("ERROR", 1)]),
            ("abc ERROR xyz WARN", vec![("WARN", 1), ("ERROR", 2)]),
            ("ERROR", vec![("ERR", 1), ("ERROR", 2)]),
            ("ERROR", vec![("ERROR", 1), ("ERROR", 2)]),
            ("abcdef", vec![("abcde", 1), ("cdef", 2), ("def", 3)]),
            ("界ERROR后 WARN", vec![("ERROR", 1), ("WARN", 2)]),
        ];

        for (line, patterns) in cases {
            let compiled = patterns
                .into_iter()
                .enumerate()
                .map(|(priority, (pattern, color))| CompiledKeywordRule {
                    regex: regex::Regex::new(pattern).unwrap(),
                    color,
                    priority,
                })
                .collect::<Vec<_>>();

            assert_eq!(
                keyword_matches_compiled(line, &compiled),
                keyword_matches_reference(line, &compiled),
                "line: {line}"
            );
        }
    }

    #[test]
    fn keyword_highlighter_literal_automaton_preserves_priority_and_longest_match() {
        let rules = vec![
            ResolvedKeywordHighlightRule {
                id: "short".to_string(),
                name: "Short".to_string(),
                patterns: vec!["ERR".to_string()],
                color: "#111111".to_string(),
                enabled: true,
            },
            ResolvedKeywordHighlightRule {
                id: "long".to_string(),
                name: "Long".to_string(),
                patterns: vec!["ERROR".to_string()],
                color: "#222222".to_string(),
                enabled: true,
            },
            ResolvedKeywordHighlightRule {
                id: "regex".to_string(),
                name: "Regex".to_string(),
                patterns: vec![r"WARN\d+".to_string()],
                color: "#333333".to_string(),
                enabled: true,
            },
        ];
        let highlighter = compile_terminal_keyword_highlighter(&rules);

        assert_eq!(
            keyword_matches_highlighter("error WARN42 ERR", &highlighter),
            vec![(0, 5, 0x222222), (6, 12, 0x333333), (13, 16, 0x111111)]
        );
    }

    #[test]
    fn compile_keyword_rules_skips_invalid_patterns_without_dropping_rule() {
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "mixed".to_string(),
            name: "Mixed".to_string(),
            patterns: vec!["ERROR".to_string(), "(".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];
        let compiled = compile_keyword_rules(&rules);

        assert_eq!(compiled.len(), 1);
        assert_eq!(
            keyword_matches_compiled("prefix ERROR suffix", &compiled),
            vec![(7, 12, 0xff2244)]
        );
    }

    #[test]
    #[ignore]
    fn keyword_highlight_benchmark() {
        let rules = nyaterm_core::get_builtin_keyword_rules(true);
        let compiled = compile_keyword_rules(&rules);
        let cases = [
            (
                "no-match",
                benchmark_lines("plain alphabet segment with calm filler ", 200, 200),
            ),
            (
                "sparse",
                benchmark_lines("prefix INFO user=42 ok=true trailing text ", 200, 200),
            ),
            (
                "dense",
                benchmark_lines(
                    "2026-08-01 22:30:01 ERROR user=123 status=500 retry=3 duration=15ms a+b=c ",
                    200,
                    200,
                ),
            ),
            (
                "unicode",
                benchmark_lines(
                    "错误 ERROR 用户=张三 状态=500 界面=a\u{301} 🚀 duration=15ms ",
                    200,
                    200,
                ),
            ),
            (
                "dense-short-token",
                benchmark_lines("E W I 1 2 3 + - = / E W I 4 5 6 ", 200, 200),
            ),
        ];
        let iterations = 10usize;
        println!(
            "case,rows,rules,ranges,iterations,old_match_us,new_match_us,new_span_us,span_count,text_run_count"
        );
        for (case, lines) in cases {
            let mut warmup = 0usize;
            for line in &lines {
                warmup = warmup.saturating_add(keyword_matches_compiled(line, &compiled).len());
            }
            std::hint::black_box(warmup);

            let old_started = std::time::Instant::now();
            let mut old_ranges = 0usize;
            for _ in 0..iterations {
                for line in &lines {
                    old_ranges =
                        old_ranges.saturating_add(keyword_matches_reference(line, &compiled).len());
                }
            }
            let old_duration = old_started.elapsed();

            let new_started = std::time::Instant::now();
            let mut new_ranges = 0usize;
            for _ in 0..iterations {
                for line in &lines {
                    new_ranges =
                        new_ranges.saturating_add(keyword_matches_compiled(line, &compiled).len());
                }
            }
            let new_duration = new_started.elapsed();

            let span_started = std::time::Instant::now();
            let mut span_count = 0usize;
            for _ in 0..iterations {
                for line in &lines {
                    span_count = span_count
                        .saturating_add(keyword_highlight_spans_compiled(line, &compiled).len());
                }
            }
            let span_duration = span_started.elapsed();

            let divisor = iterations.max(1) as u128;
            println!(
                "{case},{},{},{},{iterations},{},{},{},{},{}",
                lines.len(),
                compiled.len(),
                new_ranges / iterations.max(1),
                old_duration.as_micros() / divisor,
                new_duration.as_micros() / divisor,
                span_duration.as_micros() / divisor,
                span_count / iterations.max(1),
                span_count / iterations.max(1),
            );
            std::hint::black_box(old_ranges);
        }
    }

    fn benchmark_lines(seed: &str, rows: usize, cols: usize) -> Vec<String> {
        (0..rows)
            .map(|idx| {
                let mut line = format!("{idx:04} {seed}");
                while line.len() < cols {
                    line.push_str(seed);
                }
                while line.len() > cols {
                    line.pop();
                }
                line
            })
            .collect()
    }

    #[test]
    fn precomputed_keyword_snapshot_checks_row_revisions() {
        let mut snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut snapshot, 0, "prefix ERROR suffix", 41);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        let highlights =
            precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);

        assert!(
            highlights
                .lookup(0, &snapshot)
                .and_then(|row| row.ranges())
                .is_some()
        );
        let mut changed_revision = snapshot.clone();
        {
            let rows = Arc::make_mut(&mut changed_revision.row_data);
            Arc::make_mut(&mut rows[0]).revision = 42;
        }
        assert!(highlights.lookup(0, &changed_revision).is_none());
        assert!(matches!(
            highlights.lookup(0, &snapshot),
            Some(TerminalKeywordHighlightLookup::Current(_))
        ));
        assert!(
            highlights
                .stale_lookup(0, &snapshot)
                .and_then(|row| row.ranges())
                .is_some()
        );
        assert!(highlights.stale_lookup(0, &changed_revision).is_none());
        assert!(highlights.stale_lookup(usize::MAX, &snapshot).is_none());
        assert!(
            highlights
                .stale_lookup(0, &shifted_display_offset(&snapshot, 1))
                .is_none()
        );
        assert!(highlights.matches_snapshot(&snapshot, palette));
        let mut shifted_snapshot = snapshot.clone();
        shifted_snapshot.display_offset = shifted_snapshot.display_offset.saturating_add(1);
        assert!(!highlights.matches_snapshot(&shifted_snapshot, palette));
        assert!(highlights.matches_snapshot(&snapshot, nyaterm_ui::theme_palette("github-light"),));
        assert!(highlights.rows.iter().skip(1).all(Option::is_none));
    }

    #[test]
    fn oversized_wrapped_group_degrades_to_individual_rows() {
        let row_count = MAX_KEYWORD_WRAPPED_GROUP_ROWS + 2;
        let mut screen = TerminalScreen::new(4, row_count as u16);
        screen.advance("a".repeat(row_count * 4).as_bytes());
        let snapshot = screen.snapshot();
        assert!(snapshot.rows().iter().skip(1).all(|row| row.wrapped));
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "cross-row".to_string(),
            name: "Cross row".to_string(),
            patterns: vec!["aa".to_string()],
            color: "#ff0000".to_string(),
            enabled: true,
        }];
        let highlighter = compile_terminal_keyword_highlighter(&rules);

        let (highlights, stats) = precompute_terminal_keyword_highlights_for_rows_with_stats(
            &snapshot,
            &highlighter,
            nyaterm_ui::theme_palette("github-dark"),
            None,
            0..snapshot.row_count(),
        );

        assert!(stats.oversized_wrapped_groups > 0);
        assert!(stats.degraded_rows > 0);
        assert!(stats.processed_bytes > 0);
        assert_eq!(highlights.known_row_count(), snapshot.row_count());
    }

    #[test]
    fn precomputed_keyword_snapshot_highlights_hyperlinked_cells() {
        let mut snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut snapshot, 0, "ERROR ERROR", 7);
        set_snapshot_row_hyperlink(&mut snapshot, 0, 6, 11, "https://example.com");
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "errors".to_string(),
            name: "Errors".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        let highlights =
            precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);
        let ranges = highlights
            .lookup(0, &snapshot)
            .and_then(|row| row.ranges())
            .expect("first ERROR should be highlighted");

        assert_eq!(
            ranges.as_slice(),
            &[
                TerminalKeywordRange {
                    start_col: 0,
                    end_col: 5,
                    color: 0xff2244,
                },
                TerminalKeywordRange {
                    start_col: 6,
                    end_col: 11,
                    color: 0xff2244,
                }
            ]
        );
    }

    #[test]
    fn precomputed_keyword_snapshot_keeps_motd_version_and_hyperlink_url() {
        let mut snapshot = TerminalScreen::default().snapshot();
        let line = "Ubuntu 24.04.4 https://help.ubuntu.com";
        set_snapshot_row(&mut snapshot, 0, line, 7);
        set_snapshot_row_hyperlink(
            &mut snapshot,
            0,
            "Ubuntu 24.04.4 ".len(),
            line.len(),
            "https://help.ubuntu.com",
        );
        let rules = nyaterm_core::get_builtin_keyword_rules(true);

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        let highlights =
            precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);
        let ranges = highlights
            .lookup(0, &snapshot)
            .and_then(|row| row.ranges())
            .expect("version should still be highlighted");

        assert_eq!(
            ranges.as_slice(),
            &[
                TerminalKeywordRange {
                    start_col: "Ubuntu ".len(),
                    end_col: "Ubuntu 24.04.4".len(),
                    color: 0xff9e64,
                },
                TerminalKeywordRange {
                    start_col: "Ubuntu 24.04.4 ".len(),
                    end_col: line.len(),
                    color: 0x8be9fd,
                }
            ]
        );
    }

    #[test]
    fn precomputed_keyword_snapshot_marks_empty_rows_as_known() {
        let mut snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut snapshot, 0, "plain text", 41);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let highlights = precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            nyaterm_ui::theme_palette("github-dark"),
            None,
        );

        let lookup = highlights.lookup(0, &snapshot).expect("row lookup");
        assert!(lookup.is_known_empty());
        assert!(lookup.ranges().is_none());
    }

    #[test]
    fn partial_keyword_snapshot_keeps_unparsed_rows_unknown_and_accumulates() {
        let mut snapshot = TerminalScreen::default().snapshot();
        for (row, signature) in [(0, 41), (1, 42)] {
            set_snapshot_row(&mut snapshot, row, format!("row {row} ERROR"), signature);
        }
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];
        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");

        let first = precompute_terminal_keyword_highlights_for_rows(
            &snapshot,
            &highlighter,
            palette,
            None,
            0..1,
        );
        assert!(first.lookup(0, &snapshot).is_some());
        assert!(first.lookup(1, &snapshot).is_none());
        assert!(first.matches_snapshot_rows(&snapshot, palette, 0..1));
        assert!(!first.matches_snapshot(&snapshot, palette));

        let second = precompute_terminal_keyword_highlights_for_rows(
            &snapshot,
            &highlighter,
            palette,
            Some(&first),
            1..2,
        );
        assert!(second.lookup(0, &snapshot).is_some());
        assert!(second.lookup(1, &snapshot).is_some());
        assert!(second.matches_snapshot_rows(&snapshot, palette, 0..2));
    }

    #[test]
    fn precomputed_keyword_stats_count_reused_rows_and_ranges() {
        let mut snapshot = TerminalScreen::default().snapshot();
        for (row, signature) in [(0, 41), (1, 42)] {
            set_snapshot_row(&mut snapshot, row, format!("row {row} ERROR"), signature);
        }
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];
        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");

        let (first, first_stats) = precompute_terminal_keyword_highlights_for_rows_with_stats(
            &snapshot,
            &highlighter,
            palette,
            None,
            0..1,
        );
        assert_eq!(first_stats.requested_rows, 1);
        assert_eq!(first_stats.known_rows, 1);
        assert_eq!(first_stats.range_count, 1);
        assert_eq!(first_stats.reused_rows, 0);

        let (_second, second_stats) = precompute_terminal_keyword_highlights_for_rows_with_stats(
            &snapshot,
            &highlighter,
            palette,
            Some(&first),
            0..2,
        );
        assert_eq!(second_stats.requested_rows, 2);
        assert_eq!(second_stats.known_rows, 2);
        assert_eq!(second_stats.range_count, 2);
        assert_eq!(second_stats.reused_rows, 1);
    }

    #[test]
    fn precomputed_keyword_snapshot_reuses_matching_rows_after_scroll() {
        let mut snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut snapshot, 0, "prefix ERROR suffix", 41);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let highlights = precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            nyaterm_ui::theme_palette("github-dark"),
            None,
        );

        assert!(
            highlights
                .lookup(0, &snapshot)
                .and_then(|row| row.ranges())
                .is_some()
        );
        let mut scrolled_snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut scrolled_snapshot, 3, "prefix ERROR suffix", 41);
        assert!(
            highlights
                .lookup(3, &scrolled_snapshot)
                .and_then(|row| row.ranges())
                .is_some()
        );
    }

    #[test]
    fn precomputed_keyword_snapshot_reuses_previous_rows_by_revision() {
        let mut first_snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut first_snapshot, 0, "prefix ERROR suffix", 41);
        let mut second_snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut second_snapshot, 5, "prefix ERROR suffix", 41);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        let first_highlights =
            precompute_terminal_keyword_highlights(&first_snapshot, &highlighter, palette, None);
        let first_ranges = first_highlights
            .lookup(0, &first_snapshot)
            .and_then(|row| row.ranges())
            .expect("first ranges")
            .clone();

        let second_highlights = precompute_terminal_keyword_highlights(
            &second_snapshot,
            &highlighter,
            palette,
            Some(&first_highlights),
        );
        let second_ranges = second_highlights
            .lookup(5, &second_snapshot)
            .and_then(|row| row.ranges())
            .expect("second ranges");

        assert!(Arc::ptr_eq(&first_ranges, second_ranges));
    }

    #[test]
    fn keyword_matching_follows_alacritty_soft_wraps_but_stops_at_hard_line_breaks() {
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".into(),
            name: "Error".into(),
            patterns: vec!["ERROR".into()],
            color: "#ff2244".into(),
            enabled: true,
        }];
        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        for (input, expected_cells) in [("ERROR", 5), ("ERR\r\nOR", 0)] {
            let mut screen = TerminalScreen::new(3, 4);
            screen.advance(input.as_bytes());
            let snapshot = screen.snapshot();
            let highlights =
                precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);
            let highlighted_cells: usize = highlights
                .rows
                .iter()
                .flatten()
                .flat_map(|ranges| ranges.iter())
                .map(|range| range.end_col - range.start_col)
                .sum();
            assert_eq!(highlighted_cells, expected_cells, "{input:?}");
        }
    }

    #[test]
    fn keyword_matching_survives_terminal_width_reflow() {
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".into(),
            name: "Error".into(),
            patterns: vec!["ERROR".into()],
            color: "#ff2244".into(),
            enabled: true,
        }];
        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        let mut screen = TerminalScreen::new(12, 4);
        screen.advance(b"ERROR");
        let mut previous = None;
        for columns in [12, 3, 8] {
            screen.resize(columns, 4);
            // Narrowing may move the start of the logical line into scrollback.
            let snapshot = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
            let highlights = precompute_terminal_keyword_highlights(
                &snapshot,
                &highlighter,
                palette,
                previous.as_ref(),
            );
            let highlighted_cells: usize = highlights
                .rows
                .iter()
                .flatten()
                .flat_map(|ranges| ranges.iter())
                .map(|range| range.end_col - range.start_col)
                .sum();
            assert_eq!(
                highlighted_cells,
                5,
                "columns={columns}, rows={:?}",
                snapshot
                    .rows()
                    .iter()
                    .map(|row| (&row.text, row.wrapped))
                    .collect::<Vec<_>>()
            );
            previous = Some(highlights);
        }
    }

    #[test]
    fn precomputed_keyword_snapshot_matches_across_wrapped_rows() {
        let screen = TerminalScreen::new(3, 4);
        let mut snapshot = screen.snapshot();
        set_snapshot_row_wrapped(&mut snapshot, 0, "ERR", 41, false);
        set_snapshot_row_wrapped(&mut snapshot, 1, "OR", 42, true);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let highlights = precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            nyaterm_ui::theme_palette("github-dark"),
            None,
        );

        let first = highlights
            .lookup(0, &snapshot)
            .and_then(|row| row.ranges())
            .expect("first wrapped row ranges");
        let second = highlights
            .lookup(1, &snapshot)
            .and_then(|row| row.ranges())
            .expect("second wrapped row ranges");

        assert_eq!(
            first.as_ref(),
            &[TerminalKeywordRange {
                start_col: 0,
                end_col: 3,
                color: 0xff2244,
            }]
        );
        assert_eq!(
            second.as_ref(),
            &[TerminalKeywordRange {
                start_col: 0,
                end_col: 2,
                color: 0xff2244,
            }]
        );
    }

    #[test]
    fn wrapped_row_reuse_keys_share_group_key_with_row_offsets() {
        let screen = TerminalScreen::new(3, 4);
        let mut snapshot = screen.snapshot();
        set_snapshot_row_wrapped(&mut snapshot, 0, "ERR", 41, false);
        set_snapshot_row_wrapped(&mut snapshot, 1, "OR ", 42, true);
        set_snapshot_row_wrapped(&mut snapshot, 2, "END", 43, true);

        let keys = terminal_keyword_row_reuse_keys(&snapshot);

        assert_eq!(keys.len(), snapshot.row_count());
        let Some(super::TerminalKeywordRowReuseKey::Wrapped {
            group_key: first_group,
            row_offset: 0,
        }) = keys[0]
        else {
            panic!("first row should be wrapped group key");
        };
        assert_eq!(
            keys[1],
            Some(super::TerminalKeywordRowReuseKey::Wrapped {
                group_key: first_group,
                row_offset: 1,
            })
        );
        assert_eq!(
            keys[2],
            Some(super::TerminalKeywordRowReuseKey::Wrapped {
                group_key: first_group,
                row_offset: 2,
            })
        );
    }

    #[test]
    fn precomputed_keyword_snapshot_maps_wide_and_attached_marks_to_cells() {
        let mut snapshot = TerminalScreen::default().snapshot();
        set_snapshot_row(&mut snapshot, 0, "界ERROR", 41);
        set_snapshot_row(&mut snapshot, 1, "e\u{301}ERROR", 42);
        set_snapshot_row(&mut snapshot, 2, "a\u{fe0f}ERROR", 43);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let highlights = precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            nyaterm_ui::theme_palette("github-dark"),
            None,
        );

        let wide = highlights
            .lookup(0, &snapshot)
            .and_then(|row| row.ranges())
            .expect("wide row ranges");
        let combining = highlights
            .lookup(1, &snapshot)
            .and_then(|row| row.ranges())
            .expect("combining row ranges");
        let variation = highlights
            .lookup(2, &snapshot)
            .and_then(|row| row.ranges())
            .expect("variation row ranges");

        assert_eq!(wide[0].start_col, 2);
        assert_eq!(wide[0].end_col, 7);
        assert_eq!(combining[0].start_col, 1);
        assert_eq!(combining[0].end_col, 6);
        assert_eq!(variation[0].start_col, 1);
        assert_eq!(variation[0].end_col, 6);
    }

    #[test]
    fn wrapped_keyword_snapshot_does_not_reuse_plain_row_context() {
        let mut wrapped = TerminalScreen::new(3, 4).snapshot();
        set_snapshot_row_wrapped(&mut wrapped, 0, "ERR", 41, false);
        set_snapshot_row_wrapped(&mut wrapped, 1, "OR", 42, true);
        let mut plain = TerminalScreen::new(3, 4).snapshot();
        set_snapshot_row_wrapped(&mut plain, 0, "ERR", 41, false);
        set_snapshot_row_wrapped(&mut plain, 1, "OR", 42, false);
        let rules = vec![ResolvedKeywordHighlightRule {
            id: "error".to_string(),
            name: "Error".to_string(),
            patterns: vec!["ERROR".to_string()],
            color: "#ff2244".to_string(),
            enabled: true,
        }];

        let highlighter = compile_terminal_keyword_highlighter(&rules);
        let palette = nyaterm_ui::theme_palette("github-dark");
        let wrapped_highlights =
            precompute_terminal_keyword_highlights(&wrapped, &highlighter, palette, None);

        assert!(wrapped_highlights.lookup(0, &plain).is_none());

        let plain_highlights = precompute_terminal_keyword_highlights(
            &plain,
            &highlighter,
            palette,
            Some(&wrapped_highlights),
        );
        assert!(
            plain_highlights
                .lookup(0, &plain)
                .expect("plain row known")
                .ranges()
                .is_none()
        );
    }

    #[test]
    fn terminal_buffer_matches_count_combining_marks_with_previous_cell() {
        let flags = TerminalSearchFlags {
            case_sensitive: true,
            regex: false,
            whole_word: false,
        };

        let matches = terminal_buffer_matches("e\u{301}x", "x", &flags, 10).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 1);
        assert_eq!(matches[0].end_col, 2);
    }

    #[test]
    fn terminal_buffer_matches_count_wide_chars_as_two_cells() {
        let flags = TerminalSearchFlags {
            case_sensitive: true,
            regex: false,
            whole_word: false,
        };

        let matches = terminal_buffer_matches("界x", "x", &flags, 10).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 2);
        assert_eq!(matches[0].end_col, 3);
    }

    #[test]
    fn regex_terminal_buffer_matches_count_combining_marks_with_previous_cell() {
        let flags = TerminalSearchFlags {
            case_sensitive: true,
            regex: true,
            whole_word: false,
        };

        let matches = terminal_buffer_matches("e\u{301}x", "x", &flags, 10).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 1);
        assert_eq!(matches[0].end_col, 2);
    }

    #[test]
    fn regex_terminal_buffer_matches_count_wide_chars_as_two_cells() {
        let flags = TerminalSearchFlags {
            case_sensitive: true,
            regex: true,
            whole_word: false,
        };

        let matches = terminal_buffer_matches("界x", "x", &flags, 10).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 2);
        assert_eq!(matches[0].end_col, 3);
    }
}
