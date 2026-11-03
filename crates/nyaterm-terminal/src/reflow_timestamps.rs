use std::collections::HashMap;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;

use crate::{NyaTermEventProxy, ScreenLineState, render_cell_width};

struct RowTimestamp {
    byte_start: usize,
    metadata: ReflowRowMetadata,
}

#[derive(Clone, Copy)]
pub(crate) struct ReflowRowMetadata {
    pub timestamp_ms: Option<u64>,
    pub cleared: bool,
    pub display_line_number: Option<usize>,
    pub logical_line_number: Option<usize>,
}

pub(crate) struct ReflowTimestamps {
    lines: HashMap<String, Vec<CapturedLine>>,
}

struct CapturedLine {
    first_line: Line,
    rows: Vec<RowTimestamp>,
}

struct TextLine {
    text: String,
    rows: Vec<(Line, usize)>,
}

// Match complete hard lines, keeping spaces inside soft wraps and skipping
// wide-character spacers. Only trailing padding at a hard break is ignored.
// Reflow can discard the oldest history, so match duplicate lines newest first.
fn text_lines(term: &Term<NyaTermEventProxy>) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let mut current = TextLine {
        text: String::new(),
        rows: Vec::new(),
    };
    let cols = term.columns();
    for index in term.topmost_line().0..=term.bottommost_line().0 {
        let line = Line(index);
        current.rows.push((line, current.text.len()));
        for col in 0..cols {
            let cell = &term.grid()[line][Column(col)];
            if render_cell_width(cell) == 0 {
                continue;
            }
            current.text.push(cell.c);
            if let Some(marks) = cell.zerowidth() {
                current.text.extend(marks.iter().copied());
            }
        }
        if !term.grid()[line][Column(cols - 1)]
            .flags
            .contains(Flags::WRAPLINE)
            || line == term.bottommost_line()
        {
            current
                .text
                .truncate(current.text.trim_end_matches(' ').len());
            lines.push(current);
            current = TextLine {
                text: String::new(),
                rows: Vec::new(),
            };
        }
    }
    lines
}

pub(crate) fn capture(term: &Term<NyaTermEventProxy>, state: &ScreenLineState) -> ReflowTimestamps {
    let mut lines: HashMap<String, Vec<CapturedLine>> = HashMap::new();
    for line in text_lines(term) {
        let first_line = line.rows[0].0;
        let timestamps = line
            .rows
            .into_iter()
            .map(|(line, byte_start)| {
                let logical = state.logical_line(line);
                let metadata = state.metadata.get(&logical);
                RowTimestamp {
                    byte_start,
                    metadata: ReflowRowMetadata {
                        timestamp_ms: metadata.and_then(|metadata| metadata.timestamp_ms),
                        cleared: metadata.is_some_and(|metadata| metadata.cleared),
                        display_line_number: state
                            .clear_line_origin
                            .and_then(|origin| usize::try_from(logical - origin).ok())
                            .map(|line| line.saturating_add(1)),
                        logical_line_number: (!metadata.is_some_and(|metadata| metadata.cleared))
                            .then(|| {
                                state
                                    .line_numbers
                                    .number(logical, state.logical_line(term.topmost_line()))
                            })
                            .flatten(),
                    },
                }
            })
            .collect();
        lines.entry(line.text).or_default().push(CapturedLine {
            first_line,
            rows: timestamps,
        });
    }
    ReflowTimestamps { lines }
}

pub(crate) fn restore(
    term: &Term<NyaTermEventProxy>,
    mut timestamps: ReflowTimestamps,
) -> Vec<(Line, ReflowRowMetadata)> {
    let mut restored = Vec::new();
    for line in text_lines(term).into_iter().rev() {
        let mut old = timestamps.lines.get_mut(&line.text).and_then(Vec::pop);
        let mut byte_offset = 0;
        if old.is_none() && line.rows[0].0 == term.topmost_line() && !line.text.is_empty() {
            // Shrinking may evict the first rows of a soft-wrapped hard line.
            // Its retained suffix still belongs to the original output.
            let key = timestamps
                .lines
                .iter()
                .filter(|(text, rows)| !rows.is_empty() && text.ends_with(&line.text))
                .max_by_key(|(_, rows)| rows.last().map(|row| row.first_line))
                .map(|(text, _)| text.clone());
            if let Some(key) = key {
                byte_offset = key.len() - line.text.len();
                old = timestamps.lines.get_mut(&key).and_then(Vec::pop);
            }
        }
        let Some(old) = old else {
            continue;
        };
        for (line, byte_start) in line.rows {
            // A new row inherits the time of the old row containing its first
            // character. Resizing never counts as newly received output.
            let index = old
                .rows
                .partition_point(|row| row.byte_start <= byte_start + byte_offset)
                .saturating_sub(1);
            let metadata = old.rows[index].metadata;
            if metadata.timestamp_ms.is_some()
                || metadata.cleared
                || metadata.display_line_number.is_some()
                || metadata.logical_line_number.is_some()
            {
                restored.push((line, metadata));
            }
        }
    }
    restored
}
