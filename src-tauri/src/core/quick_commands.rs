use crate::config::{self, QuickCommand, QuickCommandCategory, QuickCommandsConfig};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use uuid::Uuid;

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
}

#[derive(Debug, Deserialize)]
struct ImportCommand {
    #[serde(default)]
    id: Option<String>,
    label: String,
    command: String,
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

impl QuickCommandsStore {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(QuickCommandsConfig::default()),
        }
    }

    pub fn load_from_disk(&self, app: &AppHandle) -> AppResult<()> {
        let config = config::load_quick_commands(app)?;
        self.replace(config);
        Ok(())
    }

    pub fn snapshot(&self) -> QuickCommandsConfig {
        self.config.read().unwrap().clone()
    }

    pub fn save_all(&self, app: &AppHandle, config: QuickCommandsConfig) -> AppResult<()> {
        config::save_quick_commands(app, &config)?;
        self.replace(config);
        Ok(())
    }

    pub fn upsert(
        &self,
        app: &AppHandle,
        mut command: QuickCommand,
        new_category: Option<QuickCommandCategory>,
    ) -> AppResult<QuickCommandsConfig> {
        let mut config = self.snapshot();
        let now = now_millis();

        if let Some(category) = new_category {
            if !config.categories.iter().any(|item| item.id == category.id) {
                config.categories.push(category);
            }
        }

        command.updated_at = Some(now);

        if let Some(existing) = config
            .commands
            .iter_mut()
            .find(|item| item.id == command.id)
        {
            let original_created_at = existing.created_at;
            let original_use_count = existing.use_count;
            *existing = command;
            existing.created_at = existing.created_at.or(original_created_at);
            existing.use_count = existing.use_count.or(original_use_count);
        } else {
            command.created_at = command.created_at.or(Some(now));
            config.commands.push(command);
        }

        self.save_all(app, config.clone())?;
        Ok(config)
    }

    pub fn increment_use_count(&self, app: &AppHandle, id: &str) -> AppResult<()> {
        let mut config = self.snapshot();
        if let Some(cmd) = config.commands.iter_mut().find(|c| c.id == id) {
            cmd.use_count = Some(cmd.use_count.unwrap_or(0) + 1);
            cmd.updated_at = Some(now_millis());
            self.save_all(app, config)?;
        }
        Ok(())
    }

    pub fn import_from_file(
        &self,
        app: &AppHandle,
        file_path: &str,
        source: QuickCommandsImportSource,
    ) -> AppResult<QuickCommandsImportResult> {
        let import_config = match source {
            QuickCommandsImportSource::NyatermJson => {
                let raw = std::fs::read_to_string(file_path)?;
                parse_nyaterm_import(&raw)?
            }
            QuickCommandsImportSource::WindtermQuickbar => {
                let raw = std::fs::read_to_string(file_path)?;
                parse_windterm_quickbar(&raw)?
            }
            QuickCommandsImportSource::XshellXts => parse_xshell_xts_quick_buttons(file_path)?,
        };

        if import_config.commands.is_empty() {
            return Err(AppError::Config(
                "No valid quick commands found in import file".to_string(),
            ));
        }

        let mut config = self.snapshot();
        let stats = merge_import(&mut config, import_config)?;
        let result = QuickCommandsImportResult {
            imported_commands: stats.added_commands,
            imported_categories: stats.added_categories,
            updated_commands: stats.updated_commands,
            total_commands: config.commands.len(),
            total_categories: config.categories.len(),
        };

        self.save_all(app, config)?;
        Ok(result)
    }

    fn replace(&self, config: QuickCommandsConfig) {
        *self.config.write().unwrap() = config;
    }
}

fn parse_nyaterm_import(raw: &str) -> AppResult<ImportConfig> {
    let import_file: ImportFile = serde_json::from_str(raw)?;
    Ok(match import_file {
        ImportFile::Config(config) => config,
        ImportFile::Commands(commands) => ImportConfig {
            commands,
            categories: Vec::new(),
        },
    })
}

fn parse_windterm_quickbar(raw: &str) -> AppResult<ImportConfig> {
    let entries: Vec<Value> = serde_json::from_str(raw)
        .map_err(|e| AppError::Config(format!("Invalid WindTerm quickbar JSON: {e}")))?;
    let mut commands = Vec::new();

    for entry in entries {
        let label = entry
            .get("quick.label")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let command = entry
            .get("quick.text")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if label.is_empty() || command.is_empty() {
            continue;
        }

        let id = entry
            .get("quick.uuid")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let category = entry
            .get("quick.group")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let icon_tag = entry
            .get("quick.icon")
            .and_then(Value::as_str)
            .and_then(map_windterm_icon);
        let execution_mode = match entry
            .get("quick.type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
        {
            value if value.eq_ignore_ascii_case("Send Text") => "append".to_string(),
            _ => "execute".to_string(),
        };

        commands.push(ImportCommand {
            id,
            label: label.to_string(),
            command: command.to_string(),
            category_id: None,
            category,
            description: None,
            color_tag: None,
            icon_tag,
            pinned: false,
            execution_mode,
            source: Some("manual".to_string()),
            risk_level: None,
        });
    }

    Ok(ImportConfig {
        commands,
        categories: Vec::new(),
    })
}

fn parse_xshell_xts_quick_buttons(path: &str) -> AppResult<ImportConfig> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Config(format!("Cannot open Xshell XTS file: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Config(format!("Invalid ZIP/XTS file: {e}")))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::Config(format!("ZIP entry error: {e}")))?;
        let entry_path = decode_text(entry.name_raw()).replace('\\', "/");
        let normalized_path = entry_path.trim_start_matches("./").trim_start_matches('/');
        let lookup_path = normalized_path.to_ascii_lowercase();
        if lookup_path != "xsl/quickbutton files/commands.qbl"
            && !lookup_path.ends_with("/xsl/quickbutton files/commands.qbl")
        {
            continue;
        }

        let mut raw = Vec::new();
        entry
            .read_to_end(&mut raw)
            .map_err(|e| AppError::Config(format!("Failed to read {entry_path}: {e}")))?;
        return Ok(parse_xshell_quick_buttons_content(&decode_text(&raw)));
    }

    Err(AppError::Config(
        "Xshell quick button file not found: xsl/QuickButton Files/commands.qbl".to_string(),
    ))
}

fn parse_xshell_quick_buttons_content(raw: &str) -> ImportConfig {
    let sections = parse_ini_sections(raw);
    let Some(quick_button) = sections.get("QuickButton") else {
        return ImportConfig::default();
    };

    let mut buttons: BTreeMap<usize, HashMap<String, String>> = BTreeMap::new();
    for (key, value) in quick_button {
        let Some(rest) = key.strip_prefix("Button_") else {
            continue;
        };
        let Some((index, field)) = rest.split_once('_') else {
            continue;
        };
        let Ok(index) = index.parse::<usize>() else {
            continue;
        };

        buttons
            .entry(index)
            .or_default()
            .insert(field.to_string(), value.clone());
    }

    let commands = buttons
        .into_iter()
        .filter_map(|(_, fields)| {
            let button_type = fields.get("Type").map(String::as_str).unwrap_or("");
            if button_type.trim() != "1" {
                return None;
            }

            let label = fields.get("Name").map(String::as_str).unwrap_or("").trim();
            let command = fields
                .get("Action")
                .map(String::as_str)
                .unwrap_or("")
                .trim();
            if label.is_empty() || command.is_empty() {
                return None;
            }

            Some(ImportCommand {
                id: None,
                label: label.to_string(),
                command: command.to_string(),
                category_id: None,
                category: None,
                description: trim_optional(fields.get("Desc").cloned()),
                color_tag: None,
                icon_tag: None,
                pinned: false,
                execution_mode: "append".to_string(),
                source: Some("manual".to_string()),
                risk_level: None,
            })
        })
        .collect();

    ImportConfig {
        commands,
        categories: Vec::new(),
    }
}

fn parse_ini_sections(raw: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_string();
            sections.entry(current_section.clone()).or_default();
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current_section.clone())
                .or_default()
                .insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    sections
}

fn decode_text(raw: &[u8]) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(raw) {
        let (decoded, _, _) = encoding.decode(&raw[bom_len..]);
        return decoded.into_owned();
    }

    match std::str::from_utf8(raw) {
        Ok(value) => value.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GBK.decode(raw);
            decoded.into_owned()
        }
    }
}

fn merge_import(
    config: &mut QuickCommandsConfig,
    import_config: ImportConfig,
) -> AppResult<ImportStats> {
    let mut stats = ImportStats::default();
    let mut category_names = BTreeMap::new();
    for category in &config.categories {
        category_names.insert(category.name.clone(), category.id.clone());
    }

    for category in import_config.categories {
        let name = require_text(&category.name, "category.name")?;
        let id_input = category.id.unwrap_or_else(|| slugify(&name));
        let id = normalize_id(&id_input, "category.id")?;
        if upsert_category(
            config,
            QuickCommandCategory {
                id: id.clone(),
                name: name.clone(),
            },
        ) {
            stats.added_categories += 1;
        }
        category_names.insert(name, id);
    }

    let mut seen_ids = BTreeSet::new();
    let now = now_millis();

    for command in import_config.commands {
        let label = require_text(&command.label, "command.label")?;
        let command_text = require_text(&command.command, "command.command")?;
        let id_input = command
            .id
            .unwrap_or_else(|| format!("cmd-{}", Uuid::new_v4()));
        let id = normalize_id(&id_input, "command.id")?;

        if !seen_ids.insert(id.clone()) {
            return Err(AppError::Config(format!(
                "Duplicate command id in import file: {id}"
            )));
        }

        let category_id = match (command.category_id, command.category) {
            (Some(category_id), _) => {
                let category_id = normalize_id(&category_id, "command.category_id")?;
                ensure_category(
                    config,
                    &mut category_names,
                    &category_id,
                    &category_id,
                    &mut stats,
                );
                Some(category_id)
            }
            (None, Some(category_name)) => {
                let category_name = require_text(&category_name, "command.category")?;
                let category_id = category_names
                    .get(&category_name)
                    .cloned()
                    .unwrap_or_else(|| slugify(&category_name));
                ensure_category(
                    config,
                    &mut category_names,
                    &category_id,
                    &category_name,
                    &mut stats,
                );
                Some(category_id)
            }
            (None, None) => None,
        };

        let execution_mode = command.execution_mode.trim().to_string();
        let source = trim_optional(command.source);
        let risk_level = trim_optional(command.risk_level);

        validate_one_of(
            &execution_mode,
            &["execute", "append"],
            "command.execution_mode",
        )?;
        if let Some(source) = source.as_deref() {
            validate_one_of(source, &["manual", "ai"], "command.source")?;
        }
        if let Some(risk_level) = risk_level.as_deref() {
            validate_one_of(
                risk_level,
                &["low", "medium", "high", "critical"],
                "command.risk_level",
            )?;
        }

        let imported = QuickCommand {
            id,
            label,
            command: command_text,
            category_id,
            description: trim_optional(command.description),
            color_tag: trim_optional(command.color_tag),
            icon_tag: trim_optional(command.icon_tag),
            pinned: command.pinned,
            execution_mode,
            source,
            risk_level,
            updated_at: Some(now),
            created_at: Some(now),
            use_count: None,
        };

        if upsert_command(config, imported) {
            stats.added_commands += 1;
        } else {
            stats.updated_commands += 1;
        }
    }

    Ok(stats)
}

fn ensure_category(
    config: &mut QuickCommandsConfig,
    category_names: &mut BTreeMap<String, String>,
    id: &str,
    name: &str,
    stats: &mut ImportStats,
) {
    if config.categories.iter().any(|category| category.id == id) {
        category_names.insert(name.to_string(), id.to_string());
        return;
    }

    config.categories.push(QuickCommandCategory {
        id: id.to_string(),
        name: name.to_string(),
    });
    category_names.insert(name.to_string(), id.to_string());
    stats.added_categories += 1;
}

fn upsert_category(config: &mut QuickCommandsConfig, category: QuickCommandCategory) -> bool {
    if let Some(existing) = config
        .categories
        .iter_mut()
        .find(|item| item.id == category.id)
    {
        *existing = category;
        false
    } else {
        config.categories.push(category);
        true
    }
}

fn upsert_command(config: &mut QuickCommandsConfig, command: QuickCommand) -> bool {
    if let Some(existing) = config
        .commands
        .iter_mut()
        .find(|item| item.id == command.id)
    {
        let created_at = existing.created_at;
        let use_count = existing.use_count;
        *existing = command;
        existing.created_at = created_at.or(existing.created_at);
        existing.use_count = use_count.or(existing.use_count);
        false
    } else {
        config.commands.push(command);
        true
    }
}

fn require_text(value: &str, field: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn normalize_id(value: &str, field: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_one_of(value: &str, allowed: &[&str], field: &str) -> AppResult<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "{field} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

fn slugify(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            output.push(ch);
        } else if ch.is_whitespace() && !output.ends_with('-') {
            output.push('-');
        }
    }

    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        format!("category-{}", Uuid::new_v4())
    } else {
        output
    }
}

fn map_windterm_icon(value: &str) -> Option<String> {
    let normalized = value.to_ascii_lowercase();
    let mappings = [
        ("kubernetes", "k8s"),
        ("k8s", "k8s"),
        ("docker", "docker"),
        ("linux", "linux"),
        ("ubuntu", "ubuntu"),
        ("debian", "debian"),
        ("centos", "centos"),
        ("fedora", "fedora"),
        ("apple", "apple"),
        ("github", "github"),
        ("gitlab", "gitlab"),
        ("nginx", "nginx"),
        ("redis", "redis"),
        ("postgres", "postgres"),
        ("mysql", "mysql"),
        ("mongo", "mongodb"),
        ("python", "python"),
        ("javascript", "js"),
        ("typescript", "ts"),
        ("rust", "rust"),
        ("node", "node"),
        ("php", "php"),
        ("aws", "aws"),
        ("gcp", "gcp"),
    ];

    mappings
        .iter()
        .find_map(|(needle, icon)| normalized.contains(needle).then(|| (*icon).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> QuickCommandsConfig {
        QuickCommandsConfig {
            commands: Vec::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn imports_nyaterm_config_json() {
        let raw = r#"{
            "categories": [{"id": "general", "name": "General"}],
            "commands": [{
                "id": "cmd-list",
                "label": "List",
                "command": "ls -la",
                "category_id": "general",
                "execution_mode": "execute",
                "source": "manual",
                "risk_level": "low"
            }]
        }"#;
        let import_config = parse_nyaterm_import(raw).unwrap();
        let mut config = empty_config();

        let stats = merge_import(&mut config, import_config).unwrap();

        assert_eq!(stats.added_commands, 1);
        assert_eq!(stats.added_categories, 1);
        assert_eq!(config.commands[0].label, "List");
        assert_eq!(config.commands[0].category_id.as_deref(), Some("general"));
    }

    #[test]
    fn imports_nyaterm_command_array_json() {
        let raw = r#"[{"label":"Pods","command":"kubectl get pods -A","category":"Kubernetes","execution_mode":"append"}]"#;
        let import_config = parse_nyaterm_import(raw).unwrap();
        let mut config = empty_config();

        let stats = merge_import(&mut config, import_config).unwrap();

        assert_eq!(stats.added_commands, 1);
        assert_eq!(stats.added_categories, 1);
        assert_eq!(config.commands[0].execution_mode, "append");
        assert_eq!(config.categories[0].name, "Kubernetes");
    }

    #[test]
    fn imports_windterm_quickbar_json() {
        let raw = r#"[{
            "quick.group": "快速",
            "quick.icon": "session::arrow-coral",
            "quick.label": "miniconda3 安装",
            "quick.text": "echo install",
            "quick.type": "Send Text",
            "quick.uuid": "70127d80-24b8-46eb-958d-f944c5e423dd"
        }]"#;
        let import_config = parse_windterm_quickbar(raw).unwrap();
        let mut config = empty_config();

        let stats = merge_import(&mut config, import_config).unwrap();

        assert_eq!(stats.added_commands, 1);
        assert_eq!(stats.added_categories, 1);
        assert_eq!(
            config.commands[0].id,
            "70127d80-24b8-46eb-958d-f944c5e423dd"
        );
        assert_eq!(config.commands[0].label, "miniconda3 安装");
        assert_eq!(config.commands[0].command, "echo install");
        assert_eq!(config.commands[0].execution_mode, "append");
        assert_eq!(config.categories[0].name, "快速");
    }

    #[test]
    fn imports_xshell_quick_buttons_type_one_only() {
        let raw = r#"[Info]
Version=8.2
Count=3
Expanded=1
[QuickButton]
Button_0_Name=测试
Button_1_Name=TEST
Button_2_Name=Ignored
Button_0_Type=1
Button_1_Type=1
Button_2_Type=2
Button_0_Action=ls -la
Button_1_Action=pwd
Button_2_Action=whoami
"#;
        let import_config = parse_xshell_quick_buttons_content(raw);
        let mut config = empty_config();

        let stats = merge_import(&mut config, import_config).unwrap();

        assert_eq!(stats.added_commands, 2);
        assert_eq!(config.commands[0].label, "测试");
        assert_eq!(config.commands[0].command, "ls -la");
        assert_eq!(config.commands[0].execution_mode, "append");
        assert_eq!(config.commands[1].label, "TEST");
        assert_eq!(config.commands[1].command, "pwd");
    }

    #[test]
    fn updates_same_id_and_preserves_created_at_and_use_count() {
        let mut config = QuickCommandsConfig {
            commands: vec![QuickCommand {
                id: "same".to_string(),
                label: "Old".to_string(),
                command: "old".to_string(),
                category_id: None,
                description: None,
                color_tag: None,
                icon_tag: None,
                pinned: false,
                execution_mode: "execute".to_string(),
                source: Some("manual".to_string()),
                risk_level: None,
                updated_at: Some(10),
                created_at: Some(5),
                use_count: Some(7),
            }],
            categories: Vec::new(),
        };
        let import_config = parse_nyaterm_import(
            r#"[{"id":"same","label":"New","command":"new","execution_mode":"append"}]"#,
        )
        .unwrap();

        let stats = merge_import(&mut config, import_config).unwrap();

        assert_eq!(stats.added_commands, 0);
        assert_eq!(stats.updated_commands, 1);
        assert_eq!(config.commands[0].label, "New");
        assert_eq!(config.commands[0].created_at, Some(5));
        assert_eq!(config.commands[0].use_count, Some(7));
        assert_eq!(config.commands[0].execution_mode, "append");
    }

    #[test]
    fn rejects_invalid_execution_mode() {
        let import_config =
            parse_nyaterm_import(r#"[{"label":"Bad","command":"bad","execution_mode":"run"}]"#)
                .unwrap();
        let mut config = empty_config();

        let error = merge_import(&mut config, import_config).unwrap_err();

        assert!(error.to_string().contains("command.execution_mode"));
    }

    #[test]
    fn windterm_without_valid_commands_is_empty() {
        let import_config = parse_windterm_quickbar(
            r#"[{"quick.label":"","quick.text":"echo no"},{"quick.label":"No text"}]"#,
        )
        .unwrap();

        assert!(import_config.commands.is_empty());
    }
}
