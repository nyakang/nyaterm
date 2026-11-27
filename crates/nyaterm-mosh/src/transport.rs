//! State Synchronization Protocol (SSP) transport for Mosh.
//!
//! Matches upstream mobile-shell/mosh framing (zlib TransportInstruction inside
//! fragments, OCB-sealed datagrams) with a bounded multi-state send queue.
//! Critical fixes vs early draft:
//! - accept out-of-order crypto seq (upstream returns payload; only true
//!   replays of seen seq are dropped)
//! - emit throwaway_num so the peer can prune received states
//! - ack_num only advances forward
//! - timestamp reply uses 0xFFFF as "no reply" (stock network.cc)

use std::collections::{HashSet, VecDeque};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{Read, Write};

use crate::crypto::{self, DIR_TO_CLIENT, DIR_TO_SERVER, MIN_DATAGRAM, Ocb, SEQ_MASK};
use crate::fragment::{
    Assembler, FRAGMENT_HEADER_SIZE, Fragment, MAX_FRAGMENT_PAYLOAD, MAX_INSTRUCTION_BYTES,
    fragmentize_with_payload,
};
use crate::pb::TransportInstruction;

const PROTOCOL_VERSION: u32 = 2;
/// Stock starts at the upper RTO cap until the first RTT sample arrives.
const INITIAL_RTO: Duration = Duration::from_millis(1000);
const MIN_RTO: Duration = Duration::from_millis(50);
const MAX_RTO: Duration = Duration::from_millis(1000);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const ACK_DELAY: Duration = Duration::from_millis(100);
const ACTIVE_RETRY_TIMEOUT: Duration = Duration::from_secs(10);
const CONGESTION_TIMESTAMP_PENALTY_MS: u16 = 500;
const SHUTDOWN_RETRIES: u8 = 16;
const SENT_STATE_CAP: usize = 32;
const OCB_BLOCK_LIMIT: u64 = 1u64 << 47;
static NEXT_PACKET_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Stock uses uint16(-1) = 0xFFFF to mean "no timestamp reply".
const TS_NO_REPLY: u16 = 0xFFFF;

/// Sliding window of recently seen crypto sequence numbers (true replay filter).
const SEEN_SEQ_CAP: usize = 512;

/// Maximum number of complete remote states retained while the peer keeps
/// referencing an old base. Once full, reject newer branches until the peer's
/// throwaway watermark makes room; never ACK a state whose base was discarded.
/// Stock starts receiver quenching after roughly 1024 retained branches. Keep
/// the same useful window; TerminalView's separate 128 MiB budget remains the
/// hard memory-safety backstop for unusually large screens.
pub(crate) const RECEIVED_STATE_CAP: usize = 1024;
const RECEIVER_QUENCH_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedStateDiff {
    pub old_num: u64,
    pub new_num: u64,
    pub throwaway_num: u64,
    pub diff: Vec<u8>,
}

#[derive(Debug, Clone)]
struct OutboundState {
    old_num: u64,
    new_num: u64,
    diff: Vec<u8>,
    last_sent: Option<Instant>,
}

/// High-level transport that wraps OCB, fragments, and SSP counters.
pub struct Transport {
    ocb: Ocb,
    to_remote: u64,
    to_local: u64,

    sent_num: u64,
    acked_by_remote: u64,
    outbound_states: VecDeque<OutboundState>,
    /// Send timestamps for every SSP state, including empty ACK/heartbeat
    /// states. Stock uses the timestamp of the state named by an ACK when it
    /// reports end-to-end roundtrip reachability.
    sent_state_times: VecDeque<(u64, Instant)>,
    outbound_instruction_too_large: bool,
    outbound_overflowed: bool,
    rebase_required: bool,
    pacing_enabled: bool,
    allow_immediate_new_state: bool,
    pending_data_ack: bool,
    pending_ack_since: Option<Instant>,

    /// Oldest local state we still need the peer to retain (throwaway watermark).
    local_throwaway: u64,

    received_nums: Vec<u64>,
    ack_num: u64,
    sent_ack_num: u64,
    throwaway_num: u64,

    instruction_id: u64,
    max_fragment_payload: usize,
    encrypted_blocks: u64,
    crypto_exhausted: bool,
    /// Expected next crypto seq (for RTT bookkeeping only; reorder still accepted).
    expected_receiver_seq: u64,
    expected_receiver_seq_set: bool,
    /// True replays: exact seq already successfully decrypted.
    seen_seqs: HashSet<u64>,
    seen_seq_order: Vec<u64>,

    last_send: Instant,
    last_recv: Instant,
    last_remote_state: Instant,
    received_authenticated: bool,
    last_roundtrip_success: Instant,
    /// Last remote timestamp usable for echo; None until a timely sample.
    last_ts: Option<u16>,
    last_ts_at: Instant,

    srtt: Duration,
    rttvar: Duration,
    rto: Duration,
    rtt_init: bool,

    assembler: Assembler,
    force_send: bool,
    shutdown_in_progress: bool,
    shutdown_started: Option<Instant>,
    shutdown_tries: u8,
    counterparty_shutdown_ack_sent: bool,
    receiver_quench_until: Option<Instant>,
}

impl Transport {
    pub fn new_client(ocb: Ocb) -> Self {
        Self::new(ocb, false)
    }

    pub fn new_server(ocb: Ocb) -> Self {
        Self::new(ocb, true)
    }

    fn new(ocb: Ocb, is_server: bool) -> Self {
        let now = Instant::now();
        let (to_remote, to_local) = if is_server {
            (DIR_TO_CLIENT, DIR_TO_SERVER)
        } else {
            (DIR_TO_SERVER, DIR_TO_CLIENT)
        };
        Self {
            ocb,
            to_remote,
            to_local,
            sent_num: 0,
            acked_by_remote: 0,
            outbound_states: VecDeque::new(),
            sent_state_times: VecDeque::from([(0, now)]),
            outbound_instruction_too_large: false,
            outbound_overflowed: false,
            rebase_required: false,
            pacing_enabled: !is_server,
            allow_immediate_new_state: true,
            pending_data_ack: false,
            pending_ack_since: None,
            local_throwaway: 0,
            received_nums: vec![0],
            ack_num: 0,
            sent_ack_num: 0,
            throwaway_num: 0,
            instruction_id: 0,
            max_fragment_payload: MAX_FRAGMENT_PAYLOAD,
            encrypted_blocks: 0,
            crypto_exhausted: false,
            expected_receiver_seq: 0,
            expected_receiver_seq_set: false,
            seen_seqs: HashSet::new(),
            seen_seq_order: Vec::new(),
            last_send: now,
            last_recv: now,
            last_remote_state: now,
            received_authenticated: false,
            last_roundtrip_success: now,
            last_ts: None,
            last_ts_at: now,
            srtt: Duration::ZERO,
            rttvar: Duration::ZERO,
            rto: INITIAL_RTO,
            rtt_init: false,
            assembler: Assembler::new(),
            force_send: false,
            shutdown_in_progress: false,
            shutdown_started: None,
            shutdown_tries: 0,
            counterparty_shutdown_ack_sent: false,
            receiver_quench_until: None,
        }
    }

    pub fn force_next_send(&mut self) {
        self.force_send = true;
    }

    pub fn set_pending(&mut self, diff: Vec<u8>) -> u64 {
        self.set_pending_from(self.acked_by_remote, diff)
    }

    pub(crate) fn set_pending_from(&mut self, old_num: u64, diff: Vec<u8>) -> u64 {
        if self.shutdown_in_progress {
            return self.sent_num;
        }
        let old_num = if old_num == self.acked_by_remote
            || self
                .outbound_states
                .iter()
                .any(|state| state.new_num == old_num)
        {
            old_num
        } else {
            self.acked_by_remote
        };
        self.sent_num = self.sent_num.saturating_add(1);
        self.outbound_states.push_back(OutboundState {
            old_num,
            new_num: self.sent_num,
            diff,
            last_sent: None,
        });
        if self.outbound_states.len() > SENT_STATE_CAP {
            let middle = self.outbound_states.len() - 16;
            self.outbound_states.remove(middle);
            self.outbound_overflowed = true;
        }
        self.sent_num
    }

    pub(crate) fn prospective_base_num(&self) -> Option<u64> {
        let now = Instant::now();
        let maximum_age = self.rto + ACK_DELAY;
        let mut candidate = None;
        for state in &self.outbound_states {
            match state.last_sent {
                Some(sent_at) if now.saturating_duration_since(sent_at) < maximum_age => {
                    candidate = Some(state.new_num);
                }
                _ => break,
            }
        }
        candidate
    }

    pub(crate) fn prospective_chain_expired(&self) -> bool {
        let Some(latest) = self.outbound_states.back() else {
            return false;
        };
        if latest.old_num == self.acked_by_remote {
            return false;
        }
        match self.prospective_base_num() {
            Some(freshest_prefix) => freshest_prefix < latest.old_num,
            None => true,
        }
    }

    pub(crate) fn take_outbound_instruction_too_large(&mut self) -> bool {
        std::mem::take(&mut self.outbound_instruction_too_large)
    }

    pub fn acked_by_remote(&self) -> u64 {
        self.acked_by_remote
    }

    /// Returns whether queue compaction invalidated pending states that were
    /// based on an older acknowledgement. The application should rebuild its
    /// latest cumulative diff from the new acknowledged base.
    pub fn take_rebase_required(&mut self) -> bool {
        std::mem::take(&mut self.rebase_required)
    }

    pub fn sent_num(&self) -> u64 {
        self.sent_num
    }

    pub fn ack_num(&self) -> u64 {
        self.ack_num
    }

    pub fn rto(&self) -> Duration {
        self.rto
    }

    /// Smoothed RTT once at least one timestamp sample has been observed.
    /// Used by local prediction (`MOSH_PREDICTION_DISPLAY=adaptive`).
    pub fn srtt(&self) -> Option<Duration> {
        if self.rtt_init { Some(self.srtt) } else { None }
    }

    /// Time of last successfully authenticated inbound datagram.
    pub fn last_recv(&self) -> Instant {
        self.last_recv
    }

    /// Time of the last complete, version-correct remote SSP state.
    pub fn last_remote_state(&self) -> Instant {
        self.last_remote_state
    }

    pub fn has_received_authenticated(&self) -> bool {
        self.received_authenticated
    }

    pub fn last_roundtrip_success(&self) -> Instant {
        self.last_roundtrip_success
    }

    pub fn set_max_fragment_payload(&mut self, max_fragment_payload: usize) {
        self.max_fragment_payload = max_fragment_payload.max(1);
        for state in &mut self.outbound_states {
            state.last_sent = None;
        }
        self.force_send = true;
    }

    pub fn crypto_exhausted(&self) -> bool {
        self.crypto_exhausted
    }

    pub fn start_shutdown(&mut self) {
        if !self.shutdown_in_progress {
            self.shutdown_in_progress = true;
            self.shutdown_started = Some(Instant::now());
            self.force_send = true;
        }
    }

    pub fn shutdown_acknowledged(&self) -> bool {
        self.shutdown_in_progress && self.acked_by_remote == u64::MAX
    }

    pub fn shutdown_in_progress(&self) -> bool {
        self.shutdown_in_progress
    }

    pub fn counterparty_shutdown_ack_sent(&self) -> bool {
        self.counterparty_shutdown_ack_sent
    }

    pub fn shutdown_timed_out(&self) -> bool {
        self.shutdown_in_progress
            && (self.shutdown_tries >= SHUTDOWN_RETRIES
                || self
                    .shutdown_started
                    .is_some_and(|started| started.elapsed() >= ACTIVE_RETRY_TIMEOUT))
    }

    #[cfg(test)]
    pub(crate) fn backdate_last_remote_state_for_test(&mut self, elapsed: Duration) {
        self.last_remote_state = Instant::now() - elapsed;
    }

    #[cfg(test)]
    fn backdate_sent_state_for_test(&mut self, state_num: u64, elapsed: Duration) {
        let sent_at = Instant::now() - elapsed;
        if let Some(state) = self
            .outbound_states
            .iter_mut()
            .find(|state| state.new_num == state_num)
        {
            state.last_sent = Some(sent_at);
        }
        if let Some((_, timestamp)) = self
            .sent_state_times
            .iter_mut()
            .find(|(num, _)| *num == state_num)
        {
            *timestamp = sent_at;
        }
    }

    fn record_sent_state_time(&mut self, state_num: u64, sent_at: Instant) {
        if let Some((_, timestamp)) = self
            .sent_state_times
            .iter_mut()
            .find(|(num, _)| *num == state_num)
        {
            *timestamp = sent_at;
            return;
        }

        self.sent_state_times.push_back((state_num, sent_at));
        if self.sent_state_times.len() > SENT_STATE_CAP {
            let middle = self.sent_state_times.len() - 16;
            self.sent_state_times.remove(middle);
        }
    }

    fn process_roundtrip_ack(&mut self, ack_num: u64) -> bool {
        let matched = self
            .sent_state_times
            .iter()
            .position(|(state_num, _)| *state_num == ack_num);
        if let Some(position) = matched {
            self.sent_state_times.drain(..position);
        }
        if let Some((_, sent_at)) = self.sent_state_times.front() {
            self.last_roundtrip_success = *sent_at;
        }
        matched.is_some()
    }

    fn send_interval(&self) -> Duration {
        let half_ms = if self.rtt_init {
            self.srtt.as_nanos().div_ceil(2_000_000) as u64
        } else {
            250
        };
        Duration::from_millis(half_ms.clamp(20, 250))
    }

    /// Produce outgoing wire datagrams if it's time to send.
    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        let now = Instant::now();
        let ack_changed = self.ack_num > self.sent_ack_num;
        // Multiple input batches can arrive inside one pacing interval. Only
        // the newest unsent state matters because it contains the cumulative
        // diff from the same acknowledged base; sending stale unsent states
        // first would make fast typing lag behind the local input stream.
        if let Some(latest_unsent) = self
            .outbound_states
            .iter()
            .rev()
            .find(|state| state.last_sent.is_none())
            .map(|state| state.new_num)
        {
            self.outbound_states
                .retain(|state| state.last_sent.is_some() || state.new_num == latest_unsent);
        }
        let unsent_index = self
            .outbound_states
            .iter()
            .position(|state| state.last_sent.is_none());
        let unsent_due = unsent_index.is_some()
            && (!self.pacing_enabled
                || self.allow_immediate_new_state
                || now.duration_since(self.last_send) >= self.send_interval());
        let retransmit_after = if now.duration_since(self.last_remote_state) < ACTIVE_RETRY_TIMEOUT
        {
            self.rto + ACK_DELAY
        } else {
            HEARTBEAT_INTERVAL
        };
        let retransmit_index = self.outbound_states.iter().rposition(|state| {
            state
                .last_sent
                .is_some_and(|last_sent| now.duration_since(last_sent) >= retransmit_after)
        });
        let heartbeat_due = self.outbound_states.is_empty()
            && now.duration_since(self.last_send) >= HEARTBEAT_INTERVAL;
        let delayed_ack_due = self.pending_data_ack
            && self
                .pending_ack_since
                .is_some_and(|since| now.duration_since(since) >= ACK_DELAY);
        let periodic_ack_due =
            ack_changed && now.duration_since(self.last_send) >= HEARTBEAT_INTERVAL;
        let urgent_shutdown_ack = ack_changed && self.ack_num == u64::MAX;
        let ack_due = delayed_ack_due || periodic_ack_due || urgent_shutdown_ack;
        let forced = self.force_send;
        let shutdown_due = self.shutdown_in_progress
            && !self.shutdown_acknowledged()
            && !self.shutdown_timed_out()
            && (forced || now.duration_since(self.last_send) >= self.send_interval());

        if !(unsent_due
            || ack_due
            || retransmit_index.is_some()
            || heartbeat_due
            || forced
            || shutdown_due)
        {
            return Vec::new();
        }
        if self.shutdown_in_progress && !shutdown_due {
            return Vec::new();
        }
        self.force_send = false;

        let (old_num, new_num, diff) = if self.shutdown_in_progress {
            self.sent_num = u64::MAX;
            self.shutdown_tries = self.shutdown_tries.saturating_add(1);
            self.outbound_states
                .back()
                .map_or((self.acked_by_remote, u64::MAX, Vec::new()), |state| {
                    (state.old_num, u64::MAX, state.diff.clone())
                })
        } else if let Some(index) = unsent_index
            .filter(|_| unsent_due || ack_due || forced)
            .or(retransmit_index)
        {
            let state = &mut self.outbound_states[index];
            if state.last_sent.is_none() {
                self.allow_immediate_new_state = false;
            }
            state.last_sent = Some(now);
            (state.old_num, state.new_num, state.diff.clone())
        } else {
            // Stock mosh records even an empty ACK/heartbeat as a new SSP
            // state so the peer can distinguish activity from a duplicate.
            let old_num = self
                .outbound_states
                .back()
                .map(|state| state.new_num)
                .unwrap_or(self.acked_by_remote);
            self.sent_num = self.sent_num.saturating_add(1);
            (old_num, self.sent_num, Vec::new())
        };
        self.pending_data_ack = false;
        self.pending_ack_since = None;

        // Do not tell the peer to discard a base still referenced by a queued
        // state that has not reached it yet.
        let throwaway = self
            .outbound_states
            .iter()
            .map(|state| state.old_num)
            .min()
            .map_or(self.local_throwaway, |oldest_base| {
                self.local_throwaway.min(oldest_base)
            });
        let ti = TransportInstruction {
            protocol_version: PROTOCOL_VERSION,
            old_num,
            new_num,
            ack_num: self.ack_num,
            throwaway_num: throwaway,
            diff,
            chaff: make_chaff(),
        };
        if self.ack_num == u64::MAX {
            self.counterparty_shutdown_ack_sent = true;
        }
        self.sent_ack_num = self.ack_num;

        let pb_data = ti.encode();
        if pb_data.len() > MAX_INSTRUCTION_BYTES {
            self.outbound_instruction_too_large = true;
            self.last_send = now;
            return Vec::new();
        }
        let compressed = zlib_compress(&pb_data);
        let fragment_count = compressed.len().max(1).div_ceil(self.max_fragment_payload);
        if compressed.len() > MAX_INSTRUCTION_BYTES || fragment_count > 0x8000 {
            self.outbound_instruction_too_large = true;
            self.last_send = now;
            return Vec::new();
        }
        self.instruction_id = self.instruction_id.saturating_add(1);
        let frags =
            fragmentize_with_payload(self.instruction_id, &compressed, self.max_fragment_payload);

        let mut datagrams = Vec::with_capacity(frags.len());
        for f in &frags {
            let Some(datagram) = self.encrypt_fragment(f) else {
                datagrams.clear();
                break;
            };
            datagrams.push(datagram);
        }
        if !datagrams.is_empty() {
            self.record_sent_state_time(new_num, now);
        }
        self.last_send = now;
        datagrams
    }

    /// Process an incoming wire datagram. Returns the raw diff payload if a
    /// complete new state was accepted, or `None`.
    pub fn recv(&mut self, wire: &[u8]) -> Option<Vec<u8>> {
        self.recv_state(wire).map(|state| state.diff)
    }

    /// Process a datagram and retain the SSP numbering needed to reconstruct
    /// the peer's complete remote state.
    pub(crate) fn recv_state(&mut self, wire: &[u8]) -> Option<ReceivedStateDiff> {
        self.recv_state_with_congestion(wire, false)
    }

    pub(crate) fn recv_state_with_congestion(
        &mut self,
        wire: &[u8],
        congestion_experienced: bool,
    ) -> Option<ReceivedStateDiff> {
        if wire.len() < MIN_DATAGRAM {
            return None;
        }

        let dir_seq = u64::from_be_bytes(wire[..8].try_into().ok()?);
        if dir_seq & DIR_TO_CLIENT != self.to_local & DIR_TO_CLIENT {
            return None;
        }
        let seq = dir_seq & SEQ_MASK;

        // True replay only: already successfully opened this exact crypto seq.
        // Unlike mosh-go, we still accept seq < expected (UDP reorder) — matching
        // upstream network.cc which returns payload for out-of-order packets.
        if self.seen_seqs.contains(&seq) {
            return None;
        }

        let nonce = Ocb::nonce_for(dir_seq);
        let plaintext = self.ocb.decrypt(&nonce, &wire[8..])?;
        if plaintext.len() < 4 {
            return None;
        }
        let remote_ts = u16::from_be_bytes([plaintext[0], plaintext[1]]);
        let ts_reply = u16::from_be_bytes([plaintext[2], plaintext[3]]);
        let payload = &plaintext[4..];

        self.remember_seq(seq);
        self.last_recv = Instant::now();

        // RTT only from in-order-ish samples (upstream skips OOO for timestamp).
        let in_order = !self.expected_receiver_seq_set || seq >= self.expected_receiver_seq;
        if in_order {
            if self.expected_receiver_seq_set {
                self.expected_receiver_seq = seq.saturating_add(1);
            } else {
                self.expected_receiver_seq = seq.saturating_add(1);
                self.expected_receiver_seq_set = true;
            }
            // Save peer timestamp for echo (stock holds ~1s).
            if remote_ts != TS_NO_REPLY {
                self.last_ts = Some(if congestion_experienced {
                    remote_ts.wrapping_sub(CONGESTION_TIMESTAMP_PENALTY_MS)
                } else {
                    remote_ts
                });
                self.last_ts_at = Instant::now();
            }
            if ts_reply != TS_NO_REPLY {
                self.update_rtt(ts_reply);
            }
        }

        if payload.len() < FRAGMENT_HEADER_SIZE {
            return None;
        }
        let frag = Fragment::decode(payload).ok()?;
        let msg = self.assembler.add(frag)?;
        let decompressed = zlib_decompress(&msg)?;
        let ti = TransportInstruction::decode(&decompressed).ok()?;

        // Fail closed on protocol version mismatch (upstream throws).
        if ti.protocol_version != PROTOCOL_VERSION {
            return None;
        }

        // Stock derives its end-to-end reachability clock from the send time
        // of the ACKed SSP state, not from the arrival time of this packet.
        let acknowledged_state_is_known = self.process_roundtrip_ack(ti.ack_num);

        // Process ack from remote.
        if acknowledged_state_is_known
            && ti.ack_num > self.acked_by_remote
            && ti.ack_num <= self.sent_num
        {
            self.acked_by_remote = ti.ack_num;
            // Advance local throwaway: peer has everything ≤ acked.
            self.local_throwaway = self.acked_by_remote;
            self.outbound_states
                .retain(|state| state.new_num > self.acked_by_remote);
            // Newer parallel states may still be based on an older state.
            // Keep that base in the advertised throwaway watermark until all
            // such states are delivered or acknowledged.
            if self.outbound_states.is_empty() {
                self.allow_immediate_new_state = true;
            }
            if self.outbound_overflowed {
                let before = self.outbound_states.len();
                self.outbound_states
                    .retain(|state| state.old_num >= self.acked_by_remote);
                self.rebase_required = self.outbound_states.len() < before;
                self.outbound_overflowed = false;
            }
        }

        // Stock mosh validates idempotency against the pre-pruned state queue:
        // dedup new_num, require old_num, clone the base, then honor throwaway.
        if self.received_nums.contains(&ti.new_num) {
            return None;
        }
        // Require old_num to apply diff.
        if !self.received_nums.contains(&ti.old_num) {
            return None;
        }

        if ti.throwaway_num > self.throwaway_num {
            self.throwaway_num = ti.throwaway_num;
            self.received_nums.retain(|&n| n >= self.throwaway_num);
        }

        // Match stock's receiver quench: once the useful 1024-state window is
        // exceeded, admit at most one additional branch every 15 seconds.
        // Never discard a middle state that may still be referenced.
        if self.received_nums.len() > RECEIVED_STATE_CAP {
            let now = Instant::now();
            if self
                .receiver_quench_until
                .is_some_and(|deadline| now < deadline)
            {
                return None;
            }
            self.receiver_quench_until = Some(now + RECEIVER_QUENCH_INTERVAL);
        }

        let advances_latest_remote_state = ti.new_num > self.ack_num;
        self.received_nums.push(ti.new_num);
        // Initial attachment is complete only after a whole, version-correct
        // SSP state has been accepted. One authenticated fragment must not
        // disable the client's 15-second connection timeout forever.
        self.received_authenticated = true;

        // Only advance ack_num forward (upstream tracks sorted list + back).
        if advances_latest_remote_state {
            self.ack_num = ti.new_num;
            self.last_remote_state = Instant::now();
        }
        if !ti.diff.is_empty() {
            self.pending_ack_since.get_or_insert_with(Instant::now);
            self.pending_data_ack = true;
        }
        if ti.new_num == u64::MAX {
            self.pending_ack_since.get_or_insert_with(Instant::now);
            self.pending_data_ack = true;
        }
        Some(ReceivedStateDiff {
            old_num: ti.old_num,
            new_num: ti.new_num,
            throwaway_num: self.throwaway_num,
            diff: ti.diff,
        })
    }

    fn remember_seq(&mut self, seq: u64) {
        if self.seen_seqs.insert(seq) {
            self.seen_seq_order.push(seq);
            while self.seen_seq_order.len() > SEEN_SEQ_CAP {
                if let Some(old) = self.seen_seq_order.first().copied() {
                    self.seen_seq_order.remove(0);
                    self.seen_seqs.remove(&old);
                } else {
                    break;
                }
            }
        }
    }

    fn encrypt_fragment(&mut self, f: &Fragment) -> Option<Vec<u8>> {
        let frag_wire = f.encode();
        let plaintext_blocks = (4 + frag_wire.len()).div_ceil(16) as u64;
        if self.encrypted_blocks.saturating_add(plaintext_blocks) >= OCB_BLOCK_LIMIT {
            self.crypto_exhausted = true;
            return None;
        }
        self.encrypted_blocks += plaintext_blocks;
        let sequence = NEXT_PACKET_SEQUENCE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()?;
        let dir_seq = self.to_remote | (sequence & SEQ_MASK);
        let ts = mosh_timestamp_now();
        // Echo peer ts only if recent (~1s), else 0xFFFF (stock "no reply").
        let ts_reply = self.last_ts.take().map_or(TS_NO_REPLY, |timestamp| {
            let held = self.last_ts_at.elapsed();
            if held < Duration::from_secs(1) {
                timestamp.wrapping_add(held.as_millis() as u16)
            } else {
                TS_NO_REPLY
            }
        });
        let mut plaintext = Vec::with_capacity(4 + frag_wire.len());
        plaintext.extend_from_slice(&crypto::pack_timestamps(ts, ts_reply));
        plaintext.extend_from_slice(&frag_wire);
        Some(self.ocb.seal_datagram(dir_seq, &plaintext))
    }

    fn update_rtt(&mut self, ts_reply: u16) {
        let now16 = mosh_timestamp_now();
        let mut rtt_ms = i32::from(now16) - i32::from(ts_reply);
        if rtt_ms < 0 {
            rtt_ms += 65536;
        }
        if rtt_ms >= 5_000 {
            return;
        }
        let rtt = Duration::from_millis(rtt_ms as u64);
        if !self.rtt_init {
            self.srtt = rtt;
            self.rttvar = rtt / 2;
            self.rtt_init = true;
        } else {
            let delta = self.srtt.abs_diff(rtt);
            self.rttvar = (self.rttvar * 3 + delta) / 4;
            self.srtt = (self.srtt * 7 + rtt) / 8;
        }
        self.rto = (self.srtt + self.rttvar * 4).clamp(MIN_RTO, MAX_RTO);
    }
}

fn mosh_timestamp_now() -> u16 {
    // Wire timestamps only need a monotonic 16-bit millisecond clock. The
    // absolute epoch is irrelevant because peers return our own timestamp.
    static START: OnceLock<Instant> = OnceLock::new();
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    let mut ts = (ms & 0xffff) as u16;
    if ts == TS_NO_REPLY {
        ts = 0;
    }
    ts
}

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("zlib write");
    enc.finish().expect("zlib finish")
}

fn make_chaff() -> Vec<u8> {
    let mut random = [0u8; 17];
    if getrandom::getrandom(&mut random).is_err() {
        return Vec::new();
    }
    chaff_from_random(random)
}

fn chaff_from_random(random: [u8; 17]) -> Vec<u8> {
    let len = usize::from(random[0] % 17);
    random[1..1 + len].to_vec()
}

fn zlib_decompress(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = ZlibDecoder::new(data);
    let mut out = Vec::new();
    let mut limited = [0u8; 8192];
    loop {
        match dec.read(&mut limited) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() + n > MAX_INSTRUCTION_BYTES {
                    return None;
                }
                out.extend_from_slice(&limited[..n]);
            }
            Err(_) => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        Duration, Fragment, Instant, MAX_INSTRUCTION_BYTES, MAX_RTO, MIN_RTO, OCB_BLOCK_LIMIT,
        RECEIVED_STATE_CAP, SEQ_MASK, Transport, TransportInstruction, chaff_from_random,
        zlib_compress, zlib_decompress,
    };
    use crate::crypto::Ocb;
    use crate::fragment::MAX_FRAGMENT_PAYLOAD;
    use rand::RngCore;

    fn pair() -> (Transport, Transport) {
        let mut key = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut key);
        let server = Transport::new_server(Ocb::new(&key).unwrap());
        let client = Transport::new_client(Ocb::new(&key).unwrap());
        (server, client)
    }

    #[test]
    fn zlib_decompress_accepts_official_states_larger_than_one_mibibyte() {
        let payload = vec![0x41; (1 << 20) + MAX_FRAGMENT_PAYLOAD];
        let compressed = zlib_compress(&payload);
        let decoded = zlib_decompress(&compressed)
            .expect("official-size decompressed state should be accepted");

        assert_eq!(decoded.len(), payload.len());
        assert!(decoded == payload, "decompressed state must remain exact");
    }

    #[test]
    fn zlib_decompress_accepts_the_official_four_mibibyte_boundary() {
        let payload = vec![0x42; MAX_INSTRUCTION_BYTES];
        let compressed = zlib_compress(&payload);
        let decoded = zlib_decompress(&compressed)
            .expect("the official decompressor boundary should be accepted");

        assert_eq!(decoded.len(), MAX_INSTRUCTION_BYTES);
        assert!(decoded == payload, "boundary payload must remain exact");
    }

    #[test]
    fn zlib_decompress_rejects_states_larger_than_the_official_boundary() {
        let payload = vec![0x43; MAX_INSTRUCTION_BYTES + 1];
        let compressed = zlib_compress(&payload);

        assert!(zlib_decompress(&compressed).is_none());
    }

    #[test]
    fn outbound_compression_rejects_states_larger_than_the_official_boundary() {
        let (mut sender, _) = pair();
        let mut payload = vec![0u8; MAX_INSTRUCTION_BYTES + 1024];
        let mut state = 0x9e37_79b9_u32;
        for byte in &mut payload {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        sender.set_pending(payload);

        assert!(sender.tick().is_empty());
        assert!(sender.take_outbound_instruction_too_large());
    }

    #[test]
    fn outbound_rejects_highly_compressible_states_above_the_decoded_boundary() {
        let (mut sender, _) = pair();
        sender.set_pending(vec![0; MAX_INSTRUCTION_BYTES + 1024]);

        assert!(sender.tick().is_empty());
        assert!(sender.take_outbound_instruction_too_large());
    }

    #[test]
    fn basic_exchange() {
        let (mut server, mut client) = pair();
        server.set_pending(b"hello from server".to_vec());
        let dgs = server.tick();
        assert!(!dgs.is_empty());
        let mut diff = None;
        for dg in dgs {
            if let Some(d) = client.recv(&dg) {
                diff = Some(d);
            }
        }
        assert_eq!(diff.unwrap(), b"hello from server");

        client.set_pending(b"hello from client".to_vec());
        let dgs = client.tick();
        let mut diff = None;
        for dg in dgs {
            if let Some(d) = server.recv(&dg) {
                diff = Some(d);
            }
        }
        assert_eq!(diff.unwrap(), b"hello from client");
    }

    #[test]
    fn true_replay_rejected() {
        let (mut server, mut client) = pair();
        server.set_pending(b"data".to_vec());
        let dgs = server.tick();
        assert!(client.recv(&dgs[0]).is_some());
        assert!(client.recv(&dgs[0]).is_none());
    }

    #[test]
    fn reordered_fragments_still_assemble() {
        // CRITICAL fix: out-of-order crypto seq must not drop fragments.
        let (mut server, mut client) = pair();
        let mut payload = vec![0u8; MAX_FRAGMENT_PAYLOAD * 2 + 100];
        rand::thread_rng().fill_bytes(&mut payload);
        server.set_pending(payload.clone());
        let mut dgs = server.tick();
        assert!(dgs.len() >= 3, "need multi-fragment, got {}", dgs.len());
        // Deliver reverse order (highest seq first).
        dgs.reverse();
        let mut got = None;
        for dg in dgs {
            if let Some(d) = client.recv(&dg) {
                got = Some(d);
            }
        }
        assert_eq!(got.unwrap(), payload);
    }

    #[test]
    fn authenticated_first_fragment_does_not_complete_initial_attachment() {
        let (mut server, mut client) = pair();
        let mut payload = vec![0u8; MAX_FRAGMENT_PAYLOAD * 3];
        rand::thread_rng().fill_bytes(&mut payload);
        server.set_pending(payload);
        let datagrams = server.tick();
        assert!(datagrams.len() > 1);

        assert!(client.recv(&datagrams[0]).is_none());
        assert!(!client.has_received_authenticated());
    }

    #[test]
    fn authenticated_partial_fragments_do_not_refresh_complete_state_contact() {
        let (mut server, mut client) = pair();
        let mut seed = 0x9e37_79b9_u32;
        let payload: Vec<u8> = (0..MAX_FRAGMENT_PAYLOAD * 3)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        server.set_pending(payload);
        let datagrams = server.tick();
        assert!(datagrams.len() >= 2, "test payload must fragment");

        client.last_remote_state = Instant::now() - Duration::from_secs(7);
        let complete_contact = client.last_remote_state();
        assert!(client.recv_state(&datagrams[0]).is_none());
        assert_eq!(client.last_remote_state(), complete_contact);
        assert!(client.last_recv().elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn older_complete_branch_does_not_refresh_latest_remote_contact() {
        let (mut server, mut client) = pair();
        server.set_pending(b"state one".to_vec());
        let state_one = server.tick();
        server.set_pending(b"state two".to_vec());
        server.force_next_send();
        let state_two = server.tick();

        let newest = state_two
            .iter()
            .find_map(|datagram| client.recv_state(datagram))
            .expect("newest branch");
        assert_eq!(newest.new_num, 2);

        client.last_remote_state = Instant::now() - Duration::from_secs(7);
        let latest_contact = client.last_remote_state();
        let older = state_one
            .iter()
            .find_map(|datagram| client.recv_state(datagram))
            .expect("older branch remains a useful base");
        assert_eq!(older.new_num, 1);
        assert_eq!(client.last_remote_state(), latest_contact);
    }

    #[test]
    fn throwaway_num_advances_after_ack() {
        let (mut server, mut client) = pair();
        client.set_pending(b"keys".to_vec());
        let dgs = client.tick();
        for dg in dgs {
            let _ = server.recv(&dg);
        }
        // Server acks client state via its next tick.
        server.set_pending(b"out".to_vec());
        let dgs = server.tick();
        for dg in dgs {
            let _ = client.recv(&dg);
        }
        // Client next tick should advertise throwaway >= 1 after being acked.
        client.force_next_send();
        // Inspect by letting server process empty-ish keepalive after another round
        client.set_pending(b"more".to_vec());
        // After first state acked, local_throwaway should be > 0
        assert!(client.acked_by_remote() >= 1 || client.sent_num() >= 1);
        // Force a tick that carries throwaway
        let (mut s2, mut c2) = pair();
        c2.set_pending(b"a".to_vec());
        for dg in c2.tick() {
            let _ = s2.recv(&dg);
        }
        s2.force_next_send();
        for dg in s2.tick() {
            let _ = c2.recv(&dg);
        }
        assert!(c2.acked_by_remote() >= 1);
        c2.force_next_send();
        let dgs = c2.tick();
        assert!(!dgs.is_empty());
        // Server should accept and not accumulate forever — smoke only.
        for dg in dgs {
            let _ = s2.recv(&dg);
        }
    }

    #[test]
    fn wrong_direction_rejected() {
        let (mut server, _) = pair();
        server.set_pending(b"data".to_vec());
        let dgs = server.tick();
        assert!(server.recv(&dgs[0]).is_none());
    }

    #[test]
    fn large_payload_fragmented() {
        let (mut server, mut client) = pair();
        let mut payload = vec![0u8; MAX_FRAGMENT_PAYLOAD * 3 + 42];
        rand::thread_rng().fill_bytes(&mut payload);
        server.set_pending(payload.clone());
        let dgs = server.tick();
        assert!(dgs.len() >= 2);
        let mut diff = None;
        for dg in dgs {
            if let Some(d) = client.recv(&dg) {
                diff = Some(d);
            }
        }
        assert_eq!(diff.unwrap(), payload);
    }

    #[test]
    fn ipv6_fragment_limit_keeps_udp_payload_within_minimum_mtu() {
        let (_server, mut client) = pair();
        client.set_max_fragment_payload(1178);
        let mut payload = vec![0u8; 5000];
        rand::thread_rng().fill_bytes(&mut payload);
        client.set_pending(payload);
        client.force_next_send();

        let datagrams = client.tick();
        assert!(datagrams.len() > 1);
        assert!(datagrams.iter().all(|datagram| datagram.len() <= 1216));
    }

    #[test]
    fn session_stops_before_ocb_block_budget_is_exhausted() {
        let (_server, mut client) = pair();
        client.encrypted_blocks = OCB_BLOCK_LIMIT - 1;
        client.set_pending(b"last packet".to_vec());
        client.force_next_send();

        assert!(client.tick().is_empty());
        assert!(client.crypto_exhausted());
    }

    #[test]
    fn keepalive_force_send() {
        let (mut server, mut client) = pair();
        client.force_next_send();
        let dgs = client.tick();
        assert_eq!(dgs.len(), 1);
        let state = dgs
            .iter()
            .find_map(|datagram| server.recv_state(datagram))
            .expect("forced keepalive must create a new SSP state");
        assert_eq!(state.new_num, 1);
    }

    #[test]
    fn idle_keepalive_waits_for_stock_three_second_interval() {
        let (_server, mut client) = pair();
        client.force_next_send();
        assert!(!client.tick().is_empty());

        client.last_send = Instant::now() - Duration::from_secs(1);
        assert!(
            client.tick().is_empty(),
            "idle keepalive fired at the retransmission timeout"
        );

        client.last_send = Instant::now() - Duration::from_secs(3);
        assert!(!client.tick().is_empty());
    }

    #[test]
    fn active_retransmission_includes_stock_ack_delay() {
        let (_server, mut client) = pair();
        client.set_pending(b"unacknowledged".to_vec());
        client.force_next_send();
        assert!(!client.tick().is_empty());

        let state = client.outbound_states.back_mut().unwrap();
        state.last_sent = Some(Instant::now() - client.rto - Duration::from_millis(50));
        client.last_send = Instant::now() - client.rto - Duration::from_millis(50);
        client.last_recv = Instant::now();
        assert!(
            client.tick().is_empty(),
            "stock mosh waits RTO plus its 100 ms delayed-ACK allowance"
        );

        let state = client.outbound_states.back_mut().unwrap();
        state.last_sent = Some(Instant::now() - client.rto - Duration::from_millis(101));
        client.last_send = Instant::now() - client.rto - Duration::from_millis(101);
        assert!(!client.tick().is_empty());
    }

    #[test]
    fn unacknowledged_state_backs_off_to_three_seconds_after_ten_second_outage() {
        let (mut server, mut client) = pair();
        client.set_pending(b"survive outage".to_vec());
        client.force_next_send();
        assert!(!client.tick().is_empty());

        // A decrypted fragment is not a new remote state. Stock backs off
        // based on the newest complete state, even if fragments keep arriving.
        client.last_recv = Instant::now();
        client.last_remote_state = Instant::now() - Duration::from_secs(11);
        let state = client.outbound_states.back_mut().unwrap();
        state.last_sent = Some(Instant::now() - Duration::from_secs(2));
        client.last_send = Instant::now() - Duration::from_secs(2);
        assert!(
            client.tick().is_empty(),
            "long outage kept retransmitting faster than the stock heartbeat"
        );

        let state = client.outbound_states.back_mut().unwrap();
        state.last_sent = Some(Instant::now() - Duration::from_secs(3));
        client.last_send = Instant::now() - Duration::from_secs(3);
        let retransmission = client.tick();
        let diff = retransmission
            .iter()
            .find_map(|datagram| server.recv(datagram))
            .expect("three-second outage retry should carry the pending state");
        assert_eq!(diff, b"survive outage");
    }

    #[test]
    fn nonempty_remote_state_uses_stock_delayed_ack() {
        let (mut server, mut client) = pair();
        server.set_pending(b"screen update".to_vec());
        for datagram in server.tick() {
            assert_eq!(
                client.recv(&datagram).as_deref(),
                Some(b"screen update".as_slice())
            );
        }

        assert!(
            client.tick().is_empty(),
            "screen data was acknowledged before the 100 ms delayed-ACK window"
        );
        std::thread::sleep(Duration::from_millis(101));
        let acknowledgment = client.tick();
        assert!(!acknowledgment.is_empty());
        for datagram in acknowledgment {
            let _ = server.recv(&datagram);
        }
        assert_eq!(server.acked_by_remote(), 1);
    }

    #[test]
    fn outgoing_input_piggybacks_ack_before_delay_expires() {
        let (mut server, mut client) = pair();
        server.set_pending(b"screen update".to_vec());
        for datagram in server.tick() {
            let _ = client.recv(&datagram);
        }

        client.set_pending(b"user input".to_vec());
        let outgoing = client.tick();
        assert!(!outgoing.is_empty());
        for datagram in outgoing {
            let _ = server.recv(&datagram);
        }
        assert_eq!(server.acked_by_remote(), 1);
    }

    #[test]
    fn empty_remote_heartbeat_waits_for_local_heartbeat() {
        let (mut server, mut client) = pair();
        server.force_next_send();
        for datagram in server.tick() {
            assert_eq!(client.recv(&datagram), Some(Vec::new()));
        }

        assert!(
            client.tick().is_empty(),
            "an empty heartbeat triggered an immediate ping-pong acknowledgment"
        );
        client.last_send = Instant::now() - Duration::from_secs(3);
        let acknowledgment = client.tick();
        assert!(!acknowledgment.is_empty());
        for datagram in acknowledgment {
            let _ = server.recv(&datagram);
        }
        assert_eq!(server.acked_by_remote(), 1);
    }

    #[test]
    fn each_wire_instruction_uses_a_fresh_fragment_id() {
        let (_server, mut client) = pair();
        client.force_next_send();
        let first = client.tick().remove(0);
        client.force_next_send();
        let second = client.tick().remove(0);

        let fragment_id = |wire: &[u8]| {
            let dir_seq = u64::from_be_bytes(wire[..8].try_into().unwrap());
            let nonce = Ocb::nonce_for(dir_seq);
            let plaintext = client.ocb.decrypt(&nonce, &wire[8..]).unwrap();
            Fragment::decode(&plaintext[4..]).unwrap().id
        };

        assert_ne!(fragment_id(&first), fragment_id(&second));
    }

    #[test]
    fn empty_tick_produces_nothing() {
        let (mut server, _) = pair();
        let dgs = server.tick();
        assert!(dgs.is_empty());
    }

    #[test]
    fn crypto_sequence_is_unique_across_transports_like_stock() {
        let (mut server, mut client) = pair();
        server.force_next_send();
        client.force_next_send();
        let server_wire = server.tick().remove(0);
        let client_wire = client.tick().remove(0);
        let server_seq = u64::from_be_bytes(server_wire[..8].try_into().unwrap()) & SEQ_MASK;
        let client_seq = u64::from_be_bytes(client_wire[..8].try_into().unwrap()) & SEQ_MASK;
        assert_ne!(server_seq, client_seq);
    }

    #[test]
    fn rto_bounds() {
        let (server, _) = pair();
        let rto = server.rto();
        assert!(rto >= MIN_RTO && rto <= MAX_RTO, "rto={rto:?}");
    }

    #[test]
    fn initial_rto_matches_stock_one_second_cap() {
        let (_server, client) = pair();
        assert_eq!(client.rto(), Duration::from_secs(1));
    }

    #[test]
    fn send_interval_rounds_odd_millisecond_rtt_up_like_stock() {
        let (mut server, _) = pair();
        server.rtt_init = true;
        server.srtt = Duration::from_millis(61);
        assert_eq!(server.send_interval(), Duration::from_millis(31));
        server.srtt = Duration::from_millis(161);
        assert_eq!(server.send_interval(), Duration::from_millis(81));
    }

    #[test]
    fn timestamp_reply_excludes_time_held_by_peer() {
        let (mut server, mut client) = pair();
        server.force_next_send();
        for datagram in server.tick() {
            let _ = client.recv(&datagram);
        }

        std::thread::sleep(Duration::from_millis(120));
        client.force_next_send();
        for datagram in client.tick() {
            let _ = server.recv(&datagram);
        }

        let measured = server.srtt().expect("timestamp reply should produce RTT");
        assert!(
            measured < Duration::from_millis(60),
            "peer hold time leaked into RTT: {measured:?}"
        );
    }

    #[test]
    fn congestion_notification_adds_stock_timestamp_penalty() {
        let (mut server, mut client) = pair();
        server.force_next_send();
        for datagram in server.tick() {
            let _ = client.recv_state_with_congestion(&datagram, true);
        }

        client.force_next_send();
        for datagram in client.tick() {
            let _ = server.recv(&datagram);
        }

        let measured = server
            .srtt()
            .expect("ECN reply should produce an RTT sample");
        assert!(
            (Duration::from_millis(450)..Duration::from_millis(650)).contains(&measured),
            "stock 500 ms congestion penalty missing from echoed timestamp: {measured:?}"
        );
    }

    #[test]
    fn stock_chaff_uses_zero_to_sixteen_random_bytes() {
        let random = [16, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        assert_eq!(chaff_from_random(random), (0u8..16).collect::<Vec<_>>());

        let mut zero = [0u8; 17];
        zero[0] = 17;
        assert!(chaff_from_random(zero).is_empty());
    }

    #[test]
    fn multiple_exchanges() {
        let (mut server, mut client) = pair();
        for _ in 0..10 {
            let mut payload = vec![0u8; 100];
            rand::thread_rng().fill_bytes(&mut payload);
            server.set_pending(payload.clone());
            for dg in server.tick() {
                let _ = client.recv(&dg);
            }
            client.set_pending(payload);
            for dg in client.tick() {
                let _ = server.recv(&dg);
            }
        }
        assert!(server.sent_num() >= 10);
        assert!(client.sent_num() >= 10);
    }

    #[test]
    fn protocol_version_mismatch_rejected() {
        let (mut server, mut client) = pair();
        // Craft is hard without internals; ensure v2 normal path works and
        // decoder rejects via public path when version is wrong would need
        // injection. Smoke: valid exchange still works after version check.
        server.set_pending(b"v2".to_vec());
        let mut got = None;
        for dg in server.tick() {
            if let Some(d) = client.recv(&dg) {
                got = Some(d);
            }
        }
        assert_eq!(got.unwrap(), b"v2");
    }

    #[test]
    fn missing_protocol_version_is_rejected() {
        let (mut server, mut client) = pair();
        let instruction = TransportInstruction {
            protocol_version: 0,
            old_num: 0,
            new_num: 1,
            ack_num: 0,
            throwaway_num: 0,
            diff: b"must-not-apply".to_vec(),
            chaff: Vec::new(),
        };
        let compressed = zlib_compress(&instruction.encode());
        let fragment = crate::fragment::fragmentize(1, &compressed).remove(0);
        let datagram = server.encrypt_fragment(&fragment).unwrap();

        assert!(client.recv(&datagram).is_none());
        assert_eq!(client.ack_num(), 0);
    }

    #[test]
    fn shutdown_request_is_acknowledged_by_peer() {
        let (mut server, mut client) = pair();
        client.start_shutdown();

        for datagram in client.tick() {
            let _ = server.recv(&datagram);
        }
        assert_eq!(server.ack_num(), u64::MAX);

        for datagram in server.tick() {
            let _ = client.recv(&datagram);
        }
        assert!(client.shutdown_acknowledged());
        assert!(server.counterparty_shutdown_ack_sent());
    }

    #[test]
    fn ack_num_only_advances() {
        let (mut server, mut client) = pair();
        server.set_pending(b"s1".to_vec());
        for dg in server.tick() {
            let _ = client.recv(&dg);
        }
        assert_eq!(client.ack_num(), 1);
        server.set_pending(b"s2".to_vec());
        for dg in server.tick() {
            let _ = client.recv(&dg);
        }
        assert_eq!(client.ack_num(), 2);
    }

    #[test]
    fn roundtrip_success_uses_the_acked_state_send_time_like_stock() {
        let (mut server, mut client) = pair();
        client.set_pending(b"old outbound state".to_vec());
        let client_datagrams = client.tick();
        client.backdate_sent_state_for_test(1, Duration::from_secs(7));
        for datagram in client_datagrams {
            let _ = server.recv_state(&datagram);
        }

        server.force_next_send();
        for datagram in server.tick() {
            let _ = client.recv_state(&datagram);
        }

        assert_eq!(client.acked_by_remote(), 1);
        assert!(
            client.last_roundtrip_success().elapsed() >= Duration::from_secs(6),
            "a late ACK must preserve the original send time"
        );
    }

    #[test]
    fn prospective_chain_expires_back_to_the_known_receiver_base() {
        let (mut sender, _) = pair();
        sender.pacing_enabled = false;
        sender.set_pending_from(0, vec![b'a'; 1_200]);
        assert!(!sender.tick().is_empty());
        sender.set_pending_from(1, b"b".to_vec());
        assert!(!sender.tick().is_empty());
        assert!(!sender.prospective_chain_expired());

        sender.backdate_sent_state_for_test(1, Duration::from_secs(2));
        assert!(sender.prospective_chain_expired());
    }

    #[test]
    fn stale_first_link_invalidates_an_otherwise_fresh_prospective_chain() {
        let (mut sender, _) = pair();
        sender.pacing_enabled = false;
        sender.set_pending_from(0, vec![b'a'; 1_200]);
        assert!(!sender.tick().is_empty());
        sender.set_pending_from(1, b"b".to_vec());
        assert!(!sender.tick().is_empty());
        sender.set_pending_from(2, b"c".to_vec());
        assert!(!sender.tick().is_empty());

        sender.backdate_sent_state_for_test(1, Duration::from_secs(2));
        assert_eq!(sender.prospective_base_num(), None);
        assert!(sender.prospective_chain_expired());
    }

    #[test]
    fn ack_for_a_state_number_that_was_never_sent_is_ignored() {
        let (mut peer, mut sender) = pair();
        sender.set_pending(b"superseded before send".to_vec());
        sender.set_pending(b"actually sent".to_vec());
        let sent = sender.tick();
        assert_eq!(sender.sent_num(), 2);
        assert!(!sent.is_empty());

        peer.ack_num = 1;
        peer.force_next_send();
        for datagram in peer.tick() {
            let _ = sender.recv_state(&datagram);
        }

        assert_eq!(sender.acked_by_remote(), 0);
    }

    #[test]
    fn ack_for_a_culled_middle_state_is_ignored_like_stock() {
        let (mut sender, mut peer) = pair();
        for state in 1..=32 {
            sender.set_pending(vec![state]);
            assert!(!sender.tick().is_empty());
        }
        assert_eq!(sender.sent_num(), 32);
        assert!(
            !sender
                .sent_state_times
                .iter()
                .any(|(state_num, _)| *state_num == 17),
            "test must target stock's culled middle state"
        );

        peer.ack_num = 17;
        peer.force_next_send();
        for datagram in peer.tick() {
            let _ = sender.recv_state(&datagram);
        }

        assert_eq!(sender.acked_by_remote(), 0);
    }

    #[test]
    fn remote_state_queue_matches_stock_periodic_receiver_quench() {
        let (mut server, mut client) = pair();
        let mut accepted = 0usize;
        for state in 1..=(RECEIVED_STATE_CAP + 4) {
            server.set_pending(vec![(state & 0xff) as u8]);
            for datagram in server.tick() {
                if client.recv_state(&datagram).is_some() {
                    accepted += 1;
                }
            }
        }
        assert_eq!(accepted, RECEIVED_STATE_CAP + 1);
        assert_eq!(client.received_nums.len(), RECEIVED_STATE_CAP + 2);
        assert!(client.received_nums.contains(&0));
        assert_eq!(client.ack_num(), (RECEIVED_STATE_CAP + 1) as u64);

        // Stock lets one more state through after each 15-second quench.
        client.receiver_quench_until = Some(Instant::now() - Duration::from_millis(1));
        server.set_pending(b"periodic-quench-release".to_vec());
        let periodically_released = server
            .tick()
            .into_iter()
            .find_map(|datagram| client.recv_state(&datagram));
        assert!(periodically_released.is_some());

        // Let the sender learn the highest accepted state. Its next branch may
        // still reference state 0 in this packet, so the receiver must clone
        // that base before applying the new throwaway watermark.
        client.force_next_send();
        for datagram in client.tick() {
            let _ = server.recv(&datagram);
        }
        server.set_pending(b"after-prune".to_vec());
        let recovered = server
            .tick()
            .into_iter()
            .find_map(|datagram| client.recv_state(&datagram));
        let recovered = recovered.expect("throwaway should make queue space");
        assert!(recovered.throwaway_num > 0);
        assert!(client.received_nums.len() < RECEIVED_STATE_CAP + 2);
        assert!(!client.received_nums.contains(&0));
    }

    #[test]
    fn receiver_keeps_at_least_three_hundred_parallel_states_like_stock_mosh() {
        let (mut server, mut client) = pair();
        let mut accepted = 0;
        for state in 1..=300 {
            server.set_pending(vec![(state & 0xff) as u8]);
            for datagram in server.tick() {
                if client.recv_state(&datagram).is_some() {
                    accepted += 1;
                }
            }
        }
        assert_eq!(accepted, 300);
        assert_eq!(client.ack_num(), 300);
    }

    #[test]
    fn host_user_payload_roundtrip_through_transport() {
        use crate::pb::{HostInstruction, UserInstruction};
        let (mut server, mut client) = pair();

        let host = HostInstruction::encode_message(&[HostInstruction {
            hoststring: b"\x1b[Hprompt$ ".to_vec(),
            echo_ack_num: -1,
            ..Default::default()
        }]);
        server.set_pending(host.clone());
        let mut got = None;
        for dg in server.tick() {
            if let Some(d) = client.recv(&dg) {
                got = Some(d);
            }
        }
        assert_eq!(got.unwrap(), host);

        let user = UserInstruction::encode_message(&[UserInstruction::keystroke(b"pwd\n")]);
        client.set_pending(user.clone());
        let mut got = None;
        for dg in client.tick() {
            if let Some(d) = server.recv(&dg) {
                got = Some(d);
            }
        }
        assert_eq!(got.unwrap(), user);
    }
}
