use super::{SmartSplitMode, WorkspacePaneNode, WorkspaceSplitDirection};

/// In-window multi-leaf tab groups (Tauri `TerminalWindowNode` / TabWindowsWorkspace).
/// Distinct from per-tab pane splits (`WorkspacePaneNode`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalWindowNode {
    Leaf {
        id: String,
        tab_ids: Vec<String>,
        active_tab_id: Option<String>,
    },
    Split {
        id: String,
        direction: WorkspaceSplitDirection,
        ratio_percent: u8,
        first: Box<TerminalWindowNode>,
        second: Box<TerminalWindowNode>,
    },
}

mod docking;
mod groups;
mod mutation;
mod persistence;
mod tree;

/// Edge drop zones for tab docking (Tauri SplitEdgeDirection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabDockEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl TabDockEdge {
    pub(crate) fn direction(self) -> WorkspaceSplitDirection {
        match self {
            Self::Left | Self::Right => WorkspaceSplitDirection::Vertical,
            Self::Top | Self::Bottom => WorkspaceSplitDirection::Horizontal,
        }
    }

    pub(crate) fn first_is_dropped(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabDockZone {
    Center,
    Edge(TabDockEdge),
}

impl TabDockZone {
    /// Map local pointer position within a leaf's bounds to a dock zone.
    pub(crate) fn detect(local_x: f32, local_y: f32, width: f32, height: f32) -> Self {
        if width <= 1.0 || height <= 1.0 {
            return Self::Center;
        }
        let h_thresh = (width * 0.38).clamp(48.0, 180.0);
        let v_thresh = (height * 0.34).clamp(40.0, 140.0);
        let mut edges = [
            (TabDockEdge::Left, local_x, h_thresh),
            (TabDockEdge::Right, width - local_x, h_thresh),
            (TabDockEdge::Top, local_y, v_thresh),
            (TabDockEdge::Bottom, height - local_y, v_thresh),
        ];
        edges.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((edge, distance, threshold)) = edges.first().copied()
            && distance <= threshold
        {
            return Self::Edge(edge);
        }
        Self::Center
    }
}

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
