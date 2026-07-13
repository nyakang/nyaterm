use super::super::{default_false, default_true};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeywordHighlightRule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default = "default_highlight_color_dark")]
    pub color_dark: String,
    #[serde(default = "default_highlight_color_light")]
    pub color_light: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_highlight_color_dark() -> String {
    "#79c0ff".to_string()
}
fn default_highlight_color_light() -> String {
    "#0969da".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLinksMatcherSettings {
    #[serde(default = "default_true")]
    pub ipv4: bool,
    #[serde(default = "default_true")]
    pub archive: bool,
    #[serde(default = "default_true")]
    pub host_port: bool,
}

impl Default for ActionLinksMatcherSettings {
    fn default() -> Self {
        Self {
            ipv4: true,
            archive: true,
            host_port: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSettings {
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: u32,
    #[serde(default = "default_keep_alive")]
    pub keep_alive_interval: u32,
    #[serde(default)]
    pub font_size_delta: f64,
    #[serde(default)]
    pub x11_display: String,
    #[serde(default = "default_false")]
    pub hardware_acceleration: bool,
    #[serde(default = "default_false")]
    pub keyword_highlights_enabled: bool,
    #[serde(default = "default_false")]
    pub keyword_highlights_across_wrapped_lines: bool,
    #[serde(default)]
    pub keyword_highlight_builtin_rules: BTreeMap<String, bool>,
    #[serde(default)]
    pub keyword_highlights: Vec<KeywordHighlightRule>,
    #[serde(default = "default_false")]
    pub action_links_enabled: bool,
    #[serde(default)]
    pub action_links_matchers: ActionLinksMatcherSettings,
    #[serde(default = "default_false")]
    pub show_workspace_padding: bool,
    #[serde(default = "default_false")]
    pub show_line_numbers: bool,
    #[serde(default = "default_false")]
    pub show_timestamps: bool,
    #[serde(default = "default_false")]
    pub show_timestamp_milliseconds: bool,
    #[serde(default = "default_true")]
    pub show_multi_line_paste_dialog: bool,
    #[serde(default = "default_true")]
    pub paste_image_as_path: bool,
    /// SSH channel window size in MB.  Controls how much data the remote
    /// can send before waiting for a window adjust.  Larger values improve
    /// throughput on high-latency links; smaller values improve
    /// compatibility with lightweight SSH servers.  Default 4 MB.
    #[serde(default = "default_ssh_window_size")]
    pub ssh_channel_window_size_mb: u32,
}

fn default_scrollback() -> u32 {
    10000
}
fn default_keep_alive() -> u32 {
    60
}
fn default_ssh_window_size() -> u32 {
    4
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            scrollback_lines: default_scrollback(),
            keep_alive_interval: default_keep_alive(),
            font_size_delta: 0.0,
            x11_display: String::new(),
            hardware_acceleration: false,
            keyword_highlights_enabled: false,
            keyword_highlights_across_wrapped_lines: false,
            keyword_highlight_builtin_rules: BTreeMap::new(),
            keyword_highlights: Vec::new(),
            action_links_enabled: false,
            action_links_matchers: ActionLinksMatcherSettings::default(),
            show_workspace_padding: false,
            show_line_numbers: false,
            show_timestamps: false,
            show_timestamp_milliseconds: false,
            show_multi_line_paste_dialog: true,
            paste_image_as_path: true,
            ssh_channel_window_size_mb: default_ssh_window_size(),
        }
    }
}
