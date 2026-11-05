//! Streaming terminal controls and single-row editing for plain-text transcripts.

use unicode_width::UnicodeWidthChar;

const MAX_CSI_PARAMS: usize = 32;
/// Transcripts have no viewport geometry. Bound virtual cursor gaps independently
/// of the caller's line-fragment budget.
const MAX_CURSOR_COLUMN: usize = 64 * 1024;

#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    String {
        osc: bool,
        escape: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingEvent {
    Character(char),
    Backspace,
    CarriageReturn,
    Newline,
    Tab,
    CursorLeft(usize),
    CursorRight(usize),
    CursorColumn(usize),
    EraseLine(usize),
    DeleteCharacters(usize),
    InsertCharacters(usize),
}

#[derive(Default)]
pub struct RecordingSanitizer {
    state: State,
    csi_params: [u8; MAX_CSI_PARAMS],
    csi_len: usize,
    csi_invalid: bool,
}

impl RecordingSanitizer {
    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.csi_len = 0;
        self.csi_invalid = false;
    }

    /// Compatibility API: remove controls without applying cursor edits.
    pub fn feed(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        self.feed_events(text, |event| match event {
            RecordingEvent::Character(ch) => output.push(ch),
            RecordingEvent::Newline => output.push('\n'),
            RecordingEvent::Tab => output.push('\t'),
            _ => {}
        });
        output
    }

    /// Emits edits directly, avoiding a temporary string and per-CSI allocations.
    pub fn feed_events(&mut self, text: &str, mut emit: impl FnMut(RecordingEvent)) {
        for ch in text.chars() {
            match self.state {
                State::Ground => match ch {
                    '\x1b' => self.state = State::Escape,
                    '\u{009b}' => self.begin_csi(),
                    '\u{009d}' => {
                        self.state = State::String {
                            osc: true,
                            escape: false,
                        }
                    }
                    '\u{0090}' | '\u{0098}' | '\u{009e}' | '\u{009f}' => {
                        self.state = State::String {
                            osc: false,
                            escape: false,
                        };
                    }
                    '\r' => emit(RecordingEvent::CarriageReturn),
                    '\u{8}' => emit(RecordingEvent::Backspace),
                    '\n' => emit(RecordingEvent::Newline),
                    '\t' => emit(RecordingEvent::Tab),
                    ch if !ch.is_control() => emit(RecordingEvent::Character(ch)),
                    _ => {}
                },
                State::Escape => match ch {
                    '[' => self.begin_csi(),
                    ']' => {
                        self.state = State::String {
                            osc: true,
                            escape: false,
                        }
                    }
                    'P' | 'X' | '^' | '_' => {
                        self.state = State::String {
                            osc: false,
                            escape: false,
                        }
                    }
                    '\x1b' => self.state = State::Escape,
                    '\x20'..='\x2f' => self.state = State::EscapeIntermediate,
                    _ => self.state = State::Ground,
                },
                State::EscapeIntermediate => {
                    if ('\x30'..='\x7e').contains(&ch) {
                        self.state = State::Ground;
                    } else if ch == '\x1b' {
                        self.state = State::Escape;
                    }
                }
                State::Csi => {
                    if ('\x30'..='\x3f').contains(&ch) {
                        if self.csi_len < MAX_CSI_PARAMS {
                            self.csi_params[self.csi_len] = ch as u8;
                            self.csi_len += 1;
                        } else {
                            self.csi_invalid = true;
                        }
                    } else if ('\x40'..='\x7e').contains(&ch) {
                        self.state = State::Ground;
                        if let Some(event) = self.csi_event(ch) {
                            emit(event);
                        }
                    } else if ch == '\x1b' {
                        self.state = State::Escape;
                    } else if ch == '\x18' || ch == '\x1a' {
                        self.state = State::Ground;
                    } else {
                        // Intermediate/private/malformed controls must not turn
                        // into an ordinary cursor edit by ignoring their bytes.
                        self.csi_invalid = true;
                    }
                }
                State::String { osc, escape } => {
                    self.state =
                        if ch == '\u{009c}' || (osc && ch == '\x07') || (escape && ch == '\\') {
                            State::Ground
                        } else {
                            State::String {
                                osc,
                                escape: ch == '\x1b',
                            }
                        };
                }
            }
        }
    }

    fn begin_csi(&mut self) {
        self.csi_len = 0;
        self.csi_invalid = false;
        self.state = State::Csi;
    }

    fn csi_event(&self, final_char: char) -> Option<RecordingEvent> {
        if self.csi_invalid || !matches!(final_char, 'D' | 'C' | 'G' | '`' | 'K' | 'P' | '@') {
            return None;
        }
        let mut count = 0usize;
        for &byte in &self.csi_params[..self.csi_len] {
            if !byte.is_ascii_digit() {
                return None;
            }
            count = count
                .saturating_mul(10)
                .saturating_add((byte - b'0') as usize);
        }
        count = count.min(MAX_CURSOR_COLUMN);
        let movement = count.max(1);
        Some(match final_char {
            'D' => RecordingEvent::CursorLeft(movement),
            'C' => RecordingEvent::CursorRight(movement),
            'G' | '`' => RecordingEvent::CursorColumn(movement - 1),
            'K' => RecordingEvent::EraseLine(count),
            'P' => RecordingEvent::DeleteCharacters(movement),
            '@' => RecordingEvent::InsertCharacters(movement),
            _ => unreachable!(),
        })
    }
}

/// Keeps one editable row. Cursor moves do not erase text; printing overwrites
/// cells, CR returns to column zero, and CSI K explicitly erases. This is a
/// transcript row model, not a viewport emulator (vertical moves are ignored).
#[derive(Default)]
pub struct RecordingLineEditor {
    line: String,
    cursor: usize,
    width: usize,
}

impl RecordingLineEditor {
    pub fn feed(&mut self, event: RecordingEvent) -> Option<String> {
        match event {
            RecordingEvent::Tab => self.cursor = ((self.cursor / 8 + 1) * 8).min(MAX_CURSOR_COLUMN),
            RecordingEvent::Backspace => self.cursor = self.cursor.saturating_sub(1),
            RecordingEvent::CarriageReturn => self.cursor = 0,
            RecordingEvent::Newline => return Some(self.take_pending()),
            RecordingEvent::CursorLeft(count) => self.cursor = self.cursor.saturating_sub(count),
            RecordingEvent::CursorRight(count) => {
                self.cursor = self.cursor.saturating_add(count).min(MAX_CURSOR_COLUMN)
            }
            RecordingEvent::CursorColumn(column) => self.cursor = column.min(MAX_CURSOR_COLUMN),
            RecordingEvent::Character(ch) => self.write_character(ch),
            RecordingEvent::EraseLine(0) => {
                let (start, column) = self.index_at(self.cursor);
                self.line.truncate(start);
                self.line.extend(std::iter::repeat_n(
                    ' ',
                    self.cursor.min(self.width) - column,
                ));
                self.width = self.cursor.min(self.width);
            }
            RecordingEvent::EraseLine(1) => {
                let end_column = self.cursor.saturating_add(1).min(self.width);
                let (mut end, column) = self.index_at(end_column);
                let mut erased = end_column;
                if column < end_column
                    && let Some(ch) = self.line[end..].chars().next()
                {
                    end += ch.len_utf8();
                    erased = column + ch.width().unwrap_or(0);
                }
                self.line.replace_range(..end, &" ".repeat(erased));
            }
            RecordingEvent::EraseLine(2) => {
                self.line.clear();
                self.width = 0;
            }
            RecordingEvent::DeleteCharacters(count) => {
                self.normalize_cell_at(self.cursor);
                self.normalize_cell_at(self.cursor.saturating_add(count));
                let (start, column) = self.index_at(self.cursor);
                let (end, end_column) = self.index_at(self.cursor.saturating_add(count));
                self.line.replace_range(start..end, "");
                self.width -= end_column - column;
            }
            RecordingEvent::InsertCharacters(count) => {
                self.normalize_cell_at(self.cursor);
                let count = count.min(MAX_CURSOR_COLUMN.saturating_sub(self.width));
                let (start, _) = self.index_at(self.cursor);
                self.line.insert_str(start, &" ".repeat(count));
                self.width += count;
            }
            RecordingEvent::EraseLine(_) => {}
        }
        None
    }

    fn index_at(&self, target: usize) -> (usize, usize) {
        if target >= self.width {
            return (self.line.len(), self.width);
        }
        let mut column = 0;
        for (index, ch) in self.line.char_indices() {
            let width = ch.width().unwrap_or(0);
            if column + width > target {
                return (index, column);
            }
            column += width;
        }
        (self.line.len(), self.width)
    }

    // Editing either half of a wide glyph first clears that glyph to blanks,
    // preserving the other cells' columns before insertion or deletion.
    fn normalize_cell_at(&mut self, target: usize) {
        let (index, column) = self.index_at(target);
        if column < target
            && let Some(ch) = self.line[index..].chars().next()
        {
            let width = ch.width().unwrap_or(0);
            self.line
                .replace_range(index..index + ch.len_utf8(), &" ".repeat(width));
        }
    }

    fn write_character(&mut self, ch: char) {
        let width = ch.width().unwrap_or(0);
        // The append path is O(1) even for very long output lines.
        if self.cursor >= self.width {
            self.line
                .extend(std::iter::repeat_n(' ', self.cursor - self.width));
            self.line.push(ch);
            self.width = self.cursor + width;
        } else {
            let (start, start_column) = self.index_at(self.cursor);
            let target = self.cursor + width;
            let (mut end, mut end_column) = self.index_at(target);
            if end_column < target
                && let Some(existing) = self.line[end..].chars().next()
            {
                end += existing.len_utf8();
                end_column += existing.width().unwrap_or(0);
            }
            let replacement = format!(
                "{}{}{}",
                " ".repeat(self.cursor - start_column),
                ch,
                " ".repeat(end_column.saturating_sub(target))
            );
            self.line.replace_range(start..end, &replacement);
            self.width = self.width.max(target);
        }
        self.cursor = self.cursor.saturating_add(width).min(MAX_CURSOR_COLUMN);
    }

    /// Flush points reset row coordinates. Later edits cannot retract released
    /// fragments. Blanks erased at the right edge are omitted from transcripts.
    pub fn take_pending(&mut self) -> String {
        self.cursor = 0;
        self.width = 0;
        let mut line = std::mem::take(&mut self.line);
        line.truncate(line.trim_end_matches(' ').len());
        line
    }

    pub fn pending(&self) -> &str {
        &self.line
    }
    pub fn is_empty(&self) -> bool {
        self.line.is_empty()
    }
    pub fn clear(&mut self) {
        self.line.clear();
        self.cursor = 0;
        self.width = 0;
    }
}

#[cfg(test)]
mod tests {
    use crate::recording_sanitizer::{RecordingLineEditor, RecordingSanitizer};

    #[test]
    fn control_strings_survive_every_utf8_chunk_boundary() {
        let input = "\x1b[31m红\x1b[0m\x1b]0;hidden\x1b\\\x1bPpayload\x1b\\\x1b(0\u{009b}31m\u{009d}hidden\u{009c}\r\nvisible";
        for boundary in 0..=input.len() {
            if !input.is_char_boundary(boundary) {
                continue;
            }
            let mut sanitizer = RecordingSanitizer::default();
            let mut output = sanitizer.feed(&input[..boundary]);
            output.push_str(&sanitizer.feed(&input[boundary..]));
            assert_eq!(output, "红\nvisible", "boundary {boundary}");
        }
    }

    #[test]
    fn default_sanitizer_drops_line_editing_controls() {
        let mut sanitizer = RecordingSanitizer::default();
        assert_eq!(sanitizer.feed("pi\u{8} \u{8}ng\r\n"), "pi ng\n");
    }

    #[test]
    fn line_editor_applies_edits_across_chunks() {
        let mut sanitizer = RecordingSanitizer::default();
        let mut editor = RecordingLineEditor::default();
        let mut text = String::new();
        for chunk in [
            "PS> pingng",
            "\u{8} \u{8}",
            "\u{8} \u{8}",
            " qq.com",
            "\r\n",
        ] {
            sanitizer.feed_events(chunk, |event| {
                if let Some(line) = editor.feed(event) {
                    text.push_str(&line);
                    text.push('\n');
                }
            });
        }
        assert_eq!(editor.pending(), "");
        assert_eq!(text, "PS> ping qq.com\n");
    }

    fn edited(input: &str) -> String {
        let mut sanitizer = RecordingSanitizer::default();
        let mut editor = RecordingLineEditor::default();
        let mut result = String::new();
        sanitizer.feed_events(input, |event| {
            if let Some(line) = editor.feed(event) {
                result.push_str(&line);
                result.push('\n');
            }
        });
        result.push_str(editor.pending());
        result
    }

    #[test]
    fn cursor_movement_preserves_cells_and_printing_overwrites_them() {
        for (input, expected) in [
            ("abcde\x1b[2D\n", "abcde\n"),
            ("hello\x08\n", "hello\n"),
            ("abcde\x1b[2DXY\n", "abcXY\n"),
            ("abcdef\rXY\n", "XYcdef\n"),
            ("abcdef\rXY\x1b[K\n", "XY\n"),
            ("abcdef\r\x1b[2KXY\n", "XY\n"),
            ("abc\r\x1b[2CX\n", "abX\n"),
            ("abc\x1b[2GX\n", "aXc\n"),
            ("abc\x1b[2G\x1b[P\n", "ac\n"),
            ("abc\x1b[2G\x1b[@X\n", "aXbc\n"),
            ("abc\x1b[2G\x1b[1K\n", "  c\n"),
            ("a\x1b[2CX\n", "a  X\n"),
            ("界ab\x1b[2DX\n", "界Xb\n"),
            ("界ab\rX\n", "X ab\n"),
            ("界ab\x1b[2G\x1b[P\n", " ab\n"),
            ("a\tX\n", "a       X\n"),
            ("hello\r\nworld\r\n", "hello\nworld\n"),
            (" 10%\r 99%\r\n", " 99%\n"),
        ] {
            assert_eq!(edited(input), expected, "{input:?}");
            for boundary in 0..=input.len() {
                if !input.is_char_boundary(boundary) {
                    continue;
                }
                let mut sanitizer = RecordingSanitizer::default();
                let mut editor = RecordingLineEditor::default();
                let mut actual = String::new();
                for chunk in [&input[..boundary], &input[boundary..]] {
                    sanitizer.feed_events(chunk, |event| {
                        if let Some(line) = editor.feed(event) {
                            actual.push_str(&line);
                            actual.push('\n');
                        }
                    });
                }
                actual.push_str(editor.pending());
                assert_eq!(actual, expected, "boundary {boundary}: {input:?}");
            }
        }
    }

    #[test]
    fn csi_parameters_are_bounded_and_invalid_edits_are_ignored() {
        let mut sanitizer = RecordingSanitizer::default();
        sanitizer.feed_events("\x1b[", |_| unreachable!());
        for _ in 0..4096 {
            sanitizer.feed_events("1234567890", |_| unreachable!());
            assert!(sanitizer.csi_len <= super::MAX_CSI_PARAMS);
        }
        let mut events = Vec::new();
        sanitizer.feed_events(
            "Dvisible\x1b[?2D\x1b[2 D\x1b[2;3D\x1b[999999999999999999999999D",
            |event| events.push(event),
        );
        assert_eq!(
            events.last(),
            Some(&super::RecordingEvent::CursorLeft(super::MAX_CURSOR_COLUMN))
        );
        assert_eq!(events.len(), "visible".len() + 1);
        sanitizer.reset();
        assert_eq!(sanitizer.feed("ok"), "ok");
    }
}
