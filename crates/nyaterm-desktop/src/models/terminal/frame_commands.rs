//! Bounded frame command queue and command ordering.

use super::frame_events::TerminalFrameSnapshotPurpose;
use super::frame_search::{TerminalFrameSearchKey, TerminalFrameSearchPurpose};
use super::frame_worker::TerminalFrameSession;
use super::{TerminalSelection, TerminalSelectionTextError};
use crate::models::recording::ExecutionRecordingEvent;
use futures::channel::oneshot;
use nyaterm_core::ActionLinksMatcherSettings;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Copy)]
pub(super) struct TerminalFrameScrollTarget {
    pub(super) offset: usize,
    pub(super) history: Option<nyaterm_terminal::TerminalHistoryState>,
}

#[derive(Debug)]
pub(super) enum TerminalFrameCommand {
    SetCollapsedCommands {
        session_id: String,
        ids: std::collections::BTreeSet<nyaterm_terminal::command_blocks::CommandBlockId>,
    },
    SetLogicalLineSnapshots {
        enabled: bool,
    },
    ExecutionRecording {
        session_id: String,
        event: ExecutionRecordingEvent,
    },
    DecodedOutput {
        session_id: String,
        text: String,
    },
    SyntheticOutput {
        session_id: String,
        text: String,
    },

    NavigateCommand {
        session_id: String,
        action: nyaterm_terminal::command_navigation::CommandNavigationAction,
        offset: usize,
        source_history: nyaterm_terminal::TerminalHistoryState,
    },
    CommandInput {
        session_id: String,
        fallback: Option<nyaterm_terminal::TerminalLineId>,
    },
    EnsureSession {
        session_id: String,
        encoding: String,
        scrollback_limit: usize,
    },
    SeedSession {
        session_id: String,
        output: String,
        encoding: String,
        scrollback_limit: usize,
    },
    RemoveSession {
        session_id: String,
    },
    AppendLocalText {
        session_id: String,
        text: String,
    },
    SystemRecording {
        session_id: String,
        text: String,
    },
    RekeySession {
        old_id: String,
        new_id: String,
        encoding: String,
        scrollback_limit: usize,
    },
    ClearExceptInput {
        session_id: String,
    },
    TakeSession {
        session_id: String,
        response_tx: std::sync::mpsc::SyncSender<Option<TerminalFrameSession>>,
    },
    InsertSessions {
        sessions: Vec<(String, TerminalFrameSession)>,
    },
    ResizeSession {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    Output {
        session_id: String,
        data: Vec<u8>,
        encoding: String,
        scrollback_limit: usize,
    },
    RequestSnapshot {
        session_id: String,
        offset: usize,
        source_history: Option<nyaterm_terminal::TerminalHistoryState>,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
        priority: bool,
        purpose: TerminalFrameSnapshotPurpose,
    },
    RequestAllText {
        session_id: String,
        response_tx: oneshot::Sender<Option<String>>,
    },
    RequestSelectionText {
        session_id: String,
        request: Box<TerminalSelectionTextRequest>,
        response_tx:
            oneshot::Sender<Result<TerminalSelectionTextResult, TerminalSelectionTextError>>,
    },
    RequestSearch {
        session_id: String,
        purpose: TerminalFrameSearchPurpose,
        key: TerminalFrameSearchKey,
    },
    CancelFindSearch {
        session_id: String,
    },
    /// Prefer building full viewport snapshots for these sessions (visible tabs).
    /// Empty list means no session is paint-priority (all background).
    SetSnapshotPriority {
        session_ids: Vec<String>,
    },
    Fence {
        complete_tx: std::sync::mpsc::SyncSender<()>,
    },
}

pub(crate) struct TerminalSelectionTextResult {
    pub(crate) text: Option<String>,
    pub(crate) selection: TerminalSelection,
    pub(crate) history: nyaterm_terminal::TerminalHistoryState,
}

#[derive(Debug)]
pub(super) struct TerminalSelectionTextRequest {
    pub(super) selection: TerminalSelection,
    pub(super) history: nyaterm_terminal::TerminalHistoryState,
}

#[derive(Debug)]
pub(super) struct TerminalFrameCommandSender {
    pub(super) shared: Arc<TerminalFrameCommandQueueShared>,
}

#[derive(Debug)]
pub(super) struct TerminalFrameCommandReceiver {
    pub(super) shared: Arc<TerminalFrameCommandQueueShared>,
}

#[derive(Debug)]
pub(super) struct TerminalFrameCommandQueueShared {
    pub(super) inner: Mutex<TerminalFrameCommandQueueInner>,
    pub(super) ready: Condvar,
    // Approximate backpressure gauge; command ordering remains protected by `inner`.
    pub(super) queued_output_bytes: AtomicUsize,
}

#[derive(Debug)]
pub(super) struct TerminalFrameCommandQueueInner {
    pub(super) commands: VecDeque<TerminalFrameCommand>,
    pub(super) sender_count: usize,
    pub(super) closed: bool,
}

pub(super) fn terminal_frame_command_channel()
-> (TerminalFrameCommandSender, TerminalFrameCommandReceiver) {
    let shared = Arc::new(TerminalFrameCommandQueueShared {
        inner: Mutex::new(TerminalFrameCommandQueueInner {
            commands: VecDeque::new(),
            sender_count: 1,
            closed: false,
        }),
        ready: Condvar::new(),
        queued_output_bytes: AtomicUsize::new(0),
    });
    (
        TerminalFrameCommandSender {
            shared: shared.clone(),
        },
        TerminalFrameCommandReceiver { shared },
    )
}

impl TerminalFrameCommandSender {
    pub(super) fn send(&self, command: TerminalFrameCommand) -> bool {
        self.send_owned(command).is_ok()
    }

    pub(super) fn send_owned(
        &self,
        command: TerminalFrameCommand,
    ) -> Result<(), TerminalFrameCommand> {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return Err(command);
        };
        if inner.closed {
            return Err(command);
        }
        let output_bytes = terminal_frame_command_output_bytes(&command);
        push_terminal_frame_command(&mut inner.commands, command);
        self.shared
            .queued_output_bytes
            .fetch_add(output_bytes, Ordering::Relaxed);
        self.shared.ready.notify_one();
        Ok(())
    }

    pub(super) fn send_many<I>(&self, commands: I) -> bool
    where
        I: IntoIterator<Item = TerminalFrameCommand>,
    {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return false;
        };
        if inner.closed {
            return false;
        }
        let mut sent = false;
        for command in commands {
            let output_bytes = terminal_frame_command_output_bytes(&command);
            push_terminal_frame_command(&mut inner.commands, command);
            self.shared
                .queued_output_bytes
                .fetch_add(output_bytes, Ordering::Relaxed);
            sent = true;
        }
        if sent {
            self.shared.ready.notify_one();
        }
        sent
    }

    pub(super) fn len(&self) -> usize {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.commands.len())
            .unwrap_or(0)
    }

    pub(super) fn queued_output_bytes(&self) -> usize {
        self.shared.queued_output_bytes.load(Ordering::Relaxed)
    }

    pub(super) fn close(&self) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.closed = true;
        inner.commands.clear();
        self.shared.queued_output_bytes.store(0, Ordering::Relaxed);
        self.shared.ready.notify_all();
    }
}

impl Clone for TerminalFrameCommandSender {
    fn clone(&self) -> Self {
        if let Ok(mut inner) = self.shared.inner.lock() {
            inner.sender_count = inner.sender_count.saturating_add(1);
        }
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for TerminalFrameCommandSender {
    fn drop(&mut self) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.sender_count = inner.sender_count.saturating_sub(1);
        self.shared.ready.notify_all();
    }
}

impl TerminalFrameCommandReceiver {
    pub(super) fn recv(&self) -> Option<TerminalFrameCommand> {
        let mut inner = self.shared.inner.lock().ok()?;
        loop {
            if inner.closed {
                return None;
            }
            if let Some(command) = pop_terminal_frame_command(&self.shared, &mut inner) {
                return Some(command);
            }
            if inner.sender_count == 0 {
                return None;
            }
            inner = self.shared.ready.wait(inner).ok()?;
        }
    }

    pub(super) fn try_recv(&self) -> Option<TerminalFrameCommand> {
        let mut inner = self.shared.inner.lock().ok()?;
        pop_terminal_frame_command(&self.shared, &mut inner)
    }
}

pub(super) fn pop_terminal_frame_command(
    shared: &TerminalFrameCommandQueueShared,
    inner: &mut TerminalFrameCommandQueueInner,
) -> Option<TerminalFrameCommand> {
    let command = inner.commands.pop_front()?;
    shared.queued_output_bytes.fetch_sub(
        terminal_frame_command_output_bytes(&command),
        Ordering::Relaxed,
    );
    Some(command)
}

pub(super) fn push_terminal_frame_command(
    commands: &mut VecDeque<TerminalFrameCommand>,
    command: TerminalFrameCommand,
) {
    match command {
        TerminalFrameCommand::Output {
            session_id,
            data,
            encoding,
            scrollback_limit,
        } => {
            let insert_at = commands
                .iter()
                .rposition(|queued| !terminal_frame_command_is_low_priority_derived(queued))
                .map_or(0, |index| index + 1);
            if let Some(TerminalFrameCommand::Output {
                session_id: queued_session_id,
                data: queued_data,
                encoding: queued_encoding,
                scrollback_limit: queued_scrollback_limit,
            }) = insert_at
                .checked_sub(1)
                .and_then(|index| commands.get_mut(index))
                && queued_session_id == &session_id
                && queued_encoding == &encoding
                && *queued_scrollback_limit == scrollback_limit
                && queued_data.len().saturating_add(data.len()) <= TERMINAL_FRAME_OUTPUT_CHUNK_SIZE
            {
                queued_data.extend(data);
                return;
            }
            commands.insert(
                insert_at,
                TerminalFrameCommand::Output {
                    session_id,
                    data,
                    encoding,
                    scrollback_limit,
                },
            );
        }
        TerminalFrameCommand::ResizeSession {
            session_id,
            cols,
            rows,
        } => {
            if let Some(TerminalFrameCommand::ResizeSession {
                session_id: last_session_id,
                cols: last_cols,
                rows: last_rows,
            }) = commands.back_mut()
                && *last_session_id == session_id
            {
                *last_cols = cols;
                *last_rows = rows;
                return;
            }
            commands.push_back(TerminalFrameCommand::ResizeSession {
                session_id,
                cols,
                rows,
            });
        }
        TerminalFrameCommand::RequestSnapshot {
            session_id,
            offset,
            source_history,
            action_links_enabled,
            action_link_matchers,
            priority: true,
            purpose,
        } => {
            let segment_start = commands
                .iter()
                .rposition(terminal_frame_command_is_fence)
                .map_or(0, |index| index + 1);
            let mut index = segment_start;
            while index < commands.len() {
                let stale = matches!(
                    commands.get(index),
                    Some(TerminalFrameCommand::RequestSnapshot {
                        session_id: queued_session_id,
                        priority: true,
                        ..
                    }) if queued_session_id == &session_id
                );
                if stale {
                    commands.remove(index);
                } else {
                    index += 1;
                }
            }
            let insert_at = commands
                .iter()
                .enumerate()
                .skip(segment_start)
                .find_map(|(index, command)| {
                    terminal_frame_command_priority_snapshot_insert_before(command).then_some(index)
                })
                .unwrap_or(commands.len());
            commands.insert(
                insert_at,
                TerminalFrameCommand::RequestSnapshot {
                    session_id,
                    offset,
                    source_history,
                    action_links_enabled,
                    action_link_matchers,
                    priority: true,
                    purpose,
                },
            );
        }
        other => commands.push_back(other),
    }
    compact_terminal_frame_command_queue(commands, TERMINAL_FRAME_COMMAND_QUEUE_CAP);
}

pub(super) fn compact_terminal_frame_command_queue(
    commands: &mut VecDeque<TerminalFrameCommand>,
    cap: usize,
) {
    compact_stale_terminal_frame_commands(commands);
    while commands.len() > cap {
        let Some(drop_index) = commands
            .iter()
            .position(terminal_frame_command_can_drop_under_pressure)
        else {
            break;
        };
        commands.remove(drop_index);
    }
}

pub(super) fn compact_stale_terminal_frame_commands(commands: &mut VecDeque<TerminalFrameCommand>) {
    if commands.len() <= 1 {
        return;
    }
    let mut seen_snapshots: HashSet<(String, usize)> = HashSet::new();
    let mut seen_priority_snapshots: HashSet<String> = HashSet::new();
    let mut seen_anchored_history_snapshots: HashSet<String> = HashSet::new();
    let mut seen_searches: HashSet<(String, u8)> = HashSet::new();
    let mut kept_snapshot_priority = false;
    let mut compacted = VecDeque::with_capacity(commands.len());

    for command in commands.drain(..).rev() {
        if terminal_frame_command_is_fence(&command) {
            seen_snapshots.clear();
            seen_priority_snapshots.clear();
            seen_anchored_history_snapshots.clear();
            seen_searches.clear();
            kept_snapshot_priority = false;
            compacted.push_front(command);
            continue;
        }
        let keep = match &command {
            TerminalFrameCommand::RequestSnapshot {
                session_id,
                priority: true,
                ..
            } => seen_priority_snapshots.insert(session_id.clone()),
            TerminalFrameCommand::RequestSnapshot {
                session_id,
                offset,
                source_history: Some(_),
                ..
            } if *offset > 0 => seen_anchored_history_snapshots.insert(session_id.clone()),
            TerminalFrameCommand::RequestSnapshot {
                session_id, offset, ..
            } => seen_snapshots.insert((session_id.clone(), *offset)),
            TerminalFrameCommand::RequestSearch {
                session_id,
                purpose,
                ..
            } => seen_searches.insert((
                session_id.clone(),
                match purpose {
                    TerminalFrameSearchPurpose::Find => 0,
                    TerminalFrameSearchPurpose::FindVisible { .. } => 1,
                    TerminalFrameSearchPurpose::SelectedOccurrence => 2,
                    TerminalFrameSearchPurpose::SelectedOccurrenceVisible { .. } => 3,
                },
            )),
            TerminalFrameCommand::SetSnapshotPriority { .. } => {
                if kept_snapshot_priority {
                    false
                } else {
                    kept_snapshot_priority = true;
                    true
                }
            }
            _ => true,
        };
        if keep {
            compacted.push_front(command);
        }
    }

    *commands = compacted;
}

pub(super) fn terminal_frame_command_is_fence(_command: &TerminalFrameCommand) -> bool {
    if matches!(
        _command,
        TerminalFrameCommand::RekeySession { .. }
            | TerminalFrameCommand::SetCollapsedCommands { .. }
            | TerminalFrameCommand::NavigateCommand { .. }
            | TerminalFrameCommand::CommandInput { .. }
    ) {
        return true;
    }
    if matches!(_command, TerminalFrameCommand::Fence { .. }) {
        return true;
    }
    false
}

pub(super) fn terminal_frame_command_can_drop_under_pressure(
    command: &TerminalFrameCommand,
) -> bool {
    matches!(
        command,
        TerminalFrameCommand::RequestSnapshot {
            priority: false,
            ..
        } | TerminalFrameCommand::RequestSearch { .. }
    )
}

pub(super) fn terminal_frame_command_is_low_priority_derived(
    command: &TerminalFrameCommand,
) -> bool {
    matches!(
        command,
        TerminalFrameCommand::RequestSnapshot {
            priority: false,
            ..
        } | TerminalFrameCommand::RequestSearch { .. }
    )
}

pub(super) fn terminal_frame_command_priority_snapshot_insert_before(
    command: &TerminalFrameCommand,
) -> bool {
    matches!(
        command,
        TerminalFrameCommand::Output { .. }
            | TerminalFrameCommand::RequestSnapshot { .. }
            | TerminalFrameCommand::RequestSearch { .. }
    )
}

pub(super) fn terminal_frame_command_output_bytes(command: &TerminalFrameCommand) -> usize {
    match command {
        TerminalFrameCommand::ExecutionRecording { event, .. } => event.payload_bytes(),
        TerminalFrameCommand::Output { data, .. } => data.len(),
        TerminalFrameCommand::DecodedOutput { text, .. }
        | TerminalFrameCommand::SyntheticOutput { text, .. }
        | TerminalFrameCommand::SystemRecording { text, .. } => text.len(),
        _ => 0,
    }
}

pub(super) const TERMINAL_FRAME_COMMAND_QUEUE_CAP: usize = 512;

/// Maximum adjacent output growth per queued command.
pub(super) const TERMINAL_FRAME_OUTPUT_CHUNK_SIZE: usize = 8 * 1024;
