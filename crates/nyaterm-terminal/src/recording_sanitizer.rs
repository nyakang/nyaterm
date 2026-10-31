//! Streaming removal of terminal control strings for plain-text transcripts.

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

/// Upper bound on the backspaces one `CSI n D` may expand to.
const MAX_CURSOR_BACK: u32 = 4096;

#[derive(Default)]
pub struct RecordingSanitizer {
    state: State,
    cursor_back: bool,
    csi_count: Option<u32>,
    csi_plain: bool,
}

impl RecordingSanitizer {
    /// Also emit one `\x08` per column for backspace and `CSI n D`, so a
    /// line-oriented consumer can apply in-line redraws (pager prompts such as
    /// `---- More ----`, or edited echo) instead of keeping the erased text.
    pub fn with_cursor_back() -> Self {
        Self {
            cursor_back: true,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        *self = Self {
            cursor_back: self.cursor_back,
            ..Self::default()
        };
    }

    fn enter_csi(&mut self) {
        self.state = State::Csi;
        self.csi_count = None;
        self.csi_plain = true;
    }

    pub fn feed(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.state {
                State::Ground => match ch {
                    '\x1b' => self.state = State::Escape,
                    '\u{009b}' => self.enter_csi(),
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
                    '\r' => {}
                    '\x08' if self.cursor_back => output.push('\x08'),
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
                    if ch == '[' {
                        self.enter_csi();
                        continue;
                    }
                    self.state = match ch {
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
                    if let Some(digit) = ch.to_digit(10) {
                        self.csi_count = Some(
                            self.csi_count
                                .unwrap_or(0)
                                .saturating_mul(10)
                                .saturating_add(digit),
                        );
                    } else if ('\x40'..='\x7e').contains(&ch) {
                        if ch == 'D' && self.cursor_back && self.csi_plain {
                            let count = self.csi_count.unwrap_or(1).clamp(1, MAX_CURSOR_BACK);
                            output.extend(std::iter::repeat_n('\x08', count as usize));
                        }
                        self.state = State::Ground;
                    } else if ch == '\x1b' {
                        self.state = State::Escape;
                    } else {
                        // Parameter separators or private markers: not a plain `CSI n D`.
                        self.csi_plain = false;
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

#[cfg(test)]
mod tests {
    use crate::recording_sanitizer::RecordingSanitizer;

    #[test]
    fn cursor_back_is_only_reported_when_requested() {
        let input = "ab\x08c\x1b[2Dd\x1b[Ce\x1b[1;2De\x1b[?5D";
        assert_eq!(RecordingSanitizer::default().feed(input), "abcdee");
        let mut sanitizer = RecordingSanitizer::with_cursor_back();
        assert_eq!(sanitizer.feed(input), "ab\x08c\x08\x08dee");
        sanitizer.reset();
        assert_eq!(sanitizer.feed("x\x1b[D"), "x\x08", "reset keeps the mode");
        assert_eq!(
            RecordingSanitizer::with_cursor_back()
                .feed("\x1b[99999999D")
                .len(),
            4096
        );
    }

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
}
