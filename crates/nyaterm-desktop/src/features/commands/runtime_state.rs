//! Background runtime ownership shared by command history and quick commands.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nyaterm_store::{StoreBlockingClient, StoreDomain};

use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::{
    runtime_jobs::CommandPersistenceRequest, runtime_jobs::CommandPersistenceResult,
};

pub(in crate::features) struct CommandRuntimeState {
    store: StoreBlockingClient,
    scheduler: BlockingJobScheduler,
    result_tx: UnboundedSender<CommandPersistenceResult>,
    /// Taken once by `NyaTermApp::start_command_persistence_event_drain`,
    /// which owns delivery from then on. `None` afterwards, so a second start
    /// is a no-op.
    rx: Option<UnboundedReceiver<CommandPersistenceResult>>,
    pending: usize,
}

impl CommandRuntimeState {
    pub(in crate::features) fn new(
        store: StoreBlockingClient,
        scheduler: BlockingJobScheduler,
    ) -> Self {
        let (result_tx, rx) = unbounded();
        Self {
            store,
            scheduler,
            result_tx,
            rx: Some(rx),
            pending: 0,
        }
    }

    pub(in crate::features) fn queue(&mut self, request: CommandPersistenceRequest) -> bool {
        let store = self.store.clone();
        let result_tx = self.result_tx.clone();
        if self
            .scheduler
            .submit_detached("command-persistence", move |_| {
                let result = match request {
                    CommandPersistenceRequest::AppendHistory(commands) => {
                        CommandPersistenceResult::History(
                            store
                                .request_fn(StoreDomain::Commands, move |database| {
                                    for command in commands {
                                        database.append_command_history(&command)?;
                                    }
                                    database.list_command_history(64)
                                })
                                .map_err(|error| error.to_string()),
                        )
                    }
                    CommandPersistenceRequest::IncrementQuickCommand(command_id) => {
                        let persisted_id = command_id.clone();
                        let result = store
                            .request_fn(StoreDomain::Commands, move |database| {
                                database.increment_quick_command_use_count(&persisted_id)
                            })
                            .map_err(|error| error.to_string());
                        CommandPersistenceResult::QuickCommandUseCount { command_id, result }
                    }
                };
                let _ = result_tx.unbounded_send(result);
            })
            .is_err()
        {
            return false;
        }
        self.pending = self.pending.saturating_add(1);
        true
    }

    pub(super) fn result_sender(&self) -> UnboundedSender<CommandPersistenceResult> {
        self.result_tx.clone()
    }
    pub(super) fn note_queued(&mut self) {
        self.pending = self.pending.saturating_add(1);
    }
    pub(in crate::features) fn take_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<CommandPersistenceResult>> {
        self.rx.take()
    }

    /// Account for one delivered result.
    pub(in crate::features) fn note_event_delivered(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    /// Account for the worker thread dropping its sender, reporting whether any
    /// request was still outstanding and so lost.
    pub(in crate::features) fn note_worker_disconnected(&mut self) -> bool {
        std::mem::take(&mut self.pending) > 0
    }

    /// The pending counter still gates `note_worker_disconnected`; this read
    /// accessor is only needed to assert the lifecycle.
    #[cfg(test)]
    pub(in crate::features) fn is_idle(&self) -> bool {
        self.pending == 0
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt as _;

    use super::CommandRuntimeState;
    use crate::blocking_jobs::BlockingJobScheduler;
    use crate::features::{
        runtime_jobs::CommandPersistenceRequest, runtime_jobs::CommandPersistenceResult,
    };
    use crate::test_support::{TestConfigDir, blocking_test_store};

    #[test]
    fn command_runtime_owns_pending_request_lifecycle() {
        let test_dir = TestConfigDir::new("nyaterm-command-runtime-test");
        let store = blocking_test_store(test_dir.path());
        let scheduler = BlockingJobScheduler::new();
        let mut runtime = CommandRuntimeState::new(store.clone(), scheduler.clone());

        assert!(runtime.is_idle());
        assert!(runtime.queue(CommandPersistenceRequest::AppendHistory(vec![
            "pwd".to_string(),
        ])));
        assert!(!runtime.is_idle());

        let mut result_rx = runtime
            .take_event_receiver()
            .expect("the runtime holds its receiver until the drain starts");
        let result = futures::executor::block_on(result_rx.next())
            .expect("scheduler should return a persistence result");
        assert!(matches!(
            result,
            CommandPersistenceResult::History(Ok(history)) if history.iter().any(|entry| entry.command == "pwd")
        ));
        runtime.note_event_delivered();
        assert!(runtime.is_idle());

        store
            .request_fn(nyaterm_store::StoreDomain::Commands, |store| {
                store.upsert_quick_command(
                    serde_json::from_value(serde_json::json!({
                        "id":"fixture", "label":"Fixture", "command":"pwd"
                    }))?,
                    None,
                )
            })
            .unwrap();
        let generation = store.sync_mutation_generation();
        assert!(
            runtime.queue(CommandPersistenceRequest::IncrementQuickCommand(
                "fixture".into()
            ))
        );
        assert!(matches!(
            futures::executor::block_on(result_rx.next()).unwrap(),
            CommandPersistenceResult::QuickCommandUseCount { result: Ok(()), .. }
        ));
        runtime.note_event_delivered();
        assert_eq!(store.sync_mutation_generation(), generation);
        assert_eq!(
            store
                .request_fn(nyaterm_store::StoreDomain::Commands, |store| store
                    .load_quick_commands())
                .unwrap()
                .commands[0]
                .use_count,
            Some(1)
        );

        scheduler.shutdown();
        assert!(!runtime.queue(CommandPersistenceRequest::AppendHistory(Vec::new())));
        assert!(!runtime.note_worker_disconnected());
        assert!(runtime.is_idle());
    }
}
