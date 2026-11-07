//! Process-owned executor for native AI network tasks.
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Builder, Runtime};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct AiRuntime(Arc<RuntimeOwner>);
struct RuntimeOwner(Option<Runtime>);
impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}
impl AiRuntime {
    pub(crate) fn new() -> Result<Self, String> {
        Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nyaterm-ai")
            .enable_all()
            .build()
            .map(|runtime| Self(Arc::new(RuntimeOwner(Some(runtime)))))
            .map_err(|_| "Cannot start AI runtime".into())
    }
    pub(crate) fn spawn(
        &self,
        cancel: CancellationToken,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> AiTask {
        let task_cancel = cancel.clone();
        let task = self
            .0
            .0
            .as_ref()
            .expect("runtime lifetime")
            .spawn(async move {
                tokio::select! { biased; _ = task_cancel.cancelled() => {}, _ = future => {} }
            });
        AiTask {
            cancel,
            abort: task.abort_handle(),
        }
    }
}

/// The authoritative conversation owns this guard, including while offscreen.
pub(crate) struct AiTask {
    cancel: CancellationToken,
    abort: AbortHandle,
}
impl AiTask {
    pub(crate) fn is_finished(&self) -> bool {
        self.abort.is_finished()
    }
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
        self.abort.abort();
    }
}
impl Drop for AiTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::AiRuntime;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn dropping_conversation_guard_reclaims_network_future() {
        struct DropSignal(Option<oneshot::Sender<()>>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        let runtime = AiRuntime::new().unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let task = runtime.spawn(CancellationToken::new(), async move {
            let _signal = DropSignal(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx.await.unwrap();
        drop(task);
        tokio::time::timeout(std::time::Duration::from_secs(2), dropped_rx)
            .await
            .unwrap()
            .unwrap();
    }
}
