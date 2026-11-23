pub(crate) use nyaterm_remote_monitor_gpui::models::{
    DockerConfirmAction, DockerConfirmState, DockerTab, RemoteProcessSortDirection,
    RemoteProcessSortKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Appearance,
    Interaction,
    Keybindings,
    TerminalGeneral,
    RemoteMonitoring,
    Search,
    Translation,
    AiGeneral,
    AiModels,
    AiRules,
    AiAgents,
    Transfer,
    Security,
    SyncBackup,
}

impl SettingsTab {
    pub(crate) fn i18n_key(self) -> &'static str {
        match self {
            Self::General | Self::TerminalGeneral => "settings.general",
            Self::Appearance => "settings.appearance",
            Self::Interaction => "settings.interaction",
            Self::Keybindings => "settings.keybindings",
            Self::RemoteMonitoring => "settings.remoteMonitoring",
            Self::Search => "settings.search",
            Self::Translation => "settings.translation",
            Self::AiGeneral => "ai.general",
            Self::AiModels => "ai.models",
            Self::AiRules => "ai.rules",
            Self::AiAgents => "ai.localAgents",
            Self::Transfer => "settings.groupTransfer",
            Self::Security => "settings.groupSecurity",
            Self::SyncBackup => "settings.groupSyncBackup",
        }
    }

    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::General => "icons/settings/settings.svg",
            Self::Appearance => "icons/settings/palette.svg",
            Self::Interaction => "icons/settings/mouse.svg",
            Self::Keybindings => "icons/settings/keyboard.svg",
            Self::TerminalGeneral => "icons/settings/settings.svg",
            Self::RemoteMonitoring => "icons/resources.svg",
            Self::Search => "icons/settings/search.svg",
            Self::Translation => "icons/settings/translate.svg",
            Self::AiGeneral => "icons/settings/settings.svg",
            Self::AiModels => "icons/settings/model.svg",
            Self::AiRules => "icons/settings/rules.svg",
            Self::AiAgents => "icons/settings/agent.svg",
            Self::Transfer => "icons/settings/transfer.svg",
            Self::Security => "icons/settings/security.svg",
            Self::SyncBackup => "icons/settings/backup.svg",
        }
    }

    pub(crate) fn expandable_group_id(self) -> Option<&'static str> {
        match self {
            Self::General | Self::Appearance | Self::Interaction | Self::Keybindings => {
                Some("workspace")
            }
            Self::TerminalGeneral | Self::RemoteMonitoring | Self::Search | Self::Translation => {
                Some("terminal_session")
            }
            Self::AiGeneral | Self::AiModels | Self::AiRules | Self::AiAgents => Some("ai_group"),
            Self::Transfer | Self::Security | Self::SyncBackup => None,
        }
    }
}
