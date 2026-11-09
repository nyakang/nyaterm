use super::{NetworkRoute, NetworkRouteConfig, RouteConnectError, open_stream};
use crate::connection_attempt::ConnectionAttempt;
use crate::{
    SessionManager, SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshProxyConfig,
    SshSessionConfig, TelnetSessionConfig,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{Channel, ChannelOpenFailure, server};
use std::io::{Read as _, Write as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

async fn echo_target() -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4];
        // A relay must consume its token before exposing application bytes.
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"ping");
        stream.write_all(b"pong").await.unwrap();
        assert_eq!(stream.read(&mut bytes).await.unwrap(), 0);
    });
    (port, task)
}

async fn proxy(protocol: &str, target: u16, auth: bool) -> (SshProxyConfig, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let socks = protocol == "socks5";
    let task = tokio::spawn(async move {
        let (mut local, _) = listener.accept().await.unwrap();
        if socks {
            assert_eq!(local.read_u8().await.unwrap(), 5);
            let count = local.read_u8().await.unwrap();
            let mut methods = vec![0; count.into()];
            local.read_exact(&mut methods).await.unwrap();
            let method = if auth { 2 } else { 0 };
            assert!(methods.contains(&method));
            local.write_all(&[5, method]).await.unwrap();
            if auth {
                assert_eq!(local.read_u8().await.unwrap(), 1);
                let len = local.read_u8().await.unwrap();
                let mut username = vec![0; len.into()];
                local.read_exact(&mut username).await.unwrap();
                assert_eq!(username, b"proxy-user");
                let len = local.read_u8().await.unwrap();
                let mut password = vec![0; len.into()];
                local.read_exact(&mut password).await.unwrap();
                assert_eq!(password, b"proxy-password");
                local.write_all(&[1, 0]).await.unwrap();
            }
            let mut head = [0; 4];
            local.read_exact(&mut head).await.unwrap();
            assert_eq!(&head[..3], &[5, 1, 0]);
            match head[3] {
                1 => {
                    let mut ip = [0; 4];
                    local.read_exact(&mut ip).await.unwrap();
                    assert_eq!(ip, [127, 0, 0, 1]);
                }
                3 => {
                    let len = local.read_u8().await.unwrap();
                    let mut host = vec![0; len.into()];
                    local.read_exact(&mut host).await.unwrap();
                    assert_eq!(host, b"device.internal");
                }
                _ => panic!("unexpected address family"),
            }
            assert_eq!(local.read_u16().await.unwrap(), target);
            local
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
                .await
                .unwrap();
        } else {
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(local.read_u8().await.unwrap());
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with(&format!("CONNECT device.internal:{target} HTTP/1.1\r\n")));
            if auth {
                use base64::Engine as _;
                let expected =
                    base64::engine::general_purpose::STANDARD.encode("proxy-user:proxy-password");
                assert!(request.contains(&format!("Proxy-Authorization: Basic {expected}")));
            }
            local
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
        }
        let mut remote = TcpStream::connect(("127.0.0.1", target)).await.unwrap();
        let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
    });
    (
        SshProxyConfig {
            protocol: protocol.into(),
            host: "127.0.0.1".into(),
            port,
            command: None,
            username: auth.then(|| "proxy-user".into()),
            password: auth.then(|| "proxy-password".into()),
        },
        task,
    )
}

async fn exchange(config: NetworkRouteConfig) {
    let address = tokio::task::spawn_blocking(move || {
        let (mut stream, route) =
            NetworkRoute::connect_with_attempt(config, ConnectionAttempt::default()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(b"ping").unwrap();
        let mut reply = [0; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"pong");
        route.close();
        route.endpoint.address
    })
    .await
    .unwrap();
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn socks5_and_http_routes_support_authentication_and_remote_dns() {
    for protocol in ["socks5", "http"] {
        for auth in [false, true] {
            let (port, target) = echo_target().await;
            let (proxy, server) = proxy(protocol, port, auth).await;
            exchange(NetworkRouteConfig {
                host: "device.internal".into(),
                port,
                proxy: Some(proxy),
                jump: None,
            })
            .await;
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), target)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

struct AcceptKey;
impl SshHostKeyVerifier for AcceptKey {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

struct WaitingKey {
    attempt: ConnectionAttempt,
    entered: Arc<tokio::sync::Notify>,
}
impl SshHostKeyVerifier for WaitingKey {
    fn for_attempt(&self, attempt: ConnectionAttempt) -> Option<Arc<dyn SshHostKeyVerifier>> {
        Some(Arc::new(Self {
            attempt,
            entered: self.entered.clone(),
        }))
    }
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        self.entered.notify_one();
        let (_sender, receiver) = std::sync::mpsc::channel::<()>();
        self.attempt.receive(&receiver, Duration::from_secs(300))?;
        Ok(SshHostKeyDecision::Accept)
    }
}

#[tokio::test]
async fn cancelling_jump_host_key_confirmation_uses_the_terminal_attempt() {
    let (mut jump, server, _) = jump(false).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    jump.host_key_verifier = Some(Arc::new(WaitingKey {
        attempt: ConnectionAttempt::default(),
        entered: entered.clone(),
    }));
    let attempt = ConnectionAttempt::default();
    let stop = attempt.clone();
    let connecting = tokio::task::spawn_blocking(move || {
        NetworkRoute::connect_with_attempt(
            NetworkRouteConfig {
                host: "127.0.0.1".into(),
                port: 23,
                jump: Some(jump),
                proxy: None,
            },
            attempt,
        )
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    stop.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), connecting)
            .await
            .unwrap()
            .unwrap(),
        Err(RouteConnectError::Cancelled)
    ));
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

struct JumpPeer {
    reject: bool,
    forwards: Arc<Mutex<Vec<u32>>>,
}
impl server::Handler for JumpPeer {
    type Error = russh::Error;
    async fn auth_password(
        &mut self,
        _: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if password == "jump-password" {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.forwards.lock().unwrap().push(port);
        if self.reject {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let mut target = TcpStream::connect((host, port as u16)).await.unwrap();
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut target).await;
        });
        Ok(())
    }
}

async fn jump(reject: bool) -> (SshSessionConfig, JoinHandle<()>, Arc<Mutex<Vec<u32>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let forwards = Arc::new(Mutex::new(Vec::new()));
    let seen = forwards.clone();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![
                russh::keys::PrivateKey::new(
                    KeypairData::Ed25519(Ed25519Keypair::from_seed(&[29; 32])),
                    "route-test",
                )
                .unwrap(),
            ],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        if let Ok(session) = server::run_stream(
            config,
            stream,
            JumpPeer {
                reject,
                forwards: seen,
            },
        )
        .await
        {
            let _ = session.await;
        }
    });
    (
        SshSessionConfig {
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            password: Some("jump-password".into()),
            host_key_verifier: Some(Arc::new(AcceptKey)),
            ..Default::default()
        },
        task,
        forwards,
    )
}

#[tokio::test]
async fn multi_level_jumps_and_proxied_first_jump_release_every_connection() {
    for depth in [1, 2] {
        for proxied in [false, true] {
            let (port, target) = echo_target().await;
            let (mut first, first_server, first_forwards) = jump(false).await;
            let mut servers = vec![first_server];
            if proxied {
                let (proxy, server) = proxy("socks5", first.port, true).await;
                first.proxy = Some(proxy);
                servers.push(server);
            }
            let route_jump = if depth == 2 {
                let (mut second, server, _) = jump(false).await;
                second.proxy_jump = Some(Box::new(first));
                servers.push(server);
                second
            } else {
                first
            };
            // This unreachable proxy must not participate when a jump exists.
            let ignored_proxy = SshProxyConfig {
                protocol: "invalid".into(),
                host: String::new(),
                port: 0,
                command: None,
                username: None,
                password: None,
            };
            exchange(NetworkRouteConfig {
                host: "127.0.0.1".into(),
                port,
                proxy: Some(ignored_proxy),
                jump: Some(route_jump),
            })
            .await;
            assert_eq!(first_forwards.lock().unwrap().len(), 1);
            for server in servers {
                tokio::time::timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap();
            }
            tokio::time::timeout(Duration::from_secs(5), target)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

#[tokio::test]
async fn rejected_ssh_authentication_and_forwarding_fail_before_session_registration() {
    for bad_password in [false, true] {
        let (mut jump, server, _) = jump(!bad_password).await;
        if bad_password {
            jump.password = Some("wrong".into());
        }
        let manager = Arc::new(SessionManager::new());
        let worker_manager = manager.clone();
        let error = tokio::task::spawn_blocking(move || {
            worker_manager.create_telnet_session(TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: 23,
                proxy_jump: Some(Box::new(jump)),
                ..Default::default()
            })
        })
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("network route"));
        assert!(!error.to_string().contains("wrong"));
        assert!(manager.list_sessions().unwrap().is_empty());
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn proxy_refusal_is_reported_before_a_telnet_session_exists() {
    for protocol in ["socks5", "http"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let socks = protocol == "socks5";
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            assert!(stream.read(&mut buf).await.unwrap() > 0);
            stream
                .write_all(if socks {
                    &[5, 255][..]
                } else {
                    b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"
                })
                .await
                .unwrap();
        });
        let manager = Arc::new(SessionManager::new());
        let worker_manager = manager.clone();
        let proxy = SshProxyConfig {
            protocol: protocol.into(),
            host: "127.0.0.1".into(),
            port,
            command: None,
            username: Some("proxy-user".into()),
            password: Some("proxy-password".into()),
        };
        let error = tokio::task::spawn_blocking(move || {
            worker_manager.create_telnet_session(TelnetSessionConfig {
                host: "device.internal".into(),
                port: 23,
                proxy: Some(proxy),
                ..Default::default()
            })
        })
        .await
        .unwrap()
        .unwrap_err();
        assert!(!error.to_string().contains("proxy-password"));
        assert!(manager.list_sessions().unwrap().is_empty());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_interrupts_pending_proxy_handshake_and_closes_the_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (accepted, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0; 1024];
        assert!(stream.read(&mut buf).await.unwrap() > 0);
        accepted.send(()).unwrap();
        assert_eq!(stream.read(&mut buf).await.unwrap(), 0);
    });
    let attempt = ConnectionAttempt::default();
    let stop = attempt.clone();
    let connecting = tokio::task::spawn_blocking(move || {
        NetworkRoute::connect_with_attempt(
            NetworkRouteConfig {
                host: "device.internal".into(),
                port: 23,
                proxy: Some(SshProxyConfig {
                    protocol: "socks5".into(),
                    host: "127.0.0.1".into(),
                    port,
                    command: None,
                    username: None,
                    password: None,
                }),
                jump: None,
            },
            attempt,
        )
    });
    ready.await.unwrap();
    stop.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), connecting)
            .await
            .unwrap()
            .unwrap(),
        Err(RouteConnectError::Cancelled)
    ));
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn stalled_proxy_handshake_has_a_network_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let pending = tokio::spawn(open_stream(NetworkRouteConfig {
        host: "device.internal".into(),
        port: 23,
        proxy: Some(SshProxyConfig {
            protocol: "socks5".into(),
            host: "127.0.0.1".into(),
            port,
            command: None,
            username: None,
            password: None,
        }),
        jump: None,
    }));
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut buf = [0; 1024];
    assert!(stream.read(&mut buf).await.unwrap() > 0);
    tokio::time::advance(Duration::from_secs(31)).await;
    let error = pending.await.unwrap().err().unwrap();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(stream.read(&mut buf).await.unwrap(), 0);
}

#[tokio::test]
async fn routed_telnet_keeps_negotiation_naws_login_and_escaped_input_intact() {
    use crate::telnet_codec::{
        DO, IAC, OPT_NAWS, OPT_SUPPRESS_GO_AHEAD, WILL, naws_subnegotiation,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (proxy, proxy_server) = proxy("http", port, true).await;
    let config = TelnetSessionConfig {
        host: "device.internal".into(),
        port,
        username: "operator".into(),
        proxy: Some(proxy),
        force_character_at_a_time: true,
        keep_alive_interval_secs: 0,
        auto_login: crate::TelnetAutoLoginConfig {
            send_wake_enter: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let (negotiated_tx, negotiated_rx) = tokio::sync::oneshot::channel();
    let target = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut offer = [0; 3];
        stream.read_exact(&mut offer).await.unwrap();
        assert_eq!(offer, [IAC, WILL, OPT_NAWS]);
        stream.write_all(&[IAC, DO, OPT_NAWS]).await.unwrap();
        let mut negotiated = [0; 9];
        stream.read_exact(&mut negotiated).await.unwrap();
        assert_eq!(negotiated, naws_subnegotiation(80, 24)[..]);
        negotiated_tx.send(()).unwrap();
        for byte in [IAC, WILL, OPT_SUPPRESS_GO_AHEAD] {
            stream.write_all(&[byte]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stream.write_all(b"Username: ").await.unwrap();
        let mut packets = vec![
            naws_subnegotiation(100, 40),
            vec![IAC, DO, OPT_SUPPRESS_GO_AHEAD],
            b"operator\r".to_vec(),
            b"a\xff\xffb".to_vec(),
        ];
        let mut bytes = vec![0; packets.iter().map(Vec::len).sum()];
        stream.read_exact(&mut bytes).await.unwrap();
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            let index = packets
                .iter()
                .position(|packet| remaining.starts_with(packet))
                .unwrap();
            remaining = &remaining[packets.remove(index).len()..];
        }
        assert!(packets.is_empty());
    });
    let manager = Arc::new(SessionManager::new());
    let worker_manager = manager.clone();
    let info = tokio::task::spawn_blocking(move || worker_manager.create_telnet_session(config))
        .await
        .unwrap()
        .unwrap();
    negotiated_rx.await.unwrap();
    manager.write_raw(&info.id, b"a\xffb").unwrap();
    manager.resize(&info.id, 100, 40).unwrap();
    tokio::time::timeout(Duration::from_secs(5), target)
        .await
        .unwrap()
        .unwrap();
    tokio::task::spawn_blocking(move || manager.close(&info.id))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), proxy_server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn routed_telnet_sends_idle_keepalive_before_the_target_disconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (proxy, server) = proxy("http", port, false).await;
    let target = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut nop = [0; 2];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut nop))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(nop, crate::telnet_codec::NOP);
    });
    let manager = Arc::new(SessionManager::new());
    let worker_manager = manager.clone();
    let info = tokio::task::spawn_blocking(move || {
        worker_manager.create_telnet_session(TelnetSessionConfig {
            host: "device.internal".into(),
            port,
            proxy: Some(proxy),
            send_naws: false,
            keep_alive_interval_secs: 1,
            auto_login: crate::TelnetAutoLoginConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        })
    })
    .await
    .unwrap()
    .unwrap();
    target.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(crate::SessionEvent::Exited { session_id, .. }) =
                manager.try_recv_event().unwrap()
            {
                assert_eq!(session_id, info.id);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::task::spawn_blocking(move || manager.close(&info.id))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn raw_tcp_routes_can_be_closed_and_recreated_without_stale_resources() {
    for _ in 0..3 {
        let (port, target) = echo_target().await;
        let (proxy, server) = proxy("socks5", port, false).await;
        let manager = Arc::new(SessionManager::new());
        let worker_manager = manager.clone();
        let info = tokio::task::spawn_blocking(move || {
            worker_manager.create_telnet_session(TelnetSessionConfig {
                host: "device.internal".into(),
                port,
                proxy: Some(proxy),
                raw_tcp: true,
                ..Default::default()
            })
        })
        .await
        .unwrap()
        .unwrap();
        manager.write_raw(&info.id, b"ping").unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(crate::SessionEvent::Output { data, .. }) =
                    manager.try_recv_event().unwrap()
                {
                    break data;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(output, b"pong");
        tokio::task::spawn_blocking(move || manager.close(&info.id))
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), target)
            .await
            .unwrap()
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_a_proxy_command_route_reaps_its_process() {
    tokio::task::spawn_blocking(|| {
        let attempt = ConnectionAttempt::default();
        let (mut stream, route) = NetworkRoute::connect_with_attempt(
            NetworkRouteConfig {
                host: "unused".into(),
                port: 23,
                jump: None,
                proxy: Some(SshProxyConfig {
                    protocol: "proxycommand".into(),
                    host: String::new(),
                    port: 0,
                    command: Some("printf '%s\\n' \"$$\"; exec cat".into()),
                    username: None,
                    password: None,
                }),
            },
            attempt.clone(),
        )
        .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut line = Vec::new();
        loop {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
        }
        let pid: libc::pid_t = String::from_utf8(line).unwrap().parse().unwrap();
        attempt.cancel();
        route.close();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(std::net::TcpStream::connect(route.endpoint.address).is_err());
    })
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_proxy_command_exit_delivers_final_output_and_a_session_exit_without_stderr() {
    let manager = Arc::new(SessionManager::new());
    let worker_manager = manager.clone();
    let info = tokio::task::spawn_blocking(move || {
        worker_manager.create_telnet_session(TelnetSessionConfig {
            host: "unused".into(),
            raw_tcp: true,
            proxy: Some(SshProxyConfig {
                protocol: "proxycommand".into(),
                host: String::new(),
                port: 0,
                command: Some("printf public-output; printf private-stderr >&2; exit 1".into()),
                username: None,
                password: None,
            }),
            ..Default::default()
        })
    })
    .await
    .unwrap()
    .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(5), async {
        let mut output = Vec::new();
        loop {
            match manager.try_recv_event().unwrap() {
                Some(crate::SessionEvent::Output { data, .. }) => output.extend(data),
                Some(crate::SessionEvent::Exited { reason, .. }) => {
                    assert!(!reason.contains("private-stderr"));
                    break output;
                }
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(output, b"public-output");
    tokio::task::spawn_blocking(move || manager.close(&info.id))
        .await
        .unwrap()
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn proxy_command_routes_forward_bytes_and_close_the_owned_process() {
    for _ in 0..2 {
        let config = NetworkRouteConfig {
            host: "unused".into(),
            port: 23,
            jump: None,
            proxy: Some(SshProxyConfig {
                protocol: "proxycommand".into(),
                host: String::new(),
                port: 0,
                command: Some("exec cat".into()),
                username: None,
                password: None,
            }),
        };
        tokio::task::spawn_blocking(move || {
            let (mut stream, route) =
                NetworkRoute::connect_with_attempt(config, ConnectionAttempt::default()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream.write_all(b"ping").unwrap();
            let mut bytes = [0; 4];
            stream.read_exact(&mut bytes).unwrap();
            assert_eq!(&bytes, b"ping");
            route.close();
            assert!(std::net::TcpStream::connect(route.endpoint.address).is_err());
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn authenticated_route_forwards_bytes_and_drop_releases_listener() {
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_port = target.local_addr().unwrap().port();
    let echo = tokio::spawn(async move {
        let (mut stream, _) = target.accept().await.unwrap();
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).await.unwrap();
        stream.write_all(&bytes).await.unwrap();
    });
    let route = NetworkRoute::start(NetworkRouteConfig {
        host: "127.0.0.1".into(),
        port: target_port,
        proxy: None,
        jump: None,
    })
    .unwrap();
    let address = route.endpoint.address;
    let mut invalid = tokio::net::TcpStream::connect(address).await.unwrap();
    invalid.write_all(&[0; 32]).await.unwrap();
    let mut byte = [0; 1];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), invalid.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client
        .write_all(route.endpoint.token.expose_secret().as_bytes())
        .await
        .unwrap();
    client.write_all(b"ping").await.unwrap();
    let mut result = [0; 4];
    tokio::time::timeout(Duration::from_secs(3), client.read_exact(&mut result))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&result, b"ping");
    echo.await.unwrap();
    drop(route);
    tokio::time::timeout(Duration::from_secs(3), async {
        while tokio::net::TcpStream::connect(address).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}
