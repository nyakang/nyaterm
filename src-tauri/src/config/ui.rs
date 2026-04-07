use super::default_false;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RestorableTab {
    pub title: String,
    pub session_type: String,
    pub connection_id: Option<String>,
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
        "fileTransfer".to_string(),
        "securityAuth".to_string(),
    ]
}

fn default_left_bottom() -> Vec<String> {
    vec!["settings".to_string()]
}

fn default_right_top() -> Vec<String> {
    vec![
        "savedConnections".to_string(),
        "activeSessions".to_string(),
        "commandHistory".to_string(),
        "resourceMonitor".to_string(),
    ]
}

fn default_right_bottom() -> Vec<String> {
    vec!["quickCmdBar".to_string(), "lock".to_string()]
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
            zoom_level: default_zoom(),
            language: default_language(),
            show_remote_stats: false,
            remote_stats_interval: default_remote_stats_interval(),
            saved_connections_sort_mode: default_sort_mode(),
            activity_bar_layout: ActivityBarLayout::default(),
        }
    }
}
