//! High-level mosh client session (UDP + SSP + HostBytes paint).
//!
//! Stock mosh-server puts `Display::new_frame` output in HostBytes.hoststring
//! (ANSI-like paint). Writing that stream to a real PTY/xterm is correct for
//! Netcatty's node-pty sandwich and eliminates the Cygwin terminfo path.

use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use quinn_udp::EcnCodepoint;

use crate::crypto::Ocb;
use crate::ecn::EcnSocket;
use crate::error::{Error, Result};
use crate::pb::UserInstruction;
use crate::terminal::TerminalView;
use crate::transport::Transport;

const TICK_INTERVAL: Duration = Duration::from_millis(8);
const MAX_DATAGRAM_SIZE: usize = 2048;
/// Bound receive work per caller turn so a busy or hostile UDP path cannot
/// starve local input and prediction. Stock mosh receives one packet per
/// select-loop turn; a larger bounded batch preserves fast fragment assembly
/// without restoring the old unbounded drain.
const MAX_RECEIVE_BATCHES_PER_POLL: usize = 1024;
/// Stock mosh only times out the initial attachment; an established session
/// remains resumable across long network outages.
const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PORT_HOP_INTERVAL: Duration = Duration::from_secs(10);
const MAX_OLD_SOCKET_AGE: Duration = Duration::from_secs(60);
const MAX_PORTS_OPEN: usize = 10;
const MAX_BOOTSTRAP_ADDRESSES: usize = 16;
const IPV6_FRAGMENT_PAYLOAD: usize = 1178;
const FALLBACK_FRAGMENT_PAYLOAD: usize = 462;
const CONNECTING_NOTICE_AFTER: Duration = Duration::from_millis(250);
const SERVER_LATE_AFTER: Duration = Duration::from_millis(6_500);
const REPLY_LATE_AFTER: Duration = Duration::from_secs(10);

/// Stock-mosh-compatible connection state used by the terminal notification
/// overlay. `Online` means no notification is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Online,
    Connecting(Duration),
    LastContact(Duration),
    LastReply(Duration),
}

impl ConnectionStatus {
    /// Text drawn by stock mosh's blue notification bar.
    pub fn message(self, remote_port: u16) -> Option<String> {
        match self {
            Self::Online => None,
            Self::Connecting(_) => Some(format!(
                "mosh: Nothing received from server on UDP port {remote_port}."
            )),
            Self::LastContact(elapsed) => Some(format!(
                "mosh: Last contact {} ago.",
                human_readable_duration(elapsed)
            )),
            Self::LastReply(elapsed) => Some(format!(
                "mosh: Last reply {} ago.",
                human_readable_duration(elapsed)
            )),
        }
    }
}

fn classify_connection_status(
    attached: bool,
    since_contact: Duration,
    since_reply: Duration,
) -> ConnectionStatus {
    if !attached {
        return if since_contact > CONNECTING_NOTICE_AFTER {
            ConnectionStatus::Connecting(since_contact)
        } else {
            ConnectionStatus::Online
        };
    }

    // Upstream prefers the downlink failure whenever both directions are late.
    if since_contact > SERVER_LATE_AFTER {
        ConnectionStatus::LastContact(since_contact)
    } else if since_reply > REPLY_LATE_AFTER {
        ConnectionStatus::LastReply(since_reply)
    } else {
        ConnectionStatus::Online
    }
}

fn human_readable_duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds} seconds")
    } else if seconds < 3_600 {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3_600,
            (seconds / 60) % 60,
            seconds % 60
        )
    }
}

fn is_message_too_long(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::EMSGSIZE)
    }
    #[cfg(windows)]
    {
        // WSAEMSGSIZE from WinSock2.
        error.raw_os_error() == Some(10040)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = error;
        false
    }
}

/// A connected mosh client session.
pub struct Client {
    sockets: VecDeque<EcnSocket>,
    receive_buffer: Vec<u8>,
    remote_addr: SocketAddr,
    last_port_choice: Instant,
    transport: Transport,
    terminal: TerminalView,
    actions: Vec<UserInstruction>,
    acked_action_count: usize,
    last_acked: u64,
    sent_action_counts: Vec<(u64, usize)>,
    dirty: bool,
    last_tick: Instant,
    /// Set when UDP peer is gone / hard error — CLI should exit.
    dead: bool,
    dead_reason: Option<String>,
    initial_timeout_shutdown: bool,
    force_known_receiver_base: bool,
}

impl Client {
    /// True only after a complete authenticated server state has been accepted.
    pub fn is_attached(&self) -> bool {
        self.transport.has_received_authenticated()
    }

    pub fn pending_input_bytes(&self) -> usize {
        self.actions.iter().map(|action| action.keys.len()).sum()
    }

    /// Connect to a mosh-server endpoint.
    pub fn dial(host: &str, port: u16, key: &str) -> Result<Self> {
        Self::dial_with_size(host, port, key, 80, 24)
    }

    /// Connect with the actual local terminal size so state 0 matches the
    /// server-side terminal before the first resize instruction is applied.
    pub fn dial_with_size(host: &str, port: u16, key: &str, cols: u16, rows: u16) -> Result<Self> {
        Self::dial_candidates_with_size(&[host], port, key, cols, rows)
    }

    /// Try every resolved bootstrap endpoint until one returns an
    /// authenticated state. This mirrors stock mosh's use of the exact SSH
    /// endpoint and avoids sending UDP only to the first DNS result.
    pub fn dial_candidates_with_size(
        hosts: &[&str],
        port: u16,
        key: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let ocb = Ocb::from_base64(key)?;
        let mut addresses = Vec::new();
        let mut last_resolution_error = None;
        for host in hosts {
            let resolved = match (*host, port).to_socket_addrs() {
                Ok(resolved) => resolved,
                Err(error) => {
                    last_resolution_error = Some(error);
                    continue;
                }
            };
            for address in resolved {
                if !addresses.contains(&address) {
                    addresses.push(address);
                    if addresses.len() == MAX_BOOTSTRAP_ADDRESSES {
                        break;
                    }
                }
            }
            if addresses.len() == MAX_BOOTSTRAP_ADDRESSES {
                break;
            }
        }
        if addresses.is_empty() {
            return Err(last_resolution_error.map_or_else(
                || {
                    Error::Other(format!(
                        "could not resolve a mosh endpoint on UDP port {port}"
                    ))
                },
                Error::Io,
            ));
        }

        let mut sockets = VecDeque::new();
        let mut receive_buffer_size = MAX_DATAGRAM_SIZE;
        let mut last_open_error = None;
        for address in addresses {
            match Self::open_socket(address) {
                Ok(socket) => {
                    receive_buffer_size =
                        receive_buffer_size.max(socket.receive_buffer_size(MAX_DATAGRAM_SIZE));
                    sockets.push_back(socket);
                }
                Err(error) => last_open_error = Some(error),
            }
        }
        if sockets.is_empty() {
            return Err(last_open_error.unwrap_or_else(|| {
                Error::Other("could not open a UDP socket for any mosh endpoint".into())
            }));
        }
        let has_ipv6_candidate = sockets.iter().any(|socket| socket.peer_addr().is_ipv6());
        let remote_addr = sockets.front().expect("non-empty sockets").peer_addr();
        let receive_buffer = vec![0; receive_buffer_size];

        let mut transport = Transport::new_client(ocb);
        if has_ipv6_candidate {
            transport.set_max_fragment_payload(IPV6_FRAGMENT_PAYLOAD);
        }
        transport.force_next_send();
        let mut client = Self {
            sockets,
            receive_buffer,
            remote_addr,
            last_port_choice: Instant::now(),
            transport,
            terminal: TerminalView::new(cols, rows),
            actions: Vec::new(),
            acked_action_count: 0,
            last_acked: 0,
            sent_action_counts: Vec::new(),
            dirty: false,
            last_tick: Instant::now(),
            dead: false,
            dead_reason: None,
            initial_timeout_shutdown: false,
            force_known_receiver_base: false,
        };
        // Stock queues the initial Resize before its first network tick. This
        // is the state that releases mosh-server's child process, so it must
        // already carry the real PTY dimensions rather than an empty 80x24
        // placeholder followed by a later correction.
        client.resize(cols, rows);
        client.flush_ticks()?;
        Ok(client)
    }

    pub fn send_keys(&mut self, keys: &[u8]) {
        if keys.is_empty() || self.dead {
            return;
        }
        self.actions.push(UserInstruction::keystroke(keys.to_vec()));
        self.dirty = true;
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if self.dead {
            return;
        }
        self.actions
            .push(UserInstruction::resize(cols as i32, rows as i32));
        self.dirty = true;
    }

    /// True after network death / timeout — caller should exit the process.
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    pub fn dead_reason(&self) -> Option<&str> {
        self.dead_reason.as_deref()
    }

    /// Current reachability state using the same timers and precedence as
    /// stock mosh's NotificationEngine.
    pub fn connection_status(&self) -> ConnectionStatus {
        classify_connection_status(
            self.transport.has_received_authenticated(),
            self.transport.last_remote_state().elapsed(),
            self.transport.last_roundtrip_success().elapsed(),
        )
    }

    pub fn remote_shutdown_ack_sent(&self) -> bool {
        self.transport.counterparty_shutdown_ack_sent()
    }

    /// Ask the peer to close the SSP session and keep pumping UDP until the
    /// request is acknowledged or the caller's deadline expires.
    pub fn graceful_shutdown(&mut self, timeout: Duration) -> Result<bool> {
        if self.dead {
            return Ok(false);
        }
        if self.remote_shutdown_ack_sent() {
            return Ok(true);
        }
        // Stock shutdown carries the current user state. Flush input queued in
        // the same stdin read as EOF / the local quit command before changing
        // the transport into its no-more-writes shutdown mode.
        self.flush_ticks()?;
        self.transport.start_shutdown();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let _ = self.poll()?;
            if self.transport.shutdown_acknowledged() || self.remote_shutdown_ack_sent() {
                return Ok(true);
            }
            if self.transport.shutdown_timed_out() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(self.transport.shutdown_acknowledged() || self.remote_shutdown_ack_sent())
    }

    /// Smoothed RTT from the transport, if any sample is available.
    pub fn srtt(&self) -> Option<std::time::Duration> {
        self.transport.srtt()
    }

    /// SSP state numbers for stock-style prediction frame expiry.
    pub fn sent_num(&self) -> u64 {
        self.transport.sent_num()
    }

    pub fn acked_by_remote(&self) -> u64 {
        self.transport.acked_by_remote()
    }

    /// Stock late_ack: max echo_ack_num from HostInstructions (prediction Pending).
    pub fn echo_ack(&self) -> u64 {
        self.terminal.echo_ack()
    }

    /// Stock prediction uses ceil(SRTT/2) clamped to 20–250ms as `send_interval`.
    pub fn send_interval(&self) -> std::time::Duration {
        self.srtt().map_or(Duration::from_millis(250), |d| {
            let half_ms = d.as_nanos().div_ceil(2_000_000) as u64;
            let ms = half_ms.clamp(20, 250);
            std::time::Duration::from_millis(ms)
        })
    }

    /// Poll network + flush pending ticks. Returns newly painted local bytes.
    pub fn poll(&mut self) -> Result<Vec<u8>> {
        if self.dead {
            return Ok(Vec::new());
        }

        let mut paint = Vec::new();
        let buf = &mut self.receive_buffer;
        let mut received_datagram = false;
        let was_attached = self.transport.has_received_authenticated();
        let mut authenticated_socket_index = None;
        // Visit every open migration socket once per round. This prevents an
        // old, flooded path from starving the newest port while still giving
        // the caller a predictable chance to service keyboard input.
        let mut received_batches = 0;
        'receive: loop {
            let mut received_this_round = false;
            for (socket_index, socket) in self.sockets.iter().enumerate() {
                if received_batches >= MAX_RECEIVE_BATCHES_PER_POLL {
                    break 'receive;
                }
                match socket.recv(buf) {
                    Ok(datagrams) => {
                        received_datagram = true;
                        received_this_round = true;
                        received_batches += 1;
                        for datagram in datagrams {
                            let end = datagram.offset + datagram.len;
                            if let Some(state) = self.transport.recv_state_with_congestion(
                                &buf[datagram.offset..end],
                                datagram.ecn == Some(EcnCodepoint::Ce),
                            ) {
                                authenticated_socket_index = Some(socket_index);
                                let chunk = match self.terminal.apply_host_state(
                                    state.old_num,
                                    state.new_num,
                                    state.throwaway_num,
                                    &state.diff,
                                ) {
                                    Ok(chunk) => chunk,
                                    Err(error) => {
                                        self.mark_dead("remote terminal state was rejected");
                                        return Err(error);
                                    }
                                };
                                paint.extend_from_slice(&chunk);
                            }
                        }
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    // A connected UDP socket can surface transient ICMP and
                    // route errors here. Mosh must keep the session resumable.
                    Err(_) => continue,
                }
            }
            if !received_this_round {
                break;
            }
        }
        if !was_attached
            && self.transport.has_received_authenticated()
            && let Some(index) = authenticated_socket_index
            && let Some(active_socket) = self.sockets.remove(index)
        {
            self.remote_addr = active_socket.peer_addr();
            self.sockets.clear();
            self.sockets.push_back(active_socket);
        }
        if received_datagram {
            self.prune_old_sockets();
        }

        if self.initial_timeout_shutdown {
            if self.transport.shutdown_acknowledged() || self.transport.shutdown_timed_out() {
                self.mark_dead("mosh did not hear from the server (initial connection timeout)");
                return Ok(paint);
            }
        } else if !self.transport.has_received_authenticated()
            && self.transport.last_remote_state().elapsed() > INITIAL_CONNECT_TIMEOUT
        {
            // Stock Mosh does not disappear immediately here. It sends the
            // normal SSP shutdown state and pumps the socket until the peer
            // acknowledges it (or the bounded shutdown retry expires), so a
            // one-way path failure does not leave the remote shell behind.
            self.initial_timeout_shutdown = true;
            self.transport.start_shutdown();
            self.flush_ticks()?;
            return Ok(paint);
        }

        if self.last_tick.elapsed() >= TICK_INTERVAL {
            self.flush_ticks()?;
            self.last_tick = Instant::now();
        }

        Ok(paint)
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut acc = Vec::new();
        while Instant::now() < deadline && !self.dead {
            let chunk = self.poll()?;
            if !chunk.is_empty() {
                acc.extend_from_slice(&chunk);
                let more = self.poll()?;
                acc.extend_from_slice(&more);
                return Ok(acc);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(acc)
    }

    pub fn terminal(&self) -> &TerminalView {
        &self.terminal
    }

    /// Latest complete server framebuffer and its SSP state number.
    pub fn remote_framebuffer(&self) -> &crate::framebuffer::Framebuffer {
        self.terminal.remote_framebuffer()
    }

    pub fn remote_state_num(&self) -> u64 {
        self.terminal.remote_state_num()
    }

    fn mark_dead(&mut self, reason: &str) {
        self.dead = true;
        self.dead_reason = Some(reason.to_string());
    }

    fn open_socket(addr: SocketAddr) -> Result<EcnSocket> {
        Ok(EcnSocket::bind(addr)?)
    }

    fn maybe_hop_port(&mut self) {
        if self.last_port_choice.elapsed() < PORT_HOP_INTERVAL
            || self.transport.last_roundtrip_success().elapsed() < PORT_HOP_INTERVAL
        {
            return;
        }
        // Opening a replacement socket is opportunistic. Keep the current
        // sockets and retry later if the OS temporarily cannot allocate one.
        self.last_port_choice = Instant::now();
        let Ok(socket) = Self::open_socket(self.remote_addr) else {
            return;
        };
        let required = socket.receive_buffer_size(MAX_DATAGRAM_SIZE);
        if self.receive_buffer.len() < required {
            self.receive_buffer.resize(required, 0);
        }
        self.sockets.push_back(socket);
        while self.sockets.len() > MAX_PORTS_OPEN {
            self.sockets.pop_front();
        }
    }

    fn prune_old_sockets(&mut self) {
        if self.sockets.len() > 1
            && self.last_port_choice.elapsed() > MAX_OLD_SOCKET_AGE
            && let Some(newest) = self.sockets.pop_back()
        {
            self.sockets.clear();
            self.sockets.push_back(newest);
        }
    }

    fn flush_ticks(&mut self) -> Result<()> {
        if self.dead {
            return Ok(());
        }
        self.process_acks();
        if self.dirty {
            self.dirty = false;
            let new_actions = self.actions[self.acked_action_count..].to_vec();
            if !new_actions.is_empty() {
                let mut old_num = self.transport.acked_by_remote();
                let mut payload = UserInstruction::encode_message(&new_actions);
                if !self.force_known_receiver_base
                    && let Some(prospective_num) = self.transport.prospective_base_num()
                    && let Some(prospective_count) = self
                        .sent_action_counts
                        .iter()
                        .find(|(state_num, _)| *state_num == prospective_num)
                        .map(|(_, count)| *count)
                        .filter(|count| *count <= self.actions.len())
                {
                    let prospective_payload =
                        UserInstruction::encode_message(&self.actions[prospective_count..]);
                    let resend_overhead = payload.len().saturating_sub(prospective_payload.len());
                    let prefer_known_receiver = payload.len() <= prospective_payload.len()
                        || (payload.len() < 1_000 && resend_overhead < 100);
                    if !prefer_known_receiver {
                        old_num = prospective_num;
                        payload = prospective_payload;
                    }
                }
                let next_num = self.transport.set_pending_from(old_num, payload);
                self.force_known_receiver_base = false;
                self.sent_action_counts.push((next_num, self.actions.len()));
            }
        }

        self.maybe_hop_port();
        let datagrams = self.transport.tick();
        if self.transport.take_outbound_instruction_too_large() {
            self.mark_dead("local terminal state exceeded the mosh protocol limit");
            return Err(Error::Protocol(
                "local terminal state exceeded the mosh protocol limit".into(),
            ));
        }
        if self.transport.crypto_exhausted() {
            self.mark_dead("mosh session encryption limit reached");
            return Ok(());
        }
        'send: for dg in datagrams {
            if !self.transport.has_received_authenticated() {
                for socket in &self.sockets {
                    if let Err(error) = socket.send(&dg)
                        && is_message_too_long(&error)
                    {
                        self.transport
                            .set_max_fragment_payload(FALLBACK_FRAGMENT_PAYLOAD);
                        break 'send;
                    }
                }
            } else if let Some(socket) = self.sockets.back() {
                // Like stock mosh, reportable UDP send failures do not end the
                // session; a later port hop or route recovery can resume it.
                if let Err(error) = socket.send(&dg)
                    && is_message_too_long(&error)
                {
                    self.transport
                        .set_max_fragment_payload(FALLBACK_FRAGMENT_PAYLOAD);
                    break;
                }
            }
        }
        Ok(())
    }

    fn process_acks(&mut self) {
        let acked = self.transport.acked_by_remote();
        let rebase_required =
            self.transport.take_rebase_required() || self.transport.prospective_chain_expired();
        if rebase_required {
            self.force_known_receiver_base = true;
            if !self.actions.is_empty() {
                self.dirty = true;
            }
        }
        if acked > self.last_acked {
            self.last_acked = acked;
            if let Some(count) = self
                .sent_action_counts
                .iter()
                .filter(|(state, _)| *state <= acked)
                .map(|(_, count)| *count)
                .max()
                && count > self.acked_action_count
            {
                self.acked_action_count = count;
            }
            self.sent_action_counts.retain(|(state, _)| *state > acked);
            // Drop fully-acked keystroke history (long-session bound).
            if self.acked_action_count > 0 && self.acked_action_count <= self.actions.len() {
                let drained = self.acked_action_count;
                self.actions.drain(..drained);
                for (_, count) in &mut self.sent_action_counts {
                    *count = count.saturating_sub(drained);
                }
                self.acked_action_count = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Client, ConnectionStatus, Duration, INITIAL_CONNECT_TIMEOUT, Instant, SocketAddr,
        classify_connection_status,
    };
    use crate::crypto::Ocb;
    use crate::pb::{HostInstruction, UserInstruction};
    use crate::transport::{ReceivedStateDiff, Transport};
    use std::collections::HashMap;
    use std::net::UdpSocket;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn apply_user_state(states: &mut HashMap<u64, Vec<u8>>, state: ReceivedStateDiff) -> Vec<u8> {
        let mut value = states.get(&state.old_num).cloned().unwrap_or_default();
        value.extend(
            UserInstruction::decode_message(&state.diff)
                .unwrap()
                .into_iter()
                .flat_map(|instruction| instruction.keys),
        );
        states.insert(state.new_num, value.clone());
        value
    }

    #[test]
    fn connection_status_matches_stock_mosh_thresholds() {
        assert_eq!(
            classify_connection_status(
                false,
                Duration::from_millis(250),
                Duration::from_millis(250)
            ),
            ConnectionStatus::Online
        );
        assert_eq!(
            classify_connection_status(
                false,
                Duration::from_millis(251),
                Duration::from_millis(251)
            ),
            ConnectionStatus::Connecting(Duration::from_millis(251))
        );
        assert_eq!(
            classify_connection_status(true, Duration::from_millis(6_500), Duration::from_secs(11)),
            ConnectionStatus::LastReply(Duration::from_secs(11))
        );
        assert_eq!(
            classify_connection_status(true, Duration::from_millis(6_501), Duration::from_secs(11)),
            ConnectionStatus::LastContact(Duration::from_millis(6_501))
        );
        assert_eq!(
            classify_connection_status(true, Duration::from_secs(2), Duration::from_millis(10_001)),
            ConnectionStatus::LastReply(Duration::from_millis(10_001))
        );
        assert_eq!(
            classify_connection_status(true, Duration::from_secs(2), Duration::from_secs(3)),
            ConnectionStatus::Online
        );
    }

    #[test]
    fn connection_status_uses_stock_human_readable_duration() {
        assert_eq!(
            ConnectionStatus::LastContact(Duration::from_secs(9)).message(60001),
            Some("mosh: Last contact 9 seconds ago.".to_string())
        );
        assert_eq!(
            ConnectionStatus::LastReply(Duration::from_secs(65)).message(60001),
            Some("mosh: Last reply 1:05 ago.".to_string())
        );
        assert_eq!(
            ConnectionStatus::Connecting(Duration::from_secs(1)).message(60001),
            Some("mosh: Nothing received from server on UDP port 60001.".to_string())
        );
        assert_eq!(ConnectionStatus::Online.message(60001), None);
    }

    fn spawn_echo_server(marker: &'static str) -> (u16, String, thread::JoinHandle<()>) {
        let mut key = [0u8; 16];
        for (i, b) in key.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(17).wrapping_add(3);
        }
        let key_b64 = {
            use base64::Engine;
            let s = base64::engine::general_purpose::STANDARD.encode(key);
            s.trim_end_matches('=').to_string()
        };

        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        sock.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();

        let done = Arc::new(Mutex::new(false));
        let done2 = done.clone();

        let handle = thread::spawn(move || {
            let ocb = Ocb::new(&key).unwrap();
            let mut transport = Transport::new_server(ocb);
            let mut client_addr = None;
            let mut sent_banner = false;
            let mut buf = [0u8; 4096];

            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(8) {
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
                            width: 0,
                            height: 0,
                            echo_ack_num: -1,
                        }]);
                        transport.set_pending(host);
                    }
                    if !sent_banner {
                        let host = HostInstruction::encode_message(&[HostInstruction {
                            hoststring: b"\x1b[H\x1b[2J$ ".to_vec(),
                            width: 0,
                            height: 0,
                            echo_ack_num: -1,
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
                thread::sleep(Duration::from_millis(5));
            }
            *done2.lock().unwrap() = true;
        });

        thread::sleep(Duration::from_millis(20));
        (port, key_b64, handle)
    }

    fn spawn_parallel_state_server() -> (u16, String, thread::JoinHandle<()>) {
        let mut key = [0u8; 16];
        for (i, b) in key.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(13).wrapping_add(5);
        }
        let key_b64 = {
            use base64::Engine;
            let s = base64::engine::general_purpose::STANDARD.encode(key);
            s.trim_end_matches('=').to_string()
        };

        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        let handle = thread::spawn(move || {
            let ocb = Ocb::new(&key).unwrap();
            let mut transport = Transport::new_server(ocb);
            let mut buf = [0u8; 4096];
            let (n, client_addr) = sock.recv_from(&mut buf).expect("client hello");
            let _ = transport.recv(&buf[..n]);

            // Before the client's ACK can return, the server emits two newer
            // states from the same state 0 base. Each branch legitimately
            // contains the same `h`; applying both diffs to the live screen
            // would display `hh` even though the server state contains one h.
            let branch = HostInstruction::encode_message(&[HostInstruction {
                hoststring: b"h".to_vec(),
                width: 0,
                height: 0,
                echo_ack_num: -1,
            }]);
            transport.set_pending(branch.clone());
            for datagram in transport.tick() {
                sock.send_to(&datagram, client_addr).unwrap();
            }
            transport.set_pending(branch);
            for datagram in transport.tick() {
                sock.send_to(&datagram, client_addr).unwrap();
            }
            thread::sleep(Duration::from_millis(250));
        });

        (port, key_b64, handle)
    }

    #[test]
    fn client_dial_recv_and_command_against_fake_server() {
        let marker = "NETCATTY_MOSH_UNIT_OK";
        let (port, key, handle) = spawn_echo_server(marker);

        let mut client = Client::dial("127.0.0.1", port, &key).expect("dial");
        let mut saw_prompt = false;
        for _ in 0..100 {
            let out = client.poll().unwrap();
            if !out.is_empty() {
                saw_prompt = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_prompt, "expected initial banner paint");

        client.send_keys(format!("echo {marker}\n").as_bytes());
        let mut all = String::new();
        for _ in 0..200 {
            let out = client.poll().unwrap();
            if !out.is_empty() {
                all.push_str(&String::from_utf8_lossy(&out));
            }
            if crate::terminal::strip_ansi(&all).contains(marker) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = handle.join();
        panic!("marker not found in output: {all:?}");
    }

    #[test]
    fn client_tries_every_bootstrap_address_until_one_authenticates() {
        let (port, key, _handle) = spawn_echo_server("MULTI_ADDRESS_OK");
        let mut client = Client::dial_candidates_with_size(
            &["not-a-real-mosh-host.invalid", "127.0.0.2", "127.0.0.1"],
            port,
            &key,
            80,
            24,
        )
        .expect("dial candidates");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !client.poll().unwrap().is_empty() {
                assert_eq!(
                    client.remote_addr.ip(),
                    "127.0.0.1".parse::<std::net::IpAddr>().unwrap()
                );
                assert_eq!(client.sockets.len(), 1);
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("client never selected the reachable bootstrap address");
    }

    #[test]
    fn parallel_remote_states_render_shared_content_once() {
        let (port, key, handle) = spawn_parallel_state_server();
        let mut client = Client::dial("127.0.0.1", port, &key).expect("dial");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut output = Vec::new();
        while Instant::now() < deadline {
            output.extend_from_slice(&client.poll().unwrap());
            if !output.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        handle.join().unwrap();

        let plain = crate::terminal::strip_ansi(&String::from_utf8_lossy(&output));
        assert_eq!(
            plain.chars().filter(|&ch| ch == 'h').count(),
            1,
            "parallel SSP branches must be reconstructed as states, not replayed as raw diffs: {plain:?}",
        );
    }

    #[test]
    fn dial_initializes_remote_state_with_actual_terminal_size() {
        let client = Client::dial_with_size("127.0.0.1", 9, "AAAAAAAAAAAAAAAAAAAAAA", 120, 40)
            .expect("dial");

        assert_eq!(client.remote_framebuffer().cols, 120);
        assert_eq!(client.remote_framebuffer().rows, 40);
    }

    #[test]
    fn poll_yields_before_draining_a_network_flood() {
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sink.local_addr().unwrap().port();
        let mut client = Client::dial("127.0.0.1", port, "AAAAAAAAAAAAAAAAAAAAAA").expect("dial");
        let client_addr = SocketAddr::from((
            [127, 0, 0, 1],
            client.sockets.front().unwrap().local_addr().unwrap().port(),
        ));
        let flood = UdpSocket::bind("127.0.0.1:0").unwrap();

        for _ in 0..4096 {
            flood.send_to(b"not-authenticated", client_addr).unwrap();
        }

        client.poll().expect("poll under flood");

        let mut buffer = [0u8; 2048];
        assert!(
            client.sockets.front().unwrap().recv(&mut buffer).is_ok(),
            "one poll must yield with queued datagrams left so keyboard input gets a turn"
        );
    }

    #[test]
    fn first_wire_state_carries_the_actual_terminal_size() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let key = "AAAAAAAAAAAAAAAAAAAAAA";
        let mut server = Transport::new_server(Ocb::from_base64(key).unwrap());

        let _client = Client::dial_with_size("127.0.0.1", port, key, 120, 40).expect("dial");
        let mut wire = [0u8; 2048];
        let first_state = loop {
            let (len, _) = socket.recv_from(&mut wire).expect("first client state");
            if let Some(state) = server.recv_state(&wire[..len]) {
                break state;
            }
        };
        let actions = UserInstruction::decode_message(&first_state.diff).unwrap();

        assert_eq!(first_state.new_num, 1);
        assert_eq!(actions, vec![UserInstruction::resize(120, 40)]);
    }

    #[test]
    fn client_reassembles_a_thousand_fragment_official_state_across_receive_turns() {
        let key = [67u8; 16];
        let key_b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(key)
                .trim_end_matches('=')
                .to_string()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut server = Transport::new_server(Ocb::new(&key).unwrap());
        let mut client = Client::dial_with_size("127.0.0.1", port, &key_b64, 1000, 70).unwrap();
        let mut wire = [0u8; 4096];

        let (n, client_addr) = socket.recv_from(&mut wire).expect("initial client state");
        server
            .recv_state(&wire[..n])
            .expect("initial client state decodes");

        let mut hoststring = Vec::with_capacity(2_200_000);
        for i in 0..68_000u64 {
            hoststring.push(b'X');
            let mut state = i.wrapping_add(1);
            for _ in 0..15 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let ch = char::from_u32(0x300 + (state % 112) as u32).unwrap();
                let mut encoded = [0u8; 4];
                hoststring.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
            }
        }
        let diff = HostInstruction::encode_message(&[HostInstruction {
            hoststring,
            width: 1000,
            height: 70,
            echo_ack_num: -1,
        }]);
        server.set_pending(diff);
        let datagrams = server.tick();
        assert!(
            datagrams.len() > 512,
            "fixture must cross the replay-window-sized fragment count"
        );
        for chunk in datagrams.chunks(64) {
            for datagram in chunk {
                socket.send_to(datagram, client_addr).unwrap();
            }
            client.poll().unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        while client.remote_state_num() == 0 && Instant::now() < deadline {
            client.poll().unwrap();
            thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(client.remote_state_num(), 1);
        let frame = client.remote_framebuffer();
        let mut painted_x = 0;
        for y in 0..frame.rows {
            for x in 0..frame.cols {
                if frame.cell_at(x, y).is_some_and(|cell| cell.ch == 'X') {
                    painted_x += 1;
                }
            }
        }
        assert!(
            painted_x >= 67_000,
            "large state was truncated: {painted_x}"
        );
    }

    #[test]
    fn initial_timeout_finishes_the_stock_shutdown_handshake_before_exit() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let key = "AAAAAAAAAAAAAAAAAAAAAA";
        let mut server = Transport::new_server(Ocb::from_base64(key).unwrap());
        let mut client = Client::dial("127.0.0.1", port, key).expect("dial");
        let mut wire = [0u8; 4096];

        let (len, client_addr) = socket.recv_from(&mut wire).expect("initial client state");
        let initial = server
            .recv_state(&wire[..len])
            .expect("initial state should decode");
        assert_eq!(initial.new_num, 1);

        client
            .transport
            .backdate_last_remote_state_for_test(INITIAL_CONNECT_TIMEOUT + Duration::from_secs(1));
        client.poll().expect("start timeout shutdown");

        assert!(!client.is_dead(), "shutdown ACK must be given a chance");
        assert!(client.transport.shutdown_in_progress());

        let shutdown = loop {
            let (len, _) = socket.recv_from(&mut wire).expect("shutdown client state");
            if let Some(state) = server.recv_state(&wire[..len])
                && state.new_num == u64::MAX
            {
                break state;
            }
        };
        assert_eq!(shutdown.new_num, u64::MAX);

        for datagram in server.tick() {
            socket.send_to(&datagram, client_addr).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while !client.is_dead() && Instant::now() < deadline {
            client.poll().expect("finish timeout shutdown");
            thread::sleep(Duration::from_millis(5));
        }

        assert!(client.is_dead());
        assert_eq!(
            client.dead_reason(),
            Some("mosh did not hear from the server (initial connection timeout)")
        );
    }

    #[test]
    fn prediction_starts_with_stock_two_hundred_fifty_ms_send_interval() {
        let client = Client::dial("127.0.0.1", 9, "AAAAAAAAAAAAAAAAAAAAAA").expect("dial");
        assert_eq!(client.send_interval(), Duration::from_millis(250));
    }

    #[test]
    fn graceful_shutdown_waits_for_peer_acknowledgement() {
        let (port, key, _handle) = spawn_echo_server("shutdown");
        let mut client = Client::dial("127.0.0.1", port, &key).expect("dial");

        let deadline = Instant::now() + Duration::from_secs(2);
        while !client.transport.has_received_authenticated() && Instant::now() < deadline {
            let _ = client.poll().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(client.transport.has_received_authenticated());
        assert!(
            client
                .graceful_shutdown(Duration::from_secs(1))
                .expect("shutdown")
        );
    }

    #[test]
    fn graceful_shutdown_includes_keys_queued_immediately_before_exit() {
        let key = [41u8; 16];
        let key_b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(key)
                .trim_end_matches('=')
                .to_string()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let server_received = received.clone();
        let server = thread::spawn(move || {
            let mut transport = Transport::new_server(Ocb::new(&key).unwrap());
            let mut wire = [0u8; 4096];
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut client_addr = None;
            while Instant::now() < deadline {
                if let Ok((n, addr)) = socket.recv_from(&mut wire) {
                    client_addr = Some(addr);
                    if let Some(diff) = transport.recv(&wire[..n]) {
                        for instruction in
                            UserInstruction::decode_message(&diff).unwrap_or_default()
                        {
                            if !instruction.keys.is_empty() {
                                *server_received.lock().unwrap() = instruction.keys;
                            }
                        }
                    }
                }
                if let Some(addr) = client_addr {
                    for datagram in transport.tick() {
                        socket.send_to(&datagram, addr).unwrap();
                    }
                }
                if transport.ack_num() == u64::MAX {
                    break;
                }
            }
        });

        let mut client = Client::dial("127.0.0.1", port, &key_b64).unwrap();
        client.send_keys(b"FINAL_BEFORE_EXIT");
        assert!(
            client
                .graceful_shutdown(Duration::from_secs(2))
                .expect("shutdown")
        );
        server.join().unwrap();
        assert_eq!(&*received.lock().unwrap(), b"FINAL_BEFORE_EXIT");
    }

    #[test]
    fn later_keystrokes_are_sent_before_the_previous_state_is_acked() {
        let key = [23u8; 16];
        let key_b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(key)
                .trim_end_matches('=')
                .to_string()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut server = Transport::new_server(Ocb::new(&key).unwrap());
        let mut client = Client::dial("127.0.0.1", port, &key_b64).unwrap();
        let mut wire = [0u8; 4096];

        let (n, client_addr) = socket.recv_from(&mut wire).expect("initial client state");
        let _ = server.recv(&wire[..n]);
        server.force_next_send();
        for datagram in server.tick() {
            socket.send_to(&datagram, client_addr).unwrap();
        }
        for _ in 0..20 {
            let _ = client.poll().unwrap();
            if client.acked_by_remote() > 0 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        let first_batch = vec![b'a'; 1_200];
        client.send_keys(&first_batch);
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut first_keys = Vec::new();
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            let _ = client.poll().unwrap();
            if let Ok((n, _)) = socket.recv_from(&mut wire)
                && let Some(diff) = server.recv(&wire[..n])
            {
                first_keys = UserInstruction::decode_message(&diff)
                    .unwrap()
                    .into_iter()
                    .flat_map(|instruction| instruction.keys)
                    .collect();
                if first_keys.len() == first_batch.len() {
                    break;
                }
            }
        }
        assert_eq!(first_keys, first_batch);

        // Deliberately withhold the ACK for the large first batch. Stock mosh
        // uses that recently sent state as a prospective receiver base, so the
        // second state contains only the new byte rather than the whole input.
        client.send_keys(b"b");
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut second_keys = Vec::new();
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            let _ = client.poll().unwrap();
            if let Ok((n, _)) = socket.recv_from(&mut wire)
                && let Some(diff) = server.recv(&wire[..n])
            {
                second_keys = UserInstruction::decode_message(&diff)
                    .unwrap()
                    .into_iter()
                    .flat_map(|instruction| instruction.keys)
                    .collect();
                if second_keys.contains(&b'b') {
                    break;
                }
            }
        }
        assert_eq!(
            second_keys, b"b",
            "a recently sent state should be used as the prospective base"
        );
    }

    #[test]
    fn latest_input_keeps_sending_after_thirty_two_unacknowledged_states() {
        let key = [29u8; 16];
        let key_b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(key)
                .trim_end_matches('=')
                .to_string()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut server = Transport::new_server(Ocb::new(&key).unwrap());
        let mut client = Client::dial("127.0.0.1", port, &key_b64).unwrap();
        let mut wire = [0u8; 16384];

        let (n, client_addr) = socket.recv_from(&mut wire).expect("initial client state");
        let _ = server.recv(&wire[..n]);
        let mut user_states = HashMap::from([(server.ack_num(), Vec::new())]);
        server.force_next_send();
        for datagram in server.tick() {
            socket.send_to(&datagram, client_addr).unwrap();
        }
        for _ in 0..20 {
            let _ = client.poll().unwrap();
            if client.acked_by_remote() > 0 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        let expected = (0..40).map(|i| b'a' + (i % 26) as u8).collect::<Vec<_>>();
        let mut latest = Vec::new();
        for byte in &expected {
            client.send_keys(&[*byte]);
            thread::sleep(Duration::from_millis(25));
            let _ = client.poll().unwrap();
            while let Ok((n, _)) = socket.recv_from(&mut wire) {
                if let Some(state) = server.recv_state(&wire[..n]) {
                    latest = apply_user_state(&mut user_states, state);
                }
            }
        }

        assert!(client.sent_num() > 32);
        assert_eq!(
            latest, expected,
            "new input stopped after the send window filled"
        );
    }

    #[test]
    fn early_ack_after_send_window_overflow_preserves_later_input() {
        let key = [30u8; 16];
        let key_b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(key)
                .trim_end_matches('=')
                .to_string()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut server = Transport::new_server(Ocb::new(&key).unwrap());
        let mut client = Client::dial("127.0.0.1", port, &key_b64).unwrap();
        let mut wire = [0u8; 16384];

        let (n, client_addr) = socket.recv_from(&mut wire).expect("initial client state");
        let _ = server.recv(&wire[..n]);
        let mut user_states = HashMap::from([(server.ack_num(), Vec::new())]);
        server.force_next_send();
        for datagram in server.tick() {
            socket.send_to(&datagram, client_addr).unwrap();
        }
        for _ in 0..20 {
            let _ = client.poll().unwrap();
            if client.acked_by_remote() > 0 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        while socket.recv_from(&mut wire).is_ok() {}

        let expected = (0..40).map(|i| b'A' + (i % 26) as u8).collect::<Vec<_>>();
        let mut delivered_prefix = Vec::new();
        for byte in &expected {
            client.send_keys(&[*byte]);
            thread::sleep(Duration::from_millis(25));
            let _ = client.poll().unwrap();
            while let Ok((n, _)) = socket.recv_from(&mut wire) {
                if delivered_prefix.is_empty()
                    && let Some(state) = server.recv_state(&wire[..n])
                {
                    delivered_prefix = apply_user_state(&mut user_states, state);
                }
                // Drop every later client state to model a one-way path that
                // delivered only the first input before the send queue filled.
            }
        }
        assert!(!delivered_prefix.is_empty());
        assert!(client.sent_num() > 32);

        server.force_next_send();
        for datagram in server.tick() {
            socket.send_to(&datagram, client_addr).unwrap();
        }
        let ack_deadline = Instant::now() + Duration::from_secs(1);
        while client.acked_by_remote() < server.ack_num() && Instant::now() < ack_deadline {
            let _ = client.poll().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        client.flush_ticks().unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut latest = delivered_prefix.clone();
        while Instant::now() < deadline && latest != expected {
            let _ = client.poll().unwrap();
            while let Ok((n, _)) = socket.recv_from(&mut wire) {
                if let Some(state) = server.recv_state(&wire[..n]) {
                    latest = apply_user_state(&mut user_states, state);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(
            latest, expected,
            "partial ACK discarded cumulative input after send-window overflow"
        );
    }

    #[test]
    fn remote_shutdown_state_keeps_its_final_host_frame() {
        let key = [31u8; 16];
        let key_b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(key)
                .trim_end_matches('=')
                .to_string()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut server = Transport::new_server(Ocb::new(&key).unwrap());
        let mut client = Client::dial("127.0.0.1", port, &key_b64).unwrap();
        let mut wire = [0u8; 4096];

        let (n, client_addr) = socket.recv_from(&mut wire).expect("initial client state");
        let _ = server.recv(&wire[..n]);
        let final_frame = HostInstruction::encode_message(&[HostInstruction {
            hoststring: b"\x1b[HFINAL".to_vec(),
            width: 0,
            height: 0,
            echo_ack_num: -1,
        }]);
        server.set_pending(final_frame);
        server.start_shutdown();
        for datagram in server.tick() {
            socket.send_to(&datagram, client_addr).unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(1);
        while client.remote_state_num() != u64::MAX && Instant::now() < deadline {
            let _ = client.poll().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        let framebuffer = client.remote_framebuffer();
        let rendered = (0..5)
            .map(|x| framebuffer.cell_at(x, 0).unwrap().ch)
            .collect::<String>();
        assert_eq!(rendered, "FINAL");

        thread::sleep(Duration::from_millis(10));
        let _ = client.poll().unwrap();
        assert!(client.remote_shutdown_ack_sent());
    }
}
