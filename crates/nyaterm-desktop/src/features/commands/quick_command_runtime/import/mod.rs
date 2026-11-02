use nyaterm_core::RiskLevel;

use crate::models::QuickCommandImportPathPromptKind;

#[derive(Debug, Default)]
struct ImportSummary {
    imported_commands: usize,
    imported_categories: usize,
    updated_commands: usize,
    total_commands: usize,
    total_categories: usize,
}

#[derive(Default)]
struct ImportConfig {
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

impl QuickCommandImportPathPromptKind {
    /// i18n key for the native file picker's prompt. The picker is OS chrome the
    /// user reads, so it has to be localized; resolving happens at the call site,
    /// which owns `tr`.
    fn prompt_label_key(self) -> &'static str {
        match self {
            Self::NyatermJson => "quickCommands.importPromptNyaTermJson",
            Self::WindTermQuickbar => "quickCommands.importPromptWindTerm",
            Self::XshellXts => "quickCommands.importPromptXshell",
        }
    }

    fn selecting_status(self) -> &'static str {
        match self {
            Self::NyatermJson => "selecting quick command JSON import file",
            Self::WindTermQuickbar => "selecting WindTerm quickbar import file",
            Self::XshellXts => "selecting Xshell quick button import file",
        }
    }
}

mod dialog;
mod helpers;
mod json;
mod merge;
mod sources;

#[cfg(test)]
mod tests;
