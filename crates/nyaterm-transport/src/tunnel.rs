//! SSH tunnels: local, dynamic (SOCKS5) and remote port forwarding.
//!
//! Split out of `lib.rs` by domain. Bind address handling, the SOCKS5
//! handshake and the per-mode forwarding loops are unchanged; this only moves
//! the code.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use russh::{Disconnect, client};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use super::{
    ForwardedTcpIpChannel, SshClientHandler, SshMultiplexHandle, SshSessionConfig,
    open_authenticated_ssh_handle_with_forwarded_tx,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshTunnelMode {
    Local,
    Remote,
    Dynamic,
}

#[derive(Clone)]
pub struct SshTunnelConfig {
    pub id: String,
    pub ssh_config: SshSessionConfig,
    pub mode: SshTunnelMode,
    pub bind_host: String,
    pub listen_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTunnelInfo {
    pub id: String,
    pub mode: SshTunnelMode,
    pub bind_host: String,
    pub listen_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
}

#[derive(Debug, Default)]
pub struct SshTunnelManager {
    active: Mutex<HashMap<String, SshTunnelHandle>>,
}

#[derive(Debug)]
struct SshTunnelHandle {
    info: SshTunnelInfo,
    shutdown_tx: Option<oneshot::Sender<()>>,
    worker_thread: Option<JoinHandle<()>>,
}

impl SshTunnelManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self, config: SshTunnelConfig) -> anyhow::Result<SshTunnelInfo> {
        self.open_inner(config, None)
    }

    pub fn open_with_multiplex(
        &self,
        config: SshTunnelConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<SshTunnelInfo> {
        multiplex.ensure_matches_config(&config.ssh_config)?;
        self.open_inner(config, Some(multiplex))
    }

    fn open_inner(
        &self,
        config: SshTunnelConfig,
        multiplex: Option<SshMultiplexHandle>,
    ) -> anyhow::Result<SshTunnelInfo> {
        if let Some(info) = self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("SSH tunnel registry lock is poisoned"))?
            .get(&config.id)
            .map(|handle| handle.info.clone())
        {
            return Ok(info);
        }

        validate_tunnel_config(&config)?;
        let bind_host = normalized_bind_host(&config.bind_host);
        let (listener, actual_port) = match config.mode {
            SshTunnelMode::Local | SshTunnelMode::Dynamic => {
                let listener = StdTcpListener::bind((bind_host.as_str(), config.listen_port))
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "failed to bind tunnel listener {}:{}: {error}",
                            bind_host,
                            config.listen_port
                        )
                    })?;
                listener.set_nonblocking(true)?;
                let actual_port = listener.local_addr()?.port();
                (Some(listener), actual_port)
            }
            SshTunnelMode::Remote => (None, config.listen_port),
        };
        let info = SshTunnelInfo {
            id: config.id.clone(),
            mode: config.mode,
            bind_host,
            listen_port: actual_port,
            target_host: config.target_host.clone(),
            target_port: config.target_port,
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_info = info.clone();
        let worker_thread = std::thread::spawn(move || {
            run_tunnel_worker(
                config,
                listener,
                worker_info,
                shutdown_rx,
                ready_tx,
                multiplex,
            );
        });

        match ready_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(info)) => {
                self.active
                    .lock()
                    .map_err(|_| anyhow::anyhow!("SSH tunnel registry lock is poisoned"))?
                    .insert(
                        info.id.clone(),
                        SshTunnelHandle {
                            info: info.clone(),
                            shutdown_tx: Some(shutdown_tx),
                            worker_thread: Some(worker_thread),
                        },
                    );
                Ok(info)
            }
            Ok(Err(error)) => {
                let _ = shutdown_tx.send(());
                let _ = worker_thread.join();
                Err(anyhow::anyhow!(error))
            }
            Err(error) => {
                let _ = shutdown_tx.send(());
                let _ = worker_thread.join();
                Err(anyhow::anyhow!("SSH tunnel startup timed out: {error}"))
            }
        }
    }

    pub fn close(&self, tunnel_id: &str) -> anyhow::Result<()> {
        let Some(mut handle) = self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("SSH tunnel registry lock is poisoned"))?
            .remove(tunnel_id)
        else {
            return Ok(());
        };
        if let Some(shutdown_tx) = handle.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(worker_thread) = handle.worker_thread.take() {
            let _ = worker_thread.join();
        }
        Ok(())
    }

    pub fn is_open(&self, tunnel_id: &str) -> anyhow::Result<bool> {
        Ok(self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("SSH tunnel registry lock is poisoned"))?
            .contains_key(tunnel_id))
    }

    pub fn list(&self) -> anyhow::Result<Vec<SshTunnelInfo>> {
        Ok(self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("SSH tunnel registry lock is poisoned"))?
            .values()
            .map(|handle| handle.info.clone())
            .collect())
    }
}

fn validate_tunnel_config(config: &SshTunnelConfig) -> anyhow::Result<()> {
    if config.id.trim().is_empty() {
        anyhow::bail!("SSH tunnel id is required");
    }
    match config.mode {
        SshTunnelMode::Local | SshTunnelMode::Remote => {
            if config
                .target_host
                .as_deref()
                .is_none_or(|host| host.trim().is_empty())
            {
                anyhow::bail!("{:?} SSH tunnel requires a target host", config.mode);
            }
            if config.target_port.unwrap_or(0) == 0 {
                anyhow::bail!("{:?} SSH tunnel requires a target port", config.mode);
            }
        }
        SshTunnelMode::Dynamic => {}
    }
    Ok(())
}

fn normalized_bind_host(bind_host: &str) -> String {
    let bind_host = bind_host.trim();
    if bind_host.is_empty() {
        "127.0.0.1".to_string()
    } else {
        bind_host.to_string()
    }
}

fn run_tunnel_worker(
    config: SshTunnelConfig,
    listener: Option<StdTcpListener>,
    mut info: SshTunnelInfo,
    shutdown_rx: oneshot::Receiver<()>,
    ready_tx: mpsc::Sender<Result<SshTunnelInfo, String>>,
    multiplex: Option<SshMultiplexHandle>,
) {
    // Each tunnel owns a runtime; bound I/O workers without changing its lifetime.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("nyaterm-ssh-tunnel")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("failed to start SSH tunnel runtime: {error}")));
            return;
        }
    };

    runtime.block_on(async move {
        let (forwarded_tx, forwarded_rx) = if config.mode == SshTunnelMode::Remote {
            let (tx, rx) = tokio_mpsc::unbounded_channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (handle, jump_handles, forwarded_registry, disconnect_on_close) =
            match multiplex.as_ref() {
                Some(multiplex) => (
                    multiplex.target_handle(),
                    Vec::new(),
                    Some(multiplex.forwarded_tcpip_registry()),
                    false,
                ),
                None => {
                    match open_authenticated_ssh_handle_with_forwarded_tx(
                        &config.ssh_config,
                        forwarded_tx.clone(),
                    )
                    .await
                    {
                        Ok((handle, jumps)) => (
                            Arc::new(tokio::sync::Mutex::new(handle)),
                            jumps
                                .into_iter()
                                .map(|jump| Arc::new(tokio::sync::Mutex::new(jump)))
                                .collect(),
                            None,
                            true,
                        ),
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return;
                        }
                    }
                }
            };

        match config.mode {
            SshTunnelMode::Local => {
                let Some(listener) = listener else {
                    let _ =
                        ready_tx.send(Err("local SSH tunnel listener was not created".to_string()));
                    return;
                };
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ =
                            ready_tx.send(Err(format!("failed to adopt tunnel listener: {error}")));
                        return;
                    }
                };
                let target_host = config.target_host.unwrap_or_default();
                let target_port = config.target_port.unwrap_or_default();
                let _ = ready_tx.send(Ok(info));
                run_local_tunnel_loop(
                    listener,
                    handle.clone(),
                    target_host,
                    target_port,
                    shutdown_rx,
                )
                .await;
            }
            SshTunnelMode::Remote => {
                let target_host = config.target_host.unwrap_or_default();
                let target_port = config.target_port.unwrap_or_default();
                let actual_port = match handle
                    .lock()
                    .await
                    .tcpip_forward(&info.bind_host, info.listen_port.into())
                    .await
                {
                    Ok(0) => info.listen_port,
                    Ok(port) => port.try_into().unwrap_or(info.listen_port),
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!(
                            "failed to request remote SSH tunnel {}:{}: {error}",
                            info.bind_host, info.listen_port
                        )));
                        return;
                    }
                };
                info.listen_port = actual_port;
                if let (Some(registry), Some(tx)) = (forwarded_registry.as_ref(), forwarded_tx) {
                    registry
                        .lock()
                        .await
                        .by_listener
                        .insert((info.bind_host.clone(), info.listen_port.into()), tx);
                }
                let _ = ready_tx.send(Ok(info.clone()));
                run_remote_tunnel_loop(
                    handle.clone(),
                    info.bind_host.clone(),
                    info.listen_port,
                    target_host,
                    target_port,
                    forwarded_rx.expect("remote tunnel receiver"),
                    shutdown_rx,
                )
                .await;
                if let Some(registry) = forwarded_registry.as_ref() {
                    registry
                        .lock()
                        .await
                        .by_listener
                        .remove(&(info.bind_host.clone(), info.listen_port.into()));
                }
            }
            SshTunnelMode::Dynamic => {
                let Some(listener) = listener else {
                    let _ = ready_tx.send(Err(
                        "dynamic SSH tunnel listener was not created".to_string()
                    ));
                    return;
                };
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ =
                            ready_tx.send(Err(format!("failed to adopt tunnel listener: {error}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(info));
                run_dynamic_tunnel_loop(listener, handle.clone(), shutdown_rx).await;
            }
        }

        if disconnect_on_close {
            let _ = handle
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
                .await;
            for jump_handle in jump_handles {
                let _ = jump_handle
                    .lock()
                    .await
                    .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
                    .await;
            }
        } else {
            drop(jump_handles);
        }
    });
}

async fn run_local_tunnel_loop(
    listener: tokio::net::TcpListener,
    ssh_handle: Arc<tokio::sync::Mutex<client::Handle<SshClientHandler>>>,
    target_host: String,
    target_port: u16,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            accepted = listener.accept() => {
                let Ok((local_stream, peer_addr)) = accepted else {
                    continue;
                };
                let ssh_handle = ssh_handle.clone();
                let target_host = target_host.clone();
                tokio::spawn(async move {
                    let _ = forward_tcp_stream_over_ssh(
                        local_stream,
                        ssh_handle,
                        target_host,
                        target_port,
                        peer_addr,
                    )
                    .await;
                });
            }
        }
    }
}

async fn run_dynamic_tunnel_loop(
    listener: tokio::net::TcpListener,
    ssh_handle: Arc<tokio::sync::Mutex<client::Handle<SshClientHandler>>>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            accepted = listener.accept() => {
                let Ok((mut local_stream, peer_addr)) = accepted else {
                    continue;
                };
                let ssh_handle = ssh_handle.clone();
                tokio::spawn(async move {
                    let Ok((target_host, target_port)) = read_socks5_connect_request(&mut local_stream).await else {
                        let _ = local_stream.shutdown().await;
                        return;
                    };
                    let _ = forward_tcp_stream_over_ssh(
                        local_stream,
                        ssh_handle,
                        target_host,
                        target_port,
                        peer_addr,
                    )
                    .await;
                });
            }
        }
    }
}

async fn run_remote_tunnel_loop(
    ssh_handle: Arc<tokio::sync::Mutex<client::Handle<SshClientHandler>>>,
    listen_addr: String,
    listen_port: u16,
    target_host: String,
    target_port: u16,
    mut forwarded_rx: tokio_mpsc::UnboundedReceiver<ForwardedTcpIpChannel>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            forwarded = forwarded_rx.recv() => {
                let Some(forwarded) = forwarded else {
                    break;
                };
                let target_host = target_host.clone();
                tokio::spawn(async move {
                    let _ = forward_remote_channel_to_target(
                        forwarded,
                        target_host,
                        target_port,
                    )
                    .await;
                });
            }
        }
    }

    let _ = ssh_handle
        .lock()
        .await
        .cancel_tcpip_forward(&listen_addr, listen_port.into())
        .await;
}

async fn forward_remote_channel_to_target(
    forwarded: ForwardedTcpIpChannel,
    target_host: String,
    target_port: u16,
) -> anyhow::Result<()> {
    let ForwardedTcpIpChannel {
        channel,
        connected_address,
        connected_port,
        originator_address,
        originator_port,
    } = forwarded;
    let _forward_context = (
        connected_address,
        connected_port,
        originator_address,
        originator_port,
    );
    let mut local_stream =
        tokio::net::TcpStream::connect((target_host.as_str(), target_port)).await?;
    let mut channel_stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut local_stream, &mut channel_stream).await?;
    Ok(())
}

async fn forward_tcp_stream_over_ssh(
    mut local_stream: tokio::net::TcpStream,
    ssh_handle: Arc<tokio::sync::Mutex<client::Handle<SshClientHandler>>>,
    target_host: String,
    target_port: u16,
    peer_addr: SocketAddr,
) -> anyhow::Result<()> {
    let channel = {
        let handle = ssh_handle.lock().await;
        handle
            .channel_open_direct_tcpip(
                target_host,
                target_port.into(),
                peer_addr.ip().to_string(),
                peer_addr.port().into(),
            )
            .await?
    };
    let mut channel_stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut local_stream, &mut channel_stream).await?;
    Ok(())
}

async fn read_socks5_connect_request<S>(stream: &mut S) -> anyhow::Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut greeting = [0_u8; 2];
    stream.read_exact(&mut greeting).await?;
    if greeting[0] != 0x05 || greeting[1] == 0 {
        anyhow::bail!("invalid SOCKS5 greeting");
    }
    let mut methods = vec![0_u8; greeting[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0x00) {
        stream.write_all(&[0x05, 0xff]).await?;
        anyhow::bail!("SOCKS5 client did not offer no-auth method");
    }
    stream.write_all(&[0x05, 0x00]).await?;

    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await?;
    if header[0] != 0x05 || header[1] != 0x01 || header[2] != 0x00 {
        write_socks5_reply(stream, 0x07).await?;
        anyhow::bail!("unsupported SOCKS5 request");
    }
    let target_host = match header[3] {
        0x01 => {
            let mut addr = [0_u8; 4];
            stream.read_exact(&mut addr).await?;
            std::net::Ipv4Addr::from(addr).to_string()
        }
        0x03 => {
            let mut len = [0_u8; 1];
            stream.read_exact(&mut len).await?;
            let mut domain = vec![0_u8; len[0] as usize];
            stream.read_exact(&mut domain).await?;
            String::from_utf8(domain)
                .map_err(|_| anyhow::anyhow!("SOCKS5 domain is not valid UTF-8"))?
        }
        0x04 => {
            let mut addr = [0_u8; 16];
            stream.read_exact(&mut addr).await?;
            std::net::Ipv6Addr::from(addr).to_string()
        }
        _ => {
            write_socks5_reply(stream, 0x08).await?;
            anyhow::bail!("unsupported SOCKS5 address type");
        }
    };
    let mut port_bytes = [0_u8; 2];
    stream.read_exact(&mut port_bytes).await?;
    let target_port = u16::from_be_bytes(port_bytes);
    write_socks5_reply(stream, 0x00).await?;
    Ok((target_host, target_port))
}

async fn write_socks5_reply<S>(stream: &mut S, code: u8) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(&[0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        SshTunnelConfig, SshTunnelMode, read_socks5_connect_request, validate_tunnel_config,
    };
    use crate::SshSessionConfig;

    #[test]
    fn tunnel_config_validation_matches_tunnel_modes() {
        let mut config = SshTunnelConfig {
            id: "tunnel-1".to_string(),
            ssh_config: SshSessionConfig::default(),
            mode: SshTunnelMode::Local,
            bind_host: String::new(),
            listen_port: 0,
            target_host: None,
            target_port: None,
        };
        assert!(
            validate_tunnel_config(&config)
                .expect_err("missing target host")
                .to_string()
                .contains("target host")
        );

        config.target_host = Some("127.0.0.1".to_string());
        assert!(
            validate_tunnel_config(&config)
                .expect_err("missing target port")
                .to_string()
                .contains("target port")
        );

        config.target_port = Some(8080);
        validate_tunnel_config(&config).expect("local tunnel");

        config.mode = SshTunnelMode::Dynamic;
        config.target_host = None;
        config.target_port = None;
        validate_tunnel_config(&config).expect("dynamic tunnel");

        config.mode = SshTunnelMode::Remote;
        assert!(
            validate_tunnel_config(&config)
                .expect_err("remote missing target")
                .to_string()
                .contains("target host")
        );
        config.target_host = Some("127.0.0.1".to_string());
        config.target_port = Some(5432);
        validate_tunnel_config(&config).expect("remote tunnel");
    }

    #[test]
    fn socks5_connect_parser_accepts_domain_requests() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let (mut client, mut server) = tokio::io::duplex(128);
            let parser =
                tokio::spawn(async move { read_socks5_connect_request(&mut server).await });

            client
                .write_all(&[0x05, 0x01, 0x00])
                .await
                .expect("greeting");
            let mut method_reply = [0_u8; 2];
            client
                .read_exact(&mut method_reply)
                .await
                .expect("method reply");
            assert_eq!(method_reply, [0x05, 0x00]);

            let domain = b"example.com";
            let mut request = vec![0x05, 0x01, 0x00, 0x03, domain.len() as u8];
            request.extend_from_slice(domain);
            request.extend_from_slice(&443_u16.to_be_bytes());
            client.write_all(&request).await.expect("connect request");
            let mut connect_reply = [0_u8; 10];
            client
                .read_exact(&mut connect_reply)
                .await
                .expect("connect reply");
            assert_eq!(connect_reply, [0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);

            let (host, port) = parser.await.expect("parser task").expect("parsed");
            assert_eq!(host, "example.com");
            assert_eq!(port, 443);
        });
    }
}
