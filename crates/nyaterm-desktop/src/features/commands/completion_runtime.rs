//! Persistence and event adapters for the UI-independent completion worker.
use crate::features::runtime_jobs::CommandPersistenceResult;
use futures::channel::mpsc::UnboundedSender;
use nyaterm_completion_runtime::history::{
    CompletionEvent, CompletionHistoryRepository, HistoryAppend, HistorySnapshot,
};
pub(in crate::features) use nyaterm_completion_runtime::runtime::{
    CompletionQuery, CompletionSearchOutcome,
};
use nyaterm_core::completion::types::ShellCommandContext;
use nyaterm_core::models::quick_commands::CommandHistoryEntry;
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct StoreHistoryRepository(StoreBlockingClient);
impl CompletionHistoryRepository for StoreHistoryRepository {
    fn generation(&self) -> Result<Arc<AtomicU64>, String> {
        self.0
            .request_fn(StoreDomain::Commands, |db| Ok(db.completion_generation()))
            .map_err(|error| error.to_string())
    }
    fn snapshot(&self) -> Result<HistorySnapshot, String> {
        self.0
            .request_fn(StoreDomain::Commands, |db| {
                let generation = db.completion_generation().load(Ordering::Acquire);
                Ok(HistorySnapshot {
                    history: db.completion_history_snapshot()?,
                    generation,
                })
            })
            .map_err(|error| error.to_string())
    }
    fn append(
        &self,
        context: ShellCommandContext,
        command: String,
        include_global: bool,
    ) -> Result<HistoryAppend, String> {
        self.0
            .request_fn(StoreDomain::Commands, move |db| {
                let before = db.completion_generation().load(Ordering::Acquire);
                let update = db.append_completion_contribution_with_update(
                    &context.scope,
                    context.dialect,
                    &command,
                    include_global,
                )?;
                let after = db.completion_generation().load(Ordering::Acquire);
                Ok(HistoryAppend {
                    before,
                    after,
                    update: update.map(|(_, update)| update),
                    entries: db.list_command_history(64)?,
                    warning: db.take_command_history_retention_warning(),
                })
            })
            .map_err(|error| error.to_string())
    }
    fn delete(&self, command: String) -> Result<Vec<CommandHistoryEntry>, String> {
        self.0
            .request_fn(StoreDomain::Commands, move |db| {
                db.delete_command_history(&command)?;
                db.list_command_history(64)
            })
            .map_err(|error| error.to_string())
    }
}
#[derive(Clone)]
pub(in crate::features) struct CompletionRuntime(
    nyaterm_completion_runtime::runtime::CompletionRuntime,
);
impl std::ops::Deref for CompletionRuntime {
    type Target = nyaterm_completion_runtime::runtime::CompletionRuntime;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl CompletionRuntime {
    pub(super) fn new_with_user_specs(
        store: StoreBlockingClient,
        events: UnboundedSender<CommandPersistenceResult>,
        dir: Option<PathBuf>,
    ) -> Self {
        Self(nyaterm_completion_runtime::runtime::CompletionRuntime::new(
            Arc::new(StoreHistoryRepository(store)),
            dir,
            Arc::new(move |event| {
                let event = match event {
                    CompletionEvent::History(result) => CommandPersistenceResult::History(result),
                    CompletionEvent::HistoryMaintenanceWarning(message) => {
                        CommandPersistenceResult::HistoryMaintenanceWarning(message)
                    }
                };
                let _ = events.unbounded_send(event);
            }),
        ))
    }
    #[cfg(test)]
    fn new(store: StoreBlockingClient, events: UnboundedSender<CommandPersistenceResult>) -> Self {
        Self::new_with_user_specs(store, events, None)
    }
    #[cfg(test)]
    async fn search(
        &self,
        query: CompletionQuery,
    ) -> nyaterm_core::completion::types::CompletionResult {
        match self.search_outcome(query).await {
            CompletionSearchOutcome::Ready(result) => result,
            _ => Default::default(),
        }
    }
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
    fn newest_queued_search_supersedes_older_requests() {
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
    }
}
