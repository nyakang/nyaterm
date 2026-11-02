use serde::{Deserialize, Serialize};

/// Tauri per-tab pane tree node (`RestorablePaneNode` in ui.open_tabs[].root).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum RestorablePaneNode {
    #[serde(rename = "leaf")]
    Leaf {
        #[serde(default)]
        id: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        session_type: String,
        #[serde(default)]
        connection_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ssh_runtime_mode: Option<String>,
    },
    #[serde(rename = "split")]
    Split {
        #[serde(default)]
        id: String,
        direction: String,
        #[serde(default = "default_restorable_split_ratio")]
        ratio: f64,
        first: Box<RestorablePaneNode>,
        second: Box<RestorablePaneNode>,
    },
}

impl RestorablePaneNode {
    /// Flatten session leaves in left-to-right / top-to-bottom order.
    pub fn collect_leaves(&self) -> Vec<RestorablePaneLeaf> {
        let mut out = Vec::new();
        self.collect_leaves_into(&mut out);
        out
    }

    fn collect_leaves_into(&self, out: &mut Vec<RestorablePaneLeaf>) {
        match self {
            Self::Leaf {
                id,
                title,
                session_type,
                connection_id,
                ssh_runtime_mode,
            } => out.push(RestorablePaneLeaf {
                id: id.clone(),
                title: title.clone(),
                session_type: session_type.clone(),
                connection_id: connection_id.clone(),
                ssh_runtime_mode: ssh_runtime_mode.clone(),
            }),
            Self::Split { first, second, .. } => {
                first.collect_leaves_into(out);
                second.collect_leaves_into(out);
            }
        }
    }

    /// Map this pane tree onto ordered open-tab indexes starting at `base_index`.
    pub fn to_workspace_pane_layout(
        &self,
        base_index: usize,
    ) -> Option<RestorableWorkspacePaneNode> {
        let mut next = base_index;
        self.to_workspace_pane_layout_inner(&mut next)
    }

    fn to_workspace_pane_layout_inner(
        &self,
        next_index: &mut usize,
    ) -> Option<RestorableWorkspacePaneNode> {
        match self {
            Self::Leaf { .. } => {
                let tab_index = *next_index;
                *next_index += 1;
                Some(RestorableWorkspacePaneNode::Leaf { tab_index })
            }
            Self::Split {
                id,
                direction,
                ratio,
                first,
                second,
            } => {
                let first = first.to_workspace_pane_layout_inner(next_index);
                let second = second.to_workspace_pane_layout_inner(next_index);
                match (first, second) {
                    (None, None) => None,
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (Some(first), Some(second)) => Some(RestorableWorkspacePaneNode::Split {
                        id: if id.trim().is_empty() {
                            format!("pane-{}", *next_index)
                        } else {
                            id.clone()
                        },
                        direction: direction.clone(),
                        ratio: (*ratio).clamp(0.2, 0.8),
                        first: Box::new(first),
                        second: Box::new(second),
                    }),
                }
            }
        }
    }

    pub fn leaf_session(
        title: impl Into<String>,
        session_type: impl Into<String>,
        connection_id: Option<String>,
    ) -> Self {
        Self::Leaf {
            id: String::new(),
            title: title.into(),
            session_type: session_type.into(),
            connection_id,
            ssh_runtime_mode: None,
        }
    }
}

/// One restorable session leaf extracted from a tab pane tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorablePaneLeaf {
    pub ssh_runtime_mode: Option<String>,
    pub id: String,
    pub title: String,
    pub session_type: String,
    pub connection_id: Option<String>,
}

/// Tauri `ui.open_tabs` entry (native restores connection/local leaf sessions).
/// Optional `root` preserves Tauri per-tab pane trees for interop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestorableOpenTab {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_runtime_mode: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub session_type: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub tab_color: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    #[serde(default)]
    pub active_pane_id: Option<String>,
    #[serde(default)]
    pub root: Option<RestorablePaneNode>,
}

impl RestorableOpenTab {
    pub fn ssh_runtime_modes_supported(&self) -> bool {
        let supported = |mode: Option<&str>| {
            mode.is_none_or(|mode| super::ssh_runtime::SshRuntimeMode::parse(mode).is_some())
        };
        supported(self.ssh_runtime_mode.as_deref())
            && self.root.as_ref().is_none_or(|root| {
                root.collect_leaves()
                    .iter()
                    .all(|leaf| supported(leaf.ssh_runtime_mode.as_deref()))
            })
    }

    /// Expand this tab into one or more session restore descriptors.
    /// Split roots become multiple leaves (native global sessions).
    pub fn expanded_sessions(&self) -> Vec<RestorableOpenTabSession> {
        if let Some(root) = &self.root {
            let leaves = root.collect_leaves();
            if !leaves.is_empty() {
                return leaves
                    .into_iter()
                    .map(|leaf| {
                        let title = if leaf.title.trim().is_empty() {
                            self.title.clone()
                        } else {
                            leaf.title
                        };
                        let session_type = if leaf.session_type.trim().is_empty() {
                            self.session_type.clone()
                        } else {
                            leaf.session_type
                        };
                        let connection_id = leaf
                            .connection_id
                            .filter(|id| !id.is_empty())
                            .or_else(|| self.connection_id.clone());
                        RestorableOpenTabSession {
                            ssh_runtime_mode: leaf
                                .ssh_runtime_mode
                                .or_else(|| self.ssh_runtime_mode.clone()),
                            title,
                            session_type,
                            connection_id,
                            custom_name: self.custom_name.clone(),
                            tab_color: self.tab_color.clone(),
                            locked: self.locked,
                        }
                    })
                    .collect();
            }
        }

        vec![RestorableOpenTabSession {
            ssh_runtime_mode: self.ssh_runtime_mode.clone(),
            title: self.title.clone(),
            session_type: self.session_type.clone(),
            connection_id: self.connection_id.clone(),
            custom_name: self.custom_name.clone(),
            tab_color: self.tab_color.clone(),
            locked: self.locked,
        }]
    }

    /// If this tab carries a multi-pane root, map it onto ordered tab indexes
    /// starting at `base_index` (index of the first expanded leaf).
    pub fn workspace_pane_layout_from_root(
        &self,
        base_index: usize,
    ) -> Option<RestorableWorkspacePaneNode> {
        let root = self.root.as_ref()?;
        if matches!(root, RestorablePaneNode::Leaf { .. }) {
            return None;
        }
        let layout = root.to_workspace_pane_layout(base_index)?;
        match &layout {
            RestorableWorkspacePaneNode::Split { .. } => Some(layout),
            RestorableWorkspacePaneNode::Leaf { .. } => None,
        }
    }

    pub fn with_leaf_root(
        title: impl Into<String>,
        session_type: impl Into<String>,
        connection_id: Option<String>,
        custom_name: Option<String>,
        tab_color: Option<String>,
    ) -> Self {
        let title = title.into();
        let session_type = session_type.into();
        let root = RestorablePaneNode::leaf_session(
            title.clone(),
            session_type.clone(),
            connection_id.clone(),
        );
        Self {
            ssh_runtime_mode: None,
            title,
            session_type,
            connection_id,
            custom_name,
            tab_color,
            locked: false,
            active_pane_id: None,
            root: Some(root),
        }
    }
}

/// Flattened session restore unit (one native tab / session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorableOpenTabSession {
    pub ssh_runtime_mode: Option<String>,
    pub title: String,
    pub session_type: String,
    pub connection_id: Option<String>,
    pub custom_name: Option<String>,
    pub tab_color: Option<String>,
    pub locked: bool,
}

/// Native workspace pane split tree (indexes into ordered open tabs).
/// Distinct from Tauri per-tab pane trees: native H/V splits arrange sessions globally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum RestorableWorkspacePaneNode {
    #[serde(rename = "leaf")]
    Leaf {
        #[serde(default)]
        tab_index: usize,
    },
    #[serde(rename = "split")]
    Split {
        #[serde(default)]
        id: String,
        direction: String,
        #[serde(default = "default_restorable_split_ratio")]
        ratio: f64,
        first: Box<RestorableWorkspacePaneNode>,
        second: Box<RestorableWorkspacePaneNode>,
    },
}

/// Tauri `ui.terminal_window_layout` node (indexes into ordered open tabs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum RestorableTerminalWindowNode {
    #[serde(rename = "leaf")]
    Leaf {
        #[serde(default)]
        tab_indexes: Vec<usize>,
        #[serde(default)]
        active_tab_index: Option<usize>,
    },
    #[serde(rename = "split")]
    Split {
        direction: String,
        #[serde(default = "default_restorable_split_ratio")]
        ratio: f64,
        first: Box<RestorableTerminalWindowNode>,
        second: Box<RestorableTerminalWindowNode>,
    },
}

fn default_restorable_split_ratio() -> f64 {
    0.5
}

#[cfg(test)]
mod tests {
    use super::{RestorableOpenTab, RestorablePaneNode};
    use crate::models::ssh_runtime::SshRuntimeMode;
    use serde_json::json;

    #[test]
    fn legacy_tabs_default_to_standard_without_rewriting_the_data_contract() {
        let tab: RestorableOpenTab = serde_json::from_value(
            json!({"title":"old", "session_type":"SSH", "connection_id":"saved"}),
        )
        .unwrap();
        let session = tab.expanded_sessions().remove(0);
        assert!(session.ssh_runtime_mode.is_none());
        assert_eq!(
            session
                .ssh_runtime_mode
                .as_deref()
                .and_then(SshRuntimeMode::parse)
                .unwrap_or_default(),
            SshRuntimeMode::Standard
        );
        assert!(
            serde_json::to_value(tab)
                .unwrap()
                .get("ssh_runtime_mode")
                .is_none()
        );
    }

    #[test]
    fn tauri_split_leaf_modes_override_tab_mode_and_preserve_unknown_values() {
        let value = json!({"title":"split", "session_type":"SSH", "connection_id":"saved", "ssh_runtime_mode":"standard", "root": {
            "kind":"split", "id":"tree", "direction":"horizontal", "ratio":0.5,
            "first":{"kind":"leaf", "id":"a", "ssh_runtime_mode":"sftp"},
            "second":{"kind":"leaf", "id":"b", "ssh_runtime_mode":"future-mode"}
        }});
        let tab: RestorableOpenTab = serde_json::from_value(value).unwrap();
        let sessions = tab.expanded_sessions();
        assert_eq!(sessions[0].ssh_runtime_mode.as_deref(), Some("sftp"));
        assert_eq!(sessions[1].ssh_runtime_mode.as_deref(), Some("future-mode"));
        assert!(
            sessions[1]
                .ssh_runtime_mode
                .as_deref()
                .and_then(SshRuntimeMode::parse)
                .is_none()
        );
        let serialized = serde_json::to_string(&tab).unwrap();
        let restored: RestorableOpenTab = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored, tab);
        assert!(serialized.contains("future-mode"));
        assert!(!tab.ssh_runtime_modes_supported());
        let mut tab = RestorableOpenTab::with_leaf_root("future", "SSH", None, None, None);
        tab.ssh_runtime_mode = Some("future-mode".into());
        if let Some(RestorablePaneNode::Leaf {
            ssh_runtime_mode, ..
        }) = tab.root.as_mut()
        {
            *ssh_runtime_mode = Some("standard".into());
        }
        assert!(!tab.ssh_runtime_modes_supported());
    }

    #[test]
    fn all_runtime_modes_round_trip_in_tabs_and_leaf_roots() {
        for mode in [
            SshRuntimeMode::Standard,
            SshRuntimeMode::Terminal,
            SshRuntimeMode::Sftp,
        ] {
            let mut tab = RestorableOpenTab::with_leaf_root(
                "session",
                "SSH",
                Some("saved".into()),
                None,
                None,
            );
            tab.ssh_runtime_mode = Some(mode.as_str().into());
            if let Some(RestorablePaneNode::Leaf {
                ssh_runtime_mode, ..
            }) = tab.root.as_mut()
            {
                *ssh_runtime_mode = tab.ssh_runtime_mode.clone();
            }
            let restored: RestorableOpenTab =
                serde_json::from_str(&serde_json::to_string(&tab).unwrap()).unwrap();
            assert_eq!(restored, tab);
            assert_eq!(
                restored.expanded_sessions()[0].ssh_runtime_mode.as_deref(),
                Some(mode.as_str())
            );
        }
    }
}
