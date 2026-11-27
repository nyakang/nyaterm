#![cfg(unix)]
use nyaterm_core::x11::{X11Connection, X11Endpoint};
use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use zeroize::Zeroizing;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires the system Xvfb executable"]
fn real_x_server_accepts_only_the_matching_authority() {
    let mut authority = tempfile::NamedTempFile::new().unwrap();
    authority.write_all(&65535u16.to_be_bytes()).unwrap();
    for field in [
        &[][..],
        &b"0"[..],
        &b"MIT-MAGIC-COOKIE-1"[..],
        &[7u8; 16][..],
    ] {
        authority
            .write_all(&(field.len() as u16).to_be_bytes())
            .unwrap();
        authority.write_all(field).unwrap();
    }
    authority.flush().unwrap();
    let mut server = Server(
        Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "800x600x24",
                "-nolisten",
                "tcp",
                "-auth",
            ])
            .arg(authority.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let output = server.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut display = String::new();
        let _ = std::io::BufReader::new(output).read_line(&mut display);
        let _ = sender.send(display);
    });
    let display: u16 = receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reader.join().unwrap();
    let mut connection = X11Connection {
        endpoint: X11Endpoint::UnixSocket {
            path: format!("/tmp/.X11-unix/X{display}").into(),
        },
        cookie: Some(Zeroizing::new(vec![7; 16])),
        lifetime: None,
    };
    assert!(nyaterm_x11::probe(&connection).is_ok());
    connection.cookie = Some(Zeroizing::new(vec![8; 16]));
    assert!(nyaterm_x11::probe(&connection).is_err());
    connection.cookie = None;
    assert!(nyaterm_x11::probe(&connection).is_err());
}
