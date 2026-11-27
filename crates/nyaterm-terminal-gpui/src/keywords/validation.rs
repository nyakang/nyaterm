//! Editor diagnostics use the same normalization as the runtime matcher.

use super::matching::keyword_pattern_source;

/// A one-based pattern line and its regex compiler diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordPatternDiagnostic {
    pub line: usize,
    pub message: String,
}

/// Check the patterns of a rule against the same options the runtime matcher
/// uses.
///
/// A pattern that is not a regular expression matches literally, so it never
/// produces a diagnostic.
pub fn validate_keyword_highlight_patterns(
    patterns: &[String],
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
) -> Vec<KeywordPatternDiagnostic> {
    patterns
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                return None;
            }
            let source = keyword_pattern_source(pattern, case_sensitive, whole_word, use_regex);
            regex::Regex::new(&source)
                .err()
                .map(|error| KeywordPatternDiagnostic {
                    line: index + 1,
                    message: error.to_string(),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::validate_keyword_highlight_patterns;

    #[test]
    fn diagnostics_keep_blank_line_offsets_and_accept_runtime_case_flags() {
        let patterns = ["", "(?-i)ERROR", "[", "(?i)warning", "中文"].map(str::to_string);
        let errors = validate_keyword_highlight_patterns(&patterns, false, false, true);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 3);
        assert!(!errors[0].message.is_empty());
    }

    #[test]
    fn escaped_terminal_selections_are_valid_literal_patterns() {
        for text in ["192.168.1.1", "[ERROR]", "[", r"C:\work\file", "中文(a+b)"] {
            let pattern = regex::escape(text);
            assert!(
                validate_keyword_highlight_patterns(
                    std::slice::from_ref(&pattern),
                    false,
                    false,
                    true
                )
                .is_empty()
            );
            assert!(
                regex::Regex::new(&format!("^(?:{pattern})$"))
                    .unwrap()
                    .is_match(text)
            );
        }
    }

    #[test]
    fn a_literal_pattern_never_reports_a_regex_error() {
        // The pattern is broken as a regular expression but matches literally.
        let patterns = vec!["[".to_string()];
        assert!(
            validate_keyword_highlight_patterns(&patterns, false, false, true).len() == 1,
            "a regular expression still reports its error"
        );
        assert!(
            validate_keyword_highlight_patterns(&patterns, false, false, false).is_empty(),
            "a literal pattern matches without a diagnostic"
        );
    }

    #[test]
    fn word_boundaries_and_case_sensitivity_stay_valid() {
        let patterns = vec!["ERROR".to_string()];
        assert!(
            validate_keyword_highlight_patterns(&patterns, true, true, true).is_empty(),
            "word boundaries around a literal word are valid"
        );
        assert!(
            validate_keyword_highlight_patterns(&patterns, false, true, false).is_empty(),
            "an escaped literal with word boundaries is valid"
        );
    }
}
