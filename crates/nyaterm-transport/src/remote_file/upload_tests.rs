//! A real SSH client against a peer that executes the generated commands in a
//! POSIX shell. On Windows these tests can be explicitly run against WSL Ubuntu.
use std::collections::HashMap;
use std::io::Write as _;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};

use crate::remote_file::shell::{shell_quote, shell_quote_path};
use crate::remote_file::{RemoteFileBackendKind, RemoteFileService};
use crate::{
    SftpDuplicatePolicy, SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions,
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig,
};

fn posix(command: &str, input: &[u8]) -> Output {
    #[cfg(windows)]
    let mut process = {
        let mut process = Command::new("wsl.exe");
        process.args(["-d", "Ubuntu", "--exec", "sh"]);
        process
    };
    #[cfg(not(windows))]
    let mut process = Command::new("sh");
    let mut child = process
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn ok(command: &str) -> String {
    let output = posix(command, &[]);
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_string()
}

struct RemoteDirectory(String);
impl Drop for RemoteDirectory {
    fn drop(&mut self) {
        assert!(self.0.starts_with("/tmp/nyaterm-upload-test-"));
        ok(&format!("rm -rf -- {}", shell_quote(&self.0)));
    }
}

struct AcceptKey;
impl SshHostKeyVerifier for AcceptKey {
    fn verify(&self, _key: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

struct ShellPeer {
    channels: HashMap<ChannelId, Channel<Msg>>,
    pending: HashMap<ChannelId, (String, Vec<u8>)>,
    directory: Arc<Mutex<String>>,
    prefix: Arc<Mutex<String>>,
    commands: Arc<Mutex<Vec<String>>>,
}

impl russh::server::Handler for ShellPeer {
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
        let command = String::from_utf8(command.to_vec()).unwrap();
        self.commands.lock().unwrap().push(command.clone());
        let command = format!(
            "cd {} && {}\n{command}",
            shell_quote(&self.directory.lock().unwrap()),
            self.prefix.lock().unwrap()
        );
        self.pending.insert(id, (command, Vec::new()));
        session.channel_success(id)?;
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.pending.get_mut(&id).unwrap().1.extend_from_slice(data);
        Ok(())
    }
    async fn channel_eof(
        &mut self,
        id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let (command, input) = self.pending.remove(&id).unwrap();
        let output = tokio::task::spawn_blocking(move || posix(&command, &input))
            .await
            .unwrap();
        session.data(id, output.stdout)?;
        session.extended_data(id, 1, output.stderr)?;
        session.eof(id)?;
        session.exit_status_request(id, output.status.code().unwrap_or(255) as u32)?;
        session.close(id)?;
        Ok(())
    }
}

#[tokio::test]
#[cfg_attr(
    any(windows, target_os = "macos"),
    ignore = "Requires a GNU userland (WSL Ubuntu on Windows); run explicitly with --ignored"
)]
async fn posix_shell_upload_preserves_metadata_through_public_entry_points() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let directory = Arc::new(Mutex::new(String::new()));
    let prefix = Arc::new(Mutex::new(String::new()));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let config = Arc::new(russh::server::Config {
        keys: vec![
            russh::keys::PrivateKey::new(
                KeypairData::Ed25519(Ed25519Keypair::from_seed(&[11; 32])),
                "posix-upload-test",
            )
            .unwrap(),
        ],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let (peer_directory, peer_prefix, peer_commands) =
        (directory.clone(), prefix.clone(), commands.clone());
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let peer = ShellPeer {
                channels: HashMap::new(),
                pending: HashMap::new(),
                directory: peer_directory.clone(),
                prefix: peer_prefix.clone(),
                commands: peer_commands.clone(),
            };
            let config = config.clone();
            tokio::spawn(async move {
                let _ = russh::server::run_stream(config, socket, peer)
                    .await
                    .unwrap()
                    .await;
            });
        }
    });
    tokio::task::spawn_blocking(move || {
        let root = ok("mktemp -d /tmp/nyaterm-upload-test-XXXXXX");
        let _cleanup = RemoteDirectory(root.clone());
        *directory.lock().unwrap() = root.clone();
        let setup = |command: &str| ok(&format!("cd {} && {command}", shell_quote(&root)));
        let local_root = tempfile::tempdir().unwrap();
        let local = local_root.path().join("source");
        std::fs::write(&local, b"new\0data").unwrap();
        let config = SshSessionConfig { host: "127.0.0.1".into(), port, username: "test".into(),
            allow_none_auth: true, host_key_verifier: Some(Arc::new(AcceptKey)), ..Default::default() };
        let multiplex = crate::open_ssh_multiplex_handle(config.clone()).unwrap();
        let service = RemoteFileService::with_multiplex(config, multiplex.clone()).unwrap();
        let root_user = ok("id -u") == "0";
        for backend in [RemoteFileBackendKind::ScpNormal, RemoteFileBackendKind::ScpEnhanced] {
            *service.selected.lock().unwrap() = Some(backend);
            commands.lock().unwrap().clear();
            service.shell(backend).probe().unwrap();
            for mode in ["755", "640", "6755", "7644"] {
                for with_progress in [false, true] {
                    *prefix.lock().unwrap() = String::new();
                    setup(&format!("printf old > original && chmod {mode} original"));
                    if root_user { setup(&format!("chown 65534:65533 original && chmod {mode} original")); }
                    let before = setup("stat -c '%u:%g:%a' original");
                    if with_progress {
                        service.upload_file_with_progress_and_control_options(&local, "original",
                            SftpTransferControl::new(), SftpTransferOptions::default().with_default_file_permissions("644"), |_| {}).unwrap();
                    } else { service.upload_file(&local, "original").unwrap(); }
                    assert_eq!(setup("stat -c '%u:%g:%a' original"), before);
                    assert_eq!(posix(&format!("cat {}/original", shell_quote(&root)), &[]).stdout, b"new\0data");
                }
            }
            // A no-op ownership change must not be required for an ordinary user's file.
            setup("rm original && printf old > original && chmod 755 original");
            *prefix.lock().unwrap() = "chown() { return 1; }".into();
            service.upload_file(&local, "original").unwrap();
            // Attribute failures, including tools silently ignoring requests, preserve the original.
            for failure in ["chmod() { return 1; }", "chmod() { return 0; }",
                "test() { return 1; }", "cat() { command cat; printf extra; }", if backend == RemoteFileBackendKind::ScpNormal {
                    "ls() { return 1; }" } else { "stat() { return 1; }" }] {
                setup("printf old > original && chmod 755 original");
                let before = setup("stat -c '%u:%g:%a' original");
                *prefix.lock().unwrap() = failure.into();
                assert!(service.upload_file(&local, "original").is_err(), "{failure}");
                assert_eq!(setup("cat original"), "old");
                assert_eq!(setup("stat -c '%u:%g:%a' original"), before);
                assert!(setup("find . -name '*.nyaterm-upload-*' -print").is_empty());
            }
            if root_user {
                for failure in ["chown() { return 1; }", "chown() { return 0; }"] {
                    setup("printf old > original && chown 65534:65533 original && chmod 6755 original");
                    *prefix.lock().unwrap() = failure.into();
                    assert!(service.upload_file(&local, "original").is_err());
                    assert_eq!(setup("cat original"), "old");
                    assert_eq!(setup("stat -c '%u:%g:%a' original"), "65534:65533:6755");
                }
            }
            *prefix.lock().unwrap() = String::new();
            // A directory, dangling link, or special file cannot be replaced as a regular file.
            setup("mkdir -p target-dir && ln -sf absent target-link && { test -p target-fifo || mkfifo target-fifo; }");
            for target in ["target-dir", "target-link", "target-fifo"] {
                assert!(service.upload_file(&local, target).is_err());
            }
            // Quoting and POSIX operand handling, including a relative leading dash.
            let quoted = "-file ' [*].txt";
            setup(&format!("printf old > {} && chmod 755 {}", shell_quote(quoted), shell_quote_path(quoted)));
            service.upload_file(&local, quoted).unwrap();
            assert_eq!(setup(&format!("stat -c '%a' -- {}", shell_quote(quoted))), "755");
            for (name, configured, expected) in [("new-600", "600", "600"), ("new-empty", "", "600")] {
                setup(&format!("rm -f {name}"));
                service.upload_file_with_progress_and_control_options(&local, name, SftpTransferControl::new(),
                    SftpTransferOptions::default().with_default_file_permissions(configured), |_| {}).unwrap();
                assert_eq!(setup(&format!("stat -c '%a' {name}")), expected);
            }
            // Directory merges preserve an existing child's mode and configure only new children.
            let folder = local_root.path().join("folder");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("existing"), b"new").unwrap();
            std::fs::write(folder.join("new"), b"new").unwrap();
            setup("mkdir -p folder && printf old > folder/existing && chmod 755 folder/existing && rm -f folder/new");
            service.upload_path_with_progress_and_path_options(&folder, "folder", SftpTransferControl::new(),
                SftpPathTransferOptions::new(SftpDuplicatePolicy::Overwrite, None,
                    SftpTransferOptions::default().with_default_file_permissions("640")), |_| {}).unwrap();
            assert_eq!(setup("stat -c '%a' folder/existing"), "755");
            assert_eq!(setup("stat -c '%a' folder/new"), "640");
            if backend == RemoteFileBackendKind::ScpNormal {
                assert!(!commands.lock().unwrap().iter().any(|command| command.contains("stat ")));
            }
        }
        assert!(!multiplex.is_closed());

    }).await.unwrap();
    server.abort();
}
