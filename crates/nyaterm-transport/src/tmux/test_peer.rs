//! Loopback SSH transport shared by protocol and real-tmux regressions.
use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server::{Auth, ChannelOpenHandle, Session};
use russh::{Channel, ChannelReadHalf, ChannelWriteHalf, client, server};

struct Client;
impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

struct Peer(Option<tokio::sync::oneshot::Sender<Channel<server::Msg>>>);
impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        let _ = self.0.take().unwrap().send(channel);
        Ok(())
    }
}

pub(super) struct Connection {
    pub(super) tx: Arc<ChannelWriteHalf<client::Msg>>,
    pub(super) rx: ChannelReadHalf,
    pub(super) peer_tx: Arc<ChannelWriteHalf<server::Msg>>,
    pub(super) peer_rx: Option<ChannelReadHalf>,
    _client: client::Handle<Client>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Connection {
    pub(super) async fn disconnect(&self) {
        self._client
            .disconnect(russh::Disconnect::ByApplication, "test", "")
            .await
            .unwrap();
    }
}

pub(super) async fn connect() -> Connection {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[19; 32])),
            "tmux-test",
        )
        .unwrap();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let _ = server::run_stream(config, socket, Peer(Some(tx)))
            .await
            .unwrap()
            .await;
    });
    let mut client = client::connect(Arc::new(client::Config::default()), address, Client)
        .await
        .unwrap();
    assert!(client.authenticate_none("test").await.unwrap().success());
    let channel = client.channel_open_session().await.unwrap();
    let peer = rx.await.unwrap();
    let (rx, tx) = channel.split();
    let (peer_rx, peer_tx) = peer.split();
    Connection {
        tx: Arc::new(tx),
        rx,
        peer_tx: Arc::new(peer_tx),
        peer_rx: Some(peer_rx),
        _client: client,
        server,
    }
}
