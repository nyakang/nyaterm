use crate::process::OwnedChild;
use nyaterm_core::x11::{X11Connection, X11Endpoint, X11Error, X11Mode};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(2);

fn output(
    command: &mut Command,
    cancelled: &impl Fn() -> bool,
) -> Result<Zeroizing<Vec<u8>>, X11Error> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = OwnedChild::spawn(command).map_err(|_| X11Error::Startup)?;
    let stdout = child.child_mut().stdout.take().ok_or(X11Error::Startup)?;
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut bytes = Zeroizing::new(Vec::new());
        let result = stdout.take(65537).read_to_end(&mut bytes);
        let _ = tx.send(result.map(|_| bytes));
    });
    let deadline = Instant::now() + TIMEOUT;
    let result = loop {
        if cancelled() {
            break Err(X11Error::Cancelled);
        }
        if Instant::now() >= deadline {
            break Err(X11Error::Startup);
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(bytes)) if bytes.len() <= 65536 => {
                loop {
                    if cancelled() || Instant::now() >= deadline {
                        break;
                    }
                    if let Ok(Some(_)) = child.child_mut().try_wait() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                break match child.child_mut().try_wait() {
                    Ok(Some(status)) if status.success() => Ok(bytes),
                    _ => Err(X11Error::Startup),
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => break Err(X11Error::Startup),
        }
    };
    child.terminate();
    let _ = reader.join();
    result
}

fn display_number(value: &str) -> Option<u16> {
    value.rsplit_once(':')?.1.split('.').next()?.parse().ok()
}

fn endpoint(display: &str) -> Result<X11Endpoint, X11Error> {
    #[cfg(unix)]
    {
        if display.starts_with('/') {
            return Ok(X11Endpoint::UnixSocket {
                path: display.into(),
            });
        }
    }
    if display.starts_with(':') || display.starts_with("unix:") {
        let number = display_number(display).ok_or(X11Error::NoDisplay)?;
        #[cfg(unix)]
        return Ok(X11Endpoint::UnixSocket {
            path: format!("/tmp/.X11-unix/X{number}").into(),
        });
        #[cfg(not(unix))]
        return Ok(X11Endpoint::Tcp {
            host: "localhost".into(),
            port: 6000u16.checked_add(number).ok_or(X11Error::NoDisplay)?,
        });
    }
    let (host, _) = display.rsplit_once(':').ok_or(X11Error::NoDisplay)?;
    let number = display_number(display).ok_or(X11Error::NoDisplay)?;
    // Preserve the historical explicit TCP-port syntax for external users.
    let port = if number >= 100 { number } else { 6000 + number };
    Ok(X11Endpoint::Tcp {
        host: if host.is_empty() || host == "unix" {
            "localhost"
        } else {
            host
        }
        .into(),
        port,
    })
}

fn cookie(output: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let output = std::str::from_utf8(output).ok()?;
    let mut result = None;
    for line in output.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.len() != 3 || words[1] != "MIT-MAGIC-COOKIE-1" || words[2].len() != 32 {
            continue;
        }
        let bytes: Option<Vec<u8>> = words[2]
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                Some(
                    (pair[0] as char).to_digit(16)? as u8 * 16
                        + (pair[1] as char).to_digit(16)? as u8,
                )
            })
            .collect();
        let bytes = Zeroizing::new(bytes?);
        if result.as_ref().is_some_and(|previous| previous != &bytes) {
            return None;
        }
        result = Some(bytes);
    }
    result
}

pub fn prepare_external(
    mode: &X11Mode,
    configured: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<X11Connection, X11Error> {
    if matches!(mode, X11Mode::Managed | X11Mode::Unknown(_)) {
        return Err(X11Error::Unsupported);
    }
    if cancelled() {
        return Err(X11Error::Cancelled);
    }
    #[allow(unused_mut)]
    let mut display = if configured.trim().is_empty() {
        std::env::var("DISPLAY").unwrap_or_default()
    } else {
        configured.trim().to_string()
    };
    #[cfg(windows)]
    if display.is_empty() {
        display = "localhost:0".into();
    }
    #[cfg(target_os = "macos")]
    if configured.trim().is_empty() && matches!(mode, X11Mode::Auto) {
        if !display.is_empty() {
            if let Ok(connection) = prepare_display(&display, cancelled) {
                return Ok(connection);
            }
        }
        let mut query = Command::new("/bin/launchctl");
        query.args(["getenv", "DISPLAY"]);
        display = output(&mut query, cancelled)
            .ok()
            .and_then(|b| String::from_utf8(b.to_vec()).ok())
            .unwrap_or_default()
            .trim()
            .into();
        if !display.is_empty() {
            if let Ok(connection) = prepare_display(&display, cancelled) {
                return Ok(connection);
            }
        }
        if cancelled() {
            return Err(X11Error::Cancelled);
        }
        output(
            Command::new("/usr/bin/open").args(["-g", "-b", "org.xquartz.X11"]),
            cancelled,
        )
        .map_err(|_| X11Error::XQuartzUnavailable)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if cancelled() {
                return Err(X11Error::Cancelled);
            }
            display = output(&mut query, cancelled)
                .ok()
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
                .unwrap_or_default()
                .trim()
                .into();
            if !display.is_empty() {
                if let Ok(connection) = prepare_display(&display, cancelled) {
                    return Ok(connection);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        return Err(X11Error::XQuartzUnavailable);
    }
    prepare_display(&display, cancelled)
}

fn prepare_display(
    display: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<X11Connection, X11Error> {
    if display.is_empty() {
        return Err(X11Error::NoDisplay);
    }
    let xauth = if cfg!(target_os = "macos") {
        "/opt/X11/bin/xauth"
    } else {
        "xauth"
    };
    // xauth performs display/hostname normalization and restricts the result to
    // this display. Never fall back to another display's first cookie.
    let auth = match output(Command::new(xauth).args(["list", display]), cancelled) {
        Ok(bytes) => cookie(&bytes),
        Err(X11Error::Cancelled) => return Err(X11Error::Cancelled),
        Err(_) => None,
    };
    let mut connection = X11Connection {
        endpoint: endpoint(display)?,
        cookie: auth,
        lifetime: None,
    };
    probe_external(&mut connection, display)?;
    if cancelled() {
        return Err(X11Error::Cancelled);
    }
    Ok(connection)
}

fn probe_external(connection: &mut X11Connection, display: &str) -> Result<(), X11Error> {
    match probe(connection) {
        Ok(()) => {}
        Err(X11Error::NoDisplay)
            if matches!(connection.endpoint, X11Endpoint::UnixSocket { .. }) =>
        {
            // Preserve legacy local socket -> TCP fallback, but never retry an
            // authentication rejection against a different server.
            let port = display_number(display)
                .and_then(|number| 6000u16.checked_add(number))
                .ok_or(X11Error::NoDisplay)?;
            connection.endpoint = X11Endpoint::Tcp {
                host: "localhost".into(),
                port,
            };
            probe(connection)?;
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn handshake(stream: &mut (impl Read + Write), cookie: Option<&[u8]>) -> Result<(), X11Error> {
    let protocol = if cookie.is_some() {
        &b"MIT-MAGIC-COOKIE-1"[..]
    } else {
        &[]
    };
    let cookie = cookie.unwrap_or_default();
    let mut setup = Zeroizing::new(vec![b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    setup[6..8].copy_from_slice(&(protocol.len() as u16).to_le_bytes());
    setup[8..10].copy_from_slice(&(cookie.len() as u16).to_le_bytes());
    setup.extend_from_slice(protocol);
    setup.resize(12 + protocol.len().div_ceil(4) * 4, 0);
    setup.extend_from_slice(cookie);
    let length = setup.len().div_ceil(4) * 4;
    setup.resize(length, 0);
    stream
        .write_all(&setup)
        .map_err(|_| X11Error::Authentication)?;
    let mut header = [0; 8];
    stream
        .read_exact(&mut header)
        .map_err(|_| X11Error::Authentication)?;
    if header[0] != 1 || u16::from_le_bytes([header[2], header[3]]) != 11 {
        return Err(X11Error::Authentication);
    }
    let size = u16::from_le_bytes([header[6], header[7]]) as usize * 4;
    if size < 32 {
        return Err(X11Error::Authentication);
    }
    let deadline = Instant::now() + TIMEOUT;
    let mut remaining = size;
    let mut buffer = [0; 4096];
    while remaining > 0 {
        if Instant::now() >= deadline {
            return Err(X11Error::Authentication);
        }
        let count = remaining.min(buffer.len());
        let count = stream
            .read(&mut buffer[..count])
            .map_err(|_| X11Error::Authentication)?;
        if count == 0 {
            return Err(X11Error::Authentication);
        }
        remaining -= count;
    }
    Ok(())
}

pub fn probe(connection: &X11Connection) -> Result<(), X11Error> {
    let cookie = connection.cookie.as_ref().map(|cookie| cookie.as_slice());
    match &connection.endpoint {
        X11Endpoint::Tcp { host, port } => {
            for address in (host.as_str(), *port)
                .to_socket_addrs()
                .map_err(|_| X11Error::NoDisplay)?
            {
                if let Ok(mut stream) = TcpStream::connect_timeout(&address, TIMEOUT) {
                    stream
                        .set_read_timeout(Some(TIMEOUT))
                        .map_err(|_| X11Error::NoDisplay)?;
                    stream
                        .set_write_timeout(Some(TIMEOUT))
                        .map_err(|_| X11Error::NoDisplay)?;
                    return handshake(&mut stream, cookie);
                }
            }
            Err(X11Error::NoDisplay)
        }
        #[cfg(unix)]
        X11Endpoint::UnixSocket { path } => {
            let mut stream =
                std::os::unix::net::UnixStream::connect(path).map_err(|_| X11Error::NoDisplay)?;
            stream
                .set_read_timeout(Some(TIMEOUT))
                .map_err(|_| X11Error::NoDisplay)?;
            stream
                .set_write_timeout(Some(TIMEOUT))
                .map_err(|_| X11Error::NoDisplay)?;
            handshake(&mut stream, cookie)
        }
        #[cfg(not(unix))]
        X11Endpoint::UnixSocket { .. } => Err(X11Error::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use crate::system::{cookie, endpoint, handshake, probe};
    use nyaterm_core::x11::{X11Connection, X11Endpoint};
    #[test]
    fn conflicting_authorities_are_not_guessed() {
        assert!(cookie(b"host:0 MIT-MAGIC-COOKIE-1 00000000000000000000000000000000\nhost:0 MIT-MAGIC-COOKIE-1 11111111111111111111111111111111").is_none());
        assert!(cookie(b"host:0 MIT-MAGIC-COOKIE-1 invalid").is_none());
    }
    #[test]
    fn external_port_syntax_is_preserved() {
        #[cfg(not(unix))]
        assert_eq!(
            endpoint(":6000").unwrap(),
            X11Endpoint::Tcp {
                host: "localhost".into(),
                port: 12000
            }
        );
        assert_eq!(
            endpoint("localhost:6001").unwrap(),
            X11Endpoint::Tcp {
                host: "localhost".into(),
                port: 6001
            }
        );
    }
    #[test]
    fn truncated_setup_reply_is_not_ready() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        assert!(handshake(&mut bytes, None).is_err());
    }

    #[test]
    fn readiness_requires_a_complete_authenticated_setup_reply() {
        use std::io::{Read, Write};
        use std::net::{Ipv4Addr, TcpListener};
        for success in [false, true] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut setup = [0; 48];
                stream.read_exact(&mut setup).unwrap();
                assert_eq!(&setup[12..30], b"MIT-MAGIC-COOKIE-1");
                assert_eq!(&setup[32..48], &[7; 16]);
                let mut reply = [0; 40];
                reply[0] = u8::from(success);
                reply[2] = 11;
                reply[6] = 8;
                stream.write_all(&reply).unwrap();
            });
            let connection = X11Connection {
                endpoint: X11Endpoint::Tcp {
                    host: "127.0.0.1".into(),
                    port,
                },
                cookie: Some(zeroize::Zeroizing::new(vec![7; 16])),
                lifetime: None,
            };
            assert_eq!(probe(&connection).is_ok(), success);
            server.join().unwrap();
        }
    }

    #[test]
    #[cfg(unix)]
    fn missing_local_socket_preserves_the_legacy_tcp_fallback() {
        use crate::system::probe_external;
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let display = port
            .checked_sub(6000)
            .expect("ephemeral port above X11 range start");
        let root = tempfile::tempdir().unwrap();
        let mut connection = X11Connection {
            endpoint: X11Endpoint::UnixSocket {
                path: root.path().join("missing-socket"),
            },
            cookie: None,
            lifetime: None,
        };
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut setup = [0; 12];
            stream.read_exact(&mut setup).unwrap();
            assert_eq!(&setup[6..10], &[0; 4]);
            let mut reply = [0; 40];
            reply[0] = 1;
            reply[2] = 11;
            reply[6] = 8;
            stream.write_all(&reply).unwrap();
        });
        probe_external(&mut connection, &format!(":{display}")).unwrap();
        assert_eq!(
            connection.endpoint,
            X11Endpoint::Tcp {
                host: "localhost".into(),
                port
            }
        );
        server.join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn cancelled_probe_reaps_the_process_group_and_pipe_reader() {
        use crate::system::output;
        use nyaterm_core::x11::X11Error;
        let started = std::time::Instant::now();
        assert_eq!(
            output(
                std::process::Command::new("/bin/sh").args(["-c", "sleep 30"]),
                &|| true
            ),
            Err(X11Error::Cancelled)
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
