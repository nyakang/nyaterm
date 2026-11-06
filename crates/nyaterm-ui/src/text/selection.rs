use std::ops::Range;

use gpui::{App, Hsla};
use gpui_base::text::{RangeHighlight, RenderedText};

use super::{NyaTextFormat, NyaTextState, document_content};

pub(super) struct RetainedRange {
    source: String,
    range: Range<usize>,
}
pub(super) struct AppliedRange {
    document: RenderedText,
    range: Range<usize>,
    color: Hsla,
}

impl NyaTextState {
    /// The parser replaces a changed tail block, including its native inline
    /// selection. Retain source byte endpoints until the next pointer gesture.
    pub(super) fn retain_current_selection(&self, previous: &str, next: &str, cx: &App) {
        let previous = document_content(self.format, previous);
        let next = document_content(self.format, next);
        if next.len() <= previous.len() || !next.starts_with(previous) {
            return;
        }
        if self.retained_selection.borrow().is_some() {
            return;
        }
        let state = self.inner.read(cx);
        let document = state.rendered_text();
        let source = document_content(self.format, document.source());
        if let Some(mut range) = state.selected_source_range() {
            if self.format != NyaTextFormat::Markdown {
                let prefix = document.source().len() - source.len();
                range.start = range.start.saturating_sub(prefix);
                range.end = range.end.saturating_sub(prefix);
            }
            if range.start < range.end && range.end <= source.len() {
                *self.retained_selection.borrow_mut() = Some(RetainedRange {
                    source: source.to_string(),
                    range,
                });
            }
        }
    }

    pub(super) fn has_retained_selection(&self) -> bool {
        self.retained_selection.borrow().is_some()
    }

    pub(super) fn retained_selected_text(&self, cx: &App) -> Option<String> {
        let retained = self.retained_selection.borrow();
        let retained = retained.as_ref()?;
        let document = self.inner.read(cx).rendered_text();
        // Literal wrapper rebuilds briefly have an empty committed document.
        // Its unchanged selected payload remains valid while parsing catches up.
        if !document_content(self.format, &self.requested_document.borrow())
            .starts_with(&retained.source)
        {
            return Some(String::new());
        }
        Some(if self.format == NyaTextFormat::Markdown {
            let selected = document
                .range_for_source(retained.range.clone())
                .and_then(|range| document.as_str().get(range))
                .unwrap_or_default();
            selected.strip_suffix('\n').unwrap_or(selected).to_string()
        } else {
            retained
                .source
                .get(retained.range.clone())
                .unwrap_or_default()
                .to_string()
        })
    }

    pub(super) fn prepare_retained_selection(&self, color: Hsla, cx: &mut App) {
        let retained = self.retained_selection.borrow();
        let Some(retained) = retained.as_ref() else {
            return;
        };
        let document = self.inner.read(cx).rendered_text();
        let prefix =
            document.source().len() - document_content(self.format, document.source()).len();
        let source_range = retained.range.start + prefix..retained.range.end + prefix;
        let Some(range) = document.range_for_source(source_range) else {
            return;
        };
        if self
            .applied_selection
            .borrow()
            .as_ref()
            .is_some_and(|applied| {
                applied.document == document && applied.range == range && applied.color == color
            })
        {
            return;
        }
        self.inner.update(cx, |state, cx| {
            let _ = state.set_range_highlights([RangeHighlight::new(range.clone(), color)], cx);
        });
        *self.applied_selection.borrow_mut() = Some(AppliedRange {
            document,
            range,
            color,
        });
    }

    pub(super) fn clear_retained_selection(&self, cx: &mut App) {
        if self.retained_selection.borrow_mut().take().is_none() {
            return;
        }
        self.applied_selection.borrow_mut().take();
        self.inner.update(cx, |state, cx| {
            state.clear_range_highlights(cx);
            state.clear_selection(cx);
        });
    }
}
