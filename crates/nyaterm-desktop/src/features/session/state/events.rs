//! session events state and transitions.

use super::SessionFeatureState;
use super::start::SessionStartTabPlacement;
use crate::models::{SessionEventBridgeDrain, SessionEventBridgeStats, SessionRuntimeMetadata};
use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_transport::SessionEvent;
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct SessionEventQueueState {
    pub(super) pending: VecDeque<SessionEvent>,
}

impl SessionFeatureState {
    pub(in crate::features) fn configure_event_bridge(
        &self,
        encoding: String,
        scrollback_limit: usize,
    ) {
        let session_encodings = self
            .metadata_entries()
            .filter_map(|(id, metadata)| {
                metadata
                    .launch_config
                    .encoding()
                    .map(|encoding| (id.to_string(), encoding.to_string()))
            })
            .collect();
        self.event_bridge
            .configure(encoding, scrollback_limit, session_encodings);
    }
    pub(in crate::features) fn begin_terminal_execution_boundary(
        &self,
        session_id: &str,
        token: &str,
    ) {
        self.event_bridge.begin_output_boundary(session_id, token);
    }
    pub(in crate::features) fn route_session_events_to_ui(&self, session_id: &str) {
        self.event_bridge.route_session_to_ui(session_id);
    }
    pub(in crate::features) fn resume_session_direct_output(&self, session_id: &str) {
        self.event_bridge.resume_session_direct_output(session_id);
    }
    /// Release UI handoffs only after the caller has submitted its output batch.
    pub(in crate::features) fn complete_event_bridge_ui_drain(&self) {
        let pending_sessions = self
            .events
            .pending
            .iter()
            .map(|event| match event {
                SessionEvent::Output { session_id, .. }
                | SessionEvent::OutputBoundary { session_id, .. }
                | SessionEvent::OutputDropped { session_id, .. }
                | SessionEvent::CwdChanged { session_id, .. }
                | SessionEvent::CommandContext { session_id, .. }
                | SessionEvent::CommandAccepted { session_id, .. }
                | SessionEvent::Exited { session_id, .. }
                | SessionEvent::CleanupFailed { session_id, .. }
                | SessionEvent::TmuxState { session_id, .. }
                | SessionEvent::Notice { session_id, .. }
                | SessionEvent::Error { session_id, .. } => session_id.clone(),
            })
            .collect();
        self.event_bridge.complete_ui_drain(&pending_sessions);
    }
    pub(in crate::features) fn clear_event_bridge_session(&self, session_id: &str) {
        self.event_bridge.clear_session(session_id);
    }
    pub(in crate::features) fn drain_event_bridge(
        &self,
        max_events: usize,
        max_output_bytes: usize,
    ) -> SessionEventBridgeDrain {
        self.event_bridge
            .drain_events_with_output_budget(max_events, max_output_bytes)
    }
    /// Taken once by `NyaTermApp::start_runtime_data_plane_drain`.
    pub(in crate::features) fn take_event_bridge_wake_receiver(
        &self,
    ) -> Option<UnboundedReceiver<()>> {
        self.event_bridge.take_ui_queue_wake_receiver()
    }
    pub(in crate::features) fn arm_event_bridge_wake(&self) {
        self.event_bridge.arm_ui_queue_wake();
    }
    #[cfg(test)]
    pub(in crate::features) fn push_event_bridge_ui_event_for_test(&self, event: SessionEvent) {
        self.event_bridge.push_ui_event_for_test(event);
    }
    pub(in crate::features) fn harvest_event_bridge_stats(&self) -> SessionEventBridgeStats {
        self.event_bridge.harvest_direct_stats()
    }
    pub(in crate::features) fn event_bridge_has_pending_ui_work(&self) -> bool {
        self.event_bridge.has_pending_ui_work()
    }
    pub(in crate::features) fn event_bridge_queued_event_count(&self) -> usize {
        self.event_bridge.queued_event_count()
    }
    pub(in crate::features) fn event_bridge_source_queued_event_count(&self) -> usize {
        self.event_bridge.source_queued_event_count()
    }
    pub(in crate::features) fn event_bridge_queued_output_bytes(&self) -> usize {
        self.event_bridge.queued_output_bytes()
    }
    pub(in crate::features) fn event_bridge_source_queued_output_bytes(&self) -> usize {
        self.event_bridge.source_queued_output_bytes()
    }
    pub(in crate::features) fn pending_event_count(&self) -> usize {
        self.events.pending.len()
    }
    pub(in crate::features) fn pending_events_are_empty(&self) -> bool {
        self.events.pending.is_empty()
    }
    pub(in crate::features) fn extend_pending_events(
        &mut self,
        events: impl IntoIterator<Item = SessionEvent>,
    ) {
        self.events.pending.extend(events);
    }
    pub(in crate::features) fn defer_pending_event(&mut self, event: SessionEvent) {
        self.events.pending.push_front(event);
    }
    pub(in crate::features) fn pop_pending_event(&mut self) -> Option<SessionEvent> {
        self.events.pending.pop_front()
    }
    pub(in crate::features) fn pending_event_output_bytes(&self) -> usize {
        self.events
            .pending
            .iter()
            .map(|event| match event {
                SessionEvent::Output { data, .. } => data.len(),
                _ => 0,
            })
            .sum()
    }
    pub(in crate::features) fn register_session_metadata_for_start(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
        tab_placement: Option<SessionStartTabPlacement>,
        insert_index: Option<usize>,
    ) {
        self.prepare_session_metadata_for_start(session_id, metadata, tab_placement, insert_index);
        self.event_bridge.claim_session(session_id);
    }
    pub(in crate::features) fn claim_session_events(&self, session_id: &str) {
        self.event_bridge.claim_session(session_id);
    }
    #[cfg(test)]
    pub(in crate::features) fn register_provisional_reconnect(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
    ) {
        self.prepare_provisional_reconnect(session_id, metadata);
        self.event_bridge.claim_session(session_id);
    }
}
