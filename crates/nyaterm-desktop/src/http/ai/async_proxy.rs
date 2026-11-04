use nyaterm_core::ai::proxy::{AiProxyMode, AiProxyProtocol, AiProxySettings};
use reqwest::{ClientBuilder, NoProxy, Proxy, Url};

pub(super) fn apply_proxy(
    builder: ClientBuilder,
    settings: &AiProxySettings,
) -> Result<ClientBuilder, String> {
    match settings.mode {
        AiProxyMode::System => Ok(builder),
        AiProxyMode::Direct => Ok(builder.no_proxy()),
        AiProxyMode::Custom => {
            let url = proxy_url(settings)?;
            let username = settings
                .username
                .as_deref()
                .filter(|value| !value.is_empty());
            let password = settings
                .password
                .as_deref()
                .filter(|value| !value.is_empty());
            if password.is_some() && username.is_none() {
                return Err("AI proxy password requires a username".to_string());
            }
            if settings.protocol == AiProxyProtocol::Socks5
                && (username.is_some_and(|value| value.len() > 255)
                    || password.is_some_and(|value| value.len() > 255))
            {
                return Err("SOCKS5 proxy credentials must be at most 255 bytes".to_string());
            }
            let mut proxy = Proxy::all(url)
                .map_err(|_| "Invalid AI proxy configuration".to_string())?
                .no_proxy(NoProxy::from_string(&settings.no_proxy));
            if let Some(username) = username {
                proxy = proxy.basic_auth(username, password.unwrap_or_default());
            }
            // Explicitly disable automatic proxies so bypassed requests connect
            // directly, even when environment / system proxies are configured.
            Ok(builder.no_proxy().proxy(proxy))
        }
    }
}

fn proxy_url(settings: &AiProxySettings) -> Result<Url, String> {
    let invalid = || "Invalid AI proxy host or port".to_string();
    let host = settings.host.trim();
    if host.is_empty()
        || settings.port == 0
        || host
            .chars()
            .any(|value| value.is_whitespace() || "/\\@?#%".contains(value))
    {
        return Err(invalid());
    }
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    let host = if unbracketed.contains(':') {
        let address = unbracketed
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| invalid())?;
        format!("[{address}]")
    } else {
        host.to_string()
    };
    let scheme = match settings.protocol {
        AiProxyProtocol::Http => "http",
        AiProxyProtocol::Socks5 => "socks5h",
    };
    // Validate DNS names using a special URL scheme too: SOCKS URLs otherwise
    // allow opaque hosts with characters that cannot be used as a DNS name.
    Url::parse(&format!("http://{host}:{}", settings.port)).map_err(|_| invalid())?;
    Url::parse(&format!("{scheme}://{host}:{}", settings.port)).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::{apply_proxy, proxy_url};
    use nyaterm_core::ai::proxy::{AiProxyMode, AiProxyProtocol, AiProxySettings};
    use reqwest::Client;

    #[test]
    fn socks_uses_remote_dns_and_rejects_invalid_hosts_without_echoing_them() {
        let mut settings = AiProxySettings {
            mode: AiProxyMode::Custom,
            protocol: AiProxyProtocol::Socks5,
            ..Default::default()
        };
        assert_eq!(proxy_url(&settings).unwrap().scheme(), "socks5h");
        settings.host = "[::1]".into();
        assert_eq!(proxy_url(&settings).unwrap().host_str(), Some("[::1]"));
        settings.host = "user:secret@example.com".into();
        assert_eq!(
            proxy_url(&settings).unwrap_err(),
            "Invalid AI proxy host or port"
        );
        settings.host = "127.0.0.1".into();
        settings.password = Some("fixture".into());
        assert!(apply_proxy(Client::builder(), &settings).is_err());
        settings.username = Some("u".repeat(256));
        assert!(apply_proxy(Client::builder(), &settings).is_err());
    }

    #[tokio::test]
    async fn custom_bypass_and_direct_reach_the_origin_without_a_proxy() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;
        for mode in [AiProxyMode::Custom, AiProxyMode::Direct] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let server = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // macOS inherits the nonblocking listener mode on accept.
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let mut request = [0; 2048];
                            assert!(stream.read(&mut request).unwrap() > 0);
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                            return;
                        }
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(error) => panic!("origin did not receive request: {error}"),
                    }
                }
            });
            let settings = AiProxySettings {
                mode,
                port: 1,
                ..Default::default()
            };
            let result = apply_proxy(Client::builder(), &settings)
                .unwrap()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap()
                .get(format!("http://{address}/"))
                .send()
                .await;
            server.join().unwrap();
            assert_eq!(result.unwrap().text().await.unwrap(), "ok");
        }
    }

    #[tokio::test]
    async fn socks_authenticates_and_resolves_the_origin_at_the_proxy() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let settings = AiProxySettings {
                mode: AiProxyMode::Custom,
                protocol: AiProxyProtocol::Socks5,
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                username: Some("proxy-user".into()),
                password: Some("proxy-secret".into()),
                no_proxy: String::new(),
            };
            let client = apply_proxy(Client::builder(), &settings)
                .unwrap()
                .build()
                .unwrap();
            let proxy = async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                assert_eq!(socket.read_u8().await.unwrap(), 5);
                let count = socket.read_u8().await.unwrap();
                let mut methods = vec![0; count as usize];
                socket.read_exact(&mut methods).await.unwrap();
                assert!(methods.contains(&2));
                socket.write_all(&[5, 2]).await.unwrap();
                assert_eq!(socket.read_u8().await.unwrap(), 1);
                let count = socket.read_u8().await.unwrap();
                let mut username = vec![0; count as usize];
                socket.read_exact(&mut username).await.unwrap();
                let count = socket.read_u8().await.unwrap();
                let mut password = vec![0; count as usize];
                socket.read_exact(&mut password).await.unwrap();
                assert_eq!(username, b"proxy-user");
                assert_eq!(password, b"proxy-secret");
                socket.write_all(&[1, 0]).await.unwrap();
                let mut header = [0; 4];
                socket.read_exact(&mut header).await.unwrap();
                assert_eq!(header, [5, 1, 0, 3]);
                let count = socket.read_u8().await.unwrap();
                let mut host = vec![0; count as usize];
                socket.read_exact(&mut host).await.unwrap();
                assert_eq!(host, b"nyaterm-proxy-fixture.invalid");
                assert_eq!(socket.read_u16().await.unwrap(), 80);
                socket
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                    .await
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            };
            let request = async {
                assert_eq!(
                    client
                        .get("http://nyaterm-proxy-fixture.invalid/")
                        .send()
                        .await
                        .unwrap()
                        .text()
                        .await
                        .unwrap(),
                    "ok"
                );
            };
            tokio::join!(proxy, request);
        })
        .await
        .unwrap();
    }
}
