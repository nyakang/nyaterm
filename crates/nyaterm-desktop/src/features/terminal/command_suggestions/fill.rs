//! Suggestions may append confirmed text, but never infer remote editing keys.

use nyaterm_core::terminal::editing::supports_scalar_editing;
use nyaterm_core::terminal::input_tracker::{MAX_TRACKED_INPUT_BYTES, TerminalInputState};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SuggestionFill<'a> {
    NoChange,
    Append(&'a str),
    Rejected(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SuggestionSubmitResult {
    NotHandled,
    Rejected,
    WriteFailed,
    Submitted,
    NoChange,
}

impl SuggestionSubmitResult {
    pub(super) fn handled(&self) -> bool {
        *self != Self::NotHandled
    }
}

pub(super) fn plan_suggestion_fill<'a>(
    input: &TerminalInputState,
    command: &'a str,
) -> SuggestionFill<'a> {
    if command.is_empty()
        || command.len() > MAX_TRACKED_INPUT_BYTES
        || !supports_scalar_editing(command)
    {
        return SuggestionFill::Rejected("Suggestion must be supported single-line text");
    }
    if input.desynced
        || input.line_rewrite_required
        || input.multiline
        || input.paste_mode
        || input.cursor != input.value.len()
        || !supports_scalar_editing(&input.value)
    {
        return SuggestionFill::Rejected("Input is not confirmed at the end of the line");
    }
    match command.strip_prefix(&input.value) {
        Some("") => SuggestionFill::NoChange,
        Some(suffix) => SuggestionFill::Append(suffix),
        None => SuggestionFill::Rejected("Cannot safely replace input; edit the command manually"),
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::terminal::input_tracker::{MAX_TRACKED_INPUT_BYTES, TerminalInputState};

    use super::{SuggestionFill, plan_suggestion_fill};

    fn input(value: &str) -> TerminalInputState {
        TerminalInputState {
            value: value.into(),
            cursor: value.len(),
            ..TerminalInputState::default()
        }
    }

    #[test]
    fn suggestions_only_append_an_exact_raw_suffix() {
        for (current, target, suffix) in [
            ("display ver", "display version", "sion"),
            (" echo ", " echo  x ", " x "),
            ("echo 中", "echo 中文", "文"),
            ("", "echo x", "echo x"),
        ] {
            assert_eq!(
                plan_suggestion_fill(&input(current), target),
                SuggestionFill::Append(suffix)
            );
        }
        assert_eq!(
            plan_suggestion_fill(&input("display version"), "display version"),
            SuggestionFill::NoChange
        );
        for (current, target) in [
            ("show ver", "display version"),
            (" echo", "echo x"),
            ("echo ", "echo"),
        ] {
            assert!(matches!(
                plan_suggestion_fill(&input(current), target),
                SuggestionFill::Rejected(_)
            ));
        }
    }

    #[test]
    fn unsafe_command_content_and_tracking_states_are_rejected() {
        for target in [
            "",
            "echo\nhi",
            "echo\rhi",
            "echo\t",
            "echo\x1b[A",
            "echo\x7f",
            "e\u{301}",
            "👩\u{200d}💻",
        ] {
            assert!(matches!(
                plan_suggestion_fill(&input(""), target),
                SuggestionFill::Rejected(_)
            ));
        }
        assert!(matches!(
            plan_suggestion_fill(&input(""), &"x".repeat(MAX_TRACKED_INPUT_BYTES + 1)),
            SuggestionFill::Rejected(_)
        ));
        for invalid in [
            TerminalInputState {
                desynced: true,
                ..input("echo")
            },
            TerminalInputState {
                line_rewrite_required: true,
                ..input("echo")
            },
            TerminalInputState {
                multiline: true,
                ..input("echo")
            },
            TerminalInputState {
                paste_mode: true,
                ..input("echo")
            },
            TerminalInputState {
                cursor: 1,
                ..input("echo")
            },
        ] {
            assert!(matches!(
                plan_suggestion_fill(&invalid, "echo x"),
                SuggestionFill::Rejected(_)
            ));
        }
    }
}
