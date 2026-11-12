//! Session-owned shell editing state, independent of suggestion visibility.

use std::collections::HashMap;
use std::time::Instant;

use nyaterm_core::terminal::editing::{
    EditCapability, EditPhase, ShellEditState, supports_scalar_editing,
};
use nyaterm_core::terminal::input_tracker::TerminalInputState;
use nyaterm_core::terminal::shell_prompt::ShellPrompt;
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
    // Survives snapshot reconciliation: recovering a history line is not typing.
    pub(super) browsing_history: bool,
    pub(super) command_navigation: super::navigation_input::CommandNavigationInputState,
    pub(super) model: ShellEditState,
    pub(super) capability: EditCapability,
    pub(super) echo_line_editor: bool,
    pub(super) launched_line_editor: bool,
    pub(super) line_editor_prompt: Option<String>,
    pub(super) editor_evidence: Option<ShellPrompt>,
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
    /// Survives `clear_active`: a password prompt or alternate screen inside
    /// the nested shell must not make the outer host's files look current.
    pub(super) foreign_shells: ForeignShells,
}

/// Interactive shells this session opened on another host, container or
/// distro (`ssh`, `docker exec -it`, ...). Path suggestions list the session's
/// own file system, which is the wrong one while this is non-zero.
#[derive(Default)]
pub(super) struct ForeignShells {
    depth: u8,
    /// Prompt prefix of the outermost shell, to notice a return without `exit`
    /// (a failed `ssh` never opens a shell, yet nothing else announces it).
    outer_prompt: Option<String>,
}

impl ForeignShells {
    /// A bound keeps a runaway chain from overflowing; real nesting is shallow.
    const MAX_DEPTH: u8 = 8;

    pub(super) fn is_active(&self) -> bool {
        self.depth > 0
    }

    pub(super) fn enter(&mut self, count: usize, outer_prompt: Option<&str>) {
        if count == 0 {
            return;
        }
        if self.depth == 0 {
            self.outer_prompt = outer_prompt
                .filter(|prompt| !prompt.trim().is_empty())
                .map(str::to_owned);
        }
        let added = u8::try_from(count).unwrap_or(u8::MAX);
        self.depth = self.depth.saturating_add(added).min(Self::MAX_DEPTH);
    }

    /// `exit`, `logout`, or Ctrl+D on an empty line.
    pub(super) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            self.outer_prompt = None;
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// The outermost shell's prompt is back on screen: the nested shell ended
    /// or never started. Prompts that differ keep the guard on.
    pub(super) fn note_prompt(&mut self, prompt: &str) {
        if self.depth > 0 && self.outer_prompt.as_deref() == Some(prompt) {
            self.reset();
        }
    }
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
    pub(super) fn note_suggestion_input(&mut self, text: &str) {
        if matches!(
            text,
            "\u{1b}[A" | "\u{1b}OA" | "\u{1b}[B" | "\u{1b}OB" | "\u{10}" | "\u{e}"
        ) {
            self.browsing_history = true;
        } else if (!text.is_empty()
            && text
                .chars()
                .all(|ch| !ch.is_control() || matches!(ch, '\r' | '\n' | '\t')))
            || text.contains("\u{1b}[200~")
            || matches!(
                text,
                "\r" | "\n"
                    | "\u{3}"
                    | "\u{4}"
                    | "\t"
                    | "\u{7f}"
                    | "\u{8}"
                    | "\u{15}"
                    | "\u{17}"
                    | "\u{b}"
                    | "\u{1b}[3~"
            )
        {
            self.browsing_history = false;
        }
    }

    pub(super) fn confirmed_input_at_end(&self) -> bool {
        let input = &self.model.input;
        !self.awaiting_snapshot
            && self.model.phase == EditPhase::Ready
            && self.mapping.is_some()
            && !input.desynced
            && !input.line_rewrite_required
            && !input.multiline
            && !input.paste_mode
            && input.cursor == input.value.len()
            && supports_scalar_editing(&input.value)
    }

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

    pub(super) fn observe_editor_evidence(&mut self, backend: Option<ShellPrompt>) {
        if self.editor_evidence != backend {
            self.editor_evidence = backend;
            self.model.version = self.model.version.wrapping_add(1);
            self.suggestion_fill = None;
        }
    }

    fn deactivate(&mut self) {
        self.model.deactivate(self.allows_editing());
        self.invalidate();
    }

    pub(super) fn invalidate(&mut self) {
        self.suggestion_fill = None;
        self.model.invalidate();
        self.capability = EditCapability::None;
        self.editor_evidence = None;
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
    pub(super) fn is_active(&self, session_id: &str) -> bool {
        self.active == session_id
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
        // Losing editing authorization (alternate screen, credentials, locks)
        // cannot prove that the foreground program returned to the shell.
        let command_navigation = std::mem::take(&mut self.state_mut().command_navigation);
        let awaiting_snapshot = self.state_mut().awaiting_snapshot;
        let foreign_shells = std::mem::take(&mut self.state_mut().foreign_shells);
        let version = self.state_mut().model.version.wrapping_add(1);
        let mut model = ShellEditState::default();
        model.version = version;
        *self.state_mut() = SessionEditingState {
            command_navigation,
            awaiting_snapshot,
            foreign_shells,
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
            // Only a line that already holds tracked text has an echo that the
            // new geometry can invalidate. The first pane layout also resizes an
            // untouched line; marking that empty tracker desynced would leave
            // the first line of a new session ineligible for suggestions until
            // an Enter reset the flag.
            let tracked_text = !state.model.input.value.is_empty();
            state.invalidate();
            state.model.clear_input();
            if tracked_text {
                state.model.input.desynced = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SuggestionFillRequest, TerminalEditingState};
    use nyaterm_core::terminal::editing::{EDIT_CONFIRMATION_TIMEOUT, EditCapability, EditPhase};
    use std::time::Instant;

    #[test]
    fn history_navigation_suppression_survives_cursor_moves_and_session_switches() {
        let mut state = TerminalEditingState::default();
        state.activate("one");
        state.state_mut().note_suggestion_input("\u{1b}[A");
        for input in ["\u{1b}[D", "\u{1b}[C", "\u{1b}[H", "\u{1b}[F", "\u{1b}"] {
            state.state_mut().note_suggestion_input(input);
            assert!(state.state().unwrap().browsing_history);
        }
        state.activate("two");
        assert!(!state.state().unwrap().browsing_history);
        state.activate("one");
        assert!(state.state().unwrap().browsing_history);
        for input in [
            "x",
            "中文",
            "\u{7f}",
            "\u{1b}[3~",
            "\t",
            "\r",
            "\u{3}",
            "one\ntwo",
            "\u{1b}[200~paste\u{1b}[201~",
        ] {
            state.state_mut().note_suggestion_input("\u{1b}[A");
            state.state_mut().note_suggestion_input(input);
            assert!(!state.state().unwrap().browsing_history);
        }
    }

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

#[cfg(test)]
mod foreign_shell_tests {
    use super::{ForeignShells, TerminalEditingState};

    #[test]
    fn depth_follows_enter_and_leave_and_never_underflows() {
        let mut shells = ForeignShells::default();
        assert!(!shells.is_active());
        shells.leave();
        assert!(!shells.is_active());
        shells.enter(0, None);
        assert!(!shells.is_active());
        shells.enter(1, Some("$ "));
        shells.enter(1, Some("root@box# "));
        shells.leave();
        assert!(shells.is_active());
        shells.leave();
        assert!(!shells.is_active());
    }

    #[test]
    fn nesting_is_bounded() {
        let mut shells = ForeignShells::default();
        shells.enter(usize::MAX, None);
        for _ in 0..ForeignShells::MAX_DEPTH {
            assert!(shells.is_active());
            shells.leave();
        }
        assert!(!shells.is_active());
    }

    #[test]
    fn the_outer_prompt_returning_ends_the_guard_but_other_prompts_do_not() {
        let mut shells = ForeignShells::default();
        shells.enter(1, Some("[me@mac ~]$ "));
        shells.note_prompt("root@box:/# ");
        assert!(shells.is_active());
        shells.note_prompt("[me@mac ~]$ ");
        assert!(!shells.is_active());
    }

    #[test]
    fn without_a_known_outer_prompt_only_exit_ends_the_guard() {
        let mut shells = ForeignShells::default();
        shells.enter(1, None);
        shells.note_prompt("anything");
        assert!(shells.is_active());
        let mut blank = ForeignShells::default();
        blank.enter(1, Some("   "));
        blank.note_prompt("   ");
        assert!(blank.is_active());
    }

    #[test]
    fn losing_editing_authority_does_not_forget_the_nested_shell() {
        let mut editing = TerminalEditingState::default();
        editing.activate("s1");
        editing.state_mut().foreign_shells.enter(2, Some("$ "));
        // A password prompt or alternate screen inside the nested shell.
        editing.clear_active();
        assert!(editing.state().unwrap().foreign_shells.is_active());
        editing.state_mut().foreign_shells.leave();
        editing.clear_active();
        assert!(editing.state().unwrap().foreign_shells.is_active());
        editing.state_mut().foreign_shells.leave();
        assert!(!editing.state().unwrap().foreign_shells.is_active());
    }
}
