pub(super) use nyaterm_terminal_gpui::ime::utf16_byte_range;
use std::ops::Range;

/// The terminal retains only uncommitted IME text, never a copy of remote input.
#[derive(Default)]
pub(super) struct TerminalImeState {
    text: String,
    selection: Range<usize>,
    session_id: Option<String>,
}

impl TerminalImeState {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub(super) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(super) fn is_active_for(&self, session_id: Option<&str>) -> bool {
        !self.text.is_empty() && self.session_id.as_deref() == session_id
    }

    pub(super) fn clear(&mut self) -> Option<String> {
        self.text.clear();
        self.selection = 0..0;
        self.session_id.take()
    }

    pub(super) fn replace(
        &mut self,
        session_id: &str,
        replacement: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        if self.session_id.as_deref() != Some(session_id) {
            self.clear();
        }
        let range = replacement
            .map(|range| utf16_byte_range(&self.text, range))
            .unwrap_or(0..self.text.len());
        let prefix_len = self.text[..range.start].encode_utf16().count();
        let selection = selected.unwrap_or_else(|| {
            let len = text.encode_utf16().count();
            len..len
        });
        let selection = utf16_byte_range(text, selection);
        self.selection = prefix_len + text[..selection.start].encode_utf16().count()
            ..prefix_len + text[..selection.end].encode_utf16().count();
        self.text.replace_range(range, text);
        self.session_id = Some(session_id.to_owned());
        if self.text.is_empty() {
            self.clear();
        }
    }

    pub(super) fn select(&mut self, range: Range<usize>) {
        let range = utf16_byte_range(&self.text, range);
        self.selection = self.text[..range.start].encode_utf16().count()
            ..self.text[..range.end].encode_utf16().count();
    }

    pub(super) fn text_for_range(&self, range: Range<usize>) -> (String, Range<usize>) {
        let bytes = utf16_byte_range(&self.text, range);
        let adjusted = self.text[..bytes.start].encode_utf16().count()
            ..self.text[..bytes.end].encode_utf16().count();
        (self.text[bytes].to_owned(), adjusted)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalImeState;

    #[test]
    fn ime_defaults_to_end_and_tracks_explicit_cursor_and_partial_replacement() {
        let mut ime = TerminalImeState::default();
        ime.replace("s1", None, "zhong", None);
        assert_eq!(ime.selection(), 5..5);
        ime.replace("s1", None, "zhong", Some(2..2));
        assert_eq!(ime.selection(), 2..2);
        ime.replace("s1", Some(2..5), "中文", Some(1..1));
        assert_eq!(ime.text(), "zh中文");
        assert_eq!(ime.selection(), 3..3);
        assert!(ime.is_active_for(Some("s1")));
        assert!(!ime.is_active_for(Some("s2")));
    }

    #[test]
    fn ime_ranges_clamp_without_splitting_surrogate_pairs() {
        let mut ime = TerminalImeState::default();
        ime.replace("s1", None, "a😀中", Some(2..3));
        assert_eq!(ime.selection(), 1..3);
        assert_eq!(ime.text_for_range(2..3), ("😀".into(), 1..3));
        ime.select(usize::MAX..usize::MAX);
        assert_eq!(ime.selection(), 4..4);
        ime.select(Range { start: 4, end: 1 });
        assert_eq!(ime.selection(), 4..4);
        ime.replace("s1", Some(1..3), "x", None);
        assert_eq!(ime.text(), "ax中");
        assert_eq!(ime.selection(), 2..2);
    }

    #[test]
    fn ime_clear_empty_update_and_session_change_reset_selection() {
        let mut ime = TerminalImeState::default();
        ime.replace("s1", None, "old", None);
        ime.replace("s2", Some(1..2), "new", None);
        assert_eq!(ime.text(), "new");
        assert_eq!(ime.session_id(), Some("s2"));
        assert_eq!(ime.clear().as_deref(), Some("s2"));
        assert_eq!(ime.selection(), 0..0);
        ime.replace("s1", None, "", Some(99..100));
        assert_eq!(ime.session_id(), None);
        assert_eq!(ime.selection(), 0..0);
    }

    use std::ops::Range;
}
