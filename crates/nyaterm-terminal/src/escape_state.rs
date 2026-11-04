//! Classify the parser escape phase without parsing the grid twice.
//!
//! A slice of terminal input can be stamped narrowly only when it cannot move
//! the cursor or rewrite rows above the bottom line. [`EscapeState`] mirrors
//! the parser's escape phase across slice boundaries and reports, per slice,
//! whether its sequences are harmless:
//!
//! * SGR (`CSI ... m`) only sets the pen of cells written afterwards;
//! * OSC payloads (title, hyperlink, palette, shell marks) never reach the
//!   grid.
//!
//! Everything else is treated as risky - cursor motion, erase, scroll, mode
//! changes, DCS/APC - so the caller keeps its full-screen stamp. A sequence
//! that starts in one slice and ends in the next is classified by the slice
//! that contains its final byte, and the following slice starts out of ground
//! state, which also forces the full stamp.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    /// OSC payload. `ESC` may start its string terminator.
    Osc,
    OscEscape,
    /// DCS / SOS / PM / APC payload; these can drive graphics or queries.
    Str,
    StrEscape,
}

#[derive(Debug, Default)]
pub(crate) struct EscapeState {
    state: State,
    /// Continuation bytes still expected by a valid UTF-8 lead byte. Text is
    /// UTF-8 by now (session encodings are converted first), so a byte in
    /// `0x80..=0x9f` inside a valid sequence is a glyph byte, not a C1
    /// control. The parser executes C1 only for standalone invalid bytes.
    utf8_continuations: u8,
    risky: bool,
}

impl EscapeState {
    pub(crate) fn is_ground(&self) -> bool {
        self.state == State::Ground
    }

    /// Advance one slice and report whether nothing in it can move the cursor
    /// or write above the bottom line.
    pub(crate) fn advance_classified(&mut self, bytes: &[u8]) -> bool {
        self.risky = false;
        let mut state = self.state;
        for &byte in bytes {
            state = match state {
                State::Ground => {
                    if self.utf8_continuations > 0 {
                        if (0x80..=0xbf).contains(&byte) {
                            self.utf8_continuations -= 1;
                            State::Ground
                        } else {
                            // The partial sequence was invalid; the byte
                            // starts over as ground state content.
                            self.utf8_continuations = 0;
                            ground(byte, &mut self.risky, &mut self.utf8_continuations)
                        }
                    } else {
                        ground(byte, &mut self.risky, &mut self.utf8_continuations)
                    }
                }
                State::Escape => match byte {
                    b'[' => State::Csi,
                    b']' => State::Osc,
                    // ESC \ is the string terminator.
                    b'\\' => State::Ground,
                    b'P' | b'X' | b'^' | b'_' => {
                        self.risky = true;
                        State::Str
                    }
                    // Charset selection, keypad modes, save/restore, RIS and
                    // any other final byte: cannot be proven harmless.
                    0x00..=0x2f => {
                        self.risky = true;
                        State::Escape
                    }
                    _ => {
                        self.risky = true;
                        State::Ground
                    }
                },
                State::Csi => match byte {
                    // An aborting ESC has no grid effect; the sequence it
                    // starts is classified on its own.
                    0x1b => State::Escape,
                    0x40..=0x7e => {
                        if byte != b'm' {
                            self.risky = true;
                        }
                        State::Ground
                    }
                    _ => State::Csi,
                },
                State::Osc => match byte {
                    0x07 | 0x9c => State::Ground,
                    0x1b => State::OscEscape,
                    _ => State::Osc,
                },
                State::OscEscape => match byte {
                    b'\\' | 0x9c => State::Ground,
                    _ => State::Osc,
                },
                State::Str => match byte {
                    0x9c => State::Ground,
                    0x1b => State::StrEscape,
                    _ => State::Str,
                },
                State::StrEscape => match byte {
                    b'\\' => State::Ground,
                    _ => State::Str,
                },
            };
        }
        self.state = state;
        !self.risky
    }

    pub(crate) fn reset(&mut self) {
        self.state = State::Ground;
        self.utf8_continuations = 0;
        self.risky = false;
    }
}

/// Ground-state byte classification, including UTF-8 lead/tail tracking.
///
/// C0 controls only move the cursor down, left or within its row, so they stay
/// inside the bottom-row proof. C1 controls are unsafe; the parser executes
/// them only for invalid UTF-8, so a plain `0x80..=0x9f` byte inside a valid
/// sequence is ordinary text.
fn ground(byte: u8, risky: &mut bool, utf8_continuations: &mut u8) -> State {
    match byte {
        0x1b => State::Escape,
        // Valid UTF-8 leads; the tail bytes are ordinary glyph content.
        0xc2..=0xdf => {
            *utf8_continuations = 1;
            State::Ground
        }
        0xe0..=0xef => {
            *utf8_continuations = 2;
            State::Ground
        }
        0xf0..=0xf4 => {
            *utf8_continuations = 3;
            State::Ground
        }
        // Standalone invalid bytes in this range reach the parser as C1.
        0x9b => {
            *risky = true;
            State::Csi
        }
        0x90 | 0x98 | 0x9d | 0x9e | 0x9f => {
            *risky = true;
            State::Str
        }
        0x80..=0x9f => {
            *risky = true;
            State::Ground
        }
        // C0, DEL and invalid leads the parser renders as U+FFFD.
        _ => State::Ground,
    }
}
