use redb::{Database, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const DATABASE_FILE: &str = "nyaterm.redb";
const JSON_DOCS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("json_docs");
const QUICK_COMMAND_KEY: &str = "quick-command";

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Default, Serialize, Deserialize)]
struct QuickCommandsConfig {
    #[serde(default)]
    commands: Vec<QuickCommand>,
    #[serde(default)]
    categories: Vec<QuickCommandCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuickCommandCategory {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuickCommand {
    id: String,
    label: String,
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon_tag: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default = "default_execution_mode")]
    execution_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    risk_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sort_order: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_connection_ids: Option<Vec<String>>,
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
    #[serde(default)]
    sort_order: Option<i32>,
}

struct Args {
    input_path: PathBuf,
    db_path: PathBuf,
    replace: bool,
    dry_run: bool,
    backup: bool,
}

fn default_execution_mode() -> String {
    "execute".to_string()
}

fn main() -> AnyResult<()> {
    let args = parse_args()?;
    let import_file = read_import_file(&args.input_path)?;

    let mut config = if args.replace {
        QuickCommandsConfig::default()
    } else if args.db_path.exists() {
        let db = Database::open(&args.db_path)?;
        load_current_config(&db)?
    } else {
        QuickCommandsConfig::default()
    };

    let previous_commands = config.commands.len();
    let previous_categories = config.categories.len();
    let (imported_commands, imported_categories) = merge_import(&mut config, import_file)?;

    println!(
        "Prepared quick-command import: {imported_commands} command(s), {imported_categories} category/categories"
    );
    println!(
        "Resulting config: {} command(s), {} category/categories",
        config.commands.len(),
        config.categories.len()
    );

    if args.dry_run {
        println!("Dry run enabled. Database was not modified.");
        return Ok(());
    }

    if let Some(parent) = args.db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if args.backup && args.db_path.exists() {
        let backup_path = backup_database(&args.db_path)?;
        println!("Backup created: {}", backup_path.display());
    }

    let db = if args.db_path.exists() {
        Database::open(&args.db_path)?
    } else {
        Database::create(&args.db_path)?
    };
    save_config(&db, &config)?;

    println!(
        "Imported quick commands into {} (was {} command(s), {} category/categories).",
        args.db_path.display(),
        previous_commands,
        previous_categories
    );
    Ok(())
}

fn parse_args() -> AnyResult<Args> {
    let mut input_path = None;
    let mut db_path = None;
    let mut replace = false;
    let mut dry_run = false;
    let mut backup = true;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db" => {
                let value = args
                    .next()
                    .ok_or_else(|| input_error("--db requires a path"))?;
                db_path = Some(PathBuf::from(value));
            }
            "--replace" => replace = true,
            "--dry-run" => dry_run = true,
            "--no-backup" => backup = false,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(input_error(format!("unknown option: {value}")).into());
            }
            value => {
                if input_path.is_some() {
                    return Err(input_error("only one input JSON file is supported").into());
                }
                input_path = Some(PathBuf::from(value));
            }
        }
    }

    let input_path = input_path.ok_or_else(|| input_error("missing input JSON file"))?;
    let db_path = db_path.unwrap_or(default_db_path()?);

    Ok(Args {
        input_path,
        db_path,
        replace,
        dry_run,
        backup,
    })
}

fn print_usage() {
    println!(
        "Usage:\n  cargo run --manifest-path src-tauri/Cargo.toml --example import_quick_commands -- <commands.json> [--db <nyaterm.redb>] [--replace] [--dry-run] [--no-backup]\n\nClose NyaTerm before writing the database. Without --db, the script writes to ~/.nyaterm/nyaterm.redb."
    );
}

fn default_db_path() -> AnyResult<PathBuf> {
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| input_error("cannot determine home directory; pass --db explicitly"))?;
    Ok(home.join(".nyaterm").join(DATABASE_FILE))
}

fn read_import_file(path: &Path) -> AnyResult<ImportFile> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn load_current_config(db: &Database) -> AnyResult<QuickCommandsConfig> {
    let txn = db.begin_read()?;
    let table = match txn.open_table(JSON_DOCS_TABLE) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(QuickCommandsConfig::default()),
        Err(error) => return Err(error.into()),
    };

    let Some(raw) = table.get(QUICK_COMMAND_KEY)? else {
        return Ok(QuickCommandsConfig::default());
    };

    Ok(serde_json::from_str(raw.value())?)
}

fn save_config(db: &Database, config: &QuickCommandsConfig) -> AnyResult<()> {
    let content = serde_json::to_string_pretty(config)?;
    let txn = db.begin_write()?;
    {
        let mut table = txn.open_table(JSON_DOCS_TABLE)?;
        table.insert(QUICK_COMMAND_KEY, content.as_str())?;
    }
    txn.commit()?;
    Ok(())
}

fn merge_import(
    config: &mut QuickCommandsConfig,
    import_file: ImportFile,
) -> AnyResult<(usize, usize)> {
    let import_config = match import_file {
        ImportFile::Config(config) => config,
        ImportFile::Commands(commands) => ImportConfig {
            commands,
            categories: Vec::new(),
        },
    };

    let mut category_names = BTreeMap::new();
    for category in &config.categories {
        category_names.insert(category.name.clone(), category.id.clone());
    }

    let mut imported_categories = 0usize;
    for category in import_config.categories {
        let name = require_text(&category.name, "category.name")?;
        let id_input = category.id.unwrap_or_else(|| slugify(&name));
        let id = normalize_id(&id_input, "category.id")?;
        upsert_category(
            config,
            QuickCommandCategory {
                id: id.clone(),
                name: name.clone(),
            },
        );
        category_names.insert(name, id);
        imported_categories += 1;
    }

    let existing_category_ids = config
        .categories
        .iter()
        .map(|category| category.id.clone())
        .collect::<BTreeSet<_>>();

    let mut seen_ids = BTreeSet::new();
    let mut imported_commands = 0usize;
    for command in import_config.commands {
        let label = require_text(&command.label, "command.label")?;
        let command_text = require_text(&command.command, "command.command")?;
        let id_input = command
            .id
            .unwrap_or_else(|| format!("cmd-{}", Uuid::new_v4()));
        let id = normalize_id(&id_input, "command.id")?;

        if !seen_ids.insert(id.clone()) {
            return Err(input_error(format!("duplicate command id in import file: {id}")).into());
        }

        let category_id = match (command.category_id, command.category) {
            (Some(category_id), _) => Some(normalize_id(&category_id, "command.category_id")?),
            (None, Some(category_name)) => {
                let category_name = require_text(&category_name, "command.category")?;
                let category_id = category_names
                    .get(&category_name)
                    .cloned()
                    .unwrap_or_else(|| slugify(&category_name));
                if !config
                    .categories
                    .iter()
                    .any(|category| category.id == category_id)
                {
                    upsert_category(
                        config,
                        QuickCommandCategory {
                            id: category_id.clone(),
                            name: category_name.clone(),
                        },
                    );
                    category_names.insert(category_name, category_id.clone());
                    imported_categories += 1;
                }
                Some(category_id)
            }
            (None, None) => None,
        };

        if let Some(category_id) = &category_id {
            if !existing_category_ids.contains(category_id)
                && !config
                    .categories
                    .iter()
                    .any(|category| category.id == *category_id)
            {
                upsert_category(
                    config,
                    QuickCommandCategory {
                        id: category_id.clone(),
                        name: category_id.clone(),
                    },
                );
                imported_categories += 1;
            }
        }

        let description = trim_optional(command.description);
        let color_tag = trim_optional(command.color_tag);
        let icon_tag = trim_optional(command.icon_tag);
        let execution_mode = command.execution_mode.trim().to_string();
        let source = trim_optional(command.source);
        let risk_level = trim_optional(command.risk_level);

        validate_execution_mode(&execution_mode)?;
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

        upsert_command(
            config,
            QuickCommand {
                id,
                label,
                command: command_text,
                category_id,
                description,
                color_tag,
                icon_tag,
                pinned: command.pinned,
                execution_mode,
                source,
                risk_level,
                sort_order: command.sort_order,
                target_connection_ids: None,
            },
        );
        imported_commands += 1;
    }

    Ok((imported_commands, imported_categories))
}

fn upsert_category(config: &mut QuickCommandsConfig, category: QuickCommandCategory) {
    if let Some(existing) = config
        .categories
        .iter_mut()
        .find(|item| item.id == category.id)
    {
        *existing = category;
    } else {
        config.categories.push(category);
    }
}

fn upsert_command(config: &mut QuickCommandsConfig, command: QuickCommand) {
    if let Some(existing) = config
        .commands
        .iter_mut()
        .find(|item| item.id == command.id)
    {
        *existing = command;
    } else {
        config.commands.push(command);
    }
}

fn backup_database(path: &Path) -> AnyResult<PathBuf> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(DATABASE_FILE);
    let backup_path = path.with_file_name(format!("{file_name}.bak-{timestamp}"));
    fs::copy(path, &backup_path)?;
    Ok(backup_path)
}

fn require_text(value: &str, field: &str) -> AnyResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(input_error(format!("{field} cannot be empty")).into());
    }
    Ok(trimmed.to_string())
}

fn normalize_id(value: &str, field: &str) -> AnyResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(input_error(format!("{field} cannot be empty")).into());
    }
    Ok(trimmed.to_string())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_execution_mode(value: &str) -> AnyResult<()> {
    validate_one_of(value, &["execute", "append"], "command.execution_mode")
}

fn validate_one_of(value: &str, allowed: &[&str], field: &str) -> AnyResult<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(input_error(format!("{field} must be one of: {}", allowed.join(", "))).into())
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

fn input_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}
