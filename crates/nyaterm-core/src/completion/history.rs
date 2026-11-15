use crate::completion::parser::{unwrap_words, words};
use crate::completion::privacy::{command_may_be_learned, safe_word};
use crate::completion::types::{CompletionScope, ScopedHistoryRecord, ShellDialect};
use std::collections::{BTreeMap, HashMap, HashSet};

type ContextKey = (CompletionScope, ShellDialect, Vec<String>);
#[derive(Default)]
struct Contributions {
    commands: HashMap<String, (u32, u64)>,
    count: u64,
    times: BTreeMap<u64, usize>,
}
impl Contributions {
    fn insert(&mut self, command: String, stats: (u32, u64)) {
        self.remove(&command);
        self.commands.insert(command, stats);
        self.count += u64::from(stats.0);
        *self.times.entry(stats.1).or_default() += 1;
    }
    fn remove(&mut self, command: &str) {
        if let Some((count, time)) = self.commands.remove(command) {
            self.count -= u64::from(count);
            if let Some(occurrences) = self.times.get_mut(&time) {
                *occurrences -= 1;
                if *occurrences == 0 {
                    self.times.remove(&time);
                }
            }
        }
    }
}

#[derive(Default)]
pub struct TokenHistoryIndex {
    records: HashMap<(CompletionScope, String), ScopedHistoryRecord>,
    buckets: HashMap<ContextKey, HashMap<String, Contributions>>,
}

pub(crate) fn normalized(mut preceding: Vec<String>, dialect: ShellDialect) -> Vec<String> {
    let mut stopped = false;
    let native = preceding
        .first()
        .is_some_and(|root| dialect.native_options(root));
    preceding = preceding
        .into_iter()
        .flat_map(|token| {
            if token == "--" {
                stopped = true;
            }
            let pair = token
                .split_once('=')
                .filter(|(flag, _)| !stopped && flag.starts_with("--"))
                .or_else(|| {
                    (!stopped
                        && native
                        && dialect == ShellDialect::PowerShell
                        && token.starts_with('-'))
                    .then(|| token.split_once(':'))
                    .flatten()
                });
            if let Some((flag, value)) = pair {
                vec![flag.into(), value.into()]
            } else {
                vec![token]
            }
        })
        .collect();
    if preceding
        .first()
        .is_some_and(|root| dialect.native_options(root))
    {
        let mut stopped = false;
        for token in preceding.iter_mut().skip(1) {
            if token == "--" {
                stopped = true;
            }
            if !stopped
                && (token.starts_with('-')
                    || (dialect == ShellDialect::Cmd && token.starts_with('/') && token.len() == 2))
            {
                *token = token.to_ascii_lowercase();
            }
        }
    }
    if dialect.case_insensitive_commands()
        && let Some(first) = preceding.first_mut()
    {
        *first = first.to_lowercase();
    }
    preceding
}

pub(crate) fn transitions(command: &str, dialect: ShellDialect) -> Vec<(Vec<String>, String)> {
    let Some(words) = words(command, dialect) else {
        return Vec::new();
    };
    let mut result = HashSet::new();
    let mut segment = 0;
    let mut line = Vec::new();
    let add = |line: &mut Vec<String>, result: &mut HashSet<(Vec<String>, String)>| {
        if unwrap_words(line, dialect).is_none() {
            return;
        }
        let mut preceding: Vec<String> = Vec::new();
        let mut stopped = false;
        for word in line.drain(..) {
            if word == "--" {
                stopped = true;
            }
            if let Some((flag, value)) = word
                .split_once('=')
                .filter(|(flag, _)| !stopped && flag.starts_with("--"))
                .or_else(|| {
                    (!stopped
                        && dialect == ShellDialect::PowerShell
                        && word.starts_with('-')
                        && preceding
                            .first()
                            .is_some_and(|root| dialect.native_options(root)))
                    .then(|| word.split_once(':'))
                    .flatten()
                })
            {
                if safe_word(flag, dialect) {
                    result.insert((normalized(preceding.clone(), dialect), flag.into()));
                }
                preceding.push(flag.into());
                if safe_word(value, dialect) {
                    result.insert((normalized(preceding.clone(), dialect), value.into()));
                }
                preceding.push(value.into());
            } else {
                if safe_word(&word, dialect) {
                    result.insert((normalized(preceding.clone(), dialect), word.clone()));
                }
                preceding.push(word);
            }
        }
    };
    for word in words {
        if word.segment != segment {
            add(&mut line, &mut result);
            segment = word.segment;
        }
        if !word.text.is_empty() || word.quoted {
            line.push(word.text);
        }
    }
    add(&mut line, &mut result);
    result.into_iter().collect()
}

impl TokenHistoryIndex {
    pub fn upsert(&mut self, record: ScopedHistoryRecord) {
        self.remove_record(&record.scope, &record.command);
        if record.version != 1 {
            return;
        }
        if record.scope != CompletionScope::legacy()
            && command_may_be_learned(&record.command, false)
        {
            for (context, token) in transitions(&record.command, record.dialect) {
                self.buckets
                    .entry((record.scope.clone(), record.dialect, context))
                    .or_default()
                    .entry(token)
                    .or_default()
                    .insert(
                        record.command.clone(),
                        (record.use_count, record.last_used_at_ms),
                    );
            }
        }
        self.records
            .insert((record.scope.clone(), record.command.clone()), record);
    }
    fn remove_record(&mut self, scope: &CompletionScope, command: &str) {
        if let Some(old) = self.records.remove(&(scope.clone(), command.into())) {
            for (context, token) in transitions(&old.command, old.dialect) {
                let key = (scope.clone(), old.dialect, context);
                if let Some(bucket) = self.buckets.get_mut(&key) {
                    if let Some(contributions) = bucket.get_mut(&token) {
                        contributions.remove(command);
                        if contributions.commands.is_empty() {
                            bucket.remove(&token);
                        }
                    }
                    if bucket.is_empty() {
                        self.buckets.remove(&key);
                    }
                }
            }
        }
    }
    /// Shared-store refreshes apply only changed contributions, never rebuild
    /// token buckets for unchanged records.
    pub fn reconcile(&mut self, records: Vec<ScopedHistoryRecord>) {
        let keys: HashSet<_> = records
            .iter()
            .map(|r| (r.scope.clone(), r.command.clone()))
            .collect();
        let removed: Vec<_> = self
            .records
            .keys()
            .filter(|key| !keys.contains(*key))
            .cloned()
            .collect();
        for (scope, command) in removed {
            self.remove_record(&scope, &command);
        }
        for record in records {
            if self
                .records
                .get(&(record.scope.clone(), record.command.clone()))
                != Some(&record)
            {
                self.upsert(record);
            }
        }
    }
    pub fn remove_scoped(&mut self, scope: &CompletionScope, command: &str) {
        self.remove_record(scope, command);
    }
    pub fn remove_command(&mut self, command: &str) {
        let scopes: Vec<_> = self
            .records
            .keys()
            .filter(|(_, c)| c == command)
            .map(|(scope, _)| scope.clone())
            .collect();
        for scope in scopes {
            self.remove_record(&scope, command);
        }
    }
    pub fn candidates(
        &self,
        scope: &CompletionScope,
        preceding: &[String],
        dialect: ShellDialect,
    ) -> Vec<(String, u32, u64)> {
        self.candidate_iter(scope, preceding, dialect)
            .map(|(token, count, time)| (token.to_string(), count, time))
            .collect()
    }
    pub fn candidate_iter<'a>(
        &'a self,
        scope: &CompletionScope,
        preceding: &[String],
        dialect: ShellDialect,
    ) -> impl Iterator<Item = (&'a str, u32, u64)> {
        self.buckets
            .get(&(
                scope.clone(),
                dialect,
                normalized(preceding.to_vec(), dialect),
            ))
            .into_iter()
            .flat_map(|bucket| bucket.iter())
            .map(|(token, values)| {
                (
                    token.as_str(),
                    values.count.min(u64::from(u32::MAX)) as u32,
                    values.times.last_key_value().map_or(0, |(time, _)| *time),
                )
            })
    }
    pub fn records_for(&self, scope: &CompletionScope) -> Vec<&ScopedHistoryRecord> {
        let mut records: Vec<_> = self
            .records
            .values()
            .filter(|r| r.scope == *scope || r.scope == CompletionScope::legacy())
            .collect();
        records.sort_by(|a, b| {
            b.last_used_at_ms
                .cmp(&a.last_used_at_ms)
                .then(a.command.cmp(&b.command))
        });
        records
    }
}

#[cfg(test)]
mod tests {
    use crate::completion::history::TokenHistoryIndex;
    use crate::completion::types::{CompletionScope, ScopedHistoryRecord, ShellDialect};
    use std::collections::BTreeMap;
    fn record(scope: CompletionScope, command: &str, count: u32, time: u64) -> ScopedHistoryRecord {
        ScopedHistoryRecord {
            version: 1,
            scope,
            dialect: ShellDialect::Bash,
            command: command.into(),
            use_count: count,
            last_used_at_ms: time,
            extensions: BTreeMap::new(),
        }
    }
    #[test]
    fn incremental_updates_and_deletion_recompute_frequency_and_recency() {
        let scope = CompletionScope::new(&["one"]);
        let mut index = TokenHistoryIndex::default();
        index.upsert(record(scope.clone(), "tool run one", 3, 10));
        index.upsert(record(scope.clone(), "tool run two", 4, 20));
        index.upsert(record(scope.clone(), "tool run one", 5, 30));
        assert_eq!(
            index.candidates(&scope, &["tool".into()], ShellDialect::Bash),
            [("run".into(), 9, 30)]
        );
        index.remove_command("tool run one");
        assert_eq!(
            index.candidates(&scope, &["tool".into()], ShellDialect::Bash),
            [("run".into(), 4, 20)]
        );
        index.reconcile(vec![]);
        assert!(
            index
                .candidates(&scope, &["tool".into()], ShellDialect::Bash)
                .is_empty()
        );
    }
    #[test]
    fn old_global_history_never_seeds_host_tokens() {
        let mut index = TokenHistoryIndex::default();
        index.upsert(record(CompletionScope::legacy(), "private-tool arg", 2, 1));
        assert!(
            index
                .candidates(&CompletionScope::legacy(), &[], ShellDialect::Bash)
                .is_empty()
        );
        assert_eq!(index.records_for(&CompletionScope::new(&["host"])).len(), 1);
    }

    #[test]
    fn saturated_counts_remain_exact_after_removing_a_contributor_and_dialects_are_isolated() {
        let scope = CompletionScope::new(&["isolated"]);
        let mut index = TokenHistoryIndex::default();
        index.upsert(record(scope.clone(), "tool run one", u32::MAX, 20));
        index.upsert(record(scope.clone(), "tool run two", 5, 30));
        assert_eq!(
            index.candidates(&scope, &["tool".into()], ShellDialect::Bash),
            [("run".into(), u32::MAX, 30)]
        );
        assert!(
            index
                .candidates(&scope, &["tool".into()], ShellDialect::PowerShell)
                .is_empty()
        );
        index.remove_command("tool run one");
        assert_eq!(
            index.candidates(&scope, &["tool".into()], ShellDialect::Bash),
            [("run".into(), 5, 30)]
        );
        let mut updated = record(scope.clone(), "tool run two", 7, 40);
        updated.dialect = ShellDialect::PowerShell;
        index.upsert(updated);
        assert!(
            index
                .candidates(&scope, &["tool".into()], ShellDialect::Bash)
                .is_empty()
        );
        assert_eq!(
            index.candidates(&scope, &["tool".into()], ShellDialect::PowerShell),
            [("run".into(), 7, 40)]
        );
    }
}
