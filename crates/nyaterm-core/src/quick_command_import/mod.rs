//! Schema-neutral quick-command import preparation and merging.
use crate::RiskLevel;

#[derive(Debug, Default)]
pub struct ImportSummary {
    pub imported_commands: usize,
    pub imported_categories: usize,
    pub updated_commands: usize,
    pub total_commands: usize,
    pub total_categories: usize,
}

#[derive(Default)]
pub struct ImportConfig {
    commands: Vec<ImportCommand>,
    categories: Vec<ImportCategory>,
}

struct ImportCategory {
    id: Option<String>,
    name: String,
    parent_id: Option<String>,
    sort_order: i32,
}

struct ImportCommand {
    id: Option<String>,
    label: String,
    command: String,
    preserve_command_text: bool,
    category_id: Option<String>,
    category: Option<String>,
    description: Option<String>,
    color_tag: Option<String>,
    icon_tag: Option<String>,
    pinned: Option<bool>,
    execution_mode: Option<String>,
    raw_send: Option<bool>,
    source: Option<String>,
    risk_level: Option<RiskLevel>,
    sort_order: Option<i32>,
}

impl ImportConfig {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

mod helpers;
mod json;
mod merge;
mod sources;
pub use merge::merge_import;
pub use sources::{
    MAX_QUICK_COMMAND_IMPORT_BYTES, decode_text, ensure_quick_command_import_size,
    parse_nyaterm_import, parse_windterm_quickbar, parse_xshell_quick_buttons_content,
};
#[cfg(test)]
mod tests;
