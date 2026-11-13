use nyaterm_files::remote_file::{RemoteFileBackendKind, RemoteFileService};
use nyaterm_files::sftp::sudo::{FileAccessMode, SftpSudoGrant};
use nyaterm_transport::{
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig, open_ssh_multiplex_handle,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{ChannelId, server};
use russh_sftp::protocol::{Attrs, File, FileAttributes, Handle, Name, Status, StatusCode};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

#[derive(Default)]
struct Counts {
    authentications: AtomicUsize,
    ordinary: AtomicUsize,
    elevated: AtomicUsize,
    renames: AtomicUsize,
    reject: AtomicBool,
}
struct SftpPeer {
    root: bool,
    listed: bool,
    counts: Arc<Counts>,
}
impl russh_sftp::server::Handler for SftpPeer {
    type Error = StatusCode;
    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }
    async fn realpath(&mut self, id: u32, _: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::new("/home/test", FileAttributes::default())],
        })
    }
    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        if path.starts_with("/root") && !self.root {
            return Err(StatusCode::PermissionDenied);
        }
        Ok(Attrs {
            id,
            attrs: FileAttributes {
                permissions: Some(0o040755),
                ..Default::default()
            },
        })
    }
    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if path.starts_with("/root") && !self.root {
            return Err(StatusCode::PermissionDenied);
        }
        self.listed = false;
        Ok(Handle { id, handle: path })
    }
    async fn readdir(&mut self, id: u32, _: String) -> Result<Name, Self::Error> {
        if self.listed {
            return Err(StatusCode::Eof);
        }
        self.listed = true;
        Ok(Name {
            id,
            files: vec![File::new(
                if self.root { "root-file" } else { "user-file" },
                FileAttributes {
                    permissions: Some(0o100600),
                    ..Default::default()
                },
            )],
        })
    }
    async fn rename(&mut self, id: u32, old: String, _: String) -> Result<Status, Self::Error> {
        if old.starts_with("/root") && !self.root {
            return Err(StatusCode::PermissionDenied);
        }
        self.counts.renames.fetch_add(1, Ordering::SeqCst);
        Ok(ok(id))
    }
    async fn close(&mut self, id: u32, _: String) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }
}
fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}
struct Peer {
    channels: HashMap<ChannelId, russh::Channel<server::Msg>>,
    counts: Arc<Counts>,
}
impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        self.counts.authentications.fetch_add(1, Ordering::SeqCst);
        Ok(server::Auth::Accept)
    }
    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(name, "sftp");
        self.counts.ordinary.fetch_add(1, Ordering::SeqCst);
        session.channel_success(id)?;
        let channel = self.channels.remove(&id).unwrap();
        tokio::spawn(russh_sftp::server::run(
            channel.into_stream(),
            SftpPeer {
                root: false,
                listed: false,
                counts: self.counts.clone(),
            },
        ));
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8(command.to_vec()).unwrap();
        if command == "mcp-wait" {
            session.channel_success(id)?;
            session.data(id, b"exec-output".to_vec())?;
            return Ok(());
        }
        if command.contains("$HOME") {
            session.channel_success(id)?;
            session.data(id, b"/home/test".to_vec())?;
            session.exit_status_request(id, 0)?;
            session.eof(id)?;
            session.close(id)?;
            return Ok(());
        }
        if !command.starts_with("LC_ALL=C sudo ") {
            session.channel_failure(id)?;
            session.close(id)?;
            return Ok(());
        }
        self.counts.elevated.fetch_add(1, Ordering::SeqCst);
        if self.counts.reject.load(Ordering::SeqCst) {
            session.channel_failure(id)?;
            session.close(id)?;
            return Ok(());
        }
        session.channel_success(id)?;
        let start = command.find("nyaterm-sudo-ready-").unwrap();
        let end = start + command[start..].find(':').unwrap() + 1;
        session.extended_data(id, 1, command.as_bytes()[start..end].to_vec())?;
        let channel = self.channels.remove(&id).unwrap();
        tokio::spawn(russh_sftp::server::run(
            channel.into_stream(),
            SftpPeer {
                root: true,
                listed: false,
                counts: self.counts.clone(),
            },
        ));
        Ok(())
    }

    async fn signal(
        &mut self,
        id: ChannelId,
        signal: russh::Sig,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert!(matches!(signal, russh::Sig::INT));
        session.exit_status_request(id, 130)?;
        session.eof(id)?;
        session.close(id)?;
        Ok(())
    }
}
struct Verify;
impl SshHostKeyVerifier for Verify {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}
async fn peer() -> (SshSessionConfig, Arc<Counts>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counts = Arc::new(Counts::default());
    let state = counts.clone();
    let worker = tokio::spawn(async move {
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[29; 32])),
            "sftp-sudo-fixture",
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
                channels: HashMap::new(),
                counts: state,
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
            username: "fixture".into(),
            allow_none_auth: true,
            host_key_verifier: Some(Arc::new(Verify)),
            ..Default::default()
        },
        counts,
        worker,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_exec_cancellation_preserves_concurrent_sftp_on_the_same_transport() {
    use nyaterm_transport::ssh_execution::streaming::{SshExecEvent, stream_ssh_command};
    use tokio_util::sync::CancellationToken;

    let (config, counts, peer) = peer().await;
    let dial = config.clone();
    let connection = tokio::task::spawn_blocking(move || open_ssh_multiplex_handle(dial))
        .await
        .unwrap()
        .unwrap();
    let cancellation = CancellationToken::new();
    let (sender, mut events) = tokio::sync::mpsc::channel(64);
    let exec = tokio::spawn(stream_ssh_command(
        config.clone(),
        Some(connection.clone()),
        b"mcp-wait".to_vec(),
        Duration::from_secs(5),
        cancellation.clone(),
        sender,
    ));
    loop {
        match tokio::time::timeout(Duration::from_secs(6), events.recv())
            .await
            .unwrap()
        {
            Some(SshExecEvent::Started) => break,
            Some(_) => {}
            None => panic!("the Exec channel did not start"),
        }
    }
    let file_connection = connection.clone();
    let files = tokio::task::spawn_blocking(move || {
        let service = RemoteFileService::with_multiplex(config, file_connection).unwrap();
        assert_eq!(service.home_dir().unwrap(), "/home/test");
        for _ in 0..20 {
            assert_eq!(service.list_dir(".").unwrap()[0].name, "user-file");
        }
    });
    cancellation.cancel();
    let mut output = Vec::new();
    let mut exited = false;
    while let Some(event) = events.recv().await {
        match event {
            SshExecEvent::Output { data, .. } => output.extend(data),
            SshExecEvent::ExitStatus(130) => exited = true,
            _ => {}
        }
    }
    exec.await.unwrap();
    files.await.unwrap();
    assert!(exited);
    assert_eq!(output, b"exec-output");
    assert!(counts.ordinary.load(Ordering::SeqCst) >= 20);
    assert_eq!(counts.authentications.load(Ordering::SeqCst), 1);
    assert!(!connection.is_closed());
    tokio::task::spawn_blocking(move || connection.disconnect().unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_and_sudo_caches_are_isolated_and_revocation_reaches_old_clones() {
    for compatibility in [false, true] {
        let (mut config, counts, peer) = peer().await;
        config.sftp.compatibility_mode = compatibility;
        tokio::task::spawn_blocking(move || {
            let connection = open_ssh_multiplex_handle(config.clone()).unwrap();
            let ordinary =
                RemoteFileService::with_multiplex(config.clone(), connection.clone()).unwrap();
            assert_eq!(ordinary.list_dir(".").unwrap()[0].name, "user-file");
            let grant = SftpSudoGrant::new(connection.clone(), None);
            let root =
                RemoteFileService::with_sudo(config.clone(), connection.clone(), grant.lease())
                    .unwrap();
            assert_eq!(root.access_mode(), FileAccessMode::SudoRoot);
            assert!(!ordinary.same_execution_identity(&root));
            assert_eq!(root.endpoint_key(), ordinary.endpoint_key());
            assert_eq!(root.list_dir("/root").unwrap()[0].name, "root-file");
            let old = root.clone();
            assert!(root.same_execution_identity(&old));
            old.rename_path("/root/a", "/root/b").unwrap();
            assert!(ordinary.list_dir("/root").is_err());
            assert_eq!(ordinary.list_dir(".").unwrap()[0].name, "user-file");
            if compatibility {
                assert_eq!(counts.elevated.load(Ordering::SeqCst), 1);
            } else {
                assert!(counts.elevated.load(Ordering::SeqCst) >= 2);
            }
            grant.close();
            assert!(!old.authorization_is_valid());
            assert!(old.rename_path("/root/b", "/root/c").is_err());
            assert!(root.list_dir("/root").is_err());
            assert_eq!(counts.renames.load(Ordering::SeqCst), 1);
            assert_eq!(ordinary.list_dir(".").unwrap()[0].name, "user-file");
            let second = SftpSudoGrant::new(connection.clone(), None);
            let renewed =
                RemoteFileService::with_sudo(config, connection.clone(), second.lease()).unwrap();
            assert!(!old.same_execution_identity(&renewed));
            assert!(renewed.list_dir("/root").is_ok());
            drop(second);
            assert!(renewed.list_dir("/root").is_err());
            connection.disconnect().unwrap();
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
async fn sudo_failure_never_opens_a_subsystem_or_changes_the_ordinary_backend() {
    let (config, counts, peer) = peer().await;
    counts.reject.store(true, Ordering::SeqCst);
    tokio::task::spawn_blocking(move || {
        let connection = open_ssh_multiplex_handle(config.clone()).unwrap();
        let ordinary =
            RemoteFileService::with_multiplex(config.clone(), connection.clone()).unwrap();
        let grant = SftpSudoGrant::new(connection.clone(), None);
        let root = RemoteFileService::with_sudo(config, connection.clone(), grant.lease()).unwrap();
        let error = root.list_dir("/root").unwrap_err();
        assert_eq!(
            error.downcast_ref::<nyaterm_transport::ssh_execution::sudo::SudoError>(),
            Some(&nyaterm_transport::ssh_execution::sudo::SudoError::ExecUnavailable)
        );
        assert_eq!(counts.elevated.load(Ordering::SeqCst), 1);
        assert_eq!(counts.ordinary.load(Ordering::SeqCst), 0);
        assert_eq!(ordinary.selected_backend(), None);
        assert_eq!(root.selected_backend(), Some(RemoteFileBackendKind::Sftp));
        assert!(!root.authorization_is_valid());
        assert!(root.list_dir(".").is_err());
        assert!(ordinary.list_dir(".").is_ok());
        connection.disconnect().unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_a_grant_interrupts_a_paused_file_operation() {
    let (config, _, peer) = peer().await;
    tokio::task::spawn_blocking(move || {
        let connection = open_ssh_multiplex_handle(config.clone()).unwrap();
        let grant = SftpSudoGrant::new(connection.clone(), None);
        let root = RemoteFileService::with_sudo(config.clone(), connection.clone(), grant.lease())
            .unwrap();
        root.list_dir("/root").unwrap();
        let control = nyaterm_files::sftp::SftpTransferControl::new();
        control.pause();
        let paused = control.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let temporary = tempfile::tempdir().unwrap();
            let result = root.download_path_with_progress_and_path_options(
                "/root/file",
                temporary.path().join("download"),
                paused,
                Default::default(),
                |_| {},
            );
            tx.send(result.is_err()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        grant.close();
        let stopped_without_resume = rx.recv_timeout(Duration::from_secs(2));
        control.cancel();
        worker.join().unwrap();
        assert!(stopped_without_resume.unwrap());
        let ordinary = RemoteFileService::with_multiplex(config, connection.clone()).unwrap();
        assert!(ordinary.list_dir(".").is_ok());
        connection.disconnect().unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}
