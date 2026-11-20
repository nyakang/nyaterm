//! Text-input policy, independent of Telnet negotiation and binary writes.

use nyaterm_core::character_encoding::CharacterEncoding;
use nyaterm_terminal::terminal_cell_count;

use crate::telnet_codec::escape_telnet_application_data;
use crate::{TelnetEnterMode, TelnetSessionConfig, telnet_prompts};

// Input can contain credentials; do not derive Debug for this state.
#[derive(Clone, Default)]
pub(super) struct TelnetInputState {
    line: Vec<u8>,
    escape: EscapeState,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Csi,
    Ss3,
    Osc,
    OscEscape,
    ControlString,
    ControlStringEscape,
}

impl EscapeState {
    /// Forward escape bytes immediately, but keep sequence tails out of local
    /// editing and echo even when a sequence spans multiple writes.
    fn consume(&mut self, byte: u8) -> bool {
        if *self == Self::Ground {
            if byte == 0x1b {
                *self = Self::Escape;
                return true;
            }
            return false;
        }
        // A standalone Escape key must not consume the next Enter or control
        // key as part of an unfinished keyboard sequence. String payloads, on
        // the other hand, may legitimately contain these bytes.
        if matches!(*self, Self::Escape | Self::Csi | Self::Ss3) && byte < 0x20 && byte != 0x1b {
            *self = Self::Ground;
            return false;
        }
        *self = match (*self, byte) {
            (Self::Escape, b'[') => Self::Csi,
            (Self::Escape, b'O') => Self::Ss3,
            (Self::Escape, b']') => Self::Osc,
            (Self::Escape, b'P' | b'X' | b'^' | b'_') => Self::ControlString,
            (Self::Escape, 0x20..=0x2f) => Self::Escape,
            (Self::Csi | Self::Ss3, 0x40..=0x7e) => Self::Ground,
            (Self::Osc, 0x07) => Self::Ground,
            (Self::Osc, 0x1b) => Self::OscEscape,
            (Self::OscEscape | Self::ControlStringEscape, b'\\') => Self::Ground,
            (Self::OscEscape, 0x1b) => Self::OscEscape,
            (Self::OscEscape, _) => Self::Osc,
            (Self::ControlString, 0x1b) => Self::ControlStringEscape,
            (Self::ControlStringEscape, 0x1b) => Self::ControlStringEscape,
            (Self::ControlStringEscape, _) => Self::ControlString,
            (_, 0x1b) => Self::Escape,
            (Self::Escape, _) => Self::Ground,
            (state, _) => state,
        };
        true
    }
}

impl TelnetInputState {
    pub(super) fn prepare(
        &mut self,
        data: &[u8],
        config: &TelnetSessionConfig,
    ) -> (Vec<u8>, Vec<u8>) {
        self.process(data, config, config.local_line_edit, config.local_echo)
    }

    fn process(
        &mut self,
        data: &[u8],
        config: &TelnetSessionConfig,
        line_edit: bool,
        local_echo: bool,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut send = Vec::new();
        let mut echo = Vec::new();
        let mut index = 0;
        while index < data.len() {
            // Recognize a complete Delete key packet before forwarding ESC.
            // Partial escape packets still pass through without buffering text.
            if line_edit
                && self.escape == EscapeState::Ground
                && data[index..].starts_with(b"\x1b[3~")
            {
                self.backspace(config, local_echo, &mut echo);
                index += 4;
                continue;
            }
            let byte = data[index];
            if self.escape.consume(byte) {
                send.push(byte);
                index += 1;
                continue;
            }
            match byte {
                b'\r' | b'\n' => {
                    if line_edit {
                        send.append(&mut self.line);
                    }
                    send.extend_from_slice(enter_bytes(config.enter_mode));
                    if local_echo {
                        echo.extend_from_slice(b"\r\n");
                    }
                    if byte == b'\r' && data.get(index + 1) == Some(&b'\n') {
                        index += 1;
                    }
                }
                0x08 | 0x7f if line_edit => self.backspace(config, local_echo, &mut echo),
                0x00..=0x1f | 0x7f => {
                    // In particular, Ctrl+C does not discard the pending line.
                    send.push(byte);
                    if local_echo && byte == b'\t' {
                        echo.push(byte);
                    }
                }
                _ => {
                    if line_edit {
                        self.line.push(byte);
                    } else {
                        send.push(byte);
                    }
                    if local_echo {
                        echo.push(byte);
                    }
                }
            }
            index += 1;
        }
        (escape_telnet_application_data(&send, config.raw_tcp), echo)
    }

    fn backspace(&mut self, config: &TelnetSessionConfig, local_echo: bool, echo: &mut Vec<u8>) {
        if self.line.is_empty() {
            return;
        }
        let encoding = CharacterEncoding::parse(&config.encoding).unwrap_or_default();
        let codec = encoding.codec();
        let mut start = 0;
        let mut width = 1;
        let mut index = 0;
        // The desktop has already encoded input into the session charset. Find
        // byte boundaries without re-encoding: legacy mappings need not round-trip.
        while index < self.line.len() {
            start = index;
            let tail = &self.line[index..];
            let character = (1..=4.min(tail.len())).find_map(|len| {
                codec
                    .decode_without_bom_handling_and_without_replacement(&tail[..len])
                    .map(|text| (len, terminal_cell_count(&text)))
            });
            let (len, cells) = character.unwrap_or((1, 1));
            width = cells;
            index += len;
        }
        self.line.truncate(start);
        if local_echo {
            for _ in 0..width {
                echo.extend_from_slice(b"\x08 \x08");
            }
        }
    }
}

fn enter_bytes(mode: TelnetEnterMode) -> &'static [u8] {
    match mode {
        TelnetEnterMode::Cr => b"\r",
        TelnetEnterMode::Lf => b"\n",
        TelnetEnterMode::Crlf => b"\r\n",
    }
}

fn normalize_telnet_input(data: &[u8], config: &TelnetSessionConfig) -> Vec<u8> {
    TelnetInputState::default()
        .process(data, config, false, false)
        .0
}

pub(super) fn telnet_auto_login_line_bytes(value: &str, config: &TelnetSessionConfig) -> Vec<u8> {
    telnet_prompts::telnet_auto_login_line_bytes(value, config, normalize_telnet_input)
}

#[cfg(test)]
mod tests {
    use nyaterm_core::character_encoding::CharacterEncoding;
    use nyaterm_terminal::TerminalScreen;

    use super::{TelnetInputState, telnet_auto_login_line_bytes};
    use crate::{TelnetEnterMode, TelnetSessionConfig};

    fn editing_config() -> TelnetSessionConfig {
        TelnetSessionConfig {
            local_line_edit: true,
            local_echo: true,
            ..Default::default()
        }
    }

    #[test]
    fn empty_lines_multiline_paste_and_consecutive_enters_submit_once_each() {
        for local_line_edit in [false, true] {
            for local_echo in [false, true] {
                let config = TelnetSessionConfig {
                    local_line_edit,
                    local_echo,
                    enter_mode: TelnetEnterMode::Crlf,
                    ..Default::default()
                };
                let (send, echo) =
                    TelnetInputState::default().prepare(b"\r\na\nb\r\n\r\r", &config);
                assert_eq!(send, b"\r\na\r\nb\r\n\r\n\r\n");
                assert_eq!(echo, if local_echo { send } else { Vec::new() });
            }
        }
    }

    #[test]
    fn controls_and_escape_sequences_pass_through_without_entering_the_line() {
        let config = editing_config();
        let mut state = TelnetInputState::default();
        let (send, echo) = state.prepare(b"a\x03\x04\t\x1b[A\x1bOB\x1b[3~", &config);
        assert_eq!(send, b"\x03\x04\t\x1b[A\x1bOB");
        assert_eq!(echo, b"a\t\x08 \x08");
        assert!(state.line.is_empty());

        assert_eq!(state.prepare(b"abc\x03", &config).0, b"\x03");
        assert_eq!(state.prepare(b"\r", &config).0, b"abc\r");
        assert_eq!(state.prepare(b"\x08\x7f\x1b[3~", &config), (vec![], vec![]));
    }

    #[test]
    fn escape_tails_across_writes_are_forwarded_and_never_echoed_or_buffered() {
        for local_line_edit in [false, true] {
            let config = TelnetSessionConfig {
                local_line_edit,
                local_echo: true,
                ..Default::default()
            };
            let mut state = TelnetInputState::default();
            for packet in [
                b"\x1b".as_slice(),
                b"[",
                b"1;5D",
                b"\x1b]title",
                b"\r\n",
                b"\x1b",
                b"\\",
                b"\x1bPignored",
                b"\x1b\\",
            ] {
                let (send, echo) = state.prepare(packet, &config);
                assert_eq!(send, packet);
                assert!(echo.is_empty());
                assert!(state.line.is_empty());
            }
            assert_eq!(
                state.prepare(b"ok\r", &config),
                (b"ok\r".to_vec(), b"ok\r\n".to_vec())
            );
        }
    }

    #[test]
    fn ordinary_echo_filters_controls_and_normalizes_display_without_changing_wire_controls() {
        let config = TelnetSessionConfig {
            local_echo: true,
            enter_mode: TelnetEnterMode::Lf,
            ..Default::default()
        };
        let (send, echo) =
            TelnetInputState::default().prepare(b"a\x03\x04\0\x08\x7f\t\x1b[A\r\nb\nc\r", &config);
        assert_eq!(send, b"a\x03\x04\0\x08\x7f\t\x1b[A\nb\nc\n");
        assert_eq!(echo, b"a\t\r\nb\r\nc\r\n");
    }

    #[test]
    fn enter_after_a_standalone_escape_still_submits_the_pending_line() {
        let config = TelnetSessionConfig {
            enter_mode: TelnetEnterMode::Crlf,
            ..editing_config()
        };
        let mut state = TelnetInputState::default();
        state.prepare(b"abc", &config);
        assert_eq!(state.prepare(b"\x1b", &config), (b"\x1b".to_vec(), vec![]));
        assert_eq!(
            state.prepare(b"\r", &config),
            (b"abc\r\n".to_vec(), b"\r\n".to_vec())
        );
    }

    #[test]
    fn multibyte_backspace_removes_whole_characters_across_input_calls() {
        for encoding in [CharacterEncoding::Utf8, CharacterEncoding::Gb18030] {
            let config = TelnetSessionConfig {
                encoding: encoding.label().into(),
                ..editing_config()
            };
            let mut state = TelnetInputState::default();
            for byte in encoding.encode("中😀").unwrap() {
                assert!(state.prepare(&[byte], &config).0.is_empty());
            }
            state.prepare(b"\x08", &config);
            assert_eq!(state.line, encoding.encode("中").unwrap());
            state.prepare(b"\x7f", &config);
            assert!(state.line.is_empty());
            assert_eq!(state.prepare(b"ok\r", &config).0, b"ok\r");
        }
    }

    #[test]
    fn backspace_preserves_original_bytes_in_every_supported_session_charset() {
        for encoding in CharacterEncoding::ALL {
            let config = TelnetSessionConfig {
                encoding: encoding.label().into(),
                ..editing_config()
            };
            let mut state = TelnetInputState::default();
            let (sample, remaining) = if encoding == CharacterEncoding::Windows1252 {
                ("éaé", "é")
            } else {
                ("中a中", "中")
            };
            let text = encoding.encode(sample).unwrap();
            for byte in &text {
                assert!(state.prepare(&[*byte], &config).0.is_empty());
            }
            state.prepare(b"\x7f\x08", &config);
            assert_eq!(state.line, encoding.encode(remaining).unwrap());
            let mut expected = encoding.encode(remaining).unwrap();
            expected.push(b'\r');
            assert_eq!(state.prepare(b"\r", &config).0, expected);
        }
        // This Big5 mapping has an alternative canonical encoding. Editing must
        // retain the original byte sequence rather than decode and re-encode it.
        let config = TelnetSessionConfig {
            encoding: "Big5".into(),
            ..editing_config()
        };
        let mut state = TelnetInputState::default();
        state.prepare(b"\xa2\xccx", &config);
        assert_eq!(state.prepare(b"\x08\r", &config).0, b"\xa2\xcc\r");
    }

    #[test]
    fn wide_character_erasure_and_enter_render_as_a_blank_line() {
        for encoding in [CharacterEncoding::Utf8, CharacterEncoding::Gbk] {
            let config = TelnetSessionConfig {
                encoding: encoding.label().into(),
                ..editing_config()
            };
            let mut state = TelnetInputState::default();
            let mut screen = TerminalScreen::new(80, 24);
            screen.set_encoding(encoding.label()).unwrap();
            let (_, echo) = state.prepare(&encoding.encode("中").unwrap(), &config);
            screen.advance(&echo);
            let (_, echo) = state.prepare(b"\x08\r", &config);
            screen.advance(&echo);
            let snapshot = screen.snapshot();
            assert_eq!(snapshot.cursor.row, 1);
            assert_eq!(snapshot.cursor.col, 0);
            assert!(snapshot.rows().iter().all(|row| row.text.trim().is_empty()));
        }
    }

    #[test]
    fn application_data_is_escaped_once_and_local_echo_keeps_the_original_bytes() {
        for raw_tcp in [false, true] {
            for local_line_edit in [false, true] {
                let config = TelnetSessionConfig {
                    raw_tcp,
                    local_line_edit,
                    local_echo: true,
                    enter_mode: TelnetEnterMode::Crlf,
                    ..Default::default()
                };
                let (send, echo) = TelnetInputState::default().prepare(b"a\xffb\r\n", &config);
                assert_eq!(
                    send,
                    if raw_tcp {
                        b"a\xffb\r\n".as_slice()
                    } else {
                        b"a\xff\xffb\r\n".as_slice()
                    }
                );
                assert_eq!(echo, b"a\xffb\r\n");
            }
        }
    }

    #[test]
    fn auto_login_uses_the_selected_enter_mode_in_both_transports_without_local_editing() {
        for raw_tcp in [false, true] {
            for (enter_mode, newline) in [
                (TelnetEnterMode::Cr, b"\r".as_slice()),
                (TelnetEnterMode::Lf, b"\n".as_slice()),
                (TelnetEnterMode::Crlf, b"\r\n".as_slice()),
            ] {
                let config = TelnetSessionConfig {
                    raw_tcp,
                    enter_mode,
                    ..editing_config()
                };
                let mut expected = b"operator".to_vec();
                expected.extend_from_slice(newline);
                assert_eq!(telnet_auto_login_line_bytes("operator", &config), expected);
            }
        }
    }
}
