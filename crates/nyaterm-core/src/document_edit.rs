//! UI-independent document search, selection and atomic edit transactions.
use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl SearchOptions {
    pub fn compile(&self, query: &str) -> Result<Option<regex::Regex>, regex::Error> {
        if query.is_empty() {
            return Ok(None);
        }
        let pattern = if self.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let pattern = if self.whole_word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        regex::RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map(Some)
    }
    pub fn matches(&self, content: &str, query: &str) -> Result<Vec<Range<usize>>, regex::Error> {
        Ok(self
            .compile(query)?
            .map(|regex| {
                regex
                    .find_iter(content)
                    .map(|found| found.range())
                    .collect()
            })
            .unwrap_or_default())
    }
    pub fn replacements(
        &self,
        content: &str,
        query: &str,
        replacement: &str,
        index: Option<usize>,
    ) -> Result<Vec<TextEdit>, regex::Error> {
        let Some(regex) = self.compile(query)? else {
            return Ok(Vec::new());
        };
        Ok(regex
            .captures_iter(content)
            .enumerate()
            .filter(|(i, _)| index.is_none_or(|index| index == *i))
            .map(|(_, captures)| {
                let range = captures.get(0).expect("whole match").range();
                let mut text = String::new();
                if self.regex {
                    captures.expand(replacement, &mut text);
                } else {
                    text.push_str(replacement);
                }
                TextEdit { range, text }
            })
            .collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone)]
pub struct DocumentSnapshot {
    pub content: String,
    pub anchor: usize,
    pub head: usize,
    pub additional_selections: Vec<Range<usize>>,
}

pub struct TextDocument {
    pub content: String,
    pub anchor: usize,
    pub head: usize,
    pub additional_selections: Vec<Range<usize>>,
    pub undo_stack: Vec<DocumentSnapshot>,
    pub redo_stack: Vec<DocumentSnapshot>,
}

impl TextDocument {
    pub fn new(content: String, cursor: usize) -> Self {
        let cursor = boundary(&content, cursor);
        Self {
            content,
            anchor: cursor,
            head: cursor,
            additional_selections: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
    pub fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            content: self.content.clone(),
            anchor: self.anchor,
            head: self.head,
            additional_selections: self.additional_selections.clone(),
        }
    }
    pub fn checkpoint(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > 64 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }
    pub fn selections(&self) -> Vec<Range<usize>> {
        let mut selections = self.additional_selections.clone();
        selections.push(self.anchor.min(self.head)..self.anchor.max(self.head));
        normalize_selections(&self.content, selections)
    }
    /// Validate the entire transaction before mutating, then apply back-to-front.
    pub fn apply(&mut self, mut edits: Vec<TextEdit>) -> Result<(), &'static str> {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut previous_end = 0;
        for edit in &edits {
            if edit.range.start < previous_end
                || edit.range.start > edit.range.end
                || edit.range.end > self.content.len()
                || !self.content.is_char_boundary(edit.range.start)
                || !self.content.is_char_boundary(edit.range.end)
            {
                return Err("invalid or overlapping edit ranges");
            }
            previous_end = edit.range.end;
        }
        if edits.is_empty() {
            return Ok(());
        }
        self.checkpoint();
        let mut shift = 0isize;
        let mut cursors = Vec::new();
        for edit in &edits {
            let cursor = (edit.range.start as isize + shift) as usize + edit.text.len();
            cursors.push(cursor..cursor);
            shift += edit.text.len() as isize - edit.range.len() as isize;
        }
        for edit in edits.iter().rev() {
            self.content.replace_range(edit.range.clone(), &edit.text);
        }
        let primary = cursors.pop().expect("nonempty edits");
        self.anchor = primary.start;
        self.head = primary.end;
        self.additional_selections = cursors;
        Ok(())
    }
    pub fn replace_selections(&mut self, text: &str) -> Result<(), &'static str> {
        let edits = self
            .selections()
            .into_iter()
            .map(|range| TextEdit {
                range,
                text: text.to_owned(),
            })
            .collect();
        self.apply(edits)
    }
}

pub fn normalize_selections(content: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut ranges = ranges
        .into_iter()
        .map(|range| {
            let start = boundary(content, range.start.min(range.end));
            start..boundary(content, range.end.max(range.start))
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut result: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = result.last_mut()
            && (range.start < last.end
                || range == *last
                || (range.is_empty() && range.start == last.end))
        {
            last.end = last.end.max(range.end);
        } else {
            result.push(range);
        }
    }
    result
}

pub fn rectangle_selections(content: &str, anchor: usize, head: usize) -> Vec<Range<usize>> {
    let position = |offset| {
        let offset = boundary(content, offset);
        let before = &content[..offset];
        (
            before.bytes().filter(|byte| *byte == b'\n').count(),
            before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .count(),
        )
    };
    let (start_line, start_col) = position(anchor);
    let (end_line, end_col) = position(head);
    let mut offset = 0;
    let mut result = Vec::new();
    for (line, text) in content.split_inclusive('\n').enumerate() {
        if (start_line.min(end_line)..=start_line.max(end_line)).contains(&line) {
            let text = text.strip_suffix('\n').unwrap_or(text);
            let column = |col| {
                text.char_indices()
                    .nth(col)
                    .map_or(text.len(), |(byte, _)| byte)
            };
            result.push(
                offset + column(start_col.min(end_col))..offset + column(start_col.max(end_col)),
            );
        }
        offset += text.len();
    }
    result
}

fn boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::{
        SearchOptions, TextDocument, TextEdit, normalize_selections, rectangle_selections,
    };

    #[test]
    fn regex_zero_width_unicode_replacements_finish_in_one_transaction() {
        let mut document = TextDocument::new("中\n文".into(), 0);
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        document
            .apply(
                options
                    .replacements(&document.content, "^", ">", None)
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(document.content, ">中\n>文");
        assert_eq!(document.undo_stack.len(), 1);
        assert_eq!(document.undo_stack[0].content, "中\n文");
    }
    #[test]
    fn replacements_expand_captures_only_in_regex_mode() {
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        assert_eq!(
            options
                .replacements("port=22", "(port)=(\\d+)", "$1: $2", None)
                .unwrap()[0]
                .text,
            "port: 22"
        );
        assert_eq!(
            SearchOptions::default()
                .replacements("port", "port", "$1", None)
                .unwrap()[0]
                .text,
            "$1"
        );
    }
    #[test]
    fn overlapping_multi_cursor_selections_merge_before_typing() {
        let mut document = TextDocument::new("abcdef".into(), 0);
        document.head = 3;
        document.additional_selections = vec![2..5, 6..6, 6..6];
        document.replace_selections("X").unwrap();
        assert_eq!(document.content, "XfX");
        assert_eq!(document.undo_stack.len(), 1);
        assert_eq!(
            normalize_selections("中文", std::iter::once(1..3).collect()),
            vec![0..3]
        );
    }
    #[test]
    fn invalid_transaction_leaves_document_and_history_unchanged() {
        let mut document = TextDocument::new("中文".into(), 0);
        assert!(
            document
                .apply(vec![TextEdit {
                    range: 1..2,
                    text: "x".into()
                }])
                .is_err()
        );
        assert_eq!(document.content, "中文");
        assert!(document.undo_stack.is_empty());
        assert_eq!(
            rectangle_selections("abc\n中文d\nxy", 1, 14),
            vec![1..2, 7..10, 13..14]
        );
    }
}

#[derive(Default)]
pub struct FoldProjection {
    pub text: String,
    segments: Vec<(Range<usize>, Range<usize>, bool)>,
}

impl FoldProjection {
    pub fn new(content: &str, mut folds: Vec<Range<usize>>) -> Self {
        folds.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
        let mut projection = Self::default();
        let mut cursor = 0;
        for range in folds {
            if range.start < cursor || range.start >= range.end || range.end > content.len() {
                continue;
            }
            if !content.is_char_boundary(range.start) || !content.is_char_boundary(range.end) {
                continue;
            }
            let start = projection.text.len();
            projection.text.push_str(&content[cursor..range.start]);
            projection
                .segments
                .push((cursor..range.start, start..projection.text.len(), false));
            let start = projection.text.len();
            projection.text.push_str("…\n");
            projection
                .segments
                .push((range.clone(), start..projection.text.len(), true));
            cursor = range.end;
        }
        let start = projection.text.len();
        projection.text.push_str(&content[cursor..]);
        projection
            .segments
            .push((cursor..content.len(), start..projection.text.len(), false));
        projection
    }
    pub fn display_offset(&self, source: usize) -> usize {
        for (input, output, hidden) in &self.segments {
            if input.contains(&source) {
                return output.start + if *hidden { 0 } else { source - input.start };
            }
        }
        self.text.len()
    }
    pub fn source_offset(&self, display: usize) -> usize {
        for (input, output, hidden) in &self.segments {
            if output.contains(&display) {
                return input.start + if *hidden { 0 } else { display - output.start };
            }
        }
        self.segments.last().map_or(0, |(input, _, _)| input.end)
    }
}

#[cfg(test)]
mod fold_tests {
    use super::FoldProjection;
    #[test]
    fn folded_projection_maps_utf8_offsets_without_mutating_source() {
        let source = "head\n中文\nend\n";
        let projection = FoldProjection::new(source, std::iter::once(5..12).collect());
        assert_eq!(projection.text, "head\n…\nend\n");
        assert_eq!(projection.display_offset(8), 5);
        assert_eq!(projection.display_offset(12), 9);
        assert_eq!(projection.source_offset(9), 12);
        assert_eq!(projection.source_offset(5), 5);
        assert_eq!(source, "head\n中文\nend\n");
    }
}
