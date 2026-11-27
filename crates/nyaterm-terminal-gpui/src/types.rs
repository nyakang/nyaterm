//! Shared terminal paint types.

use gpui::{Bounds, Pixels};

#[derive(Clone, Copy)]
pub(super) struct TerminalPaintGeometry {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) visual_y_offset: f32,
    pub(super) cell_width: f32,
    pub(super) cell_height: f32,
}

#[derive(Clone)]
pub(super) struct TerminalHighlightSpan {
    pub(super) text: String,
    pub(super) color: Option<u32>,
    pub(super) bg: Option<u32>,
    pub(super) keyword: bool,
    pub(super) underline: bool,
    pub(super) strikeout: bool,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalKeywordRange {
    pub(super) start_col: usize,
    pub(super) end_col: usize,
    pub(super) color: u32,
    /// Explicit highlight background from the rule, if any.
    pub(super) background: Option<u32>,
}
