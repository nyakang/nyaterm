use crate::remote_file::{RemoteFileBackendKind, RemoteFileService};
use crate::sftp::SftpFileType;
use nyaterm_transport::{
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig, open_ssh_multiplex_handle,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct AcceptTestKey;

impl SshHostKeyVerifier for AcceptTestKey {
    fn verify(&self, _key: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

struct ListingPeer {
    channels: HashMap<ChannelId, Channel<Msg>>,
    pending: HashMap<ChannelId, String>,
    commands: Arc<Mutex<Vec<String>>>,
}

impl russh::server::Handler for ListingPeer {
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
        self.pending.insert(id, command);
        session.channel_success(id)?;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = self.pending.remove(&id).unwrap();
        if command.starts_with("cat -- ") {
            session.data(
                id,
                vec![0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65, 13, 0, 10, 0, 120, 0],
            )?;
            session.eof(id)?;
            session.exit_status_request(id, 0)?;
            session.close(id)?;
            return Ok(());
        }
        assert!(
            command.starts_with("LC_ALL=C ls -la ") || command.starts_with("LC_ALL=C ls -ld "),
            "unexpected command: {command}"
        );
        // Model a server whose account and group names both contain spaces.
        // Returning numeric fixtures unconditionally would hide a missing -n.
        let identities = if command.split_whitespace().any(|arg| arg == "-n") {
            "1000 1001"
        } else {
            "Domain User Domain Users"
        };
        let output = format!("-rw-r----- 1 {identities} 12 Jan 1 00:00 a file.txt\n");
        session.data(id, output.into_bytes())?;
        session.eof(id)?;
        session.exit_status_request(id, 0)?;
        session.close(id)?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ListingOperation {
    Directory,
    Properties,
    EncodedDocument,
}

async fn assert_numeric_ownership(operation: ListingOperation, ssh_compatibility_mode: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let peer_commands = commands.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let config = Arc::new(russh::server::Config {
            keys: vec![
                russh::keys::PrivateKey::new(
                    KeypairData::Ed25519(Ed25519Keypair::from_seed(&[12; 32])),
                    "normal-listing-test",
                )
                .unwrap(),
            ],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let peer = ListingPeer {
            channels: HashMap::new(),
            pending: HashMap::new(),
            commands: peer_commands,
        };
        let _ = russh::server::run_stream(config, socket, peer)
            .await
            .unwrap()
            .await;
    });
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            let config = SshSessionConfig {
                ssh_compatibility_mode,
                host: "127.0.0.1".into(),
                port,
                username: "test".into(),
                allow_none_auth: true,
                host_key_verifier: Some(Arc::new(AcceptTestKey)),
                ..Default::default()
            };
            let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
            let service = RemoteFileService::with_multiplex(config, multiplex).unwrap();
            *service.selected.lock().unwrap() = Some(RemoteFileBackendKind::ScpNormal);
            match operation {
                ListingOperation::Directory => {
                    let entries = service.list_dir("-team's files").unwrap();
                    assert_eq!(entries.len(), 1);
                    let entry = &entries[0];
                    assert_eq!(entry.name, "a file.txt");
                    assert_eq!(entry.path, "-team's files/a file.txt");
                    assert_eq!(entry.file_type, SftpFileType::File);
                    assert_eq!(entry.size, Some(12));
                    assert_eq!(entry.permissions, Some(0o640));
                    assert_eq!(entry.owner, "1000");
                    assert_eq!(entry.group, "1001");
                }
                ListingOperation::EncodedDocument => {
                    use nyaterm_core::file_encoding::{FileEncoding, FileReadEncoding};
                    let file = service
                        .read_document_bytes(
                            &crate::sftp::RemoteFilePath::new("-team's files/a file.txt"),
                            1024,
                        )
                        .unwrap();
                    let revision = file.revision.clone();
                    let decoded = file.decode(FileReadEncoding::Auto).unwrap();
                    assert_eq!(decoded.document.content, "中文\r\nx");
                    assert_eq!(decoded.format.encoding, FileEncoding::Utf16Le);
                    assert!(decoded.format.bom);
                    assert!(decoded.round_trips);
                    assert_eq!(decoded.document.revision, revision);
                }
                ListingOperation::Properties => {
                    let properties = service.file_properties("-team's files/a file.txt").unwrap();
                    assert_eq!(properties.name, "a file.txt");
                    assert_eq!(properties.path, "-team's files/a file.txt");
                    assert_eq!(properties.file_type, SftpFileType::File);
                    assert_eq!(properties.size, Some(12));
                    assert_eq!(properties.permissions, Some(0o640));
                    assert_eq!(properties.permissions_symbolic, "-rw-r-----");
                    assert_eq!(properties.owner, "1000");
                    assert_eq!(properties.group, "1001");
                    assert_eq!(properties.uid, None);
                    assert_eq!(properties.gid, None);
                }
            }
        }),
    )
    .await;
    server.abort();
    result.unwrap().unwrap();
    if matches!(operation, ListingOperation::EncodedDocument) {
        let commands = commands.lock().unwrap();
        assert_eq!(commands.len(), 3);
        assert!(commands[0].starts_with("LC_ALL=C ls -ld -n "));
        assert!(commands[1].starts_with("cat -- "));
        assert_eq!(commands[0], commands[2]);
        return;
    }
    let expected_command = match operation {
        ListingOperation::EncodedDocument => unreachable!(),
        ListingOperation::Directory => "LC_ALL=C ls -la -n './-team'\\''s files'",
        ListingOperation::Properties => "LC_ALL=C ls -ld -n './-team'\\''s files/a file.txt'",
    };
    assert_eq!(*commands.lock().unwrap(), [expected_command]);
}

#[tokio::test]
async fn normal_directory_listing_requests_numeric_ownership() {
    for enabled in [false, true] {
        assert_numeric_ownership(ListingOperation::Directory, enabled).await;
    }
}

#[tokio::test]
async fn normal_file_properties_request_numeric_ownership() {
    for enabled in [false, true] {
        assert_numeric_ownership(ListingOperation::Properties, enabled).await;
    }
}

#[tokio::test]
async fn encoded_shell_document_checks_metadata_around_raw_utf16_read() {
    for enabled in [false, true] {
        assert_numeric_ownership(ListingOperation::EncodedDocument, enabled).await;
    }
}
