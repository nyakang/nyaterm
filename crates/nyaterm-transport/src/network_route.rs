use crate::{SshProxyConfig, SshSessionConfig, connection_attempt::ConnectionAttempt};
use nyaterm_core::connection_route::RelayEndpoint;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};

trait RouteStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RouteStream for T {}
type Stream = Box<dyn RouteStream>;

#[derive(Clone)]
pub struct NetworkRouteConfig {
    pub host: String,
    pub port: u16,
    pub proxy: Option<SshProxyConfig>,
    pub jump: Option<SshSessionConfig>,
}

pub struct NetworkRoute {
    pub endpoint: RelayEndpoint,
    cancel: ConnectionAttempt,
}

impl Drop for NetworkRoute {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl NetworkRoute {
    pub fn start(mut config: NetworkRouteConfig) -> anyhow::Result<Arc<Self>> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = RelayEndpoint {
            address: listener.local_addr()?,
            token: uuid::Uuid::new_v4().simple().to_string().into(),
        };
        let cancel = ConnectionAttempt::default();
        if let Some(jump) = config.jump.as_mut() {
            jump.bind_attempt(cancel.clone());
        }
        let stop = cancel.clone();
        let token = endpoint.token.clone();
        std::thread::Builder::new()
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
                            let mut presented = [0; 32];
                            if !matches!(
                                tokio::time::timeout(
                                    std::time::Duration::from_secs(2),
                                    local.read_exact(&mut presented)
                                )
                                .await,
                                Ok(Ok(_))
                            ) {
                                return;
                            }
                            if presented
                                .iter()
                                .zip(token.expose_secret().as_bytes())
                                .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                                != 0
                            {
                                return;
                            }
                            let operation = async {
                                let (mut remote, owners) = open_stream(config).await?;
                                tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
                                drop(owners);
                                Ok::<(), anyhow::Error>(())
                            };
                            let _ = stop.until_cancelled(operation).await;
                        });
                    }
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                });
            })?;
        Ok(Arc::new(Self { endpoint, cancel }))
    }
}

async fn open_stream(
    config: NetworkRouteConfig,
) -> anyhow::Result<(Stream, Option<crate::SshHandleChain>)> {
    if let Some(jump) = config.jump {
        let (handle, jumps) = crate::open_authenticated_ssh_handle(&jump).await?;
        let channel = handle
            .channel_open_direct_tcpip(config.host, config.port.into(), "127.0.0.1", 0)
            .await?;
        return Ok((Box::new(channel.into_stream()), Some((handle, jumps))));
    }
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
                Box::new(stream.into_inner())
            }
            "http" => {
                let mut stream = tokio::net::TcpStream::connect(address).await?;
                if let (Some(username), Some(password)) =
                    (proxy.username.as_deref(), proxy.password.as_deref())
                {
                    async_http_proxy::http_connect_tokio_with_basic_auth(
                        &mut stream,
                        &config.host,
                        config.port,
                        username,
                        password,
                    )
                    .await?;
                } else {
                    async_http_proxy::http_connect_tokio(&mut stream, &config.host, config.port)
                        .await?;
                }
                Box::new(stream)
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
        Box::new(tokio::net::TcpStream::connect((config.host.as_str(), config.port)).await?),
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::{NetworkRoute, NetworkRouteConfig};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
}
