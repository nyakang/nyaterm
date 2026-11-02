//! Bounded execution for short-lived blocking desktop work.
//!
//! Jobs must not capture GPUI state. They return to feature owners through the
//! feature's typed event channel. Long-lived protocol and session workers keep
//! dedicated, explicitly owned threads instead of occupying this pool.

use std::fmt;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use futures::channel::oneshot;

const DEFAULT_QUEUE_CAPACITY: usize = 256;
const MIN_WORKERS: usize = 2;
const MAX_WORKERS: usize = 8;

#[derive(Clone)]
pub struct CancellationToken {
    job_cancelled: Arc<AtomicBool>,
    scheduler_stopping: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.job_cancelled.load(Ordering::Acquire)
            || self.scheduler_stopping.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFailure {
    Panicked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Completed,
    Cancelled,
    Failed(JobFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobExecutionError {
    Cancelled,
    Failed(JobFailure),
}

impl fmt::Display for JobExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("blocking job was cancelled"),
            Self::Failed(JobFailure::Panicked) => formatter.write_str("blocking job panicked"),
        }
    }
}

impl std::error::Error for JobExecutionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobRejected {
    QueueFull,
    ShuttingDown,
}

impl fmt::Display for JobRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => formatter.write_str("blocking job queue is full"),
            Self::ShuttingDown => formatter.write_str("blocking job scheduler is shutting down"),
        }
    }
}

impl std::error::Error for JobRejected {}

#[derive(Debug)]
pub struct SchedulerStartError {
    worker_index: usize,
    source: std::io::Error,
}

impl fmt::Display for SchedulerStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to start blocking worker {}: {}",
            self.worker_index, self.source
        )
    }
}

impl std::error::Error for SchedulerStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub struct JobHandle {
    id: u64,
    cancelled: Arc<AtomicBool>,
    outcome_rx: mpsc::Receiver<JobOutcome>,
}

pub struct JobTask<T> {
    handle: JobHandle,
    result_rx: oneshot::Receiver<Result<T, JobExecutionError>>,
}

impl<T> JobTask<T> {
    pub fn id(&self) -> u64 {
        self.handle.id()
    }

    pub fn cancel(&self) {
        self.handle.cancel();
    }
}

impl<T> Future for JobTask<T> {
    type Output = Result<T, JobExecutionError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.result_rx).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(JobExecutionError::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl JobHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn try_outcome(&self) -> Result<JobOutcome, mpsc::TryRecvError> {
        self.outcome_rx.try_recv()
    }

    pub fn wait(self) -> JobOutcome {
        self.outcome_rx.recv().unwrap_or(JobOutcome::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerMetrics {
    pub queue_depth: usize,
    pub submitted: u64,
    pub rejected: u64,
    pub panicked: u64,
    pub shutdown_millis: u64,
}

type JobFn = Box<dyn FnOnce(CancellationToken) + Send + 'static>;
type OutcomeFn = Box<dyn FnOnce(JobOutcome) + Send + 'static>;
type TaskResultSender<T> = Arc<Mutex<Option<oneshot::Sender<Result<T, JobExecutionError>>>>>;

struct QueuedJob {
    id: u64,
    name: &'static str,
    cancelled: Arc<AtomicBool>,
    run: JobFn,
    outcome_tx: mpsc::SyncSender<JobOutcome>,
    on_outcome: Option<OutcomeFn>,
}

struct SchedulerShared {
    stopping: Arc<AtomicBool>,
    queue_depth: AtomicUsize,
    submitted: AtomicU64,
    rejected: AtomicU64,
    panicked: AtomicU64,
    shutdown_millis: AtomicU64,
}

impl SchedulerShared {
    fn metrics(&self) -> SchedulerMetrics {
        SchedulerMetrics {
            queue_depth: self.queue_depth.load(Ordering::Acquire),
            submitted: self.submitted.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            panicked: self.panicked.load(Ordering::Relaxed),
            shutdown_millis: self.shutdown_millis.load(Ordering::Relaxed),
        }
    }
}

struct SchedulerInner {
    sender: Mutex<Option<mpsc::SyncSender<QueuedJob>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    shared: Arc<SchedulerShared>,
    next_job_id: AtomicU64,
}

impl SchedulerInner {
    fn begin_shutdown(&self) -> impl FnOnce() + Send + 'static + use<> {
        let shared = Arc::clone(&self.shared);
        let started_at = Instant::now();
        let workers = self.take_shutdown_workers();
        move || {
            if workers.is_empty() {
                return;
            }
            for worker in workers {
                if worker.join().is_err() {
                    shared.panicked.fetch_add(1, Ordering::Relaxed);
                }
            }
            let elapsed = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            shared.shutdown_millis.store(elapsed, Ordering::Relaxed);
        }
    }

    fn take_shutdown_workers(&self) -> Vec<JoinHandle<()>> {
        if self.shared.stopping.swap(true, Ordering::AcqRel) {
            return Vec::new();
        }
        self.sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        std::mem::take(
            &mut *self
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn shutdown(&self) {
        self.begin_shutdown()();
    }
}

impl Drop for SchedulerInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
pub struct BlockingJobScheduler {
    inner: Arc<SchedulerInner>,
}

impl BlockingJobScheduler {
    pub fn new() -> Self {
        Self::try_new().expect("start NyaTerm blocking job scheduler")
    }

    pub fn try_new() -> Result<Self, SchedulerStartError> {
        let workers = thread::available_parallelism()
            .map(|parallelism| parallelism.get())
            .unwrap_or(MIN_WORKERS)
            .clamp(MIN_WORKERS, MAX_WORKERS);
        Self::with_limits(workers, DEFAULT_QUEUE_CAPACITY)
    }

    pub(crate) fn with_limits(
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Self, SchedulerStartError> {
        assert!(worker_count > 0, "blocking scheduler needs a worker");
        assert!(
            queue_capacity > 0,
            "blocking scheduler needs queue capacity"
        );
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let shared = Arc::new(SchedulerShared {
            stopping: Arc::new(AtomicBool::new(false)),
            queue_depth: AtomicUsize::new(0),
            submitted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            panicked: AtomicU64::new(0),
            shutdown_millis: AtomicU64::new(0),
        });
        let mut workers = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let worker_shared = Arc::clone(&shared);
            match thread::Builder::new()
                .name(format!("nyaterm-blocking-{worker_index}"))
                .spawn(move || worker_loop(receiver, worker_shared))
            {
                Ok(worker) => workers.push(worker),
                Err(source) => {
                    shared.stopping.store(true, Ordering::Release);
                    drop(sender);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(SchedulerStartError {
                        worker_index,
                        source,
                    });
                }
            }
        }
        Ok(Self {
            inner: Arc::new(SchedulerInner {
                sender: Mutex::new(Some(sender)),
                workers: Mutex::new(workers),
                shared,
                next_job_id: AtomicU64::new(1),
            }),
        })
    }

    pub fn submit(
        &self,
        name: &'static str,
        run: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> Result<JobHandle, JobRejected> {
        self.submit_inner(name, Box::new(run), None)
    }

    fn submit_inner(
        &self,
        name: &'static str,
        run: JobFn,
        on_outcome: Option<OutcomeFn>,
    ) -> Result<JobHandle, JobRejected> {
        if self.inner.shared.stopping.load(Ordering::Acquire) {
            self.inner.shared.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(JobRejected::ShuttingDown);
        }
        let id = self.inner.next_job_id.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (outcome_tx, outcome_rx) = mpsc::sync_channel(1);
        let job = QueuedJob {
            id,
            name,
            cancelled: Arc::clone(&cancelled),
            run,
            outcome_tx,
            on_outcome,
        };
        let Some(sender) = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            self.inner.shared.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(JobRejected::ShuttingDown);
        };
        self.inner.shared.queue_depth.fetch_add(1, Ordering::AcqRel);
        match sender.try_send(job) {
            Ok(()) => {
                self.inner.shared.submitted.fetch_add(1, Ordering::Relaxed);
                Ok(JobHandle {
                    id,
                    cancelled,
                    outcome_rx,
                })
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.inner.shared.queue_depth.fetch_sub(1, Ordering::AcqRel);
                self.inner.shared.rejected.fetch_add(1, Ordering::Relaxed);
                Err(JobRejected::QueueFull)
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.inner.shared.queue_depth.fetch_sub(1, Ordering::AcqRel);
                self.inner.shared.rejected.fetch_add(1, Ordering::Relaxed);
                Err(JobRejected::ShuttingDown)
            }
        }
    }

    pub fn submit_detached(
        &self,
        name: &'static str,
        run: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> Result<(), JobRejected> {
        self.submit(name, run).map(drop)
    }

    pub(crate) fn submit_with_outcome(
        &self,
        name: &'static str,
        run: impl FnOnce(CancellationToken) + Send + 'static,
        on_outcome: impl FnOnce(JobOutcome) + Send + 'static,
    ) -> Result<(), JobRejected> {
        self.submit_inner(name, Box::new(run), Some(Box::new(on_outcome)))
            .map(drop)
    }

    pub fn submit_task<T: Send + 'static>(
        &self,
        name: &'static str,
        run: impl FnOnce(CancellationToken) -> T + Send + 'static,
    ) -> Result<JobTask<T>, JobRejected> {
        let (result_tx, result_rx) = oneshot::channel();
        let result_tx = Arc::new(Mutex::new(Some(result_tx)));
        let run_result_tx = Arc::clone(&result_tx);
        let run = Box::new(move |cancel: CancellationToken| {
            let result = run(cancel.clone());
            let result = if cancel.is_cancelled() {
                Err(JobExecutionError::Cancelled)
            } else {
                Ok(result)
            };
            send_task_result(&run_result_tx, result);
        });
        let on_outcome = Box::new(move |outcome| {
            let error = match outcome {
                JobOutcome::Failed(failure) => JobExecutionError::Failed(failure),
                JobOutcome::Completed | JobOutcome::Cancelled => JobExecutionError::Cancelled,
            };
            send_task_result(&result_tx, Err(error));
        });
        let handle = self.submit_inner(name, run, Some(on_outcome))?;
        Ok(JobTask { handle, result_rx })
    }

    pub fn metrics(&self) -> SchedulerMetrics {
        self.inner.shared.metrics()
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// Cancel queued/running jobs now, then wait on a background executor.
    #[must_use = "the returned cleanup must run before releasing workspace resources"]
    pub(crate) fn begin_shutdown(&self) -> impl FnOnce() + Send + 'static + use<> {
        self.inner.begin_shutdown()
    }
}

impl Default for BlockingJobScheduler {
    fn default() -> Self {
        Self::new()
    }
}

fn worker_loop(receiver: Arc<Mutex<mpsc::Receiver<QueuedJob>>>, shared: Arc<SchedulerShared>) {
    loop {
        let received = receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv();
        let Ok(job) = received else {
            break;
        };
        shared.queue_depth.fetch_sub(1, Ordering::AcqRel);
        let token = CancellationToken {
            job_cancelled: Arc::clone(&job.cancelled),
            scheduler_stopping: Arc::clone(&shared.stopping),
        };
        if token.is_cancelled() {
            let _ = job.outcome_tx.send(JobOutcome::Cancelled);
            if let Some(on_outcome) = job.on_outcome {
                on_outcome(JobOutcome::Cancelled);
            }
            continue;
        }
        let QueuedJob {
            id,
            name,
            run,
            outcome_tx,
            on_outcome,
            ..
        } = job;
        let _job_identity = (id, name);
        let outcome = match catch_unwind(AssertUnwindSafe(|| run(token.clone()))) {
            Ok(()) if token.is_cancelled() => JobOutcome::Cancelled,
            Ok(()) => JobOutcome::Completed,
            Err(_) => {
                shared.panicked.fetch_add(1, Ordering::Relaxed);
                JobOutcome::Failed(JobFailure::Panicked)
            }
        };
        let _ = outcome_tx.send(outcome);
        if let Some(on_outcome) = on_outcome {
            on_outcome(outcome);
        }
    }
}

fn send_task_result<T>(result_tx: &TaskResultSender<T>, result: Result<T, JobExecutionError>) {
    if let Some(result_tx) = result_tx
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        let _ = result_tx.send(result);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{
        BlockingJobScheduler, JobExecutionError, JobFailure, JobOutcome, JobRejected,
        SchedulerMetrics,
    };

    #[test]
    fn full_queue_rejects_without_running_the_extra_job() {
        let scheduler = BlockingJobScheduler::with_limits(1, 1).expect("scheduler");
        let (release_tx, release_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let first = scheduler
            .submit("test-blocking", move |_| {
                started_tx.send(()).expect("report started");
                release_rx.recv().expect("release worker");
            })
            .expect("first job");
        started_rx.recv().expect("first job started");
        let second = scheduler.submit("test-queued", |_| {}).expect("queued job");
        assert!(matches!(
            scheduler.submit("test-rejected", |_| {}),
            Err(JobRejected::QueueFull)
        ));
        release_tx.send(()).expect("release first job");
        assert_eq!(first.wait(), JobOutcome::Completed);
        assert_eq!(second.wait(), JobOutcome::Completed);
        assert_eq!(scheduler.metrics().rejected, 1);
        scheduler.shutdown();
    }

    #[test]
    fn queued_and_running_jobs_observe_cooperative_cancellation() {
        let scheduler = BlockingJobScheduler::with_limits(1, 2).expect("scheduler");
        let (started_tx, started_rx) = mpsc::channel();
        let running = scheduler
            .submit("test-running-cancel", move |cancel| {
                started_tx.send(()).expect("report started");
                while !cancel.is_cancelled() {
                    std::thread::yield_now();
                }
            })
            .expect("running job");
        started_rx.recv().expect("running job started");
        let queued = scheduler
            .submit("test-queued-cancel", |_| panic!("cancelled job ran"))
            .expect("queued job");
        queued.cancel();
        running.cancel();
        assert_eq!(running.wait(), JobOutcome::Cancelled);
        assert_eq!(queued.wait(), JobOutcome::Cancelled);
        scheduler.shutdown();
    }

    #[test]
    fn panics_become_typed_failures_and_workers_keep_running() {
        let scheduler = BlockingJobScheduler::with_limits(1, 2).expect("scheduler");
        let panicked = scheduler
            .submit("test-panic", |_| panic!("expected test panic"))
            .expect("panic job");
        assert_eq!(panicked.wait(), JobOutcome::Failed(JobFailure::Panicked));
        let after = scheduler
            .submit("test-after-panic", |_| {})
            .expect("next job");
        assert_eq!(after.wait(), JobOutcome::Completed);
        assert_eq!(scheduler.metrics().panicked, 1);
        scheduler.shutdown();
    }

    #[test]
    fn task_returns_typed_result_and_converts_panics() {
        let scheduler = BlockingJobScheduler::with_limits(1, 2).expect("scheduler");
        let completed = scheduler
            .submit_task("test-result", |_| 42)
            .expect("result job");
        assert_eq!(futures::executor::block_on(completed), Ok(42));

        let panicked = scheduler
            .submit_task::<usize>("test-result-panic", |_| panic!("expected test panic"))
            .expect("panic job");
        assert_eq!(
            futures::executor::block_on(panicked),
            Err(JobExecutionError::Failed(JobFailure::Panicked))
        );
        scheduler.shutdown();
    }

    #[test]
    fn beginning_shutdown_cancels_jobs_without_waiting_on_the_caller() {
        let scheduler = BlockingJobScheduler::with_limits(1, 1).expect("scheduler");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let job = scheduler
            .submit("held-http-request", move |cancel| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                assert!(cancel.is_cancelled());
            })
            .unwrap();
        started_rx.recv().unwrap();
        let cleanup = scheduler.begin_shutdown();
        assert!(matches!(
            scheduler.submit("after-shutdown", |_| {}),
            Err(JobRejected::ShuttingDown)
        ));
        // The caller can continue serving UI events while an in-flight request finishes.
        release_tx.send(()).unwrap();
        std::thread::spawn(cleanup).join().unwrap();
        assert_eq!(job.wait(), JobOutcome::Cancelled);
    }

    #[test]
    fn shutdown_cancels_waiting_work_wakes_workers_and_joins() {
        let scheduler = BlockingJobScheduler::with_limits(2, 2).expect("scheduler");
        let (started_tx, started_rx) = mpsc::channel();
        let first = scheduler
            .submit("test-shutdown", move |cancel| {
                started_tx.send(()).expect("report started");
                while !cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .expect("shutdown job");
        started_rx.recv().expect("job started");
        scheduler.shutdown();
        assert_eq!(first.wait(), JobOutcome::Cancelled);
        assert!(matches!(
            scheduler.submit("after-shutdown", |_| {}),
            Err(JobRejected::ShuttingDown)
        ));
        assert!(scheduler.metrics().shutdown_millis < 5_000);
    }

    #[test]
    fn metrics_never_include_job_payloads() {
        let metrics = SchedulerMetrics::default();
        let debug = format!("{metrics:?}");
        assert!(debug.contains("queue_depth"));
        assert!(!debug.contains("payload"));
    }
}
