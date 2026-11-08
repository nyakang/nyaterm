//! Text-only suggestion plans; the caller must authorize shell replacement.

use nyaterm_core::completion::types::supports_full_command_fill;
use nyaterm_core::terminal::editing::supports_scalar_editing;
use nyaterm_core::terminal::input_tracker::TerminalInputState;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SuggestionFill<'a> {
    NoChange,
    Append(&'a str),
    Replace(&'a str),
    PasteReview(&'a str),
    Rejected(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SuggestionSubmitResult {
    NotHandled,
    Rejected,
    WriteFailed,
    Submitted,
    PasteReviewRequested,
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
    if !supports_full_command_fill(command) {
        return SuggestionFill::Rejected("Suggestion contains unsupported text");
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
    if command.contains(['\r', '\n']) {
        return SuggestionFill::PasteReview(command);
    }
    match command.strip_prefix(&input.value) {
        Some("") => SuggestionFill::NoChange,
        Some(suffix) => SuggestionFill::Append(suffix),
        None => SuggestionFill::Replace(command),
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::terminal::input_tracker::{MAX_TRACKED_INPUT_BYTES, TerminalInputState};

    use crate::features::terminal::command_suggestions::fill::{
        SuggestionFill, plan_suggestion_fill,
    };

    fn input(value: &str) -> TerminalInputState {
        TerminalInputState {
            value: value.into(),
            cursor: value.len(),
            ..TerminalInputState::default()
        }
    }

    #[test]
    fn suggestions_append_exact_suffixes_and_replace_non_prefix_matches() {
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
            assert_eq!(
                plan_suggestion_fill(&input(current), target),
                SuggestionFill::Replace(target)
            );
        }
    }

    #[test]
    fn multiline_commands_request_full_paste_review_even_for_prefix_matches() {
        for target in [
            "echo first\necho second",
            "echo first\\\r\n  second",
            "echo e\u{301}\n\t😀",
        ] {
            for current in ["", "echo", "快捷命令"] {
                assert_eq!(
                    plan_suggestion_fill(&input(current), target),
                    SuggestionFill::PasteReview(target)
                );
            }
        }
        assert!(matches!(
            plan_suggestion_fill(&input("echo"), "echo first\n\x1b[201~"),
            SuggestionFill::Rejected(_)
        ));
    }

    #[test]
    fn unsafe_command_content_and_tracking_states_are_rejected() {
        for target in [
            "",
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
