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
    async fn shell_request(
        &mut self,
        id: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(id)?;
        Ok(())
    }
    async fn pty_request(
        &mut self,
        id: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(id)?;
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.data(id, data.to_vec())?;
        Ok(())
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let mut target = tokio::net::TcpStream::connect((host, port as u16))
            .await
            .unwrap();
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut target).await;
        });
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_compatibility_mode_preserves_interactive_shell_and_shared_tunnel_channels() {
    use nyaterm_transport::{
        SessionEvent, SessionManager, SftpCwdFollowMode, SshTunnelConfig, SshTunnelManager,
        SshTunnelMode, open_ssh_multiplex_handle,
    };
    use std::io::{Read as _, Write as _};
    use std::time::Instant;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    for enabled in [false, true] {
        let (mut config, _, server) = peer().await;
        config.ssh_compatibility_mode = enabled;
        config.terminal_shell_integration = false;
        config.sftp.cwd_follow_mode = SftpCwdFollowMode::Off;
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        let echo = tokio::spawn(async move {
            let (mut socket, _) = target.accept().await.unwrap();
            let mut input = [0; 6];
            socket.read_exact(&mut input).await.unwrap();
            socket.write_all(&input).await.unwrap();
        });
        tokio::time::timeout(
            Duration::from_secs(15),
            tokio::task::spawn_blocking(move || {
                let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
                let sessions = SessionManager::new();
                let session = sessions
                    .create_ssh_session_with_multiplex(config.clone(), multiplex.clone())
                    .unwrap();
                sessions.write_raw(&session.id, b"shell-input").unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut output = Vec::new();
                while Instant::now() < deadline {
                    for event in sessions.drain_events(64).unwrap().events {
                        if let SessionEvent::Output { data, .. } = event {
                            output.extend(data);
                        }
                    }
                    if output
                        .windows(b"shell-input".len())
                        .any(|part| part == b"shell-input")
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(
                    output
                        .windows(b"shell-input".len())
                        .any(|part| part == b"shell-input")
                );
                let tunnels = SshTunnelManager::new();
                let tunnel = tunnels
                    .open_with_multiplex(
                        SshTunnelConfig {
                            id: "compatibility-tunnel".into(),
                            ssh_config: config,
                            mode: SshTunnelMode::Local,
                            bind_host: "127.0.0.1".into(),
                            listen_port: 0,
                            target_host: Some("127.0.0.1".into()),
                            target_port: Some(target_port),
                        },
                        multiplex.clone(),
                    )
                    .unwrap();
                let mut client =
                    std::net::TcpStream::connect(("127.0.0.1", tunnel.listen_port)).unwrap();
                client
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                client.write_all(b"tunnel").unwrap();
                let mut returned = [0; 6];
                client.read_exact(&mut returned).unwrap();
                assert_eq!(&returned, b"tunnel");
                drop(client);
                tunnels.close(&tunnel.id).unwrap();
                sessions.close(&session.id).unwrap();
                multiplex.disconnect().unwrap();
            }),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), echo)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
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
