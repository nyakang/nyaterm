use crate::error::{AppError, AppResult};
use crate::utils::fuzzy::{fuzzy_search_items, FuzzyResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY: usize = 5000;
const HISTORY_STORE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub command: String,
    pub last_used_at_ms: u64,
    pub use_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryStoreFileV2 {
    version: u32,
    entries: Vec<HistoryEntry>,
}

/// In-memory history store with prompt cleanup, migration, persistence, and fuzzy search.
pub struct CommandHistoryStore {
    entries: Vec<HistoryEntry>,
    dirty: bool,
    history_path: Option<PathBuf>,
}

impl CommandHistoryStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            dirty: false,
            history_path: None,
        }
    }

    pub fn set_history_path(&mut self, path: PathBuf) {
        self.history_path = Some(path);
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn load(&mut self) -> AppResult<()> {
        let Some(path) = self.history_path.clone() else {
            return Ok(());
        };

        if !path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&path)?;
        if content.trim().is_empty() {
            return Ok(());
        }

        let (entries, changed) = load_history_entries(&content)?;
        self.entries = entries;
        self.dirty = changed;

        if self.dirty {
            self.save()?;
        }

        Ok(())
    }

    pub fn save(&mut self) -> AppResult<()> {
        if let Some((path, bytes)) = self.prepare_save() {
            flush_to_disk(&path, &bytes)?;
        }
        Ok(())
    }

    /// Serializes dirty state and marks clean. Returns `(path, bytes)` for
    /// the caller to write to disk (possibly via `spawn_blocking`).
    pub fn prepare_save(&mut self) -> Option<(PathBuf, Vec<u8>)> {
        if !self.dirty {
            return None;
        }
        let path = self.history_path.clone()?;
        let payload = HistoryStoreFileV2 {
            version: HISTORY_STORE_VERSION,
            entries: self.entries.clone(),
        };
        let bytes = serde_json::to_vec(&payload).ok()?;
        self.dirty = false;
        Some((path, bytes))
    }

    pub fn add(&mut self, command: String) -> bool {
        let Some(command) = sanitize_history_command(&command) else {
            return false;
        };

        let last_used_at_ms = current_time_ms();
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.command == command)
        {
            let mut existing = self.entries.remove(index);
            existing.last_used_at_ms = last_used_at_ms;
            existing.use_count = existing.use_count.saturating_add(1);
            self.entries.push(existing);
        } else {
            self.entries.push(HistoryEntry {
                command,
                last_used_at_ms,
                use_count: 1,
            });
        }

        trim_to_max_history(&mut self.entries);
        self.dirty = true;
        true
    }

    pub fn list(&self) -> Vec<String> {
        self.entries
            .iter()
            .rev()
            .map(|entry| entry.command.clone())
            .collect()
    }

    pub fn search(&self, pattern_str: &str, limit: usize) -> Vec<FuzzyResult> {
        let items: Vec<(&str, &str)> = self
            .entries
            .iter()
            .map(|entry| (entry.command.as_str(), entry.command.as_str()))
            .collect();
        fuzzy_search_items(&items, pattern_str, "history", limit)
    }
}

pub(crate) fn sanitize_history_command(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let without_prompt = strip_known_prompt_prefix(strip_leading_env_prefixes(trimmed))
        .unwrap_or(trimmed)
        .trim();

    if without_prompt.is_empty() {
        None
    } else {
        Some(without_prompt.to_string())
    }
}

fn load_history_entries(content: &str) -> AppResult<(Vec<HistoryEntry>, bool)> {
    if let Ok(store) = serde_json::from_str::<HistoryStoreFileV2>(content) {
        if store.version != HISTORY_STORE_VERSION {
            return Err(AppError::Config(format!(
                "Unsupported command history version {}",
                store.version
            )));
        }

        let (entries, changed) = normalize_v2_entries(store.entries);
        return Ok((entries, changed));
    }

    let legacy_commands: Vec<String> = serde_json::from_str(content)?;
    Ok((migrate_legacy_commands(legacy_commands), true))
}

fn normalize_v2_entries(entries: Vec<HistoryEntry>) -> (Vec<HistoryEntry>, bool) {
    let original_len = entries.len();
    let mut normalized = Vec::new();
    let mut changed = false;

    for entry in entries {
        let use_count = entry.use_count.max(1);
        let Some(command) = sanitize_history_command(&entry.command) else {
            changed = true;
            continue;
        };

        if command != entry.command || use_count != entry.use_count {
            changed = true;
        }

        merge_entry(
            &mut normalized,
            HistoryEntry {
                command,
                last_used_at_ms: entry.last_used_at_ms,
                use_count,
            },
        );
    }

    normalized.sort_by_key(|entry| entry.last_used_at_ms);
    let trimmed = trim_to_max_history(&mut normalized);
    changed |= trimmed || normalized.len() != original_len;
    (normalized, changed)
}

fn migrate_legacy_commands(commands: Vec<String>) -> Vec<HistoryEntry> {
    let mut migrated = Vec::new();
    let base_timestamp = current_time_ms().saturating_sub(commands.len() as u64);

    for (index, command) in commands.into_iter().enumerate() {
        let Some(cleaned) = sanitize_history_command(&command) else {
            continue;
        };

        merge_entry(
            &mut migrated,
            HistoryEntry {
                command: cleaned,
                last_used_at_ms: base_timestamp.saturating_add(index as u64),
                use_count: 1,
            },
        );
    }

    trim_to_max_history(&mut migrated);
    migrated
}

fn merge_entry(entries: &mut Vec<HistoryEntry>, incoming: HistoryEntry) {
    if let Some(index) = entries
        .iter()
        .position(|entry| entry.command == incoming.command)
    {
        let mut existing = entries.remove(index);
        existing.last_used_at_ms = existing.last_used_at_ms.max(incoming.last_used_at_ms);
        existing.use_count = existing.use_count.saturating_add(incoming.use_count);
        entries.push(existing);
    } else {
        entries.push(incoming);
    }
}

fn trim_to_max_history(entries: &mut Vec<HistoryEntry>) -> bool {
    if entries.len() <= MAX_HISTORY {
        return false;
    }

    let overflow = entries.len() - MAX_HISTORY;
    entries.drain(..overflow);
    true
}

fn strip_leading_env_prefixes(mut input: &str) -> &str {
    loop {
        let Some(rest) = input.strip_prefix('(') else {
            return input;
        };
        let Some(close_idx) = rest.find(')') else {
            return input;
        };
        let after_close = &rest[close_idx + 1..];
        input = after_close.trim_start_matches([' ', '\t']);
    }
}

fn strip_known_prompt_prefix(input: &str) -> Option<&str> {
    strip_bracket_prompt(input)
        .or_else(|| strip_posix_prompt(input))
        .or_else(|| strip_powershell_prompt(input))
        .or_else(|| strip_windows_prompt(input))
}

fn strip_bracket_prompt(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('[')?;
    let close_idx = rest.find(']')?;
    let after_bracket = rest[close_idx + 1..].trim_start_matches([' ', '\t']);
    let after_marker = after_bracket
        .strip_prefix('#')
        .or_else(|| after_bracket.strip_prefix('$'))?;
    Some(after_marker.trim_start_matches([' ', '\t']))
}

fn strip_posix_prompt(input: &str) -> Option<&str> {
    let prompt_end = input.find(['#', '$'])?;
    let prompt = &input[..prompt_end];
    let after_marker = &input[prompt_end + 1..];

    let at_idx = prompt.find('@')?;
    let colon_rel = prompt[at_idx + 1..].find(':')?;
    let colon_idx = at_idx + 1 + colon_rel;

    let user = &prompt[..at_idx];
    let host = &prompt[at_idx + 1..colon_idx];
    if user.is_empty()
        || host.is_empty()
        || user.chars().any(char::is_whitespace)
        || host.chars().any(char::is_whitespace)
    {
        return None;
    }

    Some(after_marker.trim_start_matches([' ', '\t']))
}

fn strip_powershell_prompt(input: &str) -> Option<&str> {
    let rest = input
        .strip_prefix("PS ")
        .or_else(|| input.strip_prefix("PS\t"))?;
    let marker_idx = rest.find('>')?;
    let prompt = &rest[..marker_idx];
    if prompt.trim().is_empty() {
        return None;
    }

    Some(rest[marker_idx + 1..].trim_start_matches([' ', '\t']))
}

fn strip_windows_prompt(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }

    let marker_idx = input.find('>')?;
    if input[..marker_idx].contains(['\r', '\n']) {
        return None;
    }

    Some(input[marker_idx + 1..].trim_start_matches([' ', '\t']))
}

fn current_time_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

/// Writes serialized history bytes to disk. Safe to call from a blocking context.
pub(crate) fn flush_to_disk(path: &Path, bytes: &[u8]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(path, bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let tmp_path = temporary_path_for(path);
    fs::write(&tmp_path, bytes)?;
    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp_path);
            Err(err.into())
        }
    }
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let mut tmp_path = path.to_path_buf();
    let next_extension = match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{ext}.tmp"),
        _ => "tmp".to_string(),
    };
    tmp_path.set_extension(next_extension);
    tmp_path
}

#[cfg(test)]
mod tests {
    use super::{
        sanitize_history_command, CommandHistoryStore, HistoryEntry, HistoryStoreFileV2,
        HISTORY_STORE_VERSION, MAX_HISTORY,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_history_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("dragonfly-history-{name}-{nanos}.json"))
    }

    #[test]
    fn sanitizes_known_prompt_prefixes() {
        assert_eq!(
            sanitize_history_command("root@ubuntu:~# docker ps"),
            Some("docker ps".to_string())
        );
        assert_eq!(
            sanitize_history_command("[root@dev-76 ~]# docker images"),
            Some("docker images".to_string())
        );
        assert_eq!(
            sanitize_history_command("(base) user@host:~/x$ ls -la"),
            Some("ls -la".to_string())
        );
        assert_eq!(
            sanitize_history_command("PS C:\\Users\\CoderKang> dir"),
            Some("dir".to_string())
        );
        assert_eq!(
            sanitize_history_command("C:\\Users\\CoderKang>ls"),
            Some("ls".to_string())
        );
        assert_eq!(
            sanitize_history_command("echo 'root@ubuntu:~# keep me'"),
            Some("echo 'root@ubuntu:~# keep me'".to_string())
        );
    }

    #[test]
    fn drops_empty_and_prompt_only_records() {
        assert_eq!(sanitize_history_command(""), None);
        assert_eq!(sanitize_history_command("   "), None);
        assert_eq!(sanitize_history_command("root@ubuntu:~# "), None);
        assert_eq!(sanitize_history_command("(venv) [root@dev-76 ~]#"), None);
        assert_eq!(
            sanitize_history_command("PS C:\\Users\\CoderKang>   "),
            None
        );
    }

    #[test]
    fn migrates_legacy_history_and_cleans_prompt_noise() {
        let path = unique_history_path("legacy");
        fs::write(
            &path,
            serde_json::to_string(&vec![
                "root@ubuntu:~# docker ps",
                "ls",
                "root@ubuntu:~# docker ps",
                "PS C:\\Users\\CoderKang> dir",
                "root@ubuntu:~# ",
            ])
            .expect("serialize legacy history"),
        )
        .expect("write legacy history");

        let mut store = CommandHistoryStore::new();
        store.set_history_path(path.clone());
        store.load().expect("load history");

        assert_eq!(
            store.list(),
            vec!["dir".to_string(), "docker ps".to_string(), "ls".to_string()]
        );

        let saved: HistoryStoreFileV2 =
            serde_json::from_str(&fs::read_to_string(&path).expect("read migrated history"))
                .expect("parse migrated history");
        assert_eq!(saved.version, HISTORY_STORE_VERSION);
        assert_eq!(saved.entries.len(), 3);
        assert_eq!(saved.entries[1].command, "docker ps");
        assert_eq!(saved.entries[1].use_count, 2);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn normalizes_v2_duplicates_and_invalid_entries_on_load() {
        let path = unique_history_path("v2");
        let payload = HistoryStoreFileV2 {
            version: HISTORY_STORE_VERSION,
            entries: vec![
                HistoryEntry {
                    command: "root@ubuntu:~# docker ps".to_string(),
                    last_used_at_ms: 10,
                    use_count: 0,
                },
                HistoryEntry {
                    command: "docker ps".to_string(),
                    last_used_at_ms: 20,
                    use_count: 3,
                },
                HistoryEntry {
                    command: "PS C:\\Users\\CoderKang> dir".to_string(),
                    last_used_at_ms: 30,
                    use_count: 1,
                },
                HistoryEntry {
                    command: "root@ubuntu:~# ".to_string(),
                    last_used_at_ms: 40,
                    use_count: 1,
                },
            ],
        };
        fs::write(
            &path,
            serde_json::to_string(&payload).expect("serialize v2 history"),
        )
        .expect("write v2 history");

        let mut store = CommandHistoryStore::new();
        store.set_history_path(path.clone());
        store.load().expect("load v2 history");

        assert_eq!(
            store.list(),
            vec!["dir".to_string(), "docker ps".to_string()]
        );

        let saved: HistoryStoreFileV2 =
            serde_json::from_str(&fs::read_to_string(&path).expect("read normalized history"))
                .expect("parse normalized history");
        assert_eq!(saved.entries[0].command, "docker ps");
        assert_eq!(saved.entries[0].use_count, 4);
        assert_eq!(saved.entries[1].command, "dir");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn updates_existing_command_and_enforces_max_history() {
        let mut store = CommandHistoryStore::new();
        assert!(store.add("root@ubuntu:~# docker ps".to_string()));
        assert!(store.add("ls".to_string()));
        assert!(store.add("docker ps".to_string()));

        assert_eq!(
            store.list(),
            vec!["docker ps".to_string(), "ls".to_string()]
        );

        let search = store.search("dp", 5);
        assert_eq!(
            search.first().map(|item| item.command.as_str()),
            Some("docker ps")
        );

        for index in 0..=MAX_HISTORY {
            assert!(store.add(format!("echo {index}")));
        }

        let all = store.list();
        assert_eq!(all.len(), MAX_HISTORY);
        assert!(!all.iter().any(|command| command == "ls"));
    }
}
