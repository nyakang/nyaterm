//! Background transports retain a cache lease, never a terminal or PTY.
use super::{SshConnectionLease, SshConnectionPool};
use nyaterm_transport::{SshSessionConfig, open_ssh_multiplex_handle};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(crate) struct BackgroundConnections {
    entries: Mutex<HashMap<String, Arc<Entry>>>,
}

struct Entry {
    config: SshSessionConfig,
    policy: String,
    state: Mutex<EntryState>,
    changed: Condvar,
}

#[derive(Default)]
struct EntryState {
    connecting: bool,
    failed: bool,
    connection: Option<SshConnectionLease>,
    users: usize,
    waiters: usize,
    idle_since: Option<Instant>,
}

pub(crate) struct BackgroundConnectionLease {
    entry: Arc<Entry>,
    pub(crate) connection: SshConnectionLease,
}

impl BackgroundConnectionLease {
    pub(crate) fn config(&self) -> SshSessionConfig {
        self.entry.config.clone()
    }
}

impl Drop for BackgroundConnectionLease {
    fn drop(&mut self) {
        let mut state = self.entry.state.lock().unwrap_or_else(|e| e.into_inner());
        state.users = state.users.saturating_sub(1);
        if state.users == 0 {
            state.idle_since = Some(Instant::now());
        }
    }
}

impl BackgroundConnections {
    pub(super) fn reap(&self, now: Instant) {
        let removed = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            let keys = entries
                .iter()
                .filter_map(|(key, entry)| {
                    let state = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    (state.users == 0
                        && state.waiters == 0
                        && !state.connecting
                        && (state.failed
                            || state.idle_since.is_some_and(|at| {
                                now.saturating_duration_since(at) >= IDLE_TIMEOUT
                            })))
                    .then(|| key.clone())
                })
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| entries.remove(&key))
                .collect::<Vec<_>>()
        };
        // Dropping the last transport lease queues cleanup outside the pool lock.
        drop(removed);
    }
}

impl SshConnectionPool {
    /// Blocking adapter: callers must run this on a background worker.
    pub(crate) fn acquire_background(
        &self,
        saved_connection_id: &str,
        config: SshSessionConfig,
        policy: String,
        cancellation: &CancellationToken,
        deadline: Instant,
    ) -> anyhow::Result<BackgroundConnectionLease> {
        let pool = self.clone();
        self.acquire_with(
            saved_connection_id,
            config,
            policy,
            cancellation,
            deadline,
            move |config| {
                let handle = open_ssh_multiplex_handle(config)?;
                Ok(pool.register(format!("mcp-background:{}", uuid::Uuid::new_v4()), handle))
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn acquire_with(
        &self,
        saved_connection_id: &str,
        config: SshSessionConfig,
        policy: String,
        cancellation: &CancellationToken,
        deadline: Instant,
        connect: impl FnOnce(SshSessionConfig) -> anyhow::Result<SshConnectionLease> + Send + 'static,
    ) -> anyhow::Result<BackgroundConnectionLease> {
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            anyhow::bail!("Background SSH acquisition cancelled or timed out");
        }
        let (entry, create, retired) = {
            let mut entries = self
                .inner
                .background
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let reusable = entries
                .get(saved_connection_id)
                .filter(|entry| {
                    let state = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    entry.policy == policy
                        && same_config(&entry.config, &config)
                        && !state.failed
                        && !entry.config.attempt.is_cancelled()
                        && state
                            .connection
                            .as_ref()
                            .is_none_or(|lease| !lease.is_closed())
                })
                .cloned();
            if let Some(entry) = reusable {
                entry
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .waiters += 1;
                (entry, false, None)
            } else {
                if entries.len() >= 64 && !entries.contains_key(saved_connection_id) {
                    anyhow::bail!("Background SSH connection limit reached");
                }
                let entry = Arc::new(Entry {
                    config,
                    policy,
                    state: Mutex::new(EntryState {
                        connecting: true,
                        waiters: 1,
                        ..EntryState::default()
                    }),
                    changed: Condvar::new(),
                });
                let retired = entries.insert(saved_connection_id.into(), entry.clone());
                (entry, true, retired)
            }
        };
        drop(retired);
        if create {
            let pending = entry.clone();
            let started = std::thread::Builder::new()
                .name("nyaterm-background-ssh".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        connect(pending.config.clone())
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Background SSH connector failed")));
                    let mut state = pending.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.connecting = false;
                    match result {
                        Ok(connection) => {
                            state.connection = Some(connection);
                            state.idle_since = Some(Instant::now());
                        }
                        Err(_) => state.failed = true,
                    }
                    pending.changed.notify_all();
                });
            if started.is_err() {
                let mut state = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                state.connecting = false;
                state.failed = true;
                entry.changed.notify_all();
            }
        }
        let mut state = entry.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if cancellation.is_cancelled()
                || Instant::now() >= deadline
                || entry.config.attempt.is_cancelled()
            {
                state.waiters -= 1;
                if state.waiters == 0 && state.connecting {
                    entry.config.attempt.cancel();
                }
                anyhow::bail!("Background SSH acquisition cancelled or timed out");
            }
            if let Some(connection) = state.connection.clone() {
                state.waiters -= 1;
                state.users += 1;
                state.idle_since = None;
                drop(state);
                return Ok(BackgroundConnectionLease { entry, connection });
            }
            if state.failed {
                state.waiters -= 1;
                anyhow::bail!("Background SSH connection could not be established");
            }
            state = entry
                .changed
                .wait_timeout(state, Duration::from_millis(25))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    pub(crate) fn invalidate_background(&self) {
        let entries = std::mem::take(
            &mut *self
                .inner
                .background
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        // Existing waiters and active leases may finish on retired entries.
        // Security shutdown cancels its own waiters; the last cancelled waiter
        // cancels the dial without disrupting work owned by another window.
        drop(entries);
    }
}

// Compare effective routing and authentication, including every jump. Secret
// wrappers compare in memory; no credentials are formatted, hashed or logged.
fn same_config(a: &SshSessionConfig, b: &SshSessionConfig) -> bool {
    a.host == b.host
        && a.port == b.port
        && a.host_key_alias == b.host_key_alias
        && a.username == b.username
        && a.password == b.password
        && a.key_auth == b.key_auth
        && a.agent_auth == b.agent_auth
        && a.agent_endpoint == b.agent_endpoint
        && a.agent_forwarding_config == b.agent_forwarding_config
        && a.otp_id == b.otp_id
        && a.auto_fill_otp == b.auto_fill_otp
        && a.proxy == b.proxy
        && a.allow_none_auth == b.allow_none_auth
        && a.ssh_algorithms == b.ssh_algorithms
        && a.runtime_mode == b.runtime_mode
        && a.profile == b.profile
        && a.sftp == b.sftp
        && a.encoding == b.encoding
        && a.keep_alive_interval_secs == b.keep_alive_interval_secs
        && a.keep_alive_mode == b.keep_alive_mode
        && match (&a.proxy_jump, &b.proxy_jump) {
            (Some(a), Some(b)) => same_config(a, b),
            (None, None) => true,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundConnectionLease, IDLE_TIMEOUT, SshConnectionPool, same_config};
    use nyaterm_transport::SshSessionConfig;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    fn acquire(
        pool: &SshConnectionPool,
        id: &str,
        config: SshSessionConfig,
        policy: &str,
        count: &Arc<AtomicUsize>,
        closed: &Arc<AtomicUsize>,
    ) -> BackgroundConnectionLease {
        let connector = pool.clone();
        let count = count.clone();
        let closed = closed.clone();
        pool.acquire_with(
            id,
            config,
            policy.into(),
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
            move |_| {
                let number = count.fetch_add(1, Ordering::SeqCst);
                Ok(
                    connector.register_for_test(&format!("fixture-{number}"), move || {
                        closed.fetch_add(1, Ordering::SeqCst);
                    }),
                )
            },
        )
        .unwrap()
    }

    #[test]
    fn twenty_operations_reuse_transport_and_idle_reaping_preserves_active_leases() {
        let pool = SshConnectionPool::default();
        let count = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        for _ in 0..20 {
            drop(acquire(
                &pool,
                "saved",
                SshSessionConfig::default(),
                "policy",
                &count,
                &closed,
            ));
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let active = acquire(
            &pool,
            "saved",
            SshSessionConfig::default(),
            "policy",
            &count,
            &closed,
        );
        pool.inner
            .background
            .reap(Instant::now() + IDLE_TIMEOUT * 2);
        assert_eq!(pool.inner.background.entries.lock().unwrap().len(), 1);
        assert_eq!(closed.load(Ordering::SeqCst), 0);
        drop(active);
        let idle_since = pool.inner.background.entries.lock().unwrap()["saved"]
            .state
            .lock()
            .unwrap()
            .idle_since
            .unwrap();
        pool.inner
            .background
            .reap(idle_since + IDLE_TIMEOUT - Duration::from_millis(1));
        assert_eq!(pool.inner.background.entries.lock().unwrap().len(), 1);
        pool.inner.background.reap(idle_since + IDLE_TIMEOUT);
        pool.finish_disconnects();
        assert!(pool.inner.background.entries.lock().unwrap().is_empty());
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn saved_identity_credentials_jump_and_policy_changes_do_not_merge_transports() {
        let pool = SshConnectionPool::default();
        let count = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let original = acquire(
            &pool,
            "one",
            SshSessionConfig::default(),
            "revision-1",
            &count,
            &closed,
        );
        let other = acquire(
            &pool,
            "two",
            SshSessionConfig::default(),
            "revision-1",
            &count,
            &closed,
        );
        assert!(!original.connection.same_connection(&other.connection));
        let changed = SshSessionConfig {
            password: Some("fixture".into()),
            ..Default::default()
        };
        let replacement = acquire(&pool, "one", changed.clone(), "revision-1", &count, &closed);
        assert!(!original.connection.same_connection(&replacement.connection));
        let policy = acquire(&pool, "one", changed, "revision-2", &count, &closed);
        assert!(!replacement.connection.same_connection(&policy.connection));
        let jump = SshSessionConfig {
            proxy_jump: Some(Box::new(SshSessionConfig {
                host: "jump".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert!(!same_config(&SshSessionConfig::default(), &jump));
        let mut new_jump = jump.clone();
        new_jump.proxy_jump.as_mut().unwrap().password = Some("changed".into());
        assert!(!same_config(&jump, &new_jump));
        pool.invalidate_background();
        pool.finish_disconnects();
        assert_eq!(closed.load(Ordering::SeqCst), 0);
        drop((original, other, replacement, policy));
        pool.finish_disconnects();
        assert_eq!(closed.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn cancelling_the_last_waiter_retires_the_dial_and_a_retry_uses_a_new_attempt() {
        let pool = SshConnectionPool::default();
        let cancellation = CancellationToken::new();
        let cancelled = cancellation.clone();
        let connecting_pool = pool.clone();
        let (started, attempt) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let connecting = std::thread::spawn(move || {
            connecting_pool.acquire_with(
                "saved",
                SshSessionConfig::default(),
                "policy".into(),
                &cancelled,
                Instant::now() + Duration::from_secs(5),
                move |config| {
                    started.send(config.attempt.clone()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    config.attempt.check().map_err(anyhow::Error::msg)?;
                    unreachable!("the last waiter cancelled this dial")
                },
            )
        });
        let attempt = attempt.recv_timeout(Duration::from_secs(5)).unwrap();
        cancellation.cancel();
        assert!(connecting.join().unwrap().is_err());
        assert!(attempt.is_cancelled());
        let count = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let retry = acquire(
            &pool,
            "saved",
            SshSessionConfig::default(),
            "policy",
            &count,
            &closed,
        );
        assert!(!retry.config().attempt.is_cancelled());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        drop(retry);
        pool.invalidate_background();
        pool.finish_disconnects();
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_waiters_share_one_dial_and_cancelling_one_does_not_cancel_the_other() {
        let pool = SshConnectionPool::default();
        let first_cancel = CancellationToken::new();
        let (dial_started, started) = mpsc::channel();
        let (release, dial_release) = mpsc::channel();
        let first_pool = pool.clone();
        let cancellation = first_cancel.clone();
        let first = std::thread::spawn(move || {
            let connector = first_pool.clone();
            first_pool.acquire_with(
                "saved",
                SshSessionConfig::default(),
                "policy".into(),
                &cancellation,
                Instant::now() + Duration::from_secs(5),
                move |config| {
                    dial_started.send(()).unwrap();
                    dial_release.recv_timeout(Duration::from_secs(5)).unwrap();
                    config.attempt.check().map_err(anyhow::Error::msg)?;
                    Ok(connector.register_for_test("shared", || {}))
                },
            )
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let second_pool = pool.clone();
        let second = std::thread::spawn(move || {
            second_pool.acquire_with(
                "saved",
                SshSessionConfig::default(),
                "policy".into(),
                &CancellationToken::new(),
                Instant::now() + Duration::from_secs(5),
                |_| panic!("second dial"),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let waiters = pool.inner.background.entries.lock().unwrap()["saved"]
                .state
                .lock()
                .unwrap()
                .waiters;
            if waiters == 2 {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        first_cancel.cancel();
        assert!(first.join().unwrap().is_err());
        release.send(()).unwrap();
        let lease = second.join().unwrap().unwrap();
        assert!(!lease.config().attempt.is_cancelled());
        drop(lease);
        pool.invalidate_background();
    }
}
