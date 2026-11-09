//! Command history persistence.
//!
//! Split out of `storage.rs` by domain. Table name, key layout and record
//! shape are unchanged; this only moves the code.

use std::collections::HashMap;

use redb::{ReadTransaction, ReadableDatabase, ReadableTable};
use sha2::{Digest, Sha256};

use super::{
    COMMAND_HISTORY_PREFIX, COMMAND_HISTORY_TABLE, ConnectionStore, StorageError,
    clear_prefix_in_txn, current_time_ms, lower_hex, write_json_in_txn,
};
use nyaterm_core::CommandHistoryEntry;

impl ConnectionStore {
    pub fn append_command_history(&self, command: &str) -> Result<(), StorageError> {
        let Some(command) = sanitize_history_command(command) else {
            return Ok(());
        };
        // Read the canonical raw row only. list_command_history also includes
        // legacy control-prefixed rows, whose counts must not be saved twice.
        let mut entry = self
            .list_keyed_json_by_prefix::<CommandHistoryEntry>(
                COMMAND_HISTORY_TABLE,
                COMMAND_HISTORY_PREFIX,
            )?
            .into_iter()
            .map(|(_, entry)| entry)
            .find(|entry| entry.command == command)
            .unwrap_or(CommandHistoryEntry {
                command,
                last_used_at_ms: 0,
                use_count: 0,
            });
        entry.last_used_at_ms = current_time_ms();
        entry.use_count = if entry.use_count == 0 {
            1
        } else {
            entry.use_count.saturating_add(1)
        };
        self.save_command_history_entry(&entry)
    }

    pub fn list_command_history(
        &self,
        limit: usize,
    ) -> Result<Vec<CommandHistoryEntry>, StorageError> {
        list_command_history_in_txn(&self.db.begin_read()?, limit)
    }

    pub fn delete_command_history(&self, command: &str) -> Result<(), StorageError> {
        let Some(command) = sanitize_history_command(command) else {
            return Ok(());
        };
        let txn = self.db.begin_write()?;
        let mut table = txn.open_table(COMMAND_HISTORY_TABLE)?;
        let mut keys = Vec::new();
        for row in table.iter()? {
            let (key, value) = row?;
            if !key.value().starts_with(COMMAND_HISTORY_PREFIX) {
                continue;
            }
            let entry: CommandHistoryEntry = serde_json::from_slice(value.value())?;
            if sanitize_history_command(&entry.command).as_deref() == Some(command.as_str()) {
                keys.push(key.value().to_string());
            }
        }
        for key in keys {
            table.remove(key.as_str())?;
        }
        drop(table);
        super::completion_history::delete_completion_command_in_txn(&txn, &command)?;
        txn.commit()?;
        self.invalidate_completion();
        Ok(())
    }

    pub fn replace_command_history(
        &self,
        entries: &[CommandHistoryEntry],
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        replace_command_history_in_txn(&txn, entries)?;
        txn.commit()?;
        self.invalidate_completion();
        Ok(())
    }

    fn save_command_history_entry(&self, entry: &CommandHistoryEntry) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        save_command_history_entry_in_txn(&txn, entry)?;
        super::completion_history::bump_revision(&txn)?;
        txn.commit()?;
        self.invalidate_completion();
        Ok(())
    }
}

pub(super) fn list_command_history_in_txn(
    txn: &ReadTransaction,
    limit: usize,
) -> Result<Vec<CommandHistoryEntry>, StorageError> {
    let table = match txn.open_table(COMMAND_HISTORY_TABLE) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut entries = Vec::new();
    for row in table.range(COMMAND_HISTORY_PREFIX..)? {
        let (key, value) = row?;
        if !key.value().starts_with(COMMAND_HISTORY_PREFIX) {
            break;
        }
        entries.push((
            key.value().to_string(),
            serde_json::from_slice::<CommandHistoryEntry>(value.value())?,
        ));
    }
    entries.sort_by(|left, right| right.0.cmp(&left.0));
    let mut history = Vec::new();
    let mut positions = HashMap::new();
    for (_, entry) in entries {
        if let Some(command) = sanitize_history_command(&entry.command) {
            if let Some(&index) = positions.get(&command) {
                let existing: &mut CommandHistoryEntry = &mut history[index];
                existing.last_used_at_ms = existing.last_used_at_ms.max(entry.last_used_at_ms);
                existing.use_count = existing.use_count.saturating_add(entry.use_count.max(1));
            } else {
                positions.insert(command.clone(), history.len());
                history.push(CommandHistoryEntry {
                    command,
                    last_used_at_ms: entry.last_used_at_ms,
                    use_count: entry.use_count.max(1),
                });
            }
        }
    }
    history.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at_ms));
    history.truncate(limit);
    Ok(history)
}

pub(super) fn replace_command_history_in_txn(
    txn: &redb::WriteTransaction,
    entries: &[CommandHistoryEntry],
) -> Result<(), StorageError> {
    super::completion_history::reset_completion_history_in_txn(txn, entries)?;
    clear_prefix_in_txn(txn, COMMAND_HISTORY_TABLE, COMMAND_HISTORY_PREFIX)?;
    let mut normalized = Vec::new();
    for entry in entries {
        let Some(command) = sanitize_history_command(&entry.command) else {
            continue;
        };
        merge_command_history_entry(
            &mut normalized,
            CommandHistoryEntry {
                command,
                last_used_at_ms: entry.last_used_at_ms,
                use_count: entry.use_count.max(1),
            },
        );
    }
    normalized.sort_by_key(|entry| entry.last_used_at_ms);
    trim_command_history(&mut normalized);
    for entry in normalized {
        save_command_history_entry_in_txn(txn, &entry)?;
    }
    Ok(())
}

fn save_command_history_entry_in_txn(
    txn: &redb::WriteTransaction,
    entry: &CommandHistoryEntry,
) -> Result<(), StorageError> {
    let Some(command) = sanitize_history_command(&entry.command) else {
        return Ok(());
    };
    let normalized = CommandHistoryEntry {
        command,
        last_used_at_ms: entry.last_used_at_ms,
        use_count: entry.use_count.max(1),
    };
    let id = command_history_id(&normalized.command);
    remove_command_history_id_in_txn(txn, &id)?;
    write_json_in_txn(
        txn,
        COMMAND_HISTORY_TABLE,
        &command_history_key(&normalized, &id),
        &normalized,
    )
}

fn remove_command_history_id_in_txn(
    txn: &redb::WriteTransaction,
    id: &str,
) -> Result<(), StorageError> {
    let table = txn.open_table(COMMAND_HISTORY_TABLE)?;
    let suffix = format!("|{id}");
    let mut keys = Vec::new();
    for entry in table.iter()? {
        let (key, _) = entry?;
        if key.value().ends_with(&suffix) {
            keys.push(key.value().to_string());
        }
    }
    drop(table);
    let mut table = txn.open_table(COMMAND_HISTORY_TABLE)?;
    for key in keys {
        table.remove(key.as_str())?;
    }
    Ok(())
}

fn merge_command_history_entry(
    entries: &mut Vec<CommandHistoryEntry>,
    incoming: CommandHistoryEntry,
) {
    if let Some(index) = entries
        .iter()
        .position(|entry| entry.command == incoming.command)
    {
        let mut existing = entries.remove(index);
        existing.last_used_at_ms = existing.last_used_at_ms.max(incoming.last_used_at_ms);
        existing.use_count = existing.use_count.saturating_add(incoming.use_count.max(1));
        entries.push(existing);
    } else {
        entries.push(incoming);
    }
}

fn trim_command_history(entries: &mut Vec<CommandHistoryEntry>) {
    const MAX_HISTORY: usize = 5000;
    if entries.len() > MAX_HISTORY {
        let overflow = entries.len() - MAX_HISTORY;
        entries.drain(..overflow);
    }
}

fn command_history_key(entry: &CommandHistoryEntry, id: &str) -> String {
    format!(
        "{}{:020}|{}",
        COMMAND_HISTORY_PREFIX, entry.last_used_at_ms, id
    )
}

fn command_history_id(command: &str) -> String {
    let digest = Sha256::digest(command.as_bytes());
    lower_hex(&digest[..16])
}

pub(super) fn sanitize_history_command(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Older suggestion execution recorded the wire rewrite sequence as text.
    let mut without_rewrite = trimmed;
    while let Some(rest) = without_rewrite.strip_prefix("\u{05}\u{15}") {
        without_rewrite = rest;
    }
    let without_rewrite = without_rewrite.trim();
    let without_prompt = strip_known_prompt_prefix(strip_leading_env_prefixes(without_rewrite))
        .unwrap_or(without_rewrite)
        .trim();
    if without_prompt.is_empty() {
        None
    } else {
        Some(without_prompt.to_string())
    }
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
    if rest[..marker_idx].trim().is_empty() {
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

/// Update the canonical global document without discarding extension fields.
pub(super) fn append_global_history_in_txn(
    txn: &redb::WriteTransaction,
    command: &str,
    now: u64,
) -> Result<(), StorageError> {
    let table = txn.open_table(COMMAND_HISTORY_TABLE)?;
    let mut document =
        serde_json::json!({"command": command, "use_count": 0, "last_used_at_ms": 0});
    for row in table.iter()? {
        let (_, value) = row?;
        let raw: serde_json::Value = serde_json::from_slice(value.value())?;
        if raw.get("command").and_then(serde_json::Value::as_str) == Some(command) {
            document = raw;
            break;
        }
    }
    drop(table);
    let count = document
        .get("use_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
        .saturating_add(1)
        .min(u32::MAX as u64) as u32;
    document["use_count"] = serde_json::json!(count);
    document["last_used_at_ms"] = serde_json::json!(now);
    let id = command_history_id(command);
    remove_command_history_id_in_txn(txn, &id)?;
    let entry = CommandHistoryEntry {
        command: command.into(),
        use_count: count,
        last_used_at_ms: now,
    };
    write_json_in_txn(
        txn,
        COMMAND_HISTORY_TABLE,
        &command_history_key(&entry, &id),
        &document,
    )
}
