//! Lightweight RFB security probe used by the connection editor's "test
//! connection" action.
//!
//! The full VNC session path runs in the isolated helper because the image
//! decoders parse server-controlled bytes. This probe only negotiates the
//! protocol version and the classic security types (`None` and `VncAuth`) so
//! the editor can verify a password without spawning a helper, a framebuffer or
//! a terminal tab. Security types it cannot evaluate (for example Apple RA2 or
//! VeNCrypt) fall back to a reachability result instead of a false failure.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use des::Des;
use des::cipher::{Block, BlockCipherEncrypt, Key, KeyInit};
use nyaterm_core::connection_route::RelayEndpoint;

use crate::protocol::VncSecurityMode;

const RFB_VERSION_3_3: [u8; 12] = *b"RFB 003.003\n";
const RFB_VERSION_3_8: [u8; 12] = *b"RFB 003.008\n";
const SECURITY_TYPE_NONE: u8 = 1;
const SECURITY_TYPE_VNC_AUTH: u8 = 2;
/// Server-provided reason strings are informational. Bound them so a hostile
/// server cannot make the probe allocate unbounded memory.
const MAX_REASON_BYTES: usize = 4096;
const IO_POLL_INTERVAL: Duration = Duration::from_millis(200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What a VNC probe managed to establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VncProbeOutcome {
    /// TCP connected and the server spoke RFB, but it only offered security
    /// types this probe does not evaluate.
    Reachable,
    /// TCP connected and the server accepted the configured security type
    /// (including a password challenge when one applies).
    Authenticated,
}

/// Connect to a VNC endpoint, negotiate security, and report whether the
/// configured password is accepted.
///
/// `is_cancelled` is polled between blocking I/O slices so closing the editor
/// stops the probe promptly. `timeout` bounds the whole exchange.
pub fn probe_vnc_connection(
    host: &str,
    port: u16,
    relay: Option<&RelayEndpoint>,
    password: Option<&str>,
    mode: VncSecurityMode,
    timeout: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<VncProbeOutcome, String> {
    let deadline = Instant::now() + timeout;
    let mut stream = connect(host, port, relay, deadline, is_cancelled)?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(IO_POLL_INTERVAL)).ok();
    stream.set_write_timeout(Some(IO_POLL_INTERVAL)).ok();
    probe_stream(&mut stream, password, mode, deadline, is_cancelled)
}

fn connect(
    host: &str,
    port: u16,
    relay: Option<&RelayEndpoint>,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<TcpStream, String> {
    if let Some(relay) = relay {
        relay.validate()?;
        let mut stream = connect_endpoint(relay.address, deadline, is_cancelled)?;
        stream
            .write_all(relay.token.expose_secret().as_bytes())
            .map_err(|error| error.to_string())?;
        // The relay forwards the RFB greeting once it has authenticated, so the
        // token write completes the relay handshake.
        return Ok(stream);
    }

    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("could not resolve {host}:{port}"));
    }
    let mut last_error = None;
    for address in addresses {
        check_state(deadline, is_cancelled)?;
        match connect_endpoint(address, deadline, is_cancelled) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| format!("could not connect to {host}:{port}")))
}

fn connect_endpoint(
    address: std::net::SocketAddr,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<TcpStream, String> {
    check_state(deadline, is_cancelled)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let timeout = remaining.min(CONNECT_TIMEOUT).max(Duration::from_millis(1));
    TcpStream::connect_timeout(&address, timeout).map_err(|error| error.to_string())
}

fn probe_stream(
    stream: &mut TcpStream,
    password: Option<&str>,
    mode: VncSecurityMode,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<VncProbeOutcome, String> {
    let mut greeting = [0_u8; 12];
    read_exact_until(stream, &mut greeting, deadline, is_cancelled)?;
    let minor = parse_rfb_version(&greeting)?;

    if minor >= 7 {
        write_all_until(stream, &RFB_VERSION_3_8, deadline, is_cancelled)?;
        probe_security_types(stream, password, mode, deadline, is_cancelled)
    } else {
        write_all_until(stream, &RFB_VERSION_3_3, deadline, is_cancelled)?;
        probe_security_type_3_3(stream, password, mode, deadline, is_cancelled)
    }
}

fn probe_security_types(
    stream: &mut TcpStream,
    password: Option<&str>,
    mode: VncSecurityMode,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<VncProbeOutcome, String> {
    let count = read_u8_until(stream, deadline, is_cancelled)?;
    if count == 0 {
        let reason = read_reason(stream, deadline, is_cancelled)?;
        return Err(describe_rejection(reason));
    }
    let mut types = vec![0_u8; usize::from(count)];
    read_exact_until(stream, &mut types, deadline, is_cancelled)?;

    match select_security_type(&types, password.is_some(), mode) {
        ChosenSecurity::None => {
            write_all_until(stream, &[SECURITY_TYPE_NONE], deadline, is_cancelled)?;
            // RFB 3.8 reports `None` through SecurityResult as well.
            read_security_result(stream, deadline, is_cancelled)?;
            Ok(VncProbeOutcome::Authenticated)
        }
        ChosenSecurity::VncAuth => {
            write_all_until(stream, &[SECURITY_TYPE_VNC_AUTH], deadline, is_cancelled)?;
            vnc_auth(stream, password.unwrap_or_default(), deadline, is_cancelled)?;
            Ok(VncProbeOutcome::Authenticated)
        }
        ChosenSecurity::Unsupported => Ok(VncProbeOutcome::Reachable),
    }
}

fn probe_security_type_3_3(
    stream: &mut TcpStream,
    password: Option<&str>,
    _mode: VncSecurityMode,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<VncProbeOutcome, String> {
    let security = read_u32_until(stream, deadline, is_cancelled)?;
    match security {
        0 => {
            let reason = read_reason(stream, deadline, is_cancelled)?;
            Err(describe_rejection(reason))
        }
        value if value == u32::from(SECURITY_TYPE_NONE) => Ok(VncProbeOutcome::Authenticated),
        value if value == u32::from(SECURITY_TYPE_VNC_AUTH) => {
            vnc_auth(stream, password.unwrap_or_default(), deadline, is_cancelled)?;
            Ok(VncProbeOutcome::Authenticated)
        }
        _ => Ok(VncProbeOutcome::Reachable),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChosenSecurity {
    None,
    VncAuth,
    Unsupported,
}

fn select_security_type(types: &[u8], has_password: bool, mode: VncSecurityMode) -> ChosenSecurity {
    let has_none = types.contains(&SECURITY_TYPE_NONE);
    let has_vnc_auth = types.contains(&SECURITY_TYPE_VNC_AUTH);
    match mode {
        VncSecurityMode::None => {
            if has_none {
                ChosenSecurity::None
            } else if has_vnc_auth {
                ChosenSecurity::VncAuth
            } else {
                ChosenSecurity::Unsupported
            }
        }
        VncSecurityMode::VncAuth => {
            if has_vnc_auth {
                ChosenSecurity::VncAuth
            } else if has_none {
                ChosenSecurity::None
            } else {
                ChosenSecurity::Unsupported
            }
        }
        VncSecurityMode::Auto => {
            if has_vnc_auth && has_password {
                ChosenSecurity::VncAuth
            } else if has_none {
                ChosenSecurity::None
            } else if has_vnc_auth {
                ChosenSecurity::VncAuth
            } else {
                ChosenSecurity::Unsupported
            }
        }
    }
}

fn vnc_auth(
    stream: &mut TcpStream,
    password: &str,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut challenge = [0_u8; 16];
    read_exact_until(stream, &mut challenge, deadline, is_cancelled)?;
    let response = vnc_challenge_response(password, &challenge);
    write_all_until(stream, &response, deadline, is_cancelled)?;
    read_security_result(stream, deadline, is_cancelled)
}

fn read_security_result(
    stream: &mut TcpStream,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let result = read_u32_until(stream, deadline, is_cancelled)?;
    if result == 0 {
        return Ok(());
    }
    let reason = read_reason(stream, deadline, is_cancelled).unwrap_or_default();
    Err(if reason.is_empty() {
        "VNC authentication failed".to_string()
    } else {
        format!("VNC authentication failed: {reason}")
    })
}

/// Compute the VNC DES response for a 16-byte challenge.
///
/// The VNC key is the first eight password bytes padded with zeroes and with
/// every bit reversed; the two challenge halves are encrypted with single DES.
fn vnc_challenge_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    let mut key_bytes = [0_u8; 8];
    let password = password.as_bytes();
    let copy_len = password.len().min(key_bytes.len());
    key_bytes[..copy_len].copy_from_slice(&password[..copy_len]);
    for byte in &mut key_bytes {
        *byte = byte.reverse_bits();
    }

    let cipher = Des::new(&Key::<Des>::from(key_bytes));
    let mut response = [0_u8; 16];
    for index in 0..2 {
        let start = index * 8;
        let block_bytes: [u8; 8] = challenge[start..start + 8]
            .try_into()
            .expect("challenge is exactly sixteen bytes");
        let mut block = Block::<Des>::from(block_bytes);
        cipher.encrypt_block(&mut block);
        response[start..start + 8].copy_from_slice(&block.0);
    }
    response
}

/// Parse `RFB 003.008\n` into the minor version.
fn parse_rfb_version(greeting: &[u8; 12]) -> Result<u32, String> {
    if &greeting[..4] != b"RFB " || greeting[11] != b'\n' {
        return Err("server did not send an RFB greeting".to_string());
    }
    let text = std::str::from_utf8(&greeting[4..11])
        .map_err(|_| "server sent a non-ASCII RFB version".to_string())?;
    let (major, minor) = text
        .split_once('.')
        .ok_or_else(|| "server sent a malformed RFB version".to_string())?;
    let major: u32 = major
        .parse()
        .map_err(|_| "server sent a malformed RFB version".to_string())?;
    let minor: u32 = minor
        .parse()
        .map_err(|_| "server sent a malformed RFB version".to_string())?;
    if major != 3 {
        return Err(format!("unsupported RFB version {major}.{minor}"));
    }
    Ok(minor)
}

fn describe_rejection(reason: String) -> String {
    if reason.is_empty() {
        "VNC server refused the connection".to_string()
    } else {
        reason
    }
}

fn check_state(deadline: Instant, is_cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if is_cancelled() {
        return Err("connection test cancelled".to_string());
    }
    if Instant::now() >= deadline {
        return Err("VNC connection test timed out".to_string());
    }
    Ok(())
}

fn read_u8_until(
    stream: &mut TcpStream,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<u8, String> {
    let mut bytes = [0_u8; 1];
    read_exact_until(stream, &mut bytes, deadline, is_cancelled)?;
    Ok(bytes[0])
}

fn read_u32_until(
    stream: &mut TcpStream,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<u32, String> {
    let mut bytes = [0_u8; 4];
    read_exact_until(stream, &mut bytes, deadline, is_cancelled)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_reason(
    stream: &mut TcpStream,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<String, String> {
    let length = read_u32_until(stream, deadline, is_cancelled)?;
    let length = usize::try_from(length)
        .unwrap_or(MAX_REASON_BYTES)
        .min(MAX_REASON_BYTES);
    let mut reason = vec![0_u8; length];
    read_exact_until(stream, &mut reason, deadline, is_cancelled)?;
    Ok(String::from_utf8_lossy(&reason).trim().to_string())
}

fn read_exact_until(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut filled = 0;
    while filled < buffer.len() {
        check_state(deadline, is_cancelled)?;
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => return Err("VNC server closed the connection".to_string()),
            Ok(read) => filled += read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn write_all_until(
    stream: &mut TcpStream,
    mut buffer: &[u8],
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    while !buffer.is_empty() {
        check_state(deadline, is_cancelled)?;
        match stream.write(buffer) {
            Ok(0) => return Err("VNC server closed the connection".to_string()),
            Ok(written) => buffer = &buffer[written..],
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use des::Des;
    use des::cipher::{Block, BlockCipherEncrypt, Key, KeyInit};

    use super::{VncProbeOutcome, probe_vnc_connection, vnc_challenge_response};
    use crate::protocol::VncSecurityMode;

    const TIMEOUT: Duration = Duration::from_secs(3);

    fn never_cancelled() -> bool {
        false
    }

    fn spawn_server(
        handle: impl FnOnce(TcpStreamAlias) + Send + 'static,
    ) -> (u16, mpsc::Receiver<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind probe server");
        let port = listener.local_addr().expect("probe server address").port();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let (stream, _) = listener.accept().expect("probe client");
            handle(stream);
            let _ = done_tx.send(());
        });
        (port, done_rx)
    }

    type TcpStreamAlias = std::net::TcpStream;

    fn probe(
        port: u16,
        password: Option<&str>,
        mode: VncSecurityMode,
    ) -> Result<VncProbeOutcome, String> {
        probe_vnc_connection(
            "127.0.0.1",
            port,
            None,
            password,
            mode,
            TIMEOUT,
            &never_cancelled,
        )
    }

    #[test]
    fn des_primitive_matches_the_standard_vector() {
        let cipher = Des::new(&Key::<Des>::from([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        ]));
        let mut block = Block::<Des>::from(*b"Now is t");
        cipher.encrypt_block(&mut block);
        assert_eq!(block.0, [0x3f, 0xa4, 0x0e, 0x8a, 0x98, 0x4d, 0x48, 0x15]);
    }

    #[test]
    fn vnc_auth_success_returns_authenticated() {
        let (port, done) = spawn_server(|mut stream| {
            stream.write_all(b"RFB 003.008\n").unwrap();
            let mut version = [0_u8; 12];
            stream.read_exact(&mut version).unwrap();
            assert_eq!(&version, b"RFB 003.008\n");
            stream.write_all(&[1, 2]).unwrap();
            let mut chosen = [0_u8; 1];
            stream.read_exact(&mut chosen).unwrap();
            assert_eq!(chosen[0], 2);
            let challenge = [7_u8; 16];
            stream.write_all(&challenge).unwrap();
            let mut response = [0_u8; 16];
            stream.read_exact(&mut response).unwrap();
            assert_eq!(response, vnc_challenge_response("secret", &challenge));
            stream.write_all(&0_u32.to_be_bytes()).unwrap();
        });

        let result = probe(port, Some("secret"), VncSecurityMode::VncAuth);
        assert_eq!(result, Ok(VncProbeOutcome::Authenticated));
        done.recv_timeout(TIMEOUT).expect("server finished");
    }

    #[test]
    fn vnc_auth_rejection_surfaces_the_server_reason() {
        let (port, done) = spawn_server(|mut stream| {
            stream.write_all(b"RFB 003.008\n").unwrap();
            let mut version = [0_u8; 12];
            stream.read_exact(&mut version).unwrap();
            stream.write_all(&[1, 2]).unwrap();
            let mut chosen = [0_u8; 1];
            stream.read_exact(&mut chosen).unwrap();
            stream.write_all(&[0x11; 16]).unwrap();
            let mut response = [0_u8; 16];
            stream.read_exact(&mut response).unwrap();
            stream.write_all(&1_u32.to_be_bytes()).unwrap();
            let reason = b"bad password";
            stream
                .write_all(&(reason.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(reason).unwrap();
        });

        let result = probe(port, Some("wrong"), VncSecurityMode::VncAuth);
        let error = result.expect_err("rejected password");
        assert!(error.contains("bad password"), "{error}");
        done.recv_timeout(TIMEOUT).expect("server finished");
    }

    #[test]
    fn none_security_is_accepted_when_the_server_requires_no_password() {
        let (port, done) = spawn_server(|mut stream| {
            stream.write_all(b"RFB 003.008\n").unwrap();
            let mut version = [0_u8; 12];
            stream.read_exact(&mut version).unwrap();
            stream.write_all(&[1, 1]).unwrap();
            let mut chosen = [0_u8; 1];
            stream.read_exact(&mut chosen).unwrap();
            assert_eq!(chosen[0], 1);
            stream.write_all(&0_u32.to_be_bytes()).unwrap();
        });

        let result = probe(port, None, VncSecurityMode::Auto);
        assert_eq!(result, Ok(VncProbeOutcome::Authenticated));
        done.recv_timeout(TIMEOUT).expect("server finished");
    }

    #[test]
    fn unsupported_security_falls_back_to_reachable() {
        let (port, done) = spawn_server(|mut stream| {
            stream.write_all(b"RFB 003.008\n").unwrap();
            let mut version = [0_u8; 12];
            stream.read_exact(&mut version).unwrap();
            // 30 is Apple RA2; this probe does not evaluate it.
            stream.write_all(&[2, 30, 19]).unwrap();
            // Wait for the client to drop the connection.
            let mut byte = [0_u8; 1];
            let _ = stream.read(&mut byte);
        });

        let result = probe(port, Some("secret"), VncSecurityMode::Auto);
        assert_eq!(result, Ok(VncProbeOutcome::Reachable));
        done.recv_timeout(TIMEOUT).expect("server finished");
    }

    #[test]
    fn rfb_3_3_none_returns_authenticated() {
        let (port, done) = spawn_server(|mut stream| {
            stream.write_all(b"RFB 003.003\n").unwrap();
            let mut version = [0_u8; 12];
            stream.read_exact(&mut version).unwrap();
            assert_eq!(&version, b"RFB 003.003\n");
            stream.write_all(&1_u32.to_be_bytes()).unwrap();
        });

        let result = probe(port, None, VncSecurityMode::Auto);
        assert_eq!(result, Ok(VncProbeOutcome::Authenticated));
        done.recv_timeout(TIMEOUT).expect("server finished");
    }
}
