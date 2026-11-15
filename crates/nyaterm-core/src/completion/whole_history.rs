//! Derived whole-command catalog. Global history never seeds token learning.
use crate::completion::matching::matching_chars;
use crate::completion::types::{CandidateSource, CompletionScope, ScopedHistoryRecord};
use crate::models::quick_commands::CommandHistoryEntry;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct HistoryStatistics {
    pub use_count: u32,
    pub last_used_at_ms: u64,
}

#[derive(Default)]
struct Entry {
    normalized: String,
    prepared: Option<Vec<char>>,
    char_count: usize,
    global: Option<HistoryStatistics>,
    fallback: HistoryStatistics,
    legacy: Option<HistoryStatistics>,
    scopes: HashMap<CompletionScope, HistoryStatistics>,
    sources: u8,
}

impl Entry {
    fn refresh_statistics(&mut self) {
        self.fallback = HistoryStatistics::default();
        for stats in self.scopes.values().chain(self.legacy.iter()) {
            self.fallback.use_count = self.fallback.use_count.max(stats.use_count);
            self.fallback.last_used_at_ms =
                self.fallback.last_used_at_ms.max(stats.last_used_at_ms);
        }
        self.sources = if self.legacy.is_some() {
            1 << CandidateSource::LegacyHistory as u8
        } else {
            0
        };
        if !self.scopes.is_empty() || self.sources == 0 {
            self.sources |= 1 << CandidateSource::History as u8;
        }
    }
}

#[derive(Default)]
pub struct WholeCommandHistoryIndex {
    entries: HashMap<String, Entry>,
}

#[derive(Clone, Copy)]
pub struct HistorySearchEntry<'a> {
    pub command: &'a str,
    pub(crate) normalized: &'a str,
    pub(crate) prepared: Option<&'a [char]>,
    pub(crate) char_count: usize,
    pub statistics: HistoryStatistics,
    pub current_scope: bool,
    pub(crate) sources: u8,
}

impl std::fmt::Debug for HistorySearchEntry<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistorySearchEntry")
            .field("command", &"[REDACTED]")
            .field("statistics", &self.statistics)
            .field("current_scope", &self.current_scope)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for WholeCommandHistoryIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WholeCommandHistoryIndex")
            .field("command_count", &self.entries.len())
            .finish()
    }
}

pub(crate) fn normalized_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

impl WholeCommandHistoryIndex {
    /// Keep expensive matching representations when only statistics changed.
    pub fn reconcile(&mut self, global: &[CommandHistoryEntry], scoped: &[ScopedHistoryRecord]) {
        let present: HashSet<_> = global
            .iter()
            .map(|r| r.command.as_str())
            .chain(
                scoped
                    .iter()
                    .filter(|r| r.version == 1)
                    .map(|r| r.command.as_str()),
            )
            .collect();
        self.entries
            .retain(|command, _| present.contains(command.as_str()));
        for entry in self.entries.values_mut() {
            entry.global = None;
            entry.legacy = None;
            entry.scopes.clear();
        }
        for record in global {
            self.entry(&record.command).global = Some(HistoryStatistics {
                use_count: record.use_count,
                last_used_at_ms: record.last_used_at_ms,
            });
        }
        for record in scoped.iter().filter(|r| r.version == 1) {
            let entry = self.entry(&record.command);
            let stats = HistoryStatistics {
                use_count: record.use_count,
                last_used_at_ms: record.last_used_at_ms,
            };
            if record.scope == CompletionScope::legacy() {
                entry.legacy = Some(stats);
            } else {
                entry.scopes.insert(record.scope.clone(), stats);
            }
        }
        for entry in self.entries.values_mut() {
            entry.refresh_statistics();
        }
    }

    fn entry(&mut self, command: &str) -> &mut Entry {
        if !self.entries.contains_key(command) {
            self.entries.insert(
                command.into(),
                Entry {
                    normalized: normalized_text(command),
                    prepared: (!command.is_ascii()).then(|| matching_chars(command).collect()),
                    char_count: command.trim().chars().count(),
                    ..Entry::default()
                },
            );
        }
        self.entries.get_mut(command).expect("entry was inserted")
    }

    pub fn upsert_global(&mut self, record: &CommandHistoryEntry) {
        let entry = self.entry(&record.command);
        entry.global = Some(HistoryStatistics {
            use_count: record.use_count,
            last_used_at_ms: record.last_used_at_ms,
        });
        entry.refresh_statistics();
    }
    pub fn upsert_scoped(&mut self, record: &ScopedHistoryRecord) {
        if record.version != 1 {
            return;
        }
        let entry = self.entry(&record.command);
        let stats = HistoryStatistics {
            use_count: record.use_count,
            last_used_at_ms: record.last_used_at_ms,
        };
        if record.scope == CompletionScope::legacy() {
            entry.legacy = Some(stats);
        } else {
            entry.scopes.insert(record.scope.clone(), stats);
        }
        entry.refresh_statistics();
    }
    pub fn remove_global(&mut self, command: &str) {
        if let Some(entry) = self.entries.get_mut(command) {
            entry.global = None;
        }
        self.prune(command);
    }
    pub fn remove_scoped(&mut self, scope: &CompletionScope, command: &str) {
        if let Some(entry) = self.entries.get_mut(command) {
            if *scope == CompletionScope::legacy() {
                entry.legacy = None;
            } else {
                entry.scopes.remove(scope);
            }
        }
        self.prune(command);
    }
    fn prune(&mut self, command: &str) {
        if let Some(entry) = self.entries.get_mut(command) {
            entry.refresh_statistics();
            if entry.global.is_none() && entry.legacy.is_none() && entry.scopes.is_empty() {
                self.entries.remove(command);
            }
        }
    }

    pub fn iter<'a>(
        &'a self,
        scope: &'a CompletionScope,
    ) -> impl Iterator<Item = HistorySearchEntry<'a>> {
        self.entries
            .iter()
            .map(move |(command, entry)| Self::candidate(command, entry, scope))
    }

    pub fn get<'a>(
        &'a self,
        command: &str,
        scope: &CompletionScope,
    ) -> Option<HistorySearchEntry<'a>> {
        self.entries
            .get_key_value(command)
            .map(|(command, entry)| Self::candidate(command, entry, scope))
    }

    fn candidate<'a>(
        command: &'a str,
        entry: &'a Entry,
        scope: &CompletionScope,
    ) -> HistorySearchEntry<'a> {
        let local = entry.scopes.get(scope);
        HistorySearchEntry {
            command,
            normalized: &entry.normalized,
            prepared: entry.prepared.as_deref(),
            char_count: entry.char_count,
            statistics: local.copied().or(entry.global).unwrap_or(entry.fallback),
            current_scope: local.is_some(),
            sources: entry.sources,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::completion::types::{
        CandidateSource, CompletionScope, ScopedHistoryRecord, ShellDialect,
    };
    use crate::completion::whole_history::WholeCommandHistoryIndex;
    use crate::models::quick_commands::CommandHistoryEntry;
    use std::collections::BTreeMap;

    fn record(scope: CompletionScope, count: u32, time: u64) -> ScopedHistoryRecord {
        ScopedHistoryRecord {
            version: 1,
            scope,
            dialect: ShellDialect::Bash,
            command: "curl https://www.baidu.com".into(),
            use_count: count,
            last_used_at_ms: time,
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn merges_all_scopes_without_double_counting_broadcasts_or_assigning_legacy_membership() {
        let a = CompletionScope::new(&["a"]);
        let b = CompletionScope::new(&["b"]);
        let foreign = CompletionScope::new(&["foreign"]);
        let records = [
            record(a.clone(), 3, 30),
            record(b, 4, 40),
            record(CompletionScope::legacy(), 8, 10),
        ];
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(&[], &records);
        let entry = index.iter(&foreign).next().unwrap();
        assert!(!entry.current_scope);
        assert_eq!(entry.statistics.use_count, 8);
        assert_eq!(entry.statistics.last_used_at_ms, 40);
        assert_eq!(index.iter(&a).next().unwrap().statistics.use_count, 3);
        assert!(
            !index
                .iter(&CompletionScope::legacy())
                .next()
                .unwrap()
                .current_scope
        );
        assert_eq!(
            entry.sources,
            (1 << CandidateSource::History as u8) | (1 << CandidateSource::LegacyHistory as u8)
        );

        index.reconcile(
            &[CommandHistoryEntry {
                command: records[0].command.clone(),
                use_count: 12,
                last_used_at_ms: 50,
            }],
            &records,
        );
        let entry = index.iter(&foreign).next().unwrap();
        assert_eq!(entry.statistics.use_count, 12);
        assert_eq!(entry.statistics.last_used_at_ms, 50);
        assert_eq!(index.iter(&a).next().unwrap().statistics.use_count, 3);
        index.reconcile(&[], &[]);
        assert!(index.iter(&a).next().is_none());
    }

    #[test]
    fn global_only_history_is_searchable_without_environment_affinity_and_debug_is_redacted() {
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(
            &[CommandHistoryEntry {
                command: "fixture-only-global".into(),
                use_count: 2,
                last_used_at_ms: 1,
            }],
            &[],
        );
        let scope = CompletionScope::new(&["a"]);
        let entry = index.get("fixture-only-global", &scope).unwrap();
        assert!(!entry.current_scope);
        assert_eq!(entry.statistics.use_count, 2);
        assert!(!format!("{entry:?} {index:?}").contains("fixture-only-global"));
    }
    #[test]
    fn incremental_changes_match_snapshot_and_reuse_prepared_unicode() {
        let scope = CompletionScope::new(&["incremental"]);
        let foreign = CompletionScope::new(&["foreign"]);
        let global = CommandHistoryEntry {
            command: "fixture 工具".into(),
            use_count: 2,
            last_used_at_ms: 5,
        };
        let scoped = ScopedHistoryRecord {
            command: global.command.clone(),
            ..record(scope.clone(), 3, 10)
        };
        let legacy = ScopedHistoryRecord {
            command: global.command.clone(),
            ..record(CompletionScope::legacy(), 4, 12)
        };
        let mut incremental = WholeCommandHistoryIndex::default();
        incremental.upsert_global(&global);
        incremental.upsert_scoped(&scoped);
        incremental.upsert_scoped(&legacy);
        let prepared = incremental
            .get(&global.command, &scope)
            .unwrap()
            .prepared
            .unwrap()
            .as_ptr();
        let normalized = incremental
            .get(&global.command, &scope)
            .unwrap()
            .normalized
            .as_ptr();
        let mut snapshot = WholeCommandHistoryIndex::default();
        snapshot.reconcile(
            std::slice::from_ref(&global),
            &[scoped.clone(), legacy.clone()],
        );
        for context in [&scope, &foreign, &CompletionScope::legacy()] {
            let a = incremental.get(&global.command, context).unwrap();
            let b = snapshot.get(&global.command, context).unwrap();
            assert_eq!(
                (a.statistics, a.current_scope, a.sources),
                (b.statistics, b.current_scope, b.sources)
            );
        }
        incremental.reconcile(std::slice::from_ref(&global), &[scoped.clone(), legacy]);
        let entry = incremental.get(&global.command, &scope).unwrap();
        assert_eq!(entry.prepared.unwrap().as_ptr(), prepared);
        assert_eq!(entry.normalized.as_ptr(), normalized);
        incremental.remove_global(&global.command);
        incremental.remove_scoped(&CompletionScope::legacy(), &global.command);
        snapshot.reconcile(&[], std::slice::from_ref(&scoped));
        assert_eq!(
            incremental
                .get(&global.command, &foreign)
                .unwrap()
                .statistics,
            snapshot.get(&global.command, &foreign).unwrap().statistics
        );
        incremental.remove_scoped(&scope, &global.command);
        assert!(incremental.get(&global.command, &scope).is_none());
    }
}
