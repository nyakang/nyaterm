//! Timestamp and line-number presentation derived from terminal snapshots.

#[derive(Debug, Clone)]
enum TerminalTimestampToken {
    Year4,
    Year2,
    Month2,
    Month,
    Day2,
    Day,
    Hour2,
    Hour,
    Minute2,
    Minute,
    Second2,
    Second,
    Millis3,
    Millis2,
    Millis1,
    Literal(char),
}

#[derive(Debug, Clone)]
pub struct TerminalTimestampFormatter {
    tokens: Vec<TerminalTimestampToken>,
    width_chars: usize,
    offset: time::UtcOffset,
}

impl TerminalTimestampFormatter {
    pub fn new(format: &str) -> Self {
        let format = normalized_terminal_timestamp_format(format);
        let tokens = parse_terminal_timestamp_tokens(format);
        let width_chars = terminal_timestamp_tokens_width(&tokens);
        Self {
            tokens,
            width_chars,
            offset: time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC),
        }
    }

    pub fn width_chars(&self) -> usize {
        self.width_chars
    }

    pub fn format(&self, timestamp_ms: u64) -> String {
        let secs = (timestamp_ms / 1000) as i64;
        let millis = timestamp_ms % 1000;
        let datetime = time::OffsetDateTime::from_unix_timestamp(secs)
            .ok()
            .map(|datetime| datetime.to_offset(self.offset));
        let Some(datetime) = datetime else {
            return " ".repeat(self.width_chars);
        };

        render_terminal_timestamp_tokens(&self.tokens, datetime, millis)
    }
}

pub fn terminal_timestamp_format_width_chars(format: &str) -> usize {
    let tokens = parse_terminal_timestamp_tokens(normalized_terminal_timestamp_format(format));
    terminal_timestamp_tokens_width(&tokens)
}

fn normalized_terminal_timestamp_format(format: &str) -> &str {
    let trimmed = format.trim();
    if trimmed.is_empty() {
        nyaterm_core::DEFAULT_TERMINAL_TIMESTAMP_FORMAT
    } else {
        trimmed
    }
}

fn parse_terminal_timestamp_tokens(mut input: &str) -> Vec<TerminalTimestampToken> {
    let mut tokens = Vec::new();
    while !input.is_empty() {
        let (token, consumed) = if input.starts_with("YYYY") {
            (TerminalTimestampToken::Year4, 4)
        } else if input.starts_with("SSS") {
            (TerminalTimestampToken::Millis3, 3)
        } else if input.starts_with("YY") {
            (TerminalTimestampToken::Year2, 2)
        } else if input.starts_with("MM") {
            (TerminalTimestampToken::Month2, 2)
        } else if input.starts_with("DD") {
            (TerminalTimestampToken::Day2, 2)
        } else if input.starts_with("HH") {
            (TerminalTimestampToken::Hour2, 2)
        } else if input.starts_with("mm") {
            (TerminalTimestampToken::Minute2, 2)
        } else if input.starts_with("ss") {
            (TerminalTimestampToken::Second2, 2)
        } else if input.starts_with("SS") {
            (TerminalTimestampToken::Millis2, 2)
        } else {
            let ch = input.chars().next().expect("non-empty timestamp format");
            let token = match ch {
                'M' => TerminalTimestampToken::Month,
                'D' => TerminalTimestampToken::Day,
                'H' => TerminalTimestampToken::Hour,
                'm' => TerminalTimestampToken::Minute,
                's' => TerminalTimestampToken::Second,
                'S' => TerminalTimestampToken::Millis1,
                _ => TerminalTimestampToken::Literal(ch),
            };
            (token, ch.len_utf8())
        };
        tokens.push(token);
        input = &input[consumed..];
    }
    tokens
}

fn terminal_timestamp_tokens_width(tokens: &[TerminalTimestampToken]) -> usize {
    tokens
        .iter()
        .map(|token| match token {
            TerminalTimestampToken::Year4 => 4,
            TerminalTimestampToken::Year2
            | TerminalTimestampToken::Month2
            | TerminalTimestampToken::Month
            | TerminalTimestampToken::Day2
            | TerminalTimestampToken::Day
            | TerminalTimestampToken::Hour2
            | TerminalTimestampToken::Hour
            | TerminalTimestampToken::Minute2
            | TerminalTimestampToken::Minute
            | TerminalTimestampToken::Second2
            | TerminalTimestampToken::Second
            | TerminalTimestampToken::Millis2 => 2,
            TerminalTimestampToken::Millis3 => 3,
            TerminalTimestampToken::Millis1 | TerminalTimestampToken::Literal(_) => 1,
        })
        .sum::<usize>()
        .clamp(1, 64)
}

fn render_terminal_timestamp_tokens(
    tokens: &[TerminalTimestampToken],
    datetime: time::OffsetDateTime,
    millis: u64,
) -> String {
    let mut out = String::with_capacity(terminal_timestamp_tokens_width(tokens));
    for token in tokens {
        match token {
            TerminalTimestampToken::Year4 => out.push_str(&format!("{:04}", datetime.year())),
            TerminalTimestampToken::Year2 => {
                out.push_str(&format!("{:02}", datetime.year().rem_euclid(100)))
            }
            TerminalTimestampToken::Month2 => {
                out.push_str(&format!("{:02}", datetime.month() as u8))
            }
            TerminalTimestampToken::Month => out.push_str(&(datetime.month() as u8).to_string()),
            TerminalTimestampToken::Day2 => out.push_str(&format!("{:02}", datetime.day())),
            TerminalTimestampToken::Day => out.push_str(&datetime.day().to_string()),
            TerminalTimestampToken::Hour2 => out.push_str(&format!("{:02}", datetime.hour())),
            TerminalTimestampToken::Hour => out.push_str(&datetime.hour().to_string()),
            TerminalTimestampToken::Minute2 => out.push_str(&format!("{:02}", datetime.minute())),
            TerminalTimestampToken::Minute => out.push_str(&datetime.minute().to_string()),
            TerminalTimestampToken::Second2 => out.push_str(&format!("{:02}", datetime.second())),
            TerminalTimestampToken::Second => out.push_str(&datetime.second().to_string()),
            TerminalTimestampToken::Millis3 => out.push_str(&format!("{millis:03}")),
            TerminalTimestampToken::Millis2 => out.push_str(&format!("{:02}", millis / 10)),
            TerminalTimestampToken::Millis1 => out.push_str(&(millis / 100).to_string()),
            TerminalTimestampToken::Literal(ch) => out.push(*ch),
        }
    }
    out
}

pub struct TerminalGutterLabels {
    pub timestamp: String,
    pub line_number: String,
}

pub fn terminal_gutter_row_count(snapshot: &nyaterm_terminal::TerminalSnapshot) -> usize {
    let row_count = snapshot.row_count();
    let absolute_start = snapshot
        .total_rows
        .saturating_sub(snapshot.display_offset)
        .saturating_sub(row_count);
    // Historical blank lines are output, not unused screen space. Skip them so
    // this scan stays bounded by the live screen even in a large retained window.
    let history_rows = snapshot
        .scrollback_len
        .saturating_sub(absolute_start)
        .min(row_count);
    let cursor_rows = snapshot
        .cursor
        .row
        .checked_add(1)
        .filter(|end| *end <= row_count)
        .unwrap_or(0);
    let retained_rows = history_rows.max(cursor_rows);
    let content_rows = snapshot.rows()[retained_rows..]
        .iter()
        .rposition(|row| !row.text.is_empty() || row.wrapped)
        .map_or(retained_rows, |row| retained_rows + row + 1);
    let image_rows = snapshot
        .images
        .iter()
        .filter(|image| image.height_cells > 0)
        .map(|image| image.row.saturating_add(image.height_cells))
        .max()
        .unwrap_or(0);
    content_rows.max(image_rows).min(row_count)
}

pub fn terminal_gutter_labels(
    row: Option<&nyaterm_terminal::TerminalSnapshotRow>,
    absolute_line_number: usize,
    show_timestamps: bool,
    show_line_numbers: bool,
    line_number_digits: usize,
    timestamp_formatter: &TerminalTimestampFormatter,
) -> TerminalGutterLabels {
    let wrapped = row.is_some_and(|row| row.wrapped);
    let timestamp = if show_timestamps {
        row.and_then(|row| row.timestamp_ms)
            .map(|timestamp| timestamp_formatter.format(timestamp))
            .unwrap_or_else(|| " ".repeat(timestamp_formatter.width_chars()))
    } else {
        String::new()
    };
    let line_number = if !show_line_numbers || row.is_some_and(|row| row.cleared) {
        String::new()
    } else if wrapped {
        format!("{:>line_number_digits$}", "-")
    } else {
        let absolute_line_number = row
            .and_then(|row| row.display_line_number)
            .unwrap_or(absolute_line_number);
        format!("{absolute_line_number:>line_number_digits$}")
    };
    TerminalGutterLabels {
        timestamp,
        line_number,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalTimestampFormatter, terminal_gutter_labels, terminal_gutter_row_count,
        terminal_timestamp_format_width_chars,
    };
    use nyaterm_terminal::TerminalScreen;

    #[test]
    fn timestamp_tokens_preserve_fixed_offset_defaults_and_width_limits() {
        let mut formatter = TerminalTimestampFormatter::new("YYYY-MM-DD HH:mm:ss.SSS");
        formatter.offset = time::UtcOffset::from_hms(8, 0, 0).unwrap();
        assert_eq!(formatter.format(123), "1970-01-01 08:00:00.123");
        assert_eq!(
            terminal_timestamp_format_width_chars("  "),
            terminal_timestamp_format_width_chars(nyaterm_core::DEFAULT_TERMINAL_TIMESTAMP_FORMAT)
        );
        assert_eq!(terminal_timestamp_format_width_chars(&"x".repeat(80)), 64);
    }

    #[test]
    fn gutter_omits_metadata_for_locally_cleared_rows() {
        let mut terminal = TerminalScreen::new(20, 4);
        terminal.advance(b"\x1b[4;1Hroot# draft");
        terminal.clear_except_input();
        let snapshot = terminal.snapshot();
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss");
        for row in 0..3 {
            let labels =
                terminal_gutter_labels(snapshot.row(row), row + 1, true, true, 1, &formatter);
            assert!(labels.timestamp.trim().is_empty());
            assert!(labels.line_number.is_empty());
        }
        let labels = terminal_gutter_labels(snapshot.row(3), 4, true, true, 1, &formatter);
        assert_eq!(labels.line_number, "1");
        assert!(!labels.timestamp.trim().is_empty());
    }

    #[test]
    fn gutter_omits_unused_screen_rows_even_when_they_have_timestamps() {
        let mut terminal = TerminalScreen::new(10, 6);
        terminal.advance(b"first\r\n\r\n# ");
        let snapshot = terminal.snapshot();

        assert!(snapshot.row(5).unwrap().text.is_empty());
        assert!(snapshot.row(5).unwrap().timestamp_ms.is_some());
        assert_eq!(snapshot.cursor.row, 2);
        assert!(snapshot.row(1).unwrap().text.is_empty());
        assert_eq!(terminal_gutter_row_count(&snapshot), 3);
    }

    #[test]
    fn gutter_keeps_blank_cursor_row_even_when_cursor_paint_is_disabled() {
        let mut terminal = TerminalScreen::new(10, 6);
        terminal.advance(b"first\r\n\r\n\x1b[?25l");
        let snapshot = terminal.snapshot();

        assert!(!snapshot.cursor.visible);
        assert!(snapshot.row(2).unwrap().text.is_empty());
        assert_eq!(terminal_gutter_row_count(&snapshot), 3);
    }

    #[test]
    fn gutter_keeps_output_below_cursor_and_trims_resized_or_cleared_screen_padding() {
        let mut terminal = TerminalScreen::new(10, 6);
        terminal.advance(b"first\r\n\r\n\x1b]133;A\x07last\x1b]133;B\x07\x1b[H");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.cursor.row, 0);
        assert_eq!(terminal_gutter_row_count(&snapshot), 3);

        terminal.resize(10, 12);
        assert_eq!(terminal_gutter_row_count(&terminal.snapshot()), 3);

        terminal.advance(b"\x1b[2J\x1b[H");
        assert_eq!(terminal_gutter_row_count(&terminal.snapshot()), 1);
    }

    #[test]
    fn gutter_keeps_soft_wrapped_output_and_omits_rows_after_it() {
        let mut terminal = TerminalScreen::new(4, 6);
        terminal.advance(b"abcdefghijkl");
        let snapshot = terminal.snapshot();

        assert!(snapshot.row(2).unwrap().wrapped);
        assert_eq!(terminal_gutter_row_count(&snapshot), 3);
    }

    #[test]
    fn gutter_preserves_blank_history_in_large_prefetched_windows() {
        let mut terminal = TerminalScreen::new(10, 6);
        for _ in 0..1_000 {
            terminal.advance(b"\r\n");
        }
        let history = terminal.viewport_snapshot(terminal.scrollback_len());
        assert_eq!(history.cursor.row, usize::MAX);
        assert!(history.rows().iter().all(|row| row.text.is_empty()));
        assert_eq!(terminal_gutter_row_count(&history), history.row_count());

        terminal.advance(b"\x1b[2J\x1b[H# ");
        let prefetched = terminal.viewport_snapshot_with_window(0, terminal.scrollback_len(), 0);
        assert!(prefetched.row_count() > 900);
        assert_eq!(
            terminal_gutter_row_count(&prefetched),
            prefetched.cursor.row + 1
        );
        let scrolled = terminal.viewport_snapshot(1);
        assert_eq!(scrolled.cursor.row, usize::MAX);
        assert_eq!(terminal_gutter_row_count(&scrolled), 2);
    }

    #[test]
    fn gutter_keeps_rows_occupied_by_inline_images_without_text() {
        let mut terminal = TerminalScreen::new(10, 6);
        terminal.advance(b"\x1b[4;1H\x1b_Ga=T,i=9,c=1,r=2;QUI=\x1b\\\x1b[H");
        let snapshot = terminal.snapshot();

        assert_eq!(snapshot.images.len(), 1);
        assert!(snapshot.rows().iter().all(|row| row.text.is_empty()));
        assert_eq!(terminal_gutter_row_count(&snapshot), 5);
    }

    #[test]
    fn terminal_timestamp_milliseconds_follow_format_tokens_only() {
        let seconds = TerminalTimestampFormatter::new("HH:mm:ss");
        let millis = TerminalTimestampFormatter::new("HH:mm:ss.SSS");

        assert_eq!(seconds.width_chars(), 8);
        assert_eq!(millis.width_chars(), 12);
        assert!(!seconds.format(123).ends_with(".123"));
        assert!(millis.format(123).ends_with(".123"));
        assert_eq!(terminal_timestamp_format_width_chars("H:m:s.S"), 10);
    }

    #[test]
    fn gutter_labels_are_independent_of_cursor_and_reserve_missing_timestamp_width() {
        let mut terminal = TerminalScreen::new(10, 3);
        terminal.advance(b"first\r\nsecond\r\nthird\x1b[2A");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.cursor.row, 0);
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss.SSS");

        let labels = terminal_gutter_labels(snapshot.row(2), 3, true, true, 2, &formatter);

        assert_eq!(labels.line_number, " 3");
        assert_eq!(labels.timestamp.len(), formatter.width_chars());
    }

    #[test]
    fn soft_wrapped_output_shows_timestamps_and_right_aligned_continuation_dashes() {
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss");

        for output in [b"abcdefgh".as_slice(), b"abcdefghijkl".as_slice()] {
            let row_count = output.len() / 4;
            let mut terminal = TerminalScreen::new(4, row_count as u16);
            terminal.advance(output);
            let snapshot = terminal.snapshot();

            for row_index in 0..row_count {
                let row = snapshot.row(row_index).expect("output row");
                assert_eq!(row.wrapped, row_index > 0);
                let timestamp = row.timestamp_ms.expect("output timestamp");
                let labels =
                    terminal_gutter_labels(Some(row), 37 + row_index, true, true, 3, &formatter);

                assert_eq!(labels.timestamp, formatter.format(timestamp));
                assert_eq!(
                    labels.line_number,
                    if row_index == 0 { "  1" } else { "  -" }
                );
            }
        }
    }

    #[test]
    fn continuation_dashes_do_not_consume_gutter_line_numbers() {
        let mut terminal = TerminalScreen::new(4, 4);
        terminal.advance(b"abcdefghijkl\r\nnext");
        let snapshot = terminal.snapshot();
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss");
        let labels = snapshot
            .rows()
            .iter()
            .enumerate()
            .map(|(index, row)| {
                terminal_gutter_labels(Some(row), index + 1, false, true, 1, &formatter).line_number
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, ["1", "-", "-", "2"]);
    }

    #[test]
    fn wrapped_gutter_labels_use_each_rows_existing_timestamp() {
        let mut terminal = TerminalScreen::new(4, 3);
        terminal.advance(b"abcdefghijkl");
        let snapshot = terminal.snapshot();
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss.SSS");

        for (row_index, timestamp) in [123, 123, 1_123].into_iter().enumerate() {
            let mut row = snapshot.row(row_index).expect("output row").clone();
            row.timestamp_ms = Some(timestamp);
            let labels =
                terminal_gutter_labels(Some(&row), row_index + 1, true, true, 1, &formatter);

            assert_eq!(labels.timestamp, formatter.format(timestamp));
            assert_eq!(labels.line_number, if row_index == 0 { "1" } else { "-" });
        }
    }

    #[test]
    fn wrapped_gutter_labels_respect_independent_switches_and_missing_timestamps() {
        let mut terminal = TerminalScreen::new(4, 2);
        terminal.advance(b"abcdefgh");
        let snapshot = terminal.snapshot();
        let mut row = snapshot.row(1).expect("wrapped row").clone();
        assert!(row.wrapped);
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss.SSS");

        for timestamp in [None, Some(123)] {
            row.timestamp_ms = timestamp;
            for show_timestamps in [false, true] {
                for show_line_numbers in [false, true] {
                    for snapshot_row in [None, snapshot.row(0), Some(&row)] {
                        let labels = terminal_gutter_labels(
                            snapshot_row,
                            2,
                            show_timestamps,
                            show_line_numbers,
                            3,
                            &formatter,
                        );
                        let expected_timestamp = if show_timestamps {
                            snapshot_row
                                .and_then(|row| row.timestamp_ms)
                                .map(|timestamp| formatter.format(timestamp))
                                .unwrap_or_else(|| " ".repeat(formatter.width_chars()))
                        } else {
                            String::new()
                        };

                        assert_eq!(labels.timestamp, expected_timestamp);
                        assert_eq!(
                            labels.line_number,
                            if !show_line_numbers {
                                ""
                            } else if snapshot_row.is_some_and(|row| row.wrapped) {
                                "  -"
                            } else if snapshot_row.is_some() {
                                "  1"
                            } else {
                                "  2"
                            }
                        );
                    }
                }
            }
        }
    }
}
