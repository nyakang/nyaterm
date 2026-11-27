//! Terminal state application for the Mosh client.
//!
//! Stock mosh-server / mosh-go send HostBytes as ANSI escape sequences that
//! transform a numbered SSP state (CUP + cell paints). Multiple newer states
//! can share the same older base while an ACK is in flight, so each diff must
//! be applied to a snapshot of its declared base rather than replayed against
//! whichever state happens to be on screen.
//!
//! This module accumulates that stream, strips ANSI for tests, and provides
//! helpers used by the CLI and integration tests.

use std::collections::{BTreeMap, HashSet};

use crate::ansi_apply::{AnsiPen, apply_ansi_with_pen};
use crate::error::{Error, Result};
use crate::framebuffer::{Cell, Framebuffer};
use crate::pb::HostInstruction;
#[cfg(test)]
use crate::transport::RECEIVED_STATE_CAP;

const MAX_REMOTE_STATE_BYTES: usize = 128 * 1024 * 1024;
const MAX_TERMINAL_COLS: usize = u16::MAX as usize;
const MAX_TERMINAL_ROWS: usize = u16::MAX as usize;
/// Leave one quarter of the complete state-history budget for row metadata,
/// parser state, hyperlinks, and a referenced base frame. Unlike the old
/// fixed 100,000-cell cap, this follows the actual cell representation and
/// accepts large-but-valid stock mosh terminal dimensions.
const MAX_SINGLE_FRAME_CELL_BYTES: usize = 96 * 1024 * 1024;
const BTREE_STATE_OVERHEAD: usize = 64;
const ALLOCATION_OVERHEAD: usize = 16;

#[derive(Debug, Clone)]
struct RemoteTerminalState {
    framebuffer: Framebuffer,
    pen: AnsiPen,
    echo_ack: u64,
}

impl RemoteTerminalState {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            framebuffer: Framebuffer::new(cols as usize, rows as usize),
            pen: AnsiPen::default(),
            echo_ack: 0,
        }
    }

    fn apply_host_message(&mut self, diff: &[u8]) -> Result<bool> {
        let instructions = HostInstruction::decode_message(diff)?;
        let generation_before = self.framebuffer.scroll_generation;
        for instruction in instructions {
            if instruction.width > 0 && instruction.height > 0 {
                let cols = instruction.width as usize;
                let rows = instruction.height as usize;
                let cells = cols
                    .checked_mul(rows)
                    .ok_or_else(|| Error::Protocol("remote terminal size overflow".to_string()))?;
                let cell_bytes =
                    cells
                        .checked_mul(std::mem::size_of::<Cell>())
                        .ok_or_else(|| {
                            Error::Protocol("remote terminal allocation overflow".to_string())
                        })?;
                if cols > MAX_TERMINAL_COLS
                    || rows > MAX_TERMINAL_ROWS
                    || cell_bytes > MAX_SINGLE_FRAME_CELL_BYTES
                {
                    return Err(Error::Protocol(format!(
                        "remote terminal size {cols}x{rows} exceeds safety limit"
                    )));
                }
                self.framebuffer.resize(cols, rows);
            }
            if instruction.echo_ack_num >= 0 {
                self.echo_ack = self.echo_ack.max(instruction.echo_ack_num as u64);
            }
            if !instruction.hoststring.is_empty() {
                apply_ansi_with_pen(
                    &mut self.framebuffer,
                    &mut self.pen,
                    &instruction.hoststring,
                );
            }
        }
        self.framebuffer.compact_hyperlinks();
        Ok(self.framebuffer.scroll_generation != generation_before)
    }
}

/// Local view of applied host terminal updates.
#[derive(Debug)]
pub struct TerminalView {
    /// Raw bytes received from HostBytes (ANSI screen diffs).
    paint: Vec<u8>,
    cols: u16,
    rows: u16,
    /// Max HostInstruction.echo_ack_num seen (stock late_ack for prediction).
    echo_ack: u64,
    /// Complete remote terminal snapshots keyed by SSP state number.
    remote_states: BTreeMap<u64, RemoteTerminalState>,
    latest_remote_num: u64,
    /// Remote framebuffer most recently emitted to the local PTY.
    displayed_remote: Framebuffer,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl TerminalView {
    pub fn new(cols: u16, rows: u16) -> Self {
        let initial = RemoteTerminalState::new(cols, rows);
        let displayed_remote = initial.framebuffer.clone();
        let remote_states = BTreeMap::from([(0, initial)]);
        Self {
            paint: Vec::new(),
            cols,
            rows,
            echo_ack: 0,
            remote_states,
            latest_remote_num: 0,
            displayed_remote,
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Cap retained paint history for long sessions (live path only needs the
    /// returned chunk; history is for tests / diagnostics).
    const PAINT_HISTORY_CAP: usize = 64 * 1024;

    /// Highest echo_ack from the server (stock `local_frame_late_acked`).
    pub fn echo_ack(&self) -> u64 {
        self.echo_ack
    }

    /// Apply a HostMessage diff (list of host instructions).
    /// Returns the bytes that should be written to the local terminal.
    pub fn apply_host_diff(&mut self, diff: &[u8]) -> Vec<u8> {
        let instrs = match HostInstruction::decode_message(diff) {
            Ok(i) => i,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for hi in instrs {
            if hi.width > 0 && hi.height > 0 {
                self.cols = hi.width as u16;
                self.rows = hi.height as u16;
            }
            // Stock stmclient: late_ack comes from EchoAck / echo_ack_num.
            if hi.echo_ack_num >= 0 {
                let n = hi.echo_ack_num as u64;
                if n > self.echo_ack {
                    self.echo_ack = n;
                }
            }
            if !hi.hoststring.is_empty() {
                out.extend_from_slice(&hi.hoststring);
                self.paint.extend_from_slice(&hi.hoststring);
            }
        }
        // Bound history so multi-hour sessions do not retain full scrollback.
        if self.paint.len() > Self::PAINT_HISTORY_CAP {
            let drop = self.paint.len() - Self::PAINT_HISTORY_CAP;
            self.paint.drain(..drop);
        }
        out
    }

    /// Reconstruct a numbered remote SSP state, then emit only the difference
    /// from the latest state already shown locally.
    ///
    /// A high-latency server commonly sends state 1 from base 0 and then state
    /// 2 from the same base 0 before the ACK for state 1 arrives. Replaying both
    /// raw diffs would duplicate shared glyphs. Snapshot reconstruction keeps
    /// the operation idempotent, matching stock mosh's received-state queue.
    pub fn apply_host_state(
        &mut self,
        old_num: u64,
        new_num: u64,
        throwaway_num: u64,
        diff: &[u8],
    ) -> Result<Vec<u8>> {
        if self.remote_states.contains_key(&new_num) {
            return Ok(Vec::new());
        }
        let Some(mut next) = self.remote_states.get(&old_num).cloned() else {
            return Err(Error::Protocol(format!(
                "remote state {new_num} references unavailable base {old_num}"
            )));
        };
        let geometry_changed = next.apply_host_message(diff)?;
        if geometry_changed {
            // A state number is unique across parallel SSP branches, unlike a
            // simple increment inherited from their shared base.
            next.framebuffer.scroll_generation = new_num;
        }

        let retained_bytes = self.retained_storage_bytes_with(
            &next,
            throwaway_num,
            new_num > self.latest_remote_num,
        );
        if retained_bytes > MAX_REMOTE_STATE_BYTES {
            return Err(Error::Protocol(format!(
                "remote terminal state history exceeds {} MiB safety limit",
                MAX_REMOTE_STATE_BYTES / (1024 * 1024)
            )));
        }

        // Keep the base alive until after cloning it, as stock mosh does.
        if throwaway_num > 0 {
            self.remote_states.retain(|&num, _| num >= throwaway_num);
        }
        // Transport applies stock's 1024-state receiver quench. Keep every
        // state it accepts so future branches never reference a missing base;
        // the byte budget above remains the hard memory-safety boundary.
        self.remote_states.insert(new_num, next.clone());

        // Out-of-order older states are useful as future bases, but must not
        // rewind the user's display or late-ack watermark.
        if new_num <= self.latest_remote_num {
            return Ok(Vec::new());
        }
        self.latest_remote_num = new_num;
        self.cols = next.framebuffer.cols as u16;
        self.rows = next.framebuffer.rows as u16;
        self.echo_ack = self.echo_ack.max(next.echo_ack);

        let out = next.framebuffer.diff(Some(&self.displayed_remote));
        self.displayed_remote = next.framebuffer;
        self.record_paint(&out);
        Ok(out)
    }

    fn retained_storage_bytes_with(
        &self,
        next: &RemoteTerminalState,
        throwaway_num: u64,
        will_display: bool,
    ) -> usize {
        let mut row_allocations = HashSet::new();
        let mut total = self.paint.capacity().saturating_add(ALLOCATION_OVERHEAD);
        let retained = self
            .remote_states
            .iter()
            .filter(move |(num, _)| throwaway_num == 0 || **num >= throwaway_num)
            .map(|(_, state)| state)
            .chain(std::iter::once(next));
        for state in retained {
            total = total
                .saturating_add(std::mem::size_of::<RemoteTerminalState>())
                .saturating_add(BTREE_STATE_OVERHEAD)
                .saturating_add(state.pen.carry.capacity())
                .saturating_add(if state.pen.carry.capacity() > 0 {
                    ALLOCATION_OVERHEAD
                } else {
                    0
                });
            add_framebuffer_storage(&state.framebuffer, &mut row_allocations, &mut total);
        }

        // `next` is cloned into the state map. The displayed framebuffer owns
        // a second set of Vec metadata, even though its row Arcs stay shared.
        let displayed_after = if will_display {
            &next.framebuffer
        } else {
            &self.displayed_remote
        };
        add_framebuffer_storage(displayed_after, &mut row_allocations, &mut total);
        total
    }

    /// Latest fully reconstructed server framebuffer.
    pub fn remote_framebuffer(&self) -> &Framebuffer {
        self.remote_states
            .get(&self.latest_remote_num)
            .map(|state| &state.framebuffer)
            .unwrap_or(&self.displayed_remote)
    }

    /// State number associated with [`Self::remote_framebuffer`].
    pub fn remote_state_num(&self) -> u64 {
        self.latest_remote_num
    }

    fn record_paint(&mut self, bytes: &[u8]) {
        self.paint.extend_from_slice(bytes);
        if self.paint.len() > Self::PAINT_HISTORY_CAP {
            let drop = self.paint.len() - Self::PAINT_HISTORY_CAP;
            self.paint.drain(..drop);
        }
    }

    /// All paint bytes accumulated so far.
    pub fn paint_bytes(&self) -> &[u8] {
        &self.paint
    }

    /// Drain newly accumulated paint since last drain (tests / buffered consumers).
    pub fn take_paint(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.paint)
    }

    /// Whether the view has any non-empty screen content applied.
    pub fn has_content(&self) -> bool {
        !self.paint.is_empty()
    }
}

fn add_framebuffer_storage(
    framebuffer: &Framebuffer,
    row_allocations: &mut HashSet<usize>,
    total: &mut usize,
) {
    *total = total.saturating_add(framebuffer.metadata_storage_bytes());
    for (allocation, bytes) in framebuffer.row_storage() {
        if row_allocations.insert(allocation) {
            *total = total.saturating_add(bytes);
        }
    }
}

/// Strip CSI / OSC ANSI sequences for token matching in tests.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: params then final byte 0x40-0x7E
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC ... BEL or ST
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch == '\u{7}' {
                            break;
                        }
                        if ch == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    // Other ESC sequences: skip next char if present
                    let _ = chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        AnsiPen, BTREE_STATE_OVERHEAD, Framebuffer, HashSet, RECEIVED_STATE_CAP,
        RemoteTerminalState, TerminalView, apply_ansi_with_pen, strip_ansi,
    };
    use crate::pb::HostInstruction;

    fn host_message(hoststring: &[u8]) -> Vec<u8> {
        HostInstruction::encode_message(&[HostInstruction {
            hoststring: hoststring.to_vec(),
            echo_ack_num: -1,
            ..Default::default()
        }])
    }

    fn row_text(framebuffer: &Framebuffer, row: usize) -> String {
        (0..framebuffer.cols)
            .map(|col| framebuffer.cell_at(col, row).unwrap().ch)
            .collect()
    }

    #[test]
    fn apply_hoststring_produces_paint() {
        let mut view = TerminalView::new(80, 24);
        let msg = HostInstruction::encode_message(&[HostInstruction {
            hoststring: b"\x1b[H$ echo hi\r\nhi\r\n".to_vec(),
            width: 0,
            height: 0,
            echo_ack_num: -1,
        }]);
        let painted = view.apply_host_diff(&msg);
        assert!(!painted.is_empty());
        assert!(view.has_content());
        let plain = strip_ansi(std::str::from_utf8(view.paint_bytes()).unwrap());
        assert!(plain.contains("echo hi"));
        assert!(plain.contains("hi"));
    }

    #[test]
    fn strip_ansi_removes_csi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[?25lhi\x1b[?25h"), "hi");
    }

    #[test]
    fn strip_ansi_osc_title() {
        // OSC 0 title sequences appear in real mosh HostBytes paint
        let s = "\x1b]0;root@host: ~\x07prompt$ ";
        assert_eq!(strip_ansi(s), "prompt$ ");
    }

    #[test]
    fn apply_resize_updates_dims() {
        let mut view = TerminalView::new(80, 24);
        let msg = HostInstruction::encode_message(&[HostInstruction {
            width: 132,
            height: 43,
            echo_ack_num: -1,
            ..Default::default()
        }]);
        let paint = view.apply_host_diff(&msg);
        assert!(paint.is_empty());
        assert_eq!(view.cols(), 132);
        assert_eq!(view.rows(), 43);
    }

    #[test]
    fn apply_malformed_diff_is_noop() {
        let mut view = TerminalView::new(80, 24);
        let paint = view.apply_host_diff(&[0x80, 0x80, 0x80]);
        assert!(paint.is_empty());
        assert!(!view.has_content());
    }

    #[test]
    fn take_paint_drains() {
        let mut view = TerminalView::new(80, 24);
        let msg = HostInstruction::encode_message(&[HostInstruction {
            hoststring: b"abc".to_vec(),
            echo_ack_num: -1,
            ..Default::default()
        }]);
        let _ = view.apply_host_diff(&msg);
        assert_eq!(view.take_paint(), b"abc");
        assert!(!view.has_content());
    }

    #[test]
    fn multi_instruction_host_message() {
        let mut view = TerminalView::new(80, 24);
        let msg = HostInstruction::encode_message(&[
            HostInstruction {
                hoststring: b"\x1b[H".to_vec(),
                echo_ack_num: -1,
                ..Default::default()
            },
            HostInstruction {
                hoststring: b"line1\r\n".to_vec(),
                echo_ack_num: -1,
                ..Default::default()
            },
            HostInstruction {
                hoststring: b"line2".to_vec(),
                echo_ack_num: 1,
                ..Default::default()
            },
        ]);
        let painted = view.apply_host_diff(&msg);
        let plain = strip_ansi(std::str::from_utf8(&painted).unwrap());
        assert!(plain.contains("line1"));
        assert!(plain.contains("line2"));
    }

    #[test]
    fn utf8_cjk_in_hoststring() {
        let mut view = TerminalView::new(80, 24);
        let msg = HostInstruction::encode_message(&[HostInstruction {
            hoststring: "你好世界".as_bytes().to_vec(),
            echo_ack_num: -1,
            ..Default::default()
        }]);
        let painted = view.apply_host_diff(&msg);
        assert_eq!(std::str::from_utf8(&painted).unwrap(), "你好世界");
    }

    #[test]
    fn echo_ack_tracks_max_from_host_instructions() {
        let mut view = TerminalView::new(80, 24);
        assert_eq!(view.echo_ack(), 0);
        let msg = HostInstruction::encode_message(&[
            HostInstruction {
                hoststring: b"a".to_vec(),
                width: 80,
                height: 24,
                echo_ack_num: 3,
            },
            HostInstruction {
                hoststring: b"b".to_vec(),
                width: 0,
                height: 0,
                echo_ack_num: 5,
            },
            HostInstruction {
                hoststring: Vec::new(),
                width: 0,
                height: 0,
                echo_ack_num: 4, // lower — must not regress
            },
        ]);
        let painted = view.apply_host_diff(&msg);
        assert_eq!(painted, b"ab");
        assert_eq!(view.echo_ack(), 5);
    }

    #[test]
    fn numbered_state_scrolls_only_inside_remote_margins() {
        let mut view = TerminalView::new(5, 5);
        let initial =
            host_message(b"\x1b[1;1H11111\x1b[2;1H22222\x1b[3;1H33333\x1b[4;1H44444\x1b[5;1H55555");
        view.apply_host_state(0, 1, 0, &initial).unwrap();

        let scroll = host_message(b"\x1b[2;4r\x1b[4;1H\n\x1b[r");
        view.apply_host_state(1, 2, 0, &scroll).unwrap();
        let frame = view.remote_framebuffer();

        assert_eq!(row_text(frame, 0), "11111");
        assert_eq!(row_text(frame, 1), "33333");
        assert_eq!(row_text(frame, 2), "44444");
        assert_eq!(row_text(frame, 3), "     ");
        assert_eq!(row_text(frame, 4), "55555");
        assert_eq!(frame.scroll_generation, 2);
    }

    #[test]
    fn numbered_state_preserves_terminal_modes_and_side_effects() {
        let mut view = TerminalView::new(20, 4);
        let update = host_message(
            b"\x07\x1b]0;remote title\x07\x1b]52;c;YQ==\x07\x1b[?5h\x1b[?2004h\x1b[?1002h\x1b[?1004h\x1b[?1006h\x1b]8;id=doc;https://example.test\x1b\\L\x1b]8;;\x1b\\",
        );
        let paint = view.apply_host_state(0, 1, 0, &update).unwrap();
        let frame = view.remote_framebuffer();

        assert_eq!(frame.bell_count, 1, "OSC terminators are not audible bells");
        assert_eq!(frame.icon_name.as_deref(), Some(b"remote title".as_slice()));
        assert_eq!(
            frame.window_title.as_deref(),
            Some(b"remote title".as_slice())
        );
        assert_eq!(frame.clipboard.as_deref(), Some(b"YQ==".as_slice()));
        assert!(frame.reverse_video);
        assert!(frame.bracketed_paste);
        assert_eq!(frame.mouse_reporting_mode, 1002);
        assert!(frame.mouse_focus_event);
        assert_eq!(frame.mouse_encoding_mode, 1006);
        assert_eq!(frame.cell_at(0, 0).unwrap().ch, 'L');
        assert_ne!(frame.cell_at(0, 0).unwrap().hyperlink, 0);

        let mut replay = Framebuffer::new(20, 4);
        let mut pen = AnsiPen::default();
        apply_ansi_with_pen(&mut replay, &mut pen, &paint);
        assert_eq!(replay.bell_count, 1);
        assert_eq!(replay.window_title, frame.window_title);
        assert_eq!(replay.clipboard, frame.clipboard);
        assert!(replay.bracketed_paste);
        assert_eq!(replay.mouse_reporting_mode, 1002);
        assert_eq!(replay.cell_at(0, 0).unwrap().ch, 'L');
        assert_ne!(replay.cell_at(0, 0).unwrap().hyperlink, 0);
    }

    #[test]
    fn numbered_state_queue_keeps_every_transport_accepted_base() {
        let mut view = TerminalView::new(2, 1);
        let empty = host_message(b"");
        for state in 1..=(RECEIVED_STATE_CAP as u64 + 2) {
            view.apply_host_state(0, state, 0, &empty).unwrap();
        }
        assert!(view.remote_states.contains_key(&0));
        assert_eq!(view.remote_states.len(), RECEIVED_STATE_CAP + 3);
    }

    #[test]
    fn out_of_order_states_are_retained_as_future_bases_without_rewind() {
        let mut view = TerminalView::new(8, 1);
        view.apply_host_state(0, 1, 0, &host_message(b"a")).unwrap();
        view.apply_host_state(0, 3, 0, &host_message(b"c")).unwrap();
        assert_eq!(row_text(view.remote_framebuffer(), 0), "c       ");

        assert!(
            view.apply_host_state(1, 2, 0, &host_message(b"b"))
                .unwrap()
                .is_empty()
        );
        view.apply_host_state(2, 4, 2, &host_message(b"d")).unwrap();
        assert_eq!(row_text(view.remote_framebuffer(), 0), "abd     ");
        assert!(!view.remote_states.contains_key(&0));
        assert!(!view.remote_states.contains_key(&1));
        assert!(view.remote_states.contains_key(&2));
        assert!(view.remote_states.contains_key(&4));
    }

    #[test]
    fn numbered_state_preserves_complete_unicode_graphemes() {
        let mut view = TerminalView::new(12, 2);
        let graphemes = "e\u{301} 👩\u{200d}💻";
        let paint = view
            .apply_host_state(0, 1, 0, &host_message(graphemes.as_bytes()))
            .unwrap();

        assert!(
            paint
                .windows(graphemes.len())
                .any(|window| window == graphemes.as_bytes()),
            "reconstructed paint must keep combining marks and ZWJ sequences: {paint:?}"
        );
        assert_eq!(view.remote_framebuffer().cell_at(0, 0).unwrap().width, 1);
        assert_eq!(view.remote_framebuffer().cell_at(2, 0).unwrap().width, 2);
    }

    #[test]
    fn numbered_states_repaint_a_cell_uncovered_by_a_wide_character() {
        let mut view = TerminalView::new(5, 1);
        let mut replay = Framebuffer::new(5, 1);
        let mut pen = AnsiPen::default();

        let first = host_message("界\x1b[1;2HY".as_bytes());
        let first_paint = view.apply_host_state(0, 1, 0, &first).unwrap();
        apply_ansi_with_pen(&mut replay, &mut pen, &first_paint);

        let second = host_message(b"\x1b[1;1HX");
        let second_paint = view.apply_host_state(1, 2, 0, &second).unwrap();
        apply_ansi_with_pen(&mut replay, &mut pen, &second_paint);

        assert_eq!(row_text(&replay, 0), "XY   ");
        assert!(
            view.remote_framebuffer().diff(Some(&replay)).is_empty(),
            "two numbered paints must replay to the displayed remote frame"
        );
    }

    #[test]
    fn numbered_states_erase_stock_display_wide_wrap_without_a_ghost_cell() {
        let mut view = TerminalView::new(24, 8);

        // Generated by stock mosh 1.4.0 Display::new_frame from four legal
        // terminal frames. The second frame paints wide cells at the right
        // margin; the third removes them with the stock BS+EL optimization.
        // Reconstructing those numbered HostBytes states must leave the same
        // blank screen as an xterm-256color terminal.
        let frames: [&[u8]; 4] = [
            b"\x1b[?5l\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[?25l\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\x1b[2;1H\x1b[?25h\x1b[0m\x1b[?2004l\x1b[?1003l\x1b[?1002l\x1b[?1001l\x1b[?1000l\x1b[?1004l\x1b[?1015l\x1b[?1006l\x1b[?1005l",
            "\x1b[?25l\x1b[3;22H界界\r\n\x1b[3;24H\x1b[?25h".as_bytes(),
            b"\x1b[?25l\x08\x08\x1b[K\x1b[4;2H\x1b[?25h",
            b"   ",
        ];

        for (index, frame) in frames.iter().enumerate() {
            let old_num = index as u64;
            let new_num = old_num + 1;
            view.apply_host_state(old_num, new_num, 0, &host_message(frame))
                .unwrap();
        }

        for row in 0..view.remote_framebuffer().rows {
            assert_eq!(
                row_text(view.remote_framebuffer(), row),
                " ".repeat(24),
                "stock wide-cell erase left content on row {row}"
            );
        }
    }

    #[test]
    fn numbered_states_erase_a_wide_glyph_from_its_continuation_cell() {
        let mut view = TerminalView::new(24, 8);

        // Generated by stock mosh 1.4.0 Display::new_frame. The last frame
        // positions the cursor on the continuation column of a wide glyph and
        // erases to the end of the line. Xterm erases the complete glyph.
        let frames: [&[u8]; 5] = [
            b"\x1b[?5l\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[?25l\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\n\x1b[K\x1b[1;1H\x1b[?25h\x1b[0m\x1b[?2004l\x1b[?1003l\x1b[?1002l\x1b[?1001l\x1b[?1000l\x1b[?1004l\x1b[?1015l\x1b[?1006l\x1b[?1005l",
            b"\x1b[7;14H",
            "\x1b[?25l\ré\x1b[?25h".as_bytes(),
            "\x1b[?25l\r 0\x1b[7;24H界\r\n\x1b[7;24H\x1b[?25h".as_bytes(),
            b"\x1b[?25l\x1b[7;2H\x1b[K\x1b[1;1H\x1b[?25h",
        ];

        for (index, frame) in frames.iter().enumerate() {
            let old_num = index as u64;
            let new_num = old_num + 1;
            view.apply_host_state(old_num, new_num, 0, &host_message(frame))
                .unwrap();
        }

        assert_eq!(
            row_text(view.remote_framebuffer(), 5),
            format!(" 0{}", " ".repeat(22))
        );
        assert_eq!(row_text(view.remote_framebuffer(), 6), " ".repeat(24));
    }

    #[test]
    fn numbered_state_repositions_after_a_wide_glyph_before_repainting() {
        let mut view = TerminalView::new(24, 8);
        view.apply_host_state(0, 1, 0, &host_message(b"\x1b[5;20Hx"))
            .unwrap();

        // Stock mosh 1.4.0 on Ubuntu uses width 2 for this emoji. Xterm.js
        // uses width 1, so the repaint must explicitly position the next cell
        // instead of relying on the local terminal to advance two columns.
        let paint = view
            .apply_host_state(
                1,
                2,
                0,
                &host_message("\x1b[?25l\r🙂\x1b[5;20H\x1b[K\x1b[1;1H\x1b[?25h".as_bytes()),
            )
            .unwrap();

        assert_eq!(
            row_text(view.remote_framebuffer(), 4),
            "🙂".to_owned() + &" ".repeat(23)
        );
        assert!(
            paint
                .windows(b"\x1b[5;3H".len())
                .any(|window| window == b"\x1b[5;3H"),
            "repaint after a wide glyph must address the next logical column: {paint:?}"
        );
    }

    #[test]
    fn numbered_state_keeps_the_full_leading_combining_character_limit() {
        let mut view = TerminalView::new(20, 2);
        let graphemes = "\u{301}".repeat(16);

        let paint = view
            .apply_host_state(0, 1, 0, &host_message(graphemes.as_bytes()))
            .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&paint).matches('\u{301}').count(),
            16,
            "the synthetic no-break space must not consume the grapheme byte budget"
        );
    }

    #[test]
    fn numbered_state_reclaims_unreferenced_hyperlinks() {
        let mut view = TerminalView::new(12, 2);
        let mut hoststring = Vec::new();
        for index in 0..100 {
            hoststring.extend_from_slice(
                format!("\x1b]8;id={index};https://example.test/{index}\x1b\\\x1b]8;;\x1b\\")
                    .as_bytes(),
            );
        }
        view.apply_host_state(0, 1, 0, &host_message(&hoststring))
            .unwrap();

        assert!(view.remote_framebuffer().hyperlinks.is_empty());
    }

    #[test]
    fn numbered_state_rejects_oversized_terminal_dimensions() {
        let mut view = TerminalView::new(80, 24);
        let resize = HostInstruction::encode_message(&[HostInstruction {
            width: 65_536,
            height: 1,
            echo_ack_num: -1,
            ..Default::default()
        }]);

        let err = view.apply_host_state(0, 1, 0, &resize).unwrap_err();
        assert!(err.to_string().contains("terminal size"));
    }

    #[test]
    fn numbered_state_accepts_a_large_official_terminal_within_the_memory_budget() {
        let mut view = TerminalView::new(80, 24);
        let resize = HostInstruction::encode_message(&[HostInstruction {
            width: 1600,
            height: 900,
            echo_ack_num: -1,
            ..Default::default()
        }]);

        view.apply_host_state(0, 1, 0, &resize).unwrap();

        assert_eq!(view.cols(), 1600);
        assert_eq!(view.rows(), 900);
    }

    #[test]
    fn empty_parallel_states_share_rows_under_the_memory_budget() {
        let mut view = TerminalView::new(80, 24);
        let empty = host_message(b"");
        for state in 1..200 {
            view.apply_host_state(0, state, 0, &empty).unwrap();
        }
        assert_eq!(view.remote_states.len(), 200);
    }

    #[test]
    fn empty_parallel_states_keep_linked_rows_shared() {
        let mut view = TerminalView::new(80, 24);
        let linked =
            host_message(b"\x1b]8;id=persistent;https://example.test\x1b\\L\x1b]8;;\x1b\\");
        view.apply_host_state(0, 1, 0, &linked).unwrap();
        let empty = host_message(b"");
        for state in 2..32 {
            view.apply_host_state(1, state, 0, &empty).unwrap();
        }

        let row_allocations = view
            .remote_states
            .iter()
            .filter(|(num, _)| **num >= 1)
            .filter_map(|(_, state)| {
                state
                    .framebuffer
                    .row_storage()
                    .next()
                    .map(|(allocation, _)| allocation)
            })
            .collect::<HashSet<_>>();
        assert_eq!(row_allocations.len(), 1);
    }

    #[test]
    fn retained_storage_counts_the_displayed_frame_metadata() {
        let view = TerminalView::new(80, 24);
        let next = view.remote_states.get(&0).unwrap().clone();
        let framebuffer_metadata = next.framebuffer.metadata_storage_bytes();
        let mut seen_rows = HashSet::new();
        let unique_rows = next
            .framebuffer
            .row_storage()
            .filter_map(|(allocation, bytes)| seen_rows.insert(allocation).then_some(bytes))
            .sum::<usize>();
        let minimum = 2 * (std::mem::size_of::<RemoteTerminalState>() + BTREE_STATE_OVERHEAD)
            + 3 * framebuffer_metadata
            + unique_rows;

        assert!(view.retained_storage_bytes_with(&next, 0, true) >= minimum);
    }
}
