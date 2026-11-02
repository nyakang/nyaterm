//! Process-wide admission and completion barrier for sync operations and their UI follow-ups.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use nyaterm_core::CloudSyncError;
use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;

#[derive(Clone, Default)]
pub(crate) struct CloudSyncOperationRegistry(Arc<Mutex<RegistryState>>);

#[derive(Default)]
struct RegistryState {
    paused: bool,
    next_id: u64,
    operations: HashMap<u64, Operation>,
    waiters: Vec<oneshot::Sender<()>>,
}

struct Operation {
    name: &'static str,
    controls: Vec<CloudSyncOperationControl>,
    expired: Option<Arc<AtomicBool>>,
    stopping: Arc<AtomicBool>,
}

/// Clones retain the operation through asynchronous history writes and result application.
#[derive(Clone)]
pub(crate) struct CloudSyncOperationRegistration(Arc<Registration>);

struct Registration {
    registry: CloudSyncOperationRegistry,
    id: u64,
    stopping: Arc<AtomicBool>,
}

impl CloudSyncOperationRegistry {
    pub(crate) fn accepting(&self) -> bool {
        !self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .paused
    }

    pub(crate) fn register(
        &self,
        name: &'static str,
        controls: Vec<CloudSyncOperationControl>,
        expired: Option<Arc<AtomicBool>>,
    ) -> Option<CloudSyncOperationRegistration> {
        self.register_inner(name, controls, expired, false)
    }

    /// Result/history follow-ups must finish even after admission has closed.
    pub(crate) fn followup(&self, name: &'static str) -> CloudSyncOperationRegistration {
        self.register_inner(name, Vec::new(), None, true)
            .expect("sync follow-ups remain admitted during shutdown")
    }

    fn register_inner(
        &self,
        name: &'static str,
        controls: Vec<CloudSyncOperationControl>,
        expired: Option<Arc<AtomicBool>>,
        followup: bool,
    ) -> Option<CloudSyncOperationRegistration> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.paused && !followup {
            return None;
        }
        let id = state.next_id;
        state.next_id += 1;
        let stopping = Arc::new(AtomicBool::new(state.paused));
        state.operations.insert(
            id,
            Operation {
                name,
                controls,
                expired,
                stopping: stopping.clone(),
            },
        );
        Some(CloudSyncOperationRegistration(Arc::new(Registration {
            registry: self.clone(),
            id,
            stopping,
        })))
    }

    pub(crate) fn pause_and_cancel(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if std::mem::replace(&mut state.paused, true) {
            return;
        }
        for (id, operation) in &state.operations {
            operation.stopping.store(true, Ordering::Release);
            if let Some(expired) = &operation.expired {
                expired.store(true, Ordering::Release);
            }
            for control in &operation.controls {
                let phase = control.request_cancel();
                tracing::info!(
                    pid = std::process::id(),
                    operation_id = id,
                    operation = operation.name,
                    ?phase,
                    "shutdown requested cloud sync cancellation"
                );
            }
        }
    }

    pub(crate) async fn wait_for_idle(&self) {
        let receiver = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.operations.is_empty() {
                return;
            }
            let (sender, receiver) = oneshot::channel();
            state.waiters.push(sender);
            receiver
        };
        let _ = receiver.await;
    }

    pub(crate) fn resume(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .paused = false;
    }
}

impl CloudSyncOperationRegistration {
    pub(crate) fn shutdown_cancelled<T>(&self, result: &Result<T, CloudSyncError>) -> bool {
        self.0.stopping.load(Ordering::Acquire) && matches!(result, Err(CloudSyncError::Cancelled))
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self
                .registry
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.operations.remove(&self.id);
            if state.operations.is_empty() {
                std::mem::take(&mut state.waiters)
            } else {
                Vec::new()
            }
        };
        for waiter in waiters {
            let _ = waiter.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CloudSyncOperationRegistry;
    use crate::blocking_jobs::{BlockingJobScheduler, JobExecutionError};
    use crate::features::sync::operation::await_cloud_sync_job;
    use futures::FutureExt as _;
    use nyaterm_core::CloudSyncError;
    use nyaterm_core::cloud_sync::control::{CloudSyncCancellation, CloudSyncOperationControl};
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    #[test]
    fn shutdown_cancels_preparation_and_cleanup_and_invalidates_pull_permission() {
        let registry = CloudSyncOperationRegistry::default();
        let sync = CloudSyncOperationControl::default();
        let cleanup = CloudSyncOperationControl::default();
        let expired = Arc::new(AtomicBool::new(false));
        let registration = registry
            .register(
                "auto",
                vec![sync.clone(), cleanup.clone()],
                Some(expired.clone()),
            )
            .unwrap();
        registry.pause_and_cancel();
        registry.pause_and_cancel();
        assert!(expired.load(Ordering::Acquire));
        assert!(matches!(
            sync.begin_commit(),
            Err(CloudSyncError::Cancelled)
        ));
        assert!(matches!(
            cleanup.checkpoint(),
            Err(CloudSyncError::Cancelled)
        ));
        assert!(registry.register("new", vec![], None).is_none());
        assert!(registration.shutdown_cancelled::<()>(&Err(CloudSyncError::Cancelled)));
        drop(registration);
        futures::executor::block_on(registry.wait_for_idle());
    }

    #[test]
    fn history_and_all_waiters_finish_before_the_shutdown_barrier() {
        let registry = CloudSyncOperationRegistry::default();
        let registration = registry.register("sync", vec![], None).unwrap();
        let retained = registration.clone();
        registry.pause_and_cancel();
        let mut first = Box::pin(registry.wait_for_idle());
        let mut second = Box::pin(registry.wait_for_idle());
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert!(second.as_mut().poll(&mut cx).is_pending());
        let history = registry.followup("history");
        drop(registration);
        drop(retained);
        assert!(first.as_mut().poll(&mut cx).is_pending());
        drop(history);
        assert!(first.as_mut().poll(&mut cx).is_ready());
        assert!(second.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn failed_exit_resumes_admission_without_recancelling_new_operations() {
        let registry = CloudSyncOperationRegistry::default();
        registry.pause_and_cancel();
        registry.resume();
        assert!(registry.accepting());
        let control = CloudSyncOperationControl::default();
        let registration = registry
            .register("retry", vec![control.clone()], None)
            .unwrap();
        assert!(!registration.shutdown_cancelled::<()>(&Err(CloudSyncError::Cancelled)));
        control.begin_commit().unwrap();
        registry.pause_and_cancel();
        assert_eq!(
            control.request_cancel(),
            CloudSyncCancellation::FinishingCommit
        );
        assert!(!registration.shutdown_cancelled(&Ok(())));
        assert!(
            !registration
                .shutdown_cancelled::<()>(&Err(CloudSyncError::Remote("commit failed".into())))
        );
    }

    #[test]
    fn queued_sync_does_not_mutate_after_exit_is_requested() {
        let registry = CloudSyncOperationRegistry::default();
        let scheduler = BlockingJobScheduler::with_limits(1, 2).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        scheduler
            .submit_detached("occupied", move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let control = CloudSyncOperationControl::default();
        let registration = registry
            .register("queued", vec![control.clone()], None)
            .unwrap();
        let worker = control.clone();
        let task = scheduler.submit_task("sync", move |_| worker.begin_commit());
        registry.pause_and_cancel();
        release_tx.send(()).unwrap();
        let result = futures::executor::block_on(await_cloud_sync_job(
            task,
            std::future::pending(),
            control,
            |_| {},
        ));
        assert!(registration.shutdown_cancelled(&result));
        drop(registration);
        futures::executor::block_on(registry.wait_for_idle());
        scheduler.shutdown();
    }

    #[test]
    fn committed_result_and_history_survive_until_scheduler_shutdown() {
        let registry = CloudSyncOperationRegistry::default();
        let scheduler = BlockingJobScheduler::with_limits(1, 2).unwrap();
        let control = CloudSyncOperationControl::default();
        let registration = registry
            .register("commit", vec![control.clone()], None)
            .unwrap();
        let worker = control.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = scheduler.submit_task("commit", move |_| {
            worker.begin_commit()?;
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            worker.checkpoint()?;
            Ok(42)
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        registry.pause_and_cancel();
        let mut barrier = Box::pin(registry.wait_for_idle());
        let waker = futures::task::noop_waker();
        assert!(matches!(
            barrier.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        release_tx.send(()).unwrap();
        let result = futures::executor::block_on(await_cloud_sync_job(
            task,
            std::future::pending(),
            control,
            |_| {},
        ));
        assert_eq!(result.unwrap(), 42);
        let history = registry.followup("history");
        drop(registration);
        assert!(
            barrier
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        let write = scheduler.submit_task("history", |_| "success").unwrap();
        assert_eq!(futures::executor::block_on(write), Ok("success"));
        drop(history);
        futures::executor::block_on(barrier);
        // Only now can scheduler cancellation safely discard queued task results.
        scheduler.begin_shutdown()();
    }

    #[test]
    fn rejected_and_panicked_jobs_release_their_registrations() {
        let registry = CloudSyncOperationRegistry::default();
        let scheduler = BlockingJobScheduler::with_limits(1, 1).unwrap();
        let registration = registry.register("panic", vec![], None).unwrap();
        let task = scheduler
            .submit_task::<()>("panic", |_| panic!("test worker panic"))
            .unwrap();
        assert!(matches!(
            futures::executor::block_on(task),
            Err(JobExecutionError::Failed(_))
        ));
        drop(registration);
        scheduler.shutdown();
        let registration = registry.register("rejected", vec![], None).unwrap();
        assert!(scheduler.submit_task("rejected", |_| ()).is_err());
        drop(registration);
        assert!(registry.wait_for_idle().now_or_never().is_some());
    }
}
