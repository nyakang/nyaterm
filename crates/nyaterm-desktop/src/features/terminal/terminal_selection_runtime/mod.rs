/// Approximate monospaced cell metrics used for hit-testing the painted terminal grid.
/// Keep in sync with the row height `nyaterm-terminal-gpui` paints and the
/// surface font size.
const CELL_WIDTH_RATIO: f32 = 0.62;
const LINE_HEIGHT_RATIO: f32 = 1.25;

mod helpers;
pub(in crate::features) use helpers::{
    terminal_bounds_tracker, terminal_bounds_tracker_with_exclusion,
};

mod action_links;
mod metrics;
pub(in crate::features) use metrics::{
    measure_terminal_font, terminal_font_with_features, terminal_gutter_metrics,
    terminal_line_number_digits,
};
mod selection;
mod smart_input;
mod text;
