//! A single background owner for the catalog, token index and scoped mutations.
use crate::features::runtime_jobs::CommandPersistenceResult;
use futures::channel::{mpsc::UnboundedSender, oneshot};
use nyaterm_core::command_search::{RankedCommandSourceQuery, search_ranked_command_sources_using};
use nyaterm_core::completion::history::TokenHistoryIndex;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::types::{
    CompletionCandidate, CompletionFillPolicy, CompletionMode, CompletionResult,
    ShellCommandContext,
};
use nyaterm_core::completion::whole_history::WholeCommandHistoryIndex;
use nyaterm_core::completion::{CompletionEngine, TokenCompletionRequest};
use nyaterm_core::models::quick_commands::{CommandHistoryEntry, QuickCommand};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use std::collections::HashMap;
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
    pub fill_policy: Option<CompletionFillPolicy>,
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
                let mut engine = CompletionEngine::embedded();
                let mut whole_matcher = TextMatcher::default();
                let mut history = TokenHistoryIndex::default();
                let mut whole_history = WholeCommandHistoryIndex::default();
                let mut loaded_generation =
                    reload_history(&store, &mut history, &mut whole_history).ok();
                while let Ok(request) = rx.recv() {
                    match request {
                        Request::Observe(context, command, include_global) => {
                            let outcome = store
                                .request_fn(StoreDomain::Commands, move |database| {
                                    database.append_completion_contribution(
                                        &context.scope,
                                        context.dialect,
                                        &command,
                                        include_global,
                                    )?;
                                    database.list_command_history(64)
                                })
                                .map_err(|error| error.to_string());
                            // Keep the previous generation until both indexes are refreshed.
                            let _ =
                                events.unbounded_send(CommandPersistenceResult::History(outcome));
                        }
                        Request::Search(query, revision, response) => {
                            if worker_dirty.swap(false, Ordering::AcqRel)
                                || loaded_generation != Some(generation())
                            {
                                loaded_generation =
                                    reload_history(&store, &mut history, &mut whole_history).ok();
                                if loaded_generation.is_none() {
                                    history = TokenHistoryIndex::default();
                                    whole_history = WholeCommandHistoryIndex::default();
                                }
                            }
                            let candidates = search_candidates(
                                &mut engine,
                                &history,
                                &whole_history,
                                &query,
                                &mut whole_matcher,
                            );
                            let _ = response.send(CompletionResult {
                                candidates,
                                revision: revision
                                    .wrapping_add(loaded_generation.unwrap_or_else(generation)),
                            });
                        }
                        Request::Delete(command, response) => {
                            let outcome = store
                                .request_fn(StoreDomain::Commands, move |database| {
                                    database.delete_command_history(&command)?;
                                    database.list_command_history(64)
                                })
                                .map_err(|error| error.to_string());
                            let _ = response.send(outcome);
                        }
                        Request::Reload => {
                            loaded_generation =
                                reload_history(&store, &mut history, &mut whole_history).ok();
                            if loaded_generation.is_none() {
                                history = TokenHistoryIndex::default();
                                whole_history = WholeCommandHistoryIndex::default();
                            }
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

fn reload_history(
    store: &StoreBlockingClient,
    tokens: &mut TokenHistoryIndex,
    whole: &mut WholeCommandHistoryIndex,
) -> Result<u64, String> {
    let (snapshot, generation) = store
        .request_fn(StoreDomain::Commands, |database| {
            // Capture before opening the snapshot; concurrent later writes cause another refresh.
            let generation = database.completion_generation().load(Ordering::Acquire);
            Ok((database.completion_history_snapshot()?, generation))
        })
        .map_err(|error| error.to_string())?;
    whole.reconcile(&snapshot.global_history, &snapshot.scoped_history);
    tokens.reconcile(snapshot.scoped_history);
    Ok(generation)
}

fn search_candidates(
    engine: &mut CompletionEngine,
    history: &TokenHistoryIndex,
    whole: &WholeCommandHistoryIndex,
    query: &CompletionQuery,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    let mut candidates = full_history(whole, query, matcher);
    if query.mode == CompletionMode::History {
        return candidates;
    }
    // Whole commands retain their display, kind and statistics when a token
    // provider reaches the same target. Match scores from the two queries are
    // not comparable, so complete commands precede the contextual token group.
    let mut targets: HashMap<_, _> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, item)| item.target_line(&query.input).map(|line| (line, index)))
        .collect();
    let tokens = engine.complete(TokenCompletionRequest {
        fill_policy: query.fill_policy,
        input: &query.input,
        cursor: query.input.len(),
        dialect: query.context.dialect,
        scope: query.context.scope.clone(),
        history,
        quick: &TokenHistoryIndex::default(),
        limit: 12,
    });
    for token in tokens {
        let Some(target) = token.target_line(&query.input) else {
            continue;
        };
        if let Some(&index) = targets.get(&target) {
            let sources = &mut candidates[index].sources;
            sources.extend(token.sources);
            sources.sort_unstable();
            sources.dedup();
        } else {
            targets.insert(target, candidates.len());
            candidates.push(token);
        }
    }
    candidates.truncate(12);
    candidates
}

fn full_history(
    index: &WholeCommandHistoryIndex,
    query: &CompletionQuery,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    search_ranked_command_sources_using(
        matcher,
        RankedCommandSourceQuery {
            fill_policy: query.fill_policy,
            history: index,
            scope: &query.context.scope,
            quick_commands: &query.quick,
            pattern: &query.input,
            limit: 12,
            min_history_command_length: Some(query.min),
            max_history_command_length: Some(query.max),
            now_ms: u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::features::commands::completion_runtime::{
        CompletionQuery, CompletionRuntime, full_history as search_whole,
        search_candidates as search_with_indexes,
    };
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use futures::channel::mpsc::unbounded;
    use nyaterm_core::completion::types::{
        CandidateKind, CandidateSource, CompletionMode, CompletionScope, ScopedHistoryRecord,
        ShellCommandContext, ShellDialect,
    };
    use std::sync::Arc;

    use nyaterm_core::completion::CompletionEngine;
    use nyaterm_core::completion::history::TokenHistoryIndex;
    use nyaterm_core::completion::matching::TextMatcher;
    use nyaterm_core::completion::types::CompletionCandidate;
    use nyaterm_core::completion::whole_history::WholeCommandHistoryIndex;

    #[derive(Default)]
    struct HistoryFixture {
        tokens: TokenHistoryIndex,
        whole: WholeCommandHistoryIndex,
        records: Vec<ScopedHistoryRecord>,
    }

    impl HistoryFixture {
        fn upsert(&mut self, record: ScopedHistoryRecord) {
            self.tokens.upsert(record.clone());
            self.records.push(record);
            self.whole.reconcile(&[], &self.records);
        }
    }

    fn search_candidates(
        engine: &mut CompletionEngine,
        index: &HistoryFixture,
        query: &CompletionQuery,
        matcher: &mut TextMatcher,
    ) -> Vec<CompletionCandidate> {
        search_with_indexes(engine, &index.tokens, &index.whole, query, matcher)
    }

    fn full_history(
        index: &HistoryFixture,
        query: &CompletionQuery,
        matcher: &mut TextMatcher,
    ) -> Vec<CompletionCandidate> {
        search_whole(&index.whole, query, matcher)
    }

    #[test]
    fn both_modes_offer_multiline_quick_commands_for_paste_review() {
        use crate::features::commands::completion_runtime::tests::HistoryFixture;
        use nyaterm_core::completion::CompletionEngine;
        use nyaterm_core::completion::matching::TextMatcher;
        use nyaterm_core::completion::types::CandidateKind;

        let command = "echo first\\\r\n  second\n\necho third";
        let quick = serde_json::from_value(serde_json::json!({
            "id": "multiline", "label": "下载驱动", "command": command
        }))
        .unwrap();
        let mut query = CompletionQuery {
            fill_policy: None,
            context: ShellCommandContext {
                scope: CompletionScope::new(&["multiline"]),
                dialect: ShellDialect::Bash,
                command_events: true,
            },
            input: "下载驱动".into(),
            mode: CompletionMode::History,
            quick: Arc::from([quick]),
            min: 2,
            max: 64,
        };
        let mut engine = CompletionEngine::embedded();
        let mut matcher = TextMatcher::default();
        for mode in [CompletionMode::History, CompletionMode::Token] {
            query.mode = mode;
            for input in ["下载驱动", ""] {
                query.input = input.into();
                let candidates = search_candidates(
                    &mut engine,
                    &HistoryFixture::default(),
                    &query,
                    &mut matcher,
                );
                let candidate = &candidates[0];
                assert_eq!(candidate.kind, CandidateKind::QuickCommand);
                assert_eq!(candidate.display, "下载驱动");
                assert_eq!(candidate.target_line(input).as_deref(), Some(command));
                assert!(candidate.fillable);
            }
        }
    }

    #[test]
    fn both_modes_find_cross_word_history_and_named_quick_commands() {
        use crate::features::commands::completion_runtime::tests::HistoryFixture;
        use nyaterm_core::completion::CompletionEngine;
        use nyaterm_core::completion::matching::TextMatcher;
        use nyaterm_core::models::quick_commands::QuickCommand;
        use std::collections::BTreeMap;

        let scope = CompletionScope::new(&["whole-command-search"]);
        let mut index = HistoryFixture::default();
        for origin in [scope.clone(), CompletionScope::legacy()] {
            index.upsert(ScopedHistoryRecord {
                version: 1,
                scope: origin,
                dialect: ShellDialect::Bash,
                command: "systemctl restart mihomo.service".into(),
                use_count: 3,
                last_used_at_ms: 10,
                extensions: BTreeMap::new(),
            });
        }
        let quick: QuickCommand = serde_json::from_value(serde_json::json!({
            "id": "restart-service", "label": "重启代理", "command": "systemctl restart example.service"
        })).unwrap();
        let unnamed: QuickCommand = serde_json::from_value(serde_json::json!({
            "id": "unnamed", "label": "", "command": "internal-tool deploy"
        }))
        .unwrap();
        let mut query = CompletionQuery {
            fill_policy: None,
            context: ShellCommandContext {
                scope,
                dialect: ShellDialect::Bash,
                command_events: true,
            },
            input: "systmi".into(),
            mode: CompletionMode::History,
            quick: Arc::from([quick, unnamed]),
            min: 2,
            max: 64,
        };
        let mut engine = CompletionEngine::embedded();
        let mut matcher = TextMatcher::default();
        for mode in [CompletionMode::History, CompletionMode::Token] {
            query.mode = mode;
            for (input, command, display) in [
                (
                    "systmi",
                    "systemctl restart mihomo.service",
                    "systemctl restart mihomo.service",
                ),
                ("重启代理", "systemctl restart example.service", "重启代理"),
                ("internal-t", "internal-tool deploy", "internal-tool deploy"),
            ] {
                query.input = input.into();
                let candidates = search_candidates(&mut engine, &index, &query, &mut matcher);
                assert_eq!(candidates[0].insert_text, command);
                assert_eq!(candidates[0].display, display);
                assert_eq!(candidates[0].range, 0..input.len());
                assert!(candidates[0].fillable);
                assert_eq!(
                    candidates
                        .iter()
                        .filter(|c| c.insert_text == command)
                        .count(),
                    1
                );
            }
        }
        query.context.scope = CompletionScope::new(&["other-host"]);
        query.input = "systmi".into();
        let candidates = search_candidates(&mut engine, &index, &query, &mut matcher);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].sources,
            [CandidateSource::History, CandidateSource::LegacyHistory]
        );
    }

    #[test]
    fn automatic_search_supplements_whole_commands_and_deduplicates_target_lines() {
        use crate::features::commands::completion_runtime::tests::HistoryFixture;
        use nyaterm_core::completion::CompletionEngine;
        use nyaterm_core::completion::matching::TextMatcher;
        use nyaterm_core::models::quick_commands::QuickCommand;
        use std::collections::HashSet;

        let quick: QuickCommand = serde_json::from_value(serde_json::json!({
            "id": "docker", "label": "dock", "command": "docker"
        }))
        .unwrap();
        let mut query = CompletionQuery {
            fill_policy: None,
            context: ShellCommandContext {
                scope: CompletionScope::legacy(),
                dialect: ShellDialect::Bash,
                command_events: false,
            },
            input: "dock".into(),
            mode: CompletionMode::History,
            quick: Arc::from([quick]),
            min: 2,
            max: 64,
        };
        let index = HistoryFixture::default();
        let mut engine = CompletionEngine::embedded();
        let mut matcher = TextMatcher::default();
        let manual = search_candidates(&mut engine, &index, &query, &mut matcher);
        assert_eq!(manual.len(), 1);
        assert_eq!(
            manual[0].kind,
            nyaterm_core::completion::types::CandidateKind::QuickCommand
        );
        query.mode = CompletionMode::Token;
        let automatic = search_candidates(&mut engine, &index, &query, &mut matcher);
        assert_eq!(automatic[0].display, "dock");
        assert_eq!(automatic[0].kind, manual[0].kind);
        assert_eq!(
            automatic[0].sources,
            [CandidateSource::Catalog, CandidateSource::QuickCommand]
        );
        let targets: HashSet<_> = automatic
            .iter()
            .map(|c| c.target_line(&query.input).unwrap())
            .collect();
        assert_eq!(automatic.len(), targets.len());

        query.input = "docker run --na".into();
        query.quick = Arc::from([]);
        assert!(
            search_candidates(&mut engine, &index, &query, &mut matcher)
                .iter()
                .any(|c| c.insert_text == "--name")
        );
        query.mode = CompletionMode::History;
        assert!(search_candidates(&mut engine, &index, &query, &mut matcher).is_empty());
    }

    #[test]
    fn whole_commands_keep_their_order_and_limit_before_catalog_supplements() {
        use crate::features::commands::completion_runtime::tests::HistoryFixture;
        use nyaterm_core::completion::CompletionEngine;
        use nyaterm_core::completion::matching::TextMatcher;
        use std::collections::BTreeMap;

        let scope = CompletionScope::new(&["ranking-fixture"]);
        let mut index = HistoryFixture::default();
        for n in 0..16 {
            index.upsert(ScopedHistoryRecord {
                version: 1,
                scope: scope.clone(),
                dialect: ShellDialect::Bash,
                command: format!("docker run fixture-{n}"),
                use_count: 1,
                last_used_at_ms: n,
                extensions: BTreeMap::new(),
            });
        }
        let mut query = CompletionQuery {
            fill_policy: None,
            context: ShellCommandContext {
                scope,
                dialect: ShellDialect::Bash,
                command_events: true,
            },
            input: "dock".into(),
            mode: CompletionMode::History,
            quick: Arc::from([]),
            min: 2,
            max: 64,
        };
        let mut engine = CompletionEngine::embedded();
        let mut matcher = TextMatcher::default();
        let manual = search_candidates(&mut engine, &index, &query, &mut matcher);
        query.mode = CompletionMode::Token;
        let automatic = search_candidates(&mut engine, &index, &query, &mut matcher);
        assert_eq!(automatic.len(), 12);
        assert_eq!(
            automatic.iter().map(|c| &c.insert_text).collect::<Vec<_>>(),
            manual.iter().map(|c| &c.insert_text).collect::<Vec<_>>()
        );
        query.max = 3;
        query.mode = CompletionMode::History;
        assert!(search_candidates(&mut engine, &index, &query, &mut matcher).is_empty());
    }
    #[test]
    fn full_history_merges_origins_and_uses_current_scope_statistics_once() {
        use crate::features::commands::completion_runtime::tests::HistoryFixture;
        use nyaterm_core::completion::matching::TextMatcher;
        use nyaterm_core::models::quick_commands::QuickCommand;
        use std::collections::BTreeMap;
        let scope = CompletionScope::new(&["history-origin-fixture"]);
        let mut index = HistoryFixture::default();
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
            fill_policy: None,
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
            fill_policy: None,
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
                .iter()
                .any(|candidate| candidate.insert_text == "internal-tool deploy"
                    && candidate.kind == CandidateKind::History)
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

    #[test]
    fn both_modes_find_other_host_whole_commands_without_learning_its_argument_tokens() {
        let a = CompletionScope::new(&["host-a"]);
        let b = CompletionScope::new(&["host-b"]);
        let mut index = HistoryFixture::default();
        index.upsert(ScopedHistoryRecord {
            version: 1,
            scope: a.clone(),
            dialect: ShellDialect::Bash,
            command: "curl https://www.baidu.com".into(),
            use_count: 1,
            last_used_at_ms: 1,
            extensions: std::collections::BTreeMap::new(),
        });
        let mut engine = CompletionEngine::embedded();
        let mut matcher = TextMatcher::default();
        for scope in [a, b] {
            for mode in [CompletionMode::Token, CompletionMode::History] {
                let query = CompletionQuery {
                    fill_policy: None,
                    context: ShellCommandContext {
                        scope: scope.clone(),
                        dialect: ShellDialect::Bash,
                        command_events: true,
                    },
                    input: "curl h".into(),
                    mode,
                    quick: Arc::from([]),
                    min: 2,
                    max: 64,
                };
                let results = search_candidates(&mut engine, &index, &query, &mut matcher);
                let history = results
                    .iter()
                    .find(|item| item.insert_text == "curl https://www.baidu.com")
                    .unwrap();
                assert_eq!(history.kind, CandidateKind::History);
                assert_eq!(
                    history.target_line("curl h").as_deref(),
                    Some("curl https://www.baidu.com")
                );
                assert!(history.fillable);
                assert!(
                    !results
                        .iter()
                        .any(|item| item.kind == CandidateKind::Argument
                            && item.insert_text == "https://www.baidu.com")
                );
            }
        }
    }

    #[test]
    fn unchanged_completion_queries_search_cache_while_database_worker_is_busy() {
        use nyaterm_store::StoreDomain;
        use std::sync::mpsc::channel;
        use std::time::Duration;

        let dir = TestConfigDir::new("nyaterm-cached-completion-worker");
        let store = blocking_test_store(dir.path());
        store
            .request_fn(StoreDomain::Commands, |db| {
                db.append_command_history("curl https://www.baidu.com")
            })
            .unwrap();
        let (events, _rx) = unbounded();
        let runtime = CompletionRuntime::new(store.clone(), events);
        let query = || CompletionQuery {
            fill_policy: None,
            context: ShellCommandContext {
                scope: CompletionScope::new(&["host-b"]),
                dialect: ShellDialect::Bash,
                command_events: true,
            },
            input: "curl h".into(),
            mode: CompletionMode::Token,
            quick: Arc::from([]),
            min: 2,
            max: 64,
        };
        let first = futures::executor::block_on(runtime.search(query()));
        assert_eq!(
            first.candidates[0].insert_text,
            "curl https://www.baidu.com"
        );

        let (started, waiting) = channel();
        let (release, blocked) = channel();
        let blocker =
            crate::thread_owner::spawn_joinable("completion-store-blocker-test", move || {
                store.request_fn(StoreDomain::Commands, move |_db| {
                    started.send(()).unwrap();
                    let _ = blocked.recv_timeout(Duration::from_secs(10));
                    Ok(())
                })
            })
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let (finished, result) = channel();
        let searcher =
            crate::thread_owner::spawn_joinable("completion-cache-search-test", move || {
                let _ = finished.send(futures::executor::block_on(runtime.search(query())));
            })
            .unwrap();
        let cached = result.recv_timeout(Duration::from_secs(5));
        // Release before asserting so a regression cannot strand the actor.
        let _ = release.send(());
        blocker.join().unwrap().unwrap();
        searcher.join().unwrap();
        let cached = cached.expect("unchanged queries must not wait for the database actor");
        assert_eq!(cached.revision, first.revision);
        assert_eq!(
            cached.candidates[0].insert_text,
            first.candidates[0].insert_text
        );
    }

    #[test]
    fn workers_refresh_global_only_appends_deletion_and_backup_restore() {
        use nyaterm_store::StoreDomain;

        let dir = TestConfigDir::new("nyaterm-global-completion-worker");
        let store = blocking_test_store(dir.path());
        let (tx, _rx) = unbounded();
        let runtime = CompletionRuntime::new(store.clone(), tx.clone());
        let second = CompletionRuntime::new(store.clone(), tx);
        let context = ShellCommandContext {
            scope: CompletionScope::new(&["host-a"]),
            dialect: ShellDialect::Bash,
            command_events: true,
        };
        let other = ShellCommandContext {
            scope: CompletionScope::new(&["host-b"]),
            ..context.clone()
        };
        let query = |input: &str| CompletionQuery {
            fill_policy: None,
            context: other.clone(),
            input: input.into(),
            mode: CompletionMode::Token,
            quick: Arc::from([]),
            min: 2,
            max: 64,
        };
        let search = |worker: &CompletionRuntime, input: &str| {
            futures::executor::block_on(worker.search(query(input)))
        };
        assert!(runtime.observe(context.clone(), "curl https://www.baidu.com".into()));
        assert!(
            search(&runtime, "curl h")
                .candidates
                .iter()
                .any(|item| item.insert_text == "curl https://www.baidu.com")
        );
        assert!(!search(&second, "curl h").candidates.is_empty());
        let backup_path = dir.path().join("fixture-backup.nya");
        let export_path = backup_path.clone();
        store
            .request_fn(StoreDomain::Commands, move |db| {
                db.export_encrypted_portable_snapshot_from_open_store(
                    &export_path,
                    "fixture",
                    "fixture",
                    "fixture-backup-password",
                )
            })
            .unwrap();
        assert!(search(&second, "fixture-only-glo").candidates.is_empty());
        store
            .request_fn(StoreDomain::Commands, |db| {
                db.append_command_history("fixture-only-global command")
            })
            .unwrap();
        let first = search(&second, "fixture-only-glo");
        assert_eq!(
            first.candidates[0].insert_text,
            "fixture-only-global command"
        );
        let repeated = search(&second, "fixture-only-glo");
        assert_eq!(first.revision, repeated.revision);
        assert_eq!(first.candidates[0].score, repeated.candidates[0].score);
        futures::executor::block_on(runtime.delete("curl https://www.baidu.com".into())).unwrap();
        assert!(
            !search(&second, "curl h")
                .candidates
                .iter()
                .any(|item| item.insert_text == "curl https://www.baidu.com")
        );
        store
            .request_fn(StoreDomain::Commands, move |db| {
                db.import_encrypted_portable_snapshot_into_open_store(
                    &backup_path,
                    "fixture-backup-password",
                )
            })
            .unwrap();
        assert!(
            search(&second, "curl h")
                .candidates
                .iter()
                .any(|item| item.insert_text == "curl https://www.baidu.com")
        );
        assert!(search(&second, "fixture-only-glo").candidates.is_empty());
        assert!(runtime.observe(context, "curl --user fixture-sensitive".into()));
        assert!(search(&runtime, "fixture-sensitive").candidates.is_empty());
    }
}
