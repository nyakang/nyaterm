//! Compatibility facade; execution is owned by nyaterm-executor.

pub use nyaterm_executor::blocking::{
    BlockingJobScheduler, CancellationToken, JobCancellationHandle, JobExecutionError, JobFailure,
    JobHandle, JobOutcome, JobRejected, JobTask, SchedulerMetrics, SchedulerStartError,
};
