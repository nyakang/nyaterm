//! Command history/suggestion runtime and quick command runtime.

mod command_runtime;
mod quick_command_runtime;
mod runtime_state;
mod staged;
mod state;

pub(in crate::features) use quick_command_runtime::{
    QUICK_COMMAND_COLOR_OPTIONS, encode_recorded_macro, quick_command_category_label,
    quick_command_sort_mode_from_setting, quick_command_view_mode_from_setting,
};
pub(in crate::features) use state::{
    CommandFeatureInit, CommandFeatureState, QuickCommandDropPosition, QuickCommandDropTarget,
    QuickCommandFeatureFocus,
};
