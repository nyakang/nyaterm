use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};

use super::{Files, Peer, files};
use crate::session_config::{SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig};
use crate::sftp::SftpService;

struct AcceptTestKey;

impl SshHostKeyVerifier for AcceptTestKey {
    fn verify(&self, _key: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

struct SshPeer {
    channels: HashMap<ChannelId, Channel<Msg>>,
    files: Arc<Mutex<Files>>,
    commands: Arc<Mutex<Vec<Vec<u8>>>>,
    exec_status: Option<u32>,
    requests: Option<Arc<Mutex<Vec<&'static str>>>>,
    accept_shell: bool,
    accept_sftp: bool,
}

impl russh::server::Handler for SshPeer {
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

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(name, "sftp");
        if let Some(requests) = &self.requests {
            requests.lock().unwrap().push("sftp");
        }
        if !self.accept_sftp {
            session.channel_failure(id)?;
            session.close(id)?;
            return Ok(());
        }
        session.channel_success(id)?;
        let channel = self.channels.remove(&id).unwrap();
        let peer = Peer {
            files: self.files.clone(),
            listed: HashSet::new(),
        };
        tokio::spawn(russh_sftp::server::run(channel.into_stream(), peer));
        Ok(())
    }

    async fn shell_request(
        &mut self,
        id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(requests) = &self.requests {
            requests.lock().unwrap().push("shell");
        }
        if self.accept_shell {
            session.channel_success(id)?;
        } else {
            session.channel_failure(id)?;
            session.close(id)?;
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        id: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _width: u32,
        _height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(requests) = &self.requests {
            requests.lock().unwrap().push("pty");
        }
        session.channel_success(id)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.commands.lock().unwrap().push(command.to_vec());
        if let Some(status) = self.exec_status {
            session.channel_success(id)?;
            if status == 0 {
                self.files
                    .lock()
                    .unwrap()
                    .entries
                    .retain(|path, _| !path.starts_with("/dir"));
            }
            session.exit_status_request(id, status)?;
            session.eof(id)?;
            session.close(id)?;
        } else {
            // Model an SFTP-only server that rejects SSH exec channels.
            session.channel_failure(id)?;
            session.close(id)?;
        }
        Ok(())
    }
}

async fn delete_with_server(
    exec_status: Option<u32>,
    multiplexed: bool,
) -> (Vec<Vec<u8>>, Arc<Mutex<Files>>) {
    let files = files();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let key = russh::keys::PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&[7; 32])),
        "delete-test",
    )
    .unwrap();
    let server_config = Arc::new(russh::server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_files = files.clone();
    let server_commands = commands.clone();
    let accept_task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let handler = SshPeer {
                channels: HashMap::new(),
                files: server_files.clone(),
                commands: server_commands.clone(),
                exec_status,
                requests: None,
                accept_shell: false,
                accept_sftp: true,
            };
            let config = server_config.clone();
            tokio::spawn(async move {
                let session = russh::server::run_stream(config, socket, handler)
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
            let attempt = config.attempt.clone();
            let retry_config = config.clone();
            let terminal_config = config.clone();
            let mut retained_handle = None;
            let service = if multiplexed {
                let handle = crate::open_ssh_multiplex_handle(config.clone()).unwrap();
                retained_handle = Some(handle.clone());
                SftpService::with_multiplex(config, handle).unwrap()
            } else {
                SftpService::new(config)
            };
            if exec_status.is_none()
                && let Some(handle) = retained_handle.as_ref()
            {
                let manager = crate::SessionManager::new();
                let mut config = terminal_config.clone();
                config.deferred_pty = false;
                let terminal = manager
                    .create_ssh_session_with_multiplex(config, handle.clone())
                    .unwrap();
                assert_eq!(
                    handle.shell_availability(),
                    crate::connection_attempt::ShellAvailability::Unavailable
                );
                manager.close(&terminal.id).unwrap();
            }
            service.delete_path("/dir").unwrap();
            if exec_status.is_none() {
                assert_eq!(
                    attempt.shell_availability(),
                    crate::connection_attempt::ShellAvailability::Unavailable
                );
                let error = crate::remote_process::run_ssh_command(
                    retry_config,
                    None,
                    b"id".to_vec(),
                    Duration::from_secs(1),
                )
                .unwrap_err();
                assert!(
                    error
                        .downcast_ref::<crate::connection_attempt::ShellUnavailable>()
                        .is_some()
                );
                if let Some(handle) = retained_handle {
                    let manager = crate::SessionManager::new();
                    let mut terminal_config = terminal_config;
                    terminal_config.attempt = attempt.clone();
                    terminal_config.deferred_pty = false;
                    let terminal = manager
                        .create_ssh_session_with_multiplex(terminal_config, handle)
                        .unwrap();
                    assert!(
                        manager
                            .write(&terminal.id, b"command\r")
                            .unwrap_err()
                            .to_string()
                            .contains("SFTP-only")
                    );
                    assert!(manager.write_raw(&terminal.id, b"\xff").is_err());
                    manager.close(&terminal.id).unwrap();
                }
                assert_eq!(
                    crate::connection_attempt::ConnectionAttempt::default().shell_availability(),
                    crate::connection_attempt::ShellAvailability::Unknown
                );
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    accept_task.abort();
    let commands = commands.lock().unwrap().clone();
    (commands, files)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_directory_fast_path_skips_recursive_sftp_for_dedicated_and_multiplexed_connections() {
    for multiplexed in [false, true] {
        let (commands, files) = delete_with_server(Some(0), multiplexed).await;
        assert_eq!(commands, vec![b"rm -rf -- '/dir'".to_vec()]);
        let files = files.lock().unwrap();
        assert_eq!(files.entries.len(), 1);
        assert!(files.removed.is_empty());
        assert_eq!(files.lstat_calls, 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_and_unavailable_exec_fall_back_to_sftp_directory_deletion() {
    for status in [Some(1), None] {
        let (commands, files) = delete_with_server(status, true).await;
        assert_eq!(commands.len(), usize::from(status.is_some()));
        let files = files.lock().unwrap();
        assert_eq!(files.entries.len(), 1);
        assert_eq!(files.removed.len(), 5);
        assert_eq!(files.lstat_calls, 1);
    }
}

async fn runtime_mode_server(
    accept_shell: bool,
    accept_sftp: bool,
) -> (
    SshSessionConfig,
    Arc<Mutex<Vec<&'static str>>>,
    tokio::sync::oneshot::Receiver<russh::server::Handle>,
    tokio::task::JoinHandle<()>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&[8; 32])),
        "mode-test",
    )
    .unwrap();
    let config = Arc::new(russh::server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let peer = SshPeer {
            channels: HashMap::new(),
            files: files(),
            commands: Default::default(),
            exec_status: None,
            requests: Some(server_requests),
            accept_shell,
            accept_sftp,
        };
        let session = russh::server::run_stream(config, socket, peer)
            .await
            .unwrap();
        let _ = tx.send(session.handle());
        let _ = session.await;
    });
    (
        SshSessionConfig {
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            allow_none_auth: true,
            host_key_verifier: Some(Arc::new(AcceptTestKey)),
            deferred_pty: false,
            ..Default::default()
        },
        requests,
        rx,
        server,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_modes_send_only_the_expected_server_requests_and_release_sessions() {
    use nyaterm_core::models::ssh_runtime::SshRuntimeMode;
    for (mode, shell, sftp, expected, ready) in [
        (
            SshRuntimeMode::Terminal,
            true,
            true,
            vec!["pty", "shell"],
            true,
        ),
        (
            SshRuntimeMode::Terminal,
            false,
            true,
            vec!["pty", "shell"],
            false,
        ),
        (SshRuntimeMode::Sftp, true, true, vec!["sftp"], true),
        (SshRuntimeMode::Sftp, true, false, vec!["sftp"], false),
        (
            SshRuntimeMode::Standard,
            false,
            true,
            vec!["pty", "shell", "sftp"],
            true,
        ),
    ] {
        let (mut config, requests, _, server) = runtime_mode_server(shell, sftp).await;
        config.runtime_mode = mode;
        tokio::time::timeout(
            Duration::from_secs(15),
            tokio::task::spawn_blocking(move || {
                let handle = crate::open_ssh_multiplex_handle(config.clone()).unwrap();
                // A multiplexed copy has its own cancellation attempt while
                // observing shell capability discovered on the shared connection.
                config.bind_attempt(Default::default());
                let observed_config = config.clone();
                let manager = crate::SessionManager::new();
                let result = manager.create_ssh_session_with_multiplex(config, handle.clone());
                assert_eq!(result.is_ok(), ready);
                if let Ok(session) = result {
                    if mode == SshRuntimeMode::Sftp {
                        assert!(manager.write(&session.id, b"never execute\r").is_err());
                        assert_eq!(
                            handle.shell_availability(),
                            crate::connection_attempt::ShellAvailability::Unknown
                        );
                    }
                    if mode == SshRuntimeMode::Standard {
                        assert_eq!(
                            observed_config.effective_runtime_mode(),
                            SshRuntimeMode::Sftp
                        );
                    }
                    manager.close(&session.id).unwrap();
                }
                assert!(manager.list_sessions().unwrap().is_empty());
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(*requests.lock().unwrap(), expected, "mode {mode:?}");
        server.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_only_disconnect_emits_exit_and_disabled_or_cancelled_attempts_do_not_open_channels() {
    use nyaterm_core::models::ssh_runtime::SshRuntimeMode;
    let (mut config, _, server_handle, server) = runtime_mode_server(true, true).await;
    config.runtime_mode = SshRuntimeMode::Sftp;
    let manager = Arc::new(crate::SessionManager::new());
    let worker_manager = manager.clone();
    let session = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || worker_manager.create_ssh_session(config).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    server_handle
        .await
        .unwrap()
        .disconnect(russh::Disconnect::ByApplication, "test".into(), "".into())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(crate::SessionEvent::Exited { session_id, .. }) =
                manager.try_recv_event().unwrap()
            {
                assert_eq!(session_id, session.id);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let worker_manager = manager.clone();
    tokio::task::spawn_blocking(move || worker_manager.close(&session.id).unwrap())
        .await
        .unwrap();
    server.abort();
    for cancel in [false, true] {
        let (mut config, requests, _, server) = runtime_mode_server(true, true).await;
        config.runtime_mode = SshRuntimeMode::Sftp;
        if cancel {
            config.attempt.cancel();
        } else {
            config.sftp.enabled = false;
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                assert!(
                    crate::SessionManager::new()
                        .create_ssh_session(config)
                        .is_err()
                );
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(requests.lock().unwrap().is_empty());
        server.abort();
    }
}
