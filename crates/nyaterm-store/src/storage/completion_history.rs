//! Device-local command provenance. Existing history documents remain unchanged.
use super::command_history::{
    append_global_history_in_txn, list_command_history_in_txn, sanitize_history_command,
};
use super::{ConnectionStore, StorageError, current_time_ms, lower_hex};
use nyaterm_core::completion::privacy::command_may_be_learned;
use nyaterm_core::completion::types::{
    CompletionHistorySnapshot, CompletionScope, ScopedHistoryRecord, ShellDialect,
};
use nyaterm_core::models::quick_commands::CommandHistoryEntry;
use redb::{ReadTransaction, ReadableDatabase, ReadableTable, TableDefinition};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

pub(super) const COMPLETION_HISTORY_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("completion_history");
const VERSION: u32 = 1;
const LIMIT: usize = 5000;
type HistoryUpdate = (u64, ScopedHistoryRecord, Vec<String>);

fn record_key(scope: &CompletionScope, command: &str) -> String {
    format!(
        "entry/{}/{}",
        scope.0,
        lower_hex(&Sha256::digest(command.as_bytes()))
    )
}
fn check_version(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
) -> Result<(), StorageError> {
    if let Some(value) = table.get("version")?
        && serde_json::from_slice::<u32>(value.value())? != VERSION
    {
        return Err(StorageError::CompletionHistory(
            "unsupported completion history version",
        ));
    }
    Ok(())
}
pub(super) fn bump_revision(txn: &redb::WriteTransaction) -> Result<u64, StorageError> {
    let mut table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
    check_version(&table)?;
    let revision = table
        .get("revision")?
        .map(|v| serde_json::from_slice::<u64>(v.value()))
        .transpose()?
        .unwrap_or_default()
        .saturating_add(1);
    table.insert("revision", serde_json::to_vec(&revision)?.as_slice())?;
    Ok(revision)
}

impl ConnectionStore {
    /// Shared by completion workers across windows; read without touching redb.
    pub fn completion_generation(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.completion_generation.clone()
    }
    pub(super) fn invalidate_completion(&self) {
        self.completion_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    pub(super) fn initialize_completion_history(&self) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        let mut table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
        if table.get("version")?.is_some() {
            return Ok(());
        }
        // The database actor initializes before publishing the store. Never infer
        // provenance from absent memberships after this one-time conversion.
        let legacy = self.list_command_history(usize::MAX)?;
        for entry in legacy {
            let record = ScopedHistoryRecord {
                version: VERSION,
                scope: CompletionScope::legacy(),
                dialect: ShellDialect::Unknown,
                command: entry.command,
                use_count: entry.use_count,
                last_used_at_ms: entry.last_used_at_ms,
                extensions: BTreeMap::new(),
            };
            table.insert(
                record_key(&record.scope, &record.command).as_str(),
                serde_json::to_vec(&record)?.as_slice(),
            )?;
        }
        table.insert("version", serde_json::to_vec(&VERSION)?.as_slice())?;
        drop(table);
        bump_revision(&txn)?;
        txn.commit()?;
        Ok(())
    }

    pub fn list_completion_history(&self) -> Result<(u64, Vec<ScopedHistoryRecord>), StorageError> {
        list_completion_history_in_txn(&self.db.begin_read()?)
    }

    pub fn completion_history_snapshot(&self) -> Result<CompletionHistorySnapshot, StorageError> {
        let txn = self.db.begin_read()?;
        let (revision, scoped_history) = list_completion_history_in_txn(&txn)?;
        let global_history = list_command_history_in_txn(&txn, usize::MAX)?;
        Ok(CompletionHistorySnapshot {
            revision,
            global_history,
            scoped_history,
        })
    }

    /// Adds one accepted logical command, updating global and scoped rows atomically.
    pub fn append_completion_history(
        &self,
        scope: &CompletionScope,
        dialect: ShellDialect,
        command: &str,
    ) -> Result<Option<HistoryUpdate>, StorageError> {
        self.append_completion_contribution(scope, dialect, command, true)
    }
    pub fn append_completion_contribution(
        &self,
        scope: &CompletionScope,
        dialect: ShellDialect,
        command: &str,
        include_global: bool,
    ) -> Result<Option<HistoryUpdate>, StorageError> {
        if !command_may_be_learned(command, false) || scope == &CompletionScope::legacy() {
            return Ok(None);
        }
        let Some(command) = sanitize_history_command(command) else {
            return Ok(None);
        };
        let txn = self.db.begin_write()?;
        let mut table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
        check_version(&table)?;
        let key = record_key(scope, &command);
        let mut record = table
            .get(key.as_str())?
            .map(|value| serde_json::from_slice::<ScopedHistoryRecord>(value.value()))
            .transpose()?
            .unwrap_or(ScopedHistoryRecord {
                version: VERSION,
                scope: scope.clone(),
                dialect,
                command: command.clone(),
                use_count: 0,
                last_used_at_ms: 0,
                extensions: BTreeMap::new(),
            });
        if record.version != VERSION {
            return Err(StorageError::CompletionHistory(
                "unsupported completion history record",
            ));
        }
        record.use_count = record.use_count.saturating_add(1);
        record.last_used_at_ms = current_time_ms();
        table.insert(key.as_str(), serde_json::to_vec(&record)?.as_slice())?;
        let prefix = format!("entry/{}/", scope.0);
        let mut entries = Vec::new();
        for entry in table.range(prefix.as_str()..)? {
            let (key, value) = entry?;
            if !key.value().starts_with(&prefix) {
                break;
            }
            let row: ScopedHistoryRecord = serde_json::from_slice(value.value())?;
            if row.version != VERSION {
                return Err(StorageError::CompletionHistory(
                    "unsupported completion history record",
                ));
            }
            entries.push((key.value().to_string(), row));
        }
        entries.sort_by(|a, b| {
            a.1.last_used_at_ms
                .cmp(&b.1.last_used_at_ms)
                .then(a.0.cmp(&b.0))
        });
        let overflow = entries.len().saturating_sub(LIMIT);
        let mut evicted = Vec::new();
        for (key, row) in entries.into_iter().take(overflow) {
            table.remove(key.as_str())?;
            evicted.push(row.command);
        }
        drop(table);
        if include_global {
            append_global_history_in_txn(&txn, &command, record.last_used_at_ms)?;
        }
        let revision = bump_revision(&txn)?;
        txn.commit()?;
        self.invalidate_completion();
        Ok(Some((revision, record, evicted)))
    }
}

fn list_completion_history_in_txn(
    txn: &ReadTransaction,
) -> Result<(u64, Vec<ScopedHistoryRecord>), StorageError> {
    let table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
    check_version(&table)?;
    let revision = table
        .get("revision")?
        .map(|v| serde_json::from_slice::<u64>(v.value()))
        .transpose()?
        .unwrap_or_default();
    let mut records = Vec::new();
    for entry in table.range("entry/".."entry0")? {
        let (_, value) = entry?;
        let record: ScopedHistoryRecord = serde_json::from_slice(value.value())?;
        if record.version != VERSION {
            return Err(StorageError::CompletionHistory(
                "unsupported completion history record",
            ));
        }
        records.push(record);
    }
    Ok((revision, records))
}

#[cfg(test)]
mod snapshot_tests {
    use crate::storage::completion_history::{COMPLETION_HISTORY_TABLE, record_key};
    use crate::storage::{COMMAND_HISTORY_TABLE, ConnectionStore, write_json_in_txn};
    use nyaterm_core::completion::types::{CompletionScope, ShellDialect};
    use nyaterm_core::models::quick_commands::CommandHistoryEntry;
    use nyaterm_core::test_support::TestTempDir;
    use redb::{ReadableDatabase, ReadableTable};
    use std::sync::{Arc, Barrier, atomic::Ordering};

    #[test]
    fn missing_global_table_keeps_scoped_history_available_in_snapshot() {
        let dir = TestTempDir::new("nyaterm-completion-missing-global-table");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let scope = CompletionScope::new(&["fixture"]);
        store
            .append_completion_history(&scope, ShellDialect::Bash, "fixture scoped")
            .unwrap();
        let txn = store.db.begin_write().unwrap();
        txn.delete_table(COMMAND_HISTORY_TABLE).unwrap();
        txn.commit().unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert!(snapshot.global_history.is_empty());
        assert_eq!(snapshot.scoped_history[0].command, "fixture scoped");
        assert!(store.list_command_history(64).unwrap().is_empty());
    }

    #[test]
    fn future_table_version_rolls_back_global_append_without_invalidating_cache() {
        let dir = TestTempDir::new("nyaterm-completion-future-global");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let generation = store.completion_generation();
        let previous = generation.load(Ordering::Acquire);
        let txn = store.db.begin_write().unwrap();
        txn.open_table(COMPLETION_HISTORY_TABLE)
            .unwrap()
            .insert("version", b"99".as_slice())
            .unwrap();
        txn.commit().unwrap();
        assert!(store.append_command_history("fixture rejected").is_err());
        assert!(store.list_command_history(64).unwrap().is_empty());
        assert!(store.completion_history_snapshot().is_err());
        assert_eq!(generation.load(Ordering::Acquire), previous);
    }

    #[test]
    fn global_only_append_invalidates_snapshot_and_survives_restart_without_scope_membership() {
        let dir = TestTempDir::new("nyaterm-completion-global-snapshot");
        let before;
        {
            let store = ConnectionStore::open(dir.path()).unwrap();
            before = store.completion_history_snapshot().unwrap().revision;
            let generation = store.completion_generation();
            let previous = generation.load(Ordering::Acquire);
            store.append_command_history("fixture global-only").unwrap();
            assert_eq!(generation.load(Ordering::Acquire), previous.wrapping_add(1));
            let snapshot = store.completion_history_snapshot().unwrap();
            assert!(snapshot.revision > before);
            assert_eq!(snapshot.global_history[0].command, "fixture global-only");
            assert!(snapshot.scoped_history.is_empty());
            assert!(!format!("{snapshot:?}").contains("fixture global-only"));
        }
        let store = ConnectionStore::open(dir.path()).unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert!(snapshot.revision > before);
        assert_eq!(snapshot.global_history[0].use_count, 1);
        assert!(snapshot.scoped_history.is_empty());
        store.delete_command_history("fixture global-only").unwrap();
        assert!(
            store
                .completion_history_snapshot()
                .unwrap()
                .global_history
                .is_empty()
        );
    }

    #[test]
    fn snapshot_reads_global_and_scoped_contributions_from_one_transaction() {
        let dir = TestTempDir::new("nyaterm-completion-consistent-snapshot");
        let store = Arc::new(ConnectionStore::open(dir.path()).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let scope = CompletionScope::new(&["fixture"]);
        let writer_store = store.clone();
        let writer_barrier = barrier.clone();
        let writer_scope = scope.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            for _ in 0..16 {
                writer_store
                    .append_completion_history(&writer_scope, ShellDialect::Bash, "fixture atomic")
                    .unwrap();
            }
        });
        barrier.wait();
        for _ in 0..32 {
            let snapshot = store.completion_history_snapshot().unwrap();
            let global = snapshot
                .global_history
                .first()
                .map_or(0, |row| row.use_count);
            let scoped = snapshot
                .scoped_history
                .first()
                .map_or(0, |row| row.use_count);
            assert_eq!(global, scoped);
        }
        writer.join().unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert_eq!(snapshot.global_history[0].use_count, 16);
        assert_eq!(snapshot.scoped_history[0].use_count, 16);
    }

    #[test]
    fn snapshot_preserves_legacy_normalization_and_raw_unknown_fields() {
        let dir = TestTempDir::new("nyaterm-completion-compatible-snapshot");
        let store = ConnectionStore::open(dir.path()).unwrap();
        store
            .replace_command_history(&[CommandHistoryEntry {
                command: "fixture legacy".into(),
                use_count: 2,
                last_used_at_ms: 10,
            }])
            .unwrap();
        let scope = CompletionScope::new(&["fixture"]);
        store
            .append_completion_history(&scope, ShellDialect::Bash, "fixture scoped")
            .unwrap();
        let txn = store.db.begin_write().unwrap();
        let global = serde_json::json!({"command":"\u{0005}\u{0015}fixture legacy", "use_count":3, "last_used_at_ms":20, "future":{"retain":true}});
        let key = "command_history/00000000000000000020|fixture-legacy-rewrite";
        write_json_in_txn(&txn, COMMAND_HISTORY_TABLE, key, &global).unwrap();
        let scoped_key = record_key(&scope, "fixture scoped");
        let mut table = txn.open_table(COMPLETION_HISTORY_TABLE).unwrap();
        let mut raw: serde_json::Value =
            serde_json::from_slice(table.get(scoped_key.as_str()).unwrap().unwrap().value())
                .unwrap();
        raw["future"] = serde_json::json!({"retain":true});
        table
            .insert(
                scoped_key.as_str(),
                serde_json::to_vec(&raw).unwrap().as_slice(),
            )
            .unwrap();
        drop(table);
        txn.commit().unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        let legacy = snapshot
            .global_history
            .iter()
            .find(|entry| entry.command == "fixture legacy")
            .unwrap();
        assert_eq!(legacy.use_count, 5);
        assert_eq!(legacy.last_used_at_ms, 20);
        assert_eq!(
            snapshot
                .scoped_history
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .extensions["future"],
            raw["future"]
        );
        let txn = store.db.begin_read().unwrap();
        let table = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
        let persisted: serde_json::Value =
            serde_json::from_slice(table.get(key).unwrap().unwrap().value()).unwrap();
        assert_eq!(persisted, global);
    }
}

pub(super) fn delete_completion_command_in_txn(
    txn: &redb::WriteTransaction,
    command: &str,
) -> Result<(), StorageError> {
    let mut table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
    check_version(&table)?;
    let suffix = format!("/{}", lower_hex(&Sha256::digest(command.as_bytes())));
    let mut keys = Vec::new();
    for row in table.range("entry/".."entry0")? {
        let (key, value) = row?;
        if key.value().ends_with(&suffix) {
            let record: ScopedHistoryRecord = serde_json::from_slice(value.value())?;
            if record.version != VERSION {
                return Err(StorageError::CompletionHistory(
                    "unsupported completion history record",
                ));
            }
            keys.push(key.value().to_string());
        }
    }
    for key in keys {
        table.remove(key.as_str())?;
    }
    drop(table);
    bump_revision(txn)?;
    Ok(())
}

pub(super) fn reset_completion_history_in_txn(
    txn: &redb::WriteTransaction,
    entries: &[CommandHistoryEntry],
) -> Result<(), StorageError> {
    let mut table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
    check_version(&table)?;
    let mut keys = Vec::new();
    for row in table.range("entry/".."entry0")? {
        let (key, value) = row?;
        let record: ScopedHistoryRecord = serde_json::from_slice(value.value())?;
        if record.version != VERSION {
            return Err(StorageError::CompletionHistory(
                "unsupported completion history record",
            ));
        }
        keys.push(key.value().to_string());
    }
    let mut merged = HashMap::<String, CommandHistoryEntry>::new();
    for entry in entries {
        if let Some(command) = sanitize_history_command(&entry.command) {
            let row = merged
                .entry(command.clone())
                .or_insert(CommandHistoryEntry {
                    command,
                    use_count: 0,
                    last_used_at_ms: 0,
                });
            row.use_count = row.use_count.saturating_add(entry.use_count.max(1));
            row.last_used_at_ms = row.last_used_at_ms.max(entry.last_used_at_ms);
        }
    }
    for key in keys {
        table.remove(key.as_str())?;
    }
    let mut entries: Vec<_> = merged.into_values().collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.last_used_at_ms));
    for entry in entries.into_iter().take(LIMIT) {
        let record = ScopedHistoryRecord {
            version: VERSION,
            scope: CompletionScope::legacy(),
            dialect: ShellDialect::Unknown,
            command: entry.command,
            use_count: entry.use_count,
            last_used_at_ms: entry.last_used_at_ms,
            extensions: BTreeMap::new(),
        };
        table.insert(
            record_key(&record.scope, &record.command).as_str(),
            serde_json::to_vec(&record)?.as_slice(),
        )?;
    }
    drop(table);
    bump_revision(txn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::storage::ConnectionStore;
    use nyaterm_core::completion::types::{CompletionScope, ShellDialect};
    use nyaterm_core::models::quick_commands::CommandHistoryEntry;
    #[test]
    fn scoped_history_survives_restart_and_is_removed_with_global_history() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-completion-store");
        let scope = CompletionScope::new(&["ssh", "one", "22", "user"]);
        {
            let store = ConnectionStore::open(dir.path()).unwrap();
            store
                .replace_command_history(&[CommandHistoryEntry {
                    command: "old tool".into(),
                    use_count: 4,
                    last_used_at_ms: 1,
                }])
                .unwrap();
            store
                .append_completion_history(&scope, ShellDialect::Bash, "docker run nginx")
                .unwrap();
            store
                .append_completion_history(&scope, ShellDialect::Bash, "docker run nginx")
                .unwrap();
            assert_eq!(
                store
                    .list_completion_history()
                    .unwrap()
                    .1
                    .iter()
                    .find(|r| r.scope == scope)
                    .unwrap()
                    .use_count,
                2
            );
        }
        let store = ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(store.list_completion_history().unwrap().1.len(), 2);
        store.delete_command_history("docker run nginx").unwrap();
        assert!(
            store
                .list_completion_history()
                .unwrap()
                .1
                .iter()
                .all(|r| r.scope == CompletionScope::legacy())
        );
        store.replace_command_history(&[]).unwrap();
        assert!(store.list_completion_history().unwrap().1.is_empty());
    }
}

#[cfg(test)]
mod compatibility_tests {
    use crate::storage::completion_history::{COMPLETION_HISTORY_TABLE, record_key};
    use crate::storage::{COMMAND_HISTORY_TABLE, ConnectionStore, DATABASE_FILE};
    use nyaterm_core::completion::types::{CompletionScope, ShellDialect};
    use nyaterm_core::test_support::TestTempDir;
    use redb::{Database, ReadableDatabase, ReadableTable};
    #[test]
    fn initializes_legacy_provenance_once_and_preserves_global_extension_fields() {
        let dir = TestTempDir::new("nyaterm-completion-legacy");
        std::fs::create_dir_all(dir.path()).unwrap();
        {
            let db = Database::create(dir.path().join(DATABASE_FILE)).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut table = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
                let document = serde_json::json!({"command":"internal-tool deploy","use_count":4,"last_used_at_ms":1,"future":{"kept":true}});
                table
                    .insert(
                        "command_history/00000000000000000001|fixture",
                        serde_json::to_vec(&document).unwrap().as_slice(),
                    )
                    .unwrap();
            }
            txn.commit().unwrap();
        }
        let scope = CompletionScope::new(&["new-host"]);
        {
            let store = ConnectionStore::open(dir.path()).unwrap();
            let (_, records) = store.list_completion_history().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].scope, CompletionScope::legacy());
            assert_eq!(records[0].use_count, 4);
            store
                .append_completion_history(&scope, ShellDialect::Bash, "internal-tool deploy")
                .unwrap();
            let txn = store.db.begin_read().unwrap();
            let table = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
            assert!(table.iter().unwrap().any(|row| {
                let (_, value) = row.unwrap();
                let document: serde_json::Value = serde_json::from_slice(value.value()).unwrap();
                document.get("future") == Some(&serde_json::json!({"kept":true}))
            }));
        }
        let store = ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(store.list_completion_history().unwrap().1.len(), 2);
        for kind in [
            nyaterm_core::PortableSnapshotKind::Backup,
            nyaterm_core::PortableSnapshotKind::Sync,
        ] {
            let snapshot = store
                .build_raw_portable_snapshot(kind, "test-device", "test-version")
                .unwrap();
            assert!(!snapshot.entities.contains_key("completion_history"));
        }
    }
    #[test]
    fn preserves_unknown_fields_and_refuses_future_records_before_mutation() {
        let dir = TestTempDir::new("nyaterm-completion-future");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let scope = CompletionScope::new(&["host"]);
        let (_, mut record, _) = store
            .append_completion_history(&scope, ShellDialect::Bash, "tool run")
            .unwrap()
            .unwrap();
        record
            .extensions
            .insert("future".into(), serde_json::json!({"kept":true}));
        let key = record_key(&scope, &record.command);
        let write = |record: &nyaterm_core::completion::types::ScopedHistoryRecord| {
            let txn = store.db.begin_write().unwrap();
            {
                txn.open_table(COMPLETION_HISTORY_TABLE)
                    .unwrap()
                    .insert(key.as_str(), serde_json::to_vec(record).unwrap().as_slice())
                    .unwrap();
            }
            txn.commit().unwrap();
        };
        write(&record);
        let (_, mut updated, _) = store
            .append_completion_history(&scope, ShellDialect::Bash, "tool run")
            .unwrap()
            .unwrap();
        assert_eq!(
            updated.extensions["future"],
            serde_json::json!({"kept":true})
        );
        updated.version = 99;
        write(&updated);
        let global = store.list_command_history(64).unwrap();
        assert!(
            store
                .append_completion_history(&scope, ShellDialect::Bash, "tool run")
                .is_err()
        );
        assert!(store.delete_command_history("tool run").is_err());
        assert!(store.replace_command_history(&[]).is_err());
        assert_eq!(store.list_command_history(64).unwrap(), global);
        let txn = store.db.begin_read().unwrap();
        let table = txn.open_table(COMPLETION_HISTORY_TABLE).unwrap();
        assert_eq!(
            table.get(key.as_str()).unwrap().unwrap().value(),
            serde_json::to_vec(&updated).unwrap()
        );
    }
    #[test]
    fn trims_only_the_current_scope_and_keeps_global_counts_once_per_broadcast() {
        use nyaterm_core::completion::types::ScopedHistoryRecord;
        use std::collections::BTreeMap;
        let dir = TestTempDir::new("nyaterm-completion-retention");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let scope = CompletionScope::new(&["one"]);
        let other = CompletionScope::new(&["two"]);
        let txn = store.db.begin_write().unwrap();
        {
            let mut table = txn.open_table(COMPLETION_HISTORY_TABLE).unwrap();
            for i in 0..5000 {
                let record = ScopedHistoryRecord {
                    version: 1,
                    scope: scope.clone(),
                    dialect: ShellDialect::Bash,
                    command: format!("fixture arg-{i:04}"),
                    use_count: 1,
                    last_used_at_ms: i,
                    extensions: BTreeMap::new(),
                };
                table
                    .insert(
                        record_key(&scope, &record.command).as_str(),
                        serde_json::to_vec(&record).unwrap().as_slice(),
                    )
                    .unwrap();
            }
        }
        txn.commit().unwrap();
        let (_, _, evicted) = store
            .append_completion_history(&scope, ShellDialect::Bash, "fixture last")
            .unwrap()
            .unwrap();
        assert_eq!(evicted, ["fixture arg-0000"]);
        store
            .append_completion_contribution(&other, ShellDialect::Bash, "fixture last", false)
            .unwrap();
        let (_, records) = store.list_completion_history().unwrap();
        assert_eq!(records.iter().filter(|r| r.scope == scope).count(), 5000);
        assert_eq!(records.iter().filter(|r| r.scope == other).count(), 1);
        assert_eq!(store.list_command_history(64).unwrap()[0].use_count, 1);
    }
}
