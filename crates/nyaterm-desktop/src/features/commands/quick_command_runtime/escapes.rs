use std::time::Duration;

use crate::features::recording::MacroInputChunk;

/// One `\p` in a quick command script waits this long.
///
/// The escape exists for devices whose prompts cannot keep up: send a login
/// name, wait, then send the password. One second matches the pause the issue
/// asked for, and stacking `\p\p` waits twice as long.
const QUICK_COMMAND_PAUSE: Duration = Duration::from_secs(1);

/// One write of a quick command script.
///
/// Scripts are split only where a `\p` pause separates them; everything else
/// travels in a single chunk so the ordinary path stays one write.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct QuickCommandChunk {
    /// Waited out before this chunk is written. Zero for the first chunk and
    /// for every chunk that directly follows another.
    pub(super) delay: Duration,
    pub(super) bytes: Vec<u8>,
}

/// Rewrite a quick command script into the writes it should perform.
///
/// Recognised escapes:
///
/// | script | sent |
/// | --- | --- |
/// | `\r` | carriage return |
/// | `\n` | line feed |
/// | `\t` | tab |
/// | `\p` | one second pause before whatever follows |
/// | `\\` | a literal backslash |
///
/// Anything else keeps its backslash, so `\d` in a regex and `C:\Users` in a
/// Windows path still reach the terminal as typed. A trailing `\p` has nothing
/// to wait for and is dropped.
///
/// `execute` only decides whether the last chunk gets a carriage return: a
/// script that already ends in `\r` (escape or literal) is left alone, exactly
/// like the pre-escape behaviour.
pub(super) fn plan_quick_command(text: &str, execute: bool) -> Vec<QuickCommandChunk> {
    plan_quick_command_with_source(text, execute, false)
}

/// Macro scripts may additionally decode `\xHH` into an exact control byte.
/// Manual and AI commands keep the original escape interpretation.
pub(super) fn plan_quick_command_with_source(
    text: &str,
    execute: bool,
    macro_source: bool,
) -> Vec<QuickCommandChunk> {
    let mut chunks = Vec::new();
    let mut pending = Vec::new();
    let mut delay = Duration::ZERO;
    let mut chars = text.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            pending.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        match chars.next() {
            Some('r') => pending.push(b'\r'),
            Some('n') => pending.push(b'\n'),
            Some('t') => pending.push(b'\t'),
            Some('\\') => pending.push(b'\\'),
            Some('p') => {
                // The pause belongs to the text after it, so it is banked here
                // and handed to the next chunk that is actually flushed.
                flush(&mut chunks, &mut pending, &mut delay);
                delay += QUICK_COMMAND_PAUSE;
            }
            Some('x') if macro_source => {
                let mut peek = chars.clone();
                match (
                    peek.next().and_then(|c| c.to_digit(16)),
                    peek.next().and_then(|c| c.to_digit(16)),
                ) {
                    (Some(hi), Some(lo)) => {
                        chars.next();
                        chars.next();
                        pending.push((hi * 16 + lo) as u8);
                    }
                    _ => pending.extend_from_slice(b"\\x"),
                }
            }
            Some(other) => {
                pending.push(b'\\');
                pending.extend_from_slice(other.encode_utf8(&mut [0; 4]).as_bytes());
            }
            // A lone trailing backslash stays literal.
            None => pending.push(b'\\'),
        }
    }
    flush(&mut chunks, &mut pending, &mut delay);

    if chunks.is_empty() {
        // A script that is nothing but pauses has nothing to send, so keep the
        // literal text: sends exactly what older versions sent.
        chunks.push(QuickCommandChunk {
            delay: Duration::ZERO,
            bytes: text.as_bytes().to_vec(),
        });
    }
    if execute {
        append_execution_return(&mut chunks);
    }
    chunks
}

/// Whether a plan must go through the staged writer instead of the ordinary
/// single-write input path.
///
/// The chunk count alone is not enough: a script that starts with `\p` plans
/// as one chunk that still carries the pause, and dropping that delay would
/// send the text immediately.
pub(super) fn quick_command_needs_staging(chunks: &[QuickCommandChunk]) -> bool {
    chunks.len() > 1 || chunks.iter().any(|chunk| !chunk.delay.is_zero())
}

fn flush(chunks: &mut Vec<QuickCommandChunk>, pending: &mut Vec<u8>, delay: &mut Duration) {
    if pending.is_empty() {
        // Nothing to send yet, so the banked pause carries over.
        return;
    }
    chunks.push(QuickCommandChunk {
        delay: std::mem::take(delay),
        bytes: std::mem::take(pending),
    });
}

/// Preserve the logical user bytes as a reviewable Quick Command script.
/// Backslashes are quoted so literal `\p` or `\x41` never become instructions.
pub(in crate::features) fn encode_recorded_macro(chunks: &[MacroInputChunk]) -> String {
    let mut result = String::new();
    for chunk in chunks {
        for _ in 0..chunk.delay_seconds {
            result.push_str("\\p");
        }
        let mut remaining = chunk.bytes.as_slice();
        while !remaining.is_empty() {
            let (valid, invalid_len) = match std::str::from_utf8(remaining) {
                Ok(_) => (remaining.len(), 0),
                Err(error) => (
                    error.valid_up_to(),
                    error
                        .error_len()
                        .unwrap_or(remaining.len() - error.valid_up_to()),
                ),
            };
            let text = std::str::from_utf8(&remaining[..valid]).expect("valid UTF-8 prefix");
            for ch in text.chars() {
                match ch {
                    '\r' => result.push_str("\\r"),
                    '\n' => result.push_str("\\n"),
                    '\t' => result.push_str("\\t"),
                    '\\' => result.push_str("\\\\"),
                    ch if ch.is_control() => {
                        for byte in ch.to_string().bytes() {
                            result.push_str(&format!("\\x{byte:02X}"));
                        }
                    }
                    ch => result.push(ch),
                }
            }
            remaining = &remaining[valid..];
            for byte in &remaining[..invalid_len] {
                result.push_str(&format!("\\x{byte:02X}"));
            }
            remaining = &remaining[invalid_len..];
        }
    }
    result
}

fn append_execution_return(chunks: &mut Vec<QuickCommandChunk>) {
    if let Some(last) = chunks.last_mut() {
        if !last.bytes.ends_with(b"\r") && !last.bytes.ends_with(b"\n") {
            last.bytes.push(b'\r');
        }
        return;
    }
    chunks.push(QuickCommandChunk {
        delay: Duration::ZERO,
        bytes: vec![b'\r'],
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        encode_recorded_macro, plan_quick_command, plan_quick_command_with_source,
        quick_command_needs_staging,
    };
    use crate::features::recording::MacroInputChunk;

    fn chunks(text: &str, execute: bool) -> Vec<(Duration, Vec<u8>)> {
        plan_quick_command(text, execute)
            .into_iter()
            .map(|chunk| (chunk.delay, chunk.bytes))
            .collect()
    }

    fn single(text: &str, execute: bool) -> Vec<u8> {
        let chunks = chunks(text, execute);
        assert_eq!(chunks.len(), 1, "expected one chunk for {text:?}");
        assert_eq!(chunks[0].0, Duration::ZERO);
        chunks[0].1.clone()
    }

    #[test]
    fn plain_scripts_keep_their_single_write_and_trailing_return() {
        assert_eq!(single("pwd", true), b"pwd\r".to_vec());
        assert_eq!(single("pwd", false), b"pwd".to_vec());
    }

    #[test]
    fn escape_sequences_become_control_characters() {
        // The execution return still lands at the end: `a\r` in the middle does
        // not make the script end in a return.
        assert_eq!(single("a\\rb", true), b"a\rb\r".to_vec());
        assert_eq!(single("a\\rb", false), b"a\rb".to_vec());
        assert_eq!(single("a\\nb", false), b"a\nb".to_vec());
        assert_eq!(single("a\\tb", false), b"a\tb".to_vec());
        assert_eq!(single("a\\\\b", false), b"a\\b".to_vec());
    }

    #[test]
    fn unknown_escapes_reach_the_terminal_unchanged() {
        // Regex classes, Windows paths and a lone trailing backslash.
        assert_eq!(single("grep '\\d+'", false), b"grep '\\d+'".to_vec());
        assert_eq!(
            single("cd C:\\Users\\me", false),
            b"cd C:\\Users\\me".to_vec()
        );
        assert_eq!(single("echo \\", false), b"echo \\".to_vec());
        assert_eq!(single("\\x41", false), b"\\x41".to_vec());
    }

    #[test]
    fn execution_return_is_not_duplicated_after_an_escape_carriage_return() {
        assert_eq!(single("reboot\\r", true), b"reboot\r".to_vec());
        assert_eq!(single("reboot\\n", true), b"reboot\n".to_vec());
        assert_eq!(single("reboot\\r", false), b"reboot\r".to_vec());
    }

    #[test]
    fn pause_splits_the_script_and_delays_what_follows() {
        assert_eq!(
            chunks("enable\\psecret", true),
            vec![
                (Duration::ZERO, b"enable".to_vec()),
                (Duration::from_secs(1), b"secret\r".to_vec()),
            ]
        );
    }

    #[test]
    fn stacked_pauses_wait_once_per_escape() {
        assert_eq!(
            chunks("a\\p\\pb", false),
            vec![
                (Duration::ZERO, b"a".to_vec()),
                (Duration::from_secs(2), b"b".to_vec()),
            ]
        );
    }

    #[test]
    fn a_pause_before_text_is_not_sent_before_it_finishes_waiting() {
        assert_eq!(
            chunks("\\pwhoami", true),
            vec![(Duration::from_secs(1), b"whoami\r".to_vec())]
        );
    }

    #[test]
    fn a_pause_with_nothing_after_it_changes_nothing() {
        // The trailing pause has nothing to wait for, so the execution return
        // still lands on the text the user typed.
        assert_eq!(single("reboot\\p", true), b"reboot\r".to_vec());
        assert_eq!(single("reboot\\p", false), b"reboot".to_vec());
    }

    #[test]
    fn a_script_of_only_pauses_stays_literal() {
        assert_eq!(single("\\p", false), b"\\p".to_vec());
        assert_eq!(single("\\p\\p", true), b"\\p\\p\r".to_vec());
    }

    #[test]
    fn real_newlines_are_sent_as_typed() {
        // Writing a real newline is what the editor already does for Enter.
        assert_eq!(single("pwd\n", true), b"pwd\n".to_vec());
        assert_eq!(single("pwd\n", false), b"pwd\n".to_vec());
    }

    #[test]
    fn pauses_split_multi_line_scripts_into_staged_writes() {
        assert_eq!(
            chunks("conf t\\pinterface gi0/1\\pshutdown", true),
            vec![
                (Duration::ZERO, b"conf t".to_vec()),
                (Duration::from_secs(1), b"interface gi0/1".to_vec()),
                (Duration::from_secs(1), b"shutdown\r".to_vec()),
            ]
        );
    }

    #[test]
    fn a_lone_chunk_with_a_leading_pause_still_stages() {
        // `\pwhoami` plans as one delayed chunk: a staging gate keyed only on
        // the chunk count would drop the wait and send immediately.
        assert!(quick_command_needs_staging(&plan_quick_command(
            "\\pwhoami",
            true
        )));
        assert!(quick_command_needs_staging(&plan_quick_command(
            "\\p\\pwhoami",
            false
        )));
    }

    #[test]
    fn scripts_without_a_delayed_chunk_do_not_stage() {
        assert!(!quick_command_needs_staging(&plan_quick_command(
            "pwd", true
        )));
        // A trailing pause has nothing to wait for and is dropped.
        assert!(!quick_command_needs_staging(&plan_quick_command(
            "reboot\\p",
            true
        )));
        // A pauses-only script falls back to its literal text, not a wait.
        assert!(!quick_command_needs_staging(&plan_quick_command(
            "\\p", false
        )));
        assert!(quick_command_needs_staging(&plan_quick_command(
            "enable\\psecret",
            false
        )));
    }

    #[test]
    fn macro_script_round_trip_keeps_literal_placeholders_unicode_and_control_bytes() {
        let first = "echo {{host}} C:\\temp\r\t\x1b[3~\x7f".as_bytes().to_vec();
        let second = "한글\n\\p\\x1B".as_bytes().to_vec();
        let script = encode_recorded_macro(&[
            MacroInputChunk {
                delay_seconds: 0,
                bytes: first.clone(),
            },
            MacroInputChunk {
                delay_seconds: 2,
                bytes: second.clone(),
            },
        ]);
        assert!(script.contains("{{host}}"));
        assert!(script.contains(r"\x1B[3~\x7F"));
        assert!(script.contains(r"\\p\\x1B"));
        let planned = plan_quick_command_with_source(&script, false, true);
        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].delay, Duration::ZERO);
        assert_eq!(planned[0].bytes, first);
        assert_eq!(planned[1].delay, Duration::from_secs(2));
        assert_eq!(planned[1].bytes, second);
    }

    #[test]
    fn macro_hex_is_source_specific_and_rejects_incomplete_sequences() {
        assert_eq!(
            plan_quick_command_with_source(r"\x1B\x7F", false, true)[0].bytes,
            b"\x1b\x7f"
        );
        assert_eq!(
            plan_quick_command_with_source(r"\x41", false, false)[0].bytes,
            b"\\x41"
        );
        assert_eq!(
            plan_quick_command_with_source(r"\xG1\x1", false, true)[0].bytes,
            b"\\xG1\\x1"
        );
    }
}
