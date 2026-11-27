//! Shared helpers for the matching options of a keyword highlight rule.
//!
//! A rule narrows its patterns down in three ways: it can match case
//! sensitively, it can require word boundaries and it can treat the pattern
//! as a literal instead of a regular expression. The helpers here turn those
//! options into the regular expression source that the paint path compiles.

/// Build the regular expression source of a single pattern.
///
/// * `use_regex` off escapes the pattern, so its metacharacters match
///   literally.
/// * `whole_word` anchors the pattern to word boundaries.
/// * `case_sensitive` off matches case insensitively. A pattern that carries
///   its own inline flag keeps it, so an explicit `(?-i)` still wins over the
///   option.
pub fn keyword_pattern_source(
    pattern: &str,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
) -> String {
    let mut source = if use_regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    if whole_word {
        source = format!(r"\b(?:{source})\b");
    }
    if case_sensitive || keyword_pattern_has_inline_case_flag(&source) {
        source
    } else {
        format!("(?i){source}")
    }
}

/// Whether the pattern already states its own case behaviour.
pub fn keyword_pattern_has_inline_case_flag(pattern: &str) -> bool {
    pattern.contains("(?i)") || pattern.contains("(?-i)")
}

#[cfg(test)]
mod tests {
    use super::keyword_pattern_source;

    #[test]
    fn whole_word_applies_to_every_alternative() {
        let re =
            regex::Regex::new(&keyword_pattern_source("error|warning", false, true, true)).unwrap();
        for text in ["error", "WARNING", "an error!"] {
            assert!(re.is_match(text), "{text}");
        }
        for text in ["errors", "prewarning", "error_", "错误error"] {
            assert!(!re.is_match(text), "{text}");
        }
    }

    #[test]
    fn whole_word_preserves_literal_unicode_and_inline_flags() {
        for (pattern, text, case_sensitive, use_regex, expected) in [
            ("c++", "c++", false, false, false),
            ("error|warning", "error|warning", false, false, true),
            ("错误", " 错误!", false, false, true),
            ("错误", "错误码", false, false, false),
            ("(?-i)ERROR|WARN", "error", false, true, false),
            ("(?i)error|warn", "WARN", true, true, true),
        ] {
            let re = regex::Regex::new(&keyword_pattern_source(
                pattern,
                case_sensitive,
                true,
                use_regex,
            ))
            .unwrap();
            assert_eq!(re.is_match(text), expected, "{pattern} / {text}");
        }
    }
}
