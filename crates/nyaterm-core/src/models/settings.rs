use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::default_true;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionLinksMatcherSettings {
    #[serde(default = "default_true_action_link")]
    pub ipv4: bool,
    #[serde(default = "default_true_action_link")]
    pub archive: bool,
    #[serde(default = "default_true_action_link")]
    pub host_port: bool,
}

fn default_true_action_link() -> bool {
    true
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchEngineConfig {
    pub name: String,
    pub url_template: String,
    /// Optional icon key (Tauri SEARCH_ICONS: google/bing/github/...).
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default = "default_true_search_menu")]
    pub show_in_menu: bool,
}

fn default_true_search_menu() -> bool {
    true
}

pub fn default_search_engines() -> Vec<SearchEngineConfig> {
    vec![
        SearchEngineConfig {
            name: "Google".to_string(),
            url_template: "https://www.google.com/search?q=%s".to_string(),
            icon: Some("google".to_string()),
            show_in_menu: true,
        },
        SearchEngineConfig {
            name: "Bing".to_string(),
            url_template: "https://www.bing.com/search?q=%s".to_string(),
            icon: Some("bing".to_string()),
            show_in_menu: true,
        },
        SearchEngineConfig {
            name: "GitHub".to_string(),
            url_template: "https://github.com/search?q=%s".to_string(),
            icon: Some("github".to_string()),
            show_in_menu: true,
        },
    ]
}

pub const DEFAULT_RECORDING_PATH_TEMPLATE: &str =
    "{group}/{session}/{yyyy}-{MM}-{dd}/{HH}-{mm}-{ss}-{SSS}-{session_short_id}.log";
pub const DEFAULT_TERMINAL_TIMESTAMP_FORMAT: &str = "[HH:mm:ss]";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    #[default]
    Transcript,
    Raw,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExistingFileBehavior {
    #[default]
    Unique,
    Append,
    Overwrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RecordingRotationPolicy {
    #[default]
    Session,
    Daily,
    Size {
        max_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettingsSummary {
    pub theme: String,
    #[serde(default)]
    pub background_image_path: Option<String>,
    #[serde(default = "default_background_image_fit")]
    pub background_image_fit: String,
    /// Wallpaper opacity percent (0..=100).
    #[serde(default = "default_background_image_opacity")]
    pub background_image_opacity: u8,
    /// Shell chrome opacity percent when wallpaper is active (0..=100).
    #[serde(default = "default_background_content_opacity")]
    pub background_content_opacity: u8,
    pub language: String,
    pub terminal_font_family: String,
    pub terminal_font_size: u16,
    /// Terminal cursor style: block | underline | bar (Tauri appearance.cursor_style).
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    /// Whether the terminal caret blinks (Tauri appearance.cursor_blink).
    #[serde(default = "default_cursor_blink")]
    pub cursor_blink: bool,
    /// Optional terminal color theme id; None / empty follows UI theme (Tauri appearance.terminal_theme).
    #[serde(default)]
    pub terminal_theme: Option<String>,
    /// Minimum contrast ratio for terminal colors (Tauri appearance.minimum_contrast_ratio).
    /// Stored as a display string: "1", "3", "4.5", "7", or "21".
    #[serde(default = "default_minimum_contrast_ratio")]
    pub minimum_contrast_ratio: String,
    /// UI chrome font family (Tauri appearance.ui_font_family).
    #[serde(default = "default_ui_font_family")]
    pub ui_font_family: String,
    /// UI chrome font size in px (Tauri appearance.ui_font_size).
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: u16,
    /// Terminal regular font weight (Tauri appearance.font_weight).
    #[serde(default = "default_terminal_font_weight")]
    pub terminal_font_weight: u16,
    /// Terminal bold font weight (Tauri appearance.font_weight_bold).
    #[serde(default = "default_terminal_font_weight_bold")]
    pub terminal_font_weight_bold: u16,
    pub x11_display: String,
    pub terminal_scrollback_lines: u32,
    #[serde(default = "default_terminal_keep_alive_mode")]
    pub terminal_keep_alive_mode: String,
    pub terminal_keep_alive_interval: u32,
    #[serde(default = "default_terminal_timestamp_format")]
    pub terminal_timestamp_format: String,
    pub terminal_hardware_acceleration: bool,
    pub terminal_show_workspace_padding: bool,
    pub terminal_show_line_numbers: bool,
    pub terminal_show_timestamps: bool,
    pub terminal_show_multi_line_paste_dialog: bool,
    pub terminal_paste_image_as_path: bool,
    #[serde(default)]
    pub terminal_reconnect_restore_cwd: bool,
    #[serde(default)]
    pub terminal_low_latency_mode: bool,
    #[serde(default = "default_terminal_zebra_stripes_enabled")]
    pub terminal_zebra_stripes_enabled: bool,
    /// Detect clickable entities (IP/host:port/archive) in terminal output (Tauri action_links_enabled).
    #[serde(default)]
    pub terminal_action_links_enabled: bool,
    #[serde(default)]
    pub terminal_action_links_matchers: ActionLinksMatcherSettings,
    /// Online search engines for terminal selection context menu (Tauri search.custom_engines).
    #[serde(default = "default_search_engines")]
    pub search_custom_engines: Vec<SearchEngineConfig>,
    /// Whether the Tauri-compatible Notes panel is available in the activity bar.
    #[serde(default = "default_true")]
    pub ui_show_notes_panel: bool,
    pub ui_show_remote_stats: bool,
    pub ui_remote_stats_interval: u32,
    #[serde(default)]
    pub ui_show_gpu_monitor: bool,
    #[serde(default = "default_hardware_monitor_interval")]
    pub ui_gpu_monitor_interval: u32,
    #[serde(default)]
    pub ui_show_ascend_npu_monitor: bool,
    #[serde(default = "default_hardware_monitor_interval")]
    pub ui_ascend_npu_monitor_interval: u32,
    pub ui_show_process_manager: bool,
    pub ui_process_manager_interval: u32,
    pub ui_show_docker_manager: bool,
    pub ui_docker_manager_interval: u32,
    #[serde(default = "default_quick_cmd_view_mode")]
    pub ui_quick_cmd_view_mode: String,
    #[serde(default = "default_quick_cmd_sort_mode")]
    pub ui_quick_cmd_sort_mode: String,
    #[serde(default = "default_saved_connections_sort_mode")]
    pub ui_saved_connections_sort_mode: String,
    #[serde(default)]
    pub ui_saved_connections_expanded_group_ids: Vec<String>,
    /// Which surface opens on launch: `workbench` or `assets` (Tauri
    /// `ui.start_workspace_mode`). Normalized on read/write.
    #[serde(default = "default_start_workspace_mode")]
    pub ui_start_workspace_mode: String,
    /// Asset workspace sort column key (Tauri `ui.asset_sort_key`); `None`
    /// leaves the column unset so older data round-trips unchanged.
    #[serde(default)]
    pub ui_asset_sort_key: Option<String>,
    /// Asset workspace sort direction (`asc` / `desc`, Tauri
    /// `ui.asset_sort_direction`); `None` leaves it unset.
    #[serde(default)]
    pub ui_asset_sort_direction: Option<String>,
    /// Which reading the title bar's centre shows: `session`, `resources`,
    /// `host` or `datetime`.
    #[serde(default = "default_header_status_mode")]
    pub ui_header_status_mode: String,
    #[serde(default = "default_true")]
    pub ui_header_status_visible: bool,
    #[serde(default = "default_true")]
    pub ui_file_explorer_show_hidden_files: bool,
    #[serde(default)]
    pub ui_file_explorer_auto_sync_cwd_connection_ids: Vec<String>,
    #[serde(default)]
    pub ui_file_explorer_favorite_dirs_by_connection_id: HashMap<String, Vec<String>>,
    #[serde(default = "default_left_panel_width")]
    pub ui_left_panel_width: u32,
    #[serde(default = "default_right_panel_width")]
    pub ui_right_panel_width: u32,
    #[serde(default = "default_transfer_height")]
    pub ui_transfer_height: u32,
    #[serde(default = "default_quick_cmd_height")]
    pub ui_quick_cmd_height: u32,
    /// Whether the Tauri-compatible Quick Commands bottom panel is visible.
    #[serde(default = "default_true")]
    pub ui_quick_cmd_visible: bool,
    #[serde(default = "default_serial_send_height")]
    pub ui_serial_send_height: u32,
    /// Whether the Tauri-compatible Command Send bottom panel is visible.
    #[serde(default)]
    pub ui_serial_send_visible: bool,
    #[serde(default)]
    pub ui_active_left_panel: Option<String>,
    #[serde(default)]
    pub ui_active_right_panel: Option<String>,
    #[serde(default)]
    pub ui_left_panel_collapsed: bool,
    #[serde(default)]
    pub ui_right_panel_collapsed: bool,
    #[serde(default = "default_activity_left_top")]
    pub ui_activity_bar_left_top: Vec<String>,
    #[serde(default = "default_activity_left_bottom")]
    pub ui_activity_bar_left_bottom: Vec<String>,
    #[serde(default = "default_activity_right_top")]
    pub ui_activity_bar_right_top: Vec<String>,
    #[serde(default = "default_activity_right_bottom")]
    pub ui_activity_bar_right_bottom: Vec<String>,
    #[serde(default)]
    pub ui_activity_bar_show_labels: bool,
    /// Activity-bar item ids the user has hidden from the rail (Tauri
    /// `ui.activity_bar_layout.hidden_items`). Empty by default so legacy
    /// data with no hidden entries keeps every item visible.
    #[serde(default)]
    pub ui_activity_bar_hidden_items: Vec<String>,
    /// Docked multi-open preference. This is independent from
    /// `ui_panel_open_mode`: main keeps multi-open configured while floating
    /// panels are active and applies it again after returning to docked mode.
    #[serde(default)]
    pub ui_panel_multi_open: bool,
    /// Tauri-compatible panel presentation mode: `"docked"` or `"floating"`.
    #[serde(default = "default_panel_open_mode")]
    pub ui_panel_open_mode: String,
    #[serde(default)]
    pub ui_left_open_panels: Vec<String>,
    #[serde(default)]
    pub ui_right_open_panels: Vec<String>,
    #[serde(default)]
    pub ui_panel_stack_sizes: HashMap<String, u32>,
    pub interaction_copy_on_select: bool,
    #[serde(default)]
    pub interaction_allow_osc52_clipboard_write: bool,
    pub interaction_right_click_paste: bool,
    #[serde(default = "default_true")]
    pub interaction_terminal_zoom_enabled: bool,
    pub interaction_command_suggestions_enabled: bool,
    pub interaction_command_suggestion_min_chars: u32,
    pub interaction_command_suggestion_max_chars: u32,
    pub interaction_word_separators: String,
    pub interaction_duplicate_session_command_delay_ms: u32,
    pub interaction_alt_as_meta: bool,
    pub interaction_mac_ime_compatibility: bool,
    pub interaction_tab_double_click_action: String,
    pub interaction_tab_middle_click_action: String,
    pub interaction_tab_right_click_action: String,
    pub interaction_default_encoding: String,
    pub host_key_policy: String,
    pub transfer_download_path: String,
    pub transfer_ask_save_location: bool,
    pub transfer_duplicate_strategy: String,
    pub transfer_editor_type: String,
    #[serde(default = "default_internal_editor_font_size")]
    pub transfer_internal_editor_font_size: u16,
    pub transfer_default_editor: String,
    pub transfer_download_threads: u32,
    pub transfer_upload_threads: u32,
    pub transfer_max_retries: u32,
    pub transfer_buffer_size: u32,
    pub transfer_default_file_permissions: String,
    pub transfer_preserve_timestamps: bool,
    pub transfer_resume_broken_transfer: bool,
    pub recording_path: String,
    pub recording_auto_start: bool,
    #[serde(default)]
    pub recording_default_mode: RecordingMode,
    #[serde(default = "default_recording_path_template")]
    pub recording_path_template: String,
    pub recording_include_io_labels: bool,
    pub recording_include_timestamps: bool,
    #[serde(default = "default_true")]
    pub recording_include_session_metadata: bool,
    #[serde(default)]
    pub recording_rotation: RecordingRotationPolicy,
    #[serde(default)]
    pub recording_existing_file_behavior: ExistingFileBehavior,
    #[serde(default)]
    pub recording_include_binary_transfer_payloads: bool,
    pub recording_memory_limit_bytes: u64,
    pub diagnostics_level: String,
    pub diagnostics_retention_days: u32,
    pub startup_restore: bool,
    /// When true (default), restore multi-leaf tab window layout with sessions.
    #[serde(default = "default_true")]
    pub startup_restore_window_layout: bool,
    /// When true, minimize hides the main window to the system tray (platform-dependent).
    #[serde(default)]
    pub minimize_to_tray: bool,
    pub confirm_on_close: bool,
    pub enable_screen_lock: bool,
    pub idle_lock_minutes: u32,
    pub has_master_password: bool,
    pub keybindings: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

impl Default for KeywordHighlightRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            patterns: Vec::new(),
            color_dark: default_highlight_color_dark(),
            color_light: default_highlight_color_light(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct KeywordHighlightConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Legacy Tauri setting retained for round trips. Native highlighting always
    /// matches soft-wrapped logical lines and stops at actual line breaks.
    #[serde(default)]
    pub across_wrapped_lines: bool,
    /// Per built-in rule enable map (Tauri `keyword_highlight_builtin_rules`).
    #[serde(default)]
    pub builtin_rules: std::collections::HashMap<String, bool>,
    #[serde(default)]
    pub rules: Vec<KeywordHighlightRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordHighlightImportResult {
    pub imported_rules: usize,
    pub updated_rules: usize,
    pub total_rules: usize,
}

impl Default for AppSettingsSummary {
    fn default() -> Self {
        Self {
            theme: "github-dark".to_string(),
            background_image_path: None,
            background_image_fit: default_background_image_fit(),
            background_image_opacity: default_background_image_opacity(),
            background_content_opacity: default_background_content_opacity(),
            language: "zh-CN".to_string(),
            terminal_font_family: "JetBrains Mono".to_string(),
            terminal_font_size: 16,
            cursor_style: default_cursor_style(),
            cursor_blink: default_cursor_blink(),
            terminal_theme: None,
            minimum_contrast_ratio: default_minimum_contrast_ratio(),
            ui_font_family: default_ui_font_family(),
            ui_font_size: default_ui_font_size(),
            terminal_font_weight: default_terminal_font_weight(),
            terminal_font_weight_bold: default_terminal_font_weight_bold(),
            x11_display: String::new(),
            terminal_scrollback_lines: 5000,
            terminal_keep_alive_mode: default_terminal_keep_alive_mode(),
            terminal_keep_alive_interval: 30,
            terminal_timestamp_format: default_terminal_timestamp_format(),
            terminal_hardware_acceleration: true,
            terminal_show_workspace_padding: false,
            terminal_show_line_numbers: false,
            terminal_show_timestamps: false,
            terminal_show_multi_line_paste_dialog: true,
            terminal_paste_image_as_path: true,
            terminal_reconnect_restore_cwd: false,
            terminal_low_latency_mode: false,
            terminal_zebra_stripes_enabled: default_terminal_zebra_stripes_enabled(),
            terminal_action_links_enabled: false,
            terminal_action_links_matchers: ActionLinksMatcherSettings::default(),
            search_custom_engines: default_search_engines(),
            ui_show_notes_panel: true,
            ui_show_remote_stats: true,
            ui_remote_stats_interval: 3,
            ui_show_gpu_monitor: false,
            ui_gpu_monitor_interval: 3,
            ui_show_ascend_npu_monitor: false,
            ui_ascend_npu_monitor_interval: 3,
            ui_show_process_manager: true,
            ui_process_manager_interval: 5,
            ui_show_docker_manager: true,
            ui_docker_manager_interval: 10,
            ui_quick_cmd_view_mode: default_quick_cmd_view_mode(),
            ui_quick_cmd_sort_mode: default_quick_cmd_sort_mode(),
            ui_saved_connections_sort_mode: default_saved_connections_sort_mode(),
            ui_saved_connections_expanded_group_ids: Vec::new(),
            ui_start_workspace_mode: default_start_workspace_mode(),
            ui_asset_sort_key: None,
            ui_asset_sort_direction: None,
            ui_header_status_mode: default_header_status_mode(),
            ui_header_status_visible: true,
            ui_file_explorer_show_hidden_files: true,
            ui_file_explorer_auto_sync_cwd_connection_ids: Vec::new(),
            ui_file_explorer_favorite_dirs_by_connection_id: HashMap::new(),
            ui_left_panel_width: 256,
            ui_right_panel_width: 288,
            ui_transfer_height: 180,
            ui_quick_cmd_height: 180,
            ui_quick_cmd_visible: true,
            ui_serial_send_height: 180,
            ui_serial_send_visible: false,
            ui_active_left_panel: Some("fileExplorer".to_string()),
            ui_active_right_panel: Some("savedConnections".to_string()),
            ui_left_panel_collapsed: false,
            ui_right_panel_collapsed: false,
            ui_activity_bar_left_top: default_activity_left_top(),
            ui_activity_bar_left_bottom: default_activity_left_bottom(),
            ui_activity_bar_right_top: default_activity_right_top(),
            ui_activity_bar_right_bottom: default_activity_right_bottom(),
            ui_activity_bar_show_labels: false,
            ui_activity_bar_hidden_items: Vec::new(),
            ui_panel_multi_open: false,
            ui_panel_open_mode: default_panel_open_mode(),
            ui_left_open_panels: Vec::new(),
            ui_right_open_panels: Vec::new(),
            ui_panel_stack_sizes: HashMap::new(),
            interaction_copy_on_select: false,
            interaction_allow_osc52_clipboard_write: false,
            interaction_right_click_paste: false,
            interaction_terminal_zoom_enabled: true,
            interaction_command_suggestions_enabled: true,
            interaction_command_suggestion_min_chars: 2,
            interaction_command_suggestion_max_chars: 64,
            interaction_word_separators: " \t\r\n()[]{}\"':=,;|&<>".to_string(),
            interaction_duplicate_session_command_delay_ms: 1000,
            interaction_alt_as_meta: false,
            interaction_mac_ime_compatibility: true,
            interaction_tab_double_click_action: "disconnect_session".to_string(),
            interaction_tab_middle_click_action: "rename_tab".to_string(),
            interaction_tab_right_click_action: "none".to_string(),
            interaction_default_encoding: "UTF-8".to_string(),
            host_key_policy: "prompt".to_string(),
            transfer_download_path: String::new(),
            transfer_ask_save_location: false,
            transfer_duplicate_strategy: "ask".to_string(),
            transfer_editor_type: "external".to_string(),
            transfer_internal_editor_font_size: default_internal_editor_font_size(),
            transfer_default_editor: String::new(),
            transfer_download_threads: 3,
            transfer_upload_threads: 3,
            transfer_max_retries: 2,
            transfer_buffer_size: 32,
            transfer_default_file_permissions: "644".to_string(),
            transfer_preserve_timestamps: true,
            transfer_resume_broken_transfer: true,
            recording_path: String::new(),
            recording_auto_start: false,
            recording_default_mode: RecordingMode::Transcript,
            recording_path_template: default_recording_path_template(),
            recording_include_io_labels: true,
            recording_include_timestamps: true,
            recording_include_session_metadata: true,
            recording_rotation: RecordingRotationPolicy::Session,
            recording_existing_file_behavior: ExistingFileBehavior::Unique,
            recording_include_binary_transfer_payloads: false,
            recording_memory_limit_bytes: 5 * 1024 * 1024,
            diagnostics_level: "info".to_string(),
            diagnostics_retention_days: 7,
            startup_restore: false,
            startup_restore_window_layout: true,
            minimize_to_tray: false,
            confirm_on_close: true,
            enable_screen_lock: false,
            idle_lock_minutes: 0,
            has_master_password: false,
            keybindings: HashMap::new(),
        }
    }
}

fn default_activity_left_top() -> Vec<String> {
    vec![
        "fileExplorer".to_string(),
        "notes".to_string(),
        "network".to_string(),
        "securityAuth".to_string(),
    ]
}

fn default_activity_left_bottom() -> Vec<String> {
    vec!["syncBackupHistory".to_string(), "settings".to_string()]
}

fn default_activity_right_top() -> Vec<String> {
    vec![
        "savedConnections".to_string(),
        "aiAssistant".to_string(),
        "activeSessions".to_string(),
        "commandHistory".to_string(),
        "resourceMonitor".to_string(),
        "gpuMonitor".to_string(),
        "ascendNpuMonitor".to_string(),
        "processManager".to_string(),
        "dockerManager".to_string(),
    ]
}

fn default_activity_right_bottom() -> Vec<String> {
    vec![
        "quickCmdBar".to_string(),
        "serialSend".to_string(),
        "recording".to_string(),
        "lock".to_string(),
    ]
}

fn default_left_panel_width() -> u32 {
    256
}

fn default_right_panel_width() -> u32 {
    288
}

fn default_transfer_height() -> u32 {
    180
}

fn default_quick_cmd_height() -> u32 {
    180
}

fn default_serial_send_height() -> u32 {
    180
}

fn default_quick_cmd_view_mode() -> String {
    "tile".to_string()
}

fn default_quick_cmd_sort_mode() -> String {
    "created".to_string()
}

fn default_header_status_mode() -> String {
    "session".to_string()
}

/// Tauri defaults panel presentation to docked. Multi-open is a separate
/// appearance preference and must not be encoded into this field.
pub fn default_panel_open_mode() -> String {
    "docked".to_string()
}

/// Only the explicit `floating` value enables floating presentation. Legacy,
/// unknown and obsolete values safely fall back to docked.
pub fn normalize_panel_open_mode(raw: &str) -> String {
    if raw.trim().eq_ignore_ascii_case("floating") {
        "floating".to_string()
    } else {
        "docked".to_string()
    }
}

fn default_terminal_keep_alive_mode() -> String {
    "compatible".to_string()
}

fn default_internal_editor_font_size() -> u16 {
    13
}

fn default_terminal_timestamp_format() -> String {
    DEFAULT_TERMINAL_TIMESTAMP_FORMAT.to_string()
}

fn default_terminal_zebra_stripes_enabled() -> bool {
    true
}

fn default_hardware_monitor_interval() -> u32 {
    3
}

fn default_recording_path_template() -> String {
    DEFAULT_RECORDING_PATH_TEMPLATE.to_string()
}

fn default_saved_connections_sort_mode() -> String {
    "default".to_string()
}

fn default_start_workspace_mode() -> String {
    "workbench".to_string()
}

fn default_background_image_fit() -> String {
    "cover".to_string()
}

fn default_minimum_contrast_ratio() -> String {
    "1".to_string()
}

fn default_ui_font_family() -> String {
    "Inter".to_string()
}

fn default_ui_font_size() -> u16 {
    16
}

fn default_terminal_font_weight() -> u16 {
    400
}

fn default_terminal_font_weight_bold() -> u16 {
    700
}

fn default_cursor_style() -> String {
    "block".to_string()
}

fn default_cursor_blink() -> bool {
    true
}

fn default_background_image_opacity() -> u8 {
    45
}

fn default_background_content_opacity() -> u8 {
    82
}

fn default_highlight_color_dark() -> String {
    "#79c0ff".to_string()
}

fn default_highlight_color_light() -> String {
    "#0969da".to_string()
}

#[cfg(test)]
mod tests {
    use super::{AppSettingsSummary, default_panel_open_mode, normalize_panel_open_mode};

    #[test]
    fn default_panel_open_mode_is_docked() {
        assert_eq!(default_panel_open_mode(), "docked");
    }

    #[test]
    fn normalize_panel_open_mode_accepts_only_floating() {
        assert_eq!(normalize_panel_open_mode("floating"), "floating");
        assert_eq!(normalize_panel_open_mode(" FLOATING "), "floating");
    }

    #[test]
    fn normalize_panel_open_mode_falls_back_to_docked() {
        assert_eq!(normalize_panel_open_mode("docked"), "docked");
        assert_eq!(normalize_panel_open_mode("multi"), "docked");
        assert_eq!(normalize_panel_open_mode(""), "docked");
        assert_eq!(normalize_panel_open_mode("unknown"), "docked");
    }

    #[test]
    fn summary_default_has_empty_hidden_items_and_docked_mode() {
        let summary = AppSettingsSummary::default();
        assert!(summary.ui_activity_bar_hidden_items.is_empty());
        assert_eq!(summary.ui_panel_open_mode, "docked");
        assert!(!summary.ui_panel_multi_open);
    }

    #[test]
    fn summary_deserializes_missing_new_fields_to_defaults() {
        // A minimal JSON document (legacy shape) must fill the new fields with
        // serde defaults rather than failing to deserialize. Built from a raw
        // string to avoid the `json!` macro recursion limit on this many keys.
        let json = r#"{
            "theme": "github-dark",
            "language": "en",
            "terminal_font_family": "JetBrains Mono",
            "terminal_font_size": 16,
            "x11_display": "",
            "terminal_scrollback_lines": 5000,
            "terminal_keep_alive_interval": 30,
            "terminal_hardware_acceleration": true,
            "terminal_show_workspace_padding": false,
            "terminal_show_line_numbers": false,
            "terminal_show_timestamps": false,
            "terminal_show_multi_line_paste_dialog": true,
            "terminal_paste_image_as_path": true,
            "ui_show_remote_stats": true,
            "ui_remote_stats_interval": 3,
            "ui_show_process_manager": true,
            "ui_process_manager_interval": 5,
            "ui_show_docker_manager": true,
            "ui_docker_manager_interval": 10,
            "interaction_copy_on_select": false,
            "interaction_right_click_paste": false,
            "interaction_command_suggestions_enabled": true,
            "interaction_command_suggestion_min_chars": 2,
            "interaction_command_suggestion_max_chars": 64,
            "interaction_word_separators": " ",
            "interaction_duplicate_session_command_delay_ms": 1000,
            "interaction_alt_as_meta": false,
            "interaction_mac_ime_compatibility": true,
            "interaction_tab_double_click_action": "disconnect_session",
            "interaction_tab_middle_click_action": "rename_tab",
            "interaction_tab_right_click_action": "none",
            "interaction_default_encoding": "UTF-8",
            "host_key_policy": "prompt",
            "transfer_download_path": "",
            "transfer_ask_save_location": false,
            "transfer_duplicate_strategy": "ask",
            "transfer_editor_type": "external",
            "transfer_default_editor": "",
            "transfer_download_threads": 3,
            "transfer_upload_threads": 3,
            "transfer_max_retries": 2,
            "transfer_buffer_size": 32,
            "transfer_default_file_permissions": "644",
            "transfer_preserve_timestamps": true,
            "transfer_resume_broken_transfer": true,
            "recording_path": "",
            "recording_auto_start": false,
            "recording_include_io_labels": true,
            "recording_include_timestamps": true,
            "recording_memory_limit_bytes": 5242880,
            "diagnostics_level": "info",
            "diagnostics_retention_days": 7,
            "startup_restore": false,
            "confirm_on_close": true,
            "enable_screen_lock": false,
            "idle_lock_minutes": 0,
            "has_master_password": false,
            "keybindings": {}
        }"#;

        let summary: AppSettingsSummary =
            serde_json::from_str(json).expect("legacy summary deserializes");
        assert!(summary.ui_activity_bar_hidden_items.is_empty());
        assert_eq!(summary.ui_panel_open_mode, "docked");
        assert!(!summary.ui_panel_multi_open);
        // Legacy documents lack the asset/start-workspace UI settings; they must
        // fall back to their serde defaults instead of failing to load.
        assert_eq!(summary.ui_start_workspace_mode, "workbench");
        assert!(summary.ui_asset_sort_key.is_none());
        assert!(summary.ui_asset_sort_direction.is_none());
        assert!(!summary.terminal_reconnect_restore_cwd);
        assert_eq!(summary.transfer_internal_editor_font_size, 13);
    }

    #[test]
    fn summary_default_start_workspace_and_asset_sort() {
        let summary = AppSettingsSummary::default();
        assert_eq!(summary.ui_start_workspace_mode, "workbench");
        assert!(summary.ui_asset_sort_key.is_none());
        assert!(summary.ui_asset_sort_direction.is_none());
    }

    #[test]
    fn summary_roundtrips_asset_and_start_workspace_settings() {
        let summary = AppSettingsSummary {
            ui_start_workspace_mode: "assets".to_string(),
            ui_asset_sort_key: Some("hostname".to_string()),
            ui_asset_sort_direction: Some("desc".to_string()),
            ..AppSettingsSummary::default()
        };

        let encoded = serde_json::to_string(&summary).expect("serializes");
        let decoded: AppSettingsSummary = serde_json::from_str(&encoded).expect("round-trips");
        assert_eq!(decoded.ui_start_workspace_mode, "assets");
        assert_eq!(decoded.ui_asset_sort_key.as_deref(), Some("hostname"));
        assert_eq!(decoded.ui_asset_sort_direction.as_deref(), Some("desc"));
    }
}
