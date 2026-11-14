//! Text-only suggestion plans; the caller must authorize shell replacement.

use crate::completion::types::supports_full_command_fill;
use crate::terminal::editing::supports_scalar_editing;
use crate::terminal::input_tracker::TerminalInputState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionFillRejection {
    UnsupportedText,
    UnconfirmedInput,
}

#[derive(PartialEq, Eq)]
pub enum SuggestionFill<'a> {
    NoChange,
    Append(&'a str),
    Replace(&'a str),
    PasteReview(&'a str),
    Rejected(SuggestionFillRejection),
}

impl std::fmt::Debug for SuggestionFill<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Candidates may contain user commands; diagnostics expose only shape.
        match self {
            Self::NoChange => formatter.write_str("NoChange"),
            Self::Rejected(reason) => formatter.debug_tuple("Rejected").field(reason).finish(),
            Self::Append(text) => formatter
                .debug_struct("Append")
                .field("bytes", &text.len())
                .finish(),
            Self::Replace(text) => formatter
                .debug_struct("Replace")
                .field("bytes", &text.len())
                .finish(),
            Self::PasteReview(text) => formatter
                .debug_struct("PasteReview")
                .field("bytes", &text.len())
                .finish(),
        }
    }
}

/// Plan text changes only. The caller must authorize replacement and any execution.
pub fn plan_suggestion_fill<'a>(
    input: &TerminalInputState,
    command: &'a str,
) -> SuggestionFill<'a> {
    if !supports_full_command_fill(command) {
        return SuggestionFill::Rejected(SuggestionFillRejection::UnsupportedText);
    }
    if input.desynced
        || input.line_rewrite_required
        || input.multiline
        || input.paste_mode
        || input.cursor != input.value.len()
        || !supports_scalar_editing(&input.value)
    {
        return SuggestionFill::Rejected(SuggestionFillRejection::UnconfirmedInput);
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
    use crate::terminal::input_tracker::{MAX_TRACKED_INPUT_BYTES, TerminalInputState};

    use crate::completion::fill::{SuggestionFill, SuggestionFillRejection, plan_suggestion_fill};

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
    #[test]
    fn rejection_reasons_preserve_validation_precedence() {
        let invalid = TerminalInputState {
            desynced: true,
            ..input("echo")
        };
        assert_eq!(
            plan_suggestion_fill(&invalid, "echo\x1b[A"),
            SuggestionFill::Rejected(SuggestionFillRejection::UnsupportedText)
        );
        assert_eq!(
            plan_suggestion_fill(&invalid, "echo next"),
            SuggestionFill::Rejected(SuggestionFillRejection::UnconfirmedInput)
        );
    }

    #[test]
    fn plan_debug_exposes_shape_without_command_content() {
        for (plan, expected) in [
            (
                SuggestionFill::Append("private-command"),
                "Append { bytes: 15 }",
            ),
            (
                SuggestionFill::Replace("private-command"),
                "Replace { bytes: 15 }",
            ),
            (
                SuggestionFill::PasteReview("private-command"),
                "PasteReview { bytes: 15 }",
            ),
        ] {
            assert_eq!(format!("{plan:?}"), expected);
        }
    }
}
