pub mod panel_visibility;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::default_true;
use super::window_decoration::WindowDecoration;
use super::window_material::{WindowMaterial, default_window_background_opacity};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TransferBrowserViewMode {
    #[default]
    List,
    Tree,
}

impl TransferBrowserViewMode {
    pub fn from_compat_value(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("tree") {
            Self::Tree
        } else {
            Self::List
        }
    }

    pub const fn compat_value(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Tree => "tree",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileManagerUiSettings {
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub left: FileManagerPaneRestore,
    #[serde(default)]
    pub right: FileManagerPaneRestore,
    #[serde(default = "default_file_manager_split_percent")]
    pub split_percent: u8,
    #[serde(default)]
    pub active_pane: FileManagerPaneSide,
    #[serde(default)]
    pub queue_expanded: bool,
    #[serde(default = "default_file_manager_queue_height")]
    pub queue_height: u32,
    #[serde(default = "default_true")]
    pub auto_collapse_sidebar: bool,
    #[serde(default = "default_true")]
    pub remember_layout: bool,
    #[serde(default)]
    pub left_view: FileManagerPaneUiSettings,
    #[serde(default)]
    pub right_view: FileManagerPaneUiSettings,
}

impl FileManagerUiSettings {
    /// Read each preference independently so a damaged layout value cannot erase
    /// valid source memories. The store retains the original extension fields.
    pub fn from_compat_value(value: &serde_json::Value) -> Self {
        let mut result = Self::default();
        macro_rules! read {
            ($($field:ident),* $(,)?) => { $(
                if let Some(value) = value.get(stringify!($field))
                    && let Ok(parsed) = serde_json::from_value(value.clone()) {
                    result.$field = parsed;
                }
            )* };
        }
        read!(
            open,
            left,
            right,
            active_pane,
            queue_expanded,
            auto_collapse_sidebar,
            remember_layout
        );
        if let Some(split) = value
            .get("split_percent")
            .and_then(serde_json::Value::as_i64)
        {
            result.split_percent = split.clamp(20, 80) as u8;
        }
        if let Some(height) = value
            .get("queue_height")
            .and_then(serde_json::Value::as_i64)
        {
            result.queue_height = height.clamp(80, 600) as u32;
        }
        for (key, target) in [
            ("left_view", &mut result.left_view),
            ("right_view", &mut result.right_view),
        ] {
            let Some(view) = value.get(key) else {
                continue;
            };
            if let Some(widths) = view
                .get("column_widths")
                .and_then(serde_json::Value::as_object)
            {
                for (key, width) in widths {
                    if let Some(width) = width.as_i64() {
                        target
                            .column_widths
                            .insert(key.clone(), width.clamp(72, 2000) as u32);
                    }
                }
            }
            if let Some(columns) = view
                .get("visible_columns")
                .and_then(serde_json::Value::as_array)
            {
                target.visible_columns = columns
                    .iter()
                    .filter_map(|column| column.as_str().map(str::to_owned))
                    .collect();
                if !target.visible_columns.iter().any(|key| key == "name") {
                    target.visible_columns.insert(0, "name".into());
                }
            }
            if let Some(sort) = view.get("sort_column").and_then(serde_json::Value::as_str) {
                target.sort_column = sort.into();
            }
            if let Some(descending) = view.get("descending").and_then(serde_json::Value::as_bool) {
                target.descending = descending;
            }
            target.row_height =
                if view.get("row_height").and_then(serde_json::Value::as_u64) == Some(28) {
                    28
                } else {
                    30
                };
            if let Some(directories) = view
                .get("directories")
                .and_then(serde_json::Value::as_array)
            {
                target.directories = directories
                    .iter()
                    .filter_map(|directory| serde_json::from_value(directory.clone()).ok())
                    .collect();
            }
        }
        result
    }
}

impl Default for FileManagerUiSettings {
    fn default() -> Self {
        Self {
            open: false,
            left: FileManagerPaneRestore::default(),
            right: FileManagerPaneRestore::default(),
            split_percent: default_file_manager_split_percent(),
            active_pane: FileManagerPaneSide::Left,
            queue_expanded: false,
            queue_height: default_file_manager_queue_height(),
            auto_collapse_sidebar: true,
            remember_layout: true,
            left_view: FileManagerPaneUiSettings::default(),
            right_view: FileManagerPaneUiSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileManagerPaneRestore {
    #[serde(default)]
    pub source: FileManagerSourceRestore,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub raw_path_token: Option<String>,
}

/// Presentation preferences and successful directories only; never live listings
/// or credentials. String column keys allow newer versions to retain extra columns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FileManagerPaneUiSettings {
    pub column_widths: HashMap<String, u32>,
    pub visible_columns: Vec<String>,
    pub sort_column: String,
    pub descending: bool,
    pub row_height: u32,
    pub directories: Vec<FileManagerPaneRestore>,
}

impl Default for FileManagerPaneUiSettings {
    fn default() -> Self {
        Self {
            column_widths: HashMap::new(),
            visible_columns: ["name", "size", "modified", "permissions"]
                .map(str::to_string)
                .to_vec(),
            sort_column: "name".into(),
            descending: false,
            row_height: 30,
            directories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileManagerSourceRestore {
    #[default]
    Local,
    Connection {
        #[serde(default)]
        id: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileManagerPaneSide {
    #[default]
    Left,
    Right,
}

fn default_file_manager_split_percent() -> u8 {
    50
}

fn default_file_manager_queue_height() -> u32 {
    160
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRightClickAction {
    None,
    #[default]
    Menu,
    Paste,
}

impl TerminalRightClickAction {
    pub fn from_compat_value(value: &str) -> Self {
        match value {
            "none" => Self::None,
            "paste" => Self::Paste,
            _ => Self::Menu,
        }
    }

    pub const fn compat_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Menu => "menu",
            Self::Paste => "paste",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InternalEditorDisplay {
    Workspace,
    #[default]
    Window,
}

impl InternalEditorDisplay {
    pub fn from_compat_value(value: &str) -> Self {
        if value == "workspace" {
            Self::Workspace
        } else {
            Self::Window
        }
    }

    pub const fn compat_value(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Window => "window",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionLinksMatcherSettings {
    #[serde(default = "default_true_action_link")]
    pub mac: bool,
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
            mac: true,
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
    pub custom_themes: crate::themes::ThemeCatalog,
    #[serde(default)]
    pub background_image_path: Option<String>,
    #[serde(default = "default_background_image_fit")]
    pub background_image_fit: String,
    /// Wallpaper opacity percent (0..=100).
    #[serde(default = "default_background_image_opacity")]
    pub background_image_opacity: u8,
    #[serde(default)]
    pub window_material: WindowMaterial,
    /// Device-local Linux window decoration preference.
    #[serde(default)]
    pub window_decorations: WindowDecoration,
    /// Theme tint below wallpaper and content surfaces, in percent (0..=100).
    #[serde(default = "default_window_background_opacity")]
    pub window_background_opacity: u8,
    /// Shell chrome opacity percent when wallpaper or native transparency is active (0..=100).
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
    /// Use the intense default foreground for bold cells without an explicit ANSI color.
    #[serde(default)]
    pub bold_default_foreground: bool,
    pub x11_display: String,
    pub terminal_scrollback_lines: u32,
    #[serde(default = "default_terminal_keep_alive_mode")]
    pub terminal_keep_alive_mode: String,
    pub terminal_keep_alive_interval: u32,
    #[serde(default = "default_terminal_timestamp_format")]
    pub terminal_timestamp_format: String,
    /// Retained only to round-trip legacy settings; GPUI owns renderer selection.
    pub terminal_hardware_acceleration: bool,
    pub terminal_show_workspace_padding: bool,
    pub terminal_show_line_numbers: bool,
    pub terminal_show_timestamps: bool,
    pub terminal_show_multi_line_paste_dialog: bool,
    pub terminal_paste_image_as_path: bool,
    #[serde(default)]
    pub terminal_reconnect_restore_cwd: bool,
    /// Retained only to round-trip legacy settings; no longer affects terminal behavior.
    #[serde(default)]
    pub terminal_low_latency_mode: bool,
    #[serde(default = "default_terminal_zebra_stripes_enabled")]
    pub terminal_zebra_stripes_enabled: bool,
    /// Detect clickable entities (IP/MAC/host:port/archive) in terminal output (Tauri action_links_enabled).
    #[serde(default)]
    pub terminal_action_links_enabled: bool,
    #[serde(default)]
    pub terminal_action_links_matchers: ActionLinksMatcherSettings,
    /// Online search engines for terminal selection context menu (Tauri search.custom_engines).
    #[serde(default = "default_search_engines")]
    pub search_custom_engines: Vec<SearchEngineConfig>,
    pub ui_remote_stats_interval: u32,
    /// Show `who` sessions in the resource monitor and the host header reading.
    #[serde(default)]
    pub ui_remote_stats_show_users: bool,
    #[serde(default = "default_hardware_monitor_interval")]
    pub ui_gpu_monitor_interval: u32,
    #[serde(default = "default_hardware_monitor_interval")]
    pub ui_ascend_npu_monitor_interval: u32,
    pub ui_process_manager_interval: u32,
    pub ui_docker_manager_interval: u32,
    #[serde(default = "default_quick_cmd_view_mode")]
    pub ui_quick_cmd_view_mode: String,
    #[serde(default = "default_quick_cmd_sort_mode")]
    pub ui_quick_cmd_sort_mode: String,
    #[serde(default = "default_quick_cmd_selected_category")]
    pub ui_quick_cmd_selected_category: String,
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
    pub ui_file_explorer_view_mode: TransferBrowserViewMode,
    #[serde(default)]
    pub ui_file_explorer_auto_sync_cwd_connection_ids: Vec<String>,
    #[serde(default)]
    pub ui_file_explorer_favorite_dirs_by_connection_id: HashMap<String, Vec<String>>,
    /// Dual-pane file manager layout. Independent of the sidebar explorer.
    #[serde(default)]
    pub ui_file_manager: FileManagerUiSettings,
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
    #[serde(default)]
    pub ui_serial_send_clear_after_send: bool,
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
    /// Hidden activity-bar entries. Legacy switches are migrated by the store.
    #[serde(default = "panel_visibility::default_hidden_items")]
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
    #[serde(default)]
    pub interaction_terminal_right_click_action: TerminalRightClickAction,
    #[serde(default)]
    pub interaction_mouse_events_require_alt: bool,
    #[serde(default = "default_true")]
    pub interaction_terminal_zoom_enabled: bool,
    pub interaction_command_suggestions_enabled: bool,
    /// Gates terminal credential suggestions and pending password autofill.
    /// Defaults on for existing data that predates the field.
    #[serde(default = "default_true")]
    pub interaction_credential_autofill_enabled: bool,
    pub interaction_command_suggestion_min_chars: u32,
    pub interaction_command_suggestion_max_chars: u32,
    pub interaction_word_separators: String,
    pub interaction_duplicate_session_command_delay_ms: u32,
    pub interaction_alt_as_meta: bool,
    /// Retained only to round-trip legacy settings. Native terminal IME is always enabled.
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
    #[serde(default)]
    pub transfer_internal_editor_display: InternalEditorDisplay,
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
    #[serde(default)]
    pub recording_include_input: bool,
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
    #[serde(default)]
    pub enable_startup_lock: bool,
    #[serde(default)]
    pub enable_idle_lock: bool,
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
            window_material: WindowMaterial::default(),
            window_decorations: WindowDecoration::default(),
            window_background_opacity: default_window_background_opacity(),
            language: "zh-CN".to_string(),
            terminal_font_family: "JetBrains Mono".to_string(),
            terminal_font_size: 16,
            cursor_style: default_cursor_style(),
            cursor_blink: default_cursor_blink(),
            custom_themes: crate::themes::ThemeCatalog::default(),
            terminal_theme: None,
            minimum_contrast_ratio: default_minimum_contrast_ratio(),
            ui_font_family: default_ui_font_family(),
            ui_font_size: default_ui_font_size(),
            terminal_font_weight: default_terminal_font_weight(),
            terminal_font_weight_bold: default_terminal_font_weight_bold(),
            bold_default_foreground: false,
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
            ui_remote_stats_interval: 3,
            ui_remote_stats_show_users: false,
            ui_gpu_monitor_interval: 3,
            ui_ascend_npu_monitor_interval: 3,
            ui_process_manager_interval: 5,
            ui_docker_manager_interval: 10,
            ui_quick_cmd_view_mode: default_quick_cmd_view_mode(),
            ui_quick_cmd_sort_mode: default_quick_cmd_sort_mode(),
            ui_quick_cmd_selected_category: default_quick_cmd_selected_category(),
            ui_saved_connections_sort_mode: default_saved_connections_sort_mode(),
            ui_saved_connections_expanded_group_ids: Vec::new(),
            ui_start_workspace_mode: default_start_workspace_mode(),
            ui_asset_sort_key: None,
            ui_asset_sort_direction: None,
            ui_header_status_mode: default_header_status_mode(),
            ui_header_status_visible: true,
            ui_file_explorer_show_hidden_files: true,
            ui_file_explorer_view_mode: TransferBrowserViewMode::List,
            ui_file_explorer_auto_sync_cwd_connection_ids: Vec::new(),
            ui_file_explorer_favorite_dirs_by_connection_id: HashMap::new(),
            ui_file_manager: FileManagerUiSettings::default(),
            ui_left_panel_width: 256,
            ui_right_panel_width: 288,
            ui_transfer_height: 180,
            ui_quick_cmd_height: 180,
            ui_quick_cmd_visible: true,
            ui_serial_send_height: 180,
            ui_serial_send_clear_after_send: false,
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
            ui_activity_bar_hidden_items: panel_visibility::default_hidden_items(),
            ui_panel_multi_open: false,
            ui_panel_open_mode: default_panel_open_mode(),
            ui_left_open_panels: Vec::new(),
            ui_right_open_panels: Vec::new(),
            ui_panel_stack_sizes: HashMap::new(),
            interaction_copy_on_select: false,
            interaction_allow_osc52_clipboard_write: false,
            interaction_right_click_paste: false,
            interaction_terminal_right_click_action: TerminalRightClickAction::Menu,
            interaction_mouse_events_require_alt: false,
            interaction_terminal_zoom_enabled: true,
            interaction_command_suggestions_enabled: true,
            interaction_credential_autofill_enabled: true,
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
            transfer_internal_editor_display: InternalEditorDisplay::Window,
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
            recording_include_input: false,
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
            enable_startup_lock: false,
            enable_idle_lock: false,
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
    vec![
        "syncBackupHistory".to_string(),
        "plugins".to_string(),
        "settings".to_string(),
    ]
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

fn default_quick_cmd_selected_category() -> String {
    "all".to_string()
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

pub fn default_internal_editor_font_size() -> u16 {
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
    #[test]
    fn mac_matcher_defaults_on_for_legacy_settings_and_preserves_explicit_false() {
        for value in [serde_json::json!({}), serde_json::json!({"ipv4": false})] {
            let matchers: super::ActionLinksMatcherSettings =
                serde_json::from_value(value).unwrap();
            assert!(matchers.mac);
        }
        let matchers: super::ActionLinksMatcherSettings =
            serde_json::from_value(serde_json::json!({"mac": false})).unwrap();
        assert!(!matchers.mac);
        assert_eq!(serde_json::to_value(matchers).unwrap()["mac"], false);
        assert!(!super::AppSettingsSummary::default().terminal_action_links_enabled);
    }

    #[test]
    fn legacy_and_unknown_encoding_labels_round_trip_unchanged() {
        for label in ["UTF8", " CP936 ", "shift-jis", "euckr", "unknown", ""] {
            let mut document = serde_json::to_value(super::AppSettingsSummary::default()).unwrap();
            document["interaction_default_encoding"] = serde_json::json!(label);
            let settings: super::AppSettingsSummary = serde_json::from_value(document).unwrap();
            assert_eq!(
                serde_json::to_value(settings).unwrap()["interaction_default_encoding"],
                label
            );
        }
    }

    use super::{
        AppSettingsSummary, TransferBrowserViewMode, default_panel_open_mode,
        normalize_panel_open_mode,
    };

    #[test]
    fn file_explorer_view_mode_uses_tauri_compatible_values() {
        assert_eq!(
            TransferBrowserViewMode::from_compat_value("tree"),
            TransferBrowserViewMode::Tree
        );
        assert_eq!(
            TransferBrowserViewMode::from_compat_value(" TREE "),
            TransferBrowserViewMode::Tree
        );
        assert_eq!(
            TransferBrowserViewMode::from_compat_value("invalid"),
            TransferBrowserViewMode::List
        );
        assert_eq!(TransferBrowserViewMode::Tree.compat_value(), "tree");
        assert_eq!(TransferBrowserViewMode::List.compat_value(), "list");
    }

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
        assert_eq!(
            summary.ui_activity_bar_hidden_items,
            crate::models::settings::panel_visibility::default_hidden_items()
        );
        assert_eq!(summary.ui_panel_open_mode, "docked");
        assert!(!summary.ui_panel_multi_open);
        assert_eq!(
            summary.ui_file_explorer_view_mode,
            TransferBrowserViewMode::List
        );
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
        assert_eq!(
            summary.ui_activity_bar_hidden_items,
            crate::models::settings::panel_visibility::default_hidden_items()
        );
        assert_eq!(summary.ui_panel_open_mode, "docked");
        assert!(!summary.ui_panel_multi_open);
        // Legacy documents lack the asset/start-workspace UI settings; they must
        // fall back to their serde defaults instead of failing to load.
        assert_eq!(summary.ui_start_workspace_mode, "workbench");
        assert!(summary.ui_asset_sort_key.is_none());
        assert!(summary.ui_asset_sort_direction.is_none());
        assert!(!summary.terminal_reconnect_restore_cwd);
        assert!(!summary.ui_remote_stats_show_users);
        assert_eq!(summary.transfer_internal_editor_font_size, 13);
        // The credential autofill switch joins the command-suggestion switch in
        // defaulting on, so legacy documents keep the panel enabled.
        assert!(summary.interaction_credential_autofill_enabled);
    }

    #[test]
    fn login_monitoring_is_opt_in_for_legacy_settings_and_roundtrips() {
        let mut value = serde_json::to_value(AppSettingsSummary::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("ui_remote_stats_show_users");
        let mut summary: AppSettingsSummary = serde_json::from_value(value).unwrap();
        assert!(!summary.ui_remote_stats_show_users);
        summary.ui_remote_stats_show_users = true;
        let restored: AppSettingsSummary =
            serde_json::from_str(&serde_json::to_string(&summary).unwrap()).unwrap();
        assert!(restored.ui_remote_stats_show_users);
    }

    #[test]
    fn summary_default_start_workspace_and_asset_sort() {
        let summary = AppSettingsSummary::default();
        assert_eq!(summary.ui_quick_cmd_selected_category, "all");
        assert_eq!(summary.ui_start_workspace_mode, "workbench");
        assert!(summary.ui_asset_sort_key.is_none());
        assert!(summary.ui_asset_sort_direction.is_none());
    }

    #[test]
    fn missing_activity_layout_defaults_to_plugins_before_settings() {
        let mut value = serde_json::to_value(AppSettingsSummary::default()).expect("serializes");
        value
            .as_object_mut()
            .expect("object")
            .remove("ui_activity_bar_left_bottom");
        let summary: AppSettingsSummary = serde_json::from_value(value).expect("legacy settings");
        assert_eq!(
            summary.ui_activity_bar_left_bottom,
            ["syncBackupHistory", "plugins", "settings"]
        );
    }

    #[test]
    fn recording_input_is_explicit_opt_in_for_current_and_legacy_settings() {
        let mut value = serde_json::to_value(AppSettingsSummary::default()).unwrap();
        assert_eq!(value["recording_include_input"], false);
        value
            .as_object_mut()
            .unwrap()
            .remove("recording_include_input");
        let mut decoded: AppSettingsSummary = serde_json::from_value(value).unwrap();
        assert!(!decoded.recording_include_input);
        decoded.recording_include_input = true;
        let roundtrip: AppSettingsSummary =
            serde_json::from_str(&serde_json::to_string(&decoded).unwrap()).unwrap();
        assert!(roundtrip.recording_include_input);
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

    #[test]
    fn summary_missing_quick_command_category_defaults_to_all() {
        let mut value = serde_json::to_value(AppSettingsSummary::default()).expect("serializes");
        value
            .as_object_mut()
            .expect("settings object")
            .remove("ui_quick_cmd_selected_category");
        let summary: AppSettingsSummary = serde_json::from_value(value).expect("legacy settings");
        assert_eq!(summary.ui_quick_cmd_selected_category, "all");
    }
}

#[cfg(test)]
mod file_manager_tests {
    use super::{FileManagerPaneRestore, FileManagerSourceRestore, FileManagerUiSettings};
    use serde_json::json;

    #[test]
    fn old_file_manager_settings_load_with_new_defaults() {
        let settings: FileManagerUiSettings =
            serde_json::from_value(json!({"left": {"path": "/tmp"}, "split_percent": 40})).unwrap();
        assert_eq!(settings.left.path, "/tmp");
        assert_eq!(settings.left.raw_path_token, None);
        assert_eq!(settings.left_view.row_height, 30);
        assert_eq!(
            settings.left_view.visible_columns,
            ["name", "size", "modified", "permissions"]
        );
        assert!(settings.right_view.directories.is_empty());
    }

    #[test]
    fn file_manager_raw_directory_and_column_preferences_round_trip() {
        let mut settings = FileManagerUiSettings::default();
        settings.left_view.column_widths.insert("name".into(), 400);
        settings.left_view.row_height = 28;
        settings.left_view.directories.push(FileManagerPaneRestore {
            source: FileManagerSourceRestore::Connection {
                id: "saved-ssh".into(),
            },
            path: "/目录".into(),
            raw_path_token: Some("opaque".into()),
        });
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(FileManagerUiSettings::from_compat_value(&json), settings);
    }

    #[test]
    fn damaged_file_manager_layout_keeps_valid_directory_memories() {
        let value = json!({
            "split_percent": -50, "queue_height": 999999,
            "left_view": {"row_height": 600, "column_widths": {"size": -1, "modified": 50000, "permissions": "bad"},
                "visible_columns": ["size"], "directories": [{"source": {"kind": "local"}, "path": "/kept"}]},
            "right_view": {"directories": [{"source": {"kind": "future"}, "path": "/unknown"}, {"source": {"kind": "local"}, "path": "/valid"}]}
        });
        let settings = FileManagerUiSettings::from_compat_value(&value);
        assert_eq!(settings.split_percent, 20);
        assert_eq!(settings.queue_height, 600);
        assert_eq!(settings.left_view.row_height, 30);
        assert_eq!(settings.left_view.column_widths["size"], 72);
        assert_eq!(settings.left_view.column_widths["modified"], 2000);
        assert_eq!(settings.left_view.visible_columns, ["name", "size"]);
        assert_eq!(settings.left_view.directories[0].path, "/kept");
        assert_eq!(settings.right_view.directories[0].path, "/valid");
    }
}
