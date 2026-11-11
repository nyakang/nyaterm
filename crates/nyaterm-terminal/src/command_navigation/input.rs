//! Navigation-only submission evidence. It never changes the editing tracker.

use nyaterm_core::terminal::input_tracker::{
    MAX_TRACKED_INPUT_BYTES, TerminalInputState, get_tracked_submission_command,
    known_terminal_prompt_prefix,
};
use zeroize::{Zeroize, Zeroizing};

use crate::editing::recover_marked_input;
use crate::{TerminalLineId, TerminalSnapshot};

pub struct CommandSubmissionEvidence {
    pub anchor: TerminalLineId,
    pub command: String,
    pub prompt: Option<String>,
}

impl std::fmt::Debug for CommandSubmissionEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSubmissionEvidence")
            .field("anchor", &self.anchor)
            .field("command", &"[REDACTED]")
            .field("prompt", &"[REDACTED]")
            .finish()
    }
}

impl Drop for CommandSubmissionEvidence {
    fn drop(&mut self) {
        self.command.zeroize();
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.zeroize();
        }
    }
}

fn cursor_chain(snapshot: &TerminalSnapshot) -> Option<(TerminalLineId, Zeroizing<String>)> {
    if snapshot.history.alternate_screen
        || snapshot.display_offset != 0
        || snapshot.cursor.row >= snapshot.row_count()
    {
        return None;
    }
    let mut start = snapshot.cursor.row;
    while start > 0 && snapshot.row(start)?.wrapped {
        start -= 1;
    }
    let mut end = snapshot.cursor.row + 1;
    while end < snapshot.row_count() && snapshot.row(end)?.wrapped {
        end += 1;
    }
    let anchor = snapshot.row(start)?.line_id?;
    let mut text = Zeroizing::new(String::new());
    for row in start..end {
        for cell in snapshot
            .row(row)?
            .cells
            .iter()
            .filter(|cell| cell.width > 0)
        {
            text.push_str(if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_ref()
            });
            if text.len() > MAX_TRACKED_INPUT_BYTES {
                return None;
            }
        }
    }
    Some((anchor, text))
}

pub fn visible_shell_prompt(snapshot: &TerminalSnapshot) -> Option<(TerminalLineId, String)> {
    let (anchor, text) = cursor_chain(snapshot)?;
    let prompt = known_terminal_prompt_prefix(&text)?;
    Some((anchor, prompt.trim_end().to_owned()))
}

pub fn command_submission_evidence(
    snapshot: &TerminalSnapshot,
    input: &TerminalInputState,
    bytes: &[u8],
) -> Option<CommandSubmissionEvidence> {
    let text = std::str::from_utf8(bytes).ok()?;
    let suffix = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix(['\r', '\n']))?;
    if suffix.contains(['\r', '\n'])
        || suffix.chars().any(char::is_control)
        || input.multiline
        || input.paste_mode
    {
        return None;
    }
    let (row, visible) = cursor_chain(snapshot)?;
    let prompt = known_terminal_prompt_prefix(&visible);
    let marked_anchor = snapshot.shell_input_anchor.filter(|anchor| {
        snapshot
            .row(snapshot.cursor.row)
            .and_then(|row| row.line_id)
            .is_some_and(|cursor| {
                anchor.line_id.epoch == row.epoch
                    && (row.logical_line..=cursor.logical_line)
                        .contains(&anchor.line_id.logical_line)
            })
    });
    if prompt.is_none() && marked_anchor.is_none() {
        return None;
    }
    let mut command = Zeroizing::new(get_tracked_submission_command(input));
    if command.is_empty() {
        command = Zeroizing::new(if let Some(prompt) = prompt {
            visible[prompt.len()..].trim_end().to_owned()
        } else {
            recover_marked_input(snapshot, MAX_TRACKED_INPUT_BYTES)?.0
        });
    }
    command.push_str(suffix);
    if command.trim().is_empty() || command.len() > MAX_TRACKED_INPUT_BYTES {
        return None;
    }
    Some(CommandSubmissionEvidence {
        anchor: snapshot.shell_command_prompt.unwrap_or(row),
        command: std::mem::take(&mut *command),
        prompt: prompt.map(|value| value.trim_end().to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use crate::TerminalCore;
    use crate::command_navigation::command_submission_evidence;
    use crate::command_navigation::input::visible_shell_prompt;
    use nyaterm_core::terminal::input_tracker::{TerminalInputState, apply_terminal_input_data};

    #[test]
    fn history_completion_and_single_line_paste_use_prompt_evidence() {
        let mut terminal = TerminalCore::new(40, 4);
        terminal.advance(b"PS C:\\work> echo recalled");
        for key in ["\x1b[A", "\t"] {
            let input = apply_terminal_input_data(&TerminalInputState::new(), key);
            let evidence =
                command_submission_evidence(&terminal.snapshot(), &input, b"\r").unwrap();
            assert_eq!(evidence.command, "echo recalled");
        }
        terminal.clear();
        terminal.advance(b"PS C:\\work> ");
        assert_eq!(
            command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"echo pasted\r\n"
            )
            .unwrap()
            .command,
            "echo pasted"
        );
        assert!(
            command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"echo first\necho second\n"
            )
            .is_none()
        );
    }

    #[test]
    fn android_and_themed_prompts_preserve_history_recall_and_command_navigation() {
        for prompt in ["fuxi:/sdcard # ", "~/src ❯ ", "➜ ~/src ", "~ % "] {
            let mut terminal = TerminalCore::new(80, 4);
            terminal.advance(format!("{prompt}mv source destination").as_bytes());
            let snapshot = terminal.snapshot();
            assert_eq!(
                visible_shell_prompt(&snapshot).unwrap().1,
                prompt.trim_end()
            );
            let input = apply_terminal_input_data(&TerminalInputState::new(), "\x1b[A");
            let evidence = command_submission_evidence(&snapshot, &input, b"\r").unwrap();
            assert_eq!(evidence.command, "mv source destination");
            assert_eq!(evidence.prompt.as_deref(), Some(prompt.trim_end()));
        }
    }

    #[test]
    fn output_empty_submission_and_unmarked_raw_input_are_not_commands() {
        let mut terminal = TerminalCore::new(40, 4);
        let input = apply_terminal_input_data(&TerminalInputState::new(), "secret");
        terminal.advance(b"Password: ");
        assert!(command_submission_evidence(&terminal.snapshot(), &input, b"\r").is_none());
        terminal.clear();
        terminal.advance(b"$ ");
        assert!(
            command_submission_evidence(&terminal.snapshot(), &TerminalInputState::new(), b"\r")
                .is_none()
        );
        let mut multiline = input;
        multiline.multiline = true;
        assert!(command_submission_evidence(&terminal.snapshot(), &multiline, b"\r").is_none());
    }

    #[test]
    fn stale_marked_input_does_not_authorize_a_foreground_password_submission() {
        let mut terminal = TerminalCore::new(40, 4);
        terminal.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07sudo test\r\nPassword: ");
        let input = apply_terminal_input_data(&TerminalInputState::new(), "secret");
        assert!(command_submission_evidence(&terminal.snapshot(), &input, b"\r").is_none());
        terminal.advance(b"\x1b[?1049h$ echo inside");
        assert!(command_submission_evidence(&terminal.snapshot(), &input, b"\r").is_none());
    }

    #[test]
    fn marked_submission_uses_prompt_start_and_never_exposes_command_in_debug() {
        let mut terminal = TerminalCore::new(8, 4);
        terminal.advance(b"\x1b]133;A\x07long prompt\r\n$ \x1b]133;B\x07echo");
        let evidence =
            command_submission_evidence(&terminal.snapshot(), &TerminalInputState::new(), b"\r")
                .unwrap();
        assert_eq!(
            evidence.anchor,
            terminal.snapshot().shell_command_prompt.unwrap()
        );
        assert_eq!(evidence.command, "echo");
        assert!(!format!("{evidence:?}").contains("echo"));
    }
}
