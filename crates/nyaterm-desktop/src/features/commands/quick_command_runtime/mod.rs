mod escapes;
pub(in crate::features) use escapes::encode_recorded_macro;
mod import;
mod variables;

pub(in crate::features) const QUICK_COMMAND_COLOR_OPTIONS: [Option<&str>; 6] = [
    None,
    Some("red"),
    Some("green"),
    Some("blue"),
    Some("yellow"),
    Some("purple"),
];

mod helpers;
pub(in crate::features) use helpers::{
    quick_command_category_label, quick_command_sort_mode_from_setting,
    quick_command_view_mode_from_setting,
};

mod catalog;
mod dialogs;
mod editor;
mod run;
mod window;
