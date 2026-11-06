//! Matcher scratch space and the adapter to the UI's scalar highlight indices.
use nucleo_matcher::{Config, Matcher, Utf32Str};
use unicode_segmentation::UnicodeSegmentation;

pub struct TextMatcher {
    matcher: Matcher,
    needle: Vec<char>,
    ascii_needle: Option<Vec<u8>>,
    haystack: Vec<char>,
    matched: Vec<u32>,
}

impl Default for TextMatcher {
    fn default() -> Self {
        let mut config = Config::DEFAULT;
        config.prefer_prefix = true;
        Self {
            matcher: Matcher::new(config),
            needle: Vec::new(),
            ascii_needle: None,
            haystack: Vec::new(),
            matched: Vec::new(),
        }
    }
}

impl TextMatcher {
    pub fn set_query(&mut self, query: &str) {
        if query.is_ascii() {
            let bytes = self.ascii_needle.get_or_insert_with(Vec::new);
            bytes.clear();
            bytes.extend_from_slice(query.as_bytes());
        } else {
            self.ascii_needle = None;
        }
        self.needle.clear();
        self.needle
            .extend(query.graphemes(true).filter_map(|g| g.chars().next()));
    }

    fn prepare(&mut self, text: &str) {
        self.haystack.clear();
        self.haystack
            .extend(text.graphemes(true).filter_map(|g| g.chars().next()));
    }

    pub fn score(&mut self, text: &str) -> Option<u32> {
        if text.is_ascii() {
            let needle = self
                .ascii_needle
                .as_ref()
                .map_or(Utf32Str::Unicode(&self.needle), |bytes| {
                    Utf32Str::Ascii(bytes)
                });
            return self
                .matcher
                .fuzzy_match(Utf32Str::Ascii(text.as_bytes()), needle)
                .map(u32::from);
        }
        self.prepare(text);
        self.matcher
            .fuzzy_match(
                Utf32Str::Unicode(&self.haystack),
                Utf32Str::Unicode(&self.needle),
            )
            .map(u32::from)
    }

    pub fn indices(&mut self, text: &str) -> Vec<u32> {
        self.prepare(text);
        self.matched.clear();
        self.matcher.fuzzy_indices(
            Utf32Str::Unicode(&self.haystack),
            Utf32Str::Unicode(&self.needle),
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

#[cfg(test)]
mod tests {
    use crate::completion::matching::TextMatcher;
    #[test]
    fn highlights_scalars_after_combining_characters() {
        let mut matcher = TextMatcher::default();
        matcher.set_query("x");
        assert_eq!(matcher.indices("e\u{301}x"), vec![2]);
        matcher.set_query("e");
        assert_eq!(matcher.indices("e\u{301}x"), vec![0, 1]);
    }
}
