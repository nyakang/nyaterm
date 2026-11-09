use super::pane::{SmartSplitMode, WorkspacePaneNode, WorkspaceSplitDirection};

/// In-window multi-leaf tab groups (Tauri `TerminalWindowNode` / TabWindowsWorkspace).
/// Distinct from per-tab pane splits (`WorkspacePaneNode`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalWindowNode {
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
pub enum TabDockEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl TabDockEdge {
    pub fn direction(self) -> WorkspaceSplitDirection {
        match self {
            Self::Left | Self::Right => WorkspaceSplitDirection::Vertical,
            Self::Top | Self::Bottom => WorkspaceSplitDirection::Horizontal,
        }
    }

    pub fn first_is_dropped(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDockZone {
    Center,
    Edge(TabDockEdge),
}

impl TabDockZone {
    /// Map local pointer position within a leaf's bounds to a dock zone.
    pub fn detect(local_x: f32, local_y: f32, width: f32, height: f32) -> Self {
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
        edges.sort_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((edge, distance, threshold)) = edges.first().copied()
            && distance <= threshold
        {
            return Self::Edge(edge);
        }
        Self::Center
    }
}
