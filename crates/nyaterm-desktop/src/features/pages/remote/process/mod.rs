mod data;
mod details;
mod resources;
mod table;

// Widened past the page: `features::remote` needs the width -> mode mapping to
// constrain the sort key when the panel is resized, which used to happen in render.
pub(super) use data::process_row_height_px;
pub(in crate::features) use data::{ProcessDisplayMode, process_display_mode};
pub(super) use details::{ProcessDetailLabels, process_details};
pub(super) use resources::usage_color;
pub(super) use table::{
    ProcessTableLabels, ProcessTableRowActions, ProcessTableRowPresentation, process_sort_button,
    process_table_row,
};
