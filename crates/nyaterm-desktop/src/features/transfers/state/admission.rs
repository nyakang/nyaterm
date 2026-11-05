use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobResult, TransferJobState, TransferJobStatus,
};
use crate::thread_owner::spawn_joinable;
use futures::channel::mpsc::UnboundedSender;
use std::collections::{HashMap, HashSet, VecDeque};
use std::thread::JoinHandle;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
enum Direction {
    Download,
    Upload,
    Copy,
    Delete,
}

impl Direction {
    fn of(kind: &TransferJobKind) -> Option<Self> {
        match kind {
            TransferJobKind::Download { .. } => Some(Self::Download),
            TransferJobKind::Upload { .. } => Some(Self::Upload),
            TransferJobKind::SendTo { .. } => Some(Self::Copy),
            TransferJobKind::Delete { .. } => Some(Self::Delete),
            _ => None,
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Download => 0,
            Self::Upload => 1,
            Self::Copy => 2,
            Self::Delete => 3,
        }
    }
}

struct Task {
    direction: Direction,
    name: &'static str,
    tx: UnboundedSender<TransferJobResult>,
    work: Option<Box<dyn FnOnce() + Send>>,
    thread: Option<JoinHandle<()>>,
    // A cancelling worker retains only the quota it held before cancellation;
    // cancelling a paused worker must not reacquire the released quota.
    admitted: bool,
}

/// Owns admission and worker lifetimes. Row state is the only authority for
/// whether an admitted worker is running, paused, or awaiting readmission.
#[derive(Default)]
pub(super) struct TransferAdmission {
    order: VecDeque<String>,
    tasks: HashMap<String, Task>,
}

impl TransferAdmission {
    pub(super) fn manages(&self, id: &str) -> bool {
        self.tasks.contains_key(id)
    }

    pub(super) fn enqueue(
        &mut self,
        job: &mut TransferJobState,
        name: &'static str,
        tx: UnboundedSender<TransferJobResult>,
        work: Box<dyn FnOnce() + Send>,
    ) {
        let direction = Direction::of(&job.kind).expect("bulk transfer direction");
        self.enqueue_in_direction(job, direction, name, tx, work);
    }

    pub(super) fn enqueue_delete(
        &mut self,
        job: &mut TransferJobState,
        name: &'static str,
        tx: UnboundedSender<TransferJobResult>,
        work: Box<dyn FnOnce() + Send>,
    ) {
        self.enqueue_in_direction(job, Direction::Delete, name, tx, work);
    }

    fn enqueue_in_direction(
        &mut self,
        job: &mut TransferJobState,
        direction: Direction,
        name: &'static str,
        tx: UnboundedSender<TransferJobResult>,
        work: Box<dyn FnOnce() + Send>,
    ) {
        job.status = TransferJobStatus::Queued;
        job.detail = "Queued".to_string();
        self.order.push_back(job.id.clone());
        self.tasks.insert(
            job.id.clone(),
            Task {
                direction,
                name,
                tx,
                work: Some(work),
                thread: None,
                admitted: false,
            },
        );
    }

    pub(super) fn supports(kind: &TransferJobKind) -> bool {
        Direction::of(kind).is_some()
    }

    pub(super) fn reconcile(
        &mut self,
        jobs: &mut [TransferJobState],
        download: u32,
        upload: u32,
    ) -> Vec<JoinHandle<()>> {
        self.reconcile_with_spawn(jobs, download, upload, |name, work| {
            spawn_joinable(name, work)
        })
    }

    fn reconcile_with_spawn(
        &mut self,
        jobs: &mut [TransferJobState],
        download: u32,
        upload: u32,
        mut spawn: impl FnMut(&'static str, Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>,
    ) -> Vec<JoinHandle<()>> {
        let limit = |value: u32| value.clamp(1, 10).saturating_sub(1).max(1) as usize;
        let limits = [
            limit(download),
            limit(upload),
            limit(download).min(limit(upload)),
            // All delete entry points share this quota, independently of transfer settings.
            2,
        ];
        let positions: HashMap<_, _> = jobs
            .iter()
            .enumerate()
            .map(|(index, job)| (job.id.clone(), index))
            .collect();
        let mut retired = Vec::new();
        let mut remove = Vec::new();
        let mut running = [0_usize; 4];
        for (id, task) in &mut self.tasks {
            let job = positions.get(id).map(|&index| &mut jobs[index]);
            match job {
                Some(job)
                    if matches!(
                        job.status,
                        TransferJobStatus::Queued
                            | TransferJobStatus::Paused
                            | TransferJobStatus::Running
                            | TransferJobStatus::Cancelling
                    ) =>
                {
                    if task.work.is_some()
                        && (job.status == TransferJobStatus::Cancelling
                            || job
                                .control
                                .as_ref()
                                .is_some_and(|control| control.is_cancelled()))
                    {
                        // Remain non-retryable until the terminal event is consumed;
                        // otherwise that event could cancel a retry using the same id.
                        job.status = TransferJobStatus::Cancelling;
                        job.detail = "Cancelling".to_string();
                        job.control = None;
                        let _ = task.tx.unbounded_send(TransferJobResult {
                            id: id.clone(),
                            event: TransferJobEvent::Finished(Err(
                                nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED.into(),
                            )),
                        });
                        remove.push(id.clone());
                    } else {
                        if matches!(
                            job.status,
                            TransferJobStatus::Paused | TransferJobStatus::Queued
                        ) {
                            task.admitted = false;
                        }
                        if task.admitted {
                            running[task.direction.index()] += 1;
                        }
                    }
                }
                _ => remove.push(id.clone()),
            }
        }
        for id in remove {
            if let Some(mut task) = self.tasks.remove(&id)
                && let Some(thread) = task.thread.take()
            {
                retired.push(thread);
            }
        }
        // Preserve row order when several paused jobs are resumed together.
        let readmissions = {
            let queued: HashSet<_> = self.order.iter().map(String::as_str).collect();
            jobs.iter()
                .filter(|job| {
                    job.status == TransferJobStatus::Queued
                        && self.tasks.contains_key(&job.id)
                        && !queued.contains(job.id.as_str())
                })
                .map(|job| job.id.clone())
                .collect::<Vec<_>>()
        };
        self.order.extend(readmissions);
        // One pass admits FIFO within each direction without blocking other directions.
        for _ in 0..self.order.len() {
            let id = self.order.pop_front().expect("queued id");
            let Some(task) = self.tasks.get_mut(&id) else {
                continue;
            };
            let Some(&index) = positions.get(&id) else {
                continue;
            };
            let job = &mut jobs[index];
            if job.status != TransferJobStatus::Queued {
                continue;
            }
            let direction = task.direction.index();
            if running[direction] >= limits[direction] {
                self.order.push_back(id);
                continue;
            }
            task.admitted = true;
            job.status = TransferJobStatus::Running;
            job.detail = if matches!(task.direction, Direction::Delete) {
                rust_i18n::t!(
                    "fileTransfer.detailDeleting",
                    path = job.display_name.clone()
                )
                .to_string()
            } else {
                "Transferring".to_string()
            };
            if let Some(control) = &job.control {
                control.resume();
            }
            running[direction] += 1;
            if let Some(work) = task.work.take() {
                let tx = task.tx.clone();
                let failure_id = id.clone();
                let control = job.control.clone();
                // A paused transfer must not occupy a shared blocking-pool slot:
                // its operation retains its runtime and can be readmitted later.
                match spawn(
                    task.name,
                    Box::new(move || {
                        // Cancellation may arrive after admission but before the worker starts.
                        // Once work starts, its own result remains authoritative.
                        if control
                            .as_ref()
                            .is_some_and(|control| control.is_cancelled())
                        {
                            let _ = tx.unbounded_send(TransferJobResult {
                                id: failure_id,
                                event: TransferJobEvent::Finished(Err(
                                    nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED.into(),
                                )),
                            });
                            return;
                        }
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
                            let _ = tx.unbounded_send(TransferJobResult {
                                id: failure_id,
                                event: TransferJobEvent::Finished(Err(
                                    "transfer worker panicked".into()
                                )),
                            });
                        }
                    }),
                ) {
                    Ok(thread) => task.thread = Some(thread),
                    Err(_) => {
                        let _ = task.tx.unbounded_send(TransferJobResult {
                            id,
                            event: TransferJobEvent::Finished(Err(
                                "failed to start transfer worker".into(),
                            )),
                        });
                    }
                }
            }
        }
        retired
    }

    pub(super) fn shutdown(&mut self) -> Vec<JoinHandle<()>> {
        self.order.clear();
        self.tasks
            .drain()
            .filter_map(|(_, task)| task.thread)
            .collect()
    }
}
