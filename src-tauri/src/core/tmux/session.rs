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
//! - Pane input is sent back as `send-keys -t %<id> -H <hex>`.
//! - Layout notifications rebuild the pane tree which is emitted to the
//!   frontend via the `tmux-session-state` event.
//! - `%exit` (after `detach` or a remote kill) hands the channel back to the
//!   regular SSH loop, which continues as a normal shell.

use super::control::{ControlMessage, ControlParser, LayoutCell, TmuxUiNode, parse_line};
use crate::core::capture::OutputCaptureProcessor;
use crate::core::terminal_session::TerminalOutputDecoder;
use crate::core::{
    DynamicTitleCapabilities, SessionCommand, SessionCommandReceiver, SessionHandle, SessionInfo,
    SessionManager, SessionOutputCoalescer, SessionType, SharedCwd, now_session_started_at,
    session_command_channel, update_cwd_if_changed,
};
use russh::{ChannelMsg, client};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::pin::Pin;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Sleep};

/// Event emitted (globally) whenever the tmux projection changes.
pub(crate) const TMUX_STATE_EVENT: &str = "tmux-session-state";

/// Input is batched over this interval so `send-keys` stays cheap even for
/// IME composition or paste bursts.
const INPUT_FLUSH_DELAY_MS: u64 = 4;
/// Max payload bytes per `send-keys` command line.
const SEND_KEYS_CHUNK_BYTES: usize = 1024;
/// Debounce for structural refreshes (`list-windows`/`list-panes`).
const STATE_REFRESH_DELAY_MS: u64 = 120;
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
pub(crate) enum ControlExit {
    /// tmux detached/exited: the channel is back at a normal shell; the
    /// caller should resume regular I/O and forward `leftover` bytes first.
    ReturnToShell { leftover: Vec<u8> },
    /// The channel or the whole session was closed.
    Closed,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TmuxWindowInfo {
    pub id: String,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TmuxStatePayload {
    pub control_session_id: String,
    pub exited: bool,
    pub windows: Vec<TmuxWindowInfo>,
    pub active_window_id: Option<String>,
    pub active_pane_session_id: Option<String>,
    pub tree: Option<TmuxUiNode>,
}

/// One tmux pane projected as a virtual terminal session.
struct VirtualPane {
    /// Virtual session id registered in the [`SessionManager`].
    session_id: String,
    output: Arc<SessionOutputCoalescer>,
    decoder: TerminalOutputDecoder,
    /// Forwarder task draining this pane's command channel.
    forwarder: JoinHandle<()>,
    /// Output received before `capture-pane` seeding completed. The instant
    /// is the server-side production time estimated from `%extended-output`'s
    /// age (`None` for plain `%output`, which carries no timestamp).
    pending_output: VecDeque<(Option<std::time::Instant>, Vec<u8>)>,
    seeded: bool,
    /// When this pane's latest `capture-pane` snapshot was taken; output
    /// provably produced before it is already inside the seeded screen and
    /// must not be replayed (tmux defers notifications past `%end` blocks).
    capture_at: Option<std::time::Instant>,
    /// Last renderer attach; a re-attach to a seeded pane means the terminal
    /// widget was remounted (empty buffer) and needs the screen re-seeded.
    last_attach: Option<std::time::Instant>,
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
    /// Last `resize-pane` target and attempts for the current `desired`
    /// size — bounds the layout-echo feedback when a target is unreachable
    /// (e.g. a remote `window-size=manual` that ignores the client size).
    resize_attempts: Option<((u32, u32), u8)>,
    /// AI capture: intercepts `__DF_CMD_*` markers in this pane's decoded
    /// output, exactly like the SSH loop's processor — `%output` is the
    /// pane's pty byte stream, so the same marker contract applies.
    capture: OutputCaptureProcessor,
    /// Pane working directory, shared with the `SessionHandle` so
    /// `get_terminal_cwd` and the file explorer see it. Populated by
    /// `#{pane_current_path}` queries — tmux tracks it per pane.
    cwd: SharedCwd,
    /// Bytes routed so far; only logged while small.
    routed_bytes: usize,
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
    /// Full layout — used for the reconcile pane set (zoomed-away panes stay
    /// mapped so they don't lose their terminal state).
    layout: Option<LayoutCell>,
    /// Layout to render: collapsed to the zoomed pane while `Z` is set.
    visible_layout: Option<LayoutCell>,
    /// `Z` in window flags. Resizing a zoomed window would drop the zoom.
    zoomed: bool,
}

pub(crate) struct ControlSession<'a> {
    app: &'a AppHandle,
    control_session_id: &'a str,
    manager: &'a Arc<SessionManager>,
    encoding: String,
    connection_id: Option<String>,

    parser: ControlParser,
    /// Lines collected inside a `%begin`..`%end` block.
    block_lines: Option<Vec<String>>,
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

    pane_cmd_tx: mpsc::UnboundedSender<(String, SessionCommand)>,
    pane_cmd_rx: mpsc::UnboundedReceiver<(String, SessionCommand)>,

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
    unknown_output: HashMap<String, VecDeque<(Option<std::time::Instant>, Vec<u8>)>>,

    exit_mode: Option<ExitMode>,
    exit_seen: bool,
    detach_deadline: Option<Pin<Box<Sleep>>>,
}

impl<'a> ControlSession<'a> {
    fn new(
        app: &'a AppHandle,
        control_session_id: &'a str,
        manager: &'a Arc<SessionManager>,
        encoding: &str,
        connection_id: Option<String>,
    ) -> Self {
        let (pane_cmd_tx, pane_cmd_rx) = mpsc::unbounded_channel();
        Self {
            app,
            control_session_id,
            manager,
            encoding: encoding.to_string(),
            connection_id,
            parser: ControlParser::new(),
            block_lines: None,
            block_started_at: None,
            pending_responses: VecDeque::new(),
            windows: HashMap::new(),
            panes: HashMap::new(),
            pane_names: HashMap::new(),
            pane_windows: HashMap::new(),
            active_window: None,
            active_pane: None,
            pane_cmd_tx,
            pane_cmd_rx,
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
        channel: &mut russh::Channel<client::Msg>,
        kind: PendingKind,
        line: &str,
    ) -> bool {
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        if channel.data(data.as_slice()).await.is_ok() {
            self.pending_responses.push_back(kind);
            true
        } else {
            false
        }
    }

    /// Queue an internal query whose block output should be parsed.
    async fn send_query(
        &mut self,
        channel: &mut russh::Channel<client::Msg>,
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

    async fn refresh_lists(&mut self, channel: &mut russh::Channel<client::Msg>) {
        // `window_name` goes last: names may contain tabs/spaces.
        self.send_query(
            channel,
            PendingKind::WindowsRefresh,
            // Space-separated like iTerm2's gateway: tmux renders control
            // characters in -F output as `_`, so tabs cannot be used as
            // separators. The name field goes last since it may contain
            // spaces; splitn(4, ' ') preserves it.
            "list-windows -F '#{window_id} #{window_active} #{window_layout} #{window_visible_layout} #{window_flags} #{window_name}'",
        )
        .await;
        self.send_query(
            channel,
            PendingKind::PanesRefresh,
            "list-panes -s -F '#{pane_id} #{window_id} #{pane_current_command}'",
        )
        .await;
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
        let (cmd_tx, mut cmd_rx) = session_command_channel(session_id.clone());
        let cwd = SharedCwd::default();
        let output = SessionOutputCoalescer::for_app(
            self.app.clone(),
            format!("terminal-output-{session_id}"),
            cmd_tx.clone(),
        );

        let (parent, parent_remote_fs, parent_ssh_handle) = {
            let sessions = self.manager.sessions.lock().await;
            let handle = sessions.get(self.control_session_id);
            (
                handle.map(|h| h.info.clone()),
                handle.and_then(|h| h.remote_fs.clone()),
                handle.and_then(|h| h.ssh_handle.clone()),
            )
        };
        let name = self
            .pane_names
            .get(pane_id)
            .cloned()
            .unwrap_or_else(|| format!("tmux {pane_id}"));
        let info = SessionInfo {
            id: session_id.clone(),
            name,
            session_type: SessionType::SSH,
            started_at: now_session_started_at(),
            connection_id: self.connection_id.clone(),
            connected: true,
            owner_window_label: parent
                .as_ref()
                .and_then(|info| info.owner_window_label.clone()),
            ai_execution_profile: parent
                .as_ref()
                .map(|info| info.ai_execution_profile)
                .unwrap_or_default(),
            injection_active: false,
            // The pane's cwd comes from tmux's own pane_current_path poll,
            // not from shell-integration hooks in the pane's shell.
            cwd_tracking_active: true,
            dynamic_title_capabilities: DynamicTitleCapabilities::default(),
            // The pane shares the control session's SFTP channel: it lives on
            // the same SSH connection and stays up while the shell channel is
            // in control mode.
            remote_file_browser_enabled: parent
                .as_ref()
                .is_some_and(|info| info.remote_file_browser_enabled),
            remote_stats_enabled: parent
                .as_ref()
                .is_some_and(|info| info.remote_stats_enabled),
            ssh_profile: parent.as_ref().and_then(|info| info.ssh_profile.clone()),
            ssh_runtime_mode: None,
        };
        self.manager
            .add_session(SessionHandle {
                info,
                cmd_tx,
                startup_input_barrier: None,
                ssh_config: None,
                // Exec channels (stats, probes) multiplex over the parent's
                // authenticated connection — the pane owns no SSH transport.
                ssh_handle: parent_ssh_handle,
                cwd: cwd.clone(),
                remote_fs: parent_remote_fs,
            })
            .await;

        let pane_key = pane_id.to_string();
        let forward_tx = self.pane_cmd_tx.clone();
        let forward_pane = pane_key.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                if forward_tx.send((forward_pane.clone(), cmd)).is_err() {
                    break;
                }
            }
        });

        // Output that arrived before the pane was mapped is replayed after
        // the capture-pane seed, keeping chronology.
        let mut pending_output = VecDeque::new();
        if let Some(mut buffered) = self.unknown_output.remove(&pane_key) {
            pending_output.append(&mut buffered);
        }

        self.panes.insert(
            pane_key.clone(),
            VirtualPane {
                session_id: session_id.clone(),
                output,
                decoder: TerminalOutputDecoder::new(&self.encoding),
                forwarder,
                pending_output,
                seeded: false,
                capture_at: None,
                last_attach: None,
                seeding: false,
                reseed: false,
                pending_seed: None,
                desired: None,
                pending_dims: None,
                dims: None,
                resize_attempts: None,
                capture: OutputCaptureProcessor::new(),
                cwd,
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

    async fn seed_pane(&mut self, channel: &mut russh::Channel<client::Msg>, pane_id: &str) {
        let dims = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
            .and_then(|window| window.layout.as_ref())
            .and_then(|layout| {
                pane_id
                    .trim_start_matches('%')
                    .parse::<u32>()
                    .ok()
                    .and_then(|num| layout.find_pane(num))
            })
            .map(|cell| (cell.width, cell.height));
        if let Some(pane) = self.panes.get_mut(pane_id) {
            if pane.seeding {
                return;
            }
            pane.seeding = true;
            pane.dims = dims;
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
        pane.output.close();
        pane.forwarder.abort();
        self.manager.remove_session(&pane.session_id).await;
        let _ = self
            .app
            .emit(&format!("session-closed-{}", pane.session_id), ());
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
        let tree = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
            .and_then(|window| window.visible_layout.as_ref().or(window.layout.as_ref()))
            .and_then(|layout| self.build_ui_tree(layout));

        let windows = {
            let mut list: Vec<TmuxWindowInfo> = self
                .windows
                .iter()
                .map(|(id, window)| TmuxWindowInfo {
                    id: id.clone(),
                    name: window.name.clone(),
                    active: Some(id) == self.active_window.as_ref(),
                })
                .collect();
            list.sort_by(|a, b| a.id.cmp(&b.id));
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
            tree,
        };
        if self.last_emitted.as_ref() == Some(&payload) {
            return;
        }
        tracing::info!(
            session_id = %self.control_session_id,
            windows = payload.windows.len(),
            active_window = ?payload.active_window_id,
            has_tree = payload.tree.is_some(),
            "tmux emit state"
        );
        self.last_emitted = Some(payload.clone());
        let _ = self.app.emit(TMUX_STATE_EVENT, &payload);
    }

    fn emit_exited(&self) {
        let payload = TmuxStatePayload {
            control_session_id: self.control_session_id.to_string(),
            exited: true,
            windows: Vec::new(),
            active_window_id: None,
            active_pane_session_id: None,
            tree: None,
        };
        let _ = self.app.emit(TMUX_STATE_EVENT, &payload);
    }

    /// Reconcile virtual panes with the active window's layout.
    async fn reconcile_panes(&mut self, channel: &mut russh::Channel<client::Msg>) {
        let (layout, zoomed) = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
            .map(|window| (window.layout.clone(), window.zoomed))
            .unwrap_or((None, false));
        let wanted: HashSet<String> = layout
            .as_ref()
            .map(|layout| {
                layout
                    .pane_ids()
                    .iter()
                    .map(|id| format!("%{id}"))
                    .collect()
            })
            .unwrap_or_default();

        // The client must cover the window before any capture below runs:
        // while the window outgrows the client, tmux clips panes and marks
        // clipped edges with `#`, and it never re-pushes `%output` after a
        // resize — a capture taken pre-grow would seed those marks forever.
        // Skipped while zoomed since any client resize drops the zoom.
        let mut grew = false;
        if !zoomed {
            if let Some(layout) = layout.as_ref() {
                grew = self.grow_client(channel, layout.width, layout.height).await;
            }
        }

        // Drop panes that vanished or belong to a different window.
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
            for pane_id in &wanted {
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    if pane.seeding {
                        pane.reseed = true;
                    } else {
                        pane.seeded = false;
                    }
                }
            }
        }
        // A pane whose cell resized since its seed renders rows that were
        // laid out for the old width; capture again at the new size.
        for pane_id in &wanted {
            let Some(dims) = layout
                .as_ref()
                .and_then(|layout| {
                    pane_id
                        .trim_start_matches('%')
                        .parse::<u32>()
                        .ok()
                        .and_then(|num| layout.find_pane(num))
                })
                .map(|cell| (cell.width, cell.height))
            else {
                continue;
            };
            if let Some(pane) = self.panes.get_mut(pane_id) {
                if pane.dims.is_some() && pane.dims != Some(dims) {
                    if pane.seeding {
                        pane.reseed = true;
                    } else {
                        pane.seeded = false;
                    }
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

    /// Grow the control client to fit the window layout. Shrinking would
    /// make tmux rebalance panes and echo `%layout-change`, so we only ever
    /// grow. Returns true when a `refresh-client -C` was actually sent.
    async fn grow_client(
        &mut self,
        channel: &mut russh::Channel<client::Msg>,
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
    async fn sync_pane_sizes(&mut self, channel: &mut russh::Channel<client::Msg>) {
        let Some(window) = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
        else {
            return;
        };
        // Resizing a zoomed window would drop the zoom.
        if window.zoomed {
            return;
        }
        let Some(layout) = window.layout.clone() else {
            return;
        };

        let needed = needed_window_size(&layout, &self.panes);
        if needed != (layout.width, layout.height) {
            // Unlike grow_client this also shrinks: renderers may be
            // narrower than the window's current share. `refresh-client`
            // only emits `%layout-change` when the resize actually takes
            // effect, so this self-limits against remotes ignoring it.
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

        // The window already matches the renderers' demand; fix whatever
        // drift remains per pane. Targets are collected first because
        // send_line borrows self mutably.
        let mut targets = Vec::new();
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
        for (pane_id, (cols, rows)) in targets {
            self.send_line(
                channel,
                PendingKind::Ignore,
                &format!("resize-pane -t '{pane_id}' -x {cols} -y {rows}"),
            )
            .await;
        }
    }

    async fn handle_message(&mut self, channel: &mut russh::Channel<client::Msg>, line: String) {
        // Temporary visibility while bring-up is being debugged.
        tracing::info!(
            session_id = %self.control_session_id,
            line = %line.chars().take(200).collect::<String>(),
            "tmux control: line"
        );
        // Command-response blocks capture all lines between begin/end.
        if let Some(lines) = self.block_lines.as_mut() {
            let message = parse_line(&line);
            match message {
                Some(ControlMessage::End | ControlMessage::Error) => {
                    let collected = std::mem::take(lines);
                    let failed = matches!(message, Some(ControlMessage::Error));
                    self.block_lines = None;
                    let kind = self
                        .pending_responses
                        .pop_front()
                        .unwrap_or(PendingKind::Ignore);
                    self.handle_block(channel, kind, collected, failed).await;
                }
                Some(ControlMessage::Begin) => {
                    tracing::warn!("tmux control: nested %begin; discarding outer block");
                    self.block_lines = None;
                    self.pending_responses.pop_front();
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
            tracing::info!(
                session_id = %self.control_session_id,
                line = %line.chars().take(120).collect::<String>(),
                "tmux control: unparsed line"
            );
            return;
        };
        tracing::debug!(
            session_id = %self.control_session_id,
            message = ?std::mem::discriminant(&message),
            "tmux control: message"
        );
        match message {
            ControlMessage::Begin => {
                self.block_lines = Some(Vec::new());
                self.block_started_at = Some(std::time::Instant::now());
            }
            ControlMessage::End | ControlMessage::Error => {
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
                if self.active_window.as_deref() == Some(window.as_str()) {
                    // reconcile_panes also grows the client when the layout
                    // outgrows it — that is what removes `#` clipping marks.
                    self.reconcile_panes(channel).await;
                }
            }
            ControlMessage::WindowAdd { window } => {
                self.windows.entry(window).or_insert(WindowState {
                    name: String::new(),
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
                tracing::info!(
                    session_id = %self.control_session_id,
                    client = %client,
                    "tmux client detached notification"
                );
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
            ControlMessage::ConfigError { text } | ControlMessage::Message { text } => {
                tracing::info!(
                    session_id = %self.control_session_id,
                    message = %text,
                    "tmux control server message"
                );
            }
            ControlMessage::Exit { .. } => {
                self.mark_exit();
            }
        }
    }

    fn mark_exit(&mut self) {
        self.exit_seen = true;
    }

    fn route_output(&mut self, pane: &str, produced_at: Option<std::time::Instant>, data: Vec<u8>) {
        if let Some(virtual_pane) = self.panes.get_mut(pane) {
            // Not-yet-seeded panes keep raw bytes: decoding is stateful and
            // must see each byte exactly once.
            if virtual_pane.seeded {
                // A notification provably produced before the last capture is
                // already painted by the seed — tmux defers such blocks past
                // the command's `%end`, so arrival order alone cannot tell.
                if let (Some(at), Some(capture_at)) = (produced_at, virtual_pane.capture_at) {
                    if at <= capture_at {
                        return;
                    }
                }
                let decoded = virtual_pane.decoder.decode(&data);
                let text = if virtual_pane.capture.has_active() {
                    virtual_pane.capture.process(&decoded)
                } else {
                    decoded
                };
                virtual_pane.output.push_owned(text);
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
                virtual_pane.pending_output.push_back((produced_at, data));
            }
        } else {
            // Pane output can arrive before its layout (attach, or panes in
            // background windows). Buffer a bounded amount keyed by pane id.
            let pending = self.unknown_output.entry(pane.to_string()).or_default();
            let total: usize = pending.iter().map(|(_, data)| data.len()).sum();
            if total < PENDING_PANE_OUTPUT_MAX_BYTES {
                pending.push_back((produced_at, data));
            }
        }
    }

    async fn handle_block(
        &mut self,
        channel: &mut russh::Channel<client::Msg>,
        kind: PendingKind,
        lines: Vec<String>,
        failed: bool,
    ) {
        match kind {
            PendingKind::WindowsRefresh => {
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
                if let Some(virtual_pane) = self.panes.get_mut(&pane) {
                    if !failed {
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
                        if let Some(seed_lines) = virtual_pane.pending_seed.take() {
                            let (x, y, history) = cursor.unwrap_or_default();
                            let hist = history.min(seed_lines.len());
                            let mut decoder = TerminalOutputDecoder::new(&self.encoding);
                            // Reset + clear first: this makes the seed
                            // idempotent for remounted terminals and for
                            // re-captures after a client grow (which must
                            // overwrite '#' clipping marks already on
                            // screen).
                            let mut text = String::from("\x1b[H\x1b[2J\x1b[3J");
                            if hist > 0 {
                                // `capture-pane -S -` leads with
                                // `history_size` scrollback rows. Writing
                                // them plainly would leave stale rows (e.g.
                                // leftover '#'/'%' marks) parked above the
                                // prompt, so scroll them into the renderer's
                                // own scrollback first, then paint only the
                                // screen part.
                                text.push_str(
                                    &decoder.decode(seed_lines[..hist].join("\r\n").as_bytes()),
                                );
                                text.push_str("\r\n");
                                text.push_str("\x1b[r\x1b[9999B");
                                text.push_str(&"\n".repeat(hist));
                                text.push_str("\x1b[H");
                            }
                            text.push_str(
                                &decoder.decode(seed_lines[hist..].join("\r\n").as_bytes()),
                            );
                            // `cursor_y` is screen-relative; with scrollback
                            // scrolled away it lands directly on that row.
                            text.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
                            virtual_pane.output.push_owned(text);
                        }
                    }
                    if !reseed {
                        virtual_pane.seeded = true;
                        let capture_at = virtual_pane.capture_at;
                        while let Some((produced_at, data)) =
                            virtual_pane.pending_output.pop_front()
                        {
                            // Entries provably produced before the snapshot —
                            // and un-`aged` `%output`, which is always older —
                            // are already painted; replaying them would draw
                            // the same lines twice.
                            let stale = match (produced_at, capture_at) {
                                (Some(at), Some(cap)) => at <= cap,
                                (None, Some(_)) => true,
                                _ => false,
                            };
                            if stale {
                                continue;
                            }
                            let decoded = virtual_pane.decoder.decode(&data);
                            let text = if virtual_pane.capture.has_active() {
                                virtual_pane.capture.process(&decoded)
                            } else {
                                decoded
                            };
                            virtual_pane.output.push_owned(text);
                        }
                    }
                }
                if reseed {
                    self.seed_pane(channel, &pane).await;
                }
            }
            PendingKind::PaneCwd { pane } => {
                if !failed {
                    if let (Some(pane), Some(path)) = (self.panes.get(&pane), lines.first()) {
                        let path = path.trim();
                        if let Some(next) = update_cwd_if_changed(&pane.cwd, path).await {
                            let event = format!("cwd-changed-{}", pane.session_id);
                            let _ = self.app.emit(&event, &next);
                        }
                    }
                }
            }
            PendingKind::UserCommand => {
                if failed && !lines.is_empty() {
                    let _ = self.app.emit(
                        "tmux-command-error",
                        serde_json::json!({
                            "controlSessionId": self.control_session_id,
                            "message": lines.join("\n"),
                        }),
                    );
                }
            }
            PendingKind::Ignore => {}
        }
    }

    fn apply_window_list(&mut self, lines: Vec<String>) {
        for line in lines {
            let mut parts = line.splitn(6, ' ');
            let (Some(id), Some(active), Some(layout_str)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let visible_str = parts.next().unwrap_or_default();
            let flags = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default();
            let entry = self.windows.entry(id.to_string()).or_insert(WindowState {
                name: String::new(),
                layout: None,
                visible_layout: None,
                zoomed: false,
            });
            entry.name = name.to_string();
            if let Some((layout, _)) = super::control::parse_layout(layout_str) {
                entry.visible_layout = Some(
                    super::control::parse_layout(visible_str)
                        .map(|(cell, _)| cell)
                        .unwrap_or_else(|| layout.clone()),
                );
                entry.layout = Some(layout);
            }
            entry.zoomed = flags.contains('Z');
            if active == "1" {
                self.active_window = Some(id.to_string());
            }
        }
        self.emit_state();
    }

    async fn flush_input(&mut self, channel: &mut russh::Channel<client::Msg>) {
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
            for chunk in data.chunks(SEND_KEYS_CHUNK_BYTES) {
                let mut line = String::from("send-keys -H -t ");
                line.push_str(&pane_id);
                for byte in chunk {
                    let _ = write!(line, " {byte:02x}");
                }
                self.send_line(channel, PendingKind::Ignore, &line).await;
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

    async fn flush_cwd_queries(&mut self, channel: &mut russh::Channel<client::Msg>) {
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

    async fn continue_paused(&mut self, channel: &mut russh::Channel<client::Msg>) {
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

    async fn handle_pane_command(
        &mut self,
        channel: &mut russh::Channel<client::Msg>,
        pane_id: String,
        command: SessionCommand,
    ) {
        match command {
            SessionCommand::Write { data, .. } => {
                if !data.is_empty() && self.seen_input.insert(pane_id.clone()) {
                    tracing::info!(
                        session_id = %self.control_session_id,
                        pane = %pane_id,
                        bytes = data.len(),
                        "tmux pane input received"
                    );
                }
                self.input_buf
                    .entry(pane_id)
                    .or_default()
                    .extend_from_slice(&data);
                if self.input_flush.is_none() {
                    self.input_flush = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                        INPUT_FLUSH_DELAY_MS,
                    ))));
                }
            }
            SessionCommand::Resize { cols, rows } => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    if pane.pending_dims != Some((cols, rows)) {
                        pane.pending_dims = Some((cols, rows));
                        self.resize_deadline = Some(Box::pin(tokio::time::sleep(
                            Duration::from_millis(RESIZE_SYNC_DEBOUNCE_MS),
                        )));
                    }
                }
            }
            SessionCommand::SerialModemUpload { .. } => {
                // Serial-only command; unreachable on an SSH control channel.
            }
            SessionCommand::AttachConfirmed { ack } => {
                tracing::info!(
                    session_id = %self.control_session_id,
                    pane = %pane_id,
                    known = self.panes.contains_key(&pane_id),
                    "tmux pane attach"
                );
                let mut reseed = false;
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    pane.output.attach_confirmed(ack);
                    // A renderer attaching to an already-seeded pane was
                    // remounted (fresh buffer); resend the screen. Attaches
                    // within 250 ms are the same mount seen twice (e.g.
                    // StrictMode), not a remount.
                    reseed = pane.seeded
                        && pane
                            .last_attach
                            .is_none_or(|t| t.elapsed() > Duration::from_millis(250));
                    pane.last_attach = Some(std::time::Instant::now());
                    if reseed {
                        pane.seeded = false;
                    }
                } else {
                    let _ = ack.send(());
                }
                if reseed {
                    self.seed_pane(channel, &pane_id).await;
                }
            }
            SessionCommand::DetachRenderer => {
                if let Some(pane) = self.panes.get(&pane_id) {
                    pane.output.detach();
                }
            }
            SessionCommand::AckOutput { bytes } => {
                if let Some(pane) = self.panes.get(&pane_id) {
                    pane.output.ack(bytes);
                }
            }
            SessionCommand::Close => {
                // Closing one pane leaf kills the tmux pane; the resulting
                // layout-change rebuilds the tree without it.
                self.send_line(
                    channel,
                    PendingKind::Ignore,
                    &format!("kill-pane -t '{pane_id}'"),
                )
                .await;
                self.drop_pane(&pane_id).await;
                self.emit_state();
            }
            SessionCommand::TmuxCommand { line } => {
                let clean = line.trim_matches(['\r', '\n']).to_string();
                if !clean.is_empty() {
                    self.send_line(channel, PendingKind::UserCommand, &clean)
                        .await;
                }
            }
            SessionCommand::TmuxDetach => {
                self.request_detach(channel, ExitMode::Detach).await;
            }
            SessionCommand::CaptureExec {
                marker_id,
                wrapped_command,
                result_tx,
            } => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    // The marker-wrapped command goes through the pane's pty
                    // like any typed input; its echo and markers return via
                    // `%output` where `capture` picks them apart.
                    pane.capture.register(marker_id, result_tx);
                    self.input_buf
                        .entry(pane_id)
                        .or_default()
                        .extend_from_slice(&wrapped_command);
                    if self.input_flush.is_none() {
                        self.input_flush = Some(Box::pin(tokio::time::sleep(
                            Duration::from_millis(INPUT_FLUSH_DELAY_MS),
                        )));
                    }
                } else {
                    drop(result_tx);
                }
            }
            SessionCommand::CancelCapture { marker_id } => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    pane.capture.cancel(&marker_id);
                }
            }
            SessionCommand::PauseOutput
            | SessionCommand::ResumeOutput
            | SessionCommand::ZmodemAcceptDownload { .. }
            | SessionCommand::ZmodemAcceptUpload { .. }
            | SessionCommand::ZmodemCancel => {}
        }
    }

    async fn request_detach(&mut self, channel: &mut russh::Channel<client::Msg>, mode: ExitMode) {
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
pub(crate) async fn run_control_session(
    app: &AppHandle,
    session_id: &str,
    manager: &Arc<SessionManager>,
    channel: &mut russh::Channel<client::Msg>,
    cmd_rx: &mut SessionCommandReceiver,
    initial: &[u8],
    encoding: &str,
    connection_id: Option<String>,
) -> ControlExit {
    let mut session = ControlSession::new(app, session_id, manager, encoding, connection_id);

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

    // Feed the bytes that arrived together with the first notification.
    for line in session.parser.feed(initial) {
        session.handle_message(channel, line).await;
        if session.exit_seen {
            break;
        }
    }

    let exit = loop {
        if session.exit_seen {
            break match session.exit_mode {
                Some(ExitMode::Close) => ControlExit::Closed,
                _ => ControlExit::ReturnToShell {
                    leftover: session.parser.take_pending(),
                },
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
                    if let Some(dims) = pane.pending_dims.take() {
                        if pane.desired != Some(dims) {
                            pane.desired = Some(dims);
                            pane.resize_attempts = None;
                        }
                    }
                }
                session.sync_pane_sizes(channel).await;
            }

            () = async {
                if let Some(deadline) = session.cwd_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if session.cwd_deadline.is_some() => {
                session.flush_cwd_queries(channel).await;
            }

            command = cmd_rx.recv() => {
                match command {
                    Some(SessionCommand::TmuxCommand { line }) => {
                        let clean = line.trim_matches(['\r', '\n']);
                        if !clean.is_empty() {
                            session
                                .send_line(channel, PendingKind::UserCommand, clean)
                                .await;
                        }
                    }
                    Some(SessionCommand::TmuxDetach) => {
                        session.request_detach(channel, ExitMode::Detach).await;
                    }
                    Some(SessionCommand::Close) => {
                        session.request_detach(channel, ExitMode::Close).await;
                    }
                    Some(SessionCommand::SerialModemUpload { .. }) => {}
                    Some(SessionCommand::AttachConfirmed { ack }) => {
                        let _ = ack.send(());
                    }
                    Some(
                        SessionCommand::DetachRenderer
                        | SessionCommand::Write { .. }
                        | SessionCommand::Resize { .. }
                        | SessionCommand::PauseOutput
                        | SessionCommand::ResumeOutput
                        | SessionCommand::AckOutput { .. }
                        | SessionCommand::CancelCapture { .. }
                        | SessionCommand::ZmodemAcceptDownload { .. }
                        | SessionCommand::ZmodemAcceptUpload { .. }
                        | SessionCommand::ZmodemCancel,
                    ) => {}
                    Some(SessionCommand::CaptureExec { result_tx, .. }) => {
                        drop(result_tx);
                    }
                    None => break ControlExit::Closed,
                }
            }

            command = session.pane_cmd_rx.recv() => {
                if let Some((pane_id, cmd)) = command {
                    session.handle_pane_command(channel, pane_id, cmd).await;
                }
            }

            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        for line in session.parser.feed(&data) {
                            session.handle_message(channel, line).await;
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
