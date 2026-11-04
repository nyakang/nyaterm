//! Streaming removal of terminal control strings for plain-text transcripts.

/// Maximum cursor cells a single `ESC[nD` sequence may erase.
///
/// A hostile stream could request a huge move; the flattening editor only ever
/// pops characters that exist on the pending line, so a bounded emission keeps
/// the sanitized text proportional to the real screen width.
const MAX_CURSOR_BACK_CHARS: usize = 256;

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

#[derive(Default)]
pub struct RecordingSanitizer {
    state: State,
    csi_params: String,
    /// When set, line-editing controls (`\u{8}`, `\r`, `ESC[nD`) survive
    /// sanitization so a [`RecordingLineEditor`] can apply their edit
    /// semantics. Default-constructed sanitizers keep dropping them.
    line_editing: bool,
}

impl RecordingSanitizer {
    pub fn new(line_editing: bool) -> Self {
        Self {
            state: State::Ground,
            csi_params: String::new(),
            line_editing,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.csi_params.clear();
    }

    pub fn feed(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.state {
                State::Ground => match ch {
                    '\x1b' => self.state = State::Escape,
                    '\u{009b}' => {
                        self.csi_params.clear();
                        self.state = State::Csi;
                    }
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
                        }
                    }
                    '\r' => {
                        if self.line_editing {
                            output.push('\r');
                        }
                    }
                    '\u{8}' => {
                        if self.line_editing {
                            output.push('\u{8}');
                        }
                    }
                    '\n' => {
                        output.push('\n');
                    }
                    '\t' => output.push('\t'),
                    ch if ch.is_control() => {}
                    ch => {
                        output.push(ch);
                    }
                },
                State::Escape => {
                    self.state = match ch {
                        '[' => {
                            self.csi_params.clear();
                            State::Csi
                        }
                        ']' => State::String {
                            osc: true,
                            escape: false,
                        },
                        'P' | 'X' | '^' | '_' => State::String {
                            osc: false,
                            escape: false,
                        },
                        '\x1b' => State::Escape,
                        '\x20'..='\x2f' => State::EscapeIntermediate,
                        _ => State::Ground,
                    };
                }
                State::EscapeIntermediate => {
                    if ('\x30'..='\x7e').contains(&ch) {
                        self.state = State::Ground;
                    } else if ch == '\x1b' {
                        self.state = State::Escape;
                    }
                }
                State::Csi => {
                    if ('\x30'..='\x3f').contains(&ch) {
                        self.csi_params.push(ch);
                    } else if ('\x40'..='\x7e').contains(&ch) {
                        self.state = State::Ground;
                        let params = std::mem::take(&mut self.csi_params);
                        if self.line_editing && ch == 'D' {
                            let count = params
                                .split([';', ':'])
                                .next()
                                .and_then(|first| first.parse::<usize>().ok())
                                .unwrap_or(1)
                                .clamp(1, MAX_CURSOR_BACK_CHARS);
                            for _ in 0..count {
                                output.push('\u{8}');
                            }
                        }
                    } else if ch == '\x1b' {
                        self.state = State::Escape;
                    }
                }
                State::String { osc, escape } => {
                    self.state =
                        if (osc && ch == '\x07') || ch == '\u{009c}' || (escape && ch == '\\') {
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
        output
    }
}

/// Applies terminal line-editing semantics to a sanitized transcript stream.
///
/// Terminals echo line edits as backspace/erase sequences (`\u{8} \u{8}`), cursor-left
/// moves, or carriage-return repaints. A transcript that merely drops those
/// control bytes keeps deleted characters in the flattened text. The editor
/// keeps the current (uncommitted) line editable instead: [`Self::feed_char`]
/// commits a line only at its newline, so edits can still retract characters,
/// and [`Self::take_pending`] releases the pending tail at flush points.
///
/// The carriage-return rule approximates a full-line repaint: the pending line
/// restarts when the next printable character arrives, while `\r\n` still
/// commits the already-rendered line unchanged.
#[derive(Debug, Default)]
pub struct RecordingLineEditor {
    line: String,
    overwrite: bool,
}

impl RecordingLineEditor {
    /// Applies one sanitized character. Returns the committed line when `ch`
    /// terminates it (the newline itself is not part of the returned line).
    pub fn feed_char(&mut self, ch: char) -> Option<String> {
        match ch {
            '\u{8}' => {
                self.line.pop();
                None
            }
            '\r' => {
                self.overwrite = true;
                None
            }
            '\n' => {
                self.overwrite = false;
                Some(std::mem::take(&mut self.line))
            }
            ch => {
                if self.overwrite {
                    self.line.clear();
                    self.overwrite = false;
                }
                self.line.push(ch);
                None
            }
        }
    }

    /// Releases the pending (uncommitted) line tail; later edits can no longer
    /// retract it.
    pub fn take_pending(&mut self) -> String {
        self.overwrite = false;
        std::mem::take(&mut self.line)
    }

    /// The pending line that has not been released yet.
    pub fn pending(&self) -> &str {
        &self.line
    }

    pub fn is_empty(&self) -> bool {
        self.line.is_empty()
    }

    pub fn clear(&mut self) {
        self.line.clear();
        self.overwrite = false;
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
    fn editing_sanitizer_keeps_line_editing_controls() {
        let mut sanitizer = RecordingSanitizer::new(true);
        assert_eq!(
            sanitizer.feed("pi\u{8} \u{8}ng\r\x1b[2Dng\r\n"),
            "pi\u{8} \u{8}ng\r\u{8}\u{8}ng\r\n"
        );
    }

    #[test]
    fn editing_sanitizer_limits_cursor_left_moves() {
        let mut sanitizer = RecordingSanitizer::new(true);
        assert_eq!(
            sanitizer.feed("\x1b[999999D\x1b[D"),
            "\u{8}".repeat(256) + "\u{8}"
        );
    }

    #[test]
    fn line_editor_applies_edits_across_chunks() {
        let mut sanitizer = RecordingSanitizer::new(true);
        let mut editor = RecordingLineEditor::default();
        let mut text = String::new();
        for chunk in [
            "PS> pingng",
            "\u{8} \u{8}",
            "\u{8} \u{8}",
            " qq.com",
            "\r\n",
        ] {
            for ch in sanitizer.feed(chunk).chars() {
                if let Some(line) = editor.feed_char(ch) {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
        }
        assert_eq!(editor.pending(), "");
        assert_eq!(text, "PS> ping qq.com\n");
    }

    #[test]
    fn line_editor_restarts_line_after_carriage_return_repaint() {
        let mut editor = RecordingLineEditor::default();
        let mut committed = String::new();
        for ch in " 10%\r 99%\r\n".chars() {
            if let Some(line) = editor.feed_char(ch) {
                committed.push_str(&line);
                committed.push('\n');
            }
        }
        assert_eq!(committed, " 99%\n");
    }

    #[test]
    fn line_editor_keeps_crlf_line_content() {
        let mut editor = RecordingLineEditor::default();
        let mut committed = String::new();
        for ch in "hello\r\nworld\r\n".chars() {
            if let Some(line) = editor.feed_char(ch) {
                committed.push_str(&line);
                committed.push('\n');
            }
        }
        assert_eq!(committed, "hello\nworld\n");
    }
}
