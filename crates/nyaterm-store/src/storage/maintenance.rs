//! Local storage maintenance reports contain counters only, never user data.
use std::time::{Duration, Instant};

use crate::storage::{ConnectionStore, StorageError};

const COMPACT_MIN_FREE_BYTES: u64 = 4 * 1024 * 1024;
const COMPACT_MIN_FREE_PERCENT: u64 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCompactionOutcome {
    NotNeeded,
    Completed { compacted: bool },
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStorageMaintenanceResult {
    pub bytes_before: Option<u64>,
    pub bytes_after: Option<u64>,
    pub elapsed: Duration,
    pub replicas_rewritten: usize,
    pub journal_rows_removed: usize,
    pub compaction: StorageCompactionOutcome,
}

#[derive(Default)]
pub(super) struct ReplicaMigrationPreparation {
    pub replicas_rewritten: usize,
    pub journal_rows_removed: usize,
    pub pending_compaction: bool,
}

impl ConnectionStore {
    /// Runs once on an idle Store worker, after bootstrap. Migration and
    /// routine reclaim share one compaction attempt; failure keeps `records`
    /// available for a later process to resume.
    pub fn maintain_local_storage(
        &mut self,
    ) -> Result<LocalStorageMaintenanceResult, StorageError> {
        let started = Instant::now();
        let bytes_before = self.storage_file_len();
        let migration = self.prepare_local_replica_migration()?;
        let compaction = if migration.pending_compaction || self.storage_compaction_beneficial() {
            match self.db.compact() {
                Ok(compacted) => {
                    if migration.pending_compaction {
                        self.finish_local_replica_migration()?;
                    }
                    StorageCompactionOutcome::Completed { compacted }
                }
                Err(_) => StorageCompactionOutcome::Failed,
            }
        } else {
            StorageCompactionOutcome::NotNeeded
        };
        Ok(LocalStorageMaintenanceResult {
            bytes_before,
            bytes_after: self.storage_file_len(),
            elapsed: started.elapsed(),
            replicas_rewritten: migration.replicas_rewritten,
            journal_rows_removed: migration.journal_rows_removed,
            compaction,
        })
    }

    fn storage_file_len(&self) -> Option<u64> {
        std::fs::metadata(&self.db_path)
            .ok()
            .map(|metadata| metadata.len())
    }

    fn storage_compaction_beneficial(&self) -> bool {
        let Some(file_len) = self.storage_file_len().filter(|len| *len > 0) else {
            return false;
        };
        // Opening commits table initialization before bootstrap, so redb has
        // already released pages left pending by the previous process.
        let allocated = self.db.begin_write().ok().and_then(|txn| {
            txn.stats()
                .ok()
                .map(|stats| stats.allocated_pages() * stats.page_size() as u64)
        });
        let Some(allocated) = allocated else {
            return false;
        };
        let free = file_len.saturating_sub(allocated);
        free >= COMPACT_MIN_FREE_BYTES
            && u128::from(free) * 100 > u128::from(file_len) * u128::from(COMPACT_MIN_FREE_PERCENT)
    }
}
