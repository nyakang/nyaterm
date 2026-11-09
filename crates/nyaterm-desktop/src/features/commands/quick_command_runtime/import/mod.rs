use crate::models::QuickCommandImportPathPromptKind;

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
mod sources;
