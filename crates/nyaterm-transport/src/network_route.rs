use crate::{SshProxyConfig, SshSessionConfig, connection_attempt::ConnectionAttempt};
use nyaterm_core::connection_route::RelayEndpoint;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::oneshot;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

trait RouteStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RouteStream for T {}
type Stream = Box<dyn RouteStream>;
type ReadySender = oneshot::Sender<Result<(), RouteConnectError>>;

#[derive(Clone)]
pub struct NetworkRouteConfig {
    pub host: String,
    pub port: u16,
    pub proxy: Option<SshProxyConfig>,
    pub jump: Option<SshSessionConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteConnectError {
    #[error("connection attempt cancelled")]
    Cancelled,
    #[error("network route worker unavailable")]
    WorkerUnavailable,
    #[error("failed to establish network route: {0}")]
    Establish(#[source] anyhow::Error),
}

pub struct NetworkRoute {
    pub endpoint: RelayEndpoint,
    cancel: ConnectionAttempt,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for NetworkRoute {
    fn drop(&mut self) {
        // Remote desktop drops routes on GPUI updates. Cancellation is cheap;
        // joining belongs exclusively to transport cleanup workers.
        self.cancel.cancel();
    }
}

impl NetworkRoute {
    pub fn start(config: NetworkRouteConfig) -> anyhow::Result<Arc<Self>> {
        Self::spawn(config, ConnectionAttempt::default(), None)
    }

    /// Establish one authenticated relay AND its remote stream before handing
    /// the socket to the synchronous Telnet workers. The readiness result stays
    /// in process, so the existing RDP/VNC relay wire contract is unchanged.
    pub(crate) fn connect_with_attempt(
        config: NetworkRouteConfig,
        attempt: ConnectionAttempt,
    ) -> Result<(std::net::TcpStream, Arc<Self>), RouteConnectError> {
        attempt.check().map_err(|_| RouteConnectError::Cancelled)?;
        let (ready, result) = oneshot::channel();
        let route = Self::spawn(config, attempt.clone(), Some(ready))
            .map_err(RouteConnectError::Establish)?;
        let connect = || -> Result<_, RouteConnectError> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| RouteConnectError::Establish(error.into()))?;
            let stream = runtime
                .block_on(attempt.until_cancelled(async {
                    let mut stream = tokio::time::timeout(
                        CONNECT_TIMEOUT,
                        tokio::net::TcpStream::connect(route.endpoint.address),
                    )
                    .await
                    .map_err(|error| RouteConnectError::Establish(error.into()))?
                    .map_err(|error| RouteConnectError::Establish(error.into()))?;
                    stream
                        .write_all(route.endpoint.token.expose_secret().as_bytes())
                        .await
                        .map_err(|error| RouteConnectError::Establish(error.into()))?;
                    result
                        .await
                        .map_err(|_| RouteConnectError::WorkerUnavailable)??;
                    Ok::<_, RouteConnectError>(stream)
                }))
                .map_err(|_| RouteConnectError::Cancelled)??;
            let stream = stream
                .into_std()
                .map_err(|error| RouteConnectError::Establish(error.into()))?;
            stream
                .set_nonblocking(false)
                .map_err(|error| RouteConnectError::Establish(error.into()))?;
            Ok(stream)
        };
        match connect() {
            Ok(stream) => Ok((stream, route)),
            Err(error) => {
                route.close();
                Err(error)
            }
        }
    }

    pub(crate) fn close(&self) {
        self.cancel.cancel();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    fn spawn(
        mut config: NetworkRouteConfig,
        cancel: ConnectionAttempt,
        ready: Option<ReadySender>,
    ) -> anyhow::Result<Arc<Self>> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = RelayEndpoint {
            address: listener.local_addr()?,
            token: uuid::Uuid::new_v4().simple().to_string().into(),
        };
        if let Some(jump) = config.jump.as_mut() {
            jump.bind_attempt(cancel.clone());
        }
        let stop = cancel.clone();
        let token = endpoint.token.clone();
        let worker = std::thread::Builder::new()
            .name("nyaterm-network-route".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    if let Some(ready) = ready {
                        loop {
                            let Ok(Ok((mut local, _))) =
                                stop.until_cancelled(listener.accept()).await
                            else {
                                return;
                            };
                            if !authenticate(&mut local, &token, &stop).await {
                                continue;
                            }
                            // A terminal owns exactly one route. Closing the listener
                            // also prevents reconnects from reusing a stale relay.
                            drop(listener);
                            forward(local, config, stop, Some(ready)).await;
                            return;
                        }
                    }
                    let mut tasks = tokio::task::JoinSet::new();
                    loop {
                        while tasks.try_join_next().is_some() {}
                        let Ok(Ok((mut local, _))) = stop.until_cancelled(listener.accept()).await
                        else {
                            break;
                        };
                        if tasks.len() >= 32 {
                            continue;
                        }
                        let config = config.clone();
                        let token = token.clone();
                        let stop = stop.clone();
                        tasks.spawn(async move {
                            if authenticate(&mut local, &token, &stop).await {
                                forward(local, config, stop, None).await;
                            }
                        });
                    }
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                });
            })?;
        Ok(Arc::new(Self {
            endpoint,
            cancel,
            worker: Mutex::new(Some(worker)),
        }))
    }
}

async fn authenticate(
    local: &mut tokio::net::TcpStream,
    token: &nyaterm_core::SecretString,
    stop: &ConnectionAttempt,
) -> bool {
    let mut presented = [0; 32];
    if !matches!(
        stop.until_cancelled(tokio::time::timeout(
            Duration::from_secs(2),
            local.read_exact(&mut presented)
        ))
        .await,
        Ok(Ok(Ok(_)))
    ) {
        return false;
    }
    presented
        .iter()
        .zip(token.expose_secret().as_bytes())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

async fn forward(
    mut local: tokio::net::TcpStream,
    config: NetworkRouteConfig,
    stop: ConnectionAttempt,
    ready: Option<ReadySender>,
) {
    // Preserve interactive terminal latency across the extra loopback hop.
    let _ = local.set_nodelay(true);
    let opened = stop.until_cancelled(open_stream(config)).await;
    let (mut remote, owners) = match opened {
        Ok(Ok(stream)) => stream,
        error => {
            if let Some(ready) = ready {
                let error = match error {
                    Err(_) => RouteConnectError::Cancelled,
                    Ok(Err(error)) => RouteConnectError::Establish(error),
                    _ => unreachable!(),
                };
                let _ = ready.send(Err(error));
            }
            return;
        }
    };
    if let Some(ready) = ready
        && ready.send(Ok(())).is_err()
    {
        return;
    }
    let _ = stop
        .until_cancelled(tokio::io::copy_bidirectional(&mut local, &mut remote))
        .await;
    drop(remote);
    drop(owners);
}

async fn open_stream(
    config: NetworkRouteConfig,
) -> anyhow::Result<(Stream, Option<crate::SshHandleChain>)> {
    if let Some(jump) = config.jump {
        let (handle, jumps) = crate::open_authenticated_ssh_handle(&jump).await?;
        let channel = tokio::time::timeout(
            CONNECT_TIMEOUT,
            handle.channel_open_direct_tcpip(config.host, config.port.into(), "127.0.0.1", 0),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH direct-tcpip open timed out"))??;
        return Ok((Box::new(channel.into_stream()), Some((handle, jumps))));
    }
    let operation = async {
        if let Some(proxy) = config.proxy {
            let address = format!("{}:{}", proxy.host, proxy.port);
            let stream: Stream = match proxy.protocol.as_str() {
                "socks5" => {
                    let stream = match (proxy.username.as_deref(), proxy.password.as_deref()) {
                        (Some(username), Some(password)) => {
                            tokio_socks::tcp::Socks5Stream::connect_with_password(
                                address.as_str(),
                                (config.host.as_str(), config.port),
                                username,
                                password,
                            )
                            .await?
                        }
                        _ => {
                            tokio_socks::tcp::Socks5Stream::connect(
                                address.as_str(),
                                (config.host.as_str(), config.port),
                            )
                            .await?
                        }
                    };
                    tcp_stream(stream.into_inner())
                }
                "http" => {
                    let mut stream = tokio::net::TcpStream::connect(address).await?;
                    crate::http_proxy::connect(
                        &mut stream,
                        &config.host,
                        config.port,
                        proxy.username.as_deref().zip(proxy.password.as_deref()),
                    )
                    .await?;
                    tcp_stream(stream)
                }
                "proxycommand" => Box::new(
                    crate::open_proxy_command_stream(
                        proxy.command.as_deref(),
                        &config.host,
                        config.port,
                        "",
                    )
                    .await?,
                ),
                _ => anyhow::bail!("unsupported proxy protocol"),
            };
            return Ok((stream, None));
        }
        Ok((
            tcp_stream(tokio::net::TcpStream::connect((config.host.as_str(), config.port)).await?),
            None,
        ))
    };
    tokio::time::timeout(CONNECT_TIMEOUT, operation)
        .await
        .map_err(|_| anyhow::anyhow!("network route connection timed out"))?
}

fn tcp_stream(stream: tokio::net::TcpStream) -> Stream {
    let _ = stream.set_nodelay(true);
    Box::new(stream)
}

#[cfg(test)]
mod tests;
