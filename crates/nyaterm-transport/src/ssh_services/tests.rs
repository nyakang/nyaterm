use crate::connection_attempt::ConnectionAttempt;
use crate::ssh_channel::SshChannelConnection;
use crate::ssh_services::{SshFileSessionProbe, SshSessionServices};
use crate::{
    SessionManager, SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshMultiplexHandle,
    SshSessionConfig, open_ssh_multiplex_handle,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct Peer {
    channels: Vec<russh::Channel<server::Msg>>,
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.push(channel);
        reply.accept().await;
        Ok(())
    }
}

struct AcceptTestKey;

impl SshHostKeyVerifier for AcceptTestKey {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

struct Probe {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    fail: bool,
}

impl SshFileSessionProbe for Probe {
    fn probe(
        &self,
        _: SshSessionConfig,
        _: SshMultiplexHandle,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let fail = self.fail;
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            anyhow::ensure!(!fail, "test file capability unavailable");
            Ok(())
        })
    }
}

async fn start_server() -> (SshSessionConfig, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&[9; 32])),
        "probe-test",
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let session = server::run_stream(config, socket, Peer::default())
            .await
            .unwrap();
        let _ = session.await;
    });
    let config = SshSessionConfig {
        host: "127.0.0.1".into(),
        port,
        username: "test".into(),
        allow_none_auth: true,
        host_key_verifier: Some(Arc::new(AcceptTestKey)),
        ..Default::default()
    };
    (config, server)
}

async fn start_job(
    services: SshSessionServices,
    attempt: ConnectionAttempt,
) -> (
    tokio::task::JoinHandle<Result<(), String>>,
    tokio::task::JoinHandle<()>,
) {
    let (mut config, server) = start_server().await;
    config.runtime_mode = nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp;
    let job = tokio::task::spawn_blocking(move || {
        let handle = open_ssh_multiplex_handle(config.clone()).unwrap();
        config.bind_attempt(attempt);
        let manager = SessionManager::with_ssh_services(services);
        let result = manager.create_ssh_session_with_multiplex(config, handle.clone());
        let result = match result {
            Ok(session) => manager
                .close(&session.id)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        assert!(manager.list_sessions().unwrap().is_empty());
        handle.disconnect().unwrap();
        result
    });
    (job, server)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_dedicated_channel_connection_disconnects_the_peer() {
    let (config, server) = start_server().await;
    let connection = SshChannelConnection::connect(&config, None).await.unwrap();
    let channel = connection.open_session().await.unwrap();
    connection.close("test operation completed").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("dedicated peer must disconnect")
        .unwrap();
    drop(channel);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_channel_connections_obey_the_first_channel_gate_and_close_independently() {
    let (config, server) = start_server().await;
    tokio::task::spawn_blocking(move || {
        let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
        let claim = multiplex.claim_primary_session().unwrap();
        let shared = multiplex.clone();
        multiplex
            .block_on(async move {
                let connection = SshChannelConnection::connect(&config, Some(&shared)).await?;
                let mut open =
                    Box::pin(connection.open_session_with_timeout(Duration::from_secs(1)));
                assert!(
                    tokio::time::timeout(Duration::from_millis(1100), &mut open)
                        .await
                        .is_err(),
                    "auxiliary session must wait for the reserved interactive channel"
                );
                drop(claim);
                let channel = tokio::time::timeout(Duration::from_secs(5), open).await??;
                connection.close("first operation completed").await?;
                // A later service on the same connection still opens a channel.
                let next = SshChannelConnection::connect(&config, Some(&shared)).await?;
                let next_channel = next.open_session().await?;
                next.close("second operation completed").await?;
                drop((channel, next_channel));
                Ok(())
            })
            .unwrap();
        multiplex.disconnect().unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the owning connection must disconnect the peer")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_session_readiness_waits_for_the_injected_probe_and_propagates_failure() {
    for fail in [false, true] {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let services = SshSessionServices {
            file_session_probe: Some(Arc::new(Probe {
                entered: entered.clone(),
                release: release.clone(),
                fail,
            })),
        };
        let (job, server) = start_job(services, Default::default()).await;
        tokio::time::timeout(Duration::from_secs(10), entered.notified())
            .await
            .unwrap();
        assert!(
            !job.is_finished(),
            "session cannot be ready before the probe completes"
        );
        release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(10), job)
            .await
            .unwrap()
            .unwrap();
        if fail {
            assert!(
                result
                    .unwrap_err()
                    .contains("test file capability unavailable")
            );
        } else {
            result.unwrap();
        }
        server.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_releases_a_pending_file_probe_without_reporting_ready() {
    let entered = Arc::new(Notify::new());
    let attempt = ConnectionAttempt::default();
    let services = SshSessionServices {
        file_session_probe: Some(Arc::new(Probe {
            entered: entered.clone(),
            release: Arc::new(Notify::new()),
            fail: false,
        })),
    };
    let (job, server) = start_job(services, attempt.clone()).await;
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    attempt.cancel();
    let error = tokio::time::timeout(Duration::from_secs(10), job)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("cancelled"));
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_session_without_an_injected_probe_returns_a_configuration_error() {
    let (job, server) = start_job(Default::default(), Default::default()).await;
    let error = tokio::time::timeout(Duration::from_secs(10), job)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("file session probe is not configured"));
    server.abort();
}
