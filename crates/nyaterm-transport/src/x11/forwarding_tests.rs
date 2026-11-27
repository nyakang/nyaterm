//! Exercise channel teardown over an actual encrypted SSH connection.
use crate::session_event_queue::SessionEventQueue;
use crate::x11::{X11ChannelOpen, X11ForwardingConfig, handle_x11_channel};
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

fn forwarding_config(port: u16) -> X11ForwardingConfig {
    let mut config = X11ForwardingConfig::prepared(&nyaterm_core::x11::X11Connection {
        endpoint: nyaterm_core::x11::X11Endpoint::Tcp {
            host: "127.0.0.1".into(),
            port,
        },
        cookie: Some(zeroize::Zeroizing::new(vec![2; 16])),
        lifetime: None,
    })
    .unwrap();
    config.fake_cookie = vec![1; 16];
    config
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
                forwarding_config(listener.local_addr().unwrap().port()),
            ));
            remote.data(packet.as_slice()).await.unwrap();
            assert!(worker.await.unwrap().is_err());
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err(),
                "invalid authentication must not even connect to the local server"
            );
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
            forwarding_config(listener.local_addr().unwrap().port()),
        ));
        let packet = setup(Some(1));
        // Real SSH packet boundaries must not change setup authentication.
        for fragment in packet.chunks(7) {
            remote.data(fragment).await.unwrap();
        }
        let (mut local, _) = listener.accept().await.unwrap();
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

type AcquireReply = tokio::sync::oneshot::Sender<
    Result<nyaterm_core::x11::X11ChannelConnection, nyaterm_core::x11::X11Error>,
>;
struct DeferredProvider {
    requests: mpsc::UnboundedSender<AcquireReply>,
}
impl nyaterm_core::x11::X11ServerProvider for DeferredProvider {
    fn acquire(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        Result<nyaterm_core::x11::X11ChannelConnection, nyaterm_core::x11::X11Error>,
    > {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.requests.send(reply).unwrap();
        Box::pin(async {
            result
                .await
                .unwrap_or(Err(nyaterm_core::x11::X11Error::Cancelled))
        })
    }
}
fn lazy_config() -> (X11ForwardingConfig, mpsc::UnboundedReceiver<AcquireReply>) {
    let (requests, receive) = mpsc::unbounded_channel();
    let mut config = X11ForwardingConfig::lazy(Arc::new(DeferredProvider { requests }));
    config.fake_cookie = vec![1; 16];
    (config, receive)
}
fn open(channel: Channel<client::Msg>) -> X11ChannelOpen {
    X11ChannelOpen {
        channel,
        originator_address: "127.0.0.1".into(),
        originator_port: 12345,
    }
}
async fn closed(remote: &mut Channel<server::Msg>) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if matches!(remote.wait().await, None | Some(ChannelMsg::Close)) {
                break;
            }
        }
    })
    .await
    .expect("remote channel must close");
}

#[tokio::test]
async fn invalid_session_cookie_never_requests_a_lazy_server() {
    let mut connection = Connection::new().await;
    let (config, mut requests) = lazy_config();
    let (mut remote, channel) = connection.channel().await;
    let worker = tokio::spawn(handle_x11_channel(
        SessionEventQueue::new(),
        "test".into(),
        open(channel),
        config,
    ));
    remote.data(setup(Some(9)).as_slice()).await.unwrap();
    assert!(worker.await.unwrap().is_err());
    assert!(requests.try_recv().is_err());
    closed(&mut remote).await;
    connection.close().await;
}

#[tokio::test]
async fn closing_a_client_during_server_preparation_withdraws_only_its_request() {
    let mut connection = Connection::new().await;
    let (config, mut requests) = lazy_config();
    let (mut remote, channel) = connection.channel().await;
    let worker = tokio::spawn(handle_x11_channel(
        SessionEventQueue::new(),
        "test".into(),
        open(channel),
        config,
    ));
    remote.data(setup(Some(1)).as_slice()).await.unwrap();
    let reply = requests.recv().await.unwrap();
    remote.eof().await.unwrap();
    worker.await.unwrap().unwrap();
    assert!(reply.is_closed());
    closed(&mut remote).await;
    connection.close().await;
}

#[tokio::test]
async fn authenticated_acquisition_has_its_own_sixty_second_timeout() {
    let mut connection = Connection::new().await;
    let (config, mut requests) = lazy_config();
    let (mut remote, channel) = connection.channel().await;
    let worker = tokio::spawn(handle_x11_channel(
        SessionEventQueue::new(),
        "test".into(),
        open(channel),
        config,
    ));
    remote.data(setup(Some(1)).as_slice()).await.unwrap();
    let reply = requests.recv().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    let error = worker.await.unwrap().unwrap_err();
    tokio::time::resume();
    assert_eq!(
        error.downcast_ref::<nyaterm_core::x11::X11Error>(),
        Some(&nyaterm_core::x11::X11Error::Timeout)
    );
    assert!(reply.is_closed());
    closed(&mut remote).await;
    connection.close().await;
}

struct LeaseDrop(Arc<std::sync::atomic::AtomicBool>);
impl Drop for LeaseDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}
#[tokio::test]
async fn delayed_server_preserves_setup_tail_and_abort_releases_channel_and_lease() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut connection = Connection::new().await;
        let (config, mut requests) = lazy_config();
        let (mut remote, channel) = connection.channel().await;
        let worker = tokio::spawn(handle_x11_channel(
            SessionEventQueue::new(),
            "test".into(),
            open(channel),
            config,
        ));
        let mut packet = setup(Some(1));
        packet.extend_from_slice(b"first");
        remote.data(packet.as_slice()).await.unwrap();
        let reply = requests.recv().await.unwrap();
        remote.data(&b"second"[..]).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(
            reply
                .send(Ok(nyaterm_core::x11::X11ChannelConnection {
                    connection: nyaterm_core::x11::X11Connection {
                        endpoint: nyaterm_core::x11::X11Endpoint::Tcp {
                            host: "127.0.0.1".into(),
                            port: listener.local_addr().unwrap().port()
                        },
                        cookie: Some(zeroize::Zeroizing::new(vec![2; 16])),
                        lifetime: None,
                    },
                    guard: Some(Box::new(LeaseDrop(dropped.clone()))),
                }))
                .is_ok()
        );
        let (mut local, _) = listener.accept().await.unwrap();
        let mut expected = setup(Some(2));
        expected.extend_from_slice(b"firstsecond");
        let mut received = vec![0; expected.len()];
        local.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected);
        assert!(!dropped.load(std::sync::atomic::Ordering::Acquire));
        worker.abort();
        let _ = worker.await;
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
        closed(&mut remote).await;
        connection.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_server_preparation_does_not_disable_later_channels_on_the_same_ssh_session() {
    use crate::x11::{X11Forwarder, spawn_x11_forwarder};
    use nyaterm_core::x11::{X11ChannelConnection, X11Connection, X11Endpoint, X11Error};
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut connection = Connection::new().await;
        let (config, mut requests) = lazy_config();
        let (sender, rx) = mpsc::channel(2);
        let forwarder = spawn_x11_forwarder(
            SessionEventQueue::new(),
            "test".into(),
            X11Forwarder { rx, config },
        );
        let (mut first, channel) = connection.channel().await;
        sender.send(open(channel)).await.unwrap();
        first.data(setup(Some(1)).as_slice()).await.unwrap();
        assert!(
            requests
                .recv()
                .await
                .unwrap()
                .send(Err(X11Error::Startup))
                .is_ok()
        );
        closed(&mut first).await;
        let (mut second, channel) = connection.channel().await;
        sender.send(open(channel)).await.unwrap();
        second.data(setup(Some(1)).as_slice()).await.unwrap();
        let reply = requests.recv().await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert!(
            reply
                .send(Ok(X11ChannelConnection {
                    connection: X11Connection {
                        endpoint: X11Endpoint::Tcp {
                            host: "127.0.0.1".into(),
                            port: listener.local_addr().unwrap().port()
                        },
                        cookie: Some(zeroize::Zeroizing::new(vec![8; 16])),
                        lifetime: None,
                    },
                    guard: None,
                }))
                .is_ok()
        );
        let (mut local, _) = listener.accept().await.unwrap();
        let mut received = vec![0; 48];
        local.read_exact(&mut received).await.unwrap();
        assert_eq!(
            received,
            setup(Some(8)),
            "each channel must use the current server cookie"
        );
        drop(forwarder);
        closed(&mut second).await;
        connection.close().await;
    })
    .await
    .unwrap();
}
