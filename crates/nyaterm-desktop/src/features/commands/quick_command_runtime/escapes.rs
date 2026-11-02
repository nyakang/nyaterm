use std::time::Duration;

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
    let mut chunks = Vec::new();
    let mut pending = String::new();
    let mut delay = Duration::ZERO;
    let mut chars = text.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            pending.push(ch);
            continue;
        }
        match chars.next() {
            Some('r') => pending.push('\r'),
            Some('n') => pending.push('\n'),
            Some('t') => pending.push('\t'),
            Some('\\') => pending.push('\\'),
            Some('p') => {
                // The pause belongs to the text after it, so it is banked here
                // and handed to the next chunk that is actually flushed.
                flush(&mut chunks, &mut pending, &mut delay);
                delay += QUICK_COMMAND_PAUSE;
            }
            Some(other) => {
                pending.push('\\');
                pending.push(other);
            }
            // A lone trailing backslash stays literal.
            None => pending.push('\\'),
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

/// Literal mode bypasses both variable substitution and script escapes. Only
/// the independently selected execution mode may add a final carriage return.
pub(super) fn plan_literal_quick_command(text: &str, execute: bool) -> Vec<QuickCommandChunk> {
    let mut chunks = vec![QuickCommandChunk {
        delay: Duration::ZERO,
        bytes: text.as_bytes().to_vec(),
    }];
    if execute {
        append_execution_return(&mut chunks);
    }
    chunks
}

fn flush(chunks: &mut Vec<QuickCommandChunk>, pending: &mut String, delay: &mut Duration) {
    if pending.is_empty() {
        // Nothing to send yet, so the banked pause carries over.
        return;
    }
    chunks.push(QuickCommandChunk {
        delay: std::mem::take(delay),
        bytes: std::mem::take(pending).into_bytes(),
    });
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

    use super::{plan_literal_quick_command, plan_quick_command, quick_command_needs_staging};

    #[test]
    fn literal_commands_preserve_variables_escapes_whitespace_and_unicode_without_pausing() {
        let text = "  {{变量}} \\r\\n\\t\\p\\\\ C:\\temp\n下一行\t ";
        let plan = plan_literal_quick_command(text, false);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].bytes, text.as_bytes());
        assert!(!quick_command_needs_staging(&plan));
        assert_eq!(
            plan_literal_quick_command(text, true)[0].bytes,
            format!("{text}\r").as_bytes()
        );
    }

    #[test]
    fn literal_execution_only_adds_a_missing_actual_line_ending() {
        for ending in ["\r", "\n", "\r\n"] {
            let text = format!("echo ready{ending}");
            assert_eq!(
                plan_literal_quick_command(&text, true)[0].bytes,
                text.as_bytes()
            );
        }
        assert_eq!(
            plan_literal_quick_command(r"echo ready\r", true)[0].bytes,
            b"echo ready\\r\r"
        );
    }

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
}
