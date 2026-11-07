//! Controlled SSH peer exercises lifecycle ordering on actual encrypted channels.
use nyaterm_transport::ssh_execution::streaming::{SshExecEvent, stream_ssh_command};
use nyaterm_transport::{SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{ChannelId, Sig, server};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Peer {
    channels: Vec<russh::Channel<server::Msg>>,
    signals: Arc<AtomicUsize>,
    ignore_int: bool,
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
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(id)?;
        match command {
            b"eof_first" => {
                session.data(id, b"before eof".to_vec())?;
                session.eof(id)?;
                let handle = session.handle();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(75)).await;
                    handle.exit_status_request(id, 7).await.unwrap();
                    handle.close(id).await.unwrap();
                });
            }
            b"exit_first" => {
                session.exit_status_request(id, 0)?;
                session.data(id, b"tail after exit".to_vec())?;
                session.eof(id)?;
                session.close(id)?;
            }
            b"ignore_int" => {
                self.ignore_int = true;
                session.data(id, b"running".to_vec())?;
            }
            _ => {
                session.data(id, b"running".to_vec())?;
            }
        }
        Ok(())
    }
    async fn signal(
        &mut self,
        id: ChannelId,
        signal: Sig,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert!(matches!(signal, Sig::INT));
        self.signals.fetch_add(1, Ordering::SeqCst);
        if !self.ignore_int {
            session.exit_status_request(id, 130)?;
            session.eof(id)?;
            session.close(id)?;
        }
        Ok(())
    }
}
struct Verify;
impl SshHostKeyVerifier for Verify {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}
async fn peer() -> (
    SshSessionConfig,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let signals = Arc::new(AtomicUsize::new(0));
    let count = signals.clone();
    let worker = tokio::spawn(async move {
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[17; 32])),
            "lifecycle-test",
        )
        .unwrap();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let (socket, _) = listener.accept().await.unwrap();
        let session = server::run_stream(
            config,
            socket,
            Peer {
                channels: Vec::new(),
                signals: count,
                ignore_int: false,
            },
        )
        .await
        .unwrap();
        let _ = session.await;
    });
    (
        SshSessionConfig {
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            allow_none_auth: true,
            host_key_verifier: Some(Arc::new(Verify)),
            ..Default::default()
        },
        signals,
        worker,
    )
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_submission_never_opens_an_ssh_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (sender, mut receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    stream_ssh_command(
        SshSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            ..Default::default()
        },
        None,
        b"must not run".to_vec(),
        Duration::from_secs(1),
        cancellation,
        sender,
    )
    .await;
    assert!(matches!(
        receiver.recv().await,
        Some(SshExecEvent::CancelledBeforeStart)
    ));
    assert!(receiver.recv().await.is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eof_before_exit_and_output_after_exit_are_observed_until_close() {
    for (command, expected, code) in [
        ("eof_first", "before eof", 7),
        ("exit_first", "tail after exit", 0),
    ] {
        let (config, _, peer) = peer().await;
        let (tx, mut rx) = mpsc::channel(64);
        let worker = tokio::spawn(stream_ssh_command(
            config,
            None,
            command.into(),
            Duration::from_secs(5),
            CancellationToken::new(),
            tx,
        ));
        let mut output = Vec::new();
        let mut exit = None;
        let mut finished = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(6), rx.recv())
            .await
            .unwrap()
        {
            match event {
                SshExecEvent::Output { data, .. } => output.extend(data),
                SshExecEvent::ExitStatus(code) => exit = Some(code),
                SshExecEvent::Finished { confirmed, .. } => {
                    assert!(confirmed);
                    finished = true;
                }
                _ => {}
            }
        }
        assert_eq!(String::from_utf8(output).unwrap(), expected);
        assert_eq!(exit, Some(code));
        assert!(finished);
        worker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), peer)
            .await
            .unwrap()
            .unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_is_sent_once_and_ignored_interrupt_times_out_without_exit_evidence() {
    for command in ["wait_int", "ignore_int"] {
        let (config, signals, peer) = peer().await;
        let (tx, mut rx) = mpsc::channel(64);
        let interrupt = CancellationToken::new();
        let worker = tokio::spawn(stream_ssh_command(
            config,
            None,
            command.into(),
            Duration::from_secs(1),
            interrupt.clone(),
            tx,
        ));
        let mut confirmed = false;
        let mut timed_out = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
        {
            match event {
                SshExecEvent::Started => {
                    interrupt.cancel();
                    interrupt.cancel();
                }
                SshExecEvent::Finished { confirmed: c, .. } => confirmed = c,
                SshExecEvent::TimedOut => timed_out = true,
                _ => {}
            }
        }
        assert_eq!(signals.load(Ordering::SeqCst), 1);
        assert_eq!(confirmed, command == "wait_int");
        assert_eq!(timed_out, command == "ignore_int");
        worker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), peer)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_synchronous_exec_also_waits_for_exit_status_after_eof() {
    let (config, _, peer) = peer().await;
    let output = tokio::task::spawn_blocking(move || {
        nyaterm_transport::ssh_execution::run_ssh_command(
            config,
            None,
            b"eof_first".to_vec(),
            Duration::from_secs(5),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.stdout, "before eof");
    assert_eq!(output.exit_status, Some(7));
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}
