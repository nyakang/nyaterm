//! Terminal input classification for `send-keys` injection.
//!
//! In control mode the channel only accepts commands, so pane input is
//! injected through `send-keys`. Writing the *local* terminal's sequences
//! verbatim (`send-keys -H`) leaves the pane to decode them with its own
//! TERM — e.g. xterm's End is `ESC [ F` while `screen`/`tmux` define it as
//! `ESC [ 4 ~`, so vim printed a stray `F` (`H` for Home). A regular tmux
//! client translates keys between the two terminfo tables on its input
//! side; naming the key (`send-keys End`) makes tmux do the same job,
//! including application-cursor and modified xterm forms.

/// One span of classified terminal input.
#[derive(Debug, PartialEq)]
pub(crate) enum InputSegment {
    /// Bytes delivered verbatim via `send-keys -H`.
    Literal(Vec<u8>),
    /// A `send-keys` key name such as `End` or `C-Left`.
    Key(String),
}

/// Per-pane classification state, retained across input flushes.
#[derive(Default)]
pub(crate) struct InputParser {
    in_paste: bool,
    /// Prefix of the expected paste marker already forwarded literally.
    paste_marker_len: usize,
}

impl InputParser {
    /// Split input into literal runs and named keys, preserving order.
    ///
    /// Incomplete sequences are forwarded immediately, so a lone Escape
    /// never waits for more input. Paste markers are tracked byte by byte
    /// even across flushes; their bytes and all paste payload stay literal.
    pub(crate) fn split_input(&mut self, data: &[u8]) -> Vec<InputSegment> {
        let mut segments = Vec::new();
        let mut literal_start = 0;
        let mut i = 0;
        while i < data.len() {
            if !self.in_paste
                && data[i] == 0x1b
                && let Some((name, len)) = key_at(&data[i..])
            {
                if literal_start < i {
                    segments.push(InputSegment::Literal(data[literal_start..i].to_vec()));
                }
                segments.push(InputSegment::Key(name));
                self.paste_marker_len = 0;
                i += len;
                literal_start = i;
                continue;
            }
            self.track_paste_marker(data[i]);
            i += 1;
        }
        if literal_start < data.len() {
            segments.push(InputSegment::Literal(data[literal_start..].to_vec()));
        }
        segments
    }

    fn track_paste_marker(&mut self, byte: u8) {
        let marker = if self.in_paste {
            b"\x1b[201~"
        } else {
            b"\x1b[200~"
        };
        if byte == marker[self.paste_marker_len] {
            self.paste_marker_len += 1;
            if self.paste_marker_len == marker.len() {
                self.in_paste = !self.in_paste;
                self.paste_marker_len = 0;
            }
        } else {
            // ESC can restart a marker after a mismatched partial prefix.
            self.paste_marker_len = usize::from(byte == 0x1b);
        }
    }
}

/// If `data` (starting with `ESC`) opens with a complete special-key
/// sequence, return its `send-keys` name and byte length.
fn key_at(data: &[u8]) -> Option<(String, usize)> {
    match *data.get(1)? {
        b'O' => {
            // SS3: F1-F4 and application-mode cursor keys.
            let name = match *data.get(2)? {
                b'A' => "Up",
                b'B' => "Down",
                b'C' => "Right",
                b'D' => "Left",
                b'H' => "Home",
                b'F' => "End",
                b'P' => "F1",
                b'Q' => "F2",
                b'R' => "F3",
                b'S' => "F4",
                _ => return None,
            };
            Some((name.to_string(), 3))
        }
        b'[' => {
            // CSI: parameter/intermediate bytes are 0x20..=0x3f, the first
            // byte in 0x40..=0x7e terminates the sequence.
            let end = data
                .iter()
                .skip(2)
                .position(|b| !(0x20..=0x3f).contains(b))
                .map(|i| i + 2)?;
            let final_byte = data[end];
            if !(0x40..=0x7e).contains(&final_byte) {
                return None;
            }
            let params = &data[2..end];
            // Only plain `N` or `N;M` numeric forms are keys; private
            // markers stay literal (bracketed paste `200`/`201`, SGR mouse
            // `<`, kitty `u` finals are already excluded by their final).
            if !params.iter().all(|b| b.is_ascii_digit() || *b == b';') {
                return None;
            }
            let parse_param = |field: &[u8]| {
                std::str::from_utf8(field)
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
            };
            let mut fields = params.split(|b| *b == b';');
            let first_field = fields.next()?;
            let first = if params.is_empty() {
                None
            } else {
                Some(parse_param(first_field)?)
            };
            let second = match fields.next() {
                Some(field) => Some(parse_param(field)?),
                None => None,
            };
            if fields.next().is_some() {
                return None;
            }
            let prefix = modifier_prefix(second)?;
            let name = match final_byte {
                b'Z' if params.is_empty() => Some("BTab".to_string()),
                // Modified CSI F3 is indistinguishable from a cursor
                // position report (e.g. CSI 1;5 R). Preserve the report;
                // unambiguous SS3 F3 and CSI 13[;M]~ still map normally.
                b'R' if !params.is_empty() => None,
                letter if letter.is_ascii_alphabetic() && first.unwrap_or(1) == 1 => {
                    csi_letter_key(letter).map(|base| format!("{prefix}{base}"))
                }
                b'~' => csi_tilde_key(first?).map(|base| format!("{prefix}{base}")),
                _ => None,
            }?;
            Some((name, end + 1))
        }
        _ => None,
    }
}

/// `C-M-S-` modifier prefixes for xterm modifier parameters (`2` = Shift,
/// `3` = Meta, `5` = Ctrl; bitfield `param - 1`).
fn modifier_prefix(param: Option<u32>) -> Option<&'static str> {
    Some(match param {
        None | Some(1) => "",
        Some(2) => "S-",
        Some(3) => "M-",
        Some(4) => "M-S-",
        Some(5) => "C-",
        Some(6) => "C-S-",
        Some(7) => "C-M-",
        Some(8) => "C-M-S-",
        _ => return None,
    })
}

/// `send-keys` name for `ESC [ <first> <letter>` sequences (xterm cursor
/// and SS3-over-CSI function keys; modifiers ride in the second parameter).
fn csi_letter_key(final_byte: u8) -> Option<&'static str> {
    match final_byte {
        b'A' => Some("Up"),
        b'B' => Some("Down"),
        b'C' => Some("Right"),
        b'D' => Some("Left"),
        b'H' => Some("Home"),
        b'F' => Some("End"),
        b'P' => Some("F1"),
        b'Q' => Some("F2"),
        b'R' => Some("F3"),
        b'S' => Some("F4"),
        _ => None,
    }
}

/// `send-keys` name for `ESC [ <number> ~` sequences, or `None` for non-key
/// payloads (bracketed paste markers `200`/`201`, ...).
fn csi_tilde_key(number: u32) -> Option<&'static str> {
    match number {
        1 | 7 => Some("Home"),
        2 => Some("IC"),
        3 => Some("DC"),
        4 | 8 => Some("End"),
        5 => Some("PPage"),
        6 => Some("NPage"),
        11 => Some("F1"),
        12 => Some("F2"),
        13 => Some("F3"),
        14 => Some("F4"),
        15 => Some("F5"),
        17 => Some("F6"),
        18 => Some("F7"),
        19 => Some("F8"),
        20 => Some("F9"),
        21 => Some("F10"),
        23 => Some("F11"),
        24 => Some("F12"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{InputParser, InputSegment};

    fn split_input(data: &[u8]) -> Vec<InputSegment> {
        InputParser::default().split_input(data)
    }

    fn lit(text: &str) -> InputSegment {
        InputSegment::Literal(text.as_bytes().to_vec())
    }

    fn key(name: &str) -> InputSegment {
        InputSegment::Key(name.to_string())
    }

    #[test]
    fn plain_text_stays_literal() {
        assert_eq!(split_input(b"ls -la\r"), vec![lit("ls -la\r")]);
    }

    #[test]
    fn home_and_end_map_to_names() {
        assert_eq!(split_input(b"\x1b[F"), vec![key("End")]);
        assert_eq!(split_input(b"\x1b[H"), vec![key("Home")]);
        assert_eq!(split_input(b"\x1bOF"), vec![key("End")]);
        assert_eq!(split_input(b"\x1bOH"), vec![key("Home")]);
        assert_eq!(split_input(b"\x1b[1~"), vec![key("Home")]);
        assert_eq!(split_input(b"\x1b[4~"), vec![key("End")]);
        assert_eq!(split_input(b"\x1b[7~"), vec![key("Home")]);
        assert_eq!(split_input(b"\x1b[8~"), vec![key("End")]);
    }

    #[test]
    fn arrows_and_function_keys_map() {
        assert_eq!(split_input(b"\x1b[A"), vec![key("Up")]);
        assert_eq!(split_input(b"\x1bOD"), vec![key("Left")]);
        assert_eq!(split_input(b"\x1b[1;5C"), vec![key("C-Right")]);
        assert_eq!(split_input(b"\x1b[1;3D"), vec![key("M-Left")]);
        assert_eq!(split_input(b"\x1b[1;8B"), vec![key("C-M-S-Down")]);
        assert_eq!(split_input(b"\x1bOP"), vec![key("F1")]);
        assert_eq!(split_input(b"\x1b[1;2Q"), vec![key("S-F2")]);
        assert_eq!(split_input(b"\x1b[15~"), vec![key("F5")]);
        assert_eq!(split_input(b"\x1b[24~"), vec![key("F12")]);
        assert_eq!(split_input(b"\x1b[Z"), vec![key("BTab")]);
    }

    #[test]
    fn editing_keys_map() {
        assert_eq!(split_input(b"\x1b[2~"), vec![key("IC")]);
        assert_eq!(split_input(b"\x1b[3~"), vec![key("DC")]);
        assert_eq!(split_input(b"\x1b[3;5~"), vec![key("C-DC")]);
        assert_eq!(split_input(b"\x1b[5~"), vec![key("PPage")]);
        assert_eq!(split_input(b"\x1b[6~"), vec![key("NPage")]);
    }

    #[test]
    fn non_key_sequences_stay_literal() {
        // Bracketed paste markers, focus reports, SGR mouse input, kitty
        // CSI-u keys, Alt+char — none of these are tmux key names.
        for data in [
            "\x1b[200~pasted\x1b[201~",
            "\x1b[I",
            "\x1b[O",
            "\x1b[<64;12;30M",
            "\x1b[97;5u",
            "\x1bb",
            "\x1b",
            "\x1b[1;",
        ] {
            assert_eq!(
                split_input(data.as_bytes()),
                vec![lit(data)],
                "{data:?} must pass through verbatim"
            );
        }
    }

    #[test]
    fn bracketed_paste_payload_stays_literal() {
        // Payload copied from a terminal may itself contain key-shaped
        // sequences; inside the markers none of them classify as keys.
        let data = b"\x1b[200~end \x1b[F home \x1b[H\x1b[201~";
        assert_eq!(
            split_input(data),
            vec![lit("\x1b[200~end \x1b[F home \x1b[H\x1b[201~")]
        );
        // Keys after the closing marker classify again.
        assert_eq!(
            split_input(b"\x1b[200~x\x1b[201~\x1b[F"),
            vec![lit("\x1b[200~x\x1b[201~"), key("End")]
        );
        // An unclosed marker keeps the remainder literal, matching the
        // pre-patch `send-keys -H` behavior for truncated input.
        assert_eq!(
            split_input(b"\x1b[200~x\x1b[F"),
            vec![lit("\x1b[200~x\x1b[F")]
        );
    }

    #[test]
    fn bracketed_paste_survives_flushes() {
        let mut parser = InputParser::default();
        assert_eq!(
            parser.split_input(b"\x1b[200~hello"),
            vec![lit("\x1b[200~hello")]
        );
        assert!(parser.split_input(b"").is_empty());
        assert_eq!(
            parser.split_input(b"world \x1b[F\x1b[201~\x1b[F"),
            vec![lit("world \x1b[F\x1b[201~"), key("End")]
        );
    }

    #[test]
    fn paste_markers_survive_every_flush_boundary() {
        let start = b"\x1b[200~";
        let end = b"\x1b[201~";
        for start_split in 1..start.len() {
            for end_split in 1..end.len() {
                let mut parser = InputParser::default();
                assert_eq!(
                    parser.split_input(&start[..start_split]),
                    vec![InputSegment::Literal(start[..start_split].to_vec())]
                );
                let middle = [
                    &start[start_split..],
                    b"hello\x1b[F\x1b[H",
                    &end[..end_split],
                ]
                .concat();
                assert_eq!(
                    parser.split_input(&middle),
                    vec![InputSegment::Literal(middle)]
                );
                let tail = [&end[end_split..], b"\x1b[F"].concat();
                assert_eq!(
                    parser.split_input(&tail),
                    vec![InputSegment::Literal(end[end_split..].to_vec()), key("End")]
                );
            }
        }
    }

    #[test]
    fn paste_markers_can_arrive_one_byte_per_flush() {
        let mut parser = InputParser::default();
        for byte in b"\x1b[200~" {
            assert_eq!(
                parser.split_input(&[*byte]),
                vec![InputSegment::Literal(vec![*byte])]
            );
        }
        assert_eq!(parser.split_input(b"\x1b[F"), vec![lit("\x1b[F")]);
        for byte in b"\x1b[201~" {
            assert_eq!(
                parser.split_input(&[*byte]),
                vec![InputSegment::Literal(vec![*byte])]
            );
        }
        assert_eq!(parser.split_input(b"\x1b[F"), vec![key("End")]);
    }

    #[test]
    fn paste_state_is_independent_per_pane() {
        let mut first = InputParser::default();
        let mut second = InputParser::default();
        assert_eq!(first.split_input(b"\x1b[200~"), vec![lit("\x1b[200~")]);
        assert_eq!(second.split_input(b"\x1b[F"), vec![key("End")]);
        assert_eq!(second.split_input(b"\x1b[20"), vec![lit("\x1b[20")]);
        assert_eq!(first.split_input(b"\x1b[F"), vec![lit("\x1b[F")]);
        assert_eq!(first.split_input(b"\x1b[201~"), vec![lit("\x1b[201~")]);
        assert_eq!(second.split_input(b"0~\x1b[F"), vec![lit("0~\x1b[F")]);
        assert_eq!(first.split_input(b"\x1b[F"), vec![key("End")]);
    }

    #[test]
    fn incomplete_keys_and_mismatched_markers_do_not_hold_input() {
        let mut parser = InputParser::default();
        assert_eq!(parser.split_input(b"\x1b"), vec![lit("\x1b")]);
        assert_eq!(parser.split_input(b"[F"), vec![lit("[F")]);
        assert_eq!(parser.split_input(b"\x1b[20"), vec![lit("\x1b[20")]);
        assert_eq!(parser.split_input(b"x\x1b[F"), vec![lit("x"), key("End")]);
        assert_eq!(parser.split_input(b"\x1b[20"), vec![lit("\x1b[20")]);
        assert_eq!(
            parser.split_input(b"\x1b[200~\x1b[F\x1b[201~\x1b[F"),
            vec![lit("\x1b[200~\x1b[F\x1b[201~"), key("End")]
        );
    }

    #[test]
    fn order_is_preserved() {
        assert_eq!(
            split_input(b"a\x1b[Fb"),
            vec![lit("a"), key("End"), lit("b")]
        );
        assert_eq!(
            split_input(b"\x1b[Ax\x1bOA"),
            vec![key("Up"), lit("x"), key("Up")]
        );
    }

    #[test]
    fn cursor_position_reports_stay_literal() {
        for data in ["\x1b[1;5R", "\x1b[1;2R", "\x1b[24;80R", "\x1b[?1;5R"] {
            assert_eq!(split_input(data.as_bytes()), vec![lit(data)]);
        }
        assert_eq!(split_input(b"\x1bOR"), vec![key("F3")]);
        assert_eq!(split_input(b"\x1b[R"), vec![key("F3")]);
        assert_eq!(split_input(b"\x1b[13;5~"), vec![key("C-F3")]);
    }

    #[test]
    fn unsupported_or_malformed_parameters_stay_literal() {
        for data in [
            "\x1b[1;9C",
            "\x1b[3;9~",
            "\x1b[1;0C",
            "\x1b[1;C",
            "\x1b[;5C",
            "\x1b[1;4294967296C",
            "\x1b[4294967296C",
            "\x1b[1;2;3C",
        ] {
            assert_eq!(split_input(data.as_bytes()), vec![lit(data)], "{data:?}");
        }
        assert_eq!(split_input(b"\x1b[1;1C"), vec![key("Right")]);
    }
}
