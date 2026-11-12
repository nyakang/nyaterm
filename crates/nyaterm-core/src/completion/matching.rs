//! Matcher scratch space and the adapter to the UI's scalar highlight indices.
use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use unicode_segmentation::UnicodeSegmentation;

pub(crate) fn matching_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.graphemes(true).filter_map(|g| g.chars().next())
}

pub struct TextMatcher {
    matcher: Matcher,
    query: Atom,
    haystack: Vec<char>,
    matched: Vec<u32>,
}

impl Default for TextMatcher {
    fn default() -> Self {
        let mut config = Config::DEFAULT;
        config.prefer_prefix = true;
        Self {
            matcher: Matcher::new(config),
            query: query_atom(""),
            haystack: Vec::new(),
            matched: Vec::new(),
        }
    }
}

impl TextMatcher {
    pub fn set_query(&mut self, query: &str) {
        self.query = query_atom(query);
    }

    fn prepare(&mut self, text: &str) {
        self.haystack.clear();
        self.haystack.extend(matching_chars(text));
    }

    /// Prepared catalog words share the exact grapheme representation used by
    /// `score`; highlighting still converts only selected words to scalar indices.
    pub(crate) fn score_prepared(&mut self, text: &[char]) -> Option<u32> {
        self.query
            .score(Utf32Str::Unicode(text), &mut self.matcher)
            .map(u32::from)
    }

    pub fn score(&mut self, text: &str) -> Option<u32> {
        if text.is_ascii() {
            return self
                .query
                .score(Utf32Str::Ascii(text.as_bytes()), &mut self.matcher)
                .map(u32::from);
        }
        self.prepare(text);
        self.query
            .score(Utf32Str::Unicode(&self.haystack), &mut self.matcher)
            .map(u32::from)
    }

    pub fn indices(&mut self, text: &str) -> Vec<u32> {
        self.prepare(text);
        self.matched.clear();
        self.query.indices(
            Utf32Str::Unicode(&self.haystack),
            &mut self.matcher,
            &mut self.matched,
        );
        let mut result = Vec::new();
        let mut scalar = 0u32;
        for (i, grapheme) in text.graphemes(true).enumerate() {
            let end = scalar + grapheme.chars().count() as u32;
            if self.matched.binary_search(&(i as u32)).is_ok() {
                result.extend(scalar..end);
            }
            scalar = end;
        }
        result
    }
}

fn query_atom(query: &str) -> Atom {
    // The low-level matcher requires a normalized needle. Atom keeps scoring
    // and highlighting on the same case/normalization policy without interpreting
    // literal command characters as query operators.
    Atom::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    )
}

#[cfg(test)]
mod tests {
    use crate::completion::matching::{TextMatcher, matching_chars};

    #[test]
    fn cached_unicode_scores_match_unprepared_scores() {
        let mut matcher = TextMatcher::default();
        for text in [
            "docker compose run 工具",
            "curl https://例子.test/路径",
            "echo e\u{301}x CAFÉ",
            "printf 👩\u{200d}💻 中文",
        ] {
            let prepared: Vec<_> = matching_chars(text).collect();
            for query in [
                "dcr", "工具", "curl h", "路径", "ex", "café", "中文", "zzzz",
            ] {
                matcher.set_query(query);
                assert_eq!(matcher.score_prepared(&prepared), matcher.score(text));
            }
        }
    }

    #[test]
    fn highlights_scalars_after_combining_characters() {
        let mut matcher = TextMatcher::default();
        matcher.set_query("x");
        assert_eq!(matcher.indices("e\u{301}x"), vec![2]);
        matcher.set_query("e");
        assert_eq!(matcher.indices("e\u{301}x"), vec![0, 1]);
    }

    #[test]
    fn case_and_accent_queries_share_scoring_and_highlight_normalization() {
        let mut matcher = TextMatcher::default();
        for (query, text, expected) in [
            ("git", "git status", vec![0, 1, 2]),
            ("GIT", "git status", vec![0, 1, 2]),
            ("Git", "Git status", vec![0, 1, 2]),
            ("CAFÉ", "café", vec![0, 1, 2, 3]),
            ("cafe", "café", vec![0, 1, 2, 3]),
            ("café", "café", vec![0, 1, 2, 3]),
            ("E", "e\u{301}", vec![0, 1]),
            ("工具", "echo 工具", vec![5, 6]),
        ] {
            matcher.set_query(query);
            let score = matcher.score(text);
            assert!(score.is_some(), "query {query:?}");
            assert_eq!(matcher.indices(text), expected, "query {query:?}");
            let prepared: Vec<_> = matching_chars(text).collect();
            assert_eq!(matcher.score_prepared(&prepared), score);
        }
        matcher.set_query("café");
        assert_eq!(matcher.score("cafe"), None);
        assert!(matcher.indices("cafe").is_empty());
        matcher.set_query("git");
        assert!(matcher.score("GIT").is_some());
        assert_eq!(matcher.indices("GIT"), vec![0, 1, 2]);
    }

    #[test]
    fn query_operators_and_escaped_spaces_remain_literal() {
        let mut matcher = TextMatcher::default();
        for query in ["!", "^", "$", "'", "foo bar", "foo\\ bar", "a|b"] {
            matcher.set_query(query);
            assert!(matcher.score(query).is_some(), "query {query:?}");
            assert_eq!(
                matcher.indices(query),
                (0..query.chars().count() as u32).collect::<Vec<_>>()
            );
            assert_eq!(matcher.score("unrelated"), None);
            assert!(matcher.indices("unrelated").is_empty());
        }
        matcher.set_query("foo\\ bar");
        assert_eq!(matcher.score("foo bar"), None);
        matcher.set_query("");
        assert!(matcher.score("anything").is_some());
        assert!(matcher.indices("anything").is_empty());
    }
}
