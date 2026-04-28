use super::{default_false, uuid_v4};
use serde::{Deserialize, Serialize};

fn default_leaf_id() -> String {
    format!("pane-{}", uuid_v4())
}

fn default_split_id() -> String {
    format!("split-{}", uuid_v4())
}

fn default_split_ratio() -> f64 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum RestorablePaneNode {
    #[serde(rename = "leaf")]
    Leaf {
        #[serde(default = "default_leaf_id")]
        id: String,
        title: String,
        session_type: String,
        connection_id: Option<String>,
    },
    #[serde(rename = "split")]
    Split {
        #[serde(default = "default_split_id")]
        id: String,
        direction: String,
        #[serde(default = "default_split_ratio")]
        ratio: f64,
        first: Box<RestorablePaneNode>,
        second: Box<RestorablePaneNode>,
    },
}

impl RestorablePaneNode {
    pub fn first_leaf_id(&self) -> Option<&str> {
        match self {
            Self::Leaf { id, .. } => Some(id.as_str()),
            Self::Split { first, second, .. } => {
                first.first_leaf_id().or_else(|| second.first_leaf_id())
            }
        }
    }

    pub fn first_leaf_summary(&self) -> Option<(&str, &str, Option<&str>)> {
        match self {
            Self::Leaf {
                title,
                session_type,
                connection_id,
                ..
            } => Some((
                title.as_str(),
                session_type.as_str(),
                connection_id.as_deref(),
            )),
            Self::Split { first, second, .. } => first
                .first_leaf_summary()
                .or_else(|| second.first_leaf_summary()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RestorableTab {
    #[serde(default)]
    pub active_pane_id: Option<String>,
    #[serde(default)]
    pub root: Option<RestorablePaneNode>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub session_type: String,
    pub connection_id: Option<String>,
    pub custom_name: Option<String>,
    pub tab_color: Option<String>,
}

impl RestorableTab {
    pub fn normalize(&mut self) -> bool {
        let mut changed = false;

        if self.root.is_none() && !self.session_type.is_empty() {
            let leaf_id = default_leaf_id();
            self.root = Some(RestorablePaneNode::Leaf {
                id: leaf_id.clone(),
                title: if self.title.is_empty() {
                    "Session".to_string()
                } else {
                    self.title.clone()
                },
                session_type: self.session_type.clone(),
                connection_id: self.connection_id.clone(),
            });
            if self.active_pane_id.is_none() {
                self.active_pane_id = Some(leaf_id);
            }
            changed = true;
        }

        if let Some(root) = &self.root {
            if self.active_pane_id.is_none() {
                self.active_pane_id = root.first_leaf_id().map(|id| id.to_string());
                changed = true;
            }

            if let Some((title, session_type, connection_id)) = root.first_leaf_summary() {
                if self.title.is_empty() {
                    self.title = title.to_string();
                    changed = true;
                }
                if self.session_type.is_empty() {
                    self.session_type = session_type.to_string();
                    changed = true;
                }
                if self.connection_id.is_none() && connection_id.is_some() {
                    self.connection_id = connection_id.map(|value| value.to_string());
                    changed = true;
                }
            }
        }

        changed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityBarLayout {
    #[serde(default = "default_left_top")]
    pub left_top: Vec<String>,
    #[serde(default = "default_left_bottom")]
    pub left_bottom: Vec<String>,
    #[serde(default = "default_right_top")]
    pub right_top: Vec<String>,
    #[serde(default = "default_right_bottom")]
    pub right_bottom: Vec<String>,
    #[serde(default)]
    pub show_labels: bool,
}

impl Default for ActivityBarLayout {
    fn default() -> Self {
        Self {
            left_top: default_left_top(),
            left_bottom: default_left_bottom(),
            right_top: default_right_top(),
            right_bottom: default_right_bottom(),
            show_labels: false,
        }
    }
}

fn default_left_top() -> Vec<String> {
    vec![
        "fileExplorer".to_string(),
        "network".to_string(),
        "securityAuth".to_string(),
    ]
}

fn default_left_bottom() -> Vec<String> {
    vec!["syncBackupHistory".to_string(), "settings".to_string()]
}

fn default_right_top() -> Vec<String> {
    vec![
        "savedConnections".to_string(),
        "aiAssistant".to_string(),
        "activeSessions".to_string(),
        "commandHistory".to_string(),
        "resourceMonitor".to_string(),
    ]
}

fn default_right_bottom() -> Vec<String> {
    vec![
        "quickCmdBar".to_string(),
        "serialSend".to_string(),
        "recording".to_string(),
        "lock".to_string(),
    ]
}

/// Layout and theme preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub open_tabs: Vec<RestorableTab>,
    #[serde(default = "default_left_width")]
    pub left_width: f64,
    #[serde(default = "default_right_width")]
    pub right_width: f64,
    #[serde(default = "default_quick_cmd_height")]
    pub quick_cmd_height: f64,
    #[serde(default = "default_active_left_panel")]
    pub active_left_panel: Option<String>,
    #[serde(default = "default_active_right_panel")]
    pub active_right_panel: Option<String>,
    #[serde(default = "default_true_fn")]
    pub show_quick_cmd_bar: bool,
    #[serde(default = "default_false")]
    pub show_serial_send_panel: bool,
    #[serde(default = "default_serial_send_height")]
    pub serial_send_height: f64,
    #[serde(default = "default_zoom")]
    pub zoom_level: f64,
    #[serde(default = "default_language")]
    pub language: Option<String>,
    #[serde(default = "default_false")]
    pub show_remote_stats: bool,
    #[serde(default = "default_remote_stats_interval")]
    pub remote_stats_interval: u32,
    #[serde(default = "default_sort_mode")]
    pub saved_connections_sort_mode: String,
    #[serde(default = "default_transfer_height")]
    pub transfer_height: f64,
    #[serde(default)]
    pub activity_bar_layout: ActivityBarLayout,
}

fn default_left_width() -> f64 {
    256.0
}

fn default_right_width() -> f64 {
    288.0
}

fn default_quick_cmd_height() -> f64 {
    36.0
}

fn default_active_left_panel() -> Option<String> {
    Some("fileExplorer".to_string())
}

fn default_active_right_panel() -> Option<String> {
    Some("savedConnections".to_string())
}

fn default_true_fn() -> bool {
    true
}

fn default_zoom() -> f64 {
    1.0
}

fn default_remote_stats_interval() -> u32 {
    3
}

fn default_transfer_height() -> f64 {
    180.0
}

fn default_serial_send_height() -> f64 {
    120.0
}

fn default_sort_mode() -> String {
    "default".to_string()
}

fn default_language() -> Option<String> {
    Some("en".to_string())
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            open_tabs: vec![],
            left_width: default_left_width(),
            right_width: default_right_width(),
            quick_cmd_height: default_quick_cmd_height(),
            active_left_panel: default_active_left_panel(),
            active_right_panel: default_active_right_panel(),
            show_quick_cmd_bar: true,
            show_serial_send_panel: false,
            serial_send_height: default_serial_send_height(),
            zoom_level: default_zoom(),
            language: default_language(),
            show_remote_stats: false,
            remote_stats_interval: default_remote_stats_interval(),
            saved_connections_sort_mode: default_sort_mode(),
            transfer_height: default_transfer_height(),
            activity_bar_layout: ActivityBarLayout::default(),
        }
    }
}
