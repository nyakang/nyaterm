#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteProcessSortKey {
    Cpu,
    Memory,
    Pid,
    User,
    Command,
}

impl RemoteProcessSortKey {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Pid => "PID",
            Self::User => "User",
            Self::Command => "Command",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteProcessSortDirection {
    Ascending,
    Descending,
}

impl RemoteProcessSortDirection {
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }

    pub(crate) fn reversed(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DockerConfirmAction {
    ContainerAction {
        container_id: String,
        action: &'static str,
    },
    ImageRemove {
        image_id: String,
        force: bool,
    },
    VolumeRemove {
        volume_name: String,
        force: bool,
    },
    NetworkRemove {
        network_id: String,
    },
    ComposeAction {
        project_name: String,
        config_files: Option<String>,
        action: &'static str,
    },
    Prune {
        volumes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DockerConfirmState {
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) action: DockerConfirmAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DockerTab {
    Containers,
    Images,
    Volumes,
    Networks,
    Compose,
}

impl DockerTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::Images => "Images",
            Self::Volumes => "Volumes",
            Self::Networks => "Networks",
            Self::Compose => "Compose",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainMode {
    Workspace,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Appearance,
    Interaction,
    Keybindings,
    TerminalGeneral,
    Search,
    Translation,
    AiGeneral,
    AiModels,
    AiRules,
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
            Self::Search => "settings.search",
            Self::Translation => "settings.translation",
            Self::AiGeneral => "ai.general",
            Self::AiModels => "ai.models",
            Self::AiRules => "ai.rules",
            Self::Transfer => "settings.transfer",
            Self::Security => "settings.security",
            Self::SyncBackup => "settings.syncBackup",
        }
    }

    pub(crate) fn group_i18n_key(self) -> &'static str {
        match self {
            Self::General | Self::Appearance | Self::Interaction | Self::Keybindings => {
                "settings.groupWorkspace"
            }
            Self::TerminalGeneral | Self::Search | Self::Translation => {
                "settings.groupTerminalSession"
            }
            Self::AiGeneral | Self::AiModels | Self::AiRules => "ai.title",
            Self::Transfer => "settings.groupTransfer",
            Self::Security => "settings.groupSecurity",
            Self::SyncBackup => "settings.groupSyncBackup",
        }
    }

    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::General => "icons/settings.svg",
            Self::Appearance => "icons/menu/palette.svg",
            Self::Interaction => "icons/mouse.svg",
            Self::Keybindings => "icons/keyboard.svg",
            Self::TerminalGeneral => "icons/settings.svg",
            Self::Search => "icons/fe/search.svg",
            Self::Translation => "icons/translation.svg",
            Self::AiGeneral => "icons/ai/settings.svg",
            Self::AiModels => "icons/ai.svg",
            Self::AiRules => "icons/menu/book.svg",
            Self::Transfer => "icons/swap-horiz.svg",
            Self::Security => "icons/security.svg",
            Self::SyncBackup => "icons/sync.svg",
        }
    }

    pub(crate) fn expandable_group_id(self) -> Option<&'static str> {
        match self {
            Self::General | Self::Appearance | Self::Interaction | Self::Keybindings => {
                Some("workspace")
            }
            Self::TerminalGeneral | Self::Search | Self::Translation => Some("terminal_session"),
            Self::AiGeneral | Self::AiModels | Self::AiRules => Some("ai_group"),
            Self::Transfer | Self::Security | Self::SyncBackup => None,
        }
    }
}
