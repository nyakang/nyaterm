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
        source = format!(r"\b{source}\b");
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
