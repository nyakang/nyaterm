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
        let thread = crate::thread_owner::spawn_joinable("cloud-storage", move || {
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
