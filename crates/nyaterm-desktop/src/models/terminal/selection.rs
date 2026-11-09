//! Terminal cell coordinates and absolute-buffer selection state.

pub(crate) use nyaterm_terminal::selection::{
    TerminalBufferCellPos, TerminalCellPos, TerminalSelection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSelectionTextError {
    MissingSession,
    StaleHistory,
    InvalidRange,
    WorkerUnavailable,
}
