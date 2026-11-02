use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use super::{
    MainWindowState, RestorableOpenTab, RestorableTerminalWindowNode, RestorableWorkspacePaneNode,
};
use crate::activation::ParsedActivationRequest;

pub const WORKSPACE_RESTORE_MANIFEST_VERSION: u8 = 1;
pub const DEVICE_WINDOW_MANIFEST_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct WorkspaceId(pub Uuid);

impl WorkspaceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn legacy() -> Self {
        Self(Uuid::from_u128(0x6e796174_6572_6d2d_6c65_676163790001))
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActivationOpenBehavior {
    #[default]
    NewWindow,
    ReuseMostRecent,
}

#[derive(Debug, Clone)]
pub struct OpenWorkspaceRequest {
    pub activation: Option<ParsedActivationRequest>,
    pub activate: bool,
    pub layout_source_workspace_id: Option<WorkspaceId>,
}

impl Default for OpenWorkspaceRequest {
    fn default() -> Self {
        Self {
            activation: None,
            activate: true,
            layout_source_workspace_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTabDockEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MoveTabPlacement {
    #[default]
    Append,
    BeforeTab(String),
    AfterTab(String),
    TerminalLeaf {
        leaf_id: String,
        edge: Option<MoveTabDockEdge>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveTabTreeRequest {
    pub source_workspace_id: WorkspaceId,
    pub target_workspace_id: WorkspaceId,
    pub root_tab_id: String,
    pub source_revision: u64,
    pub placement: MoveTabPlacement,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkspaceSessionState {
    #[serde(default)]
    pub open_tabs: Vec<RestorableOpenTab>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_window_layout: Option<RestorableTerminalWindowNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_pane_layout: Option<RestorableWorkspacePaneNode>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceUiState {
    #[serde(default = "default_left_panel_width")]
    pub left_panel_width: u32,
    #[serde(default = "default_right_panel_width")]
    pub right_panel_width: u32,
    #[serde(default = "default_bottom_panel_height")]
    pub bottom_panel_height: u32,
    #[serde(default = "default_transfer_panel_height")]
    pub transfer_panel_height: u32,
    #[serde(default = "default_serial_send_panel_height")]
    pub serial_send_panel_height: u32,
    #[serde(default = "default_bottom_panel_mode")]
    pub bottom_panel_mode: String,
    #[serde(default)]
    pub active_left_panel: Option<String>,
    #[serde(default)]
    pub active_right_panel: Option<String>,
    #[serde(default)]
    pub left_panel_collapsed: bool,
    #[serde(default)]
    pub right_panel_collapsed: bool,
    #[serde(default)]
    pub panel_multi_open: bool,
    #[serde(default = "default_panel_open_mode")]
    pub panel_open_mode: String,
    #[serde(default)]
    pub left_open_panels: Vec<String>,
    #[serde(default)]
    pub right_open_panels: Vec<String>,
    #[serde(default)]
    pub panel_stack_sizes: BTreeMap<String, u32>,
    #[serde(default = "default_current_page")]
    pub current_page: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for WorkspaceUiState {
    fn default() -> Self {
        Self {
            left_panel_width: default_left_panel_width(),
            right_panel_width: default_right_panel_width(),
            bottom_panel_height: default_bottom_panel_height(),
            transfer_panel_height: default_transfer_panel_height(),
            serial_send_panel_height: default_serial_send_panel_height(),
            bottom_panel_mode: default_bottom_panel_mode(),
            active_left_panel: None,
            active_right_panel: None,
            left_panel_collapsed: false,
            right_panel_collapsed: false,
            panel_multi_open: false,
            panel_open_mode: default_panel_open_mode(),
            left_open_panels: Vec::new(),
            right_open_panels: Vec::new(),
            panel_stack_sizes: BTreeMap::new(),
            current_page: default_current_page(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRestoreState {
    pub id: WorkspaceId,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub sessions: WorkspaceSessionState,
    #[serde(default)]
    pub ui: WorkspaceUiState,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WorkspaceRestoreState {
    pub fn empty(id: WorkspaceId) -> Self {
        Self {
            id,
            revision: 0,
            sessions: WorkspaceSessionState::default(),
            ui: WorkspaceUiState::default(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRestoreManifest {
    pub version: u8,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceRestoreState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub most_recent_workspace_id: Option<WorkspaceId>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WorkspaceRestoreManifest {
    pub fn single(workspace: WorkspaceRestoreState) -> Self {
        Self {
            version: WORKSPACE_RESTORE_MANIFEST_VERSION,
            most_recent_workspace_id: Some(workspace.id),
            workspaces: vec![workspace],
            extra: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceManifestValidationError> {
        if self.version != WORKSPACE_RESTORE_MANIFEST_VERSION {
            return Err(
                WorkspaceManifestValidationError::UnsupportedWorkspaceVersion(self.version),
            );
        }
        let mut ids = HashSet::with_capacity(self.workspaces.len());
        for workspace in &self.workspaces {
            if !ids.insert(workspace.id) {
                return Err(WorkspaceManifestValidationError::DuplicateWorkspaceId(
                    workspace.id,
                ));
            }
            validate_session_layout(&workspace.sessions)?;
            validate_ui(&workspace.ui)?;
        }
        if let Some(id) = self.most_recent_workspace_id
            && !ids.contains(&id)
        {
            return Err(WorkspaceManifestValidationError::UnknownMostRecentWorkspace(id));
        }
        Ok(())
    }

    pub fn most_recent(&self) -> Option<&WorkspaceRestoreState> {
        self.most_recent_workspace_id
            .and_then(|id| self.workspaces.iter().find(|workspace| workspace.id == id))
            .or_else(|| self.workspaces.last())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceWindowState {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub window: MainWindowState,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceWindowManifest {
    pub version: u8,
    #[serde(default)]
    pub windows: Vec<DeviceWindowState>,
    #[serde(default)]
    pub window_order: Vec<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub most_recent_workspace_id: Option<WorkspaceId>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl DeviceWindowManifest {
    pub fn empty() -> Self {
        Self {
            version: DEVICE_WINDOW_MANIFEST_VERSION,
            windows: Vec::new(),
            window_order: Vec::new(),
            most_recent_workspace_id: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceManifestValidationError> {
        if self.version != DEVICE_WINDOW_MANIFEST_VERSION {
            return Err(
                WorkspaceManifestValidationError::UnsupportedDeviceWindowVersion(self.version),
            );
        }
        let mut ids = HashSet::with_capacity(self.windows.len());
        for state in &self.windows {
            if !ids.insert(state.workspace_id) {
                return Err(WorkspaceManifestValidationError::DuplicateWorkspaceId(
                    state.workspace_id,
                ));
            }
            state.window.validate().map_err(|error| {
                WorkspaceManifestValidationError::InvalidWindowGeometry(error.to_string())
            })?;
        }
        let mut ordered = HashSet::with_capacity(self.window_order.len());
        for id in &self.window_order {
            if !ordered.insert(*id) {
                return Err(WorkspaceManifestValidationError::DuplicateWindowOrderId(
                    *id,
                ));
            }
            if !ids.contains(id) {
                return Err(WorkspaceManifestValidationError::UnknownWindowOrderId(*id));
            }
        }
        if let Some(id) = self.most_recent_workspace_id
            && !ids.contains(&id)
        {
            return Err(WorkspaceManifestValidationError::UnknownMostRecentWorkspace(id));
        }
        Ok(())
    }

    pub fn state_for(&self, workspace_id: WorkspaceId) -> Option<&MainWindowState> {
        self.windows
            .iter()
            .find(|state| state.workspace_id == workspace_id)
            .map(|state| &state.window)
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WorkspaceManifestValidationError {
    #[error("unsupported workspace manifest version {0}")]
    UnsupportedWorkspaceVersion(u8),
    #[error("unsupported device window manifest version {0}")]
    UnsupportedDeviceWindowVersion(u8),
    #[error("duplicate workspace id {0:?}")]
    DuplicateWorkspaceId(WorkspaceId),
    #[error("most recent workspace {0:?} is not present")]
    UnknownMostRecentWorkspace(WorkspaceId),
    #[error("window order contains duplicate workspace {0:?}")]
    DuplicateWindowOrderId(WorkspaceId),
    #[error("window order references unknown workspace {0:?}")]
    UnknownWindowOrderId(WorkspaceId),
    #[error("layout references tab index {index}, but only {tab_count} tabs exist")]
    InvalidTabIndex { index: usize, tab_count: usize },
    #[error("invalid workspace UI dimensions")]
    InvalidUiDimensions,
    #[error("invalid window geometry: {0}")]
    InvalidWindowGeometry(String),
}

fn validate_session_layout(
    sessions: &WorkspaceSessionState,
) -> Result<(), WorkspaceManifestValidationError> {
    let tab_count = sessions.open_tabs.len();
    if let Some(layout) = &sessions.terminal_window_layout {
        validate_terminal_layout(layout, tab_count)?;
    }
    if let Some(layout) = &sessions.workspace_pane_layout {
        validate_workspace_layout(layout, tab_count)?;
    }
    Ok(())
}

fn validate_terminal_layout(
    node: &RestorableTerminalWindowNode,
    tab_count: usize,
) -> Result<(), WorkspaceManifestValidationError> {
    match node {
        RestorableTerminalWindowNode::Leaf {
            tab_indexes,
            active_tab_index,
        } => {
            for index in tab_indexes.iter().copied().chain(*active_tab_index) {
                validate_tab_index(index, tab_count)?;
            }
        }
        RestorableTerminalWindowNode::Split { first, second, .. } => {
            validate_terminal_layout(first, tab_count)?;
            validate_terminal_layout(second, tab_count)?;
        }
    }
    Ok(())
}

fn validate_workspace_layout(
    node: &RestorableWorkspacePaneNode,
    tab_count: usize,
) -> Result<(), WorkspaceManifestValidationError> {
    match node {
        RestorableWorkspacePaneNode::Leaf { tab_index } => {
            validate_tab_index(*tab_index, tab_count)?;
        }
        RestorableWorkspacePaneNode::Split { first, second, .. } => {
            validate_workspace_layout(first, tab_count)?;
            validate_workspace_layout(second, tab_count)?;
        }
    }
    Ok(())
}

fn validate_tab_index(
    index: usize,
    tab_count: usize,
) -> Result<(), WorkspaceManifestValidationError> {
    if index >= tab_count {
        return Err(WorkspaceManifestValidationError::InvalidTabIndex { index, tab_count });
    }
    Ok(())
}

fn validate_ui(ui: &WorkspaceUiState) -> Result<(), WorkspaceManifestValidationError> {
    if ui.left_panel_width == 0
        || ui.right_panel_width == 0
        || ui.bottom_panel_height == 0
        || ui.transfer_panel_height == 0
        || ui.serial_send_panel_height == 0
    {
        return Err(WorkspaceManifestValidationError::InvalidUiDimensions);
    }
    Ok(())
}

fn default_left_panel_width() -> u32 {
    256
}

fn default_right_panel_width() -> u32 {
    288
}

fn default_bottom_panel_height() -> u32 {
    180
}

fn default_transfer_panel_height() -> u32 {
    180
}

fn default_serial_send_panel_height() -> u32 {
    180
}

fn default_bottom_panel_mode() -> String {
    "quick_commands".to_string()
}

fn default_panel_open_mode() -> String {
    "docked".to_string()
}

fn default_current_page() -> String {
    "workspace".to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{
        DEVICE_WINDOW_MANIFEST_VERSION, DeviceWindowManifest, DeviceWindowState, MainWindowState,
        RestorableWorkspacePaneNode, WorkspaceId, WorkspaceManifestValidationError,
        WorkspaceRestoreManifest, WorkspaceRestoreState, WorkspaceUiState,
    };
    use crate::MainWindowBounds;

    #[test]
    fn workspace_manifest_round_trips_unknown_fields() {
        let id = WorkspaceId::new();
        let raw = json!({
            "version": 1,
            "workspaces": [{
                "id": id,
                "revision": 7,
                "sessions": {"open_tabs": [], "future_sessions": {"enabled": true}},
                "ui": {"future_ui": "kept"},
                "future_workspace": 42
            }],
            "most_recent_workspace_id": id,
            "future_manifest": [1, 2, 3]
        });
        let manifest: WorkspaceRestoreManifest = serde_json::from_value(raw.clone()).unwrap();
        manifest.validate().unwrap();
        let mut ui = manifest.workspaces[0].ui.clone();
        assert_eq!(ui.extra.remove("future_ui"), Some(json!("kept")));
        assert_eq!(ui, WorkspaceUiState::default());
        let encoded = serde_json::to_value(manifest).unwrap();
        assert_eq!(encoded["future_manifest"], raw["future_manifest"]);
        assert_eq!(
            encoded["workspaces"][0]["future_workspace"],
            raw["workspaces"][0]["future_workspace"]
        );
        assert_eq!(
            encoded["workspaces"][0]["sessions"]["future_sessions"],
            raw["workspaces"][0]["sessions"]["future_sessions"]
        );
        assert_eq!(
            encoded["workspaces"][0]["ui"]["future_ui"],
            raw["workspaces"][0]["ui"]["future_ui"]
        );
    }

    #[test]
    fn workspace_ui_state_round_trips_window_local_layout() {
        let mut stack_sizes = BTreeMap::new();
        stack_sizes.insert("left:fileExplorer".to_string(), 625);
        let ui = WorkspaceUiState {
            left_panel_width: 340,
            right_panel_width: 420,
            bottom_panel_height: 210,
            transfer_panel_height: 260,
            serial_send_panel_height: 190,
            bottom_panel_mode: "command_send".to_string(),
            active_left_panel: Some("fileExplorer".to_string()),
            active_right_panel: Some("savedConnections".to_string()),
            left_panel_collapsed: false,
            right_panel_collapsed: true,
            panel_multi_open: true,
            panel_open_mode: "docked".to_string(),
            left_open_panels: vec!["fileExplorer".to_string(), "notes".to_string()],
            right_open_panels: vec!["savedConnections".to_string()],
            panel_stack_sizes: stack_sizes,
            current_page: "fileExplorer".to_string(),
            extra: BTreeMap::from([("future".to_string(), json!(true))]),
        };

        let encoded = serde_json::to_value(&ui).unwrap();
        let decoded: WorkspaceUiState = serde_json::from_value(encoded).unwrap();

        assert_eq!(decoded, ui);
    }

    #[test]
    fn workspace_manifest_rejects_invalid_layout_indexes() {
        let mut workspace = WorkspaceRestoreState::empty(WorkspaceId::new());
        workspace.sessions.workspace_pane_layout =
            Some(RestorableWorkspacePaneNode::Leaf { tab_index: 0 });
        let manifest = WorkspaceRestoreManifest::single(workspace);
        assert_eq!(
            manifest.validate(),
            Err(WorkspaceManifestValidationError::InvalidTabIndex {
                index: 0,
                tab_count: 0
            })
        );
    }

    #[test]
    fn device_window_manifest_validates_geometry_and_order() {
        let id = WorkspaceId::new();
        let manifest = DeviceWindowManifest {
            version: DEVICE_WINDOW_MANIFEST_VERSION,
            windows: vec![DeviceWindowState {
                workspace_id: id,
                window: MainWindowState::new(
                    None,
                    MainWindowBounds {
                        x: 10,
                        y: 20,
                        width: 1280,
                        height: 800,
                    },
                    false,
                ),
                extra: BTreeMap::new(),
            }],
            window_order: vec![id],
            most_recent_workspace_id: Some(id),
            extra: BTreeMap::new(),
        };
        manifest.validate().unwrap();
    }
}
