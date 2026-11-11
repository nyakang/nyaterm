use nyaterm_android::adb::client::DeviceBinding;
use nyaterm_android::ipc::{Control, PROTOCOL_VERSION, Packet, read_packet, write_packet};
use nyaterm_android::process::reap;
use nyaterm_core::models::android::AndroidScreenSettings;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn spawn() -> Child {
    Command::new(env!("CARGO_BIN_EXE_nyaterm-android-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}
fn hello(child: &mut Child) {
    assert!(matches!(
        read_packet(child.stdout.as_mut().unwrap()).unwrap(),
        Some(Packet::Control(Control::Hello {
            protocol: PROTOCOL_VERSION,
            ..
        }))
    ));
}
#[test]
fn handshake_control_input_idle_video_heartbeat_and_normal_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket.write_all(&[0]).unwrap();
        socket.write_all(&[0; 64]).unwrap();
        socket.write_all(b"h264").unwrap();
        socket
            .write_all(&[0x80, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 8])
            .unwrap();
        let (mut control, _) = listener.accept().unwrap();
        control
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut key = [0; 14];
        control.read_exact(&mut key).unwrap();
        assert_eq!(key, [0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
        // Disconnect releases held keys through the always-enabled control channel.
        control.read_exact(&mut key).unwrap();
        assert_eq!(key, [0, 1, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
        let _ = socket.read(&mut [0; 1]);
    });
    let mut child = spawn();
    hello(&mut child);
    let binding = DeviceBinding {
        server: "127.0.0.1:5037".parse().unwrap(),
        serial: "synthetic".into(),
        transport_id: None,
        generation: 1,
    };
    write_packet(
        child.stdin.as_mut().unwrap(),
        &Packet::Control(Control::Connect {
            binding,
            endpoint,
            settings: AndroidScreenSettings::default(),
        }),
    )
    .unwrap();
    assert!(matches!(
        read_packet(child.stdout.as_mut().unwrap()).unwrap(),
        Some(Packet::Control(Control::Ready {
            width: 8,
            height: 8,
            ..
        }))
    ));
    write_packet(
        child.stdin.as_mut().unwrap(),
        &Packet::Control(Control::Ping { sequence: 17 }),
    )
    .unwrap();
    assert!(matches!(
        read_packet(child.stdout.as_mut().unwrap()).unwrap(),
        Some(Packet::Control(Control::Pong { sequence: 17 }))
    ));
    // Recoverable paste errors must leave media and heartbeat alive.
    for text in [
        "private\0paste".into(),
        "x".repeat(nyaterm_android::ipc::MAX_TEXT_BYTES + 1),
    ] {
        write_packet(
            child.stdin.as_mut().unwrap(),
            &Packet::Control(Control::Paste { text, sequence: 19 }),
        )
        .unwrap();
        assert!(matches!(
            read_packet(child.stdout.as_mut().unwrap()).unwrap(),
            Some(Packet::Control(Control::InputRejected { .. }))
        ));
    }
    for visible in [false, true] {
        write_packet(
            child.stdin.as_mut().unwrap(),
            &Packet::Control(Control::Presentation { visible }),
        )
        .unwrap();
    }
    write_packet(
        child.stdin.as_mut().unwrap(),
        &Packet::Control(Control::Ping { sequence: 20 }),
    )
    .unwrap();
    assert!(matches!(
        read_packet(child.stdout.as_mut().unwrap()).unwrap(),
        Some(Packet::Control(Control::Pong { sequence: 20 }))
    ));
    write_packet(
        child.stdin.as_mut().unwrap(),
        &Packet::Control(Control::Key {
            action: 0,
            keycode: 4,
            repeat: 0,
            metastate: 0,
        }),
    )
    .unwrap();
    write_packet(
        child.stdin.as_mut().unwrap(),
        &Packet::Control(Control::Disconnect),
    )
    .unwrap();
    assert!(reap(&mut child, Duration::from_secs(3)).unwrap().success());
    server.join().unwrap();
}
#[test]
fn invalid_initial_packet_reports_fatal_and_reaps() {
    let mut child = spawn();
    hello(&mut child);
    write_packet(
        child.stdin.as_mut().unwrap(),
        &Packet::Control(Control::Rotate),
    )
    .unwrap();
    assert!(matches!(
        read_packet(child.stdout.as_mut().unwrap()).unwrap(),
        Some(Packet::Control(Control::Fatal { .. }))
    ));
    assert!(!reap(&mut child, Duration::from_secs(2)).unwrap().success());
}
#[test]
fn hung_startup_is_forcibly_reaped_within_grace() {
    let mut child = spawn();
    hello(&mut child);
    assert!(
        !reap(&mut child, Duration::from_millis(20))
            .unwrap()
            .success()
    );
    assert!(child.try_wait().unwrap().is_some());
}
#[test]
fn crashed_helper_is_waited_without_leaving_a_child() {
    let mut child = spawn();
    hello(&mut child);
    child.kill().unwrap();
    assert!(!reap(&mut child, Duration::from_secs(1)).unwrap().success());
    assert!(child.try_wait().unwrap().is_some());
}
