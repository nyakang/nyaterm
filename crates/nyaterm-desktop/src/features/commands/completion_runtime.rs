//! A single background owner for the catalog, token index and scoped mutations.
use crate::features::runtime_jobs::CommandPersistenceResult;
use futures::channel::{mpsc::UnboundedSender, oneshot};
use nyaterm_core::command_search::{
    CommandSourceQuery, manual_empty_command_suggestions, search_command_sources_using,
};
use nyaterm_core::completion::history::TokenHistoryIndex;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionMode, CompletionResult,
    CompletionScope, ScopedHistoryRecord, ShellCommandContext,
};
use nyaterm_core::completion::{CompletionEngine, TokenCompletionRequest};
use nyaterm_core::models::quick_commands::{CommandHistoryEntry, QuickCommand};
use nyaterm_core::terminal::editing::supports_scalar_editing;
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};

#[derive(Clone)]
pub(in crate::features) struct CompletionRuntime {
    tx: SyncSender<Request>,
    epoch: Arc<AtomicU64>,
    dirty: Arc<AtomicBool>,
    shared_generation: Arc<OnceLock<Arc<AtomicU64>>>,
}

pub(in crate::features) struct CompletionQuery {
    pub context: ShellCommandContext,
    pub input: String,
    pub mode: CompletionMode,
    pub quick: Arc<[QuickCommand]>,
    pub min: usize,
    pub max: usize,
}

enum Request {
    Observe(ShellCommandContext, String, bool),
    Search(CompletionQuery, u64, oneshot::Sender<CompletionResult>),
    Delete(
        String,
        oneshot::Sender<Result<Vec<CommandHistoryEntry>, String>>,
    ),
    Reload,
}

impl CompletionRuntime {
    pub(super) fn new(
        store: StoreBlockingClient,
        events: UnboundedSender<CommandPersistenceResult>,
    ) -> Self {
        let (tx, rx) = sync_channel(64);
        let epoch = Arc::new(AtomicU64::new(0));
        let dirty = Arc::new(AtomicBool::new(false));
        let worker_dirty = dirty.clone();
        let shared_generation = Arc::new(OnceLock::<Arc<AtomicU64>>::new());
        let worker_generation = shared_generation.clone();
        let spawn =
            nyaterm_core::runtime::worker::spawn("command-completion".to_string(), move || {
                if let Ok(generation) =
                    store.request_fn(StoreDomain::Commands, |db| Ok(db.completion_generation()))
                {
                    let _ = worker_generation.set(generation);
                }
                let generation = || {
                    worker_generation
                        .get()
                        .map_or(0, |value| value.load(Ordering::Acquire))
                };
                let mut loaded_generation = generation();
                let mut engine = CompletionEngine::embedded();
                let mut whole_matcher = TextMatcher::default();
                let mut history = TokenHistoryIndex::default();
                let mut quick_index = TokenHistoryIndex::default();
                let mut quick_key: Option<(CompletionScope, Arc<[QuickCommand]>)> = None;
                let reload = |history: &mut TokenHistoryIndex| -> Result<(), String> {
                    let (_, records) = store
                        .request_fn(StoreDomain::Commands, |database| {
                            database.list_completion_history()
                        })
                        .map_err(|e| e.to_string())?;
                    history.reconcile(records);
                    Ok(())
                };
                let mut loaded = reload(&mut history).is_ok();
                if !loaded {
                    history = TokenHistoryIndex::default();
                }
                while let Ok(request) = rx.recv() {
                    match request {
                        Request::Observe(context, command, include_global) => {
                            let scope = context.scope.clone();
                            let result = store.request_fn(StoreDomain::Commands, move |database| {
                                let before =
                                    database.completion_generation().load(Ordering::Acquire);
                                let update = database.append_completion_contribution(
                                    &context.scope,
                                    context.dialect,
                                    &command,
                                    include_global,
                                )?;
                                Ok((
                                    update,
                                    database.list_command_history(64)?,
                                    before,
                                    database.completion_generation().load(Ordering::Acquire),
                                ))
                            });
                            let outcome = match result {
                                Ok((Some((_, record, evicted)), global, before, after)) => {
                                    history.upsert(record);
                                    for command in evicted {
                                        history.remove_scoped(&scope, &command);
                                    }
                                    // Our contribution is already applied. Advance only if
                                    // no other window changed the snapshot before this write.
                                    if loaded
                                        && loaded_generation == before
                                        && after == before.wrapping_add(1)
                                    {
                                        loaded_generation = after;
                                    }
                                    Ok(global)
                                }
                                Ok((None, global, _, _)) => Ok(global),
                                Err(error) => Err(error.to_string()),
                            };
                            let _ =
                                events.unbounded_send(CommandPersistenceResult::History(outcome));
                        }
                        Request::Search(query, revision, response) => {
                            if worker_dirty.swap(false, Ordering::AcqRel)
                                || !loaded
                                || loaded_generation != generation()
                            {
                                loaded_generation = generation();
                                loaded = reload(&mut history).is_ok();
                                if !loaded {
                                    history = TokenHistoryIndex::default();
                                }
                            }
                            let candidates = match query.mode {
                                CompletionMode::Token => {
                                    let rebuild =
                                        quick_key.as_ref().is_none_or(|(scope, snapshot)| {
                                            scope != &query.context.scope
                                                || !Arc::ptr_eq(snapshot, &query.quick)
                                        });
                                    if rebuild {
                                        quick_index = TokenHistoryIndex::default();
                                        for command in query.quick.iter() {
                                            quick_index.upsert(ScopedHistoryRecord {
                                                version: 1,
                                                scope: query.context.scope.clone(),
                                                dialect: query.context.dialect,
                                                command: command.command.clone(),
                                                use_count: 0,
                                                last_used_at_ms: 0,
                                                extensions: BTreeMap::new(),
                                            });
                                        }
                                        quick_key = Some((
                                            query.context.scope.clone(),
                                            query.quick.clone(),
                                        ));
                                    }
                                    engine.complete(TokenCompletionRequest {
                                        input: &query.input,
                                        cursor: query.input.len(),
                                        dialect: query.context.dialect,
                                        scope: query.context.scope,
                                        history: &history,
                                        quick: &quick_index,
                                        limit: 12,
                                    })
                                }
                                CompletionMode::History => {
                                    full_history(&history, &query, &mut whole_matcher)
                                }
                            };
                            let _ = response.send(CompletionResult {
                                candidates,
                                revision: revision.wrapping_add(loaded_generation),
                            });
                        }
                        Request::Delete(command, response) => {
                            let persisted = command.clone();
                            let outcome = store
                                .request_fn(StoreDomain::Commands, move |database| {
                                    let before =
                                        database.completion_generation().load(Ordering::Acquire);
                                    database.delete_command_history(&persisted)?;
                                    Ok((
                                        database.list_command_history(64)?,
                                        before,
                                        database.completion_generation().load(Ordering::Acquire),
                                    ))
                                })
                                .map(|(global, before, after)| {
                                    history.remove_command(&command);
                                    if loaded
                                        && loaded_generation == before
                                        && after == before.wrapping_add(1)
                                    {
                                        loaded_generation = after;
                                    }
                                    global
                                })
                                .map_err(|e| e.to_string());
                            let _ = response.send(outcome);
                        }
                        Request::Reload => {
                            loaded_generation = generation();
                            loaded = reload(&mut history).is_ok();
                            if !loaded {
                                history = TokenHistoryIndex::default();
                            }
                            quick_key = None;
                        }
                    }
                }
            });
        // Dropping the receiver on spawn failure makes every try_send fail closed.
        drop(spawn);
        Self {
            tx,
            epoch,
            dirty,
            shared_generation,
        }
    }
    pub(in crate::features) fn revision(&self) -> u64 {
        self.epoch.load(Ordering::Acquire).wrapping_add(
            self.shared_generation
                .get()
                .map_or(0, |value| value.load(Ordering::Acquire)),
        )
    }
    pub(in crate::features) fn observe(
        &self,
        context: ShellCommandContext,
        command: String,
    ) -> bool {
        self.observe_submission(context, command, true)
    }
    pub(in crate::features) fn observe_submission(
        &self,
        context: ShellCommandContext,
        command: String,
        include_global: bool,
    ) -> bool {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.tx
            .try_send(Request::Observe(context, command, include_global))
            .is_ok()
    }
    pub(in crate::features) fn reload(&self) {
        self.dirty.store(true, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.try_send(Request::Reload);
    }
    pub(in crate::features) async fn search(&self, query: CompletionQuery) -> CompletionResult {
        let revision = self.epoch.load(Ordering::Acquire);
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .try_send(Request::Search(query, revision, tx))
            .is_err()
        {
            return CompletionResult {
                candidates: Vec::new(),
                revision,
            };
        }
        rx.await.unwrap_or_default()
    }
    pub(in crate::features) async fn delete(
        &self,
        command: String,
    ) -> Result<Vec<CommandHistoryEntry>, String> {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Request::Delete(command, tx))
            .map_err(|_| "Completion worker is unavailable".to_string())?;
        rx.await
            .map_err(|_| "Completion worker disconnected".to_string())?
    }
}

fn full_history(
    index: &TokenHistoryIndex,
    query: &CompletionQuery,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    let records = index.records_for(&query.context.scope);
    let mut origins = HashMap::new();
    for record in &records {
        let legacy = record.scope == CompletionScope::legacy();
        origins
            .entry(record.command.as_str())
            .and_modify(|value| *value &= legacy)
            .or_insert(legacy);
    }
    let entries: Vec<_> = records
        .iter()
        .map(|r| CommandHistoryEntry {
            command: r.command.clone(),
            last_used_at_ms: r.last_used_at_ms,
            use_count: r.use_count,
        })
        .collect();
    let results = if query.input.trim().is_empty() {
        manual_empty_command_suggestions(
            &entries,
            &query.quick,
            12,
            Some(query.min),
            Some(query.max),
        )
    } else {
        search_command_sources_using(
            matcher,
            CommandSourceQuery {
                history: &entries,
                quick_commands: &query.quick,
                pattern: &query.input,
                limit: 12,
                min_history_command_length: Some(query.min),
                max_history_command_length: Some(query.max),
            },
        )
    };
    results
        .into_iter()
        .map(|item| {
            let (kind, source) = if item.source == "quickCommand" {
                (CandidateKind::QuickCommand, CandidateSource::QuickCommand)
            } else if origins.get(item.command.as_str()) == Some(&true) {
                (CandidateKind::History, CandidateSource::LegacyHistory)
            } else {
                (CandidateKind::History, CandidateSource::History)
            };
            let fillable =
                item.command.starts_with(&query.input) && supports_scalar_editing(&item.command);
            let matching: Vec<_> = records
                .iter()
                .filter(|record| record.command == item.command)
                .collect();
            let stats = matching
                .iter()
                .find(|record| record.scope != CompletionScope::legacy())
                .or_else(|| matching.first());
            let mut sources = vec![source];
            if matching
                .iter()
                .any(|record| record.scope != CompletionScope::legacy())
            {
                sources.push(CandidateSource::History);
            }
            if matching
                .iter()
                .any(|record| record.scope == CompletionScope::legacy())
            {
                sources.push(CandidateSource::LegacyHistory);
            }
            if query
                .quick
                .iter()
                .any(|command| command.command == item.command)
            {
                sources.push(CandidateSource::QuickCommand);
            }
            sources.sort_unstable();
            sources.dedup();
            CompletionCandidate {
                display: item.display,
                insert_text: item.command,
                description: String::new(),
                range: 0..query.input.len(),
                kind,
                sources,
                use_count: stats.map_or(0, |record| record.use_count),
                last_used_at_ms: stats.map_or(0, |record| record.last_used_at_ms),
                score: item.score,
                indices: item.indices,
                fillable,
                context_priority: 0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::features::commands::completion_runtime::{
        CompletionQuery, CompletionRuntime, full_history,
    };
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use futures::channel::mpsc::unbounded;
    use nyaterm_core::completion::types::{
        CandidateSource, CompletionMode, CompletionScope, ScopedHistoryRecord, ShellCommandContext,
        ShellDialect,
    };
    use std::sync::Arc;
    #[test]
    fn full_history_merges_origins_and_uses_current_scope_statistics_once() {
        use nyaterm_core::completion::history::TokenHistoryIndex;
        use nyaterm_core::completion::matching::TextMatcher;
        use nyaterm_core::models::quick_commands::QuickCommand;
        use std::collections::BTreeMap;
        let scope = CompletionScope::new(&["history-origin-fixture"]);
        let mut index = TokenHistoryIndex::default();
        for (scope, count, time) in [(scope.clone(), 8, 2), (CompletionScope::legacy(), 10, 1)] {
            index.upsert(ScopedHistoryRecord {
                version: 1,
                scope,
                dialect: ShellDialect::Bash,
                command: "tool run".into(),
                use_count: count,
                last_used_at_ms: time,
                extensions: BTreeMap::new(),
            });
        }
        let quick: QuickCommand = serde_json::from_value(serde_json::json!({
            "id":"origin-fixture", "label":"tool run", "command":"tool run", "use_count":99
        }))
        .unwrap();
        let query = CompletionQuery {
            context: ShellCommandContext {
                scope,
                dialect: ShellDialect::Bash,
                command_events: true,
            },
            input: "tool".into(),
            mode: CompletionMode::History,
            quick: Arc::from([quick]),
            min: 1,
            max: 100,
        };
        let candidates = full_history(&index, &query, &mut TextMatcher::default());
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].use_count, 8);
        assert_eq!(candidates[0].last_used_at_ms, 2);
        assert_eq!(
            candidates[0].sources,
            [
                CandidateSource::History,
                CandidateSource::QuickCommand,
                CandidateSource::LegacyHistory
            ]
        );
    }
    #[test]
    fn worker_orders_observation_deletion_and_restore_without_resurrecting_tokens() {
        let dir = TestConfigDir::new("nyaterm-completion-worker");
        let store = blocking_test_store(dir.path());
        let (tx, _rx) = unbounded();
        let runtime = CompletionRuntime::new(store.clone(), tx.clone());
        let second = CompletionRuntime::new(store.clone(), tx);
        let context = ShellCommandContext {
            scope: CompletionScope::new(&["one"]),
            dialect: ShellDialect::Bash,
            command_events: true,
        };
        let query = |context| CompletionQuery {
            context,
            input: "internal-t".into(),
            mode: CompletionMode::Token,
            quick: Arc::from([]),
            min: 1,
            max: 100,
        };
        let revision = runtime.revision();
        assert!(runtime.observe(context.clone(), "internal-tool deploy".into()));
        assert_ne!(revision, runtime.revision());
        assert!(
            futures::executor::block_on(runtime.search(query(context.clone())))
                .candidates
                .iter()
                .any(|r| r.insert_text == "internal-tool")
        );
        let other = ShellCommandContext {
            scope: CompletionScope::new(&["two"]),
            ..context.clone()
        };
        assert!(
            futures::executor::block_on(runtime.search(query(other)))
                .candidates
                .is_empty()
        );
        let before = futures::executor::block_on(second.search(query(context.clone())));
        assert!(!before.candidates.is_empty());
        assert_eq!(before.revision, second.revision());
        futures::executor::block_on(runtime.delete("internal-tool deploy".into())).unwrap();
        assert_ne!(before.revision, second.revision());
        assert!(
            futures::executor::block_on(second.search(query(context.clone())))
                .candidates
                .is_empty()
        );
        assert!(
            futures::executor::block_on(runtime.search(query(context.clone())))
                .candidates
                .is_empty()
        );
        assert!(runtime.observe(context.clone(), "internal-tool deploy".into()));
        assert!(
            !futures::executor::block_on(runtime.search(query(context.clone())))
                .candidates
                .is_empty()
        );
        store
            .request_fn(nyaterm_store::StoreDomain::Commands, |db| {
                db.replace_command_history(&[])
            })
            .unwrap();
        runtime.reload();
        assert!(
            futures::executor::block_on(runtime.search(query(context)))
                .candidates
                .is_empty()
        );
    }
}
