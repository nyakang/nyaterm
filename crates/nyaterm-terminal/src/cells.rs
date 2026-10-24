use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTextCell {
    pub text: String,
    pub byte_start: usize,
    pub byte_end: usize,
}

pub fn terminal_is_zero_width_mark(ch: char) -> bool {
    if UnicodeWidthChar::width(ch) == Some(0) {
        return true;
    }
    matches!(
        ch as u32,
        0x0300..=0x036f
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x20d0..=0x20ff
            | 0xfe20..=0xfe2f
    )
}

pub fn terminal_char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
}

/// Base characters that render as two cells once U+FE0F (VS16) follows them.
///
/// `unicode-width` reports these as narrow text glyphs, so `ℹ️`, `❤️` and
/// friends were counted as one cell while the renderer drew two and the next
/// glyph overlapped (issue #946). Only characters that default to text
/// presentation need this list; emoji-presentation characters are already
/// two cells wide.
pub fn terminal_is_vs16_emoji_base(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2139
            | 0x2600..=0x2604
            | 0x260e
            | 0x2611
            | 0x2614..=0x2615
            | 0x2618
            | 0x2620
            | 0x2622..=0x2623
            | 0x2626
            | 0x262a
            | 0x262e..=0x262f
            | 0x2638..=0x263a
            | 0x2640
            | 0x2642
            | 0x2648..=0x2653
            | 0x265f
            | 0x2660
            | 0x2663
            | 0x2665..=0x2666
            | 0x2668
            | 0x267b
            | 0x267e..=0x267f
            | 0x2692..=0x2697
            | 0x2699
            | 0x269b..=0x269c
            | 0x26a0..=0x26a1
            | 0x26a7
            | 0x26aa..=0x26ab
            | 0x26b0..=0x26b1
            | 0x26bd
            | 0x26c8
            | 0x26ce..=0x26cf
            | 0x26d1
            | 0x26d3..=0x26d4
            | 0x26e9..=0x26ea
            | 0x26f0..=0x26f5
            | 0x26f7..=0x26fa
            | 0x26fd
            | 0x2702
            | 0x2705
            | 0x2708..=0x270d
            | 0x270f
            | 0x2712
            | 0x2714
            | 0x2716
            | 0x271d
            | 0x2721
            | 0x2728
            | 0x2733..=0x2734
            | 0x2744
            | 0x2747
            | 0x274c
            | 0x274e
            | 0x2753..=0x2755
            | 0x2757
            | 0x2763..=0x2764
            | 0x2795..=0x2797
            | 0x27a1
            | 0x27b0
            | 0x27bf
            | 0x2934..=0x2935
            | 0x2b05..=0x2b07
            | 0x2b1b..=0x2b1c
            | 0x2b50
            | 0x2b55
            | 0x3030
            | 0x303d
            | 0x3297
            | 0x3299
    )
}

/// True when the previous character should widen from one to two cells
/// because a VS16 follows it.
pub fn terminal_vs16_widens(previous: char) -> bool {
    terminal_is_vs16_emoji_base(previous) && terminal_char_cell_width(previous) == 1
}

pub fn terminal_cell_count(text: &str) -> usize {
    let mut cells = 0usize;
    let mut previous: Option<char> = None;
    for ch in text.chars() {
        // A VS16 after a text-presentation emoji base widens it to two cells.
        if ch == '\u{fe0f}' && cells > 0 {
            if previous.is_some_and(terminal_vs16_widens) {
                cells += 1;
            }
            continue;
        }
        if terminal_is_zero_width_mark(ch) && cells > 0 {
            continue;
        }
        cells += terminal_char_cell_width(ch);
        previous = Some(ch);
    }
    cells
}

pub fn terminal_cell_col_for_byte_index(text: &str, byte_index: usize) -> usize {
    let byte_index = byte_index.min(text.len());
    let mut cells = 0usize;
    let mut previous: Option<char> = None;
    for (idx, ch) in text.char_indices() {
        if idx >= byte_index {
            break;
        }
        if ch == '\u{fe0f}' && cells > 0 {
            if previous.is_some_and(terminal_vs16_widens) {
                cells += 1;
            }
            continue;
        }
        if terminal_is_zero_width_mark(ch) && cells > 0 {
            continue;
        }
        cells += terminal_char_cell_width(ch);
        previous = Some(ch);
    }
    cells
}

pub fn terminal_byte_index_for_cell_col(text: &str, cell_col: usize) -> usize {
    if cell_col == 0 {
        return 0;
    }
    let mut cells = 0usize;
    let mut previous: Option<char> = None;
    for (idx, ch) in text.char_indices() {
        if ch == '\u{fe0f}' && cells > 0 {
            if previous.is_some_and(terminal_vs16_widens) {
                // The widened second cell belongs to the glyph plus VS16.
                cells += 1;
                if cell_col < cells {
                    return idx + ch.len_utf8();
                }
            }
            continue;
        }
        if !terminal_is_zero_width_mark(ch) || cells == 0 {
            let width = terminal_char_cell_width(ch);
            if cell_col < cells.saturating_add(width) {
                return idx;
            }
            cells += width;
            previous = Some(ch);
        }
    }
    text.len()
}

pub fn terminal_text_cells(text: &str) -> Vec<TerminalTextCell> {
    let mut cells: Vec<TerminalTextCell> = Vec::new();
    let mut previous: Option<char> = None;
    for (byte_start, ch) in text.char_indices() {
        let byte_end = byte_start + ch.len_utf8();
        // VS16 attaches to the preceding base and, for a text-presentation
        // emoji base, adds the widened second cell.
        if ch == '\u{fe0f}' && !cells.is_empty() {
            let previous_start = cells.last().map(|cell| cell.byte_start);
            if let Some(previous_start) = previous_start {
                for cell in cells
                    .iter_mut()
                    .rev()
                    .take_while(|cell| cell.byte_start == previous_start)
                {
                    cell.text.push(ch);
                    cell.byte_end = byte_end;
                }
            }
            if previous.is_some_and(terminal_vs16_widens)
                && let Some(last) = cells.last().cloned()
            {
                cells.push(last);
            }
            continue;
        }
        if terminal_is_zero_width_mark(ch)
            && let Some(previous) = cells.last_mut()
        {
            let previous_start = previous.byte_start;
            for cell in cells
                .iter_mut()
                .rev()
                .take_while(|cell| cell.byte_start == previous_start)
            {
                cell.text.push(ch);
                cell.byte_end = byte_end;
            }
            continue;
        }
        let cell = TerminalTextCell {
            text: ch.to_string(),
            byte_start,
            byte_end,
        };
        for _ in 0..terminal_cell_count(&cell.text).max(1) {
            cells.push(cell.clone());
        }
        previous = Some(ch);
    }
    cells
}

pub fn terminal_text_cell_slice(cells: &[TerminalTextCell], start: usize, end: usize) -> String {
    let Some(slice) = cells.get(start..end) else {
        return String::new();
    };
    let mut out = String::new();
    let mut last_range = None;
    for cell in slice {
        let range = (cell.byte_start, cell.byte_end);
        if last_range == Some(range) {
            continue;
        }
        out.push_str(&cell.text);
        last_range = Some(range);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        terminal_byte_index_for_cell_col, terminal_cell_col_for_byte_index, terminal_cell_count,
        terminal_is_zero_width_mark, terminal_text_cell_slice, terminal_text_cells,
    };

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
    fn terminal_cell_count_widens_vs16_emoji_base_to_two_cells() {
        // U+2139 U+FE0F (information) and U+2764 U+FE0F (heart) default to
        // text presentation but render as two cells with VS16 (issue #946).
        assert_eq!(terminal_cell_count("\u{2139}\u{fe0f}"), 2);
        assert_eq!(terminal_cell_count("\u{2764}\u{fe0f}"), 2);
        assert_eq!(terminal_cell_count("\u{2139}\u{fe0f} da"), 5);
        // Plain letters keep the old behaviour.
        assert_eq!(terminal_cell_count("a\u{fe0f}"), 1);
    }

    #[test]
    fn terminal_text_cells_keeps_vs16_emoji_two_cells() {
        let cells = terminal_text_cells("\u{2139}\u{fe0f}x");
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].text, "\u{2139}\u{fe0f}");
        assert_eq!(cells[1].text, "\u{2139}\u{fe0f}");
        assert_eq!(cells[2].text, "x");
    }

    #[test]
    fn byte_and_cell_columns_account_for_vs16_emoji() {
        let text = "\u{2139}\u{fe0f}x";
        assert_eq!(terminal_cell_col_for_byte_index(text, text.len()), 3);
        assert_eq!(
            terminal_byte_index_for_cell_col(text, 2),
            "\u{2139}\u{fe0f}".len()
        );
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
    fn terminal_text_cell_slice_deduplicates_wide_and_combining_cells() {
        let cells = terminal_text_cells("a界e\u{301}z");

        assert_eq!(terminal_text_cell_slice(&cells, 0, 1), "a");
        assert_eq!(terminal_text_cell_slice(&cells, 1, 3), "界");
        assert_eq!(terminal_text_cell_slice(&cells, 3, 4), "e\u{301}");
        assert_eq!(terminal_text_cell_slice(&cells, 4, 5), "z");
    }
}
