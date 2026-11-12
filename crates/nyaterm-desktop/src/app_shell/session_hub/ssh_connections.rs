mod background;
pub(crate) use background::{BackgroundConnectionLease, BackgroundConnections};

use nyaterm_transport::SshMultiplexHandle;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::{self, JoinHandle};

/// Process-wide connection ownership. A lease belongs to a live session or a
/// pending start, and travels with that owner when its window changes.
#[derive(Clone)]
pub(crate) struct SshConnectionPool {
    inner: Arc<ConnectionPoolInner>,
}

struct ConnectionPoolInner {
    connections: Mutex<HashMap<String, Weak<ManagedConnection>>>,
    disconnects: Arc<DisconnectWorker>,
    background: Arc<BackgroundConnections>,
}

#[derive(Clone)]
pub(crate) struct SshConnectionLease {
    inner: Arc<ManagedConnection>,
}

struct ManagedConnection {
    // Only test probes omit the live handle; they exercise the same lease and
    // disconnect queue without requiring an external SSH server.
    handle: Option<SshMultiplexHandle>,
    docker_state: nyaterm_remote_ops::docker::DockerConnectionState,
    cleanup: Option<Box<dyn FnOnce() + Send + Sync>>,
    disconnects: Arc<DisconnectWorker>,
}

enum DisconnectJob {
    Disconnect(Box<dyn FnOnce() + Send>),
    Barrier(mpsc::Sender<()>),
}

struct DisconnectWorker {
    sender: Option<mpsc::Sender<DisconnectJob>>,
    worker: Option<JoinHandle<()>>,
}

impl Default for SshConnectionPool {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        let background = Arc::new(BackgroundConnections::default());
        let background_weak = Arc::downgrade(&background);
        let worker = thread::Builder::new()
            .name("nyaterm-ssh-disconnect".into())
            .spawn(move || {
                loop {
                    if let Some(background) = background_weak.upgrade() {
                        background.reap(std::time::Instant::now());
                    }
                    let job = match receiver.recv_timeout(std::time::Duration::from_secs(1)) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    match job {
                        DisconnectJob::Disconnect(disconnect) => {
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(disconnect))
                                .is_err()
                            {
                                tracing::warn!("SSH connection cleanup panicked");
                            }
                        }
                        DisconnectJob::Barrier(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("spawn SSH disconnect worker");
        Self {
            inner: Arc::new(ConnectionPoolInner {
                connections: Mutex::new(HashMap::new()),
                background,
                disconnects: Arc::new(DisconnectWorker {
                    sender: Some(sender),
                    worker: Some(worker),
                }),
            }),
        }
    }
}

impl SshConnectionPool {
    pub(crate) fn register(&self, key: String, handle: SshMultiplexHandle) -> SshConnectionLease {
        let disconnect_handle = handle.clone();
        self.insert(
            key,
            Some(handle),
            Box::new(move || {
                if let Err(error) = disconnect_handle.disconnect() {
                    tracing::warn!(%error, "failed to disconnect unused SSH connection");
                }
            }),
        )
    }

    fn insert(
        &self,
        key: String,
        handle: Option<SshMultiplexHandle>,
        cleanup: Box<dyn FnOnce() + Send + Sync>,
    ) -> SshConnectionLease {
        let inner = Arc::new(ManagedConnection {
            handle,
            docker_state: Default::default(),
            cleanup: Some(cleanup),
            disconnects: Arc::clone(&self.inner.disconnects),
        });
        let mut connections = self
            .inner
            .connections
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        connections.retain(|_, connection| connection.strong_count() > 0);
        connections.insert(key, Arc::downgrade(&inner));
        SshConnectionLease { inner }
    }

    pub(crate) fn get(&self, key: &str) -> Option<SshConnectionLease> {
        let connections = self
            .inner
            .connections
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let inner = connections.get(key)?.upgrade()?;
        let lease = SshConnectionLease { inner };
        (!lease.is_closed()).then_some(lease)
    }

    /// Called on a background thread after the window's start jobs have stopped.
    /// Other windows' live leases never enter the disconnect queue.
    pub(crate) fn finish_disconnects(&self) {
        let (done, receiver) = mpsc::channel();
        if let Some(sender) = &self.inner.disconnects.sender
            && sender.send(DisconnectJob::Barrier(done)).is_ok()
        {
            let _ = receiver.recv();
        }
    }

    #[cfg(test)]
    pub(crate) fn register_for_test(
        &self,
        key: &str,
        disconnect: impl FnOnce() + Send + Sync + 'static,
    ) -> SshConnectionLease {
        self.insert(key.to_string(), None, Box::new(disconnect))
    }
}

impl SshConnectionLease {
    pub(crate) fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
    pub(crate) fn docker_state(&self) -> nyaterm_remote_ops::docker::DockerConnectionState {
        self.inner.docker_state.clone()
    }

    pub(crate) fn enqueue_cleanup(&self, cleanup: impl FnOnce() + Send + 'static) {
        self.inner.disconnects.submit(Box::new(cleanup));
    }

    pub(crate) fn handle(&self) -> SshMultiplexHandle {
        self.inner
            .handle
            .as_ref()
            .expect("live SSH connection")
            .clone()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.inner
            .handle
            .as_ref()
            .is_some_and(SshMultiplexHandle::is_closed)
    }
}

impl Drop for ManagedConnection {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            self.disconnects.submit(cleanup);
        }
    }
}

impl DisconnectWorker {
    fn submit(&self, cleanup: Box<dyn FnOnce() + Send>) {
        // A discarded start result closes its channel on this same worker. Its
        // final lease can then disconnect inline, before a queued barrier runs.
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.thread().id() == thread::current().id())
        {
            cleanup();
        } else if let Some(sender) = &self.sender
            && sender.send(DisconnectJob::Disconnect(cleanup)).is_err()
        {
            tracing::warn!("SSH disconnect worker stopped before connection cleanup");
        }
    }
}

impl Drop for DisconnectWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take()
            && worker.thread().id() != thread::current().id()
            && worker.join().is_err()
        {
            tracing::warn!("SSH disconnect worker panicked during shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::SshConnectionPool;

    #[test]
    fn connection_survives_until_sessions_and_pending_starts_release_their_leases() {
        let pool = SshConnectionPool::default();
        let disconnected = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&disconnected);
        let first = pool.register_for_test("shared", move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let second = pool.get("shared").unwrap();
        let pending_start = second.clone();
        drop(first);
        drop(second);
        pool.finish_disconnects();
        assert_eq!(disconnected.load(Ordering::SeqCst), 0);
        drop(pending_start);
        pool.finish_disconnects();
        assert_eq!(disconnected.load(Ordering::SeqCst), 1);
        assert!(pool.get("shared").is_none());
    }

    #[test]
    fn releasing_old_connection_does_not_remove_replacement_with_the_same_key() {
        let pool = SshConnectionPool::default();
        let old = pool.register_for_test("shared", || {});
        let new = pool.register_for_test("shared", || {});
        assert!(old.same_connection(&old.clone()));
        assert!(!old.same_connection(&new));
        drop(old);
        pool.finish_disconnects();
        let found = pool.get("shared").unwrap();
        assert!(Arc::ptr_eq(&new.inner, &found.inner));
    }

    #[test]
    fn failed_or_cancelled_reuse_request_does_not_disconnect_existing_session() {
        let pool = SshConnectionPool::default();
        let disconnected = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&disconnected);
        let live = pool.register_for_test("shared", move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let failed_start = pool.get("shared").unwrap();
        drop(failed_start);
        pool.finish_disconnects();
        assert_eq!(disconnected.load(Ordering::SeqCst), 0);
        assert!(pool.get("shared").is_some());
        drop(live);
        pool.finish_disconnects();
        assert_eq!(disconnected.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cleanup_panic_does_not_stop_other_connections_from_disconnecting() {
        let pool = SshConnectionPool::default();
        let broken = pool.register_for_test("broken", || panic!("test cleanup failure"));
        drop(broken);
        pool.finish_disconnects();
        let disconnected = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&disconnected);
        let healthy = pool.register_for_test("healthy", move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        drop(healthy);
        pool.finish_disconnects();
        assert_eq!(disconnected.load(Ordering::SeqCst), 1);
    }
}
