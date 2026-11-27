//! Exercise channel teardown over an actual encrypted SSH connection.
use crate::session_event_queue::SessionEventQueue;
use crate::x11::{X11ChannelOpen, X11DisplayTarget, handle_x11_channel};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{Channel, ChannelMsg, client, server};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

struct Peer;

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }
}

struct Client(mpsc::UnboundedSender<Channel<client::Msg>>);

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: Channel<client::Msg>,
        _: &str,
        _: u32,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        self.0.send(channel).unwrap();
        reply.accept().await;
        Ok(())
    }
}

struct Connection {
    client: client::Handle<Client>,
    server: server::RunningSession<Peer>,
    incoming: mpsc::UnboundedReceiver<Channel<client::Msg>>,
}

impl Connection {
    async fn new() -> Self {
        let (client_stream, server_stream) = tokio::io::duplex(128 * 1024);
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[21; 32])),
            "x11-test",
        )
        .unwrap();
        let (sender, incoming) = mpsc::unbounded_channel();
        let (server, client) = tokio::join!(
            server::run_stream(
                Arc::new(server::Config {
                    keys: vec![key],
                    ..Default::default()
                }),
                server_stream,
                Peer,
            ),
            client::connect_stream(
                Arc::new(client::Config::default()),
                client_stream,
                Client(sender),
            ),
        );
        let mut client = client.unwrap();
        assert!(client.authenticate_none("fixture").await.unwrap().success());
        Self {
            client,
            server: server.unwrap(),
            incoming,
        }
    }

    async fn channel(&mut self) -> (Channel<server::Msg>, Channel<client::Msg>) {
        let remote = self
            .server
            .handle()
            .channel_open_x11("127.0.0.1", 12345)
            .await
            .unwrap();
        (remote, self.incoming.recv().await.unwrap())
    }

    async fn close(self) {
        self.client
            .disconnect(russh::Disconnect::ByApplication, "fixture finished", "")
            .await
            .unwrap();
        self.server.await.unwrap();
    }
}

fn setup(cookie: Option<u8>) -> Vec<u8> {
    let mut packet = vec![b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    if let Some(cookie) = cookie {
        packet[6] = 18;
        packet[8] = 16;
        packet.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
        packet.extend_from_slice(&[cookie; 16]);
    }
    packet
}

#[tokio::test]
async fn rejected_x11_auth_closes_remote_channel_without_forwarding_credentials() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut connection = Connection::new().await;
        for packet in [setup(None), setup(Some(3)), vec![b'?'; 12]] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (mut remote, channel) = connection.channel().await;
            let worker = tokio::spawn(handle_x11_channel(
                SessionEventQueue::new(),
                "x11-fixture".into(),
                X11ChannelOpen {
                    channel,
                    originator_address: "127.0.0.1".into(),
                    originator_port: 12345,
                },
                X11DisplayTarget::Tcp {
                    host: "127.0.0.1".into(),
                    port: listener.local_addr().unwrap().port(),
                },
                None,
                vec![1; 16],
                Some(vec![2; 16]),
            ));
            let (mut local, _) = listener.accept().await.unwrap();
            remote.data(packet.as_slice()).await.unwrap();
            assert!(worker.await.unwrap().is_err());
            assert_eq!(local.read(&mut [0; 1]).await.unwrap(), 0);
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    match remote.wait().await {
                        Some(ChannelMsg::Close) | None => break,
                        Some(ChannelMsg::Data { .. }) => panic!("unexpected X11 response"),
                        _ => {}
                    }
                }
            })
            .await
            .expect(
                "rejected authentication must close the SSH channel, not leave the app waiting",
            );
        }
        connection.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn authenticated_x11_relays_data_and_closes_remote_channel_when_local_server_closes() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut connection = Connection::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut remote, channel) = connection.channel().await;
        let worker = tokio::spawn(handle_x11_channel(
            SessionEventQueue::new(),
            "x11-fixture".into(),
            X11ChannelOpen {
                channel,
                originator_address: "127.0.0.1".into(),
                originator_port: 12345,
            },
            X11DisplayTarget::Tcp {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
            },
            None,
            vec![1; 16],
            Some(vec![2; 16]),
        ));
        let (mut local, _) = listener.accept().await.unwrap();
        let packet = setup(Some(1));
        // Real SSH packet boundaries must not change setup authentication.
        for fragment in packet.chunks(7) {
            remote.data(fragment).await.unwrap();
        }
        let mut received = vec![0; packet.len()];
        local.read_exact(&mut received).await.unwrap();
        assert_eq!(received, setup(Some(2)));
        remote.data(&b"request"[..]).await.unwrap();
        let mut request = [0; 7];
        local.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        local.write_all(b"response").await.unwrap();
        local.shutdown().await.unwrap();
        worker.await.unwrap().unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match remote.wait().await {
                    Some(ChannelMsg::Data { data }) => response.extend_from_slice(&data),
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        })
        .await
        .expect("local server shutdown must release the remote SSH channel");
        assert_eq!(response, b"response");
        connection.close().await;
    })
    .await
    .unwrap();
}
