use gpui::{
    App, Context, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseUpEvent, Pixels, Point,
};
use nyaterm_core::terminal::editing::{EditIntent, EditPhase, EditPlan, plan_edit};
use nyaterm_core::terminal::input_tracker::InputSelectionRange;
use nyaterm_terminal::editing::resolve_input;
use nyaterm_terminal::navigation::{NavigationKey, navigation_key_bytes};
use nyaterm_terminal_gpui::{
    TerminalKeyMode, terminal_key_bytes_with_mode, terminal_key_release_bytes_with_mode,
};

use super::helpers::SmartSelectionEdge;
use super::metrics::{
    terminal_cell_for_visual_geometry, terminal_snapshot_row_for_visual_geometry,
};
use crate::features::NyaTermApp;
use crate::features::terminal::editing_state::SnapshotEvidence;
use crate::features::terminal::terminal_surface::terminal_absolute_line_for_snapshot_row;
use crate::models::{TerminalBufferCellPos, TerminalSelection};

fn shell_edit_payload(
    plan: &EditPlan,
    mode: TerminalKeyMode,
    insertion_wire: Vec<u8>,
) -> (Vec<u8>, Vec<u8>) {
    let key = if plan.move_right {
        NavigationKey::Right
    } else {
        NavigationKey::Left
    };
    // Recording/tracking consumes legacy logical intents. The wire uses exactly
    // the same press/release encoder as physical input, including Kitty flags.
    let mut logical = navigation_key_bytes(key, mode.application_cursor).repeat(plan.move_steps);
    logical.extend(std::iter::repeat_n(0x7f, plan.delete_steps));
    let mut wire = shell_edit_key_bytes(if plan.move_right { "right" } else { "left" }, mode)
        .repeat(plan.move_steps);
    wire.extend(shell_edit_key_bytes("backspace", mode).repeat(plan.delete_steps));
    wire.extend(insertion_wire);
    logical.extend_from_slice(plan.insert_text.as_bytes());
    (wire, logical)
}

fn shell_edit_key_bytes(key: &str, mode: TerminalKeyMode) -> Vec<u8> {
    let keystroke = Keystroke {
        key: key.into(),
        key_char: None,
        modifiers: Modifiers::default(),
    };
    let mut bytes = terminal_key_bytes_with_mode(
        &KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        },
        mode,
    )
    .expect("shell editing keys have a terminal encoding");
    if let Some(release) = terminal_key_release_bytes_with_mode(&KeyUpEvent { keystroke }, mode) {
        bytes.extend(release);
    }
    bytes
}

impl NyaTermApp {
    pub(in crate::features) fn smart_input_line_selection_at_mouse(
        &self,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<TerminalSelection> {
        self.input_index_at_mouse(position, cx)?;
        let state = self.terminal.editing.state()?;
        if state.model.phase != EditPhase::Ready {
            return None;
        }
        let mapping = state.mapping.as_ref()?;
        let first = mapping.cells.first()?;
        let last = mapping.cells.last()?;
        let snapshot = self.terminal_snapshot_for_session(self.session.active_id(), 0);
        Some(TerminalSelection::from_range(
            TerminalBufferCellPos::new(
                terminal_absolute_line_for_snapshot_row(&snapshot, first.row)?,
                first.col,
            ),
            TerminalBufferCellPos::new(
                terminal_absolute_line_for_snapshot_row(&snapshot, last.row)?,
                last.col + last.width - 1,
            ),
        ))
    }

    pub(in crate::features) fn handle_smart_input_click(
        &mut self,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.modified() {
            return;
        }
        if let Some(id) = self.session.active_id_owned() {
            self.reconcile_shell_editing(&id, cx);
        }
        if let Some(target) = self.input_index_at_mouse(event.position, cx) {
            self.move_smart_input_cursor(target, cx);
        }
    }

    pub(in crate::features) fn can_use_smart_cursor_selection(&self) -> bool {
        let Some(id) = self.session.active_id() else {
            return false;
        };
        !self.security.screen_locked()
            && !self.session.is_disconnected(id)
            && !self.active_terminal_uses_alternate_screen()
            && !self.terminal_protocol_state_for_session(id).mouse_reporting
            && !self.is_credential_prompt_input_mode()
            && self.terminal.assist.credential_suggestions.is_none()
            && !self.sync_input.may_fan_out_from(id)
            && self.active_terminal_display_offset() == 0
            && self.terminal.editing.state().is_some_and(|state| {
                !state.awaiting_snapshot
                    && state.allows_editing()
                    && !state.model.input.desynced
                    && !state.model.input.line_rewrite_required
                    && !state.model.input.multiline
                    && !state.model.input.paste_mode
                    && state.mapping.is_some()
            })
    }

    pub(in crate::features) fn input_index_at_mouse(
        &self,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<usize> {
        if !self.can_use_smart_cursor_selection() {
            return None;
        }
        let geometry = self.terminal_hit_test_geometry_for_session(self.session.active_id(), cx)?;
        if geometry.display_offset != 0 {
            return None;
        }
        let cell = terminal_cell_for_visual_geometry(position, &geometry);
        let painted_row = terminal_snapshot_row_for_visual_geometry(position, &geometry);
        let painted_id = geometry.snapshot.row(painted_row)?.line_id?;
        let state = self.terminal.editing.state()?;
        let snapshot = self.terminal_snapshot_for_session(self.session.active_id(), 0);
        let snapshot_row = snapshot
            .rows()
            .iter()
            .position(|row| row.line_id == Some(painted_id))?;
        for (id, revision) in &state.mapping.as_ref()?.row_revisions {
            if !geometry
                .snapshot
                .rows()
                .iter()
                .any(|row| row.line_id == Some(*id) && row.revision == *revision)
            {
                return None;
            }
        }
        let mapping = state.mapping.as_ref()?;
        // While awaiting echo, the confirmed mapping still gives logical targets
        // for consecutive clicks. Only the latest target is retained.
        if state.model.phase != EditPhase::Pending
            && resolve_input(
                &snapshot,
                &state.model.input.value,
                state.model.input.cursor,
            )
            .as_ref()
                != Some(mapping)
        {
            return None;
        }
        mapping.byte_at(snapshot_row, cell.col)
    }

    pub(in crate::features) fn move_smart_input_cursor(
        &mut self,
        target: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_use_smart_cursor_selection() {
            return false;
        }
        let state = self.terminal.editing.state_mut();
        if state.model.phase == EditPhase::Pending {
            state.model.queued_cursor = Some(target);
            return true;
        }
        if state.model.phase != EditPhase::Ready {
            return false;
        }
        self.send_shell_edit(EditIntent::Move(target), false, cx)
    }

    pub(in crate::features) fn begin_smart_input_selection(&mut self) {
        let eligible = self.can_use_smart_cursor_selection();
        let state = self.terminal.editing.state_mut();
        state.model.queued_cursor = None;
        state.selection_origin_version =
            (eligible && state.model.phase == EditPhase::Ready).then_some(state.model.version);
        state.selection = None;
    }

    pub(in crate::features) fn commit_smart_input_selection(&mut self) {
        if !self.can_use_smart_cursor_selection() {
            return;
        }
        if let Some(selection) = self.terminal.selection.selection {
            let state = self.terminal.editing.state_mut();
            if state.model.phase == EditPhase::Ready
                && state.selection_origin_version == Some(state.model.version)
            {
                state.selection = Some((selection, state.model.version));
            }
        }
    }

    pub(in crate::features) fn smart_cursor_selected_input_range(
        &self,
    ) -> Option<InputSelectionRange> {
        if !self.can_use_smart_cursor_selection() {
            return None;
        }
        if self.terminal.selection.session_id.as_deref() != self.session.active_id() {
            return None;
        }
        let selection = self.terminal.selection.selection.as_ref()?;
        if selection.is_empty() || selection.all_buffer {
            return None;
        }
        let state = self.terminal.editing.state()?;
        if state.model.phase != EditPhase::Ready
            || state.selection != Some((*selection, state.model.version))
        {
            return None;
        }
        let snapshot = self.terminal_snapshot_for_session(self.session.active_id(), 0);
        let mapping = resolve_input(
            &snapshot,
            &state.model.input.value,
            state.model.input.cursor,
        )?;
        if state.mapping.as_ref() != Some(&mapping) {
            return None;
        }
        let (start, end) = selection.cell_range(snapshot.cols)?;
        let start_row = snapshot.rows().iter().enumerate().find_map(|(row, _)| {
            (terminal_absolute_line_for_snapshot_row(&snapshot, row) == Some(start.line))
                .then_some(row)
        })?;
        let end_row = snapshot.rows().iter().enumerate().find_map(|(row, _)| {
            (terminal_absolute_line_for_snapshot_row(&snapshot, row) == Some(end.line))
                .then_some(row)
        })?;
        let range = mapping.selected_range((start_row, start.col), (end_row, end.col))?;
        InputSelectionRange::new(range.start, range.end)
    }

    fn send_shell_edit(
        &mut self,
        intent: EditIntent<'_>,
        paste: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.terminal.editing.state_mut().suggestion_fill = None;
        if !self.can_use_smart_cursor_selection() {
            return false;
        }
        let Some(id) = self.session.active_id_owned() else {
            return false;
        };
        let state = self.terminal.editing.state_mut();
        if state.model.phase != EditPhase::Ready {
            return false;
        }
        let Some(plan) = plan_edit(&state.model.input, state.model.version, intent) else {
            return false;
        };
        if plan.move_steps == 0 && plan.delete_steps == 0 && plan.insert_text.is_empty() {
            return true;
        }
        let changes_text = plan.delete_steps > 0 || !plan.insert_text.is_empty();
        let mode = self.terminal_key_mode_for_session(Some(&id));
        let insertion_wire = if paste {
            self.wrap_terminal_paste_bytes_for_session(&id, &plan.insert_text)
        } else {
            self.encode_session_outgoing(&id, plan.insert_text.as_bytes())
        };
        let insertion_wire = match insertion_wire {
            Ok(bytes) => bytes,
            Err(error) => {
                self.set_terminal_status_if_changed(format!("input failed: {error}"));
                cx.notify();
                return true;
            }
        };
        let (wire, logical) = shell_edit_payload(&plan, mode, insertion_wire);
        let snapshot = self.terminal_snapshot_for_session(Some(&id), 0);
        self.terminal.view.frame_pipeline.arm_output_event_wake();
        if let Err(error) = self.write_session_wire_input_recorded_as(&id, &wire, &logical) {
            // No state was committed before the synchronous write returned.
            if self.set_terminal_status_if_changed(format!("input failed: {error}")) {
                cx.notify();
            }
            return true;
        }
        self.terminal
            .editing
            .state_mut()
            .model
            .accept_plan(plan, std::time::Instant::now());
        self.terminal.editing.state_mut().pending_evidence =
            Some(SnapshotEvidence::from_snapshot(&snapshot));
        self.terminal.editing.state_mut().selection = None;
        if changes_text {
            self.terminal.editing.state_mut().mapping = None;
        }
        self.clear_terminal_selection(cx);
        self.dismiss_command_suggestions(cx);
        self.arm_terminal_input_wake(cx);
        self.schedule_shell_edit_confirmation_timeout(cx);
        true
    }

    pub(in crate::features) fn delete_smart_input_selection(
        &mut self,
        selected: InputSelectionRange,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_shell_edit(EditIntent::Delete(selected.start..selected.end), false, cx)
    }

    pub(in crate::features) fn replace_smart_input_selection(
        &mut self,
        selected: InputSelectionRange,
        data: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_shell_edit(
            EditIntent::Replace {
                range: selected.start..selected.end,
                text: data,
            },
            false,
            cx,
        )
    }

    pub(in crate::features) fn replace_smart_input_selection_with_paste(
        &mut self,
        selected: InputSelectionRange,
        data: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_shell_edit(
            EditIntent::Replace {
                range: selected.start..selected.end,
                text: data,
            },
            true,
            cx,
        )
    }

    pub(in crate::features) fn collapse_smart_input_selection(
        &mut self,
        selected: InputSelectionRange,
        edge: SmartSelectionEdge,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = match edge {
            SmartSelectionEdge::Start => selected.start,
            SmartSelectionEdge::End => selected.end,
        };
        let handled = self.move_smart_input_cursor(target, cx);
        if handled {
            self.clear_terminal_selection(cx);
        }
        handled
    }

    pub(in crate::features) fn handle_smart_input_selection_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(selected) = self.smart_cursor_selected_input_range() else {
            return false;
        };
        let key = &event.keystroke;
        if key.modifiers.control
            || key.modifiers.platform
            || key.modifiers.alt
            || key.modifiers.function
        {
            return false;
        }
        match key.key.as_str() {
            "left" if !key.modifiers.shift => {
                self.collapse_smart_input_selection(selected, SmartSelectionEdge::Start, cx)
            }
            "right" if !key.modifiers.shift => {
                self.collapse_smart_input_selection(selected, SmartSelectionEdge::End, cx)
            }
            "backspace" | "delete" => self.delete_smart_input_selection(selected, cx),
            _ => false,
        }
    }

    pub(in crate::features) fn commit_terminal_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        if let Some(selected) = self.smart_cursor_selected_input_range()
            && self.replace_smart_input_selection(selected, text, cx)
        {
            return;
        }
        self.send_terminal_input(text.as_bytes().to_vec(), cx);
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::terminal::editing::{EditIntent, plan_edit};
    use nyaterm_core::terminal::input_tracker::TerminalInputState;

    use super::shell_edit_payload;
    use crate::features::NyaTermApp;
    use nyaterm_terminal_gpui::TerminalKeyMode;

    #[test]
    fn replacement_paste_frames_only_inserted_encoded_text() {
        let input = TerminalInputState {
            value: "abcdef".into(),
            cursor: 6,
            ..TerminalInputState::default()
        };
        let plan = plan_edit(
            &input,
            0,
            EditIntent::Replace {
                range: 1..4,
                text: "中文",
            },
        )
        .unwrap();
        let encoded = [0xd6, 0xd0, 0xce, 0xc4];
        let body = NyaTermApp::wrap_terminal_paste_wire_bytes_for_bracketed(&encoded, true);
        let (wire, logical) = shell_edit_payload(
            &plan,
            TerminalKeyMode {
                application_cursor: true,
                ..TerminalKeyMode::default()
            },
            body,
        );
        assert_eq!(
            wire,
            b"\x1bOD\x1bOD\x7f\x7f\x7f\x1b[200~\xd6\xd0\xce\xc4\x1b[201~"
        );
        assert_eq!(logical, "\x1bOD\x1bOD\x7f\x7f\x7f中文".as_bytes());
    }

    #[test]
    fn kitty_replacement_encodes_backspace_press_and_release_without_polluting_prediction() {
        let input = TerminalInputState {
            value: "abc".into(),
            cursor: 3,
            ..TerminalInputState::default()
        };
        let plan = plan_edit(
            &input,
            0,
            EditIntent::Replace {
                range: 1..2,
                text: "X",
            },
        )
        .unwrap();
        let mode = TerminalKeyMode {
            application_cursor: true,
            kitty_keyboard_disambiguate: true,
            kitty_keyboard_report_event_types: true,
            kitty_keyboard_report_all_keys_as_esc: true,
            ..TerminalKeyMode::default()
        };
        let (wire, logical) = shell_edit_payload(&plan, mode, b"X".to_vec());
        assert_eq!(wire, b"\x1bOD\x1b[127;1:1u\x1b[127;1:3uX");
        assert_eq!(logical, b"\x1bOD\x7fX");
        assert_eq!(plan.predicted.value, "aXc");
    }
}
