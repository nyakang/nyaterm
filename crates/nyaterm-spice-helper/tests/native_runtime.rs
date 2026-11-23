//! Runs against the actual bundled library, without a VM or external server.
//! NYATERM_SPICE_RUNTIME must point to a complete runtime directory.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use nyaterm_core::spice::settings::{SpiceEndpoint, SpiceSettings};
use nyaterm_remote_desktop::spice::protocol::{
    SpiceControl, SpiceCredentialLifetime, SpiceSessionConfig,
};
use nyaterm_spice_helper::native::{NativeClient, NativeEvent, NativeRuntime};

fn native_child(name: &str) -> bool {
    if std::env::var("NYATERM_NATIVE_TEST_CHILD").ok().as_deref() == Some(name) {
        return true;
    }
    // libtest creates a new OS thread per test even with --test-threads=1.
    // The native coroutine leader is process-global and cannot migrate threads.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            name,
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("GST_REGISTRY_DISABLE", "yes")
        .env("GST_REGISTRY_FORK", "no")
        .env("GST_PLUGIN_PATH", "")
        .env("GST_PLUGIN_PATH_1_0", "")
        .env("GST_PLUGIN_SYSTEM_PATH", "")
        .env("GST_PLUGIN_SYSTEM_PATH_1_0", "")
        .env("NYATERM_NATIVE_TEST_CHILD", name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native child exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "native child failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    false
}

#[test]
#[ignore = "requires a prepared native SPICE runtime"]
fn native_abi_allocates_a_session_with_device_redirection_disabled() {
    if !native_child("native_abi_allocates_a_session_with_device_redirection_disabled") {
        return;
    }
    let runtime = NativeRuntime::load().unwrap();
    let result = runtime.probe().unwrap();
    assert!(result.session_created);
    assert!(result.device_redirection_disabled);
}

#[test]
#[ignore = "requires a prepared native SPICE runtime"]
fn native_context_drives_tcp_handshake_and_reports_malformed_server_without_hanging() {
    if !native_child(
        "native_context_drives_tcp_handshake_and_reports_malformed_server_without_hanging",
    ) {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut header = [0u8; 16];
                    stream.read_exact(&mut header).unwrap();
                    assert_eq!(&header[..4], b"REDQ");
                    // Invalid link header exercises the native decoder's error path.
                    let _ = stream.write_all(&[0; 16]);
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("fixture listener failed: {error}"),
            }
        }
    });
    let runtime = NativeRuntime::load().unwrap();
    let mut settings = SpiceSettings::default();
    settings.security.require_tls = false;
    settings.audio_playback = false;
    let config = SpiceSessionConfig {
        session_id: "native-loopback-test".into(),
        endpoint: SpiceEndpoint {
            extra: Default::default(),
            host: "127.0.0.1".into(),
            port: Some(port),
            tls_port: None,
            proxy: None,
        },
        username: "".into(),
        password: "fixture-only".into(),
        settings,
        credential_lifetime: SpiceCredentialLifetime::Temporary,
        tcp_relay: None,
        tls_relay: None,
    };
    let mut client = NativeClient::connect(&runtime, config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_error = false;
    while Instant::now() < deadline {
        if let Some(NativeEvent::Control(SpiceControl::Error { fatal: true, .. })) = client.poll() {
            saw_error = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(client);
    assert!(
        server.join().unwrap(),
        "native socket never reached the loopback server"
    );
    assert!(
        saw_error,
        "native protocol failure was not reported within the deadline"
    );
}

#[test]
#[ignore = "requires a prepared native SPICE runtime"]
fn authenticated_relay_preserves_tls_and_never_sends_plaintext_to_tls_destination() {
    if !native_child(
        "authenticated_relay_preserves_tls_and_never_sends_plaintext_to_tls_destination",
    ) {
        return;
    }
    use nyaterm_core::connection_route::RelayEndpoint;
    for (tls, proxy) in [(false, false), (true, false), (false, true), (true, true)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let token = "a".repeat(32);
        let expected = token.clone();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut received = [0u8; 32];
                        stream.read_exact(&mut received).unwrap();
                        assert_eq!(received.as_slice(), expected.as_bytes());
                        if proxy {
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") {
                                assert!(request.len() < 8192);
                                let mut byte = [0];
                                stream.read_exact(&mut byte).unwrap();
                                request.push(byte[0]);
                            }
                            let request = std::str::from_utf8(&request).unwrap();
                            assert!(request.starts_with(if tls {
                                "CONNECT pvespiceproxy:fixture-ticket:101:pve:5901 HTTP/1.1"
                            } else {
                                "CONNECT pvespiceproxy:fixture-ticket:101:pve:5900 HTTP/1.1"
                            }));
                            assert!(
                                request.contains("Proxy-Authorization: Basic dXNlcjpzZWNyZXQ=")
                            );
                            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
                        }
                        let mut header = [0u8; 5];
                        stream.read_exact(&mut header).unwrap();
                        if tls {
                            assert_eq!(header[0], 0x16, "expected a TLS handshake record");
                            assert_eq!(header[1], 3);
                        } else {
                            assert_eq!(&header[..4], b"REDQ");
                        }
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "authenticated relay was not used"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("relay fixture: {error}"),
                }
            }
        });
        let mut settings = SpiceSettings::default();
        settings.security.require_tls = tls;
        settings.audio_playback = false;
        if tls {
            let generated = rcgen::generate_simple_self_signed(vec!["vm.invalid".into()]).unwrap();
            settings.security.ca_pem = Some(generated.cert.pem());
        }
        let relay = RelayEndpoint {
            address,
            token: token.into(),
        };
        let config = SpiceSessionConfig {
            session_id: "relay-fixture".into(),
            endpoint: SpiceEndpoint {
                extra: Default::default(),
                host: if proxy {
                    "pvespiceproxy:fixture-ticket:101:pve"
                } else {
                    "vm.invalid"
                }
                .into(),
                port: (!tls).then_some(5900),
                tls_port: tls.then_some(5901),
                proxy: proxy.then(|| "http://user:secret@proxy.invalid:3128".into()),
            },
            username: "".into(),
            password: "fixture".into(),
            settings,
            credential_lifetime: SpiceCredentialLifetime::Temporary,
            tcp_relay: (!tls).then_some(relay.clone()),
            tls_relay: tls.then_some(relay),
        };
        let runtime = NativeRuntime::load().unwrap();
        let mut client = NativeClient::connect(&runtime, config).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !client.failed() {
            let _ = client.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(client);
        server.join().unwrap();
    }
}

#[test]
#[ignore = "requires a prepared native SPICE runtime"]
fn another_thread_cannot_reinitialize_the_process_native_runtime() {
    if !native_child("another_thread_cannot_reinitialize_the_process_native_runtime") {
        return;
    }
    let runtime = NativeRuntime::load().unwrap();
    drop(runtime);
    let error = std::thread::spawn(|| NativeRuntime::load().err())
        .join()
        .unwrap();
    assert_eq!(
        error,
        Some(nyaterm_spice_helper::native::NativeError::ContextBusy)
    );
}

#[test]
#[ignore = "requires a prepared native SPICE runtime"]
fn relayed_tls_checks_certificate_authority_and_original_hostname() {
    if !native_child("relayed_tls_checks_certificate_authority_and_original_hostname") {
        return;
    }
    use nyaterm_core::connection_route::RelayEndpoint;
    use nyaterm_remote_desktop::spice::protocol::SpiceErrorKind;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::sync::Arc;

    for (trusted, correct_name, subject) in [
        (true, true, None),
        (false, true, None),
        (true, false, None),
        (true, false, Some("CN=rcgen self signed cert")),
        (true, false, Some("CN=wrong subject")),
    ] {
        let accepted = trusted
            && (subject == Some("CN=rcgen self signed cert")
                || (subject.is_none() && correct_name));
        let certificate = rcgen::generate_simple_self_signed(vec!["vm.invalid".into()]).unwrap();
        let ca = if trusted {
            certificate.cert.pem()
        } else {
            rcgen::generate_simple_self_signed(vec!["unrelated.invalid".into()])
                .unwrap()
                .cert
                .pem()
        };
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                certificate.signing_key.serialize_der(),
            )),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("TLS fixture: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut token = [0; 32];
            stream.read_exact(&mut token).unwrap();
            assert_eq!(token, [b'b'; 32]);
            let connection = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, stream);
            let mut magic = [0; 4];
            let linked = tls.read_exact(&mut magic).is_ok();
            if linked {
                assert_eq!(&magic, b"REDQ");
                let _ = tls.write_all(&[0; 16]);
                let _ = tls.flush();
            }
            linked
        });
        let mut settings = SpiceSettings {
            audio_playback: false,
            ..SpiceSettings::default()
        };
        settings.security.ca_pem = Some(ca);
        settings.security.certificate_subject = subject.map(str::to_owned);
        let config = SpiceSessionConfig {
            session_id: "tls-certificate-fixture".into(),
            endpoint: SpiceEndpoint {
                extra: Default::default(),
                host: if correct_name {
                    "vm.invalid"
                } else {
                    "wrong.invalid"
                }
                .into(),
                port: None,
                tls_port: Some(5901),
                proxy: None,
            },
            username: "".into(),
            password: "fixture".into(),
            settings,
            credential_lifetime: SpiceCredentialLifetime::Temporary,
            tcp_relay: None,
            tls_relay: Some(RelayEndpoint {
                address,
                token: "b".repeat(32).into(),
            }),
        };
        let runtime = NativeRuntime::load().unwrap();
        let mut client = NativeClient::connect(&runtime, config).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut error = None;
        while Instant::now() < deadline {
            if let Some(NativeEvent::Control(SpiceControl::Error {
                error: kind,
                fatal: true,
            })) = client.poll()
            {
                error = Some(kind);
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(client);
        assert_eq!(server.join().unwrap(), accepted);
        if !accepted {
            assert_eq!(error, Some(SpiceErrorKind::Certificate));
        } else {
            assert!(error.is_some());
        }
    }
}
