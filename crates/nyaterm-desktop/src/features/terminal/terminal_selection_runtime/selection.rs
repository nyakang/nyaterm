use std::time::Duration;

use gpui::{Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
use nyaterm_terminal::selection::{TerminalSelectionEndpoint, TerminalSelectionMode};
use nyaterm_terminal::{TerminalScreen, TerminalSnapshot};

use crate::features::NyaTermApp;
use crate::features::terminal::LostTerminalSelectionRecovery;
use crate::features::terminal::state::{
    TerminalBlockSelectionGesture, TerminalSelectionAutoscroll, TerminalSelectionTextStatus,
};
use crate::features::terminal::terminal_runtime::TerminalMouseReportRequest;
use crate::features::terminal::terminal_surface::{
    terminal_absolute_line_for_snapshot_row, terminal_snapshot_absolute_range,
};
use crate::models::{
    TerminalBufferCellPos, TerminalFrameSearchKey, TerminalFrameSearchPurpose, TerminalSelection,
    TerminalViewState,
};
use crate::terminal::{TerminalTextCell, terminal_is_zero_width_mark, terminal_text_cells};

use super::metrics::{
    TerminalHitTestGeometry, terminal_cell_for_visual_geometry,
    terminal_snapshot_row_for_visual_geometry,
};

#[cfg(test)]
mod block_tests;

const TERMINAL_SELECTION_DRAG_NOTIFY_DELAY: Duration = Duration::from_millis(8);
const TERMINAL_SELECTION_AUTOSCROLL_DELAY: Duration = Duration::from_millis(24);
const TERMINAL_SELECTED_OCCURRENCE_DEBOUNCE: Duration = Duration::from_millis(100);
const TERMINAL_SELECTED_OCCURRENCE_LIMIT: usize = 2000;
const TERMINAL_SELECTED_OCCURRENCE_MAX_CHARS: usize = 256;
const TERMINAL_SELECTION_IMMEDIATE_CELL_LIMIT: usize = 4096;

impl NyaTermApp {
    pub(in crate::features) fn clear_terminal_block_selection(&mut self, cx: &mut Context<Self>) {
        if self
            .terminal
            .selection
            .selection
            .is_some_and(|selection| selection.mode == TerminalSelectionMode::Block)
            || self.terminal.selection.block_gesture.is_some()
        {
            self.clear_terminal_selection(cx);
        }
    }

    pub(in crate::features) fn clear_terminal_selection_state_for_session(
        &mut self,
        session_id: &str,
    ) {
        // Occurrence results can outlive the visible selection while a session
        // is being closed; clear that owner independently of selection state.
        self.clear_terminal_selected_occurrence_for_session(session_id);
        let selection_session_id = self
            .terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id());
        if selection_session_id != Some(session_id) {
            return;
        }
        self.terminal.selection.selection = None;
        self.terminal.selection.click_anchor = None;
        self.terminal.selection.session_id = None;
        self.terminal.selection.text.invalidate();
        self.terminal.selection.source_history = None;
        self.terminal.selection.dragging = false;
        self.terminal.selection.block_gesture = None;
        self.terminal.selection.drag_pointer_position = None;
        self.terminal.selection.scroll_rehit_armed = false;
        self.stop_terminal_selection_autoscroll();
        self.clear_terminal_selected_occurrence_for_session(session_id);
    }

    fn notify_terminal_selection_owner_surface(&mut self, cx: &mut Context<Self>) {
        let session_id = self
            .terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id())
            .map(str::to_string);
        if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) {
            self.notify_terminal_selection_visual_only(session_id.as_str(), cx);
        }
    }

    pub(in crate::features) fn clear_terminal_selection(&mut self, cx: &mut Context<Self>) {
        let previous_session_id = self
            .terminal
            .selection
            .session_id
            .clone()
            .or_else(|| self.session.active_id_owned());
        if self.terminal.selection.selection.is_some()
            || self.terminal.selection.click_anchor.is_some()
            || self.terminal.selection.dragging
        {
            self.terminal.selection.selection = None;
            self.terminal.selection.click_anchor = None;
            self.terminal.selection.session_id = None;
            self.terminal.selection.text.invalidate();
            self.terminal.selection.source_history = None;
            self.terminal.selection.dragging = false;
            self.terminal.selection.block_gesture = None;
            self.terminal.selection.drag_pointer_position = None;
            self.terminal.selection.scroll_rehit_armed = false;
            self.stop_terminal_selection_autoscroll();
            self.clear_terminal_selected_occurrence(cx);
            if let Some(previous_session_id) =
                previous_session_id.filter(|session_id| !session_id.is_empty())
            {
                self.notify_terminal_selection_visual_only(previous_session_id.as_str(), cx);
            }
        }
    }

    pub(in crate::features) fn select_all_terminal(&mut self, cx: &mut Context<Self>) {
        let (_, cols) = self.active_terminal_grid_size();
        self.terminal.selection.click_anchor = None;
        if cols > 0 && self.terminal.selection.session_id.as_deref() == self.session.active_id() {
            self.terminal.selection.text.invalidate_preserving_copy();
        } else {
            self.terminal.selection.text.invalidate();
        }
        self.terminal.selection.source_history = None;
        self.terminal.selection.dragging = false;
        self.terminal.selection.block_gesture = None;
        self.terminal.selection.drag_pointer_position = None;
        self.terminal.selection.scroll_rehit_armed = false;
        self.stop_terminal_selection_autoscroll();
        if cols == 0 {
            self.terminal.selection.selection = None;
            self.terminal.selection.session_id = None;
            self.clear_terminal_selected_occurrence(cx);
            self.notify_terminal_selection_owner_surface(cx);
            return;
        }
        self.clear_terminal_selected_occurrence(cx);
        self.terminal.selection.selection = Some(TerminalSelection::all_buffer(cols));
        self.terminal.selection.session_id = self.session.active_id_owned();
        self.ensure_terminal_selection_text(cx);
        self.shell
            .set_status("selected all terminal text".to_string());
        self.notify_terminal_selection_owner_surface(cx);
        cx.notify();
    }

    pub(in crate::features) fn selected_terminal_text(&self) -> Option<String> {
        let selection = self.terminal.selection.selection.as_ref()?;
        let session_id = self
            .terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id());
        if selection.is_empty() {
            return None;
        }
        if self
            .terminal
            .selection
            .text
            .key
            .as_ref()
            .is_some_and(|(id, cached)| Some(id.as_str()) == session_id && cached == selection)
        {
            match &self.terminal.selection.text.status {
                TerminalSelectionTextStatus::Ready(text) => return text.clone(),
                TerminalSelectionTextStatus::Pending | TerminalSelectionTextStatus::Failed(_) => {
                    return None;
                }
                TerminalSelectionTextStatus::Unrequested => {}
            }
        }
        if selection.all_buffer {
            return None;
        }
        if let Some(view) =
            session_id.and_then(|session_id| self.terminal.view.views.get(session_id))
        {
            return terminal_selected_text_for_view(view, *selection);
        }
        terminal_selected_text_from_screen(*selection, &self.terminal.view.screen)
    }

    fn terminal_selection_endpoint_in_current_history(
        &self,
        position: gpui::Point<gpui::Pixels>,
        geometry: &TerminalHitTestGeometry,
        session_id: Option<&str>,
    ) -> Option<TerminalSelectionEndpoint> {
        let endpoint = Self::terminal_selection_endpoint_for_visual_geometry(position, geometry)?;
        let current = session_id
            .and_then(|id| self.terminal.view.views.get(id))
            .map(|view| view.history_state())
            .unwrap_or(geometry.snapshot.history);
        endpoint.after_eviction(current.changes_since(geometry.snapshot.history)?.evicted)
    }

    pub(in crate::features) fn start_terminal_selection_for_session(
        &mut self,
        session_id: Option<&str>,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        self.terminal.selection.block_gesture = None;
        self.terminal.selection.suppress_action_link_click = false;
        let selection_session_id = session_id
            .filter(|session_id| !session_id.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.session
                    .active_id_owned()
                    .filter(|session_id| !session_id.is_empty())
            });
        self.stop_terminal_selection_autoscroll();
        self.terminal.selection.scroll_rehit_armed = false;
        if let Some(session_id) = selection_session_id.as_deref()
            && let Some(surface) = self.terminal.view.surfaces.get(session_id).cloned()
            && let Some(state) =
                surface.update(cx, |surface, _| surface.take_scroll_state_for_selection())
        {
            let _ = self.sync_terminal_local_scroll_visual_state_from_surface(state, cx);
        }
        let Some(geometry) =
            self.terminal_hit_test_geometry_for_session(selection_session_id.as_deref(), cx)
        else {
            return;
        };
        if let Some(id) = selection_session_id.as_deref() {
            self.reanchor_terminal_selection_for_session(id, cx);
        }
        let previous_selection_session_id = self.terminal.selection.session_id.clone();
        if (self.terminal.selection.selection.is_some()
            || self.terminal.selection.click_anchor.is_some())
            && previous_selection_session_id.as_deref() != selection_session_id.as_deref()
        {
            self.terminal.selection.selection = None;
            self.terminal.selection.click_anchor = None;
            self.terminal.selection.session_id = None;
            self.terminal.selection.dragging = false;
            self.terminal.selection.block_gesture = None;
            if let Some(previous_selection_session_id) =
                previous_selection_session_id.filter(|session_id| !session_id.is_empty())
            {
                self.notify_terminal_selection_visual_only(
                    previous_selection_session_id.as_str(),
                    cx,
                );
            }
        }
        // A new selection invalidates the previous occurrence query immediately,
        // including while a double/triple-click selection is being formed.
        self.clear_terminal_selected_occurrence(cx);
        let cell = terminal_cell_for_visual_geometry(event.position, &geometry);
        let Some(endpoint) = self.terminal_selection_endpoint_in_current_history(
            event.position,
            &geometry,
            selection_session_id.as_deref(),
        ) else {
            return;
        };
        // Applications with mouse tracking (vim/less/tmux) consume left presses.
        if let Some(session_id) = selection_session_id.as_deref()
            && self.maybe_send_mouse_report_for_session(
                TerminalMouseReportRequest {
                    session_id,
                    button: 0,
                    col: cell.col as u16,
                    row: cell.row as u16,
                    press: true,
                    motion: false,
                    modifiers: event.modifiers,
                },
                cx,
            )
        {
            self.clear_terminal_selection(cx);
            self.terminal.selection.suppress_action_link_click = true;
            return;
        }
        if !event.modifiers.modified()
            && let Some(session_id) = selection_session_id.as_deref()
        {
            let snapshot_row = terminal_snapshot_row_for_visual_geometry(event.position, &geometry);
            let target_line = geometry
                .snapshot
                .row(snapshot_row)
                .and_then(|row| row.line_id);
            if let Some(target_line) = target_line
                && let Some(view) = self.terminal.view.views.get_mut(session_id)
            {
                view.target_line = Some(target_line);
                let absolute = geometry
                    .snapshot
                    .total_rows
                    .saturating_sub(geometry.snapshot.display_offset)
                    .saturating_sub(geometry.snapshot.row_count())
                    + snapshot_row;
                if let Some(block) = geometry
                    .snapshot
                    .command_blocks
                    .iter()
                    .find(|block| block.rows.contains(&absolute))
                {
                    view.command_blocks.selected = Some(block.id);
                }
                self.notify_terminal_surface_only(Some(session_id), cx);
            }
        }
        let cols = geometry.cols;
        self.terminal.selection.text.invalidate_preserving_copy();
        let block = event.modifiers.alt
            && self.settings.summary().terminal_wrap_mode
                != nyaterm_core::models::settings::TerminalWrapMode::None;
        if block || !event.modifiers.shift || self.terminal.selection.source_history.is_none() {
            self.terminal.selection.source_history = Some(
                selection_session_id
                    .as_deref()
                    .and_then(|id| self.terminal.view.views.get(id))
                    .map(|view| view.history_state())
                    .unwrap_or(geometry.snapshot.history),
            );
        }
        self.begin_smart_input_selection();
        let click_anchor = self.terminal.selection.click_anchor.take();
        if block {
            self.terminal.selection.selection = Some(TerminalSelection::with_anchor_and_mode(
                endpoint,
                TerminalSelectionMode::Block,
            ));
            self.terminal.selection.session_id = selection_session_id;
            self.terminal.selection.dragging = true;
            self.terminal.selection.block_gesture = Some(TerminalBlockSelectionGesture {
                origin: event.position,
                confirmed: false,
            });
            self.terminal.selection.drag_pointer_position = Some(event.position);
            self.notify_terminal_selection_owner_surface(cx);
            return;
        }
        if event.modifiers.shift
            && event.click_count <= 1
            && self.terminal.selection.selection.is_none()
        {
            self.terminal.selection.selection = click_anchor.map(TerminalSelection::with_anchor);
        }
        // Shift+click extends the existing selection from its anchor (xterm-style).
        if event.modifiers.shift
            && event.click_count <= 1
            && let Some(selection) = self.terminal.selection.selection.as_mut()
        {
            selection.all_buffer = false;
            selection.head = endpoint;
            self.terminal.selection.text.invalidate_preserving_copy();
            if self.terminal.selection.session_id.is_none() {
                self.terminal.selection.session_id = selection_session_id;
            }
            self.terminal.selection.dragging = true;
            self.terminal.selection.drag_pointer_position = Some(event.position);
            // Defer status-bar shell notify until selection finishes.
            self.notify_terminal_selection_owner_surface(cx);
            return;
        }
        if event.click_count >= 3 {
            let editable = if !event.modifiers.modified()
                && selection_session_id.as_deref() == self.session.active_id()
            {
                self.smart_input_line_selection_at_mouse(event.position, cx)
            } else {
                None
            };
            let logical_line = geometry.unwrapped.as_ref().and_then(|_| {
                let selection =
                    terminal_logical_selection_at_pointer(event.position, &geometry, None)?;
                selection.reanchored_history(
                    geometry.snapshot.history,
                    self.terminal.selection.source_history?,
                )
            });
            self.terminal.selection.selection = editable.or(logical_line).or_else(|| {
                Some(TerminalSelection::from_range(
                    TerminalBufferCellPos::new(endpoint.position.line, 0),
                    TerminalBufferCellPos::new(endpoint.position.line, cols.saturating_sub(1)),
                ))
            });
            self.terminal.selection.session_id = selection_session_id;
            self.terminal.selection.dragging = false;
            self.terminal.selection.block_gesture = None;
            self.shell
                .set_status(format!("selected line {}", cell.row + 1));
            self.notify_terminal_selection_owner_surface(cx);
            // Discrete click: status bar update is fine (not a high-frequency path).
            cx.notify();
            return;
        }
        if event.click_count == 2
            && geometry.unwrapped.is_some()
            && let Some(selection) = terminal_logical_selection_at_pointer(
                event.position,
                &geometry,
                Some(&self.settings.summary().interaction_word_separators),
            )
        {
            self.terminal.selection.selection =
                self.terminal.selection.source_history.and_then(|history| {
                    selection.reanchored_history(geometry.snapshot.history, history)
                });
            self.terminal.selection.session_id = selection_session_id;
            self.terminal.selection.dragging = false;
            self.terminal.selection.block_gesture = None;
            self.notify_terminal_selection_owner_surface(cx);
            cx.notify();
            return;
        }
        if event.click_count == 2 {
            let word = self.word_bounds_at_for_visual_geometry(event.position, &geometry);
            self.terminal.selection.selection = Some(TerminalSelection::from_range(
                TerminalBufferCellPos::new(endpoint.position.line, word.0),
                TerminalBufferCellPos::new(
                    endpoint.position.line,
                    word.1.saturating_sub(1).max(word.0),
                ),
            ));
            self.terminal.selection.session_id = selection_session_id;
            self.terminal.selection.dragging = false;
            self.terminal.selection.block_gesture = None;
            self.shell.set_status("selected word".to_string());
            self.notify_terminal_selection_owner_surface(cx);
            cx.notify();
            return;
        }
        self.terminal.selection.selection = Some(TerminalSelection::with_anchor(endpoint));
        self.terminal.selection.session_id = selection_session_id;
        self.terminal.selection.dragging = true;
        self.terminal.selection.drag_pointer_position = Some(event.position);
        self.notify_terminal_selection_owner_surface(cx);
    }

    fn clear_terminal_selected_occurrence_for_session(&mut self, session_id: &str) {
        if self
            .terminal
            .selection
            .selected_occurrence
            .session_id
            .as_deref()
            != Some(session_id)
        {
            return;
        }
        self.terminal.selection.selected_occurrence.session_id = None;
        self.terminal.selection.selected_occurrence.query = None;
        self.terminal.selection.selected_occurrence.generation = self
            .terminal
            .selection
            .selected_occurrence
            .generation
            .saturating_add(1);
        if let Some(view) = self.terminal.view.views.get_mut(session_id) {
            view.selected_occurrence_result = None;
            view.pending_selected_occurrence_key = None;
            view.selected_occurrence_visible_result = None;
            view.pending_selected_occurrence_visible_key = None;
        }
    }

    fn clear_terminal_selected_occurrence(&mut self, cx: &mut Context<Self>) {
        let session_id = self
            .terminal
            .selection
            .selected_occurrence
            .session_id
            .clone();
        self.terminal.selection.selected_occurrence.session_id = None;
        self.terminal.selection.selected_occurrence.query = None;
        self.terminal.selection.selected_occurrence.generation = self
            .terminal
            .selection
            .selected_occurrence
            .generation
            .saturating_add(1);
        if let Some(session_id) = session_id {
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                view.selected_occurrence_result = None;
                view.pending_selected_occurrence_key = None;
                view.selected_occurrence_visible_result = None;
                view.pending_selected_occurrence_visible_key = None;
            }
            self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
        }
    }

    pub(in crate::features) fn update_terminal_selection_drag(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if let Some(button) = self.terminal.selection.mouse_report_button {
            let captured_session_id = self
                .terminal
                .selection
                .mouse_report_session_id
                .clone()
                .or_else(|| self.session.active_id_owned());
            if let Some(session_id) = captured_session_id
                && let Some(cell) = self.point_to_terminal_cell_for_session(
                    Some(session_id.as_str()),
                    event.position,
                    cx,
                )
                && self.maybe_send_mouse_report_for_session(
                    TerminalMouseReportRequest {
                        session_id: &session_id,
                        button,
                        col: cell.col as u16,
                        row: cell.row as u16,
                        press: true,
                        motion: true,
                        modifiers: event.modifiers,
                    },
                    cx,
                )
            {
                return;
            }
        }
        if !self.terminal.selection.dragging {
            self.stop_terminal_selection_autoscroll();
            return;
        }
        if !self.terminal.selection.confirm_block_drag(event.position) {
            return;
        }
        self.terminal.selection.drag_pointer_position = Some(event.position);
        let selection_session_id = self
            .terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id())
            .filter(|session_id| !session_id.is_empty())
            .map(str::to_string);
        if let Some(id) = selection_session_id.as_deref() {
            self.reanchor_terminal_selection_for_session(id, cx);
        }
        let Some(geometry) =
            self.terminal_hit_test_geometry_for_session(selection_session_id.as_deref(), cx)
        else {
            self.stop_terminal_selection_autoscroll();
            return;
        };
        if let Some(session_id) = selection_session_id.as_deref() {
            self.update_terminal_selection_autoscroll(
                session_id.to_string(),
                event.position,
                &geometry,
                cx,
            );
        }
        if let Some(session_id) = selection_session_id.as_deref()
            && self
                .terminal
                .view
                .views
                .get(session_id)
                .map_or(geometry.display_offset, |view| {
                    view.history_state()
                        .reanchor_offset(geometry.snapshot.history, geometry.display_offset)
                })
                != self.terminal_display_offset_for_session(Some(session_id))
        {
            return;
        }
        let Some(endpoint) = self.terminal_selection_endpoint_in_current_history(
            event.position,
            &geometry,
            selection_session_id.as_deref(),
        ) else {
            return;
        };
        if let Some(selection) = self.terminal.selection.selection.as_mut()
            && selection.head != endpoint
        {
            selection.head = endpoint;
            self.terminal.selection.text.invalidate_preserving_copy();
            if self.terminal.selection.session_id.is_none() {
                self.terminal.selection.session_id = selection_session_id;
            }
            self.queue_terminal_selection_drag_visual_notify(cx);
        }
    }

    pub(in crate::features) fn finish_terminal_selection(
        &mut self,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        if !self.terminal.selection.confirm_block_drag(event.position) {
            self.clear_terminal_selection(cx);
            return;
        }
        self.terminal.selection.block_gesture = None;
        self.stop_terminal_selection_autoscroll();
        self.terminal.selection.scroll_rehit_armed = false;
        if let Some(id) = self.session.active_id_owned() {
            self.reconcile_shell_editing(&id, cx);
        }
        self.commit_smart_input_selection();
        if self.finish_terminal_mouse_report(event, cx) {
            self.clear_terminal_selection(cx);
            return;
        }
        if let Some(id) = self.terminal.selection.session_id.clone() {
            self.reanchor_terminal_selection_for_session(&id, cx);
        }
        if !self.terminal.selection.dragging {
            if self
                .terminal
                .selection
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.is_empty())
            {
                self.ensure_terminal_selection_text(cx);
                if self.settings.summary().interaction_copy_on_select {
                    let _ = self.copy_terminal_selection(cx);
                }
                // Double/triple-click selections are committed on MouseDown,
                // but occurrence search remains a MouseUp-only operation.
                self.schedule_terminal_selected_occurrence_search(cx);
                self.notify_terminal_selection_owner_surface(cx);
            }
            // Stationary click without an active drag can still reposition the
            // tracked input cursor (Tauri handleTerminalMouseUp smart cursor).
            if self.terminal.selection.selection.is_none() {
                self.handle_smart_input_click(event, cx);
            }
            return;
        }
        let selection_session_id = self
            .terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id())
            .filter(|session_id| !session_id.is_empty());
        if let Some(geometry) =
            self.terminal_hit_test_geometry_for_session(selection_session_id, cx)
            && selection_session_id
                .and_then(|id| self.terminal.view.views.get(id))
                .map_or(geometry.display_offset, |view| {
                    view.history_state()
                        .reanchor_offset(geometry.snapshot.history, geometry.display_offset)
                })
                == self.terminal_display_offset_for_session(selection_session_id)
        {
            let endpoint = self.terminal_selection_endpoint_in_current_history(
                event.position,
                &geometry,
                selection_session_id,
            );
            if let Some(selection) = self.terminal.selection.selection.as_mut()
                && let Some(endpoint) = endpoint
            {
                selection.head = endpoint;
            }
        }
        self.terminal.selection.dragging = false;
        self.terminal.selection.block_gesture = None;
        self.terminal.selection.drag_pointer_position = None;
        self.commit_smart_input_selection();
        if self
            .terminal
            .selection
            .selection
            .as_ref()
            .is_some_and(|selection| selection.is_empty())
        {
            let block = self
                .terminal
                .selection
                .selection
                .is_some_and(|selection| selection.mode == TerminalSelectionMode::Block);
            let click_anchor = self
                .terminal
                .selection
                .selection
                .take()
                .filter(|_| !block)
                .map(|selection| selection.anchor);
            let session_id = self.terminal.selection.session_id.clone();
            self.clear_terminal_selected_occurrence(cx);
            // Empty selection after click: try smart input cursor move.
            if !block {
                self.handle_smart_input_click(event, cx);
            }
            // Smart cursor movement may clear selection state. Restore only the
            // click anchor afterwards so Shift+click can extend without exposing
            // an empty selection to editing, context-menu or copy operations.
            self.terminal.selection.click_anchor = click_anchor;
            self.terminal.selection.session_id = session_id;
        } else if self.settings.summary().interaction_copy_on_select {
            let _ = self.copy_terminal_selection(cx);
        } else if self.terminal.selection.selection.is_some() {
            // One shell notify for status after drag ends (not per mouse move).
            self.shell.set_status("selection ready".to_string());
            cx.notify();
        }
        self.ensure_terminal_selection_text(cx);
        self.schedule_terminal_selected_occurrence_search(cx);
        self.notify_terminal_selection_owner_surface(cx);
    }

    pub(in crate::features) fn recover_terminal_selection_after_lost_mouse_up(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.end_terminal_selection_drag(true, cx);
    }

    pub(in crate::features) fn cancel_terminal_selection_drag(&mut self, cx: &mut Context<Self>) {
        // Keep the selected range, but cancellation must not copy to the clipboard.
        self.end_terminal_selection_drag(false, cx);
    }

    fn end_terminal_selection_drag(&mut self, copy_on_select: bool, cx: &mut Context<Self>) {
        self.stop_terminal_selection_autoscroll();
        let previous_session_id = self
            .terminal
            .selection
            .session_id
            .clone()
            .or_else(|| self.session.active_id_owned());
        match self.terminal.recover_lost_selection_mouse_up() {
            LostTerminalSelectionRecovery::None => {}
            LostTerminalSelectionRecovery::ClearedEmpty => {
                self.clear_terminal_selected_occurrence(cx);
                if let Some(session_id) =
                    previous_session_id.filter(|session_id| !session_id.is_empty())
                {
                    self.notify_terminal_selection_visual_only(session_id.as_str(), cx);
                }
            }
            LostTerminalSelectionRecovery::Committed => {
                self.ensure_terminal_selection_text(cx);
                if copy_on_select && self.settings.summary().interaction_copy_on_select {
                    let _ = self.copy_terminal_selection(cx);
                } else {
                    self.shell.set_status("selection ready".to_string());
                    cx.notify();
                }
                self.schedule_terminal_selected_occurrence_search(cx);
                self.notify_terminal_selection_owner_surface(cx);
            }
        }
    }

    pub(super) fn schedule_terminal_selected_occurrence_search(&mut self, cx: &mut Context<Self>) {
        if self
            .terminal
            .selection
            .selection
            .is_some_and(|selection| selection.mode == TerminalSelectionMode::Block)
        {
            self.clear_terminal_selected_occurrence(cx);
            return;
        }
        let session_id = self
            .terminal
            .selection
            .session_id
            .clone()
            .or_else(|| self.session.active_id_owned());
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            self.clear_terminal_selected_occurrence(cx);
            return;
        };
        let query = self
            .selected_terminal_text()
            .and_then(|text| terminal_selected_occurrence_query(&text));
        let Some(query) = query else {
            self.clear_terminal_selected_occurrence(cx);
            return;
        };
        self.terminal.selection.selected_occurrence.session_id = Some(session_id.clone());
        self.terminal.selection.selected_occurrence.query = Some(query.clone());
        self.terminal.selection.selected_occurrence.generation = self
            .terminal
            .selection
            .selected_occurrence
            .generation
            .saturating_add(1);
        let generation = self.terminal.selection.selected_occurrence.generation;
        let selection = self.terminal.selection.selection;
        let (absolute_start, absolute_end) = self
            .terminal
            .view
            .views
            .get(&session_id)
            .and_then(|view| {
                selection
                    .and_then(|selection| terminal_snapshot_covering_selection(view, selection))
                    .map(terminal_snapshot_absolute_range)
            })
            .or_else(|| {
                self.terminal
                    .view
                    .views
                    .get(&session_id)
                    .map(|view| view.screen.viewport_absolute_range(view.scroll_offset))
            })
            .unwrap_or((0, 0));
        let visible_key = TerminalFrameSearchKey {
            query: query.clone(),
            case_sensitive: true,
            regex: false,
            whole_word: false,
            limit: TERMINAL_SELECTED_OCCURRENCE_LIMIT,
            request_generation: generation,
        };
        let _ = self.request_terminal_frame_search(
            &session_id,
            TerminalFrameSearchPurpose::SelectedOccurrenceVisible {
                absolute_start,
                absolute_end,
            },
            visible_key,
        );
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SELECTED_OCCURRENCE_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, _cx| {
                if this.terminal.selection.selected_occurrence.generation != generation
                    || this
                        .terminal
                        .selection
                        .selected_occurrence
                        .session_id
                        .as_deref()
                        != Some(session_id.as_str())
                    || this.terminal.selection.selected_occurrence.query.as_deref()
                        != Some(query.as_str())
                {
                    return;
                }
                let key = TerminalFrameSearchKey {
                    query: query.clone(),
                    case_sensitive: true,
                    regex: false,
                    whole_word: false,
                    limit: TERMINAL_SELECTED_OCCURRENCE_LIMIT,
                    request_generation: generation,
                };
                let _ = this.request_terminal_frame_search(
                    &session_id,
                    TerminalFrameSearchPurpose::SelectedOccurrence,
                    key,
                );
            });
        })
        .detach();
    }

    pub(in crate::features) fn terminal_selection_dragging_for_session(
        &self,
        session_id: &str,
    ) -> bool {
        self.terminal.selection.dragging
            && self
                .terminal
                .selection
                .block_gesture
                .as_ref()
                .is_none_or(|gesture| gesture.confirmed)
            && self.terminal.selection.session_id.as_deref() == Some(session_id)
    }

    pub(in crate::features) fn stop_terminal_selection_autoscroll(&mut self) {
        if self.terminal.selection.autoscroll.take().is_some() {
            self.terminal.selection.autoscroll_generation = self
                .terminal
                .selection
                .autoscroll_generation
                .saturating_add(1);
        }
    }

    fn update_terminal_selection_autoscroll(
        &mut self,
        session_id: String,
        position: gpui::Point<gpui::Pixels>,
        geometry: &TerminalHitTestGeometry,
        cx: &mut Context<Self>,
    ) {
        let direction = terminal_selection_autoscroll_delta(
            f32::from(position.y),
            f32::from(geometry.bounds.origin.y),
            f32::from(geometry.bounds.origin.y + geometry.bounds.size.height),
            geometry.cell_h,
        );
        if direction == 0 {
            self.stop_terminal_selection_autoscroll();
            return;
        }
        if let Some(autoscroll) = self.terminal.selection.autoscroll.as_mut()
            && autoscroll.session_id == session_id
        {
            autoscroll.position = position;
            autoscroll.direction = direction;
            return;
        }
        self.stop_terminal_selection_autoscroll();
        self.terminal.selection.autoscroll_generation = self
            .terminal
            .selection
            .autoscroll_generation
            .saturating_add(1);
        let generation = self.terminal.selection.autoscroll_generation;
        self.terminal.selection.autoscroll = Some(TerminalSelectionAutoscroll {
            session_id,
            position,
            direction,
        });
        self.schedule_terminal_selection_autoscroll(generation, cx);
    }

    fn schedule_terminal_selection_autoscroll(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SELECTION_AUTOSCROLL_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.advance_terminal_selection_autoscroll(generation, cx);
            });
        })
        .detach();
    }

    fn advance_terminal_selection_autoscroll(&mut self, generation: u64, cx: &mut Context<Self>) {
        if generation != self.terminal.selection.autoscroll_generation {
            return;
        }
        let Some(autoscroll) = self.terminal.selection.autoscroll.clone() else {
            return;
        };
        if !self.terminal_selection_dragging_for_session(&autoscroll.session_id) {
            self.stop_terminal_selection_autoscroll();
            return;
        }
        self.refresh_terminal_selection_head_from_painted_geometry(
            &autoscroll.session_id,
            autoscroll.position,
            cx,
        );
        let Some(previous) = self.terminal_scroll_visual_state_for_session(&autoscroll.session_id)
        else {
            self.stop_terminal_selection_autoscroll();
            return;
        };
        let Some(next) = self.scroll_terminal_by_for_session_state_only(
            Some(&autoscroll.session_id),
            autoscroll.direction,
        ) else {
            self.stop_terminal_selection_autoscroll();
            return;
        };
        if previous.scroll_offset == next.scroll_offset {
            self.stop_terminal_selection_autoscroll();
            return;
        }
        self.reconcile_terminal_selection_after_scroll(
            &autoscroll.session_id,
            autoscroll.position,
            previous.scroll_offset,
            next.scroll_offset,
            cx,
        );
        self.notify_terminal_scroll_after_state_change(Some(&autoscroll.session_id), cx);
        self.schedule_terminal_selection_autoscroll(generation, cx);
    }

    fn refresh_terminal_selection_head_from_painted_geometry(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.reanchor_terminal_selection_for_session(session_id, cx);
        let Some(geometry) = self.terminal_hit_test_geometry_for_session(Some(session_id), cx)
        else {
            return false;
        };
        if self.terminal_display_offset_for_session(Some(session_id))
            != self
                .terminal
                .view
                .views
                .get(session_id)
                .map_or(geometry.display_offset, |view| {
                    view.history_state()
                        .reanchor_offset(geometry.snapshot.history, geometry.display_offset)
                })
        {
            return false;
        }
        if let Some(endpoint) = self.terminal_selection_endpoint_in_current_history(
            position,
            &geometry,
            Some(session_id),
        ) && let Some(selection) = self.terminal.selection.selection.as_mut()
            && selection.head != endpoint
        {
            selection.head = endpoint;
            self.terminal.selection.text.invalidate_preserving_copy();
            self.queue_terminal_selection_drag_visual_notify(cx);
        }
        true
    }

    pub(in crate::features) fn queue_terminal_selection_scroll_rehit(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_selection_dragging_for_session(session_id)
            || self.terminal.selection.scroll_rehit_armed
        {
            return;
        }
        self.terminal.selection.scroll_rehit_armed = true;
        let session_id = session_id.to_string();
        self.schedule_terminal_selection_scroll_rehit(session_id, 0, cx);
    }

    fn schedule_terminal_selection_scroll_rehit(
        &mut self,
        session_id: String,
        attempt: usize,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SELECTION_AUTOSCROLL_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.terminal_selection_dragging_for_session(&session_id) {
                    this.terminal.selection.scroll_rehit_armed = false;
                    return;
                }
                let painted =
                    this.terminal
                        .selection
                        .drag_pointer_position
                        .is_some_and(|position| {
                            this.refresh_terminal_selection_head_from_painted_geometry(
                                &session_id,
                                position,
                                cx,
                            )
                        });
                if !painted && attempt < 3 {
                    this.schedule_terminal_selection_scroll_rehit(session_id, attempt + 1, cx);
                } else {
                    this.terminal.selection.scroll_rehit_armed = false;
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn reconcile_terminal_selection_after_scroll(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        previous_offset: usize,
        next_offset: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_selection_dragging_for_session(session_id) {
            return;
        }
        let position = self
            .terminal
            .selection
            .drag_pointer_position
            .unwrap_or(position);
        self.terminal.selection.drag_pointer_position = Some(position);
        let Some(state) = self.terminal_scroll_visual_state_for_session(session_id) else {
            return;
        };
        let Some(selection) = self.terminal.selection.selection.as_mut() else {
            return;
        };
        let max_line = state
            .scrollback_len
            .saturating_add(state.viewport_rows)
            .saturating_sub(1);
        let line = terminal_selection_line_after_scroll(
            selection.head.position.line,
            previous_offset,
            next_offset,
            max_line,
        );
        if line != selection.head.position.line {
            selection.head.position.line = line;
            self.terminal.selection.text.invalidate_preserving_copy();
            self.queue_terminal_selection_drag_visual_notify(cx);
        }
    }

    fn queue_terminal_selection_drag_visual_notify(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self
            .terminal
            .selection
            .session_id
            .clone()
            .or_else(|| self.session.active_id_owned())
        else {
            return;
        };
        if session_id.is_empty() {
            return;
        }
        if !self.shell.queue_terminal_selection_drag(session_id) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SELECTION_DRAG_NOTIFY_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flush_terminal_selection_drag_visual_notify(cx);
            });
        })
        .detach();
    }

    fn flush_terminal_selection_drag_visual_notify(&mut self, cx: &mut Context<Self>) {
        let session_ids = self.shell.drain_terminal_selection_drag_sessions();
        for session_id in session_ids {
            self.notify_terminal_selection_visual_only(session_id.as_str(), cx);
        }
    }

    fn word_bounds_at_for_visual_geometry(
        &self,
        position: gpui::Point<gpui::Pixels>,
        geometry: &TerminalHitTestGeometry,
    ) -> (usize, usize) {
        terminal_word_bounds_for_visual_geometry(
            position,
            geometry,
            self.settings.summary().interaction_word_separators.as_str(),
        )
    }

    fn terminal_selection_endpoint_for_visual_geometry(
        position: gpui::Point<gpui::Pixels>,
        geometry: &TerminalHitTestGeometry,
    ) -> Option<TerminalSelectionEndpoint> {
        let top = f32::from(geometry.bounds.origin.y);
        let bottom = (top + f32::from(geometry.bounds.size.height) - 1.0).max(top);
        let position = gpui::Point {
            x: position.x,
            y: gpui::px(f32::from(position.y).clamp(top, bottom)),
        };
        let snapshot_row = terminal_snapshot_row_for_visual_geometry(position, geometry)
            .min(geometry.snapshot_rows.saturating_sub(1));
        let absolute_line =
            terminal_absolute_line_for_snapshot_row(geometry.snapshot.as_ref(), snapshot_row)?;
        let viewport_cell = terminal_cell_for_visual_geometry(position, geometry);
        Some(TerminalSelectionEndpoint::new(
            TerminalBufferCellPos::new(absolute_line, viewport_cell.col),
            nyaterm_terminal_gpui::geometry::terminal_cell_side_for_visual_geometry(
                position, geometry,
            ),
        ))
    }
}

fn terminal_selection_autoscroll_delta(
    pointer_y: f32,
    top: f32,
    bottom: f32,
    cell_height: f32,
) -> i32 {
    if !pointer_y.is_finite() || bottom <= top {
        return 0;
    }
    let edge = cell_height.max(1.0).min((bottom - top) / 4.0);
    let distance = if pointer_y < top + edge {
        top + edge - pointer_y
    } else if pointer_y >= bottom - edge {
        bottom - edge - pointer_y
    } else {
        return 0;
    };
    let steps = (1.0 + distance.abs() / cell_height.max(1.0)).floor() as i32;
    if distance > 0.0 {
        steps.clamp(1, 6)
    } else {
        -steps.clamp(1, 6)
    }
}

fn terminal_selection_line_after_scroll(
    line: usize,
    previous_offset: usize,
    next_offset: usize,
    max_line: usize,
) -> usize {
    if next_offset > previous_offset {
        line.saturating_sub(next_offset - previous_offset)
    } else {
        line.saturating_add(previous_offset - next_offset)
            .min(max_line)
    }
}

fn terminal_snapshot_covering_selection(
    view: &TerminalViewState,
    selection: TerminalSelection,
) -> Option<&TerminalSnapshot> {
    if selection.all_buffer || selection.is_empty() {
        return None;
    }
    view.frame_snapshot
        .iter()
        .chain(view.scrollback_snapshots.values())
        .map(AsRef::as_ref)
        .filter(|snapshot| {
            if !view
                .history_state()
                .preserves_coordinates_from(snapshot.history)
            {
                return false;
            }
            let (snapshot_start, snapshot_end) = terminal_snapshot_absolute_range(snapshot);
            selection
                .cell_range(snapshot.cols)
                .is_some_and(|(start, end)| snapshot_start <= start.line && end.line < snapshot_end)
        })
        .min_by_key(|snapshot| snapshot.display_offset.abs_diff(view.scroll_offset))
}

fn terminal_selected_text_for_view(
    view: &TerminalViewState,
    selection: TerminalSelection,
) -> Option<String> {
    if !selection.all_buffer
        && let Some(snapshot) = terminal_snapshot_covering_selection(view, selection)
    {
        let (absolute_start, _) = terminal_snapshot_absolute_range(snapshot);
        let (start, end) = selection.cell_range(snapshot.cols)?;
        // A logical line may span many physical rows. Bound extraction before
        // touching cells; larger selections are read on the frame worker.
        if end.line.saturating_sub(start.line).saturating_add(1)
            > TERMINAL_SELECTION_IMMEDIATE_CELL_LIMIT / snapshot.cols.max(1)
        {
            return None;
        }
        let mut text = String::new();
        for line in start.line..=end.line {
            let row = snapshot.row(line - absolute_start)?;
            selection.append_row_text(
                &mut text,
                &row.cells,
                line,
                row.wrapped,
                row.wraps_to_next,
            )?;
        }
        return (!text.is_empty()).then_some(text);
    }
    if view.frame_snapshot.is_some() || !view.scrollback_snapshots.is_empty() {
        return None;
    }
    terminal_selected_text_from_screen(selection, &view.screen)
}

fn terminal_selected_text_from_screen(
    selection: TerminalSelection,
    screen: &TerminalScreen,
) -> Option<String> {
    let rows = if selection.all_buffer {
        screen.scrollback_len().saturating_add(screen.rows())
    } else {
        let (start, end) = selection.cell_range(screen.cols())?;
        end.line.saturating_sub(start.line).saturating_add(1)
    };
    if rows > TERMINAL_SELECTION_IMMEDIATE_CELL_LIMIT / screen.cols().max(1) {
        return None;
    }
    screen.text_for_selection(selection).ok().flatten()
}

/// Select in logical columns, then retain source grid endpoints for copying.
fn terminal_logical_selection_at_pointer(
    position: gpui::Point<gpui::Pixels>,
    geometry: &TerminalHitTestGeometry,
    separators: Option<&str>,
) -> Option<TerminalSelection> {
    let snapshot = geometry.snapshot.as_ref();
    let index = snapshot.logical_lines.as_ref()?;
    let source_row = terminal_snapshot_row_for_visual_geometry(position, geometry);
    let col = terminal_cell_for_visual_geometry(position, geometry).col;
    let (absolute_start, _) = terminal_snapshot_absolute_range(snapshot);
    let physical = absolute_start + source_row;
    let logical = index.logical_row(physical);
    let group = index.group(physical);
    let width = index.logical_col(group.end - 1, snapshot.cols);
    let mut left = 0;
    let mut right = width;
    if let Some(separators) = separators {
        let is_word = |column| {
            let (row, mut col) = index.source_position(logical, column);
            let row = snapshot.row(row.checked_sub(absolute_start)?)?;
            if row.cells.get(col)?.width == 0 && col > 0 {
                col -= 1;
            }
            let ch = row.cells.get(col)?.text.chars().next()?;
            Some(!ch.is_whitespace() && !separators.contains(ch))
        };
        left = index.logical_col(physical, col);
        right = (left + 1).min(width);
        if is_word(left) == Some(true) {
            while left > 0 && is_word(left - 1) == Some(true) {
                left -= 1;
            }
            while right < width && is_word(right) == Some(true) {
                right += 1;
            }
        }
    }
    let (start_row, start_col) = index.source_position(logical, left);
    let (end_row, end_col) = index.source_position(logical, right.saturating_sub(1));
    Some(TerminalSelection::from_range(
        TerminalBufferCellPos::new(start_row, start_col),
        TerminalBufferCellPos::new(end_row, end_col),
    ))
}

fn terminal_word_bounds_for_visual_geometry(
    position: gpui::Point<gpui::Pixels>,
    geometry: &TerminalHitTestGeometry,
    separators: &str,
) -> (usize, usize) {
    let snapshot_row = terminal_snapshot_row_for_visual_geometry(position, geometry)
        .min(geometry.snapshot_rows.saturating_sub(1));
    let col = terminal_cell_for_visual_geometry(position, geometry).col;
    terminal_word_bounds_at_snapshot_row(col, geometry.snapshot.as_ref(), snapshot_row, separators)
}

fn terminal_word_bounds_at_snapshot_row(
    col: usize,
    snapshot: &TerminalSnapshot,
    snapshot_row: usize,
    separators: &str,
) -> (usize, usize) {
    let line = snapshot.line(snapshot_row).unwrap_or("");
    let cells = terminal_text_cells(line);
    if cells.is_empty() {
        return (col, col.saturating_add(1));
    }
    let idx = col.min(cells.len().saturating_sub(1));
    // xterm wordSeparator semantics: characters listed are separators, not word body.
    let is_word = |cell: &TerminalTextCell| terminal_text_cell_is_word(cell, separators);
    if !is_word(&cells[idx]) {
        return (idx, idx.saturating_add(1));
    }
    let mut start = idx;
    while start > 0 && is_word(&cells[start - 1]) {
        start -= 1;
    }
    let mut end = idx + 1;
    while end < cells.len() && is_word(&cells[end]) {
        end += 1;
    }
    (start, end)
}

fn terminal_text_cell_is_word(cell: &TerminalTextCell, separators: &str) -> bool {
    cell.text
        .chars()
        .find(|ch| !terminal_is_zero_width_mark(*ch))
        .is_some_and(|ch| !separators.contains(ch))
}

#[cfg(test)]
fn terminal_all_lines_text(lines: Vec<String>) -> Option<String> {
    let text = lines
        .into_iter()
        .map(|line| line.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim_end_matches('\n').to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn terminal_selected_occurrence_query(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let char_count = text.chars().count();
    if !(2..=TERMINAL_SELECTED_OCCURRENCE_MAX_CHARS).contains(&char_count) {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    use gpui::{
        AppContext as _, Bounds, ClipboardItem, Context, Modifiers, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, Point, Size, TestAppContext, px,
    };
    use nyaterm_terminal::TerminalScreen;
    use nyaterm_terminal::selection::{TerminalCellSide, TerminalSelectionEndpoint};

    use super::super::metrics::TerminalHitTestGeometry;
    use crate::features::NyaTermApp;
    use crate::features::terminal::terminal_surface::{
        terminal_absolute_line_for_snapshot_row, terminal_snapshot_absolute_range,
    };
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{TerminalBufferCellPos, TerminalSelection, TerminalViewState};
    use crate::terminal::{TerminalTextCell, terminal_text_cells};
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use nyaterm_terminal::terminal_text_cell_slice;

    use super::{
        TERMINAL_SELECTED_OCCURRENCE_MAX_CHARS, TERMINAL_SELECTION_DRAG_NOTIFY_DELAY,
        terminal_all_lines_text, terminal_selected_occurrence_query,
        terminal_selected_text_for_view, terminal_selection_autoscroll_delta,
        terminal_selection_line_after_scroll, terminal_snapshot_covering_selection,
        terminal_text_cell_is_word, terminal_word_bounds_for_visual_geometry,
    };

    #[test]
    fn mouse_endpoint_from_a_painted_history_frame_follows_eviction() {
        let dir = TestConfigDir::new("nyaterm-history-mouse-endpoint");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, _| {
            let mut screen = TerminalScreen::new(16, 3);
            screen.set_scrollback_limit(8);
            screen.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
            let snapshot = Arc::new(screen.viewport_snapshot(5));
            let geometry = TerminalHitTestGeometry {
                bounds: gpui::bounds(gpui::point(px(0.), px(0.)), gpui::size(px(128.), px(54.))),
                snapshot: snapshot.clone(),
                cell_w: 8.,
                cell_h: 18.,
                padding_left: 0.,
                padding_top: 0.,
                gutter: 0.,
                rows: 3,
                cols: 16,
                display_offset: 5,
                viewport_anchor_row: 0,
                snapshot_rows: 3,
                viewport_rows: 3,
                visual_y_offset: 0.,
                unwrapped: None,
            };
            let pointer = gpui::point(px(9.), px(9.));
            let old =
                NyaTermApp::terminal_selection_endpoint_for_visual_geometry(pointer, &geometry)
                    .unwrap();
            screen.advance(b"11\r\n12\r\n");
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .frame_snapshot = Some(Arc::new(screen.viewport_snapshot(0)));
            let current = app
                .terminal_selection_endpoint_in_current_history(pointer, &geometry, Some("s1"))
                .unwrap();
            assert_eq!(current.position.line, old.position.line - 2);
            assert_eq!(current.position.col, old.position.col);
            assert_eq!(current.side, old.side);
            screen.advance(b"13\r\n14\r\n");
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .frame_snapshot = Some(Arc::new(screen.viewport_snapshot(0)));
            assert!(
                app.terminal_selection_endpoint_in_current_history(pointer, &geometry, Some("s1"))
                    .is_none()
            );
        });
    }

    #[test]
    fn horizontal_hit_testing_selects_and_copies_words_across_soft_wraps() {
        use nyaterm_terminal_gpui::unwrapped::TerminalUnwrappedLayout;
        let mut screen = TerminalScreen::new(4, 3);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"abcdefgh\r\nlast");
        let snapshot = Arc::new(screen.snapshot());
        let layout = Arc::new(TerminalUnwrappedLayout::new(&snapshot, 0, 3, 4.25).unwrap());
        let geometry = TerminalHitTestGeometry {
            bounds: gpui::bounds(
                gpui::point(px(30.0), px(40.0)),
                gpui::size(px(32.0), px(54.0)),
            ),
            snapshot: snapshot.clone(),
            cell_w: 8.0,
            cell_h: 18.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 3,
            cols: 4,
            display_offset: 0,
            viewport_anchor_row: 0,
            snapshot_rows: snapshot.row_count(),
            viewport_rows: 3,
            visual_y_offset: 0.0,
            unwrapped: Some(layout),
        };
        let pointer = gpui::point(px(40.0), px(49.0));
        let cell = super::super::metrics::terminal_cell_for_visual_geometry(pointer, &geometry);
        assert_eq!(cell.col, 1);
        assert_eq!(
            super::super::metrics::terminal_snapshot_row_for_visual_geometry(pointer, &geometry),
            1
        );
        let word =
            super::terminal_logical_selection_at_pointer(pointer, &geometry, Some(" ")).unwrap();
        assert_eq!(
            screen.text_for_selection(word).unwrap().as_deref(),
            Some("abcdefgh")
        );
        let line = super::terminal_logical_selection_at_pointer(pointer, &geometry, None).unwrap();
        assert_eq!(
            screen.text_for_selection(line).unwrap().as_deref(),
            Some("abcdefgh")
        );
    }

    pub(super) fn click_terminal_cell(
        app: &mut NyaTermApp,
        session_id: &str,
        row: usize,
        col: usize,
        shift: bool,
        cx: &mut Context<NyaTermApp>,
    ) {
        click_terminal_cell_with_click_count(app, session_id, row, col, shift, 1, cx);
    }

    fn click_terminal_cell_with_click_count(
        app: &mut NyaTermApp,
        session_id: &str,
        row: usize,
        col: usize,
        shift: bool,
        click_count: usize,
        cx: &mut Context<NyaTermApp>,
    ) {
        let geometry = app
            .terminal_hit_test_geometry_for_session(Some(session_id), cx)
            .unwrap();
        let position = Point {
            x: geometry.bounds.origin.x
                + px(geometry.padding_left
                    + geometry.gutter
                    + (col as f32 + 0.75) * geometry.cell_w),
            y: geometry.bounds.origin.y
                + px(geometry.padding_top
                    + geometry.visual_y_offset
                    + (row as f32 + 0.5) * geometry.cell_h),
        };
        let modifiers = Modifiers {
            shift,
            ..Modifiers::default()
        };
        app.start_terminal_selection_for_session(
            Some(session_id),
            &MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers,
                click_count,
                ..MouseDownEvent::default()
            },
            cx,
        );
        app.finish_terminal_selection(
            &MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers,
                ..MouseUpEvent::default()
            },
            cx,
        );
    }

    pub(super) fn seed_click_selection_fixture(app: &mut NyaTermApp) {
        app.terminal.layout.surface_bounds = Some(Bounds {
            origin: Point::default(),
            size: Size {
                width: px(800.0),
                height: px(600.0),
            },
        });
        app.terminal
            .seed_session_view("s1".into(), "first\r\nsecond\r\nthird".into(), "UTF-8");
    }

    #[test]
    fn dragging_across_a_half_cell_updates_same_cell_and_mouse_up_boundary() {
        let dir = TestConfigDir::new("nyaterm-half-cell-drag");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            seed_click_selection_fixture(app);
            app.terminal
                .seed_session_view("s1".into(), "AB C".into(), "UTF-8");
            let geometry = app
                .terminal_hit_test_geometry_for_session(Some("s1"), cx)
                .unwrap();
            let point = |x: f32| Point {
                x: geometry.bounds.origin.x
                    + px(geometry.padding_left + geometry.gutter + x * geometry.cell_w),
                y: geometry.bounds.origin.y
                    + px(geometry.padding_top + geometry.visual_y_offset + 0.5 * geometry.cell_h),
            };
            app.start_terminal_selection_for_session(
                Some("s1"),
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(0.25),
                    click_count: 1,
                    ..MouseDownEvent::default()
                },
                cx,
            );
            app.update_terminal_selection_drag(
                &MouseMoveEvent {
                    position: point(0.75),
                    ..MouseMoveEvent::default()
                },
                cx,
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("A"));
            assert_eq!(
                app.terminal.selection.selection.unwrap().head.side,
                TerminalCellSide::Right
            );
            app.update_terminal_selection_drag(
                &MouseMoveEvent {
                    position: point(1.75),
                    ..MouseMoveEvent::default()
                },
                cx,
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("AB"));
            // The release event is authoritative even without another MouseMove.
            app.finish_terminal_selection(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(1.25),
                    ..MouseUpEvent::default()
                },
                cx,
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("A"));
            assert_eq!(
                app.terminal.selection.selection.unwrap().head.side,
                TerminalCellSide::Left
            );

            // Reverse the same boundaries; selection must still contain only A.
            app.start_terminal_selection_for_session(
                Some("s1"),
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(1.25),
                    click_count: 1,
                    ..MouseDownEvent::default()
                },
                cx,
            );
            app.finish_terminal_selection(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(0.25),
                    ..MouseUpEvent::default()
                },
                cx,
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("A"));
            // Adjacent right/left boundaries are empty and retain the original side.
            app.start_terminal_selection_for_session(
                Some("s1"),
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(0.75),
                    click_count: 1,
                    ..MouseDownEvent::default()
                },
                cx,
            );
            app.finish_terminal_selection(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(1.25),
                    ..MouseUpEvent::default()
                },
                cx,
            );
            assert!(app.terminal.selection.selection.is_none());
            assert_eq!(
                app.terminal.selection.click_anchor.unwrap().side,
                TerminalCellSide::Right
            );
        });
    }

    #[test]
    fn copy_uses_cell_sides_wide_glyphs_and_true_snapshot_columns() {
        let mut screen = TerminalScreen::new(8, 3);
        screen.advance("A界\u{301}😀B\r\nnext".as_bytes());
        let mut view = TerminalViewState::new();
        view.frame_snapshot = Some(Arc::new(screen.snapshot()));
        for (col, expected) in [
            (0, "A"),
            (1, "界\u{301}"),
            (2, "界\u{301}"),
            (3, "😀"),
            (4, "😀"),
            (5, "B"),
        ] {
            let selection = TerminalSelection::from_range(
                TerminalBufferCellPos::new(0, col),
                TerminalBufferCellPos::new(0, col),
            );
            assert_eq!(
                terminal_selected_text_for_view(&view, selection).as_deref(),
                Some(expected)
            );
        }
        let selection = TerminalSelection {
            anchor: TerminalBufferCellPos::new(0, 0).into(),
            head: TerminalBufferCellPos::new(1, 0).into(),
            all_buffer: false,
            mode: nyaterm_terminal::selection::TerminalSelectionMode::Linear,
        };
        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("A界\u{301}😀B")
        );
        let mut screen = TerminalScreen::new(4, 3);
        screen.advance("abc界x".as_bytes());
        view.frame_snapshot = Some(Arc::new(screen.snapshot()));
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 0),
            TerminalBufferCellPos::new(1, 2),
        );
        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("abc界x")
        );
    }

    #[test]
    fn shift_click_after_stationary_click_selects_multiple_lines_and_keeps_anchor() {
        let dir = TestConfigDir::new("nyaterm-shift-click-selection");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            seed_click_selection_fixture(app);
            click_terminal_cell(app, "s1", 1, 1, false, cx);
            assert!(app.terminal.selection.selection.is_none());
            assert!(app.selected_terminal_text().is_none());
            assert!(!app.terminal.selection.dragging);

            click_terminal_cell(app, "s1", 2, 2, true, cx);
            assert_eq!(
                app.terminal.selection.selection,
                Some(TerminalSelection {
                    anchor: TerminalSelectionEndpoint::new(
                        TerminalBufferCellPos::new(1, 1),
                        TerminalCellSide::Right
                    ),
                    head: TerminalSelectionEndpoint::new(
                        TerminalBufferCellPos::new(2, 2),
                        TerminalCellSide::Right
                    ),
                    all_buffer: false,
                    mode: nyaterm_terminal::selection::TerminalSelectionMode::Linear,
                }),
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("cond\nthi"));

            click_terminal_cell(app, "s1", 0, 0, true, cx);
            assert_eq!(
                app.terminal.selection.selection,
                Some(TerminalSelection {
                    anchor: TerminalSelectionEndpoint::new(
                        TerminalBufferCellPos::new(1, 1),
                        TerminalCellSide::Right
                    ),
                    head: TerminalSelectionEndpoint::new(
                        TerminalBufferCellPos::new(0, 0),
                        TerminalCellSide::Right
                    ),
                    all_buffer: false,
                    mode: nyaterm_terminal::selection::TerminalSelectionMode::Linear,
                }),
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("irst\nse"));
        });
    }

    #[test]
    fn shift_click_uses_latest_ordinary_click_anchor_and_respects_explicit_clear() {
        let dir = TestConfigDir::new("nyaterm-shift-click-reset");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            seed_click_selection_fixture(app);
            click_terminal_cell(app, "s1", 0, 0, false, cx);
            click_terminal_cell(app, "s1", 1, 1, false, cx);
            click_terminal_cell(app, "s1", 2, 2, true, cx);
            assert_eq!(
                app.terminal.selection.selection.unwrap().anchor.position,
                TerminalBufferCellPos::new(1, 1)
            );

            click_terminal_cell(app, "s1", 0, 0, false, cx);
            app.clear_terminal_selection(cx);
            click_terminal_cell(app, "s1", 2, 2, true, cx);
            assert!(app.terminal.selection.selection.is_none());
            assert!(app.selected_terminal_text().is_none());
        });
    }

    #[test]
    fn shift_click_does_not_reuse_another_sessions_click_anchor() {
        let dir = TestConfigDir::new("nyaterm-shift-click-session");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            seed_click_selection_fixture(app);
            app.terminal
                .seed_session_view("s2".into(), "other\r\nsession".into(), "UTF-8");
            click_terminal_cell(app, "s1", 0, 0, false, cx);
            click_terminal_cell(app, "s2", 1, 1, true, cx);
            assert!(app.terminal.selection.selection.is_none());
            assert_eq!(app.terminal.selection.session_id.as_deref(), Some("s2"));
            assert_eq!(
                app.terminal.selection.click_anchor,
                Some(TerminalSelectionEndpoint::new(
                    TerminalBufferCellPos::new(1, 1),
                    TerminalCellSide::Right
                ))
            );
        });
    }

    #[test]
    fn terminal_selection_drag_notify_delay_is_frame_coalesced() {
        assert_eq!(
            TERMINAL_SELECTION_DRAG_NOTIFY_DELAY,
            Duration::from_millis(8)
        );
    }

    #[test]
    fn dragging_below_terminal_advances_selection_toward_newer_lines() {
        assert_eq!(
            terminal_selection_autoscroll_delta(101.0, 0.0, 100.0, 10.0),
            -2
        );
        assert_eq!(terminal_selection_line_after_scroll(42, 8, 6, 99), 44);
        assert_eq!(terminal_selection_line_after_scroll(99, 2, 0, 99), 99);
    }

    #[test]
    fn dragging_above_terminal_advances_selection_into_scrollback() {
        assert_eq!(
            terminal_selection_autoscroll_delta(-1.0, 0.0, 100.0, 10.0),
            2
        );
        assert_eq!(terminal_selection_line_after_scroll(42, 6, 8, 99), 40);
        assert_eq!(terminal_selection_line_after_scroll(0, 98, 99, 99), 0);
    }

    #[test]
    fn wheel_scroll_during_drag_keeps_selection_endpoint_with_viewport() {
        assert_eq!(
            terminal_selection_autoscroll_delta(50.0, 0.0, 100.0, 10.0),
            0
        );
        assert_eq!(terminal_selection_line_after_scroll(40, 4, 7, 99), 37);
        assert_eq!(terminal_selection_line_after_scroll(37, 7, 5, 99), 39);
    }

    #[test]
    fn scrolling_a_drag_preserves_sides_and_rehit_uses_the_painted_half_cell() {
        let dir = TestConfigDir::new("nyaterm-half-cell-scroll");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            seed_click_selection_fixture(app);
            let anchor = TerminalSelectionEndpoint::new(
                TerminalBufferCellPos::new(0, 0),
                TerminalCellSide::Right,
            );
            let head = TerminalSelectionEndpoint::new(
                TerminalBufferCellPos::new(2, 1),
                TerminalCellSide::Right,
            );
            app.terminal.selection.selection = Some(TerminalSelection {
                anchor,
                head,
                all_buffer: false,
                mode: nyaterm_terminal::selection::TerminalSelectionMode::Linear,
            });
            app.terminal.selection.session_id = Some("s1".into());
            app.terminal.selection.dragging = true;
            let geometry = app
                .terminal_hit_test_geometry_for_session(Some("s1"), cx)
                .unwrap();
            let position = Point {
                x: geometry.bounds.origin.x
                    + px(geometry.padding_left + geometry.gutter + 1.25 * geometry.cell_w),
                y: geometry.bounds.origin.y
                    + px(geometry.padding_top + geometry.visual_y_offset + 1.5 * geometry.cell_h),
            };
            app.reconcile_terminal_selection_after_scroll("s1", position, 0, 1, cx);
            let selection = app.terminal.selection.selection.unwrap();
            assert_eq!(selection.anchor, anchor);
            assert_eq!(selection.head.position.line, 1);
            assert_eq!(selection.head.side, TerminalCellSide::Right);
            assert!(app.refresh_terminal_selection_head_from_painted_geometry("s1", position, cx));
            let selection = app.terminal.selection.selection.unwrap();
            assert_eq!(selection.anchor, anchor);
            assert_eq!(selection.head.position, TerminalBufferCellPos::new(1, 1));
            assert_eq!(selection.head.side, TerminalCellSide::Left);
        });
    }

    #[test]
    fn pointer_cancel_keeps_selection_without_copying_on_select() {
        let dir = TestConfigDir::new("nyaterm-selection-pointer-cancel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        app.update(&mut cx, |app, cx| {
            seed_click_selection_fixture(app);
            let mut settings = app.settings.summary().clone();
            settings.interaction_copy_on_select = true;
            app.settings.replace_summary(settings);
            let selection = TerminalSelection {
                anchor: TerminalSelectionEndpoint::new(
                    TerminalBufferCellPos::new(0, 0),
                    TerminalCellSide::Left,
                ),
                head: TerminalSelectionEndpoint::new(
                    TerminalBufferCellPos::new(0, 2),
                    TerminalCellSide::Right,
                ),
                all_buffer: false,
                mode: nyaterm_terminal::selection::TerminalSelectionMode::Linear,
            };
            app.terminal.selection.selection = Some(selection);
            app.terminal.selection.session_id = Some("s1".into());
            app.terminal.selection.dragging = true;
            cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
            app.cancel_terminal_selection_drag(cx);
            assert!(!app.terminal.selection.dragging);
            assert_eq!(app.terminal.selection.selection, Some(selection));
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("unchanged")
            );
        });
    }

    #[test]
    fn edge_drag_hit_test_stays_on_visible_rows_with_prefetched_snapshot() {
        let mut screen = TerminalScreen::new(20, 3);
        screen.advance(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let snapshot = Arc::new(screen.viewport_snapshot_with_window(0, 2, 0));
        assert!(snapshot.row_count() >= 5);
        let anchor = snapshot.row_count() - 3;
        let geometry = TerminalHitTestGeometry {
            unwrapped: None,
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(160.0),
                    height: px(48.0),
                },
            ),
            snapshot: snapshot.clone(),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 3,
            cols: snapshot.cols,
            display_offset: 0,
            viewport_anchor_row: anchor,
            snapshot_rows: snapshot.row_count(),
            viewport_rows: 3,
            visual_y_offset: -(anchor as f32) * 16.0,
        };
        let above = NyaTermApp::terminal_selection_endpoint_for_visual_geometry(
            Point {
                x: px(8.0),
                y: px(-100.0),
            },
            &geometry,
        );
        let below = NyaTermApp::terminal_selection_endpoint_for_visual_geometry(
            Point {
                x: px(8.0),
                y: px(100.0),
            },
            &geometry,
        );
        assert_eq!(
            above.map(|cell| cell.position.line),
            terminal_absolute_line_for_snapshot_row(&snapshot, anchor)
        );
        assert_eq!(
            below.map(|cell| cell.position.line),
            terminal_absolute_line_for_snapshot_row(&snapshot, anchor + 2)
        );
    }

    #[test]
    fn selected_text_uses_worker_snapshot_when_legacy_screen_has_no_output() {
        let mut worker_screen = TerminalScreen::new(40, 3);
        worker_screen
            .advance(b"IPv4 address for br0\r\nIPv6 address for br0\r\nIPv6 address for br1");
        let snapshot = Arc::new(worker_screen.viewport_snapshot(0));
        let (absolute_start, absolute_end) = terminal_snapshot_absolute_range(&snapshot);
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(absolute_start + 2, 5),
            TerminalBufferCellPos::new(absolute_start + 2, 11),
        );
        let mut view = TerminalViewState::new();
        view.frame_snapshot = Some(snapshot);

        assert!(
            !view
                .screen
                .all_lines()
                .iter()
                .any(|line| line.contains("address"))
        );
        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("address")
        );
        assert_eq!(
            terminal_snapshot_covering_selection(&view, selection)
                .map(terminal_snapshot_absolute_range),
            Some((absolute_start, absolute_end))
        );
    }

    #[test]
    fn selected_text_joins_soft_wrapped_snapshot_rows() {
        let mut screen = TerminalScreen::new(5, 3);
        screen.advance(b"abcdef");
        let mut view = TerminalViewState::new();
        view.frame_snapshot = Some(Arc::new(screen.snapshot()));
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 0),
            TerminalBufferCellPos::new(1, 0),
        );

        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("abcdef")
        );
    }

    #[test]
    fn selected_text_keeps_hard_line_breaks() {
        let mut screen = TerminalScreen::new(5, 3);
        screen.advance(b"abcde\r\nf");
        let mut view = TerminalViewState::new();
        view.frame_snapshot = Some(Arc::new(screen.snapshot()));
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 0),
            TerminalBufferCellPos::new(1, 0),
        );

        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("abcde\nf")
        );
    }

    #[test]
    fn select_all_joins_soft_wraps_and_keeps_hard_breaks_in_scrollback() {
        let mut view = TerminalViewState::new();
        view.screen = TerminalScreen::new(5, 2);
        view.screen.advance(b"abcdef\r\nghij");

        assert_eq!(
            terminal_selected_text_for_view(&view, TerminalSelection::all_buffer(5)).as_deref(),
            Some("abcdef\nghij")
        );
    }

    /// Dragging to the start of the row below a full line puts the endpoint on an
    /// empty row. That row contributes no characters, so the copy must not carry
    /// a trailing break, otherwise pasting a single line yields two lines.
    #[test]
    fn selected_text_omits_break_when_endpoint_row_is_empty() {
        let mut screen = TerminalScreen::new(5, 3);
        screen.advance(b"abc\r\n");
        let mut view = TerminalViewState::new();
        view.frame_snapshot = Some(Arc::new(screen.snapshot()));
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 0),
            TerminalBufferCellPos::new(1, 0),
        );

        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn selected_text_omits_break_when_endpoint_row_is_empty_on_legacy_screen() {
        let mut view = TerminalViewState::new();
        view.screen = TerminalScreen::new(5, 3);
        view.screen.advance(b"abc\r\n");
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 0),
            TerminalBufferCellPos::new(1, 0),
        );

        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn selected_text_keeps_break_when_empty_endpoint_row_extends_past_column_zero() {
        let mut screen = TerminalScreen::new(5, 3);
        screen.advance(b"abc\r\n");
        let mut view = TerminalViewState::new();
        view.frame_snapshot = Some(Arc::new(screen.snapshot()));
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 0),
            TerminalBufferCellPos::new(1, 1),
        );

        assert_eq!(
            terminal_selected_text_for_view(&view, selection).as_deref(),
            Some("abc\n")
        );
    }

    #[test]
    fn terminal_text_cells_keep_combining_mark_with_previous_cell() {
        let cells = terminal_text_cells("e\u{301}x");

        assert_eq!(
            cells,
            vec![
                TerminalTextCell {
                    text: "e\u{301}".to_string(),
                    byte_start: 0,
                    byte_end: "e\u{301}".len(),
                },
                TerminalTextCell {
                    text: "x".to_string(),
                    byte_start: "e\u{301}".len(),
                    byte_end: "e\u{301}x".len(),
                },
            ]
        );
        assert_eq!(terminal_text_cell_slice(&cells, 0, 1), "e\u{301}");
        assert_eq!(terminal_text_cell_slice(&cells, 1, 2), "x");
    }

    #[test]
    fn terminal_text_word_cells_use_base_character_for_separators() {
        let cells = terminal_text_cells("e\u{301}/x");

        assert!(terminal_text_cell_is_word(&cells[0], "/"));
        assert!(!terminal_text_cell_is_word(&cells[1], "/"));
        assert!(terminal_text_cell_is_word(&cells[2], "/"));
    }

    #[test]
    fn double_click_word_bounds_use_the_fractionally_painted_snapshot_row() {
        let mut screen = TerminalScreen::new(20, 3);
        screen.advance(b"wrong\r\ntarget word");
        let snapshot = Arc::new(screen.viewport_snapshot(0));
        let geometry = TerminalHitTestGeometry {
            unwrapped: None,
            bounds: gpui::bounds(
                Point {
                    x: px(0.0),
                    y: px(0.0),
                },
                Size {
                    width: px(160.0),
                    height: px(48.0),
                },
            ),
            snapshot: snapshot.clone(),
            cell_w: 8.0,
            cell_h: 16.0,
            padding_left: 0.0,
            padding_top: 0.0,
            gutter: 0.0,
            rows: 3,
            cols: snapshot.cols,
            display_offset: 0,
            viewport_anchor_row: 0,
            snapshot_rows: snapshot.row_count(),
            viewport_rows: 3,
            visual_y_offset: -8.0,
        };
        let position = Point {
            x: px(2.5 * geometry.cell_w),
            y: px(geometry.visual_y_offset + geometry.cell_h * 1.5),
        };

        assert_eq!(
            terminal_word_bounds_for_visual_geometry(position, &geometry, " /"),
            (0, 6)
        );
    }

    #[test]
    fn terminal_text_cells_count_wide_char_as_two_terminal_cells() {
        let cells = terminal_text_cells("界x");

        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].text, "界");
        assert_eq!(cells[1].text, "界");
        assert_eq!(cells[0].byte_start, cells[1].byte_start);
        assert_eq!(cells[0].byte_end, cells[1].byte_end);
        assert_eq!(terminal_text_cell_slice(&cells, 0, 1), "界");
        assert_eq!(terminal_text_cell_slice(&cells, 1, 2), "界");
        assert_eq!(terminal_text_cell_slice(&cells, 0, 2), "界");
        assert_eq!(terminal_text_cell_slice(&cells, 2, 3), "x");
    }

    #[test]
    fn terminal_text_cells_attach_combining_mark_to_all_wide_halves() {
        let text = "界\u{301}x";
        let cells = terminal_text_cells(text);

        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].text, "界\u{301}");
        assert_eq!(cells[1].text, "界\u{301}");
        assert_eq!(cells[0].byte_end, "界\u{301}".len());
        assert_eq!(cells[1].byte_end, "界\u{301}".len());
        assert_eq!(terminal_text_cell_slice(&cells, 0, 2), "界\u{301}");
        assert_eq!(terminal_text_cell_slice(&cells, 1, 2), "界\u{301}");
    }

    #[test]
    fn terminal_text_cells_attach_variation_selector_to_previous_cell() {
        let text = "a\u{fe0f}x";
        let cells = terminal_text_cells(text);

        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].text, "a\u{fe0f}");
        assert_eq!(cells[0].byte_end, "a\u{fe0f}".len());
        assert_eq!(terminal_text_cell_slice(&cells, 0, 1), "a\u{fe0f}");
        assert_eq!(terminal_text_cell_slice(&cells, 1, 2), "x");
    }

    #[test]
    fn terminal_all_lines_text_preserves_internal_blank_lines() {
        assert_eq!(
            terminal_all_lines_text(vec![
                "first  ".to_string(),
                String::new(),
                "last".to_string(),
                String::new(),
            ]),
            Some("first\n\nlast".to_string())
        );
    }

    #[test]
    fn terminal_selected_occurrence_query_filters_short_multiline_and_long_text() {
        assert_eq!(
            terminal_selected_occurrence_query(" ab "),
            Some("ab".to_string())
        );
        assert_eq!(terminal_selected_occurrence_query("a"), None);
        assert_eq!(terminal_selected_occurrence_query("a\nb"), None);
        assert_eq!(terminal_selected_occurrence_query("   "), None);
        assert_eq!(
            terminal_selected_occurrence_query(
                &"x".repeat(TERMINAL_SELECTED_OCCURRENCE_MAX_CHARS + 1)
            ),
            None
        );
    }

    /// Selection and copy must consume the gesture locally. Anything written
    /// here reaches the remote shell, where an unbound Ctrl+Insert shows up as
    /// stray `5~`.
    #[test]
    fn double_click_and_copy_write_no_session_bytes() {
        let dir = TestConfigDir::new("nyaterm-terminal-copy-zero-bytes");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        let manager = cx.update_entity(&app, |app, _| app.session.manager_handle());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let info = manager
            .create_telnet_session(nyaterm_transport::TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp: true,
                ..nyaterm_transport::TelnetSessionConfig::default()
            })
            .unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        cx.update_entity(&app, |app, _| {
            let metadata = app.session.metadata("s1").unwrap().clone();
            app.session.register_session_metadata(&info.id, metadata);
            app.session.select_active_session(&info.id);
            let mut settings = app.settings.summary().clone();
            settings.interaction_mouse_events_require_alt = false;
            app.settings.replace_summary(settings);
            app.terminal.layout.surface_bounds = Some(Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(800.0),
                    height: px(600.0),
                },
            });
            app.terminal
                .seed_session_view(info.id.clone(), "root@host:~# ls".into(), "UTF-8");
        });

        let read_sentinel = |app: &gpui::Entity<NyaTermApp>,
                             cx: &mut TestAppContext,
                             peer: &mut std::net::TcpStream,
                             sentinel: &[u8],
                             stage: &str| {
            cx.update_entity(app, |app, _| {
                app.write_session_protocol_response(&info.id, sentinel)
                    .unwrap();
            });
            let mut received = vec![0; sentinel.len()];
            peer.read_exact(&mut received).unwrap_or_else(|error| {
                panic!("{stage}: session received no sentinel bytes: {error}")
            });
            assert_eq!(
                received, sentinel,
                "{stage}: the gesture wrote {received:?} to the session before the sentinel"
            );
        };

        cx.update_entity(&app, |app, cx| {
            click_terminal_cell_with_click_count(app, &info.id, 0, 13, false, 1, cx);
            click_terminal_cell_with_click_count(app, &info.id, 0, 13, false, 2, cx);
            assert_eq!(app.selected_terminal_text().as_deref(), Some("ls"));
        });
        cx.run_until_parked();
        read_sentinel(&app, &mut cx, &mut peer, b"CLICK", "double click");

        // The copy action is the pure-local consumer of Ctrl+Insert.
        let copied = cx.update_entity(&app, |app, cx| {
            app.copy_terminal_selection_or_visible(cx);
            cx.read_from_clipboard().and_then(|item| item.text())
        });
        assert_eq!(copied.as_deref(), Some("ls"));
        cx.run_until_parked();
        read_sentinel(&app, &mut cx, &mut peer, b"COPY!", "copy");
        manager.close(&info.id).unwrap();
    }
}
