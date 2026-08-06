fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// In-memory quick-command cache used by both management UI and suggestion search.
pub struct QuickCommandsStore {
    config: RwLock<QuickCommandsConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickCommandsImportSource {
    WindtermQuickbar,
    XshellXts,
    NyatermJson,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuickCommandsImportResult {
    pub imported_commands: usize,
    pub imported_categories: usize,
    pub updated_commands: usize,
    pub total_commands: usize,
    pub total_categories: usize,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ImportFile {
    Config(ImportConfig),
    Commands(Vec<ImportCommand>),
}

#[derive(Debug, Default, Deserialize)]
struct ImportConfig {
    #[serde(default)]
    commands: Vec<ImportCommand>,
    #[serde(default)]
    categories: Vec<ImportCategory>,
}

#[derive(Debug, Deserialize)]
struct ImportCategory {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    sort_order: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ImportCommand {
    #[serde(default)]
    id: Option<String>,
    label: String,
    command: String,
    #[serde(skip)]
    preserve_command_text: bool,
    #[serde(default)]
    category_id: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color_tag: Option<String>,
    #[serde(default)]
    icon_tag: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default = "default_execution_mode")]
    execution_mode: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    risk_level: Option<String>,
}

#[derive(Debug, Default)]
struct ImportStats {
    added_commands: usize,
    added_categories: usize,
    updated_commands: usize,
}

fn default_execution_mode() -> String {
    "execute".to_string()
}
