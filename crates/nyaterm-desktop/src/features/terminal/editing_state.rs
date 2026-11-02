//! Session-owned shell editing state, independent of suggestion visibility.

use std::collections::HashMap;
use std::time::Instant;

use nyaterm_core::terminal::editing::{EditCapability, EditPhase, ShellEditState};
use nyaterm_core::terminal::input_tracker::TerminalInputState;
use nyaterm_terminal::editing::InputMapping;
use nyaterm_terminal::{ShellInputAnchor, TerminalLineId, TerminalSnapshot};
use zeroize::Zeroize;

use crate::models::TerminalSelection;

#[derive(Default)]
pub(super) struct TerminalEditingState {
    active: String,
    next_confirmation_id: u64,
    pub(super) sessions: HashMap<String, SessionEditingState>,
}

#[derive(Default)]
pub(super) struct SessionEditingState {
    pub(super) command_navigation_interactive: bool,
    pub(super) model: ShellEditState,
    pub(super) capability: EditCapability,
    pub(super) echo_line_editor: bool,
    pub(super) launched_line_editor: bool,
    pub(super) line_editor_prompt: Option<String>,
    pub(super) mapping: Option<InputMapping>,
    pub(super) selection: Option<(TerminalSelection, u64)>,
    pub(super) selection_origin_version: Option<u64>,
    pub(super) geometry: Option<(usize, usize, u64)>,
    pub(super) observed_anchor: Option<Option<ShellInputAnchor>>,
    pub(super) submitted_anchor: Option<ShellInputAnchor>,
    pub(super) confirmation_task: Option<gpui::Task<()>>,
    pub(super) confirmation_id: u64,
    pub(super) suggestion_fill: Option<SuggestionFillRequest>,
    pub(super) pending_evidence: Option<SnapshotEvidence>,
    pub(super) awaiting_snapshot: bool,
}

/// This is a fill acknowledgement, never permission to synthesize Enter.
pub(super) struct SuggestionFillRequest {
    pub(super) request_id: u64,
    pub(super) input_version: u64,
    pub(super) target: String,
    pub(super) execute_requested: bool,
    pub(super) deadline: Instant,
    pub(super) sync_revision: u64,
}

impl Drop for SuggestionFillRequest {
    fn drop(&mut self) {
        self.target.zeroize();
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct SnapshotEvidence {
    cursor: (usize, usize),
    anchor: Option<ShellInputAnchor>,
    rows: Vec<(Option<TerminalLineId>, u64)>,
}

impl SnapshotEvidence {
    pub(super) fn from_snapshot(snapshot: &TerminalSnapshot) -> Self {
        Self {
            cursor: (snapshot.cursor.row, snapshot.cursor.col),
            anchor: snapshot.shell_input_anchor,
            rows: snapshot
                .rows()
                .iter()
                .map(|row| (row.line_id, row.revision))
                .collect(),
        }
    }
}

impl SessionEditingState {
    pub(super) fn cancel_stale_suggestion_fill(&mut self, now: Instant, sync_revision: u64) {
        if self.suggestion_fill.as_ref().is_some_and(|request| {
            request.request_id != self.confirmation_id
                || request.input_version != self.model.version
                || request.sync_revision != sync_revision
                || now >= request.deadline
                || request.target != self.model.input.value
                || self.model.input.cursor != self.model.input.value.len()
                || self.model.input.desynced
                || self.model.input.line_rewrite_required
                || self.model.input.multiline
                || self.model.input.paste_mode
                || self.model.phase == EditPhase::Uncertain
        }) {
            self.suggestion_fill = None;
        }
    }

    pub(super) fn take_confirmed_suggestion_fill(
        &mut self,
        now: Instant,
        sync_revision: u64,
    ) -> Option<bool> {
        self.cancel_stale_suggestion_fill(now, sync_revision);
        if self.model.phase != EditPhase::Ready {
            return None;
        }
        self.suggestion_fill
            .take()
            .map(|request| request.execute_requested)
    }

    pub(super) fn allows_editing(&self) -> bool {
        self.capability.allows_editing(self.echo_line_editor)
    }

    fn deactivate(&mut self) {
        self.model.deactivate(self.allows_editing());
        self.invalidate();
    }

    pub(super) fn invalidate(&mut self) {
        self.suggestion_fill = None;
        self.model.invalidate();
        self.capability = EditCapability::None;
        self.mapping = None;
        self.selection = None;
        self.selection_origin_version = None;
        self.pending_evidence = None;
        self.confirmation_task = None;
    }
}

impl TerminalEditingState {
    pub(super) fn allocate_confirmation_id(&mut self) -> u64 {
        self.next_confirmation_id = self
            .next_confirmation_id
            .checked_add(1)
            .expect("terminal confirmation identity exhausted");
        self.next_confirmation_id
    }

    pub(super) fn activate(&mut self, session_id: &str) {
        if self.active != session_id {
            if let Some(state) = self.sessions.get_mut(&self.active) {
                state.deactivate();
            }
            self.active = session_id.to_string();
            self.state_mut().invalidate();
        }
    }

    pub(super) fn state(&self) -> Option<&SessionEditingState> {
        self.sessions.get(&self.active)
    }
    pub(super) fn state_mut(&mut self) -> &mut SessionEditingState {
        self.sessions.entry(self.active.clone()).or_default()
    }

    pub(super) fn predicted_input(&self) -> &TerminalInputState {
        static EMPTY: std::sync::OnceLock<TerminalInputState> = std::sync::OnceLock::new();
        self.state().map_or_else(
            || EMPTY.get_or_init(TerminalInputState::new),
            |state| &state.model.input,
        )
    }

    pub(super) fn assist_input(&self) -> &TerminalInputState {
        static UNAVAILABLE: std::sync::OnceLock<TerminalInputState> = std::sync::OnceLock::new();
        self.state()
            .and_then(|state| state.model.assist_input())
            .unwrap_or_else(|| {
                UNAVAILABLE.get_or_init(|| TerminalInputState {
                    desynced: true,
                    desync_reason: Some("unconfirmed_input"),
                    ..TerminalInputState::new()
                })
            })
    }

    /// Invalidates the old mapping before credential input can reset the tracker.
    pub(super) fn input_mut(&mut self) -> &mut TerminalInputState {
        let state = self.state_mut();
        state.invalidate();
        state.model.clear_input();
        &mut state.model.input
    }

    pub(super) fn clear_active(&mut self) {
        let awaiting_snapshot = self.state_mut().awaiting_snapshot;
        let version = self.state_mut().model.version.wrapping_add(1);
        let mut model = ShellEditState::default();
        model.version = version;
        *self.state_mut() = SessionEditingState {
            awaiting_snapshot,
            model,
            ..SessionEditingState::default()
        };
    }
}

impl super::state::TerminalFeatureState {
    pub(in crate::features) fn activate_shell_editing_session(&mut self, session_id: &str) {
        self.editing.activate(session_id);
    }

    pub(in crate::features) fn invalidate_shell_editing_session(&mut self, session_id: &str) {
        if let Some(state) = self.editing.sessions.get_mut(session_id) {
            state.invalidate();
            state.model.clear_input();
            state.model.input.desynced = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SuggestionFillRequest, TerminalEditingState};
    use nyaterm_core::terminal::editing::{EDIT_CONFIRMATION_TIMEOUT, EditCapability, EditPhase};
    use std::time::Instant;

    #[test]
    fn switching_sessions_retains_only_confirmed_editable_input() {
        let mut state = TerminalEditingState::default();
        state.activate("one");
        state.state_mut().model.note_input("abc", Instant::now());
        state.state_mut().model.confirm();
        state.state_mut().capability = EditCapability::MarkedShell;
        state.state_mut().model.queued_cursor = Some(1);
        state.activate("two");
        state.state_mut().model.note_input("xyz", Instant::now());
        state.activate("one");
        assert_eq!(state.predicted_input().value, "abc");
        assert_eq!(state.state().unwrap().model.phase, EditPhase::Uncertain);
        assert_eq!(state.state().unwrap().model.queued_cursor, None);
        assert!(state.sessions["two"].model.input.value.is_empty());
        assert!(state.assist_input().desynced);
        state.state_mut().model.confirm();
        assert_eq!(state.assist_input().value, "abc");
    }

    #[test]
    fn suggestion_confirmation_requires_current_identity_version_and_deadline() {
        let now = Instant::now();
        for scenario in 0..7 {
            let mut editing = TerminalEditingState::default();
            editing.activate("one");
            let request_id = editing.allocate_confirmation_id();
            let state = editing.state_mut();
            state.model.note_input("abc", now);
            state.confirmation_id = request_id;
            state.suggestion_fill = Some(SuggestionFillRequest {
                request_id,
                input_version: state.model.version,
                target: "abc".into(),
                execute_requested: true,
                deadline: now + EDIT_CONFIRMATION_TIMEOUT,
                sync_revision: 0,
            });
            match scenario {
                0 => state.confirmation_id += 1,
                1 => state.model.version += 1,
                2 => state.suggestion_fill.as_mut().unwrap().deadline = now,
                3 => state.suggestion_fill.as_mut().unwrap().sync_revision = 1,
                4 => state.model.input.value.push('x'),
                5 => {
                    state.model.expire(now + EDIT_CONFIRMATION_TIMEOUT);
                }
                6 => {}
                _ => unreachable!(),
            }
            // Late echo can restore ordinary tracking, but never an old request.
            state.cancel_stale_suggestion_fill(now, 0);
            state.model.confirm();
            assert_eq!(
                state.take_confirmed_suggestion_fill(now, 0),
                (scenario == 6).then_some(true)
            );
            assert!(state.take_confirmed_suggestion_fill(now, 0).is_none());
        }
    }

    #[test]
    fn reset_and_reconnect_do_not_reuse_input_or_confirmation_identities() {
        let mut editing = TerminalEditingState::default();
        editing.activate("one");
        let first_id = editing.allocate_confirmation_id();
        let version = editing.state().unwrap().model.version;
        editing.clear_active();
        assert_ne!(editing.state().unwrap().model.version, version);
        editing.activate("two");
        editing.activate("one");
        assert_ne!(editing.allocate_confirmation_id(), first_id);
    }
}
