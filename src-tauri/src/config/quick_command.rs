use super::{get_config_dir, load_json, save_json};
use crate::error::AppResult;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickCommandCategory {
    pub id: String,
    pub name: String,
}

/// List of quick commands persisted in quick-command.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuickCommandsConfig {
    pub commands: Vec<QuickCommand>,
    #[serde(default)]
    pub categories: Vec<QuickCommandCategory>,
}

/// Loads quick commands from ~/.dragonfly/quick-command.json.
pub fn load_quick_commands(app: &AppHandle) -> AppResult<QuickCommandsConfig> {
    let dir = get_config_dir(app)?;
    load_json(&dir.join("quick-command.json"))
}

/// Saves quick commands to disk.
pub fn save_quick_commands(app: &AppHandle, config: &QuickCommandsConfig) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("quick-command.json"), config)
}
