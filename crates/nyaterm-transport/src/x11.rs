//! X11 forwarding: display resolution, xauth cookie rewriting and channel relay.
//!
//! Desktop supplies a prepared local endpoint and its lifetime. Legacy callers
//! can still resolve external displays here; every channel validates its session cookie.

#[cfg(unix)]
use std::path::PathBuf;

use russh::{ChannelMsg, client};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc as tokio_mpsc;

use std::time::Duration;
use zeroize::{Zeroize, Zeroizing};

use super::{SessionEvent, SessionEventQueue};

pub(crate) const MIT_MAGIC_COOKIE: &str = "MIT-MAGIC-COOKIE-1";

pub(crate) const XAUTH_TIMEOUT: Duration = Duration::from_secs(2);
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SETUP_BYTES: usize = 4096;
const MAX_X11_CHANNELS: usize = 64;

pub(super) struct X11ChannelOpen {
    pub(super) channel: russh::Channel<client::Msg>,
    pub(super) originator_address: String,
    pub(super) originator_port: u32,
}

pub(super) struct X11Forwarder {
    pub(super) rx: tokio_mpsc::Receiver<X11ChannelOpen>,
    pub(super) config: X11ForwardingConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11DisplayTarget {
    Tcp {
        host: String,
        port: u16,
    },
    #[cfg(unix)]
    UnixSocket {
        path: PathBuf,
    },
}

impl X11DisplayTarget {
    pub fn describe(&self) -> String {
        match self {
            Self::Tcp { host, port } => format!("{host}:{port}"),
            #[cfg(unix)]
            Self::UnixSocket { path } => path.display().to_string(),
        }
    }
}

#[derive(Clone)]
pub struct X11ForwardingConfig {
    pub target: X11DisplayTarget,
    pub fallback_target: Option<X11DisplayTarget>,
    pub fake_cookie: Vec<u8>,
    pub fake_cookie_hex: String,
    pub real_cookie: Option<Vec<u8>>,
    pub lifetime: Option<std::sync::Arc<dyn nyaterm_core::x11::X11Lifetime>>,
}

impl std::fmt::Debug for X11ForwardingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11ForwardingConfig")
            .finish_non_exhaustive()
    }
}

impl Drop for X11ForwardingConfig {
    fn drop(&mut self) {
        self.fake_cookie.zeroize();
        self.fake_cookie_hex.zeroize();
        self.real_cookie.zeroize();
    }
}

pub fn effective_x11_display(configured: &str) -> String {
    let trimmed = configured.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    if cfg!(windows) {
        "localhost:0".to_string()
    } else {
        std::env::var("DISPLAY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| ":0".to_string())
    }
}

pub async fn prepare_x11_forwarding(configured_display: &str) -> X11ForwardingConfig {
    let display = effective_x11_display(configured_display);
    let (target, fallback_target) = resolve_x11_display_targets(&display);
    let fake_cookie = uuid::Uuid::new_v4().as_bytes().to_vec();
    let fake_cookie_hex = encode_hex(&fake_cookie);
    let real_cookie = read_local_x11_auth_cookie(&display).await;

    X11ForwardingConfig {
        target,
        fallback_target,
        fake_cookie,
        fake_cookie_hex,
        real_cookie,
        lifetime: None,
    }
}

impl X11ForwardingConfig {
    pub(crate) fn prepared(connection: &nyaterm_core::x11::X11Connection) -> anyhow::Result<Self> {
        use nyaterm_core::x11::X11Endpoint;
        let target = match &connection.endpoint {
            X11Endpoint::Tcp { host, port } => X11DisplayTarget::Tcp {
                host: host.clone(),
                port: *port,
            },
            #[cfg(unix)]
            X11Endpoint::UnixSocket { path } => X11DisplayTarget::UnixSocket { path: path.clone() },
            #[cfg(not(unix))]
            X11Endpoint::UnixSocket { .. } => anyhow::bail!("Unsupported local X11 endpoint"),
        };
        let fake_cookie = uuid::Uuid::new_v4().as_bytes().to_vec();
        Ok(Self {
            target,
            fallback_target: None,
            fake_cookie_hex: encode_hex(&fake_cookie),
            fake_cookie,
            real_cookie: connection.cookie.as_ref().map(|cookie| cookie.to_vec()),
            lifetime: connection.lifetime.clone(),
        })
    }
}

pub fn resolve_x11_display_targets(display: &str) -> (X11DisplayTarget, Option<X11DisplayTarget>) {
    let target = resolve_x11_display_spec(Some(display));

    #[cfg(unix)]
    {
        let fallback = match &target {
            X11DisplayTarget::UnixSocket { .. } => {
                display_number(display).map(|n| X11DisplayTarget::Tcp {
                    host: "localhost".to_string(),
                    port: 6000 + n,
                })
            }
            _ => None,
        };
        (target, fallback)
    }

    #[cfg(not(unix))]
    {
        (target, None)
    }
}

pub fn resolve_x11_display_spec(display: Option<&str>) -> X11DisplayTarget {
    let value = display
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(if cfg!(windows) { "localhost:0" } else { ":0" });

    #[cfg(unix)]
    if value.starts_with('/') {
        return X11DisplayTarget::UnixSocket {
            path: PathBuf::from(value),
        };
    }

    if let Some(rest) = value.strip_prefix("unix:") {
        let display = parse_display_number(rest).unwrap_or(0);
        return platform_display_target(None, display);
    }

    if let Some(rest) = value.strip_prefix(':') {
        let display = parse_display_number(rest).unwrap_or(0);
        return platform_display_target(None, display);
    }

    if let Some((host, suffix)) = value.rsplit_once(':') {
        let n = parse_display_number(suffix).unwrap_or(0);
        let port = if n >= 100 { n } else { 6000 + n };
        return X11DisplayTarget::Tcp {
            host: host.to_string(),
            port,
        };
    }

    X11DisplayTarget::Tcp {
        host: "localhost".to_string(),
        port: 6000,
    }
}

fn platform_display_target(host: Option<&str>, display: u16) -> X11DisplayTarget {
    #[cfg(unix)]
    {
        if host.is_none() {
            return X11DisplayTarget::UnixSocket {
                path: PathBuf::from(format!("/tmp/.X11-unix/X{display}")),
            };
        }
    }

    X11DisplayTarget::Tcp {
        host: host.unwrap_or("localhost").to_string(),
        port: 6000 + display,
    }
}

fn parse_display_number(value: &str) -> Option<u16> {
    value
        .split('.')
        .next()
        .filter(|part| !part.is_empty())
        .and_then(|part| part.parse::<u16>().ok())
}

fn display_number(display: &str) -> Option<u16> {
    let trimmed = display.trim();
    if let Some(rest) = trimmed.strip_prefix(':') {
        return parse_display_number(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("unix:") {
        return parse_display_number(rest);
    }
    trimmed
        .rsplit_once(':')
        .and_then(|(_host, rest)| parse_display_number(rest))
        .filter(|n| *n < 100)
}

enum LocalX11Stream {
    Tcp(tokio::net::TcpStream),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
}

impl AsyncRead for LocalX11Stream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Tcp(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for LocalX11Stream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Tcp(stream) => std::pin::Pin::new(stream).poll_write(cx, data),
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_write(cx, data),
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Tcp(stream) => std::pin::Pin::new(stream).poll_flush(cx),
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Tcp(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
        }
    }
}

async fn connect_local_x_server(target: &X11DisplayTarget) -> std::io::Result<LocalX11Stream> {
    match target {
        X11DisplayTarget::Tcp { host, port } => {
            tokio::net::TcpStream::connect((host.as_str(), *port))
                .await
                .map(LocalX11Stream::Tcp)
        }
        #[cfg(unix)]
        X11DisplayTarget::UnixSocket { path } => tokio::net::UnixStream::connect(path)
            .await
            .map(LocalX11Stream::Unix),
    }
}

async fn connect_local_x_server_with_fallback(
    primary: &X11DisplayTarget,
    fallback: Option<&X11DisplayTarget>,
) -> std::io::Result<LocalX11Stream> {
    match connect_local_x_server(primary).await {
        Ok(stream) => Ok(stream),
        Err(primary_error) => {
            if let Some(fallback) = fallback {
                connect_local_x_server(fallback)
                    .await
                    .map_err(|_| primary_error)
            } else {
                Err(primary_error)
            }
        }
    }
}

async fn read_local_x11_auth_cookie(display: &str) -> Option<Vec<u8>> {
    let xauth = if cfg!(target_os = "macos") && std::path::Path::new("/opt/X11/bin/xauth").exists()
    {
        "/opt/X11/bin/xauth"
    } else {
        "xauth"
    };

    let mut command = tokio::process::Command::new(xauth);
    command
        .arg("list")
        .arg(display)
        .env("DISPLAY", display)
        .kill_on_drop(true);

    let output = tokio::time::timeout(XAUTH_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_xauth_cookie(&text, display)
}

fn parse_xauth_cookie(output: &str, display: &str) -> Option<Vec<u8>> {
    let display_num = display_number(display);

    for line in output.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 3 || parts[1] != MIT_MAGIC_COOKIE {
            continue;
        }
        let Some(cookie) = decode_hex(parts[2]) else {
            continue;
        };
        if cookie.len() == 16
            && (parts[0] == display
                || (display_num.is_some() && display_num == display_number(parts[0])))
        {
            return Some(cookie);
        }
    }

    None
}

pub struct X11AuthRewriter {
    fake_cookie: Zeroizing<Vec<u8>>,
    real_cookie: Option<Zeroizing<Vec<u8>>>,
    buffer: Zeroizing<Vec<u8>>,
    complete: bool,
    failed: bool,
}

impl X11AuthRewriter {
    pub fn new(fake_cookie: Vec<u8>, real_cookie: Option<Vec<u8>>) -> Self {
        Self {
            fake_cookie: Zeroizing::new(fake_cookie),
            real_cookie: real_cookie.map(Zeroizing::new),
            buffer: Zeroizing::new(Vec::new()),
            complete: false,
            failed: false,
        }
    }

    pub fn push(&mut self, data: &[u8]) -> std::io::Result<Vec<u8>> {
        let result = self.push_inner(data);
        if result.is_err() {
            self.failed = true;
            self.buffer.zeroize();
        }
        result
    }

    fn push_inner(&mut self, mut data: &[u8]) -> std::io::Result<Vec<u8>> {
        if self.failed {
            return Err(invalid_setup());
        }
        if self.complete {
            return Ok(data.to_vec());
        }
        // Only buffer the setup, never an arbitrarily large first SSH message.
        let header = (12 - self.buffer.len().min(12)).min(data.len());
        self.buffer.extend_from_slice(&data[..header]);
        data = &data[header..];
        let Some(packet_len) = setup_packet_len(&self.buffer)? else {
            return Ok(Vec::new());
        };
        let count = (packet_len - self.buffer.len()).min(data.len());
        self.buffer.extend_from_slice(&data[..count]);
        data = &data[count..];
        if self.buffer.len() < packet_len {
            return Ok(Vec::new());
        }
        // First validate the SSH session's fake cookie even for servers that
        // permit local unauthenticated clients. Never pass failed auth through.
        if !rewrite_x11_auth_setup_packet(
            &mut self.buffer,
            &self.fake_cookie,
            Some(&self.fake_cookie),
        ) {
            return Err(invalid_setup());
        }
        let mut output = self.buffer.to_vec();
        if let Some(cookie) = &self.real_cookie {
            if !rewrite_x11_auth_setup_packet(&mut output, &self.fake_cookie, Some(cookie)) {
                return Err(invalid_setup());
            }
        } else {
            output.truncate(12);
            output[6..10].fill(0);
        }
        self.buffer.zeroize();
        output.extend_from_slice(data);
        self.complete = true;
        Ok(output)
    }
}

fn invalid_setup() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Invalid X11 authentication setup",
    )
}

fn setup_packet_len(buffer: &[u8]) -> std::io::Result<Option<usize>> {
    if buffer
        .first()
        .is_some_and(|order| !matches!(order, b'l' | b'B'))
    {
        return Err(invalid_setup());
    }
    if buffer.len() < 12 {
        return Ok(None);
    }
    let byte_order = buffer[0];
    let read_u16 = |offset: usize| -> Option<u16> {
        let bytes = [*buffer.get(offset)?, *buffer.get(offset + 1)?];
        match byte_order {
            b'l' => Some(u16::from_le_bytes(bytes)),
            b'B' => Some(u16::from_be_bytes(bytes)),
            _ => None,
        }
    };

    if read_u16(2) != Some(11) || read_u16(4) != Some(0) {
        return Err(invalid_setup());
    }
    let auth_protocol_len = read_u16(6).ok_or_else(invalid_setup)? as usize;
    let auth_data_len = read_u16(8).ok_or_else(invalid_setup)? as usize;
    let length = 12 + pad4(auth_protocol_len) + pad4(auth_data_len);
    if length > MAX_SETUP_BYTES {
        return Err(invalid_setup());
    }
    Ok(Some(length))
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

pub fn rewrite_x11_auth_setup_packet(
    buffer: &mut [u8],
    fake_cookie: &[u8],
    real_cookie: Option<&[u8]>,
) -> bool {
    let Some(real_cookie) = real_cookie else {
        return false;
    };
    if buffer.len() < 12 {
        return false;
    }

    let byte_order = buffer[0];
    let read_u16 = |offset: usize| -> Option<u16> {
        let bytes = [*buffer.get(offset)?, *buffer.get(offset + 1)?];
        match byte_order {
            b'l' => Some(u16::from_le_bytes(bytes)),
            b'B' => Some(u16::from_be_bytes(bytes)),
            _ => None,
        }
    };

    let protocol_len = read_u16(6).unwrap_or(0) as usize;
    let auth_len = read_u16(8).unwrap_or(0) as usize;
    let protocol_start = 12;
    let protocol_end = protocol_start + protocol_len;
    let auth_start = protocol_start + pad4(protocol_len);
    let auth_end = auth_start + auth_len;

    if auth_end > buffer.len() || protocol_end > buffer.len() {
        return false;
    }
    if &buffer[protocol_start..protocol_end] != MIT_MAGIC_COOKIE.as_bytes() {
        return false;
    }
    if auth_len != real_cookie.len() || auth_len != fake_cookie.len() {
        return false;
    }
    if &buffer[auth_start..auth_end] != fake_cookie {
        return false;
    }

    buffer[auth_start..auth_end].copy_from_slice(real_cookie);
    true
}

pub(super) fn spawn_x11_forwarder(
    event_queue: SessionEventQueue,
    session_id: String,
    mut forwarder: X11Forwarder,
) -> tokio_util::task::AbortOnDropHandle<()> {
    tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        struct RuntimeGuard(Option<std::sync::Arc<dyn nyaterm_core::x11::X11Lifetime>>);
        impl Drop for RuntimeGuard {
            fn drop(&mut self) {
                if let Some(lifetime) = &self.0 {
                    lifetime.shutdown();
                }
            }
        }
        let lifetime = RuntimeGuard(forwarder.config.lifetime.clone());
        let mut tasks = tokio::task::JoinSet::new();
        let mut health = tokio::time::interval(Duration::from_millis(250));
        health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let open = tokio::select! {
                open = forwarder.rx.recv() => match open { Some(open) => open, None => break },
                _ = tasks.join_next(), if !tasks.is_empty() => continue,
                _ = health.tick(), if lifetime.0.is_some() => {
                    if lifetime.0.as_ref().is_some_and(|lease| !lease.is_alive()) {
                        event_queue.push(SessionEvent::Notice { session_id: session_id.clone(),
                            notice: crate::session_notice::SessionNotice::X11ServerUnavailable { display_target: forwarder.config.target.describe() } });
                        break;
                    }
                    continue;
                },
            };
            if tasks.len() >= MAX_X11_CHANNELS {
                let _ = open.channel.close().await;
                continue;
            }
            let target = forwarder.config.target.clone();
            let fallback = forwarder.config.fallback_target.clone();
            let fake_cookie = forwarder.config.fake_cookie.clone();
            let real_cookie = forwarder.config.real_cookie.clone();
            let event_queue = event_queue.clone();
            let session_id = session_id.clone();
            tasks.spawn(async move {
                let _ = handle_x11_channel(
                    event_queue,
                    session_id,
                    open,
                    target,
                    fallback,
                    fake_cookie,
                    real_cookie,
                )
                .await;
            });
        }
    }))
}

async fn handle_x11_channel(
    event_queue: SessionEventQueue,
    session_id: String,
    open: X11ChannelOpen,
    target: X11DisplayTarget,
    fallback: Option<X11DisplayTarget>,
    fake_cookie: Vec<u8>,
    real_cookie: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    let X11ChannelOpen {
        channel,
        originator_address,
        originator_port,
    } = open;
    let _originator = (originator_address, originator_port);

    let local = match connect_local_x_server_with_fallback(&target, fallback.as_ref()).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = channel.close().await;
            event_queue.push(SessionEvent::Notice {
                session_id,
                notice: crate::session_notice::SessionNotice::X11ServerUnavailable {
                    display_target: target.describe(),
                },
            });
            anyhow::bail!("failed to connect local X11 server: {error}");
        }
    };

    let (mut remote_read, remote_write) = channel.split();
    let mut remote_writer = remote_write.make_writer();
    let (mut local_read, mut local_write) = tokio::io::split(local);
    let mut rewriter = X11AuthRewriter::new(fake_cookie, real_cookie);

    let remote_to_local = async {
        let deadline = tokio::time::Instant::now() + SETUP_TIMEOUT;
        loop {
            let msg = if rewriter.complete {
                remote_read.wait().await
            } else {
                tokio::time::timeout_at(deadline, remote_read.wait())
                    .await
                    .map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "X11 setup timed out")
                    })?
            };
            let Some(msg) = msg else { break };
            match msg {
                ChannelMsg::Data { data } => {
                    let rewritten = Zeroizing::new(rewriter.push(&data)?);
                    if !rewritten.is_empty() {
                        local_write.write_all(&rewritten).await?;
                    }
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        let _ = local_write.shutdown().await;
        Ok::<(), std::io::Error>(())
    };

    let local_to_remote = async {
        let mut buf = [0_u8; 16 * 1024];
        loop {
            let n = local_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            remote_writer.write_all(&buf[..n]).await?;
        }
        let _ = remote_writer.shutdown().await;
        Ok::<(), std::io::Error>(())
    };

    let result = tokio::select! {
        result = remote_to_local => result,
        result = local_to_remote => result,
    };

    // Dropping russh channel halves does not send CHANNEL_CLOSE. In particular,
    // rejected setup authentication must not leave the remote X client waiting
    // forever for a reply on a channel that no longer has a forwarding task.
    let _ = remote_write.close().await;
    result.map_err(Into::into)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().as_chunks::<2>().0 {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod forwarding_tests;

#[cfg(test)]
mod tests {
    use super::{
        MIT_MAGIC_COOKIE, X11AuthRewriter, X11DisplayTarget, decode_hex, pad4, parse_xauth_cookie,
        resolve_x11_display_spec, rewrite_x11_auth_setup_packet,
    };

    fn x11_target_desc(target: X11DisplayTarget) -> String {
        target.describe()
    }

    #[test]
    fn x11_display_specs_match_legacy_resolution() {
        assert_eq!(
            x11_target_desc(resolve_x11_display_spec(Some("localhost:0"))),
            "localhost:6000"
        );
        assert_eq!(
            x11_target_desc(resolve_x11_display_spec(Some("localhost:1"))),
            "localhost:6001"
        );
        assert_eq!(
            x11_target_desc(resolve_x11_display_spec(Some("127.0.0.1:0"))),
            "127.0.0.1:6000"
        );
        assert_eq!(
            x11_target_desc(resolve_x11_display_spec(Some("host.example.com:1"))),
            "host.example.com:6001"
        );
        assert_eq!(
            x11_target_desc(resolve_x11_display_spec(Some("localhost:6000"))),
            "localhost:6000"
        );
        assert_eq!(
            x11_target_desc(resolve_x11_display_spec(Some(""))),
            x11_target_desc(resolve_x11_display_spec(None))
        );

        #[cfg(unix)]
        {
            assert_eq!(
                x11_target_desc(resolve_x11_display_spec(Some(":0"))),
                "/tmp/.X11-unix/X0"
            );
            assert_eq!(
                x11_target_desc(resolve_x11_display_spec(Some("unix:0"))),
                "/tmp/.X11-unix/X0"
            );
            assert_eq!(
                x11_target_desc(resolve_x11_display_spec(Some("/tmp/.X11-unix/X1"))),
                "/tmp/.X11-unix/X1"
            );
        }
    }

    fn x11_setup_packet(order: u8, protocol: &[u8], cookie: &[u8]) -> Vec<u8> {
        let mut packet = vec![order, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        packet[2..4].copy_from_slice(&if order == b'l' {
            11u16.to_le_bytes()
        } else {
            11u16.to_be_bytes()
        });
        let protocol_len = protocol.len() as u16;
        let cookie_len = cookie.len() as u16;
        let protocol_bytes = if order == b'l' {
            protocol_len.to_le_bytes()
        } else {
            protocol_len.to_be_bytes()
        };
        let cookie_bytes = if order == b'l' {
            cookie_len.to_le_bytes()
        } else {
            cookie_len.to_be_bytes()
        };
        packet[6..8].copy_from_slice(&protocol_bytes);
        packet[8..10].copy_from_slice(&cookie_bytes);
        packet.extend_from_slice(protocol);
        packet.resize(12 + pad4(protocol.len()), 0);
        packet.extend_from_slice(cookie);
        packet.resize(12 + pad4(protocol.len()) + pad4(cookie.len()), 0);
        packet
    }

    #[test]
    fn x11_cookie_rewrite_supports_little_and_big_endian_setup() {
        let fake = [1_u8; 16];
        let real = [2_u8; 16];

        for order in *b"lB" {
            let mut packet = x11_setup_packet(order, MIT_MAGIC_COOKIE.as_bytes(), &fake);
            assert!(rewrite_x11_auth_setup_packet(
                &mut packet,
                &fake,
                Some(&real)
            ));
            assert!(packet.windows(real.len()).any(|window| window == real));
            assert!(!packet.windows(fake.len()).any(|window| window == fake));
        }
    }

    #[test]
    fn x11_rewriter_buffers_fragmented_setup_packet() {
        let fake = [1_u8; 16];
        let real = [2_u8; 16];
        let packet = x11_setup_packet(b'l', MIT_MAGIC_COOKIE.as_bytes(), &fake);
        let mut rewriter = X11AuthRewriter::new(fake.to_vec(), Some(real.to_vec()));

        assert!(rewriter.push(&packet[..8]).unwrap().is_empty());
        let output = rewriter.push(&packet[8..]).unwrap();
        assert_eq!(output.len(), packet.len());
        assert!(output.windows(real.len()).any(|window| window == real));
    }

    #[test]
    fn x11_rewriter_rejects_mismatched_auth() {
        let fake = [1_u8; 16];
        let real = [2_u8; 16];
        let other = [3_u8; 16];
        let packet = x11_setup_packet(b'l', MIT_MAGIC_COOKIE.as_bytes(), &other);
        let mut rewriter = X11AuthRewriter::new(fake.to_vec(), Some(real.to_vec()));

        assert!(rewriter.push(&packet).is_err());

        let mut packet = x11_setup_packet(b'l', b"OTHER", &fake);
        assert!(!rewrite_x11_auth_setup_packet(
            &mut packet,
            &fake,
            Some(&real)
        ));
    }

    #[test]
    fn x11_xauth_cookie_parser_prefers_matching_display() {
        let output = "\
host/unix:0  MIT-MAGIC-COOKIE-1  00112233445566778899aabbccddeeff
host/unix:1  MIT-MAGIC-COOKIE-1  ffeeddccbbaa99887766554433221100
";

        assert_eq!(
            parse_xauth_cookie(output, ":1").expect("display 1 cookie"),
            decode_hex("ffeeddccbbaa99887766554433221100").expect("hex")
        );
        assert!(parse_xauth_cookie(output, ":9").is_none());
    }

    #[test]
    fn malformed_setup_is_rejected_without_retaining_input() {
        let mut rewriter = X11AuthRewriter::new(vec![1; 16], Some(vec![2; 16]));
        assert!(rewriter.push(&[b'?'; 65536]).is_err());
        assert!(rewriter.buffer.is_empty());
        assert!(
            rewriter
                .push(&x11_setup_packet(
                    b'l',
                    MIT_MAGIC_COOKIE.as_bytes(),
                    &[1; 16]
                ))
                .is_err()
        );
        let mut packet = x11_setup_packet(b'l', MIT_MAGIC_COOKIE.as_bytes(), &[1; 16]);
        packet[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
        let mut rewriter = X11AuthRewriter::new(vec![1; 16], Some(vec![2; 16]));
        assert!(rewriter.push(&packet[..12]).is_err());
        assert!(rewriter.buffer.is_empty());
    }

    #[test]
    fn unauthenticated_local_server_still_requires_session_cookie() {
        for order in *b"lB" {
            let mut rewriter = X11AuthRewriter::new(vec![1; 16], None);
            let mut packet = x11_setup_packet(order, MIT_MAGIC_COOKIE.as_bytes(), &[1; 16]);
            packet.extend_from_slice(b"subsequent request");
            let rewritten = rewriter.push(&packet).unwrap();
            assert_eq!(&rewritten[6..10], &[0; 4]);
            assert_eq!(&rewritten[12..], b"subsequent request");
            let mut rewriter = X11AuthRewriter::new(vec![1; 16], None);
            assert!(
                rewriter
                    .push(&x11_setup_packet(
                        order,
                        MIT_MAGIC_COOKIE.as_bytes(),
                        &[3; 16]
                    ))
                    .is_err()
            );
        }
    }

    #[test]
    fn setup_can_be_fragmented_at_every_byte_boundary() {
        for order in *b"lB" {
            let packet = x11_setup_packet(order, MIT_MAGIC_COOKIE.as_bytes(), &[1; 16]);
            for split in 1..packet.len() {
                let mut rewriter = X11AuthRewriter::new(vec![1; 16], Some(vec![2; 16]));
                assert!(rewriter.push(&packet[..split]).unwrap().is_empty());
                assert_eq!(
                    rewriter.push(&packet[split..]).unwrap(),
                    x11_setup_packet(order, MIT_MAGIC_COOKIE.as_bytes(), &[2; 16])
                );
            }
        }
    }
}
