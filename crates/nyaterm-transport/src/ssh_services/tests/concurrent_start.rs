use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{Channel, ChannelId, Pty, server};
use tokio::sync::Notify;

use super::AcceptTestKey;
use crate::connection_attempt::ConnectionAttempt;
use crate::ssh_services::{SshFileSessionProbe, SshSessionServices};
use crate::{
    SessionInfo, SessionManager, SshMultiplexHandle, SshSessionConfig, open_ssh_multiplex_handle,
};

#[derive(Default)]
struct Gate {
    block_next: AtomicBool,
    entered: Notify,
    release: Notify,
    received: Notify,
}

struct ReleaseGate(Arc<Gate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release.notify_one();
    }
}

struct Peer {
    gate: Arc<Gate>,
    channels: Vec<Channel<server::Msg>>,
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.gate.block_next.swap(false, Ordering::AcqRel) {
            self.gate.entered.notify_one();
            self.gate.release.notified().await;
        }
        self.channels.push(channel);
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if data == b"existing-session-input" {
            self.gate.received.notify_one();
        }
        session.data(channel, data.to_vec())
    }
}

async fn server(gate: Arc<Gate>) -> (SshSessionConfig, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&[51; 32])),
        "concurrent-start-test",
    )
    .unwrap();
    let task = tokio::spawn(async move {
        let mut peers = tokio::task::JoinSet::new();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let config = config.clone();
            let gate = gate.clone();
            peers.spawn(async move {
                let peer = server::run_stream(
                    config,
                    socket,
                    Peer {
                        gate,
                        channels: Vec::new(),
                    },
                )
                .await
                .unwrap();
                let _ = peer.await;
            });
        }
    });
    (
        SshSessionConfig {
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            allow_none_auth: true,
            runtime_mode: nyaterm_core::models::ssh_runtime::SshRuntimeMode::Terminal,
            host_key_verifier: Some(Arc::new(AcceptTestKey)),
            terminal_shell_integration: false,
            ..Default::default()
        },
        task,
    )
}

fn start(
    manager: Arc<SessionManager>,
    config: SshSessionConfig,
) -> tokio::task::JoinHandle<Result<SessionInfo, String>> {
    tokio::task::spawn_blocking(move || {
        let handle =
            open_ssh_multiplex_handle(config.clone()).map_err(|error| error.to_string())?;
        manager
            .create_ssh_session_with_multiplex(config, handle)
            .map_err(|error| error.to_string())
    })
}

async fn completed(
    job: tokio::task::JoinHandle<Result<SessionInfo, String>>,
) -> Result<SessionInfo, String> {
    tokio::time::timeout(Duration::from_secs(10), job)
        .await
        .expect("independent start must finish")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_for_an_ssh_channel_does_not_block_other_starts_or_existing_session_io() {
    let gate = Arc::new(Gate::default());
    let _release_on_exit = ReleaseGate(gate.clone());
    let (config, peer) = server(gate.clone()).await;
    let manager = Arc::new(SessionManager::new());
    let existing = completed(start(manager.clone(), config.clone()))
        .await
        .unwrap();
    gate.block_next.store(true, Ordering::Release);
    let waiting = start(manager.clone(), config.clone());
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    let independent = completed(start(manager.clone(), config)).await.unwrap();
    assert!(!waiting.is_finished());
    let io_manager = manager.clone();
    let existing_id = existing.id.clone();
    let visible = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            io_manager
                .write(&existing_id, b"existing-session-input")
                .unwrap();
            io_manager.list_sessions().unwrap()
        }),
    )
    .await
    .expect("registry access must remain available")
    .unwrap();
    assert_eq!(visible.len(), 2);
    tokio::time::timeout(Duration::from_secs(5), gate.received.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut output = Vec::new();
        loop {
            if let Some(crate::SessionEvent::Output { session_id, data }) =
                manager.try_recv_event().unwrap()
                && session_id == existing.id
            {
                output.extend_from_slice(&data);
                if output
                    .windows(b"existing-session-input".len())
                    .any(|bytes| bytes == b"existing-session-input")
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("existing session output must remain readable");
    gate.release.notify_one();
    let waiting = completed(waiting).await.unwrap();
    tokio::task::spawn_blocking(move || {
        for id in [existing.id, independent.id, waiting.id] {
            manager.close(&id).unwrap();
        }
        assert!(manager.list_sessions().unwrap().is_empty());
    })
    .await
    .unwrap();
    peer.abort();
}

struct FirstProbe(Arc<Gate>);
impl SshFileSessionProbe for FirstProbe {
    fn probe(
        &self,
        _: SshSessionConfig,
        _: SshMultiplexHandle,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let gate = self.0.clone();
        Box::pin(async move {
            if gate.block_next.swap(false, Ordering::AcqRel) {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            Ok(())
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_readiness_probe_does_not_hold_the_shared_session_registry() {
    let gate = Arc::new(Gate::default());
    let _release_on_exit = ReleaseGate(gate.clone());
    let (mut config, peer) = server(Arc::new(Gate::default())).await;
    config.runtime_mode = nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp;
    let manager = Arc::new(SessionManager::with_ssh_services(SshSessionServices {
        file_session_probe: Some(Arc::new(FirstProbe(gate.clone()))),
    }));
    gate.block_next.store(true, Ordering::Release);
    let attempt = ConnectionAttempt::default();
    config.bind_attempt(attempt.clone());
    let waiting = start(manager.clone(), config.clone());
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    config.bind_attempt(Default::default());
    let independent = completed(start(manager.clone(), config)).await.unwrap();
    assert!(!waiting.is_finished());
    attempt.cancel();
    assert!(completed(waiting).await.unwrap_err().contains("cancelled"));
    tokio::task::spawn_blocking(move || {
        assert_eq!(manager.list_sessions().unwrap().len(), 1);
        manager.close(&independent.id).unwrap();
    })
    .await
    .unwrap();
    peer.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_registration_reaps_the_ready_worker_without_publishing_a_session() {
    let gate = Arc::new(Gate::default());
    let _release_on_exit = ReleaseGate(gate.clone());
    let (config, peer) = server(gate.clone()).await;
    let manager = Arc::new(SessionManager::new());
    gate.block_next.store(true, Ordering::Release);
    let waiting = start(manager.clone(), config);
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    let poisoned = manager.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoned.sessions.lock().unwrap();
        panic!("test registration failure");
    })
    .join();
    gate.release.notify_one();
    assert!(completed(waiting).await.is_err());
    assert!(
        manager
            .sessions
            .lock()
            .err()
            .unwrap()
            .into_inner()
            .is_empty()
    );
    peer.abort();
}
