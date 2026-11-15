//! A single background owner for the catalog, token index and scoped mutations.
use super::completion_user_specs::UserSpecLoader;
use crate::features::runtime_jobs::CommandPersistenceResult;
use futures::channel::{mpsc::UnboundedSender, oneshot};
use nyaterm_core::completion::CompletionEngine;
use nyaterm_core::completion::history::TokenHistoryIndex;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::query::search_candidates_with_cancel;
use nyaterm_core::completion::types::{
    CompletionFillPolicy, CompletionMode, CompletionResult, ShellCommandContext,
};
use nyaterm_core::completion::whole_history::WholeCommandHistoryIndex;
use nyaterm_core::models::quick_commands::{CommandHistoryEntry, QuickCommand};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use std::path::PathBuf;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};

mod queue;

#[derive(Debug)]
pub(in crate::features) enum CompletionSearchOutcome {
    Ready(CompletionResult),
    Superseded,
    Busy,
    Unavailable,
}

#[derive(Clone)]
pub(in crate::features) struct CompletionRuntime {
    tx: queue::Sender,
    epoch: Arc<AtomicU64>,
    catalog_epoch: Arc<AtomicU64>,
    specs: UserSpecLoader,
    latest_search: Arc<AtomicU64>,
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

impl CompletionQuery {
    fn borrowed(&self) -> nyaterm_core::completion::query::CompletionQuery<'_> {
        nyaterm_core::completion::query::CompletionQuery {
            context: &self.context,
            input: &self.input,
            mode: self.mode,
            quick: &self.quick,
            min: self.min,
            max: self.max,
            fill_policy: self.fill_policy,
            now_ms: u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
        }
    }
}

enum Request {
    Observe(ShellCommandContext, String, bool),
    Search(
        CompletionQuery,
        u64,
        u64,
        oneshot::Sender<CompletionSearchOutcome>,
    ),
    Delete(
        String,
        oneshot::Sender<Result<Vec<CommandHistoryEntry>, String>>,
    ),
}

impl CompletionRuntime {
    #[cfg(test)]
    pub(super) fn new(
        store: StoreBlockingClient,
        events: UnboundedSender<CommandPersistenceResult>,
    ) -> Self {
        Self::new_with_user_specs(store, events, None)
    }

    /// `user_specs_dir` is read by the worker only; a missing directory is fine.
    pub(super) fn new_with_user_specs(
        store: StoreBlockingClient,
        events: UnboundedSender<CommandPersistenceResult>,
        user_specs_dir: Option<PathBuf>,
    ) -> Self {
        let (tx, rx) = queue::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let catalog_epoch = Arc::new(AtomicU64::new(0));
        let worker_catalog_epoch = catalog_epoch.clone();
        let specs = UserSpecLoader::spawn(user_specs_dir);
        let worker_specs = specs.clone();
        let latest_search = Arc::new(AtomicU64::new(0));
        let worker_search = latest_search.clone();
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
                while let Some(request) = rx.recv() {
                    match request {
                        Request::Observe(context, command, include_global) => {
                            let outcome = store
                                .request_fn(StoreDomain::Commands, move |database| {
                                    let before =
                                        database.completion_generation().load(Ordering::Acquire);
                                    let update = database
                                        .append_completion_contribution_with_update(
                                            &context.scope,
                                            context.dialect,
                                            &command,
                                            include_global,
                                        )?;
                                    let after =
                                        database.completion_generation().load(Ordering::Acquire);
                                    Ok((
                                        before,
                                        after,
                                        update,
                                        database.list_command_history(64)?,
                                        database.take_command_history_retention_warning(),
                                    ))
                                })
                                .map_err(|error| error.to_string());
                            let outcome =
                                outcome.map(|(before, after, update, entries, warning)| {
                                    if loaded_generation == Some(before)
                                        && after == before.wrapping_add(1)
                                        && let Some((_, update)) = update
                                    {
                                        apply_history_update(
                                            &mut history,
                                            &mut whole_history,
                                            update,
                                        );
                                        loaded_generation = Some(after);
                                    }
                                    if let Some(message) = warning {
                                        let _ = events.unbounded_send(
                                            CommandPersistenceResult::HistoryMaintenanceWarning(
                                                message,
                                            ),
                                        );
                                    }
                                    entries
                                });
                            let _ =
                                events.unbounded_send(CommandPersistenceResult::History(outcome));
                        }
                        Request::Search(query, revision, search_id, response) => {
                            let mut cancelled = || {
                                response.is_canceled()
                                    || worker_search.load(Ordering::Acquire) != search_id
                            };
                            if cancelled() {
                                continue;
                            }
                            if let Some(updated) = worker_specs.take() {
                                engine = updated;
                                worker_catalog_epoch.fetch_add(1, Ordering::AcqRel);
                            }
                            if loaded_generation != Some(generation()) {
                                loaded_generation =
                                    reload_history(&store, &mut history, &mut whole_history).ok();
                                if loaded_generation.is_none() {
                                    history = TokenHistoryIndex::default();
                                    whole_history = WholeCommandHistoryIndex::default();
                                }
                            }
                            let candidates = search_candidates_with_cancel(
                                &mut engine,
                                &history,
                                &whole_history,
                                &query.borrowed(),
                                &mut whole_matcher,
                                &mut cancelled,
                            );
                            let Some(candidates) = candidates else {
                                continue;
                            };
                            let _ =
                                response.send(CompletionSearchOutcome::Ready(CompletionResult {
                                    candidates,
                                    revision: revision
                                        .wrapping_add(loaded_generation.unwrap_or_else(generation))
                                        .wrapping_add(worker_catalog_epoch.load(Ordering::Acquire)),
                                }));
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
                    }
                }
            });
        // Dropping the receiver on spawn failure makes every try_send fail closed.
        drop(spawn);
        Self {
            tx,
            epoch,
            catalog_epoch,
            specs,
            latest_search,
            shared_generation,
        }
    }
    pub(in crate::features) fn revision(&self) -> u64 {
        self.epoch
            .load(Ordering::Acquire)
            .wrapping_add(self.catalog_epoch.load(Ordering::Acquire))
            .wrapping_add(
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
        self.specs.request();
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
    #[cfg(test)]
    pub(in crate::features) async fn search(&self, query: CompletionQuery) -> CompletionResult {
        match self.search_outcome(query).await {
            CompletionSearchOutcome::Ready(result) => result,
            _ => CompletionResult::default(),
        }
    }
    pub(in crate::features) async fn search_outcome(
        &self,
        query: CompletionQuery,
    ) -> CompletionSearchOutcome {
        let revision = self.epoch.load(Ordering::Acquire);
        let search_id = self
            .latest_search
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let (tx, rx) = oneshot::channel();
        match self
            .tx
            .try_send(Request::Search(query, revision, search_id, tx))
        {
            Ok(()) => rx.await.unwrap_or_else(|_| {
                if self.tx.is_closed() {
                    CompletionSearchOutcome::Unavailable
                } else {
                    CompletionSearchOutcome::Superseded
                }
            }),
            Err(queue::QueueError::Busy) => CompletionSearchOutcome::Busy,
            Err(queue::QueueError::Closed) => CompletionSearchOutcome::Unavailable,
        }
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

fn apply_history_update(
    tokens: &mut TokenHistoryIndex,
    whole: &mut WholeCommandHistoryIndex,
    update: nyaterm_core::completion::types::CompletionHistoryUpdate,
) {
    if let Some(global) = &update.global {
        whole.upsert_global(global);
    }
    whole.upsert_scoped(&update.record);
    let scope = update.record.scope.clone();
    tokens.upsert(update.record);
    // A clock moving backwards can evict the contribution just written.
    // Apply transaction deletions last so the cache reflects the committed rows.
    for command in &update.evicted_global {
        whole.remove_global(command);
    }
    for command in &update.evicted_legacy {
        let scope = nyaterm_core::completion::types::CompletionScope::legacy();
        whole.remove_scoped(&scope, command);
        tokens.remove_scoped(&scope, command);
    }
    for command in &update.evicted_scoped {
        whole.remove_scoped(&scope, command);
        tokens.remove_scoped(&scope, command);
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

#[cfg(test)]
mod tests {
    use super::{CompletionQuery, CompletionRuntime};
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use futures::channel::mpsc::unbounded;
    use nyaterm_core::completion::types::{
        CandidateKind, CompletionMode, CompletionScope, ShellCommandContext, ShellDialect,
    };
    use std::sync::Arc;
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
        runtime.reload();
        let expected_revision = runtime.revision();
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
        assert_eq!(cached.revision, expected_revision);
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
    #[test]
    fn newest_queued_search_supersedes_older_requests_and_worker_loss_is_visible() {
        use super::CompletionSearchOutcome;
        use futures::FutureExt;
        use nyaterm_store::StoreDomain;
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let dir = TestConfigDir::new("nyaterm-completion-newest-search");
        let store = blocking_test_store(dir.path());
        let held_store = store.clone();
        let (started, ready) = channel();
        let (release, blocked) = channel();
        let blocker = crate::thread_owner::spawn_joinable("completion-queue-test", move || {
            held_store.request_fn(StoreDomain::Commands, move |_| {
                started.send(()).unwrap();
                let _ = blocked.recv_timeout(Duration::from_secs(10));
                Ok(())
            })
        })
        .unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (events, _rx) = unbounded();
        let runtime = CompletionRuntime::new(store, events);
        let query = || CompletionQuery {
            fill_policy: None,
            context: ShellCommandContext {
                scope: CompletionScope::new(&["latest"]),
                dialect: ShellDialect::Bash,
                command_events: true,
            },
            input: "dock".into(),
            mode: CompletionMode::Token,
            quick: Arc::from([]),
            min: 1,
            max: 100,
        };
        let mut first = Box::pin(runtime.search_outcome(query()));
        let mut newest = Box::pin(runtime.search_outcome(query()));
        assert!(first.as_mut().now_or_never().is_none());
        assert!(newest.as_mut().now_or_never().is_none());
        assert!(matches!(
            futures::executor::block_on(first),
            CompletionSearchOutcome::Superseded
        ));
        release.send(()).unwrap();
        blocker.join().unwrap().unwrap();
        assert!(matches!(
            futures::executor::block_on(newest),
            CompletionSearchOutcome::Ready(_)
        ));
        // A closed queue must be distinguishable from a superseded query.
        let (sender, receiver) = super::queue::channel();
        let mut unavailable = runtime.clone();
        unavailable.tx = sender;
        drop(receiver);
        assert!(matches!(
            futures::executor::block_on(unavailable.search_outcome(query())),
            CompletionSearchOutcome::Unavailable
        ));
    }
    #[test]
    fn incremental_updates_respect_eviction_of_a_newly_written_contribution() {
        use super::apply_history_update;
        use nyaterm_core::completion::history::TokenHistoryIndex;
        use nyaterm_core::completion::types::{CompletionHistoryUpdate, ScopedHistoryRecord};
        use nyaterm_core::completion::whole_history::WholeCommandHistoryIndex;
        use nyaterm_core::models::quick_commands::CommandHistoryEntry;
        let scope = CompletionScope::new(&["clock-rollback"]);
        let command = "fixture rollback";
        let mut tokens = TokenHistoryIndex::default();
        let mut whole = WholeCommandHistoryIndex::default();
        apply_history_update(
            &mut tokens,
            &mut whole,
            CompletionHistoryUpdate {
                record: ScopedHistoryRecord {
                    version: 1,
                    scope: scope.clone(),
                    dialect: ShellDialect::Bash,
                    command: command.into(),
                    use_count: 1,
                    last_used_at_ms: 1,
                    extensions: Default::default(),
                },
                global: Some(CommandHistoryEntry {
                    command: command.into(),
                    use_count: 1,
                    last_used_at_ms: 1,
                }),
                evicted_global: vec![command.into()],
                evicted_scoped: vec![command.into()],
                evicted_legacy: Vec::new(),
            },
        );
        assert!(whole.get(command, &scope).is_none());
        assert!(
            tokens
                .candidates(&scope, &[], ShellDialect::Bash)
                .is_empty()
        );
    }
}
