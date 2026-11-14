//! Ordered terminal frame processing. Session internals are transferred opaquely.
mod frame_commands;
mod frame_events;
mod frame_pipeline;
mod frame_search;
mod frame_worker;

pub use frame_commands::TerminalSelectionTextResult;
pub use frame_events::{
    TerminalFrameEvent, TerminalFrameOutputEvent, TerminalFrameSearchEvent,
    TerminalFrameSnapshotEvent, TerminalFrameSnapshotPurpose,
};
pub use frame_pipeline::{TerminalFrameOutputSubmission, TerminalFramePipeline};
pub use frame_search::{
    TerminalFrameSearchKey, TerminalFrameSearchPurpose, TerminalFrameSearchResult,
    terminal_frame_search_result_is_current,
};
pub use frame_worker::TerminalFrameSession;

use crate::work_policy::{TerminalPresentation, TerminalWorkPolicy};
use nyaterm_terminal::action_links::{
    TerminalFrameActionLinks, prepare_terminal_frame_action_links_reusing,
};
use nyaterm_terminal::output::{
    merge_terminal_effects, protect_terminal_output_burst, trim_string_to_tail,
};
use nyaterm_terminal::protocol::TerminalProtocolState;
use nyaterm_terminal::selection::TerminalSelection;
use nyaterm_terminal::snapshot_window::{
    terminal_frame_live_snapshot_with_stats, terminal_frame_snapshot_with_scroll_window_and_stats,
};

const TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSelectionTextError {
    MissingSession,
    StaleHistory,
    InvalidRange,
    WorkerUnavailable,
}

fn append_terminal_frame_visible_tail(output: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    output.push_str(text);
    trim_string_to_tail(output, TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod selection_text_tests;
