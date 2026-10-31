//! tmux control-mode (`-CC`) session driver.
//!
//! When an SSH terminal channel switches to control mode, this loop owns the
//! channel and projects the tmux session onto the regular session model:
//! every tmux pane in the active window becomes a virtual session registered
//! in [`SessionManager`], so `write_to_session`, `resize_session`,
//! `close_session`, attach/ack and output flow control all work unchanged.
//!
//! - Pane output (`%output` / `%extended-output`) is octal-decoded and pushed
//!   to the pane's own `SessionOutputCoalescer` (i.e. `terminal-output-<id>`).
//! - Pane input is sent back through `send-keys -t %<id>`: literal bytes go
//!   as `-H <hex>`, while recognized special-key sequences are named
//!   (`send-keys -t %<id> End`) so tmux encodes them for the pane's own TERM.
//! - Layout notifications rebuild the pane tree which is emitted to the
//!   frontend via the `tmux-session-state` event.
//! - `%exit` (after `detach` or a remote kill) hands the channel back to the
//!   regular SSH loop, which continues as a normal shell.

use super::control::{BlockId, ControlMessage, ControlParser, LayoutCell, TmuxUiNode, parse_line};
use super::input::{InputParser, InputSegment};
use crate::session_event_queue::SessionEventQueue;
use crate::session_types::{SessionEvent, SessionInfo, SessionKind};
use crate::{
    ManagedSession, SessionManager, SshCommand, TmuxCommandReceiver, TmuxCommandSender, ssh_mailbox,
};
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, Sleep};

/// Input is batched over this interval so `send-keys` stays cheap even for
/// IME composition or paste bursts.
const INPUT_FLUSH_DELAY_MS: u64 = 4;
/// Max payload bytes per `send-keys` command line.
const SEND_KEYS_CHUNK_BYTES: usize = 1024;
/// Debounce for structural refreshes (`list-windows`/`list-panes`).
const STATE_REFRESH_DELAY_MS: u64 = 120;
/// Index changes (notably silent swaps) do not always emit useful notifications.
const WINDOW_POLL_INTERVAL: Duration = Duration::from_secs(2);
const WINDOWS_QUERY: &str = "list-windows -F '#{window_id} #{window_index} #{window_active} #{window_layout} #{window_visible_layout} #{window_flags} #{pane_id} #{window_name}'";
/// Buffer cap for output belonging to panes we have not mapped yet.
const PENDING_PANE_OUTPUT_MAX_BYTES: usize = 512 * 1024;
/// How long to wait for `%exit` after `detach-client` before giving up.
const DETACH_EXIT_TIMEOUT_MS: u64 = 3000;
/// Flow control: after a `%pause`, let the pane breathe, then continue it.
const PAUSE_CONTINUE_DELAY_MS: u64 = 250;
/// `resize-pane` retries per distinct target; the echoed `%layout-change`
/// retriggers the fixup, so without a cap an unreachable target would loop.
const RESIZE_PANE_MAX_ATTEMPTS: u8 = 2;
/// Renderer resize reports settle for this long before they become pane
/// targets: a freshly split pane's container transiently reports degenerate
/// dims (full width or ~zero), and applying those would collapse the layout
/// chasing transients.
const RESIZE_SYNC_DEBOUNCE_MS: u64 = 250;

/// Delay between a submitted command line and the `pane_current_path` poll:
/// the pane's shell needs a moment to actually run `cd` before tmux reports
/// the new directory.
const CWD_QUERY_DELAY_MS: u64 = 400;

#[derive(Debug)]
pub enum ControlExit {
    /// tmux detached/exited: the channel is back at a normal shell; the
    /// caller should resume regular I/O and forward `leftover` bytes first.
    ReturnToShell { leftover: Vec<u8> },
    /// The channel or the whole session was closed.
    Closed,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TmuxWindowInfo {
    pub id: String,
    pub name: String,
    /// `#{window_index}` — the user-visible `N:name` ordering key, which
    /// changes on `move-window`/`swap-window` unlike the stable `@id`.
    pub index: u32,
    pub active: bool,
    /// The window's own pane tree — every tmux window maps to a native tab.
    pub tree: Option<TmuxUiNode>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TmuxStatePayload {
    pub control_session_id: String,
    pub exited: bool,
    pub windows: Vec<TmuxWindowInfo>,
    pub active_window_id: Option<String>,
    pub active_pane_session_id: Option<String>,
}

/// One tmux pane projected as a virtual terminal session.
struct VirtualPane {
    /// Virtual session id registered in the [`SessionManager`].
    session_id: String,
    /// Paste classification state survives input flushes for this pane.
    input_parser: InputParser,
    /// Output received before `capture-pane` seeding completed. The instant
    /// is the server-side production time estimated from `%extended-output`'s
    /// age (`None` for plain `%output`, which carries no timestamp).
    pending_output: VecDeque<(Option<std::time::Instant>, Vec<u8>)>,
    /// Byte count of `pending_output`, kept incrementally so the bound in
    /// `route_output` does not rescan the queue per chunk.
    pending_bytes: usize,
    seeded: bool,
    /// When this pane's latest `capture-pane` snapshot was taken; output
    /// provably produced before it is already inside the seeded screen and
    /// must not be replayed (tmux defers notifications past `%end` blocks).
    capture_at: Option<std::time::Instant>,
    /// A capture/cursor seed pair is already in flight for this pane.
    seeding: bool,
    /// The in-flight seed predates a client grow and must be redone once it
    /// lands, so its (possibly clipped) snapshot never reaches the screen.
    reseed: bool,
    /// `capture-pane` rows held until the cursor reply lands. The seed is
    /// composed in a single payload then, because the scrollback/screen
    /// split needs `history_size` from that reply.
    pending_seed: Option<Vec<String>>,
    /// Terminal size reported by the local renderer (`resize_session`). The
    /// tmux layout owns pane dimensions, and a mismatch wraps full-width
    /// output — e.g. zsh's `#`/`%` partial-line markers, erased by `\r \r`
    /// at pane width — onto the wrong row in xterm where it survives.
    desired: Option<(u32, u32)>,
    /// Latest renderer report, promoted to `desired` after the debounce so
    /// transient container sizes never reach tmux.
    pending_dims: Option<(u32, u32)>,
    /// Cell dimensions the current screen was seeded at; a layout change
    /// that alters them triggers a fresh capture so rows realign.
    dims: Option<(u32, u32)>,
    /// Renderer dimensions (`desired`) at the time the current seed was
    /// taken. A pane first seeded while hidden in a background tab (`desired`
    /// is `None`, xterm still at default 80x24) must be reseeded once its
    /// renderer becomes visible and reports its real dimensions, even when
    /// those dimensions already match the remote tmux layout (`dims`).
    seeded_desired: Option<(u32, u32)>,
    /// Last `resize-pane` target and attempts for the current `desired`
    /// size — bounds the layout-echo feedback when a target is unreachable
    /// (e.g. a remote `window-size=manual` that ignores the client size).
    resize_attempts: Option<((u32, u32), u8)>,
    /// Pane working directory dedup for `CwdChanged` events. Populated by
    /// `#{pane_current_path}` queries — tmux tracks it per pane.
    cwd: Option<String>,
    /// Bytes routed so far; only logged while small.
    routed_bytes: usize,
}

impl VirtualPane {
    /// Pop output queued while seeding was in flight. Callers own pushing it
    /// onto the event queue so dedup stays in one place.
    fn take_pending_output(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::with_capacity(self.pending_output.len());
        while let Some((produced_at, data)) = self.pending_output.pop_front() {
            self.pending_bytes -= data.len();
            // Plain %output has no production timestamp. It may have arrived
            // during capture, so prefer a duplicate over losing new output.
            if output_precedes_capture(produced_at, self.capture_at) {
                continue;
            }
            out.push(data);
        }
        out
    }
}

fn output_precedes_capture(
    produced_at: Option<std::time::Instant>,
    capture_at: Option<std::time::Instant>,
) -> bool {
    matches!((produced_at, capture_at), (Some(at), Some(cap)) if at <= cap)
}

/// Timestamped `%output` chunks queued for a pane id that has no virtual
/// session yet; the trailing `usize` caches total bytes so the bound check
/// stays O(1).
type UnknownPaneOutput = HashMap<String, (VecDeque<(Option<std::time::Instant>, Vec<u8>)>, usize)>;

fn buffer_unknown_output(
    pending: &mut VecDeque<(Option<std::time::Instant>, Vec<u8>)>,
    total: &mut usize,
    produced_at: Option<std::time::Instant>,
    mut data: Vec<u8>,
) {
    if data.len() > PENDING_PANE_OUTPUT_MAX_BYTES {
        // Allocate just the tail: draining the prefix would retain the
        // oversized allocation even though the byte counter is bounded.
        data = data[data.len() - PENDING_PANE_OUTPUT_MAX_BYTES..].to_vec();
    }
    while *total > PENDING_PANE_OUTPUT_MAX_BYTES - data.len() {
        let Some((_, old)) = pending.pop_front() else {
            break;
        };
        *total -= old.len();
    }
    *total += data.len();
    pending.push_back((produced_at, data));
}

fn log_server_notice(session_id: &str, kind: &str, bytes: usize) {
    tracing::info!(
        session_id = %session_id,
        kind,
        bytes,
        "tmux control server notice"
    );
}

/// Why a response block was queued (FIFO correlation with `%begin`/`%end`).
enum PendingKind {
    WindowsRefresh,
    PanesRefresh,
    CapturePane {
        pane: String,
    },
    /// Cursor position query issued after a pane's capture seed.
    SeedCursor {
        pane: String,
    },
    /// `#{pane_current_path}` poll; a `cd` inside the pane emits no
    /// notification, so cwd is re-queried after each submitted command.
    PaneCwd {
        pane: String,
    },
    /// User-typed command from the tmux bar; failures are surfaced.
    UserCommand,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitMode {
    Detach,
    Close,
}

struct WindowState {
    name: String,
    /// `#{window_index}`; learned from `list-windows` (0 until then).
    index: u32,
    /// Full layout — used for the reconcile pane set (zoomed-away panes stay
    /// mapped so they don't lose their terminal state).
    layout: Option<LayoutCell>,
    /// Layout to render: collapsed to the zoomed pane while `Z` is set.
    visible_layout: Option<LayoutCell>,
    /// `Z` in window flags. Resizing a zoomed window would drop the zoom.
    zoomed: bool,
}

pub(crate) struct ControlSession<'a> {
    event_queue: &'a SessionEventQueue,
    control_session_id: &'a str,
    /// Shared session registry; pane sessions are inserted/removed here.
    sessions: &'a Arc<Mutex<HashMap<String, ManagedSession>>>,
    /// Ordered command channel shared by all projected panes.
    tmux_tx: TmuxCommandSender,

    parser: ControlParser,
    /// Lines collected inside a `%begin`..`%end` block.
    block_lines: Option<Vec<String>>,
    block_id: Option<BlockId>,
    /// When the current response block opened. Used as the capture snapshot
    /// boundary: output produced before this instant is guaranteed inside a
    /// capture executed within the block, while output produced after may
    /// still be new — the boundary must err toward replaying, not dropping.
    block_started_at: Option<std::time::Instant>,
    /// Every command line written to the control channel produces exactly one
    /// `%begin`/`%end` (or `%error`) block, in send order. The FIFO therefore
    /// must be pushed for *every* line — internal queries push their kind so
    /// the response can be interpreted, everything else pushes `Ignore`.
    pending_responses: VecDeque<PendingKind>,

    windows: HashMap<String, WindowState>,
    panes: HashMap<String, VirtualPane>,
    pane_names: HashMap<String, String>,
    pane_windows: HashMap<String, String>,
    active_window: Option<String>,
    active_pane: Option<String>,

    /// Buffered keystrokes per pane, flushed on a short timer.
    input_buf: HashMap<String, Vec<u8>>,
    /// Panes that have produced input; first write is logged for bring-up.
    seen_input: HashSet<String>,
    input_flush: Option<Pin<Box<Sleep>>>,
    /// Panes tmux paused via flow control; continued after a short delay.
    paused_panes: HashSet<String>,
    /// Last state pushed to the UI; identical states are not re-emitted so
    /// layout echoes cannot keep resizing panes.
    last_emitted: Option<TmuxStatePayload>,
    continue_deadline: Option<Pin<Box<Sleep>>>,

    refresh_deadline: Option<Pin<Box<Sleep>>>,
    /// Pending renderer-size sync; armed whenever a pane reports new dims
    /// and debounced so only settled sizes are applied.
    resize_deadline: Option<Pin<Box<Sleep>>>,
    /// Client size currently applied to tmux (grown to fit the active
    /// window's root layout; an unknown size counts as 0).
    client_size: Option<(u32, u32)>,
    /// Panes whose cwd should be re-queried once `cwd_deadline` fires —
    /// armed on spawn and whenever submitted input contains Enter.
    cwd_pending: HashSet<String>,
    cwd_deadline: Option<Pin<Box<Sleep>>>,
    /// Output received for panes we have not mapped yet (e.g. background
    /// windows), kept in case the pane becomes visible. Bounded.
    /// Buffered output for panes not yet mapped to a virtual session; the
    /// `usize` is the queue's total bytes so the bound check does not
    /// rescan it per `%output` chunk.
    unknown_output: UnknownPaneOutput,

    exit_mode: Option<ExitMode>,
    exit_seen: bool,
    detach_deadline: Option<Pin<Box<Sleep>>>,
}

impl<'a> ControlSession<'a> {
    fn new(
        event_queue: &'a SessionEventQueue,
        control_session_id: &'a str,
        sessions: &'a Arc<Mutex<HashMap<String, ManagedSession>>>,
        tmux_tx: TmuxCommandSender,
    ) -> Self {
        Self {
            event_queue,
            control_session_id,
            sessions,
            tmux_tx,
            parser: ControlParser::new(),
            block_lines: None,
            block_id: None,
            block_started_at: None,
            pending_responses: VecDeque::new(),
            windows: HashMap::new(),
            panes: HashMap::new(),
            pane_names: HashMap::new(),
            pane_windows: HashMap::new(),
            active_window: None,
            active_pane: None,
            input_buf: HashMap::new(),
            seen_input: HashSet::new(),
            input_flush: None,
            paused_panes: HashSet::new(),
            last_emitted: None,
            continue_deadline: None,
            refresh_deadline: None,
            resize_deadline: None,
            client_size: None,
            cwd_pending: HashSet::new(),
            cwd_deadline: None,
            unknown_output: HashMap::new(),
            exit_mode: None,
            exit_seen: false,
            detach_deadline: None,
        }
    }

    /// Write one command line and register its expected response block.
    async fn send_line(
        &mut self,
        channel: &Arc<ChannelWriteHalf<client::Msg>>,
        kind: PendingKind,
        line: &str,
    ) -> bool {
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        if channel.data_bytes(data).await.is_ok() {
            self.pending_responses.push_back(kind);
            true
        } else {
            false
        }
    }

    /// Queue an internal query whose block output should be parsed.
    async fn send_query(
        &mut self,
        channel: &Arc<ChannelWriteHalf<client::Msg>>,
        kind: PendingKind,
        line: &str,
    ) {
        self.send_line(channel, kind, line).await;
    }

    fn arm_refresh(&mut self) {
        if self.refresh_deadline.is_none() {
            self.refresh_deadline = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                STATE_REFRESH_DELAY_MS,
            ))));
        }
    }

    async fn refresh_windows(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        // The entry stays queued until the matching response block completes.
        if !self
            .pending_responses
            .iter()
            .any(|kind| matches!(kind, PendingKind::WindowsRefresh))
        {
            self.send_query(channel, PendingKind::WindowsRefresh, WINDOWS_QUERY)
                .await;
        }
    }

    async fn refresh_lists(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        self.refresh_windows(channel).await;
        if !self
            .pending_responses
            .iter()
            .any(|kind| matches!(kind, PendingKind::PanesRefresh))
        {
            self.send_query(
                channel,
                PendingKind::PanesRefresh,
                "list-panes -s -F '#{pane_id} #{window_id} #{pane_current_command}'",
            )
            .await;
        }
    }

    async fn spawn_pane(&mut self, pane_id: &str) {
        if self.panes.contains_key(pane_id) {
            return;
        }
        // Tauri event names only allow alphanumerics plus -/:_, so the
        // tmux `%` prefix must not leak into the virtual session id — it
        // would break every per-session event subscription.
        let session_id = format!(
            "{}::pane{}",
            self.control_session_id,
            pane_id.trim_start_matches('%')
        );
        let name = self
            .pane_names
            .get(pane_id)
            .cloned()
            .unwrap_or_else(|| format!("tmux {pane_id}"));
        // The pane's cwd comes from tmux's own pane_current_path poll, not
        // from shell-integration hooks in the pane's shell.
        let info = SessionInfo {
            id: session_id.clone(),
            name,
            kind: SessionKind::Ssh,
            working_dir: None,
            cols: 0,
            rows: 0,
        };
        // Registering the pane in the shared session map lets the desktop
        // drive write/resize/close through the ordered tmux channel.
        if let Err(error) = SessionManager::register_tmux_pane(
            self.sessions,
            info,
            pane_id.to_string(),
            self.tmux_tx.clone(),
        ) {
            tracing::warn!(
                session_id = %self.control_session_id,
                pane = %pane_id,
                %error,
                "tmux pane session registration failed"
            );
            return;
        }
        // The virtual session id is reused every time this pane re-enters
        // the tracked layout (window switch, detach/reattach, transient
        // layout drops). An earlier `remove_tmux_pane` may have cancelled
        // the queue entry — without lifting that, `accepts` silently
        // refuses every push and the pane renders forever blank.
        self.event_queue.resume_session(&session_id);

        let pane_key = pane_id.to_string();

        // Output that arrived before the pane was mapped is replayed after
        // the capture-pane seed, keeping chronology.
        let mut pending_output = VecDeque::new();
        if let Some((mut buffered, _)) = self.unknown_output.remove(&pane_key) {
            pending_output.append(&mut buffered);
        }
        let pending_bytes = pending_output.iter().map(|(_, data)| data.len()).sum();

        self.panes.insert(
            pane_key.clone(),
            VirtualPane {
                session_id: session_id.clone(),
                input_parser: InputParser::default(),
                pending_output,
                pending_bytes,
                seeded: false,
                capture_at: None,
                seeding: false,
                reseed: false,
                pending_seed: None,
                desired: None,
                pending_dims: None,
                dims: None,
                seeded_desired: None,
                resize_attempts: None,
                cwd: None,
                routed_bytes: 0,
            },
        );

        tracing::info!(
            session_id = %self.control_session_id,
            pane = %pane_id,
            virtual_session = %session_id,
            "tmux pane mapped to virtual session"
        );

        // Seed the pane's cwd from tmux's own tracking.
        self.cwd_pending.insert(pane_key.clone());
        self.arm_cwd_query();
    }

    async fn seed_pane(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>, pane_id: &str) {
        let dims = self.pane_dims(pane_id);
        if let Some(pane) = self.panes.get_mut(pane_id) {
            if pane.seeding {
                return;
            }
            pane.seeding = true;
            pane.dims = dims;
            pane.seeded_desired = pane.desired;
        }
        // `-e` keeps escape sequences, `-p` prints to stdout, `-S -` starts at
        // the beginning of the pane's scrollback.
        let command = format!("capture-pane -ep -S - -t '{pane_id}'");
        self.send_query(
            channel,
            PendingKind::CapturePane {
                pane: pane_id.to_string(),
            },
            &command,
        )
        .await;
        // Ask for the live cursor so a sparse/fresh pane does not leave the
        // terminal cursor parked at bottom-left after the seed. `cursor_y`
        // is screen-relative while the capture text includes `history_size`
        // scrollback lines on top, so the seed needs both to place the
        // cursor at the matching row.
        self.send_query(
            channel,
            PendingKind::SeedCursor {
                pane: pane_id.to_string(),
            },
            &format!(
                "display-message -p -t '{pane_id}' '#{{cursor_x}} #{{cursor_y}} #{{history_size}}'"
            ),
        )
        .await;
    }

    async fn drop_pane(&mut self, pane_id: &str) {
        let Some(pane) = self.panes.remove(pane_id) else {
            return;
        };
        SessionManager::remove_tmux_pane(self.sessions, self.event_queue, &pane.session_id);
        self.input_buf.remove(pane_id);
        self.paused_panes.remove(pane_id);
        self.unknown_output.remove(pane_id);
        self.pane_names.remove(pane_id);
        self.pane_windows.remove(pane_id);
        if self.active_pane.as_deref() == Some(pane_id) {
            self.active_pane = None;
        }
        tracing::info!(
            session_id = %self.control_session_id,
            pane = %pane_id,
            "tmux pane virtual session removed"
        );
    }

    async fn teardown_panes(&mut self) {
        let ids: Vec<String> = self.panes.keys().cloned().collect();
        for pane_id in ids {
            self.drop_pane(&pane_id).await;
        }
    }

    /// Fold a layout cell into the UI tree, mapping pane ids to virtual
    /// session ids. Panes that are not (yet) mapped render as placeholders.
    fn build_ui_tree(&self, cell: &LayoutCell) -> Option<TmuxUiNode> {
        if cell.is_leaf() {
            let pane_num = cell.pane?;
            let pane_id = format!("%{pane_num}");
            // Skip panes without a virtual session (e.g. an exited pane still
            // present in a stale layout) so splits collapse cleanly.
            let pane = self.panes.get(&pane_id)?;
            let name = self.pane_name(&pane_id);
            return Some(TmuxUiNode::Pane {
                pane_id,
                session_id: pane.session_id.clone(),
                name,
            });
        }

        let mut node: Option<TmuxUiNode> = None;
        for child in &cell.children {
            let Some(second) = self.build_ui_tree(child) else {
                continue;
            };
            node = Some(match node {
                None => second,
                Some(first) => {
                    // The divider column sits at child.offset - 1, so the
                    // first subtree's extent ends just before it. Using the
                    // raw offset would attribute the border cell to `first`
                    // and drift the ratio one cell per resize echo.
                    let span = if cell.columns {
                        cell.width
                    } else {
                        cell.height
                    };
                    let offset = if cell.columns { child.x } else { child.y };
                    let extent = offset.saturating_sub(1);
                    let ratio = if span > 0 {
                        (f64::from(extent) / f64::from(span)).clamp(0.05, 0.95)
                    } else {
                        0.5
                    };
                    TmuxUiNode::Split {
                        direction: if cell.columns {
                            "vertical"
                        } else {
                            "horizontal"
                        },
                        ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    }
                }
            });
        }
        node
    }

    fn pane_name(&self, pane_id: &str) -> String {
        self.pane_names
            .get(pane_id)
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("tmux {pane_id}"))
    }

    fn emit_state(&mut self) {
        let windows = {
            let mut list: Vec<TmuxWindowInfo> = self
                .windows
                .iter()
                .map(|(id, window)| TmuxWindowInfo {
                    id: id.clone(),
                    name: window.name.clone(),
                    index: window.index,
                    active: Some(id) == self.active_window.as_ref(),
                    tree: window
                        .visible_layout
                        .as_ref()
                        .or(window.layout.as_ref())
                        .and_then(|layout| self.build_ui_tree(layout)),
                })
                .collect();
            // Order by `#{window_index}` — the number users see in `N:name`
            // labels; renumbering (`move-window`) must reorder tabs too.
            list.sort_by_key(|window| window.index);
            list
        };

        let payload = TmuxStatePayload {
            control_session_id: self.control_session_id.to_string(),
            exited: false,
            windows,
            active_window_id: self.active_window.clone(),
            active_pane_session_id: self
                .active_pane
                .as_ref()
                .and_then(|pane| self.panes.get(pane))
                .map(|pane| pane.session_id.clone()),
        };
        if self.last_emitted.as_ref() == Some(&payload) {
            return;
        }
        tracing::info!(
            session_id = %self.control_session_id,
            windows = payload.windows.len(),
            active_window = ?payload.active_window_id,
            has_tree = payload.windows.iter().any(|window| window.tree.is_some()),
            "tmux emit state"
        );
        self.last_emitted = Some(payload.clone());
        self.event_queue.push(SessionEvent::TmuxState {
            session_id: self.control_session_id.to_string(),
            state: Box::new(payload),
        });
    }

    fn emit_exited(&self) {
        let payload = TmuxStatePayload {
            control_session_id: self.control_session_id.to_string(),
            exited: true,
            windows: Vec::new(),
            active_window_id: None,
            active_pane_session_id: None,
        };
        self.event_queue.push(SessionEvent::TmuxState {
            session_id: self.control_session_id.to_string(),
            state: Box::new(payload),
        });
    }

    /// Reconcile virtual panes with every known window's layout — each tmux
    /// window maps to a native tab, so background windows' panes stay alive
    /// and keep receiving their `%output` instead of being torn down.
    async fn reconcile_panes(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        let (active_layout, zoomed) = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
            .map(|window| (window.layout.clone(), window.zoomed))
            .unwrap_or((None, false));
        let mut wanted = HashSet::new();
        for window in self.windows.values() {
            if let Some(layout) = window.layout.as_ref() {
                wanted.extend(layout.pane_ids().iter().map(|id| format!("%{id}")));
            }
        }

        // The client must cover the active window before any capture below
        // runs: while the window outgrows the client, tmux clips panes and
        // marks clipped edges with `#`, and it never re-pushes `%output`
        // after a resize — a capture taken pre-grow would seed those marks
        // forever. Skipped while zoomed since any client resize drops the
        // zoom, and skipped once any local renderer has reported its real
        // size (`sync_pane_sizes` then owns `client_size`, so switching to a
        // background window with a larger stale layout must not bounce the
        // client back up before `sync_pane_sizes` shrinks it).
        let has_renderer_size = self.panes.values().any(|pane| pane.desired.is_some());
        let mut grew = false;
        if !zoomed
            && !has_renderer_size
            && let Some(layout) = active_layout.as_ref()
        {
            grew = self.grow_client(channel, layout.width, layout.height).await;
        }

        // Drop panes that vanished from every window's layout.
        let existing: Vec<String> = self.panes.keys().cloned().collect();
        for pane_id in existing {
            if !wanted.contains(&pane_id) {
                self.drop_pane(&pane_id).await;
            }
        }
        // Spawn panes we don't know yet.
        for pane_id in &wanted {
            if !self.panes.contains_key(pane_id) {
                self.spawn_pane(pane_id).await;
            }
        }
        if grew {
            // Panes seeded while the client was still too small may contain
            // '#' clipping marks; the resize produces no redraw `%output`,
            // so re-capture them. Panes whose seed is in flight run on the
            // pre-grow snapshot — flag them for a second seed when it lands.
            // Only the active window's panes are clipped by client size.
            if let Some(layout) = active_layout.as_ref() {
                for pane_num in layout.pane_ids() {
                    let pane_id = format!("%{pane_num}");
                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                        if pane.seeding {
                            pane.reseed = true;
                        } else {
                            pane.seeded = false;
                        }
                    }
                }
            }
        }
        // A pane whose cell resized since its seed renders rows that were
        // laid out for the old width; capture again at the new size.
        for pane_id in &wanted {
            let Some(dims) = self.pane_dims(pane_id) else {
                continue;
            };
            if let Some(pane) = self.panes.get_mut(pane_id)
                && pane.dims.is_some()
                && pane.dims != Some(dims)
            {
                if pane.seeding {
                    pane.reseed = true;
                } else {
                    pane.seeded = false;
                }
            }
        }
        let to_seed: Vec<String> = wanted
            .iter()
            .filter(|pane_id| {
                self.panes
                    .get(*pane_id)
                    .is_some_and(|pane| !pane.seeded && !pane.seeding)
            })
            .cloned()
            .collect();
        for pane_id in to_seed {
            self.seed_pane(channel, &pane_id).await;
        }
        self.sync_pane_sizes(channel).await;
        self.emit_state();
    }

    /// A pane's cell size inside whichever window's layout contains it.
    fn pane_dims(&self, pane_id: &str) -> Option<(u32, u32)> {
        let num = pane_id.trim_start_matches('%').parse::<u32>().ok()?;
        self.windows
            .values()
            .filter_map(|window| window.layout.as_ref())
            .find_map(|layout| layout.find_pane(num))
            .map(|cell| (cell.width, cell.height))
    }

    /// Grow the control client to fit the window layout. Shrinking would
    /// make tmux rebalance panes and echo `%layout-change`, so we only ever
    /// grow. Returns true when a `refresh-client -C` was actually sent.
    async fn grow_client(
        &mut self,
        channel: &Arc<ChannelWriteHalf<client::Msg>>,
        width: u32,
        height: u32,
    ) -> bool {
        let current = self.client_size.unwrap_or((0, 0));
        let grown = (width.max(current.0), height.max(current.1));
        if grown == current {
            return false;
        }
        if !self
            .send_line(
                channel,
                PendingKind::Ignore,
                &format!("refresh-client -C {}x{}", grown.0, grown.1),
            )
            .await
        {
            return false;
        }
        self.client_size = Some(grown);
        true
    }

    /// Drive pane dimensions toward the sizes reported by their renderers.
    ///
    /// `SessionCommand::Resize` records each xterm's cell size, but the tmux
    /// layout owns actual pane dims; a mismatch wraps full-width output onto
    /// the wrong xterm row (how zsh's `\r`-erased `#`/`%` markers survive).
    /// First the client — and with it the window, under the default
    /// `window-size=latest` — is sized to the fold of all renderer demands;
    /// once it matches, per-pane drift left by tmux's proportional scaling
    /// is corrected with `resize-pane`.
    async fn sync_pane_sizes(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        // Client sizing only concerns the window tmux currently shows the
        // client — i.e. the active one. Background windows keep their own
        // geometry until their tab is activated.
        if let Some(window) = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
        {
            // Resizing a zoomed window would drop the zoom.
            if !window.zoomed
                && let Some(layout) = window.layout.clone()
            {
                let needed = needed_window_size(&layout, &self.panes);
                if needed != (layout.width, layout.height) {
                    // Unlike grow_client this also shrinks: renderers may
                    // be narrower than the window's current share.
                    // `refresh-client` only emits `%layout-change` when
                    // the resize actually takes effect, so this
                    // self-limits against remotes ignoring it. The echo
                    // reruns the per-pane fixup below.
                    if self
                        .send_line(
                            channel,
                            PendingKind::Ignore,
                            &format!("refresh-client -C {}x{}", needed.0, needed.1),
                        )
                        .await
                    {
                        self.client_size = Some(needed);
                    }
                    return;
                }
            }
        }

        // The layout already matches the renderers' demand; fix whatever
        // drift remains per pane, in every window — background panes keep
        // live renderers too. Targets are collected first because send_line
        // borrows self mutably.
        let mut targets = Vec::new();
        for window in self.windows.values() {
            if window.zoomed {
                continue;
            }
            let Some(layout) = window.layout.as_ref() else {
                continue;
            };
            for pane_num in layout.pane_ids() {
                let Some(cell) = layout.find_pane(pane_num) else {
                    continue;
                };
                let pane_id = format!("%{pane_num}");
                let Some(pane) = self.panes.get_mut(&pane_id) else {
                    continue;
                };
                let Some(desired) = pane.desired else {
                    continue;
                };
                if desired == (cell.width, cell.height) {
                    pane.resize_attempts = None;
                    continue;
                }
                let attempts = match pane.resize_attempts {
                    Some((target, count)) if target == desired => count,
                    _ => 0,
                };
                if attempts >= RESIZE_PANE_MAX_ATTEMPTS {
                    continue;
                }
                pane.resize_attempts = Some((desired, attempts + 1));
                targets.push((pane_id, desired));
            }
        }
        for (pane_id, (cols, rows)) in targets {
            self.send_line(
                channel,
                PendingKind::Ignore,
                &format!("resize-pane -t '{pane_id}' -x {cols} -y {rows}"),
            )
            .await;
        }
    }

    async fn handle_message(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>, line: String) {
        // Control lines carry remote terminal content — never log them raw.
        tracing::trace!(
            session_id = %self.control_session_id,
            bytes = line.len(),
            "tmux control: line"
        );
        // Command-response blocks capture all lines between begin/end.
        if let Some(lines) = self.block_lines.as_mut() {
            let message = parse_line(&line);
            match message {
                Some(ControlMessage::End(id) | ControlMessage::Error(id))
                    if self.block_id == Some(id) =>
                {
                    let collected = std::mem::take(lines);
                    let failed = matches!(message, Some(ControlMessage::Error(_)));
                    self.block_lines = None;
                    self.block_id = None;
                    let kind = self
                        .pending_responses
                        .pop_front()
                        .unwrap_or(PendingKind::Ignore);
                    self.handle_block(channel, kind, collected, failed).await;
                }
                Some(
                    ControlMessage::Begin(_) | ControlMessage::End(_) | ControlMessage::Error(_),
                ) => {
                    tracing::warn!("tmux control: mismatched response marker");
                }
                Some(ControlMessage::Exit { .. }) => {
                    self.mark_exit();
                }
                _ => {
                    lines.push(line);
                }
            }
            return;
        }

        let Some(message) = parse_line(&line) else {
            tracing::debug!(session_id = %self.control_session_id, bytes = line.len(), "tmux control: unparsed line");
            return;
        };
        tracing::debug!(
            session_id = %self.control_session_id,
            message = ?std::mem::discriminant(&message),
            "tmux control: message"
        );
        match message {
            ControlMessage::Begin(id) => {
                self.block_id = Some(id);
                self.block_lines = Some(Vec::new());
                self.block_started_at = Some(std::time::Instant::now());
            }
            ControlMessage::End(_) | ControlMessage::Error(_) => {
                // Unsolicited end without a begin: ignore.
            }
            ControlMessage::Output { pane, data } => {
                self.route_output(&pane, None, data);
            }
            ControlMessage::ExtendedOutput { pane, age, data } => {
                let produced_at = std::time::Instant::now()
                    .checked_sub(Duration::from_millis(age))
                    .unwrap_or_else(std::time::Instant::now);
                self.route_output(&pane, Some(produced_at), data);
            }
            ControlMessage::LayoutChange {
                window,
                layout,
                visible_layout,
                flags,
            } => {
                let entry = self.windows.entry(window.clone()).or_insert(WindowState {
                    name: String::new(),
                    index: 0,
                    layout: None,
                    visible_layout: None,
                    zoomed: false,
                });
                entry.zoomed = flags.contains('Z');
                entry.layout = Some(layout);
                entry.visible_layout = Some(visible_layout);
                // If we have no active window yet (attach raced the
                // list-windows answer), assume this one is current.
                if self.active_window.is_none() {
                    self.active_window = Some(window.clone());
                }
                // Reconcile for any window's layout change — background
                // windows' panes are live too (each maps to a native tab).
                // reconcile_panes also grows the client when the active
                // layout outgrows it — that is what removes `#` clipping.
                self.reconcile_panes(channel).await;
            }
            ControlMessage::WindowAdd { window } => {
                self.windows.entry(window).or_insert(WindowState {
                    name: String::new(),
                    index: 0,
                    layout: None,
                    visible_layout: None,
                    zoomed: false,
                });
                self.arm_refresh();
            }
            ControlMessage::WindowClose { window } => {
                self.windows.remove(&window);
                if self.active_window.as_ref() == Some(&window) {
                    // tmux selects another window automatically; the follow-up
                    // notifications drive the rebuild.
                    self.active_window = None;
                }
                self.arm_refresh();
            }
            ControlMessage::WindowRenamed { window, name } => {
                if let Some(entry) = self.windows.get_mut(&window) {
                    entry.name = name;
                    self.emit_state();
                }
            }
            ControlMessage::WindowPaneChanged { window, pane } => {
                if self.active_window.as_ref() == Some(&window) {
                    self.active_pane = Some(pane);
                    self.emit_state();
                }
            }
            ControlMessage::PaneExited { pane } => {
                self.drop_pane(&pane).await;
                self.emit_state();
            }
            ControlMessage::SessionChanged { .. } => {
                // Attached to a different session: rebuild from scratch.
                self.windows.clear();
                self.pane_windows.clear();
                self.pane_names.clear();
                self.active_window = None;
                self.active_pane = None;
                self.unknown_output.clear();
                self.arm_refresh();
            }
            ControlMessage::SessionRenamed { .. } | ControlMessage::SessionsChanged => {
                self.arm_refresh();
            }
            ControlMessage::SessionWindowChanged { window } => {
                self.active_window = Some(window.clone());
                self.active_pane = None;
                self.arm_refresh();
                // Only reconcile when we already have a layout; otherwise a
                // follow-up layout-change / list-windows will drive it and we
                // avoid tearing panes down for a window we cannot render yet.
                if self
                    .windows
                    .get(&window)
                    .and_then(|window| window.layout.as_ref())
                    .is_some()
                {
                    self.reconcile_panes(channel).await;
                }
            }
            ControlMessage::PaneModeChanged { .. }
            | ControlMessage::ClientSessionChanged
            | ControlMessage::Ignored => {}
            ControlMessage::ClientDetached { client } => {
                log_server_notice(self.control_session_id, "client-detached", client.len());
            }
            ControlMessage::Pause { pane } => {
                // Flow control kicks in only if we enabled `pause-after`;
                // give the renderer a moment then let tmux continue.
                self.paused_panes.insert(pane);
                if self.continue_deadline.is_none() {
                    self.continue_deadline = Some(Box::pin(tokio::time::sleep(
                        Duration::from_millis(PAUSE_CONTINUE_DELAY_MS),
                    )));
                }
            }
            ControlMessage::Continue { pane } => {
                self.paused_panes.remove(&pane);
            }
            ControlMessage::ConfigError { text } => {
                log_server_notice(self.control_session_id, "config-error", text.len());
            }
            ControlMessage::Message { text } => {
                log_server_notice(self.control_session_id, "message", text.len());
            }
            ControlMessage::Exit { reason } => {
                log_server_notice(
                    self.control_session_id,
                    "exit",
                    reason.as_ref().map_or(0, String::len),
                );
                self.mark_exit();
            }
        }
    }

    fn mark_exit(&mut self) {
        self.exit_seen = true;
    }

    /// Emit raw pane bytes; the per-session frame pipeline owns decoding.
    /// Free-standing so `self.panes` mut borrows can coexist with the emit.
    fn push_pane_output(queue: &SessionEventQueue, pane_session_id: &str, data: Vec<u8>) {
        queue.push(SessionEvent::Output {
            session_id: pane_session_id.to_string(),
            data,
        });
    }

    /// Visible status text on the host terminal behind the control session.
    fn emit_visible_text(&self, text: &str) {
        self.event_queue.push(SessionEvent::Output {
            session_id: self.control_session_id.to_string(),
            data: text.as_bytes().to_vec(),
        });
    }

    fn route_output(&mut self, pane: &str, produced_at: Option<std::time::Instant>, data: Vec<u8>) {
        if let Some(virtual_pane) = self.panes.get_mut(pane) {
            // Not-yet-seeded panes keep raw bytes: decoding is stateful and
            // must see each byte exactly once.
            if virtual_pane.seeded {
                // A notification provably produced before the last capture is
                // already painted by the seed — tmux defers such blocks past
                // the command's `%end`, so arrival order alone cannot tell.
                if output_precedes_capture(produced_at, virtual_pane.capture_at) {
                    return;
                }
                Self::push_pane_output(self.event_queue, &virtual_pane.session_id, data.clone());
                virtual_pane.routed_bytes += data.len();
                if virtual_pane.routed_bytes < 8192 {
                    tracing::info!(
                        session_id = %self.control_session_id,
                        pane = %pane,
                        bytes = data.len(),
                        "tmux output routed to pane"
                    );
                }
            } else {
                virtual_pane.pending_bytes += data.len();
                virtual_pane.pending_output.push_back((produced_at, data));
                // Seeding under heavy output must not grow the queue without
                // bound: spill the oldest bytes straight to the pane — a
                // possibly duplicated line beats dropping output.
                while virtual_pane.pending_bytes > PENDING_PANE_OUTPUT_MAX_BYTES {
                    let Some((_, spilled)) = virtual_pane.pending_output.pop_front() else {
                        break;
                    };
                    virtual_pane.pending_bytes -= spilled.len();
                    Self::push_pane_output(self.event_queue, &virtual_pane.session_id, spilled);
                }
            }
        } else {
            // Pane output can arrive before its layout (attach, or panes in
            // background windows). Buffer a bounded amount keyed by pane id;
            // past the cap the oldest bytes are dropped — the tail matters
            // most since it replays right after the `capture-pane` seed.
            let (pending, total) = self
                .unknown_output
                .entry(pane.to_string())
                .or_insert_with(|| (VecDeque::new(), 0));
            buffer_unknown_output(pending, total, produced_at, data);
        }
    }

    async fn handle_block(
        &mut self,
        channel: &Arc<ChannelWriteHalf<client::Msg>>,
        kind: PendingKind,
        lines: Vec<String>,
        failed: bool,
    ) {
        match kind {
            PendingKind::WindowsRefresh => {
                if failed {
                    return;
                }
                self.apply_window_list(lines);
                self.reconcile_panes(channel).await;
            }
            PendingKind::PanesRefresh => {
                for line in lines {
                    let mut parts = line.splitn(3, ' ');
                    let (Some(pane), Some(window), Some(name)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    self.pane_windows
                        .insert(pane.to_string(), window.to_string());
                    self.pane_names.insert(pane.to_string(), name.to_string());
                }
                self.emit_state();
            }
            PendingKind::CapturePane { pane } => {
                if let Some(virtual_pane) = self.panes.get_mut(&pane)
                    && !failed
                {
                    // Held until the SeedCursor reply arrives: the seed is
                    // composed as a single payload there, because the
                    // scrollback/screen split needs `history_size`.
                    virtual_pane.pending_seed = Some(lines);
                    // Snapshot boundary for output dedup: the capture ran
                    // after the block opened, so output produced before
                    // %begin is guaranteed inside it; anything later may
                    // be genuinely new (tmux defers it past %end) and must
                    // be replayed rather than dropped.
                    virtual_pane.capture_at = Some(
                        self.block_started_at
                            .unwrap_or_else(std::time::Instant::now),
                    );
                }
            }
            PendingKind::SeedCursor { pane } => {
                let mut reseed = false;
                if let Some(virtual_pane) = self.panes.get_mut(&pane) {
                    virtual_pane.seeding = false;
                    reseed = std::mem::take(&mut virtual_pane.reseed);
                    if !reseed {
                        let cursor = (!failed).then(|| lines.first()).flatten().and_then(|line| {
                            let mut parts = line.split_whitespace();
                            let x = parts.next()?.parse::<u32>().ok()?;
                            let y = parts.next()?.parse::<u32>().ok()?;
                            let history = parts
                                .next()
                                .and_then(|t| t.parse::<usize>().ok())
                                .unwrap_or(0);
                            Some((x, y, history))
                        });
                        let wrote_seed = if let Some(seed_lines) = virtual_pane.pending_seed.take()
                        {
                            let (x, y, history) = cursor.unwrap_or_default();
                            let screen_rows = virtual_pane
                                .desired
                                .or(virtual_pane.dims)
                                .map(|(_, rows)| rows as usize);
                            let text =
                                compose_seed_payload(&seed_lines, x, y, history, screen_rows);
                            Self::push_pane_output(
                                self.event_queue,
                                &virtual_pane.session_id,
                                text.into_bytes(),
                            );
                            true
                        } else {
                            false
                        };
                        // A failed capture wrote nothing; leaving the pane
                        // unseeded lets the next attach or layout pass retry.
                        virtual_pane.seeded = wrote_seed;
                    }
                    if !reseed {
                        for chunk in virtual_pane.take_pending_output() {
                            Self::push_pane_output(
                                self.event_queue,
                                &virtual_pane.session_id,
                                chunk,
                            );
                        }
                    }
                }
                if reseed {
                    self.seed_pane(channel, &pane).await;
                }
            }
            PendingKind::PaneCwd { pane } => {
                if !failed && let Some(path) = lines.first() {
                    let path = path.trim();
                    if !path.is_empty()
                        && let Some(pane) = self.panes.get_mut(&pane)
                        && pane.cwd.as_deref() != Some(path)
                    {
                        pane.cwd = Some(path.to_string());
                        self.event_queue.push(SessionEvent::CwdChanged {
                            session_id: pane.session_id.clone(),
                            cwd: path.to_string(),
                        });
                    }
                }
            }
            PendingKind::UserCommand => {
                if failed && !lines.is_empty() {
                    // Surface the tmux error on the host terminal so a failed
                    // `select-window`/`new-window` is not silently lost.
                    let text = format!("\r\n[tmux] {}\r\n", lines.join("\n"));
                    self.emit_visible_text(&text);
                }
            }
            PendingKind::Ignore => {}
        }
    }

    fn apply_window_list(&mut self, lines: Vec<String>) {
        let snapshot = parse_window_snapshot(lines);
        self.windows = snapshot.windows;
        self.active_window = snapshot.active_window;
        self.active_pane = snapshot.active_pane;
        self.emit_state();
    }

    async fn flush_input(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        let panes: Vec<String> = self.input_buf.keys().cloned().collect();
        for pane_id in panes {
            let data = self.input_buf.remove(&pane_id).unwrap_or_default();
            if data.is_empty() {
                continue;
            }
            // A submitted line may have changed the pane's working directory;
            // tmux emits no event for that, so poll `pane_current_path` once
            // the command has had a moment to run.
            if data.iter().any(|b| matches!(b, b'\r' | b'\n')) {
                self.cwd_pending.insert(pane_id.clone());
                self.arm_cwd_query();
            }
            let Some(pane) = self.panes.get_mut(&pane_id) else {
                continue;
            };
            let segments = pane.input_parser.split_input(&data);
            for segment in segments {
                match segment {
                    InputSegment::Literal(bytes) => {
                        for chunk in bytes.chunks(SEND_KEYS_CHUNK_BYTES) {
                            let mut line = String::from("send-keys -H -t ");
                            line.push_str(&pane_id);
                            for byte in chunk {
                                let _ = write!(line, " {byte:02x}");
                            }
                            self.send_line(channel, PendingKind::Ignore, &line).await;
                        }
                    }
                    // A named key is re-encoded by tmux for the pane's TERM,
                    // matching the translation a normal tmux client applies.
                    InputSegment::Key(name) => {
                        let line = format!("send-keys -t {pane_id} {name}");
                        self.send_line(channel, PendingKind::Ignore, &line).await;
                    }
                }
            }
        }
        self.input_flush = None;
    }

    fn arm_cwd_query(&mut self) {
        if self.cwd_deadline.is_none() {
            self.cwd_deadline = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                CWD_QUERY_DELAY_MS,
            ))));
        }
    }

    async fn flush_cwd_queries(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        self.cwd_deadline = None;
        let panes: Vec<String> = self.cwd_pending.drain().collect();
        for pane_id in panes {
            if !self.panes.contains_key(&pane_id) {
                continue;
            }
            self.send_query(
                channel,
                PendingKind::PaneCwd {
                    pane: pane_id.clone(),
                },
                &format!("display-message -p -t '{pane_id}' '#{{pane_current_path}}'"),
            )
            .await;
        }
    }

    async fn continue_paused(&mut self, channel: &Arc<ChannelWriteHalf<client::Msg>>) {
        let panes: Vec<String> = self.paused_panes.iter().cloned().collect();
        for pane in panes {
            self.send_line(
                channel,
                PendingKind::Ignore,
                &format!("refresh-client -A '{pane}:continue'"),
            )
            .await;
        }
        self.paused_panes.clear();
        self.continue_deadline = None;
    }

    /// Input routed to a pane — either from its own virtual session
    /// (`TmuxPaneInput`) or from the bare host session id, which users and
    /// tools resolve to the pane they are looking at.
    fn buffer_pane_input(&mut self, pane_id: &str, data: &[u8]) {
        if !data.is_empty() && self.seen_input.insert(pane_id.to_string()) {
            tracing::info!(
                session_id = %self.control_session_id,
                pane = %pane_id,
                bytes = data.len(),
                "tmux pane input received"
            );
        }
        self.input_buf
            .entry(pane_id.to_string())
            .or_default()
            .extend_from_slice(data);
        if self.input_flush.is_none() {
            self.input_flush = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                INPUT_FLUSH_DELAY_MS,
            ))));
        }
    }

    async fn handle_pane_resize(
        &mut self,
        channel: &Arc<ChannelWriteHalf<client::Msg>>,
        pane_id: String,
        cols: u32,
        rows: u32,
    ) {
        let layout_dims = self.pane_dims(&pane_id);
        let mut immediate_reseed = false;
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            if layout_dims == Some((cols, rows)) {
                // The renderer just fitted to the exact dimensions
                // the remote tmux layout already has (e.g. a hidden
                // background window tab becoming visible for the
                // first time). Promote immediately without the 250 ms
                // debounce; if the previous seed ran while the
                // renderer was still at its hidden default 80x24
                // (`seeded_desired != Some((cols, rows))`), reseed
                // now so rows and cursor align with the fitted xterm.
                pane.pending_dims = None;
                if pane.desired != Some((cols, rows)) {
                    pane.desired = Some((cols, rows));
                    pane.resize_attempts = None;
                }
                if pane.seeded_desired != Some((cols, rows)) {
                    if pane.seeding {
                        pane.reseed = true;
                    } else {
                        pane.seeded = false;
                        immediate_reseed = true;
                    }
                }
            } else if pane.pending_dims != Some((cols, rows)) {
                pane.pending_dims = Some((cols, rows));
                self.resize_deadline = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                    RESIZE_SYNC_DEBOUNCE_MS,
                ))));
            }
        }
        if immediate_reseed {
            self.seed_pane(channel, &pane_id).await;
        }
    }

    async fn request_detach(
        &mut self,
        channel: &Arc<ChannelWriteHalf<client::Msg>>,
        mode: ExitMode,
    ) {
        if self.exit_mode.is_some() {
            return;
        }
        self.exit_mode = Some(mode);
        let _ = self
            .send_line(channel, PendingKind::Ignore, "detach-client")
            .await;
        self.detach_deadline = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
            DETACH_EXIT_TIMEOUT_MS,
        ))));
    }

    /// Cleanup after `%exit` or channel termination: drop all virtual panes.
    async fn teardown(&mut self) {
        self.teardown_panes().await;
        self.emit_exited();
    }
}

/// Drive the control session until `%exit` or channel termination.
///
/// `host_rx` keeps the ordinary SSH mailbox draining (user input is routed to
/// the active pane, resize grows the client, close ends the session) while
/// `tmux_cmd_rx` carries the ordered control-channel commands shared by the
/// UI and every projected pane.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_control_session(
    event_queue: &SessionEventQueue,
    sessions: &Arc<Mutex<HashMap<String, ManagedSession>>>,
    session_id: &str,
    channel: &Arc<ChannelWriteHalf<client::Msg>>,
    channel_rx: &mut ChannelReadHalf,
    host_rx: &mut ssh_mailbox::Receiver,
    tmux_cmd_rx: &mut TmuxCommandReceiver,
    tmux_tx: TmuxCommandSender,
    initial: &[u8],
) -> ControlExit {
    let mut session = ControlSession::new(event_queue, session_id, sessions, tmux_tx);

    tracing::info!(
        session_id = %session_id,
        "SSH channel switched to tmux control mode"
    );

    // Bootstrap: adopt the client's size management and get a deterministic
    // picture of windows/panes (attach notifications alone are not enough).
    session
        .send_line(
            channel,
            PendingKind::Ignore,
            "refresh-client -f pause-after=2",
        )
        .await;
    session.refresh_lists(channel).await;

    let mut window_poll = tokio::time::interval(WINDOW_POLL_INTERVAL);
    window_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    window_poll.tick().await;

    // Feed the bytes that arrived together with the first notification.
    session.parser.push(initial);
    while let Some(line) = session.parser.next_line() {
        session.handle_message(channel, line).await;
        if session.exit_seen {
            break;
        }
    }

    let exit = loop {
        if session.exit_seen {
            let leftover = session.parser.take_pending();
            break match session.exit_mode {
                Some(ExitMode::Close) => ControlExit::Closed,
                _ => ControlExit::ReturnToShell { leftover },
            };
        }

        tokio::select! {
            biased;

            () = async {
                if let Some(deadline) = session.detach_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.detach_deadline.is_some() => {
                // detach-client never produced %exit: fall back to closing.
                break ControlExit::Closed;
            }

            _ = window_poll.tick(), if session.exit_mode.is_none() => {
                session.refresh_windows(channel).await;
            }

            () = async {
                if let Some(deadline) = session.input_flush.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.input_flush.is_some() => {
                session.flush_input(channel).await;
            }

            () = async {
                if let Some(deadline) = session.refresh_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.refresh_deadline.is_some() => {
                session.refresh_deadline = None;
                session.refresh_lists(channel).await;
            }

            () = async {
                if let Some(deadline) = session.continue_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.continue_deadline.is_some() => {
                session.continue_deadline = None;
                session.continue_paused(channel).await;
            }

            () = async {
                if let Some(deadline) = session.resize_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.resize_deadline.is_some() => {
                session.resize_deadline = None;
                // Promote the settled renderer reports into resize targets.
                for pane in session.panes.values_mut() {
                    if let Some(dims) = pane.pending_dims.take()
                        && pane.desired != Some(dims)
                    {
                        pane.desired = Some(dims);
                        pane.resize_attempts = None;
                    }
                }
                session.sync_pane_sizes(channel).await;
                // If a pane's settled renderer size already matches its remote
                // layout (so `sync_pane_sizes` sent no resize and no
                // `%layout-change` will follow), but the pane was last seeded
                // at a different renderer size, reseed it now.
                let to_reseed: Vec<String> = session
                    .panes
                    .keys()
                    .filter(|pane_id| {
                        let layout_dims = session.pane_dims(pane_id);
                        session.panes.get(*pane_id).is_some_and(|pane| {
                            pane.desired.is_some()
                                && pane.desired == layout_dims
                                && pane.seeded_desired != pane.desired
                        })
                    })
                    .cloned()
                    .collect();
                for pane_id in to_reseed {
                    if let Some(pane) = session.panes.get_mut(&pane_id) {
                        if pane.seeding {
                            pane.reseed = true;
                            continue;
                        }
                        pane.seeded = false;
                    }
                    session.seed_pane(channel, &pane_id).await;
                }
            }

            () = async {
                if let Some(deadline) = session.cwd_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.cwd_deadline.is_some() => {
                session.flush_cwd_queries(channel).await;
            }

            command = tmux_cmd_rx.recv() => {
                match command {
                    Some(SshCommand::TmuxCommand { line }) => {
                        let clean = line.trim_matches(['\r', '\n']);
                        if !clean.is_empty() {
                            session
                                .send_line(channel, PendingKind::UserCommand, clean)
                                .await;
                        }
                    }
                    Some(SshCommand::TmuxDetach) => {
                        session.request_detach(channel, ExitMode::Detach).await;
                    }
                    Some(SshCommand::TmuxPaneInput { pane, data }) => {
                        session.buffer_pane_input(&pane, &data);
                    }
                    Some(SshCommand::TmuxPaneResize {
                        pane,
                        cols,
                        rows,
                        ..
                    }) => {
                        session
                            .handle_pane_resize(channel, pane, cols.into(), rows.into())
                            .await;
                    }
                    Some(SshCommand::TmuxPaneClose { pane }) => {
                        // Closing one pane leaf kills the tmux pane; the
                        // resulting layout-change rebuilds the tree.
                        session
                            .send_line(
                                channel,
                                PendingKind::Ignore,
                                &format!("kill-pane -t '{pane}'"),
                            )
                            .await;
                        session.drop_pane(&pane).await;
                        session.emit_state();
                    }
                    Some(SshCommand::Write(_) | SshCommand::Resize { .. }) => {
                        // Host-session commands belong on the mailbox channel
                        // (`host_rx`), never here; ignore stray sends.
                    }
                    Some(SshCommand::Close) | None => {
                        // The channel sender set is gone — the session itself
                        // is being torn down.
                        break ControlExit::Closed;
                    }
                }
            }

            command = host_rx.recv() => {
                match command {
                    Some(SshCommand::Write(write)) => {
                        // The control channel itself has no pty — input aimed
                        // at the bare session id means the pane the user is
                        // looking at. Forward to the active pane.
                        if let Some(pane_id) = session.active_pane.clone() {
                            session.buffer_pane_input(&pane_id, &write.bytes);
                        }
                    }
                    Some(SshCommand::Resize { cols, rows, .. }) => {
                        // Grow/shrink the tmux client to the host surface so
                        // layouts stay faithful to what the user sees.
                        session
                            .send_line(
                                channel,
                                PendingKind::Ignore,
                                &format!("refresh-client -C {cols},{rows}"),
                            )
                            .await;
                    }
                    Some(SshCommand::Close) | None => {
                        session.request_detach(channel, ExitMode::Close).await;
                    }
                    Some(_) => {}
                }
            }

            msg = channel_rx.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        session.parser.push(&data);
                        while let Some(line) = session.parser.next_line() {
                            session.handle_message(channel, line).await;
                            if session.exit_seen { break; }
                        }
                    }
                    None | Some(ChannelMsg::Eof | ChannelMsg::Close) => {
                        break ControlExit::Closed;
                    }
                    _ => {}
                }
            }
        }
    };

    session.teardown().await;
    exit
}

/// Compose the terminal payload that paints a `capture-pane -ep -S -` snapshot
/// into xterm and restores the cursor.
///
/// Two invariants keep the prompt and cursor aligned even when the renderer's
/// current height differs from the remote tmux pane (e.g. a background window
/// tab still at default 80x24 while hidden, or a host container shortened by
/// `TmuxWindowStrip`):
///
/// 1. Trailing blank screen rows are trimmed before joining with `\r\n`.
///    `capture-pane` emits all `pane_height` rows of the grid, including blank
///    rows below the cursor; writing those trailing `\r\n`s into a shorter
///    xterm scrolls the top rows (where the prompt sits) into scrollback, after
///    which `\x1b[{y+1};{x+1}H` lands on an empty line at the right column.
/// 2. When scrollback rows (`history > 0`) precede the screen portion, only
///    `hist.min(screen_rows)` newlines are emitted at the bottom row to push
///    the visible tail into xterm's scrollback without injecting blank lines.
fn compose_seed_payload(
    seed_lines: &[String],
    x: u32,
    y: u32,
    history: usize,
    screen_rows: Option<usize>,
) -> String {
    let hist = history.min(seed_lines.len());
    // Reset + clear first: this makes the seed idempotent for remounted
    // terminals and for re-captures after a client grow (which must overwrite
    // '#' clipping marks already on screen).
    let mut text = String::from("\x1b[H\x1b[2J\x1b[3J");
    if hist > 0 {
        let rows = screen_rows
            .unwrap_or_else(|| seed_lines.len().saturating_sub(hist))
            .max(1);
        text.push_str(&seed_lines[..hist].join("\r\n"));
        text.push_str("\x1b[r\x1b[9999B");
        text.push_str(&"\n".repeat(hist.min(rows)));
        text.push_str("\x1b[H");
    }
    let screen = &seed_lines[hist..];
    if let Some(last) = screen.iter().rposition(|line| !line.is_empty()) {
        text.push_str(&screen[..=last].join("\r\n"));
    }
    text.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
    text
}

/// Window dimensions under which every pane could take its renderer's size:
/// each leaf contributes its reported size when known and its current cell
/// dimensions otherwise, inner cells add back the divider lines tmux draws
/// between children.
fn needed_window_size(cell: &LayoutCell, panes: &HashMap<String, VirtualPane>) -> (u32, u32) {
    if cell.is_leaf() {
        return cell
            .pane
            .and_then(|num| panes.get(&format!("%{num}")))
            .and_then(|pane| pane.desired)
            .unwrap_or((cell.width, cell.height));
    }
    let (mut width, mut height) = (0, 0);
    for (index, child) in cell.children.iter().enumerate() {
        let (child_w, child_h) = needed_window_size(child, panes);
        if cell.columns {
            width += child_w + u32::from(index > 0);
            height = height.max(child_h);
        } else {
            height += child_h + u32::from(index > 0);
            width = width.max(child_w);
        }
    }
    (width, height)
}

#[derive(Default)]
struct WindowSnapshot {
    windows: HashMap<String, WindowState>,
    active_window: Option<String>,
    active_pane: Option<String>,
}

/// `window_name` is last because it may contain spaces. Empty flags still
/// occupy their field; tmux replaces control characters in format output.
fn parse_window_snapshot(lines: Vec<String>) -> WindowSnapshot {
    let mut snapshot = WindowSnapshot::default();
    for line in lines {
        let mut fields = line.splitn(8, ' ');
        let parsed = (|| {
            let id = fields.next()?;
            id.strip_prefix('@')?.parse::<u32>().ok()?;
            let index = fields.next()?.parse::<u32>().ok()?;
            let active = match fields.next()? {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            let (layout, _) = super::control::parse_layout(fields.next()?)?;
            let visible_layout = super::control::parse_layout(fields.next()?)
                .map(|(cell, _)| cell)
                .unwrap_or_else(|| layout.clone());
            let flags = fields.next()?;
            let pane = fields.next()?;
            pane.strip_prefix('%')?.parse::<u32>().ok()?;
            let name = fields.next().unwrap_or_default();
            Some((
                id.to_string(),
                WindowState {
                    name: name.to_string(),
                    index,
                    layout: Some(layout),
                    visible_layout: Some(visible_layout),
                    zoomed: flags.contains('Z'),
                },
                active,
                pane.to_string(),
            ))
        })();
        if let Some((id, window, active, pane)) = parsed {
            if active {
                snapshot.active_window = Some(id.clone());
                snapshot.active_pane = Some(pane);
            }
            snapshot.windows.insert(id, window);
        }
    }
    snapshot
}

#[cfg(test)]
mod output_tests {
    use super::{
        InputParser, PENDING_PANE_OUTPUT_MAX_BYTES, VirtualPane, buffer_unknown_output,
        output_precedes_capture,
    };
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    fn pane() -> VirtualPane {
        VirtualPane {
            session_id: "test-pane".to_string(),
            input_parser: InputParser::default(),
            pending_output: VecDeque::new(),
            pending_bytes: 0,
            seeded: false,
            capture_at: None,
            seeding: false,
            reseed: false,
            pending_seed: None,
            desired: None,
            pending_dims: None,
            dims: None,
            seeded_desired: None,
            resize_attempts: None,
            cwd: None,
            routed_bytes: 0,
        }
    }

    #[test]
    fn capture_replays_continuous_plain_output_and_new_extended_output() {
        let mut pane = pane();
        let capture_at = Instant::now();
        pane.capture_at = Some(capture_at);
        pane.pending_output.push_back((
            capture_at.checked_sub(Duration::from_millis(1)),
            b"already captured".to_vec(),
        ));
        pane.pending_output
            .push_back((Some(capture_at), b"also captured".to_vec()));
        let mut expected = Vec::new();
        // Output keeps arriving between capture-pane and its cursor reply.
        // Split UTF-8 across notifications to also check stream continuity.
        for i in 0..1000 {
            let line = format!("row-{i}: 猫\r\n");
            expected.extend_from_slice(line.as_bytes());
            let split = line.find('猫').unwrap() + 1;
            pane.pending_output
                .push_back((None, line.as_bytes()[..split].to_vec()));
            pane.pending_output
                .push_back((None, line.as_bytes()[split..].to_vec()));
        }
        pane.pending_output.push_back((
            Some(capture_at + Duration::from_millis(1)),
            b"after capture\r\n".to_vec(),
        ));
        expected.extend_from_slice(b"after capture\r\n");
        pane.pending_bytes = pane.pending_output.iter().map(|(_, data)| data.len()).sum();

        pane.seeded = true;
        let replayed: Vec<u8> = pane.take_pending_output().concat();

        assert_eq!(replayed, expected);
        assert!(pane.seeded);
        assert!(pane.pending_output.is_empty());
        assert_eq!(pane.pending_bytes, 0);
    }

    #[test]
    fn pending_replay_preserves_failed_seed_retry() {
        let mut pane = pane();
        pane.pending_output.push_back((None, b"live".to_vec()));
        pane.pending_bytes = 4;

        let replayed: Vec<u8> = pane.take_pending_output().concat();

        assert_eq!(replayed, b"live");
        assert!(
            !pane.seeded,
            "failed captures must remain eligible for retry"
        );
        assert_eq!(pane.pending_bytes, 0);
    }

    #[test]
    fn output_without_a_timestamp_or_snapshot_is_never_discarded() {
        let at = Instant::now();
        assert!(!output_precedes_capture(None, Some(at)));
        assert!(!output_precedes_capture(Some(at), None));
        assert!(!output_precedes_capture(None, None));
    }

    #[test]
    fn unknown_output_caps_oversized_chunks_in_empty_and_populated_queues() {
        let cap = PENDING_PANE_OUTPUT_MAX_BYTES;
        let at = Some(Instant::now());
        for prefilled in [false, true] {
            let mut pending = VecDeque::new();
            let mut total = 0;
            if prefilled {
                buffer_unknown_output(&mut pending, &mut total, None, b"old".to_vec());
            }
            let mut data = vec![b'x'; cap + 17];
            data[..17].fill(b'd');
            data[cap..].fill(b'z');
            let expected = data[17..].to_vec();
            buffer_unknown_output(&mut pending, &mut total, at, data);
            assert_eq!(total, cap);
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0], (at, expected));
            assert!(pending[0].1.capacity() <= cap);
        }
    }

    #[test]
    fn unknown_output_evicts_oldest_chunks_and_keeps_exact_limit() {
        let cap = PENDING_PANE_OUTPUT_MAX_BYTES;
        let mut pending = VecDeque::new();
        let mut total = 0;
        buffer_unknown_output(&mut pending, &mut total, None, vec![b'a'; cap / 2]);
        buffer_unknown_output(&mut pending, &mut total, None, vec![b'b'; cap / 2]);
        assert_eq!(total, cap);
        assert_eq!(pending.len(), 2);
        buffer_unknown_output(&mut pending, &mut total, None, b"tail".to_vec());
        assert_eq!(total, cap / 2 + 4);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].1, vec![b'b'; cap / 2]);
        assert_eq!(pending[1].1, b"tail");
        assert_eq!(
            total,
            pending.iter().map(|(_, data)| data.len()).sum::<usize>()
        );
    }
}

#[cfg(test)]
mod window_tests {
    use super::{compose_seed_payload, parse_window_snapshot};

    fn row(id: u32, index: u32, active: bool, name: &str) -> String {
        format!(
            "@{id} {index} {} b25f,80x24,0,0,{id} b25f,80x24,0,0,{id}  %{id} {name}",
            u8::from(active)
        )
    }

    #[test]
    fn snapshots_follow_swap_move_close_and_remote_active_pane() {
        let first = parse_window_snapshot(vec![
            row(1, 0, true, "shell"),
            row(2, 1, false, "server logs"),
        ]);
        assert_eq!(first.active_window.as_deref(), Some("@1"));
        assert_eq!(first.active_pane.as_deref(), Some("%1"));
        assert_eq!(first.windows["@2"].name, "server logs");
        let swapped = parse_window_snapshot(vec![
            row(1, 1, false, "shell"),
            row(2, 0, true, "server logs"),
        ]);
        assert_eq!(swapped.windows["@1"].index, 1);
        assert_eq!(swapped.windows["@2"].index, 0);
        assert_eq!(swapped.active_pane.as_deref(), Some("%2"));
        let moved_and_closed = parse_window_snapshot(vec![row(2, 9, true, "server logs")]);
        assert_eq!(moved_and_closed.windows.len(), 1);
        assert!(!moved_and_closed.windows.contains_key("@1"));
        assert_eq!(moved_and_closed.windows["@2"].index, 9);
        let empty = parse_window_snapshot(Vec::new());
        assert!(empty.windows.is_empty());
        assert!(empty.active_window.is_none());
    }

    #[test]
    fn seed_payload_trims_trailing_blank_screen_rows_so_hidden_xterm_does_not_scroll_prompt() {
        // 29-row tmux capture: row 0 empty, row 1 has the prompt, rows 2..28 empty.
        let mut lines = vec![String::new(), "root@dev ~ » ".to_string()];
        lines.resize(29, String::new());
        let payload = compose_seed_payload(&lines, 13, 1, 0, Some(29));
        assert_eq!(payload, "\x1b[H\x1b[2J\x1b[3J\r\nroot@dev ~ » \x1b[2;14H");
    }

    #[test]
    fn seed_payload_scrolls_history_by_at_most_screen_rows() {
        let mut lines: Vec<String> = (0..50).map(|i| format!("hist-{i}")).collect();
        lines.push("prompt$ ".to_string());
        lines.resize(50 + 24, String::new());
        let payload = compose_seed_payload(&lines, 8, 0, 50, Some(24));
        assert!(payload.contains(&format!(
            "\x1b[r\x1b[9999B{}\x1b[Hprompt$ \x1b[1;9H",
            "\n".repeat(24)
        )));
    }
}
