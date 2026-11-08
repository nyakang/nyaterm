//! Application tab routing over shared workspace tree contracts.
pub(crate) use nyaterm_core::workspace_layout::tabs::{
    TabDockEdge, TabDockZone, TerminalWindowNode,
};

/// Typed routing at the boundary of the existing string-keyed layout algorithms.
/// Editor and file-manager keys never enter session/protocol APIs or persisted
/// workspace data.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum WorkspaceTabId {
    Session(String),
    Editor(String),
    FileManager(String),
    /// The single settings surface. It is a transient tab: it is opened by
    /// `open settings`, closed when the draft ends, and never persisted in a
    /// restored workspace layout.
    Settings,
}

impl WorkspaceTabId {
    const EDITOR_PREFIX: &str = "\0editor:";
    const FILE_MANAGER_PREFIX: &str = "\0filemgr:";
    const SETTINGS_KEY: &str = "\0settings:";

    pub(crate) fn from_layout_key(key: &str) -> Self {
        if let Some(id) = key.strip_prefix(Self::EDITOR_PREFIX) {
            return Self::Editor(id.to_string());
        }
        if let Some(id) = key.strip_prefix(Self::FILE_MANAGER_PREFIX) {
            return Self::FileManager(id.to_string());
        }
        if key == Self::SETTINGS_KEY {
            return Self::Settings;
        }
        Self::Session(key.to_string())
    }

    pub(crate) fn settings_key() -> String {
        Self::SETTINGS_KEY.to_string()
    }

    pub(crate) fn layout_key(&self) -> String {
        match self {
            Self::Session(id) => id.clone(),
            Self::Editor(id) => format!("{}{id}", Self::EDITOR_PREFIX),
            Self::FileManager(id) => format!("{}{id}", Self::FILE_MANAGER_PREFIX),
            Self::Settings => Self::SETTINGS_KEY.to_string(),
        }
    }

    pub(crate) fn is_session(&self) -> bool {
        matches!(self, Self::Session(_))
    }

    pub(crate) fn is_settings(&self) -> bool {
        matches!(self, Self::Settings)
    }

    pub(crate) fn is_terminal_session(&self) -> bool {
        self.is_session()
    }

    /// True for any layout key that names a terminal/remote-desktop session tab.
    pub(crate) fn key_is_session(key: &str) -> bool {
        Self::from_layout_key(key).is_session()
    }
}
