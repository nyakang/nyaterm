//! Ownership and submission facade for the terminal frame worker.

use super::frame_commands::{
    TerminalFrameCommand, TerminalFrameCommandSender, TerminalFrameScrollTarget,
    TerminalSelectionTextRequest, TerminalSelectionTextResult, terminal_frame_command_channel,
};
use super::frame_events::{
    TERMINAL_FRAME_EVENT_QUEUE_CAP, TERMINAL_FRAME_EVENT_WAKE_ALL,
    TERMINAL_FRAME_EVENT_WAKE_OUTPUT, TERMINAL_FRAME_EVENT_WAKE_SEARCH,
    TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT, TerminalFrameEvent, TerminalFrameEventQueue,
    TerminalFrameSnapshotPurpose,
};
use super::frame_search::{TerminalFrameSearchKey, TerminalFrameSearchPurpose};
use super::frame_worker::{TerminalFrameSession, run_terminal_frame_processor};
use super::{TerminalSelection, TerminalSelectionTextError};
use crate::models::recording::{ExecutionRecordingEvent, RecordingWriteHandle};
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot;
use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_core::keyword_highlight_presets::ResolvedKeywordHighlightRule;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct TerminalFramePipeline {
    pub(super) keyword_rules: Arc<Mutex<Arc<Vec<ResolvedKeywordHighlightRule>>>>,
    pub(super) logical_line_snapshots: Arc<AtomicBool>,
    pub(super) recording_writer: RecordingWriteHandle,
    pub(super) command_tx: TerminalFrameCommandSender,
    pub(super) event_queue: TerminalFrameEventQueue,
    pub(super) event_wake_rx: Arc<Mutex<Option<UnboundedReceiver<()>>>>,
    pub(super) handle_count: Arc<AtomicUsize>,
    pub(super) worker: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

pub(crate) struct TerminalFrameOutputSubmission {
    /// Set by ingress capture before charset decoding or UI output processing.
    pub(crate) raw_already_captured: bool,
    pub(crate) session_id: String,
    pub(crate) data: Vec<u8>,
    pub(crate) encoding: String,
    pub(crate) scrollback_limit: usize,
}

/// Hand a submission to the frame processor as one command.
///
/// Splitting it here would only be undone downstream — `push_terminal_frame_command`
/// re-merges adjacent same-session output on enqueue, and the worker coalesces
/// again up to [`TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT`] — while costing a copy
/// of the payload plus two `String` clones per slice.
pub(super) fn terminal_frame_output_commands(
    output: TerminalFrameOutputSubmission,
) -> Option<TerminalFrameCommand> {
    if output.data.is_empty() {
        return None;
    }
    Some(TerminalFrameCommand::Output {
        session_id: output.session_id,
        data: output.data,
        encoding: output.encoding,
        scrollback_limit: output.scrollback_limit,
    })
}

impl TerminalFramePipeline {
    pub(crate) fn set_collapsed_commands(
        &self,
        session_id: String,
        ids: std::collections::BTreeSet<nyaterm_terminal::command_blocks::CommandBlockId>,
    ) {
        self.arm_event_wakes();
        let _ = self
            .command_tx
            .send(TerminalFrameCommand::SetCollapsedCommands { session_id, ids });
    }

    pub(crate) fn set_keyword_rules(&self, rules: Arc<Vec<ResolvedKeywordHighlightRule>>) {
        let Ok(mut current) = self.keyword_rules.lock() else {
            return;
        };
        if Arc::ptr_eq(&current, &rules) || current.as_ref() == rules.as_ref() {
            return;
        }
        *current = rules;
    }

    pub(crate) fn take_events_for_transfer(
        &self,
        session_ids: &[String],
    ) -> VecDeque<TerminalFrameEvent> {
        self.event_queue.take_sessions(session_ids)
    }

    pub(crate) fn spawn(recording_writer: RecordingWriteHandle) -> Self {
        let (command_tx, command_rx) = terminal_frame_command_channel();
        let (event_queue, event_wake_rx) =
            TerminalFrameEventQueue::new_with_wake(TERMINAL_FRAME_EVENT_QUEUE_CAP);
        let event_queue_for_worker = event_queue.clone();
        let worker_recording_writer = recording_writer.clone();
        let worker = thread::Builder::new()
            .name("nyaterm-terminal-frame-processor".to_string())
            .spawn(move || {
                run_terminal_frame_processor(
                    command_rx,
                    event_queue_for_worker,
                    worker_recording_writer,
                )
            })
            .expect("failed to spawn terminal frame processor");
        Self {
            keyword_rules: Arc::new(Mutex::new(Arc::new(Vec::new()))),
            logical_line_snapshots: Arc::new(AtomicBool::new(false)),
            recording_writer,
            command_tx,
            event_queue,
            event_wake_rx: Arc::new(Mutex::new(Some(event_wake_rx))),
            handle_count: Arc::new(AtomicUsize::new(1)),
            worker: Arc::new(Mutex::new(Some(worker))),
        }
    }

    pub(crate) fn set_logical_line_snapshots(&self, enabled: bool) {
        if self.logical_line_snapshots.swap(enabled, Ordering::Relaxed) != enabled {
            self.arm_event_wakes();
            let _ = self
                .command_tx
                .send(TerminalFrameCommand::SetLogicalLineSnapshots { enabled });
        }
    }

    pub(crate) fn arm_output_event_wake(&self) {
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_OUTPUT);
    }

    /// Declare interest in every kind of frame event.
    ///
    /// The drain task calls this before each check, so a snapshot or search reply
    /// cannot be missed by a consumer that only armed for output. The individual
    /// arms at the request sites and the input-echo accelerator stay: `arm_wake`
    /// is a `fetch_or`, so they compose.
    pub(crate) fn arm_event_wakes(&self) {
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_ALL);
    }

    pub(crate) fn take_event_wake_receiver(&self) -> Option<UnboundedReceiver<()>> {
        self.event_wake_rx.lock().ok()?.take()
    }

    pub(crate) fn event_wake_count(&self) -> u64 {
        self.event_queue.wake_count()
    }

    pub(crate) fn ensure_session(
        &self,
        session_id: impl Into<String>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::EnsureSession {
            session_id: session_id.into(),
            encoding: encoding.into(),
            scrollback_limit,
        });
    }

    pub(crate) fn seed_session(
        &self,
        session_id: impl Into<String>,
        output: impl Into<String>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::SeedSession {
            session_id: session_id.into(),
            output: output.into(),
            encoding: encoding.into(),
            scrollback_limit,
        });
    }

    pub(crate) fn remove_session(&self, session_id: impl Into<String>) {
        let _ = self.command_tx.send(TerminalFrameCommand::RemoveSession {
            session_id: session_id.into(),
        });
    }

    /// Preserve diagnostic recording order without advancing either decoder or grid.
    pub(crate) fn submit_system_recording(&self, session_id: &str, text: String) {
        let _ = self.command_tx.send(TerminalFrameCommand::SystemRecording {
            session_id: session_id.to_string(),
            text,
        });
    }

    pub(crate) fn append_local_text(&self, session_id: impl Into<String>, text: impl Into<String>) {
        let _ = self.command_tx.send(TerminalFrameCommand::AppendLocalText {
            session_id: session_id.into(),
            text: text.into(),
        });
    }

    pub(crate) fn rekey_session(
        &self,
        old_id: impl Into<String>,
        new_id: impl Into<String>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) -> bool {
        self.command_tx.send(TerminalFrameCommand::RekeySession {
            old_id: old_id.into(),
            new_id: new_id.into(),
            encoding: encoding.into(),
            scrollback_limit,
        })
    }

    pub(crate) fn navigate_command(
        &self,
        session_id: String,
        action: nyaterm_terminal::command_navigation::CommandNavigationAction,
        offset: usize,
        source_history: nyaterm_terminal::TerminalHistoryState,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::NavigateCommand {
            session_id,
            action,
            offset,
            source_history,
        });
    }

    pub(crate) fn note_command_input(
        &self,
        session_id: String,
        fallback: Option<nyaterm_terminal::TerminalLineId>,
    ) {
        let _ = self.command_tx.send(TerminalFrameCommand::CommandInput {
            session_id,
            fallback,
        });
    }

    pub(crate) fn clear_session_except_input(&self, session_id: impl Into<String>) {
        let _ = self
            .command_tx
            .send(TerminalFrameCommand::ClearExceptInput {
                session_id: session_id.into(),
            });
    }

    pub(crate) fn take_session_for_transfer(
        &self,
        session_id: impl Into<String>,
    ) -> Result<Option<TerminalFrameSession>, &'static str> {
        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(0);
        if !self.command_tx.send(TerminalFrameCommand::TakeSession {
            session_id: session_id.into(),
            response_tx,
        }) {
            return Err("the source terminal pipeline is unavailable");
        }
        response_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "the source terminal pipeline did not respond")
    }

    pub(crate) fn insert_sessions_from_transfer(
        &self,
        sessions: Vec<(String, TerminalFrameSession)>,
    ) -> Result<(), Vec<(String, TerminalFrameSession)>> {
        if sessions.is_empty() {
            return Ok(());
        }
        self.command_tx
            .send_owned(TerminalFrameCommand::InsertSessions { sessions })
            .map_err(|command| match command {
                TerminalFrameCommand::InsertSessions { sessions } => sessions,
                _ => unreachable!("failed command must be the insertion batch"),
            })
    }

    pub(crate) fn resize_session(&self, session_id: impl Into<String>, cols: u16, rows: u16) {
        let _ = self.command_tx.send(TerminalFrameCommand::ResizeSession {
            session_id: session_id.into(),
            cols,
            rows,
        });
    }

    /// Recording-only events share the frame queue with ordinary decoded output,
    /// so they cannot overtake output still being processed by this worker.
    pub(crate) fn submit_execution_recording(
        &self,
        session_id: &str,
        event: ExecutionRecordingEvent,
    ) {
        if let ExecutionRecordingEvent::Output(text) = event {
            let mut remaining = text.as_str();
            while !remaining.is_empty() {
                let mut end = remaining.len().min(64 * 1024);
                while !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                let _ = self
                    .command_tx
                    .send(TerminalFrameCommand::ExecutionRecording {
                        session_id: session_id.into(),
                        event: ExecutionRecordingEvent::Output(remaining[..end].into()),
                    });
                remaining = &remaining[end..];
            }
        } else {
            let _ = self
                .command_tx
                .send(TerminalFrameCommand::ExecutionRecording {
                    session_id: session_id.into(),
                    event,
                });
        }
    }

    pub(crate) fn submit_synthetic_output(&self, session_id: String, text: String) {
        if !text.is_empty() {
            let _ = self
                .command_tx
                .send(TerminalFrameCommand::SyntheticOutput { session_id, text });
        }
    }

    pub(crate) fn submit_decoded_output(&self, session_id: String, text: String) {
        if !text.is_empty() {
            let _ = self
                .command_tx
                .send(TerminalFrameCommand::DecodedOutput { session_id, text });
        }
    }

    #[cfg(test)]
    pub(crate) fn submit_output(
        &self,
        session_id: impl Into<String>,
        data: Vec<u8>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        self.submit_outputs(vec![TerminalFrameOutputSubmission {
            session_id: session_id.into(),
            data,
            encoding: encoding.into(),
            scrollback_limit,
            raw_already_captured: false,
        }]);
    }

    pub(crate) fn submit_captured_output(
        &self,
        session_id: impl Into<String>,
        data: Vec<u8>,
        encoding: impl Into<String>,
        scrollback_limit: usize,
    ) {
        self.submit_outputs(vec![TerminalFrameOutputSubmission {
            session_id: session_id.into(),
            data,
            encoding: encoding.into(),
            scrollback_limit,
            raw_already_captured: true,
        }]);
    }

    pub(crate) fn submit_outputs(&self, outputs: Vec<TerminalFrameOutputSubmission>) {
        for output in &outputs {
            if !output.raw_already_captured {
                self.recording_writer
                    .write_raw_output(&output.session_id, &output.data);
            }
        }
        let commands = outputs.into_iter().flat_map(terminal_frame_output_commands);
        let _ = self.command_tx.send_many(commands);
    }

    #[cfg(test)]
    pub(crate) fn request_snapshot(
        &self,
        session_id: impl Into<String>,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
    ) {
        self.request_snapshot_with_priority(
            session_id,
            TerminalFrameScrollTarget {
                offset,
                history: None,
            },
            action_links_enabled,
            action_link_matchers,
            false,
            TerminalFrameSnapshotPurpose::Paint,
        );
    }

    pub(crate) fn request_action_link_enrichment(
        &self,
        session_id: impl Into<String>,
        action_link_matchers: ActionLinksMatcherSettings,
    ) {
        self.request_snapshot_with_priority(
            session_id,
            TerminalFrameScrollTarget {
                offset: 0,
                history: None,
            },
            true,
            action_link_matchers,
            false,
            TerminalFrameSnapshotPurpose::ActionLinkEnrichment,
        );
    }

    #[cfg(test)]
    pub(crate) fn request_priority_snapshot(
        &self,
        session_id: impl Into<String>,
        offset: usize,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
    ) {
        self.event_queue
            .arm_wake(TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT);
        self.request_snapshot_with_priority(
            session_id,
            TerminalFrameScrollTarget {
                offset,
                history: None,
            },
            action_links_enabled,
            action_link_matchers,
            true,
            TerminalFrameSnapshotPurpose::Paint,
        );
    }

    pub(crate) fn request_anchored_snapshot(
        &self,
        session_id: String,
        offset: usize,
        history: nyaterm_terminal::TerminalHistoryState,
        action_links_enabled: bool,
        matchers: ActionLinksMatcherSettings,
        priority: bool,
    ) {
        if priority {
            self.event_queue
                .arm_wake(TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT);
        }
        self.request_snapshot_with_priority(
            session_id,
            TerminalFrameScrollTarget {
                offset,
                history: Some(history),
            },
            action_links_enabled,
            matchers,
            priority,
            TerminalFrameSnapshotPurpose::Paint,
        );
    }

    pub(super) fn request_snapshot_with_priority(
        &self,
        session_id: impl Into<String>,
        target: TerminalFrameScrollTarget,
        action_links_enabled: bool,
        action_link_matchers: ActionLinksMatcherSettings,
        priority: bool,
        purpose: TerminalFrameSnapshotPurpose,
    ) {
        let TerminalFrameScrollTarget {
            offset,
            history: source_history,
        } = target;
        let _ = self.command_tx.send(TerminalFrameCommand::RequestSnapshot {
            session_id: session_id.into(),
            offset,
            source_history,
            action_links_enabled,
            action_link_matchers,
            priority,
            purpose,
        });
    }

    pub(crate) fn request_all_text(&self, session_id: String) -> oneshot::Receiver<Option<String>> {
        let (response_tx, response_rx) = oneshot::channel();
        let _ = self.command_tx.send(TerminalFrameCommand::RequestAllText {
            session_id,
            response_tx,
        });
        response_rx
    }

    #[cfg(test)]
    pub(crate) async fn request_selection_text(
        &self,
        session_id: String,
        selection: TerminalSelection,
        history: nyaterm_terminal::TerminalHistoryState,
    ) -> Result<Result<Option<String>, TerminalSelectionTextError>, oneshot::Canceled> {
        self.request_selection_text_frame(session_id, selection, history)
            .await
            .map(|result| result.map(|frame| frame.text))
    }

    pub(crate) fn request_selection_text_frame(
        &self,
        session_id: String,
        selection: TerminalSelection,
        history: nyaterm_terminal::TerminalHistoryState,
    ) -> oneshot::Receiver<Result<TerminalSelectionTextResult, TerminalSelectionTextError>> {
        let (response_tx, response_rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(TerminalFrameCommand::RequestSelectionText {
                session_id,
                request: Box::new(TerminalSelectionTextRequest { selection, history }),
                response_tx,
            });
        response_rx
    }

    pub(crate) fn request_search(
        &self,
        session_id: impl Into<String>,
        purpose: TerminalFrameSearchPurpose,
        key: TerminalFrameSearchKey,
    ) {
        if key.query.trim().is_empty() || key.limit == 0 {
            return;
        }
        self.event_queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_SEARCH);
        let _ = self.command_tx.send(TerminalFrameCommand::RequestSearch {
            session_id: session_id.into(),
            purpose,
            key,
        });
    }

    pub(crate) fn cancel_find_search(&self, session_id: String) {
        let _ = self
            .command_tx
            .send(TerminalFrameCommand::CancelFindSearch { session_id });
    }

    pub(crate) fn set_snapshot_priority(&self, session_ids: Vec<String>) {
        let _ = self
            .command_tx
            .send(TerminalFrameCommand::SetSnapshotPriority { session_ids });
    }

    /// Wait until the worker has processed every command ordered before this fence.
    ///
    /// This is intentionally test-only: production consumers synchronize through
    /// event wakes, while tests need deterministic completion without sleeps.
    #[cfg(test)]
    pub(crate) fn flush_for_test(&self) {
        self.finish_pending_output()
            .expect("terminal frame worker did not reach fence");
    }

    /// Called only from background jobs so stop/export include already submitted output.
    pub(crate) fn finish_pending_output(&self) -> Result<(), String> {
        let (complete_tx, complete_rx) = std::sync::mpsc::sync_channel(0);
        if !self
            .command_tx
            .send(TerminalFrameCommand::Fence { complete_tx })
        {
            return Err("terminal frame worker stopped".into());
        }
        complete_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "terminal frame worker did not reach recording fence".into())
    }

    pub(crate) fn drain_events_into(
        &self,
        events: &mut VecDeque<TerminalFrameEvent>,
        limit: usize,
    ) -> usize {
        self.event_queue.drain_into(events, limit)
    }

    pub(crate) fn queued_event_count(&self) -> usize {
        self.event_queue.len()
    }

    pub(crate) fn queued_command_count(&self) -> usize {
        self.command_tx.len()
    }

    pub(crate) fn queued_output_bytes(&self) -> usize {
        self.command_tx.queued_output_bytes()
    }

    pub(crate) fn take_shutdown(&mut self) -> Option<impl FnOnce() + Send + 'static> {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()?;
        let commands = self.command_tx.clone();
        let events = self.event_queue.clone();
        Some(move || {
            commands.close();
            events.close();
            if worker.join().is_err() {
                tracing::warn!("terminal frame worker panicked during shutdown");
            }
        })
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(shutdown) = self.take_shutdown() {
            shutdown();
        }
    }
}

impl Clone for TerminalFramePipeline {
    fn clone(&self) -> Self {
        self.handle_count.fetch_add(1, Ordering::Relaxed);
        Self {
            keyword_rules: self.keyword_rules.clone(),
            logical_line_snapshots: self.logical_line_snapshots.clone(),
            recording_writer: self.recording_writer.clone(),
            command_tx: self.command_tx.clone(),
            event_queue: self.event_queue.clone(),
            event_wake_rx: self.event_wake_rx.clone(),
            handle_count: self.handle_count.clone(),
            worker: self.worker.clone(),
        }
    }
}

impl Drop for TerminalFramePipeline {
    fn drop(&mut self) {
        if self.handle_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shutdown();
        }
    }
}

impl Default for TerminalFramePipeline {
    fn default() -> Self {
        let recording_writer = crate::models::recording::RecordingWritePipeline::spawn(
            nyaterm_recording::recording::DEFAULT_MEMORY_LIMIT_BYTES,
        )
        .writer();
        Self::spawn(recording_writer)
    }
}
