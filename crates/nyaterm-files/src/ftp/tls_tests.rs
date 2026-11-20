//! Local TLS peers use fresh in-memory keys and never change system trust.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyaterm_core::character_encoding::CharacterEncoding;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use suppaftp::NativeTlsConnector;
use suppaftp::native_tls::{Certificate, Protocol, TlsConnector};

use crate::ftp::{FtpConfig, FtpFileService, FtpLogin, FtpSecurityMode};

enum Socket {
    Plain(TcpStream),
    Secure(Box<StreamOwned<ServerConnection, TcpStream>>),
}
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(bytes),
            Self::Secure(s) => s.read(bytes),
        }
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(bytes),
            Self::Secure(s) => s.write(bytes),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Secure(s) => s.flush(),
        }
    }
}
fn secure(socket: TcpStream, config: &Arc<ServerConfig>) -> Socket {
    Socket::Secure(Box::new(StreamOwned::new(
        ServerConnection::new(config.clone()).unwrap(),
        socket,
    )))
}
struct Peer {
    config: FtpConfig,
    certificate: Vec<u8>,
    commands: Arc<Mutex<Vec<String>>>,
    content: Arc<Mutex<Vec<u8>>>,
}
impl Peer {
    fn new(mode: FtpSecurityMode, name: &str) -> Self {
        let key = rcgen::generate_simple_self_signed(vec![name.into()]).unwrap();
        let certificate = key.cert.der().to_vec();
        let tls = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_protocol_versions(&[&rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![key.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()).into(),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = FtpConfig::new("localhost");
        config.port = listener.local_addr().unwrap().port();
        config.security_mode = mode;
        let commands = Arc::new(Mutex::new(Vec::new()));
        let content = Arc::new(Mutex::new(b"initial".to_vec()));
        let log = commands.clone();
        let body = content.clone();
        std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let socket = if mode == FtpSecurityMode::Implicit {
                secure(socket, &tls)
            } else {
                Socket::Plain(socket)
            };
            let mut control = BufReader::new(socket);
            if control.get_mut().write_all(b"220 TLS fixture\r\n").is_err() {
                return;
            }
            let mut passive = None;
            let mut protected = false;
            loop {
                let mut line = String::new();
                if !matches!(control.read_line(&mut line), Ok(n) if n > 0) {
                    return;
                }
                let command = line.trim_end().to_string();
                // Do not retain login arguments even in the fixture's command log.
                let verb = command.split(' ').next().unwrap();
                log.lock()
                    .unwrap()
                    .push(if matches!(verb, "PASS" | "USER") {
                        verb.into()
                    } else {
                        command.clone()
                    });
                let reply: &[u8] = match verb {
                    "AUTH" => {
                        control.get_mut().write_all(b"234 start TLS\r\n").unwrap();
                        let Socket::Plain(socket) = control.into_inner() else {
                            return;
                        };
                        control = BufReader::new(secure(socket, &tls));
                        continue;
                    }
                    "PBSZ" => b"200 buffer\r\n",
                    "PROT" => {
                        protected = command == "PROT P";
                        b"200 protection\r\n"
                    }
                    "USER" => b"331 password\r\n",
                    "PASS" => b"230 accepted\r\n",
                    "FEAT" => b"211-Features\r\n UTF8\r\n211 End\r\n",
                    "TYPE" | "OPTS" => b"200 ok\r\n",
                    "PWD" => b"257 \"/\"\r\n",
                    "MLST" => b"250-File\r\n type=file;size=7; /file\r\n250 End\r\n",
                    "RNFR" => b"350 ready\r\n",
                    "RNTO" | "DELE" => b"250 ok\r\n",
                    "PASV" => {
                        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                        let port = listener.local_addr().unwrap().port();
                        control
                            .get_mut()
                            .write_all(
                                format!(
                                    "227 Passive (127,0,0,1,{},{})\r\n",
                                    port / 256,
                                    port % 256
                                )
                                .as_bytes(),
                            )
                            .unwrap();
                        passive = Some(listener);
                        continue;
                    }
                    "RETR" | "STOR" => {
                        assert!(protected, "FTPS data must be private before transfer");
                        control.get_mut().write_all(b"150 data\r\n").unwrap();
                        let (socket, _) = passive.take().unwrap().accept().unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let Socket::Secure(mut data) = secure(socket, &tls) else {
                            unreachable!()
                        };
                        if verb == "RETR" {
                            data.write_all(&body.lock().unwrap()).unwrap();
                        } else {
                            let mut bytes = Vec::new();
                            if data.read_to_end(&mut bytes).is_err() {
                                return;
                            }
                            *body.lock().unwrap() = bytes;
                        }
                        data.conn.send_close_notify();
                        let _ = data.flush();
                        drop(data);
                        b"226 complete\r\n"
                    }
                    _ => b"500 unsupported\r\n",
                };
                if control.get_mut().write_all(reply).is_err() {
                    return;
                }
            }
        });
        Self {
            config,
            certificate,
            commands,
            content,
        }
    }
    fn connector(&self) -> NativeTlsConnector {
        TlsConnector::builder()
            .add_root_certificate(Certificate::from_der(&self.certificate).unwrap())
            .build()
            .unwrap()
            .into()
    }
    fn connect(&self) -> Result<FtpFileService, crate::ftp::FtpError> {
        FtpFileService::connect_with_tls(
            self.config.clone(),
            FtpLogin::anonymous(""),
            CharacterEncoding::Utf8,
            || false,
            Some(self.connector()),
        )
    }
}

#[test]
fn explicit_and_implicit_ftps_encrypt_upload_and_download_after_prot_p() {
    for mode in [FtpSecurityMode::Explicit, FtpSecurityMode::Implicit] {
        let peer = Peer::new(mode, "localhost");
        let service = peer.connect().unwrap();
        service.write_file("/file", b"encrypted body").unwrap();
        assert_eq!(*peer.content.lock().unwrap(), b"encrypted body");
        assert_eq!(
            service.read_file_bytes_limited("/file", 100).unwrap(),
            b"encrypted body"
        );
        let commands = peer.commands.lock().unwrap();
        let protection = commands.iter().position(|c| c == "PROT P").unwrap();
        let transfer = commands
            .iter()
            .position(|c| c.starts_with("STOR "))
            .unwrap();
        assert!(protection < transfer);
    }
}

#[test]
fn ftps_rejects_untrusted_certificates_and_trusted_certificates_for_another_host() {
    for mode in [FtpSecurityMode::Explicit, FtpSecurityMode::Implicit] {
        let peer = Peer::new(mode, "localhost");
        assert!(FtpFileService::connect(peer.config.clone(), FtpLogin::anonymous("")).is_err());
        let peer = Peer::new(mode, "different-host.invalid");
        assert!(peer.connect().is_err());
    }
}

#[test]
fn pinned_tls_does_not_fall_back_to_another_protocol_version() {
    let peer = Peer::new(FtpSecurityMode::Implicit, "localhost");
    let connector = TlsConnector::builder()
        .add_root_certificate(Certificate::from_der(&peer.certificate).unwrap())
        .min_protocol_version(Some(Protocol::Tlsv13))
        .max_protocol_version(Some(Protocol::Tlsv13))
        .build();
    if let Ok(connector) = connector {
        assert!(
            FtpFileService::connect_with_tls(
                peer.config.clone(),
                FtpLogin::anonymous(""),
                CharacterEncoding::Utf8,
                || false,
                Some(connector.into())
            )
            .is_err()
        );
    }
}
