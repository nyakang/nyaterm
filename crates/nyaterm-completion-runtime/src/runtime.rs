//! A single background owner for the catalog, token index and scoped mutations.
use crate::history::{CompletionEvent, CompletionHistoryRepository, HistoryAppend};
use crate::user_specs::UserSpecLoader;
use futures::channel::oneshot;
use nyaterm_core::completion::CompletionEngine;
use nyaterm_core::completion::history::TokenHistoryIndex;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::query::search_candidates_with_cancel;
use nyaterm_core::completion::types::{
    CompletionFillPolicy, CompletionMode, CompletionResult, ShellCommandContext,
};
use nyaterm_core::completion::whole_history::WholeCommandHistoryIndex;
use nyaterm_core::models::quick_commands::{CommandHistoryEntry, QuickCommand};
use std::path::PathBuf;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};

mod queue;

#[derive(Debug)]
pub enum CompletionSearchOutcome {
    Ready(CompletionResult),
    Superseded,
    Busy,
    Unavailable,
}

#[derive(Clone)]
pub struct CompletionRuntime {
    tx: queue::Sender,
    epoch: Arc<AtomicU64>,
    catalog_epoch: Arc<AtomicU64>,
    specs: UserSpecLoader,
    latest_search: Arc<AtomicU64>,
    shared_generation: Arc<OnceLock<Arc<AtomicU64>>>,
}

pub struct CompletionQuery {
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
    /// The repository and event sink run on the worker, never a GPUI update callback.
    pub fn new(
        store: Arc<dyn CompletionHistoryRepository>,
        user_specs_dir: Option<PathBuf>,
        events: Arc<dyn Fn(CompletionEvent) + Send + Sync>,
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
                if let Ok(generation) = store.generation() {
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
                    reload_history(store.as_ref(), &mut history, &mut whole_history).ok();
                while let Some(request) = rx.recv() {
                    match request {
                        Request::Observe(context, command, include_global) => {
                            let outcome = store.append(context, command, include_global);
                            let outcome = outcome.map(
                                |HistoryAppend {
                                     before,
                                     after,
                                     update,
                                     entries,
                                     warning,
                                 }| {
                                    if loaded_generation == Some(before)
                                        && after == before.wrapping_add(1)
                                        && let Some(update) = update
                                    {
                                        apply_history_update(
                                            &mut history,
                                            &mut whole_history,
                                            update,
                                        );
                                        loaded_generation = Some(after);
                                    }
                                    if let Some(message) = warning {
                                        events(CompletionEvent::HistoryMaintenanceWarning(message));
                                    }
                                    entries
                                },
                            );
                            events(CompletionEvent::History(outcome));
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
                                loaded_generation = reload_history(
                                    store.as_ref(),
                                    &mut history,
                                    &mut whole_history,
                                )
                                .ok();
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
                            let outcome = store.delete(command);
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
    pub fn revision(&self) -> u64 {
        self.epoch
            .load(Ordering::Acquire)
            .wrapping_add(self.catalog_epoch.load(Ordering::Acquire))
            .wrapping_add(
                self.shared_generation
                    .get()
                    .map_or(0, |value| value.load(Ordering::Acquire)),
            )
    }
    pub fn observe(&self, context: ShellCommandContext, command: String) -> bool {
        self.observe_submission(context, command, true)
    }
    pub fn observe_submission(
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
    pub fn reload(&self) {
        self.specs.request();
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
    pub async fn search_outcome(&self, query: CompletionQuery) -> CompletionSearchOutcome {
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
    pub async fn delete(&self, command: String) -> Result<Vec<CommandHistoryEntry>, String> {
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
    store: &dyn CompletionHistoryRepository,
    tokens: &mut TokenHistoryIndex,
    whole: &mut WholeCommandHistoryIndex,
) -> Result<u64, String> {
    let snapshot = store.snapshot()?;
    whole.reconcile(
        &snapshot.history.global_history,
        &snapshot.history.scoped_history,
    );
    tokens.reconcile(snapshot.history.scoped_history);
    Ok(snapshot.generation)
}

#[cfg(test)]
mod cache_tests {
    use nyaterm_core::completion::types::{CompletionScope, ShellDialect};
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

#[cfg(test)]
mod tests;
