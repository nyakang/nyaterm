//! Editor diagnostics use the same normalization as the runtime matcher.

use super::keyword_pattern_with_default_case;

/// A one-based pattern line and its regex compiler diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordPatternDiagnostic {
    pub line: usize,
    pub message: String,
}

pub fn validate_keyword_highlight_patterns(patterns: &[String]) -> Vec<KeywordPatternDiagnostic> {
    patterns
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                return None;
            }
            regex::Regex::new(&keyword_pattern_with_default_case(pattern))
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
        let errors = validate_keyword_highlight_patterns(&patterns);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 3);
        assert!(!errors[0].message.is_empty());
    }

    #[test]
    fn escaped_terminal_selections_are_valid_literal_patterns() {
        for text in ["192.168.1.1", "[ERROR]", "[", r"C:\work\file", "中文(a+b)"] {
            let pattern = regex::escape(text);
            assert!(validate_keyword_highlight_patterns(std::slice::from_ref(&pattern)).is_empty());
            assert!(
                regex::Regex::new(&format!("^(?:{pattern})$"))
                    .unwrap()
                    .is_match(text)
            );
        }
    }
}
