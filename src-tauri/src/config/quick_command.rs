use crate::error::AppResult;
use crate::storage::{self, SettingsDocKey};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

fn default_execute() -> String {
    "execute".to_string()
}

/// Single quick command (label + shell command).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickCommand {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color_tag: Option<String>,
    #[serde(default)]
    pub icon_tag: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "default_execute")]
    pub execution_mode: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub updated_at: Option<u64>,
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub use_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickCommandCategory {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
}

/// List of quick commands persisted in local app storage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuickCommandsConfig {
    pub commands: Vec<QuickCommand>,
    #[serde(default)]
    pub categories: Vec<QuickCommandCategory>,
}

/// Loads quick commands from local app storage.
pub fn load_quick_commands(app: &AppHandle) -> AppResult<QuickCommandsConfig> {
    let _ = app;
    storage::load_settings_doc(SettingsDocKey::QuickCommands)
}

/// Saves quick commands to local app storage.
pub fn save_quick_commands(app: &AppHandle, config: &QuickCommandsConfig) -> AppResult<()> {
    let _ = app;
    storage::save_settings_doc(SettingsDocKey::QuickCommands, config)
}
