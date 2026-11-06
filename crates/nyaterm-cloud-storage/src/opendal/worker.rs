//! One joined worker/runtime per remote, reused for the entire sync operation.

use std::future::Future;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;

use nyaterm_core::CloudSyncError;
use tokio::runtime::{Builder, Runtime};

type Job<C> = Box<dyn FnOnce(&Runtime, &C) + Send>;

pub(super) struct Worker<C = ()> {
    sender: Option<SyncSender<Job<C>>>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(test)]
impl Worker<()> {
    pub(super) fn new() -> Result<Self, CloudSyncError> {
        Self::with_context(|| Ok(()))
    }
}

impl<C: Send + 'static> Worker<C> {
    pub(super) fn with_context(
        initialize: impl FnOnce() -> Result<C, CloudSyncError> + Send + 'static,
    ) -> Result<Self, CloudSyncError> {
        let (sender, receiver) = sync_channel::<Job<C>>(1);
        let (ready, initialized) = sync_channel(1);
        let thread = crate::thread::spawn_joinable("cloud-storage", move || {
            let runtime = match Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(_) => {
                    let _ = ready.send(Err(unavailable()));
                    return;
                }
            };
            // Blocking compatibility clients must be constructed, called and
            // dropped outside block_on, including when the caller is in Tokio.
            let context = match initialize() {
                Ok(context) => context,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            if ready.send(Ok(())).is_err() {
                return;
            }
            while let Ok(job) = receiver.recv() {
                job(&runtime, &context);
            }
            // Context and runtime are both destroyed on their owning thread.
        })
        .map_err(|_| unavailable())?;
        match initialized.recv() {
            Ok(Ok(())) => Ok(Self {
                sender: Some(sender),
                thread: Some(thread),
            }),
            result => {
                let _ = thread.join();
                Err(match result {
                    Ok(Err(error)) => error,
                    _ => unavailable(),
                })
            }
        }
    }

    fn submit<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Runtime, &C) -> Result<T, CloudSyncError> + Send + 'static,
    ) -> Result<T, CloudSyncError> {
        let (reply, result) = sync_channel(1);
        self.sender
            .as_ref()
            .ok_or_else(unavailable)?
            .send(Box::new(move |runtime, context| {
                let _ = reply.send(job(runtime, context));
            }))
            .map_err(|_| unavailable())?;
        result.recv().map_err(|_| unavailable())?
    }

    pub(super) fn run<T, F>(&self, future: F) -> Result<T, CloudSyncError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, CloudSyncError>> + Send + 'static,
    {
        self.submit(move |runtime, _| runtime.block_on(future))
    }

    pub(super) fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&C) -> Result<T, CloudSyncError> + Send + 'static,
    ) -> Result<T, CloudSyncError> {
        self.submit(move |_, context| operation(context))
    }
}

impl<C> Drop for Worker<C> {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn unavailable() -> CloudSyncError {
    CloudSyncError::Remote("cloud storage worker unavailable".into())
}

#[cfg(test)]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::sync::mpsc;
    use std::thread::{self, ThreadId};

    use super::Worker;

    struct OwnedFile {
        file: Option<File>,
        closed: mpsc::Sender<ThreadId>,
    }

    impl Drop for OwnedFile {
        fn drop(&mut self) {
            drop(self.file.take());
            self.closed.send(thread::current().id()).unwrap();
        }
    }

    #[test]
    fn dropping_worker_joins_its_thread_and_releases_thread_owned_file_handles() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker-owned");
        let worker_path = path.clone();
        let (closed, receiver) = mpsc::channel();
        let worker = Worker::with_context(move || {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt as _;
                options.share_mode(0);
            }
            Ok(OwnedFile {
                file: Some(options.open(worker_path).unwrap()),
                closed,
            })
        })
        .unwrap();
        let owner = worker.blocking(|_| Ok(thread::current().id())).unwrap();
        assert_ne!(owner, thread::current().id());
        #[cfg(windows)]
        assert!(std::fs::remove_file(&path).is_err());

        drop(worker);

        // Drop returns only after the owner's destructor and thread have exited.
        assert_eq!(receiver.try_recv().unwrap(), owner);
        std::fs::remove_file(path).unwrap();
    }
}
