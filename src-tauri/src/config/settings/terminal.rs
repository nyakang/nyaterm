use super::super::{default_false, default_true};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_TIMESTAMP_FORMAT: &str = "[HH:mm:ss]";
pub const TIMESTAMP_FORMAT_WITH_MILLISECONDS: &str = "[HH:mm:ss.SSS]";
pub const MIN_SCROLLBACK_LINES: u32 = 100;
pub const DEFAULT_SCROLLBACK_LINES: u32 = 10_000;
pub const MAX_SCROLLBACK_LINES: u32 = 100_000;
const MAX_TIMESTAMP_FORMAT_LEN: usize = 64;

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
    #[serde(default = "default_keep_alive_mode")]
    pub keep_alive_mode: String,
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
    #[serde(default = "default_timestamp_format")]
    pub timestamp_format: String,
    #[serde(default = "default_true")]
    pub show_multi_line_paste_dialog: bool,
    #[serde(default = "default_true")]
    pub paste_image_as_path: bool,
    #[serde(default = "default_false")]
    pub reconnect_restore_cwd: bool,
}

fn default_scrollback() -> u32 {
    DEFAULT_SCROLLBACK_LINES
}
fn default_keep_alive() -> u32 {
    60
}
fn default_keep_alive_mode() -> String {
    "compatible".to_string()
}
fn default_timestamp_format() -> String {
    DEFAULT_TIMESTAMP_FORMAT.to_string()
}

impl TerminalSettings {
    pub fn normalize_scrollback_lines(&mut self) -> bool {
        let normalized = self
            .scrollback_lines
            .clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
        if normalized == self.scrollback_lines {
            return false;
        }

        self.scrollback_lines = normalized;
        true
    }

    pub fn normalize_timestamp_format(&mut self) -> bool {
        let current = self.timestamp_format.clone();
        let normalized = if current.trim().is_empty() {
            DEFAULT_TIMESTAMP_FORMAT.to_string()
        } else {
            current.chars().take(MAX_TIMESTAMP_FORMAT_LEN).collect()
        };

        if normalized == current {
            return false;
        }

        self.timestamp_format = normalized;
        true
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            scrollback_lines: default_scrollback(),
            keep_alive_mode: default_keep_alive_mode(),
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
            timestamp_format: default_timestamp_format(),
            show_multi_line_paste_dialog: true,
            paste_image_as_path: true,
            reconnect_restore_cwd: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalSettings;

    #[test]
    fn missing_keep_alive_mode_defaults_to_compatible() {
        let settings: TerminalSettings = serde_json::from_value(serde_json::json!({
            "scrollback_lines": 5000,
            "keep_alive_interval": 60
        }))
        .expect("legacy terminal settings deserialize");

        assert_eq!(settings.keep_alive_mode, "compatible");
        assert_eq!(settings.keep_alive_interval, 60);
    }

    #[test]
    fn missing_reconnect_restore_cwd_defaults_to_false() {
        let settings: TerminalSettings = serde_json::from_value(serde_json::json!({
            "scrollback_lines": 5000,
            "keep_alive_interval": 60
        }))
        .expect("legacy terminal settings deserialize");

        assert!(!settings.reconnect_restore_cwd);
        assert!(!TerminalSettings::default().reconnect_restore_cwd);
    }

    #[test]
    fn missing_timestamp_format_defaults_to_seconds() {
        let settings: TerminalSettings =
            serde_json::from_value(serde_json::json!({})).expect("terminal settings deserialize");

        assert_eq!(settings.timestamp_format, "[HH:mm:ss]");
    }

    #[test]
    fn normalizes_empty_and_long_timestamp_formats() {
        let mut empty = TerminalSettings {
            timestamp_format: "   ".to_string(),
            ..TerminalSettings::default()
        };
        assert!(empty.normalize_timestamp_format());
        assert_eq!(empty.timestamp_format, "[HH:mm:ss]");

        let long_format = "H".repeat(80);
        let mut long = TerminalSettings {
            timestamp_format: long_format,
            ..TerminalSettings::default()
        };
        assert!(long.normalize_timestamp_format());
        assert_eq!(long.timestamp_format.chars().count(), 64);
    }

    #[test]
    fn normalizes_scrollback_lines_to_ui_bounds() {
        let mut low = TerminalSettings {
            scrollback_lines: 1,
            ..TerminalSettings::default()
        };
        assert!(low.normalize_scrollback_lines());
        assert_eq!(low.scrollback_lines, 100);

        let mut high = TerminalSettings {
            scrollback_lines: 250_000,
            ..TerminalSettings::default()
        };
        assert!(high.normalize_scrollback_lines());
        assert_eq!(high.scrollback_lines, 100_000);

        let mut ok = TerminalSettings {
            scrollback_lines: 10_000,
            ..TerminalSettings::default()
        };
        assert!(!ok.normalize_scrollback_lines());
        assert_eq!(ok.scrollback_lines, 10_000);
    }
}
