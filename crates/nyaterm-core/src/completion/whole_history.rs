//! Derived whole-command catalog. Global history never seeds token learning.
use crate::completion::matching::matching_chars;
use crate::completion::types::{CandidateSource, CompletionScope, ScopedHistoryRecord};
use crate::models::quick_commands::CommandHistoryEntry;
use std::collections::HashMap;

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
    scopes: HashMap<CompletionScope, HistoryStatistics>,
    sources: u8,
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
    /// Rebuild only when the persisted snapshot changes, not on each keystroke.
    pub fn reconcile(&mut self, global: &[CommandHistoryEntry], scoped: &[ScopedHistoryRecord]) {
        let mut entries = HashMap::<String, Entry>::new();
        for record in global {
            entries.entry(record.command.clone()).or_default().global = Some(HistoryStatistics {
                use_count: record.use_count,
                last_used_at_ms: record.last_used_at_ms,
            });
        }
        for record in scoped.iter().filter(|record| record.version == 1) {
            let entry = entries.entry(record.command.clone()).or_default();
            // A broadcast contributes to multiple scopes but only once globally.
            entry.fallback.use_count = entry.fallback.use_count.max(record.use_count);
            entry.fallback.last_used_at_ms =
                entry.fallback.last_used_at_ms.max(record.last_used_at_ms);
            if record.scope == CompletionScope::legacy() {
                entry.sources |= 1 << CandidateSource::LegacyHistory as u8;
            } else {
                entry.sources |= 1 << CandidateSource::History as u8;
                entry.scopes.insert(
                    record.scope.clone(),
                    HistoryStatistics {
                        use_count: record.use_count,
                        last_used_at_ms: record.last_used_at_ms,
                    },
                );
            }
        }
        for (command, entry) in &mut entries {
            entry.normalized = normalized_text(command);
            entry.prepared = (!command.is_ascii()).then(|| matching_chars(command).collect());
            entry.char_count = command.trim().chars().count();
            if entry.sources == 0 {
                entry.sources = 1 << CandidateSource::History as u8;
            }
        }
        self.entries = entries;
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
}
