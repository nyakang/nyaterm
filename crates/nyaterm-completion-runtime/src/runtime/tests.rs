use super::{CompletionQuery, CompletionRuntime, CompletionSearchOutcome, queue};
use crate::history::{
    CompletionEvent, CompletionHistoryRepository, HistoryAppend, HistorySnapshot,
};
use futures::executor::block_on;
use nyaterm_core::completion::types::{
    CompletionHistorySnapshot, CompletionMode, CompletionScope, ShellCommandContext, ShellDialect,
};
use nyaterm_core::models::quick_commands::CommandHistoryEntry;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::Duration;

#[derive(Default)]
struct MemoryRepository {
    generation: Arc<AtomicU64>,
    entries: Mutex<Vec<CommandHistoryEntry>>,
    unavailable: AtomicBool,
}
impl CompletionHistoryRepository for MemoryRepository {
    fn generation(&self) -> Result<Arc<AtomicU64>, String> {
        Ok(self.generation.clone())
    }
    fn snapshot(&self) -> Result<HistorySnapshot, String> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err("offline".into());
        }
        let entries = self.entries.lock().unwrap();
        Ok(HistorySnapshot {
            generation: self.generation.load(Ordering::Acquire),
            history: CompletionHistorySnapshot {
                global_history: entries.clone(),
                revision: 0,
                scoped_history: Vec::new(),
            },
        })
    }
    fn append(
        &self,
        _: ShellCommandContext,
        command: String,
        _: bool,
    ) -> Result<HistoryAppend, String> {
        let mut entries = self.entries.lock().unwrap();
        entries.push(CommandHistoryEntry {
            command,
            use_count: 1,
            last_used_at_ms: 1,
        });
        let before = self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(HistoryAppend {
            before,
            after: before.wrapping_add(1),
            update: None,
            entries: entries.clone(),
            warning: Some("retention warning".into()),
        })
    }
    fn delete(&self, command: String) -> Result<Vec<CommandHistoryEntry>, String> {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|entry| entry.command != command);
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(entries.clone())
    }
}
fn context() -> ShellCommandContext {
    ShellCommandContext {
        scope: CompletionScope::new(&["fixture"]),
        dialect: ShellDialect::Bash,
        command_events: true,
    }
}
fn query() -> CompletionQuery {
    CompletionQuery {
        context: context(),
        input: "fixture-private".into(),
        mode: CompletionMode::Token,
        quick: Arc::from([]),
        min: 1,
        max: 64,
        fill_policy: None,
    }
}
fn candidates(runtime: &CompletionRuntime) -> Vec<String> {
    let CompletionSearchOutcome::Ready(result) = block_on(runtime.search_outcome(query())) else {
        panic!("worker unavailable")
    };
    result
        .candidates
        .into_iter()
        .map(|candidate| candidate.insert_text)
        .collect()
}

#[test]
fn failed_initial_snapshot_recovers_and_external_mutations_invalidate_cached_history() {
    let repo = Arc::new(MemoryRepository::default());
    repo.unavailable.store(true, Ordering::Release);
    let runtime = CompletionRuntime::new(repo.clone(), None, Arc::new(|_| {}));
    assert!(candidates(&runtime).is_empty());
    repo.unavailable.store(false, Ordering::Release);
    repo.append(context(), "fixture-private-one".into(), true)
        .unwrap();
    assert_eq!(candidates(&runtime), ["fixture-private-one"]);
    let revision = runtime.revision();
    repo.delete("fixture-private-one".into()).unwrap();
    assert!(candidates(&runtime).is_empty());
    assert_ne!(runtime.revision(), revision);
}

#[test]
fn mutation_warning_precedes_history_and_last_client_drains_worker_before_shutdown() {
    let repo = Arc::new(MemoryRepository::default());
    let (events, received) = mpsc::channel();
    let runtime = CompletionRuntime::new(
        repo.clone(),
        None,
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    let other = runtime.clone();
    drop(runtime);
    assert!(other.observe(context(), "fixture-private-one".into()));
    drop(other);
    assert!(matches!(
        received.recv_timeout(Duration::from_secs(5)).unwrap(),
        CompletionEvent::HistoryMaintenanceWarning(_)
    ));
    let CompletionEvent::History(Ok(entries)) =
        received.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("expected history")
    };
    assert_eq!(entries[0].command, "fixture-private-one");
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap_err(),
        mpsc::RecvTimeoutError::Disconnected
    );
}

#[test]
fn closed_worker_reports_unavailable_instead_of_empty_search_results() {
    let mut runtime = CompletionRuntime::new(
        Arc::new(MemoryRepository::default()),
        None,
        Arc::new(|_| {}),
    );
    let (tx, rx) = queue::channel();
    drop(rx);
    runtime.tx = tx;
    assert!(matches!(
        block_on(runtime.search_outcome(query())),
        CompletionSearchOutcome::Unavailable
    ));
    assert!(!runtime.observe(context(), "fixture".into()));
    assert!(block_on(runtime.delete("fixture".into())).is_err());
}
