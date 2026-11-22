//! Optional removal of alternate-screen switching from the remote byte stream.
//!
//! With the filter enabled, `CSI ? 47|1047|1049 h` is dropped so full-screen
//! programs such as tmux and screen draw on the primary screen and their output
//! reaches the scrollback, like PuTTY's "disable alternate screen" option. All
//! other bytes, including other modes in the same `CSI ? … h` sequence, are
//! forwarded unchanged.

use std::borrow::Cow;

const MODE_ALT_47: u32 = 47;
const MODE_ALT_1047: u32 = 1047;
const MODE_ALT_1049: u32 = 1049;
/// Longest `CSI ?` sequence buffered while waiting for its final byte. Longer
/// input is not a mode change we care about and is flushed verbatim.
const MAX_PENDING: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    /// Inside `ESC [ ?`, collecting parameter bytes.
    PrivateCsi,
}

#[derive(Clone, Debug, Default)]
pub struct AltScreenFilter {
    enabled: bool,
    state: State,
    /// Bytes of an unfinished `ESC [ ? …` sequence, replayed if it turns out
    /// not to be an alternate-screen switch.
    pending: Vec<u8>,
}

fn is_alt_mode(mode: u32) -> bool {
    matches!(mode, MODE_ALT_47 | MODE_ALT_1047 | MODE_ALT_1049)
}

impl AltScreenFilter {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Drop any partial sequence, for stream discontinuities such as reconnects.
    pub fn reset_stream(&mut self) {
        self.state = State::Ground;
        self.pending.clear();
    }

    /// Filter `data`. `in_alternate_screen` is the emulator's current state: a
    /// reset (`l`) is always forwarded while the emulator is on the alternate
    /// screen, so enabling the filter mid-session cannot strand the user there.
    pub fn filter<'a>(&mut self, data: &'a [u8], in_alternate_screen: bool) -> Cow<'a, [u8]> {
        if !self.enabled && self.state == State::Ground && self.pending.is_empty() {
            return Cow::Borrowed(data);
        }
        if self.enabled
            && self.state == State::Ground
            && self.pending.is_empty()
            && !data.contains(&0x1b)
        {
            return Cow::Borrowed(data);
        }
        let mut out = Vec::with_capacity(data.len());
        for &byte in data {
            self.step(byte, in_alternate_screen, &mut out);
        }
        Cow::Owned(out)
    }

    fn step(&mut self, byte: u8, in_alternate_screen: bool, out: &mut Vec<u8>) {
        match self.state {
            State::Ground => {
                if byte == 0x1b {
                    self.state = State::Esc;
                    self.pending.push(byte);
                } else {
                    out.push(byte);
                }
            }
            State::Esc => {
                if byte == b'[' {
                    self.pending.push(byte);
                    self.state = State::PrivateCsi;
                } else {
                    self.flush(out);
                    // A second ESC may itself start a sequence.
                    self.step(byte, in_alternate_screen, out);
                }
            }
            State::PrivateCsi => {
                // The first byte after `ESC [` decides whether this is a
                // private-mode sequence at all.
                if self.pending.len() == 2 {
                    if byte == b'?' {
                        self.pending.push(byte);
                    } else {
                        self.flush(out);
                        self.step(byte, in_alternate_screen, out);
                    }
                    return;
                }
                match byte {
                    b'0'..=b'9' | b';' => {
                        self.pending.push(byte);
                        if self.pending.len() > MAX_PENDING {
                            self.flush(out);
                        }
                    }
                    b'h' | b'l' => {
                        self.pending.push(byte);
                        self.finish(byte == b'l', in_alternate_screen, out);
                    }
                    _ => {
                        self.flush(out);
                        self.step(byte, in_alternate_screen, out);
                    }
                }
            }
        }
    }

    fn flush(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.pending);
        self.state = State::Ground;
    }

    fn finish(&mut self, reset: bool, in_alternate_screen: bool, out: &mut Vec<u8>) {
        // pending = ESC [ ? <params> <final>
        let sequence = std::mem::take(&mut self.pending);
        self.state = State::Ground;
        let final_byte = sequence[sequence.len() - 1];
        let params = &sequence[3..sequence.len() - 1];
        let mut kept: Vec<&[u8]> = Vec::new();
        let mut dropped = false;
        for param in params.split(|&b| b == b';') {
            let mode = std::str::from_utf8(param)
                .ok()
                .and_then(|text| text.parse::<u32>().ok());
            match mode {
                Some(mode)
                    if self.enabled && is_alt_mode(mode) && !(reset && in_alternate_screen) =>
                {
                    dropped = true;
                }
                _ => kept.push(param),
            }
        }
        if !dropped {
            out.extend_from_slice(&sequence);
            return;
        }
        if kept.is_empty() {
            return;
        }
        out.extend_from_slice(b"\x1b[?");
        for (index, param) in kept.iter().enumerate() {
            if index > 0 {
                out.push(b';');
            }
            out.extend_from_slice(param);
        }
        out.push(final_byte);
    }
}

#[cfg(test)]
mod tests {
    use super::AltScreenFilter;

    fn run(filter: &mut AltScreenFilter, chunks: &[&[u8]], in_alt: bool) -> Vec<u8> {
        chunks
            .iter()
            .flat_map(|chunk| filter.filter(chunk, in_alt).into_owned())
            .collect()
    }

    fn enabled() -> AltScreenFilter {
        let mut filter = AltScreenFilter::default();
        filter.set_enabled(true);
        filter
    }

    #[test]
    fn disabled_filter_forwards_every_byte() {
        let mut filter = AltScreenFilter::default();
        let input = b"a\x1b[?1049hb\x1b[?1049lc";
        assert_eq!(run(&mut filter, &[input], false), input);
    }

    #[test]
    fn drops_alternate_screen_set_and_reset_variants() {
        for mode in ["47", "1047", "1049"] {
            let mut filter = enabled();
            let input = format!("a\x1b[?{mode}hb\x1b[?{mode}lc");
            assert_eq!(
                run(&mut filter, &[input.as_bytes()], false),
                b"abc",
                "mode {mode}"
            );
        }
    }

    #[test]
    fn keeps_other_modes_in_a_combined_sequence() {
        let mut filter = enabled();
        assert_eq!(
            run(&mut filter, &[b"\x1b[?1049;2004;1h"], false),
            b"\x1b[?2004;1h"
        );
    }

    #[test]
    fn forwards_unrelated_sequences_unchanged() {
        let mut filter = enabled();
        let input = b"\x1b[31mred\x1b[0m \x1b[?25l\x1b[?2004h\x1b]0;title\x07\x1b[2J";
        assert_eq!(run(&mut filter, &[input], false), input);
    }

    #[test]
    fn handles_sequences_split_across_chunks_at_every_offset() {
        let input = b"x\x1b[?1049hy\x1b[?25lz";
        for split in 1..input.len() {
            let mut filter = enabled();
            let out = run(&mut filter, &[&input[..split], &input[split..]], false);
            assert_eq!(out, b"xy\x1b[?25lz", "split at {split}");
        }
    }

    #[test]
    fn forwards_reset_while_emulator_is_on_alternate_screen() {
        let mut filter = enabled();
        assert_eq!(run(&mut filter, &[b"\x1b[?1049l"], true), b"\x1b[?1049l");
        // A set is still dropped there, so nesting cannot start.
        assert_eq!(run(&mut filter, &[b"\x1b[?1049h"], true), b"");
    }

    #[test]
    fn overlong_and_malformed_sequences_are_flushed_verbatim() {
        let mut filter = enabled();
        let long = format!("\x1b[?{}h", "1;".repeat(80));
        assert_eq!(run(&mut filter, &[long.as_bytes()], false), long.as_bytes());
        let mut filter = enabled();
        assert_eq!(
            run(&mut filter, &[b"\x1b[?10x\x1b\x1bA"], false),
            b"\x1b[?10x\x1b\x1bA"
        );
    }

    #[test]
    fn pending_bytes_survive_disabling_mid_sequence() {
        let mut filter = enabled();
        assert_eq!(run(&mut filter, &[b"\x1b[?10"], false), b"");
        filter.set_enabled(false);
        assert_eq!(run(&mut filter, &[b"49h"], false), b"\x1b[?1049h");
    }

    #[test]
    fn reset_stream_discards_partial_sequence() {
        let mut filter = enabled();
        assert_eq!(run(&mut filter, &[b"\x1b[?10"], false), b"");
        filter.reset_stream();
        assert_eq!(run(&mut filter, &[b"ok"], false), b"ok");
    }
}
