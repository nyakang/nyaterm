//! Authenticated local CONNECT adapter. TLS remains end-to-end in spice-gtk.
//!
//! Its open-fd path skips native TLS setup, so supplying raw relay sockets there
//! would violate certificate policy. This proxy keeps the original host/ports
//! and forwards opaque bytes through an explicitly selected application route.

use std::io;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nyaterm_core::connection_route::RelayEndpoint;
use nyaterm_core::secret::SecretString;
use nyaterm_remote_desktop::spice::protocol::SpiceSessionConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use zeroize::Zeroizing;

const HEADER_LIMIT: usize = 16 * 1024;
const CONNECTION_LIMIT: usize = 64;

struct Routes {
    host: SecretString,
    tcp: Option<(u16, RelayEndpoint)>,
    tls: Option<(u16, RelayEndpoint)>,
    auth: SecretString,
    upstream_proxy: bool,
    upstream_auth: Option<SecretString>,
}

pub(crate) struct RelayProxy {
    pub url: SecretString,
    stop: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl RelayProxy {
    pub fn start(config: &SpiceSessionConfig) -> io::Result<Self> {
        config.validate().map_err(|_| invalid())?;
        // Every enabled destination must have an explicit route. Never fall
        // back to a direct connection when only one port was provisioned.
        let tcp = if config.settings.security.require_tls {
            None
        } else {
            route(config.endpoint.port, &config.tcp_relay)?
        };
        let tls = route(config.endpoint.tls_port, &config.tls_relay)?;
        if tcp.is_none() && tls.is_none() {
            return Err(invalid());
        }
        if let (Some((tcp_port, tcp_route)), Some((tls_port, tls_route))) = (&tcp, &tls)
            && tcp_port == tls_port
            && tcp_route != tls_route
        {
            return Err(invalid());
        }
        let upstream_auth = if let Some(proxy) = &config.endpoint.proxy {
            let url = url::Url::parse(proxy.expose_secret()).map_err(|_| invalid())?;
            if !url.username().is_empty() || url.password().is_some() {
                let username = Zeroizing::new(
                    percent_encoding::percent_decode_str(url.username())
                        .decode_utf8()
                        .map_err(|_| invalid())?
                        .into_owned(),
                );
                let password = Zeroizing::new(
                    percent_encoding::percent_decode_str(url.password().unwrap_or(""))
                        .decode_utf8()
                        .map_err(|_| invalid())?
                        .into_owned(),
                );
                Some(basic_auth(&username, &password))
            } else {
                None
            }
        } else {
            None
        };
        let token = SecretString::from(uuid::Uuid::new_v4().simple().to_string());
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let url = format!("http://nyaterm:{}@127.0.0.1:{port}", token.expose_secret()).into();
        let routes = Arc::new(Routes {
            host: config.endpoint.host.clone(),
            tcp,
            tls,
            auth: basic_auth("nyaterm", token.expose_secret()),
            upstream_proxy: config.endpoint.proxy.is_some(),
            upstream_auth,
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (stop, mut stopped) = watch::channel(false);
        let worker = thread::Builder::new()
            .name("spice-route".into())
            .spawn(move || {
                runtime.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return };
                let mut tasks = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = stopped.changed() => break,
                        Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                        incoming = listener.accept() => {
                            let Ok((stream, _)) = incoming else { break };
                            if tasks.len() >= CONNECTION_LIMIT { drop(stream); continue; }
                            let routes = routes.clone();
                            tasks.spawn(async move { let _ = connection(stream, &routes).await; });
                        }
                    }
                }
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            });
            })?;
        Ok(Self {
            url,
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for RelayProxy {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(worker) = self.worker.take() {
            for _ in 0..20 {
                if worker.is_finished() {
                    let _ = worker.join();
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn route(
    port: Option<u16>,
    endpoint: &Option<RelayEndpoint>,
) -> io::Result<Option<(u16, RelayEndpoint)>> {
    port.map(|port| endpoint.clone().map(|e| (port, e)).ok_or_else(invalid))
        .transpose()
}

fn basic_auth(username: &str, password: &str) -> SecretString {
    let plain = Zeroizing::new(format!("{username}:{password}"));
    format!("Basic {}", STANDARD.encode(plain.as_bytes())).into()
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid SPICE relay request")
}

fn select_route<'a>(
    header: &[u8],
    routes: &'a Routes,
) -> io::Result<(&'a RelayEndpoint, SecretString)> {
    let header = std::str::from_utf8(header).map_err(|_| invalid())?;
    let mut lines = header.split("\r\n");
    let parts: Vec<_> = lines
        .next()
        .ok_or_else(invalid)?
        .split_ascii_whitespace()
        .collect();
    if parts.len() != 3 || parts[0] != "CONNECT" || !matches!(parts[2], "HTTP/1.0" | "HTTP/1.1") {
        return Err(invalid());
    }
    let (host, port) = parts[1].rsplit_once(':').ok_or_else(invalid)?;
    let port = port.parse::<u16>().map_err(|_| invalid())?;
    let original_host = routes.host.expose_secret();
    let host_matches = host == original_host
        || (original_host.parse::<std::net::Ipv6Addr>().is_ok()
            && host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) == Some(original_host));
    if !host_matches {
        return Err(invalid());
    }
    let mut auth = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        if name.eq_ignore_ascii_case("Proxy-Authorization") && auth.replace(value.trim()).is_some()
        {
            return Err(invalid());
        }
    }
    let actual = auth.ok_or_else(invalid)?.as_bytes();
    let expected = routes.auth.expose_secret().as_bytes();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            != 0
    {
        return Err(invalid());
    }
    let endpoint = [&routes.tls, &routes.tcp]
        .into_iter()
        .flatten()
        .find_map(|(p, e)| (*p == port).then_some(e))
        .ok_or_else(invalid)?;
    Ok((endpoint, parts[1].into()))
}

async fn header(stream: &mut TcpStream) -> io::Result<(Zeroizing<Vec<u8>>, Vec<u8>)> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
    loop {
        let mut chunk = [0u8; 2048];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            if end + 4 > HEADER_LIMIT {
                return Err(invalid());
            }
            let remaining = bytes.split_off(end + 4);
            return Ok((bytes, remaining));
        }
        if bytes.len() >= HEADER_LIMIT {
            return Err(invalid());
        }
    }
}

async fn connection(mut client: TcpStream, routes: &Routes) -> io::Result<()> {
    client.set_nodelay(true)?;
    let established = tokio::time::timeout(Duration::from_secs(10), async {
        let (request, prefix) = header(&mut client).await?;
        let (endpoint, authority) = select_route(&request, routes)?;
        let mut upstream = TcpStream::connect(endpoint.address).await?;
        upstream.set_nodelay(true)?;
        upstream
            .write_all(endpoint.token.expose_secret().as_bytes())
            .await?;
        if routes.upstream_proxy {
            let mut connect = Zeroizing::new(format!(
                "CONNECT {} HTTP/1.1\r\nHost: {}\r\n",
                authority.expose_secret(),
                authority.expose_secret()
            ));
            if let Some(auth) = &routes.upstream_auth {
                connect.push_str("Proxy-Authorization: ");
                connect.push_str(auth.expose_secret());
                connect.push_str("\r\n");
            }
            connect.push_str("\r\n");
            upstream.write_all(connect.as_bytes()).await?;
            let (response, upstream_prefix) = header(&mut upstream).await?;
            let first_line = response
                .split(|byte| *byte == b'\n')
                .next()
                .ok_or_else(invalid)?;
            let status = std::str::from_utf8(first_line)
                .map_err(|_| invalid())?
                .split_ascii_whitespace()
                .nth(1);
            if status != Some("200") {
                return Err(invalid());
            }
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await?;
            client.write_all(&upstream_prefix).await?;
        } else {
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await?;
        }
        upstream.write_all(&prefix).await?;
        Ok::<_, io::Error>(upstream)
    })
    .await;
    let mut upstream = match established {
        Ok(Ok(upstream)) => upstream,
        _ => {
            let _ = tokio::time::timeout(
                Duration::from_millis(100),
                client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n"),
            )
            .await;
            return Err(invalid());
        }
    };
    tokio::io::copy_bidirectional_with_sizes(&mut client, &mut upstream, 32 * 1024, 32 * 1024)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::relay::{Routes, basic_auth, select_route};
    use nyaterm_core::connection_route::RelayEndpoint;

    #[test]
    fn relay_authentication_and_destination_cannot_be_bypassed() {
        let route = RelayEndpoint {
            address: "127.0.0.1:1234".parse().unwrap(),
            token: "a".repeat(32).into(),
        };
        let routes = Routes {
            host: "vm.example".into(),
            tcp: None,
            tls: Some((5901, route.clone())),
            auth: basic_auth("nyaterm", "secret"),
            upstream_proxy: false,
            upstream_auth: None,
        };
        let request = format!(
            "CONNECT vm.example:5901 HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
            routes.auth.expose_secret()
        );
        assert_eq!(select_route(request.as_bytes(), &routes).unwrap().0, &route);
        for invalid in [
            request.replace("vm.example", "other.example"),
            request.replace(":5901", ":5900"),
            request.replace(routes.auth.expose_secret(), "Basic Zm9v"),
            request.replace("CONNECT", "GET"),
        ] {
            assert!(select_route(invalid.as_bytes(), &routes).is_err());
        }
    }
}
