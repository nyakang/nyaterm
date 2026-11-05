use super::{ShellRemote, TransferActivity, receive_file, send_file};
use crate::remote_file::RemoteFileBackendKind;
use crate::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};
use crate::sftp_transfer_types::SftpTransferOptions;
use nyaterm_transport::{SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, ChannelMsg};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn streams_upload_in_blocks_and_collects_download_status_after_eof() {
    let control = SftpTransferControl::new();
    let activity = TransferActivity::new(Duration::from_secs(1));
    let input = vec![42_u8; 4096];
    let mut source = input.as_slice();
    let mut output = Vec::new();
    let mut sent = Vec::new();
    assert_eq!(
        send_file(
            &mut source,
            &mut output,
            512,
            &control,
            &activity,
            &mut |n| sent.push(n)
        )
        .await
        .unwrap(),
        4096
    );
    assert_eq!(output, input);
    assert_eq!(sent, vec![512, 1024, 1536, 2048, 2560, 3072, 3584, 4096]);
    let messages = futures::stream::iter([
        ChannelMsg::Data {
            data: vec![1_u8; 512].into(),
        },
        ChannelMsg::Data {
            data: vec![2_u8; 512].into(),
        },
        ChannelMsg::Eof,
        ChannelMsg::ExitStatus { exit_status: 0 },
    ]);
    let mut output = Vec::new();
    let mut received = Vec::new();
    assert_eq!(
        receive_file(messages, &mut output, true, &control, &activity, &mut |n| {
            received.push(n)
        })
        .await
        .unwrap(),
        1024
    );
    assert_eq!(received, vec![512, 1024]);
    assert_eq!(&output[..512], &[1_u8; 512]);
    assert_eq!(&output[512..], &[2_u8; 512]);
}

#[tokio::test]
async fn rejects_failed_and_missing_exit_status_even_after_receiving_data() {
    for status in [None, Some(1)] {
        let mut messages = vec![
            ChannelMsg::Data {
                data: vec![1_u8; 512].into(),
            },
            ChannelMsg::Eof,
        ];
        if let Some(exit_status) = status {
            messages.push(ChannelMsg::ExitStatus { exit_status });
        }
        let error = receive_file(
            futures::stream::iter(messages),
            tokio::io::sink(),
            true,
            &SftpTransferControl::new(),
            &TransferActivity::new(Duration::from_secs(1)),
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("remote file transfer failed"));
    }
}

#[tokio::test]
async fn idle_timeout_and_cancellation_interrupt_a_silent_peer() {
    let control = SftpTransferControl::new();
    let activity = TransferActivity::new(Duration::from_millis(40));
    let error = activity
        .step(&control, std::future::pending::<()>())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    let activity = TransferActivity::new(Duration::from_secs(30));
    let cancel = control.clone();
    let cancel_task = async move {
        tokio::task::yield_now().await;
        cancel.cancel();
    };
    let (result, _) = tokio::join!(
        activity.step(&control, std::future::pending::<()>()),
        cancel_task
    );
    assert_eq!(result.unwrap_err().to_string(), SFTP_TRANSFER_CANCELLED);
}

#[tokio::test]
async fn pause_does_not_consume_idle_budget_and_upload_activity_keeps_reply_wait_alive() {
    let control = SftpTransferControl::new();
    control.pause();
    let resume = control.clone();
    let activity = TransferActivity::new(Duration::from_millis(80));
    let resume_task = async move {
        tokio::time::sleep(Duration::from_millis(160)).await;
        resume.resume();
    };
    let (result, _) = tokio::join!(activity.step(&control, async { 42 }), resume_task);
    assert_eq!(result.unwrap(), 42);
    let touch = activity.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let upload = async move {
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(30)).await;
            touch.touch();
        }
        sender.send(()).unwrap();
    };
    let (result, _) = tokio::join!(activity.step(&control, receiver), upload);
    result.unwrap().unwrap();
}

struct AcceptTestKey;
impl SshHostKeyVerifier for AcceptTestKey {
    fn verify(&self, _key: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

struct FilePeer {
    channels: HashMap<ChannelId, Channel<Msg>>,
    uploads: HashSet<ChannelId>,
    uploaded: Arc<Mutex<Vec<u8>>>,
    commands: Arc<Mutex<Vec<String>>>,
    status: u32,
}

impl russh::server::Handler for FilePeer {
    type Error = russh::Error;
    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(command).into_owned();
        self.commands.lock().unwrap().push(command.clone());
        session.channel_success(id)?;
        if command.starts_with("if test -e") {
            session.data(id, b"missing".to_vec())?;
            session.eof(id)?;
            session.exit_status_request(id, 0)?;
            session.close(id)?;
        } else if command.starts_with("umask") {
            self.uploads.insert(id);
        } else {
            if command.starts_with("cat --") {
                for _ in 0..16 {
                    session.data(id, vec![42_u8; 16384])?;
                }
            }
            session.eof(id)?;
            session.exit_status_request(id, self.status)?;
            session.close(id)?;
        }
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.uploads.contains(&id) {
            self.uploaded.lock().unwrap().extend_from_slice(data);
        }
        Ok(())
    }
    async fn channel_eof(
        &mut self,
        id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.uploads.remove(&id) {
            session.eof(id)?;
            session.exit_status_request(id, self.status)?;
            session.close(id)?;
        }
        Ok(())
    }
}

#[tokio::test]
async fn real_ssh_streams_report_partial_bytes_and_failed_download_keeps_existing_file() {
    for status in [0, 1] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[9; 32])),
            "file-test",
        )
        .unwrap();
        let config = Arc::new(russh::server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let uploaded = Arc::new(Mutex::new(Vec::new()));
        let server_uploaded = uploaded.clone();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let server_commands = commands.clone();
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let peer = FilePeer {
                    channels: HashMap::new(),
                    uploads: HashSet::new(),
                    uploaded: server_uploaded.clone(),
                    commands: server_commands.clone(),
                    status,
                };
                let config = config.clone();
                tokio::spawn(async move {
                    let session = russh::server::run_stream(config, socket, peer)
                        .await
                        .unwrap();
                    let _ = session.await;
                });
            }
        });
        let config = SshSessionConfig {
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            allow_none_auth: true,
            host_key_verifier: Some(Arc::new(AcceptTestKey)),
            ..Default::default()
        };
        tokio::time::timeout(
            Duration::from_secs(15),
            tokio::task::spawn_blocking(move || {
                let root = tempfile::tempdir().unwrap();
                let source = root.path().join("upload");
                let target = root.path().join("download");
                let payload = vec![42_u8; 256 * 1024];
                std::fs::write(&source, &payload).unwrap();
                std::fs::write(&target, b"original").unwrap();
                let multiplex =
                    nyaterm_transport::open_ssh_multiplex_handle(config.clone()).unwrap();
                let shell = ShellRemote::new(
                    config,
                    Some(multiplex.clone()),
                    RemoteFileBackendKind::ScpNormal,
                );
                let samples = Arc::new(Mutex::new(Vec::new()));
                let record = samples.clone();
                let result = shell.upload_stream(
                    &source,
                    "/target",
                    SftpTransferControl::new(),
                    &SftpTransferOptions::default(),
                    move |n| record.lock().unwrap().push(n),
                );
                assert_eq!(result.is_ok(), status == 0);
                assert_eq!(*uploaded.lock().unwrap(), payload);
                assert!(
                    samples
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|n| *n > 0 && *n < payload.len() as u64)
                );
                let samples = Arc::new(Mutex::new(Vec::new()));
                let record = samples.clone();
                let result = shell.download_stream(
                    "/source",
                    &target,
                    root.path(),
                    Some(payload.len() as u64),
                    Some(1_700_000_000),
                    SftpTransferControl::new(),
                    move |n| record.lock().unwrap().push(n),
                );
                assert_eq!(result.is_ok(), status == 0);
                assert!(
                    samples
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|n| *n > 0 && *n < payload.len() as u64)
                );
                if status == 0 {
                    assert_eq!(std::fs::read(&target).unwrap(), payload);
                    assert_eq!(
                        std::fs::metadata(&target).unwrap().modified().unwrap(),
                        std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
                    );
                } else {
                    assert_eq!(std::fs::read(&target).unwrap(), b"original");
                }
                if status == 0 {
                    std::fs::write(&target, b"preserve-on-cancel").unwrap();
                    let cancel = SftpTransferControl::new();
                    let stop = cancel.clone();
                    let result = shell.download_stream(
                        "/source",
                        &target,
                        root.path(),
                        Some(payload.len() as u64),
                        None,
                        cancel,
                        move |bytes| {
                            if bytes > 0 {
                                stop.cancel();
                            }
                        },
                    );
                    assert_eq!(result.unwrap_err().to_string(), SFTP_TRANSFER_CANCELLED);
                    assert_eq!(std::fs::read(&target).unwrap(), b"preserve-on-cancel");
                    let result = shell.download_stream(
                        "/source",
                        &target,
                        root.path(),
                        Some(payload.len() as u64 + 1),
                        None,
                        SftpTransferControl::new(),
                        |_| {},
                    );
                    assert!(result.unwrap_err().to_string().contains("size changed"));
                    assert_eq!(std::fs::read(&target).unwrap(), b"preserve-on-cancel");
                }
                assert!(
                    !multiplex.is_closed(),
                    "a file failure must not close the shared session"
                );
                assert_eq!(
                    std::fs::read_dir(root.path()).unwrap().count(),
                    2,
                    "download temporary must be reaped"
                );
                if status != 0 {
                    assert!(
                        commands
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|command| command.starts_with("rm -f --"))
                    );
                }
            }),
        )
        .await
        .unwrap()
        .unwrap();
        server.abort();
    }
}
