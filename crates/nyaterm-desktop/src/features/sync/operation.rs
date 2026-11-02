use futures::future::{Either, select};
use nyaterm_core::CloudSyncError;
use nyaterm_core::cloud_sync::control::{CloudSyncCancellation, CloudSyncOperationControl};

use crate::blocking_jobs::{JobRejected, JobTask};

mod registry;
pub(crate) use registry::{CloudSyncOperationRegistration, CloudSyncOperationRegistry};

#[derive(Clone, Default)]
pub(crate) struct AutoSyncOperationControls {
    pub sync: CloudSyncOperationControl,
    // Cleanup is optional and can stop even after the sync itself committed.
    pub cleanup: CloudSyncOperationControl,
}

pub(crate) fn lock_cloud_sync_operation(
    control: &CloudSyncOperationControl,
) -> Result<std::sync::MutexGuard<'static, ()>, CloudSyncError> {
    loop {
        control.checkpoint()?;
        match crate::features::cloud_sync_operation_lock().try_lock() {
            Ok(guard) => {
                control.checkpoint()?;
                return Ok(guard);
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                control.checkpoint()?;
                return Ok(error.into_inner());
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    }
}

pub(crate) async fn await_cloud_sync_job<T>(
    task: Result<JobTask<Result<T, CloudSyncError>>, JobRejected>,
    timeout: impl std::future::Future<Output = ()>,
    control: CloudSyncOperationControl,
    on_timeout: impl FnOnce(CloudSyncCancellation),
) -> Result<T, CloudSyncError> {
    let task = task.map_err(|error| CloudSyncError::Remote(error.to_string()))?;
    let result = match select(Box::pin(task), Box::pin(timeout)).await {
        Either::Left((result, _)) => result,
        Either::Right((_, task)) => {
            on_timeout(control.request_cancel());
            // Keep the operation busy until it really exits. In particular, do
            // not cancel JobTask: that discards the result of an in-flight commit.
            task.await
        }
    };
    result.map_err(|error| CloudSyncError::Remote(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::{AutoSyncOperationControls, await_cloud_sync_job, lock_cloud_sync_operation};
    use crate::blocking_jobs::BlockingJobScheduler;
    use nyaterm_core::CloudSyncError;
    use nyaterm_core::cloud_sync::control::{CloudSyncCancellation, CloudSyncOperationControl};
    use std::future::Future;
    use std::sync::mpsc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    #[test]
    fn timeout_requests_stop_but_waits_for_the_worker_to_exit() {
        let scheduler = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let control = CloudSyncOperationControl::default();
        let worker_control = control.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = scheduler.submit_task("sync", move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            worker_control.begin_commit()
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut waiting = Box::pin(await_cloud_sync_job(task, async {}, control, |phase| {
            assert_eq!(phase, CloudSyncCancellation::Stopping);
        }));
        let waker = futures::task::noop_waker();
        assert!(matches!(
            waiting.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        release_tx.send(()).unwrap();
        let result = futures::executor::block_on(waiting);
        assert!(matches!(result, Err(CloudSyncError::Cancelled)));
        scheduler.shutdown();
    }

    #[test]
    fn timeout_during_commit_preserves_final_success_and_state() {
        let scheduler = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let control = CloudSyncOperationControl::default();
        let worker_control = control.clone();
        let (committing_tx, committing_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = scheduler.submit_task("sync", move |_| {
            worker_control.begin_commit()?;
            committing_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            worker_control.checkpoint()?;
            Ok(42)
        });
        committing_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut waiting = Box::pin(await_cloud_sync_job(task, async {}, control, |phase| {
            assert_eq!(phase, CloudSyncCancellation::FinishingCommit);
        }));
        let waker = futures::task::noop_waker();
        assert!(matches!(
            waiting.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        release_tx.send(()).unwrap();
        let result = futures::executor::block_on(waiting);
        assert_eq!(result.unwrap(), 42);
        scheduler.shutdown();
    }

    #[test]
    fn cancelled_sync_waiting_for_another_window_never_acquires_the_lock() {
        let guard = crate::features::cloud_sync_operation_lock().lock().unwrap();
        let control = CloudSyncOperationControl::default();
        let worker_control = control.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = crate::thread_owner::spawn_joinable("sync-lock-cancel-test", move || {
            let result = lock_cloud_sync_operation(&worker_control).map(|_| ());
            result_tx.send(result).unwrap();
        })
        .expect("spawn lock waiter");
        control.request_cancel();
        let result = result_rx.recv_timeout(Duration::from_secs(5));
        drop(guard);
        worker.join().unwrap();
        assert!(matches!(result.unwrap(), Err(CloudSyncError::Cancelled)));
    }

    #[test]
    fn deadline_while_queued_cancels_before_mutation() {
        let scheduler = BlockingJobScheduler::with_limits(1, 4).unwrap();
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
        let worker_control = control.clone();
        let task = scheduler.submit_task("queued-sync", move |_| worker_control.begin_commit());
        let result =
            futures::executor::block_on(await_cloud_sync_job(task, async {}, control, |_| {
                release_tx.send(()).unwrap()
            }));
        assert!(matches!(result, Err(CloudSyncError::Cancelled)));
        scheduler.shutdown();
    }

    #[test]
    fn auto_sync_timeout_can_stop_cleanup_after_sync_commit() {
        let controls = AutoSyncOperationControls::default();
        controls.sync.begin_commit().unwrap();
        assert_eq!(
            controls.sync.request_cancel(),
            CloudSyncCancellation::FinishingCommit
        );
        assert_eq!(
            controls.cleanup.request_cancel(),
            CloudSyncCancellation::Stopping
        );
        assert!(controls.sync.checkpoint().is_ok());
        assert!(matches!(
            controls.cleanup.begin_commit(),
            Err(CloudSyncError::Cancelled)
        ));
    }
}
