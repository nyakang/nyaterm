use redb::{Database, ReadableTable, TableDefinition};
use serde_json::Value;
use std::path::PathBuf;

const SETTINGS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
const SETTINGS_KEY: &str = "settings/default";

fn main() {
    if let Err(error) = run() {
        eprintln!("NyaTerm unlock rescue failed: {error}");
        wait_for_enter();
        std::process::exit(1);
    }

    wait_for_enter();
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_db_path);

    if !db_path.exists() {
        return Err(format!("database not found: {}", db_path.display()).into());
    }

    let backup_path = db_path.with_extension(format!(
        "redb.screen-lock-rescue-{}.bak",
        unix_timestamp_seconds()
    ));
    std::fs::copy(&db_path, &backup_path)?;

    let db = Database::open(&db_path)?;
    let txn = db.begin_write()?;
    let changed = {
        let mut table = txn.open_table(SETTINGS_TABLE)?;
        let (mut settings, was_enabled, old_idle): (Value, bool, u64) = {
            let Some(raw) = table.get(SETTINGS_KEY)? else {
                return Err(format!("settings document not found: {SETTINGS_KEY}").into());
            };

            let mut settings: Value = serde_json::from_slice(raw.value())?;
            let security = settings
                .as_object_mut()
                .and_then(|root| root.get_mut("security"))
                .and_then(Value::as_object_mut)
                .ok_or("settings.security is missing or not an object")?;

            let was_startup_enabled = security
                .get("enable_startup_lock")
                .or_else(|| security.get("enable_screen_lock"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let was_idle_enabled = security
                .get("enable_idle_lock")
                .or_else(|| security.get("enable_screen_lock"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let old_idle = security
                .get("idle_lock_minutes")
                .and_then(Value::as_u64)
                .unwrap_or(0);

            (settings, was_startup_enabled || was_idle_enabled, old_idle)
        };

        let security = settings
            .as_object_mut()
            .and_then(|root| root.get_mut("security"))
            .and_then(Value::as_object_mut)
            .ok_or("settings.security is missing or not an object")?;

        security.insert("enable_startup_lock".to_string(), Value::Bool(false));
        security.insert("enable_idle_lock".to_string(), Value::Bool(false));
        security.remove("enable_screen_lock");
        security.insert(
            "idle_lock_minutes".to_string(),
            Value::Number(serde_json::Number::from(0)),
        );

        let content = serde_json::to_vec(&settings)?;
        table.insert(SETTINGS_KEY, content.as_slice())?;
        was_enabled || old_idle != 0
    };
    txn.commit()?;

    println!("NyaTerm unlock rescue completed.");
    println!("Database: {}", db_path.display());
    println!("Backup:   {}", backup_path.display());
    println!("Screen lock disabled: {changed}");
    println!("Master password was left unchanged.");
    println!("You can start NyaTerm again now.");

    Ok(())
}

fn default_db_path() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nyaterm")
        .join("nyaterm.redb")
}

fn unix_timestamp_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn wait_for_enter() {
    println!();
    println!("Press Enter to close this window.");

    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
}
