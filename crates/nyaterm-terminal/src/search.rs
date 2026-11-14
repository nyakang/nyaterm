//! Coordinates shared by terminal search workers and presentation.

#[derive(Debug, Clone)]
pub struct TerminalBufferMatch {
    pub line_index: usize,
    /// Half-open character column range on the matched line.
    pub start_col: usize,
    pub end_col: usize,
}
