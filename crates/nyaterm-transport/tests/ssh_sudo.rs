//! Real encrypted channels against an in-process peer; no machine credentials or sudo policy.
use nyaterm_core::SecretString;
use nyaterm_transport::ssh_execution::sudo::{SudoError, open_sudo_exec};
use nyaterm_transport::{
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig, open_ssh_multiplex_handle,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{ChannelId, ChannelMsg, server};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

struct Peer {
    channels: Vec<russh::Channel<server::Msg>>,
    writes: Arc<AtomicUsize>,
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
        let command = String::from_utf8(command.to_vec()).unwrap();
        if command == "ordinary" {
            session.channel_success(id)?;
            session.data(id, b"login user".to_vec())?;
            session.exit_status_request(id, 0)?;
            session.eof(id)?;
            session.close(id)?;
            return Ok(());
        }
        if command.contains("fixture-exec-denied") {
            session.channel_failure(id)?;
            return Ok(());
        }
        assert!(!command.contains("fixture secret"));
        assert!(command.starts_with("LC_ALL=C sudo -S -p '"));
        let marker = |prefix: &str| {
            let start = command.find(prefix).unwrap();
            let end = start + command[start..].find(':').unwrap() + 1;
            command.as_bytes()[start..end].to_vec()
        };
        let prompt = marker("nyaterm-sudo-password-");
        let ready = marker("nyaterm-sudo-ready-");
        session.channel_success(id)?;
        let index = self
            .channels
            .iter()
            .position(|channel| channel.id() == id)
            .unwrap();
        let mut channel = self.channels.swap_remove(index);
        let handle = session.handle();
        let writes = self.writes.clone();
        tokio::spawn(async move {
            if command.contains("fixture-policy")
                || command.contains("fixture-tty")
                || command.contains("fixture-failed")
            {
                let diagnostic = if command.contains("fixture-policy") {
                    "test is not in the sudoers file"
                } else if command.contains("fixture-tty") {
                    "sudo: sorry, you must have a tty to run sudo"
                } else {
                    "unknown startup failure"
                };
                let _ = handle
                    .extended_data(id, 1, diagnostic.as_bytes().to_vec())
                    .await;
                let _ = handle.exit_status_request(id, 1).await;
                let _ = handle.close(id).await;
                return;
            }
            if command.contains("fixture-limit") {
                let _ = handle.extended_data(id, 1, vec![b'x'; 65537]).await;
                return;
            }
            if command.contains("fixture-wait") {
                while channel.wait().await.is_some() {}
                return;
            }
            if !command.contains("fixture-nopass") {
                for fragment in prompt.chunks(7) {
                    let _ = handle.extended_data(id, 1, fragment.to_vec()).await;
                }
                loop {
                    match channel.wait().await {
                        Some(ChannelMsg::Data { data }) => {
                            writes.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(
                                &data[..],
                                b"fixture secret\n",
                                "no protocol bytes before authentication"
                            );
                            if command.contains("fixture-reprompt") {
                                let _ = handle.extended_data(id, 1, prompt.clone()).await;
                                // Observe any erroneous second password send.
                                while let Some(event) = channel.wait().await {
                                    if matches!(event, ChannelMsg::Data { .. }) {
                                        writes.fetch_add(1, Ordering::SeqCst);
                                    }
                                }
                                return;
                            }
                            break;
                        }
                        Some(ChannelMsg::Close | ChannelMsg::Eof) | None => return,
                        _ => {}
                    }
                }
            }
            // This prefix is already read when the marker arrives and must not disappear.
            let _ = handle.data(id, b"prefix".to_vec()).await;
            for fragment in ready.chunks(5) {
                let _ = handle.extended_data(id, 1, fragment.to_vec()).await;
            }
            let _ = handle
                .extended_data(id, 1, b"stderr is not protocol".to_vec())
                .await;
            if command.contains("fixture-nopass-exit") {
                let _ = handle.eof(id).await;
                tokio::time::sleep(Duration::from_millis(75)).await;
                let _ = handle.exit_status_request(id, 7).await;
                let _ = handle.close(id).await;
                return;
            }
            while let Some(event) = channel.wait().await {
                match event {
                    ChannelMsg::Data { data } => {
                        let _ = handle.data(id, data.to_vec()).await;
                    }
                    ChannelMsg::Close | ChannelMsg::Eof => break,
                    _ => {}
                }
            }
            let _ = handle.close(id).await;
        });
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
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    let peer = tokio::spawn(async move {
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[23; 32])),
            "sudo-fixture",
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
                writes: count,
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
        writes,
        peer,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_and_nopass_streams_are_lossless_and_revocation_keeps_shared_connection() {
    for script in ["fixture-password", "fixture-nopass"] {
        let (config, writes, peer) = peer().await;
        tokio::task::spawn_blocking(move || {
            let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
            let handle = multiplex.clone();
            let options = config.clone();
            multiplex
                .block_on(async move {
                    let cancel = CancellationToken::new();
                    let mut stream = open_sudo_exec(
                        &options,
                        &handle,
                        script,
                        Some(SecretString::from("fixture secret")),
                        cancel.clone(),
                    )
                    .await
                    .unwrap();
                    let mut prefix = [0; 6];
                    stream.read_exact(&mut prefix).await.unwrap();
                    assert_eq!(&prefix, b"prefix");
                    stream.write_all(b"protocol bytes").await.unwrap();
                    let mut echoed = [0; 14];
                    stream.read_exact(&mut echoed).await.unwrap();
                    assert_eq!(&echoed, b"protocol bytes");
                    cancel.cancel();
                    let mut byte = [0; 1];
                    assert!(
                        tokio::time::timeout(Duration::from_secs(3), stream.read(&mut byte))
                            .await
                            .unwrap()
                            .is_err()
                    );
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                writes.load(Ordering::SeqCst),
                usize::from(script == "fixture-password")
            );
            let result = nyaterm_transport::ssh_execution::run_ssh_command(
                config,
                Some(multiplex.clone()),
                b"ordinary".to_vec(),
                Duration::from_secs(3),
            )
            .unwrap();
            assert_eq!(result.stdout, "login user");
            multiplex.disconnect().unwrap();
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), peer)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failures_are_typed_and_password_is_never_sent_twice() {
    let (config, writes, peer) = peer().await;
    tokio::task::spawn_blocking(move || {
        let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
        for (script, password, expected) in [
            ("fixture-password", false, SudoError::PasswordRequired),
            ("fixture-reprompt", true, SudoError::AuthenticationRejected),
            ("fixture-policy", true, SudoError::PolicyDenied),
            ("fixture-tty", true, SudoError::TerminalRequired),
            ("fixture-failed", true, SudoError::CommandFailed(Some(1))),
            ("fixture-exec-denied", true, SudoError::ExecUnavailable),
            ("fixture-limit", true, SudoError::OutputLimit),
            ("fixture-wait", true, SudoError::Cancelled),
            ("fixture-wait-timeout", true, SudoError::TimedOut),
        ] {
            let handle = multiplex.clone();
            let config = config.clone();
            let error = multiplex
                .block_on(async move {
                    let cancel = CancellationToken::new();
                    if script == "fixture-wait" {
                        let stop = cancel.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            stop.cancel();
                        });
                    }
                    let result = open_sudo_exec(
                        &config,
                        &handle,
                        script,
                        password.then(|| SecretString::from("fixture secret")),
                        cancel,
                    )
                    .await;
                    Ok(result.err().expect("startup must fail"))
                })
                .unwrap();
            assert_eq!(error, expected, "{script}");
        }
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        multiplex.disconnect().unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sudo_stream_observes_exit_status_after_stdout_eof() {
    let (config, _, peer) = peer().await;
    tokio::task::spawn_blocking(move || {
        let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
        let handle = multiplex.clone();
        multiplex
            .block_on(async move {
                let mut stream = open_sudo_exec(
                    &config,
                    &handle,
                    "fixture-nopass-exit",
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
                let mut output = Vec::new();
                stream.read_to_end(&mut output).await.unwrap();
                assert_eq!(output, b"prefix");
                tokio::time::timeout(Duration::from_secs(3), stream.completion().cancelled())
                    .await
                    .unwrap();
                assert_eq!(stream.exit_status(), Some(7));
                Ok(())
            })
            .unwrap();
        multiplex.disconnect().unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}
