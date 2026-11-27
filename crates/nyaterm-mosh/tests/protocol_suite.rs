//! Integration-style tests against the public `nyaterm_mosh` API.
//!
//! Cases adapted from:
//! - unixshells/mosh-go (`ocb_test`, `fragment_test`, `pb_test`, `transport_test`)
//! - RFC 7253 Appendix A (AES-128-OCB empty AAD)
//! - End-to-end client path exercising shipped modules only

use nyaterm_mosh::client::Client;
use nyaterm_mosh::crypto::{
    DIR_TO_CLIENT, DIR_TO_SERVER, MIN_DATAGRAM, Ocb, SEQ_MASK, pack_timestamps, unpack_timestamps,
};
use nyaterm_mosh::fragment::{Assembler, Fragment, MAX_FRAGMENT_PAYLOAD, fragmentize};
use nyaterm_mosh::pb::{HostInstruction, TransportInstruction, UserInstruction};
use nyaterm_mosh::terminal::{TerminalView, strip_ansi};
use nyaterm_mosh::transport::Transport;

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Crypto (public API)
// ---------------------------------------------------------------------------

#[test]
fn public_ocb_seal_open_roundtrip() {
    let ocb = Ocb::from_base64("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let dir = DIR_TO_CLIENT | 7;
    let mut pt = Vec::new();
    pt.extend_from_slice(&pack_timestamps(1, 2));
    pt.extend_from_slice(b"payload");
    let wire = ocb.seal_datagram(dir, &pt);
    assert!(wire.len() >= MIN_DATAGRAM);
    let (rx, out) = ocb.open_datagram(&wire).unwrap();
    assert_eq!(rx & SEQ_MASK, 7);
    assert_eq!(out, pt);
    let (ts, reply, body) = unpack_timestamps(&out).unwrap();
    assert_eq!((ts, reply, body), (1, 2, &b"payload"[..]));
}

#[test]
fn public_ocb_rejects_tamper() {
    let ocb = Ocb::new(&[3u8; 16]).unwrap();
    let mut wire = ocb.seal_datagram(DIR_TO_SERVER | 1, b"x");
    let last = wire.len() - 1;
    wire[last] ^= 0xff;
    assert!(ocb.open_datagram(&wire).is_none());
}

// ---------------------------------------------------------------------------
// Fragments
// ---------------------------------------------------------------------------

#[test]
fn public_fragment_out_of_order_reassembly() {
    let data: Vec<u8> = (0..MAX_FRAGMENT_PAYLOAD * 2 + 50)
        .map(|i| (i % 251) as u8)
        .collect();
    let frags = fragmentize(11, &data);
    assert!(frags.len() >= 3);
    let mut a = Assembler::new();
    // reverse order
    let mut result = None;
    for f in frags.into_iter().rev() {
        if let Some(r) = a.add(f) {
            result = Some(r);
        }
    }
    assert_eq!(result.unwrap(), data);
}

#[test]
fn public_fragment_header_layout() {
    let f = Fragment {
        id: 0x0102_0304_0506_0708,
        fragment_num: 0x1234,
        is_final: true,
        payload: b"ab".to_vec(),
    };
    let w = f.encode();
    assert_eq!(
        u64::from_be_bytes(w[0..8].try_into().unwrap()),
        0x0102_0304_0506_0708
    );
    let nf = u16::from_be_bytes([w[8], w[9]]);
    assert_eq!(nf & 0x7fff, 0x1234);
    assert_ne!(nf & 0x8000, 0);
    assert_eq!(&w[10..], b"ab");
}

// ---------------------------------------------------------------------------
// Protobuf
// ---------------------------------------------------------------------------

#[test]
fn public_transport_instruction_chaff_and_diff() {
    let ti = TransportInstruction {
        protocol_version: 2,
        old_num: 1,
        new_num: 2,
        ack_num: 1,
        throwaway_num: 0,
        diff: b"diff-bytes".to_vec(),
        chaff: vec![0, 1, 2, 3],
    };
    assert_eq!(TransportInstruction::decode(&ti.encode()).unwrap(), ti);
}

#[test]
fn public_user_host_message_interop_shape() {
    let user = UserInstruction::encode_message(&[
        UserInstruction::keystroke(b"echo hi\n"),
        UserInstruction::resize(80, 24),
    ]);
    let decoded = UserInstruction::decode_message(&user).unwrap();
    assert_eq!(decoded[0].keys, b"echo hi\n");
    assert_eq!((decoded[1].width, decoded[1].height), (80, 24));

    let host = HostInstruction::encode_message(&[HostInstruction {
        hoststring: b"\x1b[Hhi\r\n".to_vec(),
        echo_ack_num: -1,
        ..Default::default()
    }]);
    let h = HostInstruction::decode_message(&host).unwrap();
    assert!(h[0].hoststring.starts_with(b"\x1b[H"));
}

// ---------------------------------------------------------------------------
// Transport SSP
// ---------------------------------------------------------------------------

fn transport_pair() -> (Transport, Transport) {
    let key = [0xABu8; 16];
    (
        Transport::new_server(Ocb::new(&key).unwrap()),
        Transport::new_client(Ocb::new(&key).unwrap()),
    )
}

#[test]
fn public_ssp_basic_and_replay() {
    let (mut server, mut client) = transport_pair();
    server.set_pending(b"hello".to_vec());
    let dgs = server.tick();
    let mut got = None;
    for dg in &dgs {
        if let Some(d) = client.recv(dg) {
            got = Some(d);
        }
    }
    assert_eq!(got.unwrap(), b"hello");
    // replay
    assert!(client.recv(&dgs[0]).is_none());
}

#[test]
fn public_ssp_bidirectional_clean() {
    let (mut server, mut client) = transport_pair();
    for i in 0..10u8 {
        let sp = format!("s-{i}").into_bytes();
        server.set_pending(sp.clone());
        let mut got = None;
        for dg in server.tick() {
            if let Some(d) = client.recv(&dg) {
                got = Some(d);
            }
        }
        assert_eq!(got.unwrap(), sp);

        let cp = format!("c-{i}").into_bytes();
        client.set_pending(cp.clone());
        let mut got = None;
        for dg in client.tick() {
            if let Some(d) = server.recv(&dg) {
                got = Some(d);
            }
        }
        assert_eq!(got.unwrap(), cp);
    }
}

#[test]
fn public_ssp_wrong_direction() {
    let (mut server, _) = transport_pair();
    server.set_pending(b"x".to_vec());
    let dgs = server.tick();
    assert!(server.recv(&dgs[0]).is_none());
}

// ---------------------------------------------------------------------------
// Terminal view
// ---------------------------------------------------------------------------

#[test]
fn public_terminal_strips_and_applies() {
    let mut view = TerminalView::new(80, 24);
    let msg = HostInstruction::encode_message(&[HostInstruction {
        hoststring: b"\x1b[31mred\x1b[0mOK".to_vec(),
        echo_ack_num: -1,
        ..Default::default()
    }]);
    let paint = view.apply_host_diff(&msg);
    assert_eq!(strip_ansi(std::str::from_utf8(&paint).unwrap()), "redOK");
}

// ---------------------------------------------------------------------------
// Client + fake server (shipped Client::dial path)
// ---------------------------------------------------------------------------

fn spawn_fake_mosh_server(
    marker: &'static str,
) -> (u16, String, thread::JoinHandle<()>, Arc<Mutex<bool>>) {
    let key = [0x5Au8; 16];
    let key_b64 = {
        use base64::Engine;
        let s = base64::engine::general_purpose::STANDARD.encode(key);
        s.trim_end_matches('=').to_string()
    };
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = sock.local_addr().unwrap().port();
    sock.set_read_timeout(Some(Duration::from_millis(40)))
        .unwrap();
    let done = Arc::new(Mutex::new(false));
    let done2 = done.clone();

    let handle = thread::spawn(move || {
        let mut transport = Transport::new_server(Ocb::new(&key).unwrap());
        let mut client_addr = None;
        let mut sent_banner = false;
        let mut buf = [0u8; 4096];
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(6) {
            if *done2.lock().unwrap() {
                break;
            }
            if let Ok((n, addr)) = sock.recv_from(&mut buf) {
                client_addr = Some(addr);
                if let Some(diff) = transport.recv(&buf[..n])
                    && !diff.is_empty()
                {
                    let host = HostInstruction::encode_message(&[HostInstruction {
                        hoststring: format!("\x1b[H\x1b[2J$ echo {marker}\r\n{marker}\r\n$ ")
                            .into_bytes(),
                        echo_ack_num: -1,
                        ..Default::default()
                    }]);
                    transport.set_pending(host);
                }
                if !sent_banner {
                    let host = HostInstruction::encode_message(&[HostInstruction {
                        hoststring: b"\x1b[H\x1b[2J$ ".to_vec(),
                        echo_ack_num: -1,
                        ..Default::default()
                    }]);
                    transport.set_pending(host);
                    sent_banner = true;
                }
            }
            if let Some(addr) = client_addr {
                for dg in transport.tick() {
                    let _ = sock.send_to(&dg, addr);
                }
            }
            thread::sleep(Duration::from_millis(4));
        }
    });
    thread::sleep(Duration::from_millis(20));
    (port, key_b64, handle, done)
}

#[test]
fn public_client_command_echo_marker() {
    let marker = "PROTOCOL_SUITE_MARKER_OK";
    let (port, key, handle, done) = spawn_fake_mosh_server(marker);
    let mut client = Client::dial("127.0.0.1", port, &key).expect("dial");

    let mut saw = false;
    for _ in 0..80 {
        if !client.poll().unwrap().is_empty() {
            saw = true;
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    assert!(saw, "banner");

    client.send_keys(format!("echo {marker}\n").as_bytes());
    let mut all = String::new();
    for _ in 0..120 {
        let out = client.poll().unwrap();
        if !out.is_empty() {
            all.push_str(&String::from_utf8_lossy(&out));
        }
        if strip_ansi(&all).contains(marker) {
            *done.lock().unwrap() = true;
            let _ = handle.join();
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
    *done.lock().unwrap() = true;
    let _ = handle.join();
    panic!("marker missing: {all:?}");
}

#[test]
fn public_client_resize_does_not_panic() {
    let marker = "RESIZE_ONLY";
    let (port, key, handle, done) = spawn_fake_mosh_server(marker);
    let mut client = Client::dial("127.0.0.1", port, &key).unwrap();
    client.resize(120, 40);
    for _ in 0..20 {
        let _ = client.poll();
        thread::sleep(Duration::from_millis(10));
    }
    *done.lock().unwrap() = true;
    let _ = handle.join();
}
