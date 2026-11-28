//! Live integration against a real mosh-server.
//!
//! Opt-in (never runs in default CI):
//!
//! ```bash
//! MOSH_LIVE_HOST=192.168.139.227 \
//! MOSH_LIVE_USER=root \
//! MOSH_LIVE_SSH_KEY=~/.ssh/id_ed25519 \
//! cargo test --test live_mosh_prediction -- --ignored --nocapture
//! ```
//!
//! Password authentication is also available through `MOSH_LIVE_PASSWORD` and
//! requires `sshpass`. The remote host must have `mosh-server` and be reachable
//! over UDP.

use flate2::read::ZlibDecoder;
use nyaterm_mosh::client::{Client, ConnectionStatus};
use nyaterm_mosh::crypto::Ocb;
use nyaterm_mosh::fragment::Fragment;
use nyaterm_mosh::pb::{HostInstruction, TransportInstruction};
use nyaterm_mosh::prediction::{DisplayPipeline, DisplayPreference};
use nyaterm_mosh::terminal::strip_ansi;
use socket2::SockRef;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn env_or_skip(key: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("skip: set {key} (and MOSH_LIVE_*) to run live mosh tests");
            String::new()
        }
    }
}

struct RemoteServerGuard {
    host: String,
    user: String,
    password: Option<String>,
    ssh_key: Option<String>,
    pid: u32,
    session_id: u32,
    child_pid: u32,
    child_session_id: u32,
}

impl Drop for RemoteServerGuard {
    fn drop(&mut self) {
        let destination = format!("{}@{}", self.user, self.host);
        let remote_cleanup = format!(
            "pkill -TERM -s {} >/dev/null 2>&1 || true; pkill -TERM -s {} >/dev/null 2>&1 || true; kill {} {} >/dev/null 2>&1 || true; rm -f /tmp/nyaterm_mosh-live-{}-*",
            self.session_id, self.child_session_id, self.child_pid, self.pid, self.pid
        );
        let mut command;
        if let Some(key) = self.ssh_key.as_deref() {
            command = Command::new("ssh");
            command.args(["-i", key, "-o", "BatchMode=yes"]);
        } else {
            command = Command::new("sshpass");
            command.args(["-p", self.password.as_deref().unwrap_or_default(), "ssh"]);
        }
        let _ = command
            .args([
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "ConnectTimeout=3",
                &destination,
                &remote_cleanup,
            ])
            .output();
    }
}

impl RemoteServerGuard {
    fn remote_file_exists(&self, path: &str) -> Option<bool> {
        let destination = format!("{}@{}", self.user, self.host);
        let remote_check = format!("if test -f '{path}'; then echo READY; else echo WAITING; fi");
        let mut command;
        if let Some(key) = self.ssh_key.as_deref() {
            command = Command::new("ssh");
            command.args(["-i", key, "-o", "BatchMode=yes"]);
        } else {
            command = Command::new("sshpass");
            command.args(["-p", self.password.as_deref().unwrap_or_default(), "ssh"]);
        }
        let output = command
            .args([
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "ConnectTimeout=3",
                &destination,
                &remote_check,
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        match String::from_utf8_lossy(&output.stdout).trim() {
            "READY" => Some(true),
            "WAITING" => Some(false),
            _ => None,
        }
    }

    fn is_running(&self) -> Option<bool> {
        let destination = format!("{}@{}", self.user, self.host);
        let remote_check = format!(
            "if kill -0 {} >/dev/null 2>&1 || kill -0 {} >/dev/null 2>&1 || ps -eo sid= | awk -v server_sid={} -v child_sid={} '$1 == server_sid || $1 == child_sid {{ found=1 }} END {{ exit(found ? 0 : 1) }}'; then echo RUNNING; else echo STOPPED; fi",
            self.pid, self.child_pid, self.session_id, self.child_session_id
        );
        let mut command;
        if let Some(key) = self.ssh_key.as_deref() {
            command = Command::new("ssh");
            command.args(["-i", key, "-o", "BatchMode=yes"]);
        } else {
            command = Command::new("sshpass");
            command.args(["-p", self.password.as_deref().unwrap_or_default(), "ssh"]);
        }
        let output = command
            .args([
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "ConnectTimeout=3",
                &destination,
                &remote_check,
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        match String::from_utf8_lossy(&output.stdout).trim() {
            "RUNNING" => Some(true),
            "STOPPED" => Some(false),
            _ => None,
        }
    }
}

fn assert_graceful_shutdown(client: &mut Client, server: &RemoteServerGuard, label: &str) {
    let acknowledged = client
        .graceful_shutdown(Duration::from_secs(10))
        .expect("graceful shutdown");
    if acknowledged {
        return;
    }

    // Stock mosh-server sends one final shutdown ACK and then exits. A local
    // VM/network can lose that last UDP datagram after the server closes its
    // socket. In that case verify the requested outcome directly: the remote
    // server and child session must actually be gone.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match server.is_running() {
            Some(false) => return,
            Some(true) => {}
            None => panic!("could not verify remote process state after {label} shutdown"),
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("stock server neither acknowledged nor completed {label} shutdown");
}

/// Start remote mosh-server via SSH; return (port, key).
fn start_remote_mosh_server(
    host: &str,
    user: &str,
    password: Option<&str>,
    ssh_key: Option<&str>,
) -> (u16, String, RemoteServerGuard) {
    let remote_cmd = "mosh-server new -s -p 60000:60100 -- /bin/bash --noprofile --norc -c 'printf \"MCINITIAL_SIZE:%s\\n\" \"$(stty size)\"; export PS1=\"$ \"; exec bash --noprofile --norc'";
    let destination = format!("{user}@{host}");
    let mut command;
    if let Some(key) = ssh_key {
        command = Command::new("ssh");
        command.args(["-i", key, "-o", "BatchMode=yes"]);
    } else {
        command = Command::new("sshpass");
        command.args(["-p", password.expect("password or SSH key"), "ssh"]);
    }
    let output = command
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=8",
            &destination,
            remote_cmd,
        ])
        .output()
        .expect("ssh/sshpass must be available for live tests");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    let detached_pid = combined
        .split("pid = ")
        .nth(1)
        .and_then(|tail| {
            tail.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .ok()
        })
        .unwrap_or_else(|| panic!("no detached mosh-server pid in remote response"));
    let session_query = format!("ps -o sid= -p {detached_pid}");
    let mut session_command;
    if let Some(key) = ssh_key {
        session_command = Command::new("ssh");
        session_command.args(["-i", key, "-o", "BatchMode=yes"]);
    } else {
        session_command = Command::new("sshpass");
        session_command.args(["-p", password.expect("password or SSH key"), "ssh"]);
    }
    let session_output = session_command
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=3",
            &destination,
            &session_query,
        ])
        .output()
        .expect("query detached mosh-server session id");
    assert!(
        session_output.status.success(),
        "could not query detached mosh-server session id"
    );
    let session_id = String::from_utf8_lossy(&session_output.stdout)
        .trim()
        .parse::<u32>()
        .expect("detached mosh-server session id");
    let child_query = format!(
        "child=$(pgrep -P {detached_pid} | head -n 1); if [ -n \"$child\" ]; then printf '%s %s\\n' \"$child\" \"$(ps -o sid= -p \"$child\")\"; fi"
    );
    let child_deadline = Instant::now() + Duration::from_secs(3);
    let (child_pid, child_session_id) = loop {
        let mut child_command;
        if let Some(key) = ssh_key {
            child_command = Command::new("ssh");
            child_command.args(["-i", key, "-o", "BatchMode=yes"]);
        } else {
            child_command = Command::new("sshpass");
            child_command.args(["-p", password.expect("password or SSH key"), "ssh"]);
        }
        let child_output = child_command
            .args([
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "ConnectTimeout=3",
                &destination,
                &child_query,
            ])
            .output()
            .expect("query detached mosh-server child");
        if child_output.status.success() {
            let values = String::from_utf8_lossy(&child_output.stdout)
                .split_whitespace()
                .filter_map(|value| value.parse::<u32>().ok())
                .collect::<Vec<_>>();
            if values.len() == 2 {
                break (values[0], values[1]);
            }
        }
        assert!(
            Instant::now() < child_deadline,
            "could not query detached mosh-server PTY child"
        );
        thread::sleep(Duration::from_millis(50));
    };
    let server_guard = RemoteServerGuard {
        host: host.to_string(),
        user: user.to_string(),
        password: password.map(str::to_string),
        ssh_key: ssh_key.map(str::to_string),
        pid: detached_pid,
        session_id,
        child_pid,
        child_session_id,
    };
    // MOSH CONNECT <port> <key>
    for line in combined.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("MOSH CONNECT ") {
            let mut parts = rest.split_whitespace();
            let port: u16 = parts.next().expect("port").parse().expect("port number");
            let key = parts.next().expect("key").to_string();
            return (port, key, server_guard);
        }
    }
    panic!("no MOSH CONNECT in remote response");
}

fn poll_until<F>(client: &mut Client, deadline: Instant, mut pred: F) -> Vec<u8>
where
    F: FnMut(&[u8]) -> bool,
{
    let mut acc = Vec::new();
    while Instant::now() < deadline {
        let chunk = client.poll().expect("poll");
        if !chunk.is_empty() {
            acc.extend_from_slice(&chunk);
            if pred(&acc) {
                return acc;
            }
        }
        thread::sleep(Duration::from_millis(15));
    }
    acc
}

fn framebuffer_text(client: &Client) -> String {
    framebuffer_text_from_frame(client.remote_framebuffer())
}

fn framebuffer_text_from_frame(frame: &nyaterm_mosh::framebuffer::Framebuffer) -> String {
    (0..frame.rows)
        .map(|y| {
            (0..frame.cols)
                .filter_map(|x| frame.cell_at(x, y).map(|cell| cell.ch))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct UdpBlackholeProxy {
    port: u16,
    blackholed: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    server_packets: Arc<Mutex<Vec<Vec<u8>>>>,
    worker: Option<JoinHandle<()>>,
}

type CapturedFragmentGroup = (Option<usize>, HashMap<u16, Vec<u8>>);

impl UdpBlackholeProxy {
    fn start(host: &str, server_port: u16) -> Self {
        let server = (host, server_port)
            .to_socket_addrs()
            .expect("resolve live server")
            .find(SocketAddr::is_ipv4)
            .expect("live blackhole test requires an IPv4 server address");
        let socket = UdpSocket::bind("0.0.0.0:0").expect("bind UDP proxy");
        SockRef::from(&socket)
            .set_recv_buffer_size(4 * 1024 * 1024)
            .expect("large UDP proxy receive queue");
        let port = socket.local_addr().expect("proxy address").port();
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .expect("proxy read timeout");
        let blackholed = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let server_packets = Arc::new(Mutex::new(Vec::new()));
        let worker_blackholed = blackholed.clone();
        let worker_stopped = stopped.clone();
        let worker_server_packets = server_packets.clone();
        let worker = thread::spawn(move || {
            let mut client = None;
            let mut buf = [0u8; 65_535];
            while !worker_stopped.load(Ordering::SeqCst) {
                match socket.recv_from(&mut buf) {
                    Ok((len, source)) if source == server => {
                        worker_server_packets
                            .lock()
                            .expect("server packet capture")
                            .push(buf[..len].to_vec());
                        if !worker_blackholed.load(Ordering::SeqCst)
                            && let Some(destination) = client
                        {
                            let _ = socket.send_to(&buf[..len], destination);
                        }
                    }
                    Ok((len, source)) if source.ip().is_loopback() => {
                        client = Some(source);
                        if !worker_blackholed.load(Ordering::SeqCst) {
                            let _ = socket.send_to(&buf[..len], server);
                        }
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            blackholed,
            stopped,
            server_packets,
            worker: Some(worker),
        }
    }

    fn set_blackholed(&self, blackholed: bool) {
        self.blackholed.store(blackholed, Ordering::SeqCst);
    }

    fn clear_server_packets(&self) {
        self.server_packets
            .lock()
            .expect("server packet capture")
            .clear();
    }

    fn has_fragmented_server_instruction(&self, key: &str) -> bool {
        let ocb = Ocb::from_base64(key).expect("live MOSH_KEY");
        let mut fragments_by_instruction: HashMap<u64, HashSet<u16>> = HashMap::new();
        for packet in self
            .server_packets
            .lock()
            .expect("server packet capture")
            .iter()
        {
            let Some((_sequence, plaintext)) = ocb.open_datagram(packet) else {
                continue;
            };
            if plaintext.len() < 14 {
                continue;
            }
            let instruction_id = u64::from_be_bytes(plaintext[4..12].try_into().unwrap());
            let fragment_num = u16::from_be_bytes(plaintext[12..14].try_into().unwrap()) & 0x7fff;
            fragments_by_instruction
                .entry(instruction_id)
                .or_default()
                .insert(fragment_num);
        }
        fragments_by_instruction
            .values()
            .any(|fragment_nums| fragment_nums.len() > 1)
    }

    fn largest_server_instruction_payload(&self, key: &str) -> usize {
        let ocb = Ocb::from_base64(key).expect("live MOSH_KEY");
        let mut fragments_by_instruction: HashMap<u64, HashMap<u16, usize>> = HashMap::new();
        for packet in self
            .server_packets
            .lock()
            .expect("server packet capture")
            .iter()
        {
            let Some((_sequence, plaintext)) = ocb.open_datagram(packet) else {
                continue;
            };
            if plaintext.len() < 14 {
                continue;
            }
            let instruction_id = u64::from_be_bytes(plaintext[4..12].try_into().unwrap());
            let fragment_num = u16::from_be_bytes(plaintext[12..14].try_into().unwrap()) & 0x7fff;
            fragments_by_instruction
                .entry(instruction_id)
                .or_default()
                .entry(fragment_num)
                .or_insert(plaintext.len() - 14);
        }
        fragments_by_instruction
            .values()
            .map(|fragments| fragments.values().sum())
            .max()
            .unwrap_or_default()
    }

    fn complete_server_instruction_stats(
        &self,
        key: &str,
    ) -> Vec<(u64, u64, usize, usize, usize, usize, bool)> {
        let ocb = Ocb::from_base64(key).expect("live MOSH_KEY");
        let mut groups: HashMap<u64, CapturedFragmentGroup> = HashMap::new();
        for packet in self
            .server_packets
            .lock()
            .expect("server packet capture")
            .iter()
        {
            let Some((_sequence, plaintext)) = ocb.open_datagram(packet) else {
                continue;
            };
            let Ok(fragment) = Fragment::decode(plaintext.get(4..).unwrap_or_default()) else {
                continue;
            };
            let entry = groups.entry(fragment.id).or_default();
            if fragment.is_final {
                entry.0 = Some(fragment.fragment_num as usize + 1);
            }
            entry
                .1
                .entry(fragment.fragment_num)
                .or_insert(fragment.payload);
        }

        let mut stats = Vec::new();
        for (_id, (total, fragments)) in groups {
            let Some(total) = total else {
                continue;
            };
            if (0..total).any(|index| !fragments.contains_key(&(index as u16))) {
                continue;
            }
            let mut compressed = Vec::new();
            for index in 0..total {
                compressed.extend_from_slice(&fragments[&(index as u16)]);
            }
            let mut decoded = Vec::new();
            if ZlibDecoder::new(compressed.as_slice())
                .read_to_end(&mut decoded)
                .is_err()
            {
                continue;
            }
            let Ok(transport) = TransportInstruction::decode(&decoded) else {
                continue;
            };
            let Ok(host) = HostInstruction::decode_message(&transport.diff) else {
                continue;
            };
            let host_bytes = host
                .iter()
                .map(|instruction| instruction.hoststring.len())
                .sum();
            let x_count = host
                .iter()
                .flat_map(|instruction| instruction.hoststring.iter())
                .filter(|&&byte| byte == b'X')
                .count();
            let has_done = host.iter().any(|instruction| {
                instruction
                    .hoststring
                    .windows(12)
                    .any(|w| w == b"MCLARGE_DONE")
            });
            stats.push((
                transport.new_num,
                transport.old_num,
                compressed.len(),
                transport.diff.len(),
                host_bytes,
                x_count,
                has_done,
            ));
        }
        stats.sort_unstable_by_key(|stat| stat.0);
        stats
    }
}

impl Drop for UdpBlackholeProxy {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("UDP proxy worker");
        }
    }
}

#[test]
#[ignore = "live mosh-server; set MOSH_LIVE_HOST and SSH credentials"]
fn live_echo_and_prediction_pipeline_no_double_glyph() {
    let host = env_or_skip("MOSH_LIVE_HOST");
    if host.is_empty() {
        return;
    }
    let user = std::env::var("MOSH_LIVE_USER").unwrap_or_else(|_| "root".into());
    let password = std::env::var("MOSH_LIVE_PASSWORD").ok();
    let ssh_key = std::env::var("MOSH_LIVE_SSH_KEY").ok();
    if password.is_none() && ssh_key.is_none() {
        eprintln!("skip: set MOSH_LIVE_PASSWORD or MOSH_LIVE_SSH_KEY");
        return;
    }

    let (port, key, server_guard) =
        start_remote_mosh_server(&host, &user, password.as_deref(), ssh_key.as_deref());
    eprintln!("live: MOSH CONNECT {port} (key redacted)");

    let mut client = Client::dial(&host, port, &key).expect("dial mosh-server");
    client.resize(80, 24);

    // Drain initial paint (banner / prompt)
    let init = poll_until(&mut client, Instant::now() + Duration::from_secs(3), |_| {
        false
    });
    eprintln!(
        "live: initial paint plain={:?}",
        strip_ansi(&String::from_utf8_lossy(&init))
            .chars()
            .take(80)
            .collect::<String>()
    );

    let mut display = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    let mut last_remote_state_num = client.remote_state_num();
    let initial_frame = display.on_host_frame(client.remote_framebuffer());
    assert!(!initial_frame.is_empty(), "initial host frame must paint");
    display.set_frames(
        client.sent_num(),
        client.acked_by_remote(),
        client.echo_ack(),
    );

    // Local prediction for "hello"
    let local = display.on_keystroke(b"hello");
    assert!(display.predictor().pending_len() >= 5);
    eprintln!(
        "live: initial tentative prediction paint bytes={}",
        local.len()
    );

    // Send to server
    client.send_keys(b"hello");
    // Force flush
    let _ = client.poll();

    // Wait for host echo
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut host_acc = Vec::new();
    while Instant::now() < deadline {
        display.set_frames(
            client.sent_num(),
            client.acked_by_remote(),
            client.echo_ack(),
        );
        let chunk = client.poll().expect("poll");
        if !chunk.is_empty() {
            host_acc.extend_from_slice(&chunk);
            let plain = strip_ansi(&String::from_utf8_lossy(&host_acc));
            if plain.contains('h') && plain.contains('o') {
                // Keep going until the reconstructed frame below has caught up.
            }
        }
        if client.remote_state_num() != last_remote_state_num {
            last_remote_state_num = client.remote_state_num();
            let _ = display.on_host_frame(client.remote_framebuffer());
        }
        thread::sleep(Duration::from_millis(20));
    }

    // After host echo + late_ack, pending should drain (or reduce)
    display.set_frames(
        client.sent_num(),
        client.acked_by_remote(),
        client.echo_ack(),
    );
    // One more confirm path via empty host? re-apply nothing — call confirm via on_host_bytes empty not useful.
    // Poll more
    for _ in 0..20 {
        display.set_frames(
            client.sent_num(),
            client.acked_by_remote(),
            client.echo_ack(),
        );
        let chunk = client.poll().expect("poll");
        if !chunk.is_empty() {
            host_acc.extend_from_slice(&chunk);
        }
        if client.remote_state_num() != last_remote_state_num {
            last_remote_state_num = client.remote_state_num();
            let _ = display.on_host_frame(client.remote_framebuffer());
        }
        if display.predictor().pending_len() == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }

    eprintln!(
        "live: echo_ack={} sent={} early={} pending={} (pending may remain while late_ack < expiration_sent)",
        client.echo_ack(),
        client.sent_num(),
        client.acked_by_remote(),
        display.predictor().pending_len()
    );

    // Critical #2121 property: the final reconstructed screen contains exactly
    // one command, regardless of which terminal row the shell prompt occupies.
    let final_shown = display.last_shown().expect("last_shown");
    let screen = (0..final_shown.rows)
        .map(|y| {
            (0..final_shown.cols)
                .filter_map(|x| final_shown.cell_at(x, y).map(|cell| cell.ch))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        screen.matches("hello").count(),
        1,
        "expected one hello on the final screen (NyaTerm #2121); screen={screen:?} host={:?}",
        strip_ansi(&String::from_utf8_lossy(&host_acc))
    );

    // echo_ack should have advanced on a real server after keystrokes
    assert!(
        client.echo_ack() > 0 || display.predictor().pending_len() == 0,
        "real mosh-server should advance echo_ack after typed keys (got {})",
        client.echo_ack()
    );
    assert_graceful_shutdown(&mut client, &server_guard, "prediction session");
}

#[test]
#[ignore = "live mosh-server; set MOSH_LIVE_HOST and SSH credentials"]
fn live_client_survives_resize_and_more_keys() {
    let host = env_or_skip("MOSH_LIVE_HOST");
    if host.is_empty() {
        return;
    }
    let user = std::env::var("MOSH_LIVE_USER").unwrap_or_else(|_| "root".into());
    let password = std::env::var("MOSH_LIVE_PASSWORD").ok();
    let ssh_key = std::env::var("MOSH_LIVE_SSH_KEY").ok();
    if password.is_none() && ssh_key.is_none() {
        eprintln!("skip: set MOSH_LIVE_PASSWORD or MOSH_LIVE_SSH_KEY");
        return;
    }

    let (port, key, server_guard) =
        start_remote_mosh_server(&host, &user, password.as_deref(), ssh_key.as_deref());
    let mut client =
        Client::dial_candidates_with_size(&["127.0.0.2", host.as_str()], port, &key, 100, 30)
            .expect("dial candidates");
    let _ = poll_until(&mut client, Instant::now() + Duration::from_secs(2), |_| {
        false
    });
    let initial_screen = framebuffer_text(&client);
    assert!(
        initial_screen
            .lines()
            .any(|line| line == "MCINITIAL_SIZE:30 100"),
        "remote child did not start at the requested size; screen={initial_screen:?}"
    );

    // Stock mosh accepts large winsizes carried by the u16 terminal fields.
    // Exercise the real server path that the old 100,000-cell client cap
    // rejected, then restore a normal interactive size for content checks.
    client.resize(1600, 900);
    let large_deadline = Instant::now() + Duration::from_secs(8);
    while (
        client.remote_framebuffer().cols,
        client.remote_framebuffer().rows,
    ) != (1600, 900)
        && Instant::now() < large_deadline
    {
        client.poll().expect("poll large resize");
        thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(
        (
            client.remote_framebuffer().cols,
            client.remote_framebuffer().rows
        ),
        (1600, 900),
        "official server did not confirm the large terminal size"
    );
    client.resize(100, 30);
    let restore_deadline = Instant::now() + Duration::from_secs(8);
    while (
        client.remote_framebuffer().cols,
        client.remote_framebuffer().rows,
    ) != (100, 30)
        && Instant::now() < restore_deadline
    {
        client.poll().expect("poll restored resize");
        thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(
        (
            client.remote_framebuffer().cols,
            client.remote_framebuffer().rows
        ),
        (100, 30),
        "official server did not restore the interactive terminal size"
    );

    client.send_keys(b"printf 'MCTAB_A\\tMCTAB_B\\n'; echo live-ok\n");
    let paint = poll_until(
        &mut client,
        Instant::now() + Duration::from_secs(5),
        |acc| {
            let s = strip_ansi(&String::from_utf8_lossy(acc));
            s.contains("live-ok")
        },
    );
    let plain = strip_ansi(&String::from_utf8_lossy(&paint));
    eprintln!(
        "live resize/cmd plain excerpt={:?}",
        plain.chars().take(120).collect::<String>()
    );
    assert!(
        plain.contains("live-ok"),
        "command output missing; plain={plain:?}"
    );
    let final_screen = framebuffer_text(&client);
    assert!(
        final_screen.lines().any(|line| line == "MCTAB_A MCTAB_B"),
        "official server tab output was not reconstructed exactly; screen={final_screen:?}"
    );
    assert!(!client.is_dead(), "client died: {:?}", client.dead_reason());
    assert_graceful_shutdown(&mut client, &server_guard, "resize session");
}

#[test]
#[ignore = "live mosh-server; set MOSH_LIVE_HOST and SSH credentials"]
fn live_large_screen_update_reassembles_against_stock_server() {
    const LARGE_COLS: u16 = 400;
    const LARGE_ROWS: u16 = 60;
    let host = env_or_skip("MOSH_LIVE_HOST");
    if host.is_empty() {
        return;
    }
    let user = std::env::var("MOSH_LIVE_USER").unwrap_or_else(|_| "root".into());
    let password = std::env::var("MOSH_LIVE_PASSWORD").ok();
    let ssh_key = std::env::var("MOSH_LIVE_SSH_KEY").ok();
    if password.is_none() && ssh_key.is_none() {
        eprintln!("skip: set MOSH_LIVE_PASSWORD or MOSH_LIVE_SSH_KEY");
        return;
    }

    let (port, key, server_guard) =
        start_remote_mosh_server(&host, &user, password.as_deref(), ssh_key.as_deref());
    let proxy = UdpBlackholeProxy::start(&host, port);
    let mut client = Client::dial_with_size("127.0.0.1", proxy.port, &key, LARGE_COLS, LARGE_ROWS)
        .expect("dial");
    client.resize(LARGE_COLS, LARGE_ROWS);
    let _ = poll_until(&mut client, Instant::now() + Duration::from_secs(2), |_| {
        false
    });
    proxy.clear_server_packets();

    // Build a high-entropy printable ASCII screen before announcing readiness,
    // then wait one second before painting it. The deterministic unit path
    // covers the 1 MiB protocol boundary; this live path keeps a realistic
    // multi-fragment update fast enough for public-host verification.
    let completion_marker = format!("/tmp/nyaterm_mosh-live-{}-large-done", server_guard.pid);
    let command = format!(
        "python3 -c 'import hashlib,sys,time;b=hashlib.shake_256(b\"nyaterm_mosh-large-screen\").digest(23600);s=bytes(33+x%94 for x in b);print(\"MCLARGE_\"+\"ARMED\",flush=True);time.sleep(1);sys.stdout.buffer.write(s+b\"\\033[60;1HMCLARGE_DONE\");sys.stdout.buffer.flush();open(\"{}\",\"w\").close()'\n",
        completion_marker
    );
    client.send_keys(command.as_bytes());
    let armed_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < armed_deadline {
        client.poll().expect("send large-state command");
        if framebuffer_text(&client)
            .lines()
            .any(|line| line == "MCLARGE_ARMED")
        {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }

    assert!(
        framebuffer_text(&client)
            .lines()
            .any(|line| line == "MCLARGE_ARMED"),
        "large-state command was not confirmed before isolating the server"
    );
    proxy.clear_server_packets();
    proxy.set_blackholed(true);
    let completion_deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if server_guard.remote_file_exists(&completion_marker) == Some(true) {
            break;
        }
        assert!(
            Instant::now() < completion_deadline,
            "remote host did not finish producing the large terminal state"
        );
        thread::sleep(Duration::from_millis(250));
    }
    // The marker means the PTY accepted every byte. Give stock mosh-server one
    // more scheduling turn to fold the final read into its framebuffer.
    thread::sleep(Duration::from_secs(1));
    proxy.set_blackholed(false);

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut final_screen = String::new();
    let mut filled_rows = 0;
    while Instant::now() < deadline {
        client.poll().expect("poll");
        final_screen = framebuffer_text(&client);
        filled_rows = final_screen
            .lines()
            .filter(|line| line.len() == usize::from(LARGE_COLS))
            .count();
        if filled_rows >= 50
            && final_screen
                .lines()
                .any(|line| line.contains("MCLARGE_DONE"))
        {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }

    assert!(
        final_screen
            .lines()
            .any(|line| line.contains("MCLARGE_DONE")),
        "large output never reached its sentinel; screen={final_screen:?}"
    );
    assert!(
        filled_rows >= 50,
        "large fragmented terminal state was truncated; rows={filled_rows}, remote_state={}, wire_bytes={}, row_widths={:?}, instructions={:?}",
        client.remote_state_num(),
        proxy.largest_server_instruction_payload(&key),
        final_screen.lines().map(str::len).collect::<Vec<_>>(),
        proxy.complete_server_instruction_stats(&key)
    );
    assert!(
        proxy.has_fragmented_server_instruction(&key),
        "stock server never emitted a multi-fragment instruction for the large update"
    );
    assert!(
        proxy.largest_server_instruction_payload(&key) > 16 * 1024,
        "stock server response did not produce a representative fragmented update"
    );
    assert!(!client.is_dead(), "client died: {:?}", client.dead_reason());
    assert_graceful_shutdown(&mut client, &server_guard, "large-output session");
}

#[test]
#[ignore = "live mosh-server; set MOSH_LIVE_HOST and SSH credentials"]
fn live_session_recovers_after_silent_udp_blackhole() {
    let host = env_or_skip("MOSH_LIVE_HOST");
    if host.is_empty() {
        return;
    }
    let user = std::env::var("MOSH_LIVE_USER").unwrap_or_else(|_| "root".into());
    let password = std::env::var("MOSH_LIVE_PASSWORD").ok();
    let ssh_key = std::env::var("MOSH_LIVE_SSH_KEY").ok();
    if password.is_none() && ssh_key.is_none() {
        eprintln!("skip: set MOSH_LIVE_PASSWORD or MOSH_LIVE_SSH_KEY");
        return;
    }

    let (server_port, key, server_guard) =
        start_remote_mosh_server(&host, &user, password.as_deref(), ssh_key.as_deref());
    let proxy = UdpBlackholeProxy::start(&host, server_port);
    let mut client = Client::dial_with_size("127.0.0.1", proxy.port, &key, 100, 30).expect("dial");
    client.resize(100, 30);
    let _ = poll_until(&mut client, Instant::now() + Duration::from_secs(3), |_| {
        false
    });
    client.send_keys(b"echo BEFORE_BLACKHOLE_OK\n");
    let before_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < before_deadline {
        client.poll().expect("poll before blackhole");
        if framebuffer_text(&client)
            .lines()
            .any(|line| line == "BEFORE_BLACKHOLE_OK")
        {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    assert!(
        framebuffer_text(&client)
            .lines()
            .any(|line| line == "BEFORE_BLACKHOLE_OK"),
        "session was not usable before blackhole"
    );

    proxy.set_blackholed(true);
    let outage_deadline = Instant::now() + Duration::from_secs(12);
    let mut saw_outage_notification = false;
    while Instant::now() < outage_deadline {
        client.poll().expect("poll during blackhole");
        if matches!(client.connection_status(), ConnectionStatus::LastContact(_)) {
            saw_outage_notification = true;
        }
        assert!(
            !client.is_dead(),
            "client died during a recoverable outage: {:?}",
            client.dead_reason()
        );
        thread::sleep(Duration::from_millis(15));
    }
    assert!(
        saw_outage_notification,
        "stock-compatible last-contact notification never appeared"
    );
    proxy.set_blackholed(false);

    client.send_keys(b"echo AFTER_BLACKHOLE_OK\n");
    let recovery_deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < recovery_deadline {
        client.poll().expect("poll after blackhole");
        if framebuffer_text(&client)
            .lines()
            .any(|line| line == "AFTER_BLACKHOLE_OK")
        {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    let recovered_screen = framebuffer_text(&client);
    assert!(
        recovered_screen
            .lines()
            .any(|line| line == "AFTER_BLACKHOLE_OK"),
        "same session did not recover after blackhole; screen={recovered_screen:?}"
    );
    assert_eq!(
        client.connection_status(),
        ConnectionStatus::Online,
        "network notification did not clear after recovery"
    );
    assert_graceful_shutdown(&mut client, &server_guard, "recovered session");
}
