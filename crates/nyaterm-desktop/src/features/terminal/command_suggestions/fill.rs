//! Desktop submission results and presentation of pure planning rejections.
use nyaterm_core::completion::fill::SuggestionFillRejection;

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

pub(super) fn suggestion_fill_rejection_message(reason: SuggestionFillRejection) -> &'static str {
    match reason {
        SuggestionFillRejection::UnsupportedText => "Suggestion contains unsupported text",
        SuggestionFillRejection::UnconfirmedInput => {
            "Input is not confirmed at the end of the line"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::suggestion_fill_rejection_message;
    use nyaterm_core::completion::fill::SuggestionFillRejection;

    #[test]
    fn planning_rejections_preserve_status_messages() {
        assert_eq!(
            suggestion_fill_rejection_message(SuggestionFillRejection::UnsupportedText),
            "Suggestion contains unsupported text"
        );
        assert_eq!(
            suggestion_fill_rejection_message(SuggestionFillRejection::UnconfirmedInput),
            "Input is not confirmed at the end of the line"
        );
    }
}
