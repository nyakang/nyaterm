//! Fuzzy search using nucleo-matcher.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;

/// Single fuzzy match with score and highlighted character indices.
#[derive(Debug, Clone, Serialize)]
pub struct FuzzyResult {
    pub command: String,
    pub score: u32,
    pub indices: Vec<u32>,
    /// Provider tag: "history", "quickCommand", etc.
    pub source: String,
    /// Text shown in the suggestion panel (may differ from `command`).
    pub display: String,
}

/// Generic fuzzy search over `(display_text, value)` pairs.
///
/// Matches against `display_text` (shown in the suggestion panel).
/// `value` is stored as `command` in the result (used when filling/executing).
/// `source` tags every result so the frontend can distinguish providers.
pub fn fuzzy_search_items(
    items: &[(&str, &str)],
    pattern_str: &str,
    source: &str,
    limit: usize,
) -> Vec<FuzzyResult> {
    let pattern_str = pattern_str.trim();
    if pattern_str.is_empty() {
        return Vec::new();
    }

    let pattern = Pattern::new(
        pattern_str,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    if pattern.atoms.is_empty() {
        return Vec::new();
    }

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();

    let mut scored: Vec<(usize, u32)> = Vec::new();
    for (idx, (display, _value)) in items.iter().enumerate() {
        let haystack = Utf32Str::new(display, &mut buf);
        if let Some(score) = pattern.score(haystack, &mut matcher) {
            scored.push((idx, score));
        }
    }

    scored.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    scored.truncate(limit);

    let mut results = Vec::with_capacity(scored.len());
    for (idx, score) in scored {
        let (display, value) = &items[idx];
        let haystack = Utf32Str::new(display, &mut buf);
        let mut indices = Vec::new();
        pattern.indices(haystack, &mut matcher, &mut indices);
        indices.sort_unstable();
        indices.dedup();

        results.push(FuzzyResult {
            command: (*value).to_string(),
            score,
            indices,
            source: source.to_string(),
            display: (*display).to_string(),
        });
    }

    results
}
