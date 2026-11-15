//! Retention operates on raw documents, after validating a durable recovery copy.
use super::command_history::{list_command_history_in_txn, sanitize_history_command};
use super::completion_history::{COMPLETION_HISTORY_TABLE, bump_revision, check_version};
use super::{
    COMMAND_HISTORY_PREFIX, COMMAND_HISTORY_TABLE, ConnectionStore, StorageError, current_time_ms,
};
use nyaterm_core::completion::types::{CompletionScope, ScopedHistoryRecord};
use redb::{
    Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    TableHandle as _,
};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::sync::atomic::Ordering;

pub(super) const HISTORY_LIMIT: usize = 5000;
#[derive(Debug, Default)]
pub(super) struct RetentionState {
    backup_failed: bool,
    prepared_generation: Option<u64>,
    warning: Option<String>,
}

impl ConnectionStore {
    pub fn take_command_history_retention_warning(&self) -> Option<String> {
        self.history_retention
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .warning
            .take()
    }

    /// Existing oversized data is never deleted without a verified recovery copy.
    /// Call before acquiring the append transaction, on a blocking worker.
    pub(super) fn prepare_history_retention(&self) -> Result<bool, StorageError> {
        let generation = self.completion_generation().load(Ordering::Acquire);
        let txn = self.db.begin_read()?;
        let raw_count = match txn.open_table(COMMAND_HISTORY_TABLE) {
            Ok(table) => table.len()?,
            Err(redb::TableError::TableDoesNotExist(_)) => 0,
            Err(error) => return Err(error.into()),
        };
        {
            let state = self
                .history_retention
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if state.prepared_generation == Some(generation) && raw_count <= HISTORY_LIMIT as u64 {
                return Ok(!state.backup_failed);
            }
        }
        let global = list_command_history_in_txn(&txn, usize::MAX)?;
        let retained: HashSet<_> = global.iter().map(|entry| entry.command.as_str()).collect();
        let legacy = validated_legacy(&txn)?;
        let needs_backup = global.len() > HISTORY_LIMIT
            || legacy
                .iter()
                .any(|(_, command)| !retained.contains(command.as_str()));
        let mut state = self
            .history_retention
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prepared_generation = Some(generation);
        if state.backup_failed {
            return Ok(false);
        }
        if !needs_backup {
            return Ok(true);
        }
        match self.backup_history_database(&txn) {
            Ok(_) => Ok(true),
            Err(_) => {
                state.backup_failed = true;
                state.warning = Some("History safety backup failed; existing history was preserved. Check backup directory access and disk space.".into());
                Ok(false)
            }
        }
    }

    /// Ordinary mutations already validated retention. Imports and other
    /// writers leave a different generation and must be prepared again.
    pub(super) fn note_history_retention_committed(&self) {
        self.history_retention
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prepared_generation = Some(self.completion_generation().load(Ordering::Acquire));
    }

    pub(super) fn maintain_command_history(&self) -> Result<(), StorageError> {
        if !self.prepare_history_retention()? {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        let (global, legacy) = retain_history_in_txn(&txn)?;
        if global.is_empty() && legacy.is_empty() {
            return Ok(());
        }
        bump_revision(&txn)?;
        txn.commit()?;
        self.invalidate_completion();
        self.note_history_retention_committed();
        Ok(())
    }

    fn backup_history_database(
        &self,
        source: &redb::ReadTransaction,
    ) -> Result<std::path::PathBuf, StorageError> {
        let root = self
            .db_path
            .parent()
            .ok_or_else(|| StorageError::InvalidData("database has no parent directory".into()))?
            .join("history-backups");
        std::fs::create_dir_all(&root).map_err(|source| StorageError::CreateDir {
            path: root.clone(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = root.join(format!(
            "nyaterm.history-retention-{}-{}.redb",
            current_time_ms(),
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        drop(file);
        let result = (|| {
            let backup = Database::create(&path).map_err(|source| StorageError::Open {
                path: path.clone(),
                source,
            })?;
            let txn = backup.begin_write()?;
            if source.list_multimap_tables()?.next().is_some() {
                return Err(StorageError::InvalidData(
                    "unsupported multimap backup layout".into(),
                ));
            }
            for handle in source.list_tables()? {
                let name = handle.name();
                match source.open_table(TableDefinition::<&str, &str>::new(name)) {
                    Ok(table) => {
                        let mut target =
                            txn.open_table(TableDefinition::<&str, &str>::new(name))?;
                        for row in table.iter()? {
                            let (key, value) = row?;
                            target.insert(key.value(), value.value())?;
                        }
                    }
                    Err(redb::TableError::TableTypeMismatch { .. }) => {
                        match source.open_table(TableDefinition::<&str, &[u8]>::new(name)) {
                            Ok(table) => {
                                let mut target =
                                    txn.open_table(TableDefinition::<&str, &[u8]>::new(name))?;
                                for row in table.iter()? {
                                    let (key, value) = row?;
                                    target.insert(key.value(), value.value())?;
                                }
                            }
                            Err(redb::TableError::TableTypeMismatch { .. }) => {
                                let table =
                                    source.open_table(TableDefinition::<&str, u64>::new(name))?;
                                let mut target =
                                    txn.open_table(TableDefinition::<&str, u64>::new(name))?;
                                for row in table.iter()? {
                                    let (key, value) = row?;
                                    target.insert(key.value(), value.value())?;
                                }
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            txn.commit()?;
            let expected = Self::transaction_fingerprint(source)?;
            drop(backup);
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)?
                .sync_all()?;
            Self::verify_history_backup(&path, &expected)?;
            #[cfg(unix)]
            {
                std::fs::File::open(&root)?.sync_all()?;
            }
            Ok(path.clone())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        result
    }
    fn verify_history_backup(path: &std::path::Path, expected: &str) -> Result<(), StorageError> {
        let mut verified = Database::open(path).map_err(|source| StorageError::Open {
            path: path.into(),
            source,
        })?;
        if !verified
            .check_integrity()
            .map_err(|source| StorageError::Open {
                path: path.into(),
                source,
            })?
            || Self::database_fingerprint(&verified)? != expected
        {
            return Err(StorageError::InvalidData(
                "history safety backup verification failed".into(),
            ));
        }
        Ok(())
    }
}

fn validated_legacy(txn: &redb::ReadTransaction) -> Result<Vec<(String, String)>, StorageError> {
    let table = txn.open_table(COMPLETION_HISTORY_TABLE)?;
    check_version(&table)?;
    let mut legacy = Vec::new();
    for row in table.range("entry/".."entry0")? {
        let (key, value) = row?;
        let record: ScopedHistoryRecord = serde_json::from_slice(value.value())?;
        if record.version != 1 {
            return Err(StorageError::CompletionHistory(
                "unsupported completion history record",
            ));
        }
        if record.scope == CompletionScope::legacy() {
            legacy.push((key.value().into(), record.command));
        }
    }
    Ok(legacy)
}

pub(super) fn retain_history_in_txn(
    txn: &redb::WriteTransaction,
) -> Result<(Vec<String>, Vec<String>), StorageError> {
    let table = txn.open_table(COMMAND_HISTORY_TABLE)?;
    let mut rows = Vec::new();
    for row in table.range(COMMAND_HISTORY_PREFIX..)? {
        let (key, value) = row?;
        if !key.value().starts_with(COMMAND_HISTORY_PREFIX) {
            break;
        }
        let entry: nyaterm_core::models::quick_commands::CommandHistoryEntry =
            serde_json::from_slice(value.value())?;
        if let Some(command) = sanitize_history_command(&entry.command) {
            rows.push((key.value().to_string(), command, entry.last_used_at_ms));
        }
    }
    drop(table);
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
    let mut kept = HashSet::new();
    let mut remove = Vec::new();
    for (key, command, _) in &rows {
        if !kept.contains(command) && kept.len() >= HISTORY_LIMIT {
            remove.push(key.clone());
        } else {
            kept.insert(command.clone());
        }
    }
    let mut scoped = txn.open_table(COMPLETION_HISTORY_TABLE)?;
    check_version(&scoped)?;
    let mut legacy = Vec::new();
    for row in scoped.range("entry/".."entry0")? {
        let (key, value) = row?;
        let record: ScopedHistoryRecord = serde_json::from_slice(value.value())?;
        if record.version != 1 {
            return Err(StorageError::CompletionHistory(
                "unsupported completion history record",
            ));
        }
        if record.scope == CompletionScope::legacy() && !kept.contains(&record.command) {
            legacy.push((key.value().to_string(), record.command));
        }
    }
    for (key, _) in &legacy {
        scoped.remove(key.as_str())?;
    }
    drop(scoped);
    let mut table = txn.open_table(COMMAND_HISTORY_TABLE)?;
    for key in &remove {
        table.remove(key.as_str())?;
    }
    let evicted = rows
        .into_iter()
        .filter(|(_, command, _)| !kept.contains(command))
        .map(|(_, command, _)| command)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    Ok((
        evicted,
        legacy.into_iter().map(|(_, command)| command).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_LIMIT, retain_history_in_txn};
    use crate::storage::completion_history::COMPLETION_HISTORY_TABLE;
    use crate::storage::{COMMAND_HISTORY_TABLE, ConnectionStore};
    use nyaterm_core::completion::types::{CompletionScope, ScopedHistoryRecord, ShellDialect};
    use nyaterm_core::test_support::TestTempDir;
    use redb::{Database, ReadableDatabase, ReadableTable};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    fn seed(store: &ConnectionStore, count: usize, legacy: bool) {
        let txn = store.db.begin_write().unwrap();
        {
            let mut global = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
            let mut scoped = txn.open_table(COMPLETION_HISTORY_TABLE).unwrap();
            for i in 0..count {
                let command = format!("fixture {i:05}");
                let id = crate::storage::lower_hex(&Sha256::digest(command.as_bytes())[..16]);
                let key = format!("command_history/{i:020}|{id}");
                let document = serde_json::json!({"command": command, "last_used_at_ms": i, "use_count": 1, "fixture_extension": {"version": 42}});
                global
                    .insert(
                        key.as_str(),
                        serde_json::to_vec(&document).unwrap().as_slice(),
                    )
                    .unwrap();
                if legacy {
                    let scope = CompletionScope::legacy();
                    let key = format!(
                        "entry/{}/{}",
                        scope.0,
                        crate::storage::lower_hex(&Sha256::digest(command.as_bytes()))
                    );
                    let record = ScopedHistoryRecord {
                        version: 1,
                        scope,
                        dialect: ShellDialect::Unknown,
                        command,
                        use_count: 1,
                        last_used_at_ms: i as u64,
                        extensions: BTreeMap::new(),
                    };
                    scoped
                        .insert(
                            key.as_str(),
                            serde_json::to_vec(&record).unwrap().as_slice(),
                        )
                        .unwrap();
                }
            }
        }
        txn.commit().unwrap();
    }

    #[test]
    fn oversized_history_is_backed_up_with_raw_fields_before_legacy_is_trimmed() {
        let dir = TestTempDir::new("nyaterm-history-retention-backup");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 2, true);
        let txn = store.db.begin_write().unwrap();
        txn.open_table(redb::TableDefinition::<&str, u64>::new(
            "sync_v4_field_edits",
        ))
        .unwrap()
        .insert("fixture", 42)
        .unwrap();
        txn.commit().unwrap();
        let expected = ConnectionStore::database_fingerprint(&store.db).unwrap();
        store.maintain_command_history().unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert_eq!(snapshot.global_history.len(), HISTORY_LIMIT);
        assert_eq!(snapshot.scoped_history.len(), HISTORY_LIMIT);
        assert!(
            !snapshot
                .global_history
                .iter()
                .any(|entry| entry.command == "fixture 00000")
        );
        let paths: Vec<_> = std::fs::read_dir(dir.path().join("history-backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(paths.len(), 1);
        let backup = Database::open(&paths[0]).unwrap();
        assert_eq!(
            ConnectionStore::database_fingerprint(&backup).unwrap(),
            expected
        );
        let txn = backup.begin_read().unwrap();
        let table = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
        let (_, value) = table.iter().unwrap().next().unwrap().unwrap();
        let raw: serde_json::Value = serde_json::from_slice(value.value()).unwrap();
        assert_eq!(raw["fixture_extension"]["version"], 42);
        store.maintain_command_history().unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path().join("history-backups"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn ordinary_growth_is_bounded_and_repeated_use_preserves_extension_fields() {
        let dir = TestTempDir::new("nyaterm-history-retention-growth");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT, false);
        store.append_command_history("fixture 00000").unwrap();
        store.append_command_history("fixture newest").unwrap();
        let entries = store.list_command_history(usize::MAX).unwrap();
        assert_eq!(entries.len(), HISTORY_LIMIT);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.command == "fixture 00000")
                .unwrap()
                .use_count,
            2
        );
        assert!(!entries.iter().any(|entry| entry.command == "fixture 00001"));
        assert!(!dir.path().join("history-backups").exists());
        let txn = store.db.begin_read().unwrap();
        let table = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
        let raw: Vec<serde_json::Value> = table
            .iter()
            .unwrap()
            .map(|row| serde_json::from_slice(row.unwrap().1.value()).unwrap())
            .collect();
        assert_eq!(
            raw.iter()
                .find(|entry| entry["command"] == "fixture 00000")
                .unwrap()["fixture_extension"]["version"],
            42
        );
    }

    #[test]
    fn backup_failure_preserves_old_history_and_accepts_new_commands_without_repeated_exports() {
        let dir = TestTempDir::new("nyaterm-history-retention-failure");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 1, true);
        std::fs::write(dir.path().join("history-backups"), []).unwrap();
        store.maintain_command_history().unwrap();
        assert!(store.take_command_history_retention_warning().is_some());
        store.append_command_history("fixture incoming").unwrap();
        assert_eq!(
            store.list_command_history(usize::MAX).unwrap().len(),
            HISTORY_LIMIT + 2
        );
        assert!(store.take_command_history_retention_warning().is_none());
    }

    #[test]
    fn unknown_record_versions_abort_retention_before_deleting_any_history() {
        let dir = TestTempDir::new("nyaterm-history-retention-future");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 1, true);
        let txn = store.db.begin_write().unwrap();
        txn.open_table(COMPLETION_HISTORY_TABLE)
            .unwrap()
            .insert("version", b"99".as_slice())
            .unwrap();
        txn.commit().unwrap();
        assert!(store.maintain_command_history().is_err());
        assert!(store.append_command_history("fixture rejected").is_err());
        assert_eq!(
            store.list_command_history(usize::MAX).unwrap().len(),
            HISTORY_LIMIT + 1
        );
        assert!(!dir.path().join("history-backups").exists());
    }

    #[test]
    fn legacy_eviction_keeps_an_independent_environment_contribution() {
        let dir = TestTempDir::new("nyaterm-history-retention-scopes");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 1, true);
        let scope = CompletionScope::new(&["independent"]);
        store
            .append_completion_contribution(&scope, ShellDialect::Bash, "fixture 00000", false)
            .unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert!(
            !snapshot
                .global_history
                .iter()
                .any(|entry| entry.command == "fixture 00000")
        );
        assert!(
            snapshot
                .scoped_history
                .iter()
                .any(|entry| entry.scope == scope && entry.command == "fixture 00000")
        );
        let txn = store.db.begin_write().unwrap();
        let (global, legacy) = retain_history_in_txn(&txn).unwrap();
        assert!(global.is_empty() && legacy.is_empty());
    }
    #[test]
    fn equal_timestamps_retain_a_deterministic_command_order() {
        let dir = TestTempDir::new("nyaterm-history-retention-ties");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 1, false);
        let txn = store.db.begin_write().unwrap();
        {
            let mut table = txn.open_table(COMMAND_HISTORY_TABLE).unwrap();
            let rows: Vec<_> = table
                .iter()
                .unwrap()
                .map(|row| {
                    let (key, value) = row.unwrap();
                    let mut document: serde_json::Value =
                        serde_json::from_slice(value.value()).unwrap();
                    document["last_used_at_ms"] = 1.into();
                    (
                        key.value().to_string(),
                        serde_json::to_vec(&document).unwrap(),
                    )
                })
                .collect();
            for (key, value) in rows {
                table.insert(key.as_str(), value.as_slice()).unwrap();
            }
        }
        txn.commit().unwrap();
        store.maintain_command_history().unwrap();
        let entries = store.list_command_history(usize::MAX).unwrap();
        assert_eq!(entries.len(), HISTORY_LIMIT);
        assert!(entries.iter().any(|entry| entry.command == "fixture 00000"));
        assert!(!entries.iter().any(|entry| entry.command == "fixture 05000"));
    }

    #[test]
    fn unsupported_backup_table_type_prevents_all_retention_deletions() {
        let dir = TestTempDir::new("nyaterm-history-retention-unsupported-backup");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 1, true);
        let txn = store.db.begin_write().unwrap();
        txn.open_table(redb::TableDefinition::<u64, u64>::new(
            "fixture_future_table",
        ))
        .unwrap()
        .insert(1, 42)
        .unwrap();
        txn.commit().unwrap();
        store.maintain_command_history().unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert_eq!(snapshot.global_history.len(), HISTORY_LIMIT + 1);
        assert_eq!(snapshot.scoped_history.len(), HISTORY_LIMIT + 1);
        assert!(store.take_command_history_retention_warning().is_some());
        assert_eq!(
            std::fs::read_dir(dir.path().join("history-backups"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn backup_verification_rejects_readable_but_altered_documents() {
        let dir = TestTempDir::new("nyaterm-history-retention-corrupt-backup");
        let store = ConnectionStore::open(dir.path()).unwrap();
        seed(&store, HISTORY_LIMIT + 1, true);
        let source = store.db.begin_read().unwrap();
        let expected = ConnectionStore::transaction_fingerprint(&source).unwrap();
        let path = store.backup_history_database(&source).unwrap();
        let backup = Database::open(&path).unwrap();
        let txn = backup.begin_write().unwrap();
        txn.open_table(COMMAND_HISTORY_TABLE)
            .unwrap()
            .pop_first()
            .unwrap();
        txn.commit().unwrap();
        drop(backup);
        assert!(ConnectionStore::verify_history_backup(&path, &expected).is_err());
        assert_eq!(
            store.list_command_history(usize::MAX).unwrap().len(),
            HISTORY_LIMIT + 1
        );
        drop(source);
        std::fs::write(&path, b"corrupted backup").unwrap();
        assert!(ConnectionStore::verify_history_backup(&path, &expected).is_err());
    }
    #[test]
    fn a_new_database_generation_rechecks_orphan_legacy_even_below_the_global_limit() {
        let dir = TestTempDir::new("nyaterm-history-retention-import-generation");
        let store = ConnectionStore::open(dir.path()).unwrap();
        assert!(store.prepare_history_retention().unwrap());
        seed(&store, 2, true);
        let txn = store.db.begin_write().unwrap();
        txn.open_table(COMMAND_HISTORY_TABLE)
            .unwrap()
            .pop_first()
            .unwrap();
        txn.commit().unwrap();
        store.invalidate_completion();
        let expected = ConnectionStore::database_fingerprint(&store.db).unwrap();
        store.maintain_command_history().unwrap();
        let snapshot = store.completion_history_snapshot().unwrap();
        assert_eq!(snapshot.global_history.len(), 1);
        assert_eq!(snapshot.scoped_history.len(), 1);
        let paths: Vec<_> = std::fs::read_dir(dir.path().join("history-backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(paths.len(), 1);
        assert_eq!(
            ConnectionStore::database_fingerprint(&Database::open(&paths[0]).unwrap()).unwrap(),
            expected
        );
    }
}
