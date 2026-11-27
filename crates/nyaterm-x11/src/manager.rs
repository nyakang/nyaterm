//! Application-owned managed server. A single blocking supervisor serializes
//! start/stop, while channel acquisitions and drops never block on process I/O.
use futures::channel::oneshot;
use futures::future::BoxFuture;
use nyaterm_core::x11::{X11ChannelConnection, X11Connection, X11Error};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
type Reply = oneshot::Sender<Result<X11ChannelConnection, X11Error>>;
type Starter = Box<dyn FnOnce(StartContext) -> Result<X11Connection, X11Error> + Send>;

struct State {
    pending: Vec<Reply>,
    starter: Option<Starter>,
    connection: Option<X11Connection>,
    generation: u64,
    active: usize,
    idle_since: Instant,
    starting: bool,
    shutting_down: bool,
    stopped: bool,
    clipboard: bool,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    idle_timeout: Duration,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

/// Cancellation belongs to a start generation, not the first SSH session.
#[derive(Clone)]
pub struct StartContext {
    shared: Arc<Shared>,
    active: Arc<AtomicBool>,
    committed: Arc<AtomicBool>,
}
impl StartContext {
    pub fn shutting_down(&self) -> bool {
        !self.active.load(Ordering::Acquire) || self.shared.state.lock().unwrap().shutting_down
    }
    pub fn has_waiters(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .pending
            .iter()
            .any(|reply| !reply.is_canceled())
    }
    pub fn cancelled(&self) -> bool {
        self.shutting_down() || (!self.committed.load(Ordering::Acquire) && !self.has_waiters())
    }
    /// Desktop calls this when the user explicitly approves installation. The
    /// approved operation can finish even if every graphical client times out.
    pub fn commit_preparation(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        if !self.active.load(Ordering::Acquire)
            || state.shutting_down
            || !state.pending.iter().any(|reply| !reply.is_canceled())
        {
            return false;
        }
        self.committed.store(true, Ordering::Release);
        true
    }
    pub fn preparation_committed(&self) -> bool {
        self.committed.load(Ordering::Acquire)
    }
    pub fn needs_retry(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .pending
            .iter()
            .any(|reply| reply.is_canceled())
    }
    pub fn clipboard(&self) -> bool {
        self.shared.state.lock().unwrap().clipboard
    }
}

pub struct X11ServerManager {
    shared: Arc<Shared>,
}
impl Default for X11ServerManager {
    fn default() -> Self {
        Self::new()
    }
}
impl X11ServerManager {
    pub fn new() -> Self {
        Self::with_idle_timeout(IDLE_TIMEOUT)
    }
    fn with_idle_timeout(idle_timeout: Duration) -> Self {
        Self::with_clock(idle_timeout, Arc::new(Instant::now))
    }
    fn with_clock(idle_timeout: Duration, now: Arc<dyn Fn() -> Instant + Send + Sync>) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    pending: Vec::new(),
                    starter: None,
                    connection: None,
                    generation: 0,
                    active: 0,
                    idle_since: now(),
                    starting: false,
                    shutting_down: false,
                    stopped: true,
                    clipboard: false,
                }),
                changed: Condvar::new(),
                idle_timeout,
                now,
            }),
        }
    }
    /// Only the first concurrent caller's starter runs. It executes on the
    /// manager thread and may wait for desktop's installation approval.
    pub fn acquire(
        &self,
        starter: impl FnOnce(StartContext) -> Result<X11Connection, X11Error> + Send + 'static,
    ) -> BoxFuture<'static, Result<X11ChannelConnection, X11Error>> {
        let (reply, receive) = oneshot::channel();
        let mut state = self.shared.state.lock().unwrap();
        if state.shutting_down {
            let _ = reply.send(Err(X11Error::Cancelled));
        } else if state.connection.as_ref().is_some_and(alive) {
            let connection = lease(&self.shared, &mut state);
            // Delivery failure must drop the guard outside the state lock.
            drop(state);
            let _ = reply.send(Ok(connection));
            return Box::pin(async move { receive.await.unwrap_or(Err(X11Error::Cancelled)) });
        } else {
            if !state.starting {
                state.pending.retain(|reply| !reply.is_canceled());
            }
            state.pending.push(reply);
            if !state.starting && state.starter.is_none() {
                state.starter = Some(Box::new(starter));
            }
            if state.stopped {
                state.stopped = false;
                let shared = self.shared.clone();
                if std::thread::Builder::new()
                    .name("nyaterm-x11-manager".into())
                    .spawn(move || supervise(shared))
                    .is_err()
                {
                    state.stopped = true;
                    state.starter = None;
                    for reply in state.pending.drain(..) {
                        let _ = reply.send(Err(X11Error::Startup));
                    }
                }
            }
            self.shared.changed.notify_all();
        }
        Box::pin(async move { receive.await.unwrap_or(Err(X11Error::Cancelled)) })
    }
    /// Settings affect the next start, never a running server.
    pub fn set_clipboard(&self, enabled: bool) {
        self.shared.state.lock().unwrap().clipboard = enabled;
    }
    pub fn is_running(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .connection
            .as_ref()
            .is_some_and(alive)
    }
    pub fn is_shutting_down(&self) -> bool {
        self.shared.state.lock().unwrap().shutting_down
    }
    pub fn begin_shutdown(&self) {
        self.shared.state.lock().unwrap().shutting_down = true;
        self.shared.changed.notify_all();
    }
    /// Called on a background executor before shutting down the plugin service.
    pub fn shutdown(&self) {
        self.begin_shutdown();
        let mut state = self.shared.state.lock().unwrap();
        while !state.stopped {
            state = self.shared.changed.wait(state).unwrap();
        }
    }
}
impl Drop for X11ServerManager {
    fn drop(&mut self) {
        self.begin_shutdown();
    }
}
fn alive(connection: &X11Connection) -> bool {
    connection
        .lifetime
        .as_ref()
        .is_some_and(|lifetime| lifetime.is_alive())
}
struct ChannelGuard {
    shared: Weak<Shared>,
    generation: u64,
}
impl Drop for ChannelGuard {
    fn drop(&mut self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut state = shared.state.lock().unwrap();
        if state.generation == self.generation {
            state.active -= 1;
            if state.active == 0 {
                state.idle_since = (shared.now)();
            }
            shared.changed.notify_all();
        }
    }
}
fn lease(shared: &Arc<Shared>, state: &mut State) -> X11ChannelConnection {
    state.active += 1;
    let mut connection = state.connection.as_ref().unwrap().clone();
    connection.lifetime = None;
    X11ChannelConnection {
        connection,
        guard: Some(Box::new(ChannelGuard {
            shared: Arc::downgrade(shared),
            generation: state.generation,
        })),
    }
}
fn stop(connection: X11Connection) {
    if let Some(lifetime) = connection.lifetime {
        lifetime.shutdown();
        lifetime.wait_stopped();
    }
}
fn supervise(shared: Arc<Shared>) {
    loop {
        let mut state = shared.state.lock().unwrap();
        state.pending.retain(|reply| !reply.is_canceled());
        if state.shutting_down {
            state.starter = None;
            for reply in state.pending.drain(..) {
                let _ = reply.send(Err(X11Error::Cancelled));
            }
            let connection = state.connection.take();
            drop(state);
            if let Some(connection) = connection {
                stop(connection);
            }
            shared.state.lock().unwrap().stopped = true;
            shared.changed.notify_all();
            return;
        }
        if state.connection.as_ref().is_some_and(|connection| {
            !alive(connection)
                || (state.active == 0
                    && (shared.now)().saturating_duration_since(state.idle_since)
                        >= shared.idle_timeout)
        }) {
            let connection = state.connection.take().unwrap();
            state.generation += 1;
            state.active = 0;
            drop(state);
            stop(connection);
            continue;
        }
        if state.connection.is_none()
            && let Some(starter) = state.starter.take()
        {
            if state.pending.is_empty() {
                continue;
            }
            state.starting = true;
            drop(state);
            let active = Arc::new(AtomicBool::new(true));
            let context = StartContext {
                shared: shared.clone(),
                active: active.clone(),
                committed: Arc::new(AtomicBool::new(false)),
            };
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| starter(context)))
                    .unwrap_or(Err(X11Error::Startup));
            active.store(false, Ordering::Release);
            let mut state = shared.state.lock().unwrap();
            state.starting = false;
            let replies = std::mem::take(&mut state.pending);
            match result {
                Ok(connection) if !state.shutting_down => {
                    state.generation += 1;
                    state.active = 0;
                    state.idle_since = (shared.now)();
                    state.connection = Some(connection);
                    let deliveries: Vec<_> = replies
                        .into_iter()
                        .filter(|r| !r.is_canceled())
                        .map(|reply| (reply, lease(&shared, &mut state)))
                        .collect();
                    drop(state);
                    for (reply, connection) in deliveries {
                        let _ = reply.send(Ok(connection));
                    }
                }
                result => {
                    drop(state);
                    let error = match result {
                        Ok(connection) => {
                            stop(connection);
                            X11Error::Cancelled
                        }
                        Err(error) => error,
                    };
                    for reply in replies {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            continue;
        }
        // Periodic health checks also observe dropped acquisition futures.
        let _ = shared
            .changed
            .wait_timeout(state, Duration::from_millis(100))
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use crate::manager::X11ServerManager;
    use futures::executor::block_on;
    use nyaterm_core::x11::{X11Connection, X11Endpoint, X11Error, X11Lifetime};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    struct Process {
        alive: AtomicBool,
        stops: AtomicUsize,
        reaped: AtomicBool,
    }
    impl Process {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                alive: AtomicBool::new(true),
                stops: AtomicUsize::new(0),
                reaped: AtomicBool::new(false),
            })
        }
        fn connection(self: &Arc<Self>) -> X11Connection {
            X11Connection {
                endpoint: X11Endpoint::Tcp {
                    host: "127.0.0.1".into(),
                    port: 6010,
                },
                cookie: Some(zeroize::Zeroizing::new(vec![7; 16])),
                lifetime: Some(self.clone()),
            }
        }
    }
    impl X11Lifetime for Process {
        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::Acquire)
        }
        fn shutdown(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.alive.store(false, Ordering::Release);
        }
        fn wait_stopped(&self) {
            self.reaped.store(true, Ordering::Release);
        }
    }
    fn until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn concurrent_requests_share_one_start_and_cancellation_does_not_cancel_other_waiters() {
        let manager = X11ServerManager::new();
        let process = Process::new();
        let child = process.clone();
        let (entered, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let first = manager.acquire(move |context| {
            entered.send(()).unwrap();
            gate.recv().unwrap();
            assert!(context.has_waiters());
            Ok(child.connection())
        });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = manager.acquire(|_| panic!("duplicate server start"));
        drop(first);
        release.send(()).unwrap();
        let lease = block_on(second).unwrap();
        assert!(
            lease.connection.lifetime.is_none(),
            "channels cannot stop the shared process"
        );
        drop(lease);
        assert!(process.is_alive());
        manager.shutdown();
        assert_eq!(process.stops.load(Ordering::SeqCst), 1);
        assert!(process.reaped.load(Ordering::Acquire));
    }
    #[test]
    fn idle_uses_channel_count_and_new_channel_resets_the_five_minute_deadline() {
        let epoch = Instant::now();
        let seconds = Arc::new(AtomicU64::new(0));
        let clock = seconds.clone();
        let manager = X11ServerManager::with_clock(
            Duration::from_secs(300),
            Arc::new(move || epoch + Duration::from_secs(clock.load(Ordering::Acquire))),
        );
        let process = Process::new();
        let child = process.clone();
        let first = block_on(manager.acquire(move |_| Ok(child.connection()))).unwrap();
        seconds.store(600, Ordering::Release);
        let second =
            block_on(manager.acquire(|_| panic!("running server must be reused"))).unwrap();
        drop(first);
        assert!(process.is_alive());
        drop(second);
        seconds.store(899, Ordering::Release);
        let third = block_on(manager.acquire(|_| panic!("idle deadline has not expired"))).unwrap();
        drop(third);
        seconds.store(1199, Ordering::Release);
        manager.shared.changed.notify_all();
        until(|| process.reaped.load(Ordering::Acquire));
        assert_eq!(process.stops.load(Ordering::SeqCst), 1);
        manager.shutdown();
    }
    #[test]
    fn crash_restarts_on_next_request_and_old_leases_do_not_change_new_generation() {
        let manager = X11ServerManager::new();
        let old = Process::new();
        let child = old.clone();
        let first = block_on(manager.acquire(move |_| Ok(child.connection()))).unwrap();
        old.alive.store(false, Ordering::Release);
        let new = Process::new();
        let child = new.clone();
        let previous = old.clone();
        let second = block_on(manager.acquire(move |_| {
            assert!(previous.reaped.load(Ordering::Acquire));
            Ok(child.connection())
        }))
        .unwrap();
        drop(first);
        assert_eq!(manager.shared.state.lock().unwrap().active, 1);
        drop(second);
        assert_eq!(manager.shared.state.lock().unwrap().active, 0);
        manager.shutdown();
    }
    #[test]
    fn shutdown_cancels_preparation_and_never_accepts_a_late_start_or_new_request() {
        let manager = X11ServerManager::new();
        let process = Process::new();
        let child = process.clone();
        let (entered, started) = mpsc::channel();
        let request = manager.acquire(move |context| {
            entered.send(()).unwrap();
            until(|| context.shutting_down());
            Ok(child.connection())
        });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        manager.shutdown();
        assert!(matches!(block_on(request), Err(X11Error::Cancelled)));
        assert!(process.reaped.load(Ordering::Acquire));
        assert!(matches!(
            block_on(manager.acquire(|_| panic!("shutdown must reject new starts"))),
            Err(X11Error::Cancelled)
        ));
    }
    #[test]
    fn failed_start_is_shared_and_a_later_request_can_retry() {
        let manager = X11ServerManager::new();
        assert!(matches!(
            block_on(manager.acquire(|_| Err(X11Error::Startup))),
            Err(X11Error::Startup)
        ));
        let process = Process::new();
        let child = process.clone();
        let lease = block_on(manager.acquire(move |_| Ok(child.connection()))).unwrap();
        drop(lease);
        manager.shutdown();
    }
    #[test]
    fn approved_preparation_survives_client_timeout_and_stale_context_cannot_approve_again() {
        let manager = X11ServerManager::new();
        let process = Process::new();
        let child = process.clone();
        let (entered, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let request = manager.acquire(move |context| {
            assert!(context.commit_preparation());
            entered.send(context.clone()).unwrap();
            gate.recv().unwrap();
            assert!(!context.has_waiters());
            assert!(!context.cancelled());
            assert!(context.needs_retry());
            Ok(child.connection())
        });
        let context = started.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(request);
        release.send(()).unwrap();
        until(|| manager.is_running());
        assert!(context.cancelled());
        assert!(!context.commit_preparation());
        manager.shutdown();
    }
    #[test]
    fn changing_clipboard_does_not_restart_a_running_instance() {
        let manager = X11ServerManager::new();
        let process = Process::new();
        let child = process.clone();
        let first = block_on(manager.acquire(move |context| {
            assert!(!context.clipboard());
            Ok(child.connection())
        }))
        .unwrap();
        manager.set_clipboard(true);
        let second =
            block_on(manager.acquire(|_| panic!("clipboard must not restart an active server")))
                .unwrap();
        assert_eq!(process.stops.load(Ordering::SeqCst), 0);
        process.alive.store(false, Ordering::Release);
        let replacement = Process::new();
        let child = replacement.clone();
        let third = block_on(manager.acquire(move |context| {
            assert!(context.clipboard());
            Ok(child.connection())
        }))
        .unwrap();
        drop((first, second, third));
        manager.shutdown();
    }
}
