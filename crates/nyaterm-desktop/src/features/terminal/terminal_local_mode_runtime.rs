//! Keyboard-driven local selection mode runtime.
//!
//! Local Mode is a terminal-surface state, not a global modal context: the
//! existing shortcut interceptor and Terminal scope stay active, while plain
//! navigation keys are consumed here instead of being written to the PTY.

use crate::features::NyaTermApp;
use crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range;
use gpui::{Context, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
use nyaterm_terminal::local_mode::{
    LocalModeMotion, LocalModeViewport, local_mode_cell_boundary, local_mode_insertion_col,
    local_mode_scroll_delta, local_mode_selection, move_local_mode_cursor,
};
use nyaterm_terminal::selection::{TerminalBufferCellPos, TerminalSelectionMode};
use rust_i18n::t;

/// Insertion-point column just after the last non-empty cell of a snapshot
/// row. Used both for keyboard End/BufferEnd and for mouse clicks so the caret
/// never lands in the blank margin past a line's content.
fn row_content_end(row: &nyaterm_terminal::TerminalSnapshotRow) -> usize {
    row.cells
        .iter()
        .rposition(|cell| !cell.text().is_empty())
        .map(|index| index + row.cells[index].width.max(1) as usize)
        .unwrap_or(0)
}

fn local_mode_endpoint_boundary(
    endpoint: nyaterm_terminal::selection::TerminalSelectionEndpoint,
) -> TerminalBufferCellPos {
    TerminalBufferCellPos::new(
        endpoint.position.line,
        endpoint.position.col.saturating_add(usize::from(
            endpoint.side == nyaterm_terminal::selection::TerminalCellSide::Right,
        )),
    )
}

impl NyaTermApp {
    pub(in crate::features) fn terminal_local_mode_session(&self) -> Option<&str> {
        self.terminal.local_mode.session_id.as_deref()
    }

    pub(in crate::features) fn toggle_terminal_local_mode(&mut self, cx: &mut Context<Self>) {
        let session_id = self.session.active_id_owned();
        if self
            .terminal
            .local_mode
            .is_active_for(session_id.as_deref())
        {
            self.exit_terminal_local_mode(cx);
            return;
        }
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            return;
        };
        self.enter_terminal_local_mode(session_id, cx);
    }

    fn enter_terminal_local_mode(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.exit_terminal_local_mode(cx);
        self.clear_terminal_composition(cx);
        let offset = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| view.scroll_offset)
            .unwrap_or(0);
        let snapshot = self.terminal_snapshot_for_session(Some(&session_id), offset);
        let (absolute_start, absolute_end) = terminal_snapshot_absolute_range(&snapshot);
        let last_line = absolute_end.saturating_sub(1).max(absolute_start);

        let cursor = self
            .terminal
            .selection
            .selection
            .filter(|_| self.terminal.selection.session_id.as_deref() == Some(session_id.as_str()))
            .map(|selection| local_mode_endpoint_boundary(selection.head))
            .filter(|position| position.line >= absolute_start && position.line <= last_line)
            .unwrap_or_else(|| {
                let row = last_line.saturating_sub(absolute_start);
                let col = snapshot
                    .row(row)
                    .and_then(|line| {
                        line.cells
                            .iter()
                            .position(|cell| !cell.text.trim().is_empty())
                    })
                    .unwrap_or(0);
                TerminalBufferCellPos::new(last_line, col)
            });

        self.terminal.selection.dragging = false;
        self.notify_terminal_selection_visual_only(&session_id, cx);

        self.terminal.local_mode.session_id = Some(session_id.clone());
        self.terminal.local_mode.cursor = cursor;
        self.terminal.local_mode.history = snapshot.history;
        self.terminal.local_mode.preferred_col = cursor.col;
        self.terminal.local_mode.pending_end = false;
        self.terminal.local_mode.anchor = self
            .terminal
            .selection
            .selection
            .filter(|_| self.terminal.selection.session_id.as_deref() == Some(session_id.as_str()))
            .map(|selection| local_mode_endpoint_boundary(selection.anchor));
        self.stop_terminal_selection_autoscroll();
        if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
            view.local_mode_scroll_locked = true;
        }
        self.sync_terminal_local_mode_cursor(&session_id, cx);
        self.set_terminal_status_if_changed(t!("terminalCtx.localModeStatus").to_string());
        cx.notify();
    }

    pub(in crate::features) fn exit_terminal_local_mode(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.terminal.local_mode.session_id.take() else {
            return;
        };
        self.terminal.local_mode.anchor = None;
        self.terminal.local_mode.pending_end = false;
        self.stop_terminal_selection_autoscroll();
        self.terminal.selection.dragging = false;
        self.terminal.selection.drag_pointer_position = None;
        if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
            view.local_mode_scroll_locked = false;
        }
        self.sync_terminal_local_mode_cursor(&session_id, cx);
        self.set_terminal_status_if_changed(t!("terminalCtx.localModeStopped").to_string());
        cx.notify();
    }

    pub(in crate::features) fn handle_terminal_local_mode_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session_id) = self.terminal.local_mode.session_id.clone() else {
            return false;
        };
        if self.session.active_id() != Some(session_id.as_str()) {
            self.exit_terminal_local_mode(cx);
            return false;
        }
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let shift = keystroke.modifiers.shift;
        let control = keystroke.modifiers.control;
        let alt = keystroke.modifiers.alt;
        let platform = keystroke.modifiers.platform;
        let function = keystroke.modifiers.function;

        if !control && !alt && !platform && !function {
            if key == "escape" {
                self.exit_terminal_local_mode(cx);
                return true;
            }
            if key == "enter" {
                self.copy_local_mode_selection(&session_id, cx);
                return true;
            }
        }
        if control && !alt && !platform && !function && !shift && (key == "c" || key == "insert") {
            self.copy_local_mode_selection(&session_id, cx);
            return true;
        }

        let motion = if control && !alt && !platform && !function && key == "home" {
            LocalModeMotion::BufferStart
        } else if control && !alt && !platform && !function && key == "end" {
            LocalModeMotion::BufferEnd
        } else if !control && !alt && !platform && !function {
            match key {
                "left" => LocalModeMotion::Left,
                "right" => LocalModeMotion::Right,
                "up" => LocalModeMotion::Up,
                "down" => LocalModeMotion::Down,
                "pageup" => LocalModeMotion::PageUp(1),
                "pagedown" => LocalModeMotion::PageDown(1),
                "home" => LocalModeMotion::Home,
                "end" => LocalModeMotion::End,
                _ => {
                    // Unbound plain keys are swallowed: Local Mode is a reader,
                    // never a keyboard forwarder to the remote shell.
                    return true;
                }
            }
        } else {
            // Leave bound shortcuts (copy, find, ...) to the normal dispatcher.
            return false;
        };

        let offset = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| view.scroll_offset)
            .unwrap_or(0);
        let snapshot = self.terminal_snapshot_for_session(Some(&session_id), offset);
        let (absolute_start, absolute_end) = terminal_snapshot_absolute_range(&snapshot);
        let page_rows = self.active_terminal_page_rows();
        // Navigate over the whole buffer, not just the current viewport. The
        // viewport's first/last line used to clamp PageUp/PageDown to the
        // visible window, so the caret never left it and the view never
        // followed. Using the full buffer lets the caret move out and makes
        // `reveal` scroll the viewport to keep it visible.
        let viewport = LocalModeViewport {
            first_line: 0,
            last_line: snapshot.total_rows.saturating_sub(1),
            page_rows,
            cols: snapshot.cols,
        };
        let content_end = |line: usize| {
            let row = line.checked_sub(absolute_start);
            // Lines outside the current snapshot (e.g. a whole page away) have
            // no measurable content end yet. Return the full width so the
            // motion keeps the caret's column; it is clamped against the real
            // content end once `reveal` has scrolled the target line on screen.
            row.and_then(|row| snapshot.row(row))
                .map_or(snapshot.cols, row_content_end)
        };

        let current = self.terminal.local_mode.cursor;
        let vertical = matches!(
            motion,
            LocalModeMotion::Up
                | LocalModeMotion::Down
                | LocalModeMotion::PageUp(_)
                | LocalModeMotion::PageDown(_)
        );
        let mut moved = move_local_mode_cursor(current, motion, viewport, content_end);
        if vertical {
            moved.col = self.terminal.local_mode.preferred_col.min(snapshot.cols);
        }
        self.terminal.local_mode.pending_end =
            matches!(motion, LocalModeMotion::End | LocalModeMotion::BufferEnd);

        // Page motions are full-screen navigations: scroll the viewport by the
        // exact number of lines the caret moved so the display follows the page
        // flip and the caret keeps its screen row. Other motions only get a
        // minimal reveal to pull the caret back inside the visible window.
        let follow_lines = match motion {
            LocalModeMotion::PageUp(_) | LocalModeMotion::PageDown(_) => {
                Some(current.line as i32 - moved.line as i32)
            }
            _ => None,
        };
        let scroll_delta =
            local_mode_scroll_delta(follow_lines, moved.line, absolute_start, absolute_end);
        self.reveal_terminal_local_mode_cursor(&session_id, scroll_delta, cx);
        let next_offset = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| view.scroll_offset)
            .unwrap_or(0);
        let next_snapshot = self.terminal_snapshot_for_session(Some(&session_id), next_offset);
        let (next_start, _) = terminal_snapshot_absolute_range(&next_snapshot);
        let next_row = moved.line.saturating_sub(next_start);
        let target_row = (moved.line >= next_start)
            .then(|| next_snapshot.row(next_row))
            .flatten();
        let next_content_end = target_row.map_or(moved.col, row_content_end);
        if target_row.is_some() {
            self.terminal.local_mode.pending_end = false;
        }
        // End/BufferEnd may target a line outside the pre-scroll snapshot, so
        // their column was clamped against a placeholder. Recompute it against
        // the snapshot that now contains the target line.
        let next_col = match motion {
            LocalModeMotion::End | LocalModeMotion::BufferEnd => next_content_end,
            _ => moved.col.min(next_content_end),
        }
        .min(next_snapshot.cols);
        let next_col = target_row.map_or(next_col, |row| {
            local_mode_cell_boundary(
                &row.cells,
                next_col,
                matches!(motion, LocalModeMotion::Right),
            )
        });
        let next = TerminalBufferCellPos::new(moved.line, next_col);

        if shift {
            let anchor = self.terminal.local_mode.anchor.unwrap_or(current);
            self.terminal.local_mode.anchor = Some(anchor);
            let selection = local_mode_selection(anchor, next);
            self.terminal.selection.selection = Some(selection);
            self.terminal.selection.click_anchor = None;
            self.terminal.selection.session_id = Some(session_id.clone());
            self.terminal.selection.dragging = false;
            self.terminal.selection.text.invalidate_preserving_copy();
            self.terminal.selection.source_history = Some(
                self.terminal
                    .view
                    .views
                    .get(&session_id)
                    .map(|view| view.history_state())
                    .unwrap_or(snapshot.history),
            );
            self.notify_terminal_selection_visual_only(&session_id, cx);
        } else {
            self.terminal.local_mode.anchor = None;
            self.clear_terminal_selection(cx);
        }

        if !vertical {
            self.terminal.local_mode.preferred_col = next.col;
        }
        self.terminal.local_mode.cursor = next;
        self.sync_terminal_local_mode_cursor(&session_id, cx);
        true
    }

    fn copy_local_mode_selection(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if self.terminal.selection.selection.is_some()
            && self.terminal.selection.session_id.as_deref() == Some(session_id)
        {
            self.copy_terminal_selection(cx);
            self.set_terminal_status_if_changed(t!("terminalCtx.localModeCopied").to_string());
        }
    }

    fn reveal_terminal_local_mode_cursor(
        &mut self,
        session_id: &str,
        scroll_delta: i32,
        cx: &mut Context<Self>,
    ) {
        if scroll_delta != 0 {
            self.scroll_terminal_by_for_session(Some(session_id), scroll_delta, cx);
        }
    }

    pub(in crate::features) fn refresh_terminal_local_mode(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal.local_mode.is_active_for(Some(session_id)) {
            return;
        }
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return;
        };
        let history = view.history_state();
        let offset = view.scroll_offset;
        let Some(change) = history.changes_since(self.terminal.local_mode.history) else {
            let snapshot = self.terminal_snapshot_for_session(Some(session_id), offset);
            let (start, end) = terminal_snapshot_absolute_range(&snapshot);
            self.terminal.local_mode.history = history;
            self.terminal.local_mode.cursor.line = self
                .terminal
                .local_mode
                .cursor
                .line
                .clamp(start, end.saturating_sub(1).max(start));
            self.terminal.local_mode.cursor.col =
                self.terminal.local_mode.cursor.col.min(snapshot.cols);
            self.terminal.local_mode.anchor = None;
            self.terminal.local_mode.pending_end = false;
            self.terminal.selection.selection = None;
            self.terminal.selection.text.invalidate();
            self.set_terminal_status_if_changed(t!("terminalCtx.readHistoryChanged").to_string());
            cx.notify();
            self.sync_terminal_local_mode_cursor(session_id, cx);
            return;
        };
        let position_evicted = change.evicted > self.terminal.local_mode.cursor.line
            || self
                .terminal
                .local_mode
                .anchor
                .is_some_and(|anchor| change.evicted > anchor.line);
        if position_evicted {
            self.set_terminal_status_if_changed(t!("terminalCtx.readHistoryChanged").to_string());
            cx.notify();
        }
        self.terminal.local_mode.history = history;
        self.terminal.local_mode.cursor.line = self
            .terminal
            .local_mode
            .cursor
            .line
            .saturating_sub(change.evicted);
        if let Some(anchor) = self.terminal.local_mode.anchor.as_mut() {
            anchor.line = anchor.line.saturating_sub(change.evicted);
        }
        let snapshot = self.terminal_snapshot_for_session(Some(session_id), offset);
        if snapshot.history != history {
            // A cached frame may still use coordinates from before eviction or reflow.
            // Keep the navigation intent until a frame from the current history arrives.
            self.sync_terminal_local_mode_cursor(session_id, cx);
            return;
        }
        let (start, _) = terminal_snapshot_absolute_range(&snapshot);
        let cursor = self.terminal.local_mode.cursor;
        if cursor.line >= start
            && let Some(row) = snapshot.row(cursor.line - start)
        {
            let end = row_content_end(row);
            let col = if self.terminal.local_mode.pending_end {
                end
            } else {
                cursor.col.min(end)
            };
            self.terminal.local_mode.pending_end = false;
            self.terminal.local_mode.cursor.col = local_mode_cell_boundary(&row.cells, col, false);
            if let Some(anchor) = self.terminal.local_mode.anchor
                && self.terminal.selection.selection.is_some_and(|selection| {
                    selection.mode == TerminalSelectionMode::Linear && !selection.all_buffer
                })
            {
                let next = local_mode_selection(anchor, self.terminal.local_mode.cursor);
                if self.terminal.selection.selection != Some(next) {
                    self.terminal.selection.selection = Some(next);
                    self.terminal.selection.text.invalidate_preserving_copy();
                }
                self.terminal.selection.source_history = Some(history);
            }
        }
        self.sync_terminal_local_mode_cursor(session_id, cx);
    }

    fn sync_terminal_local_mode_cursor(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let cursor = self
            .terminal
            .local_mode
            .is_active_for(Some(session_id))
            .then_some(self.terminal.local_mode.cursor);
        if let Some(surface) = self.terminal.view.surfaces.get(session_id).cloned() {
            surface.update(cx, |surface, cx| {
                if surface.set_local_mode_cursor(cursor) {
                    cx.notify();
                }
            });
        }
    }

    /// Text-editor style caret placement: the clicked half of a cell decides
    /// whether the caret goes before or after that character, and clicks past
    /// a line's content snap back to just after its last non-empty cell.
    fn local_mode_insertion_point_at(
        &self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<TerminalBufferCellPos> {
        let geometry = self.terminal_hit_test_geometry_for_session(Some(session_id), cx)?;
        let endpoint = self.terminal_selection_endpoint_in_current_history(
            position,
            &geometry,
            Some(session_id),
        )?;
        let (absolute_start, _) = terminal_snapshot_absolute_range(&geometry.snapshot);
        let row = endpoint.position.line.saturating_sub(absolute_start);
        let content_end = geometry
            .snapshot
            .row(row)
            .and_then(|line| {
                line.cells
                    .iter()
                    .rposition(|cell| !cell.text().is_empty())
                    .map(|index| index + (line.cells[index].width.max(1) as usize))
            })
            .unwrap_or(0);
        let col = local_mode_insertion_col(endpoint.position.col, endpoint.side, content_end);
        let col = geometry.snapshot.row(row).map_or(col, |line| {
            local_mode_cell_boundary(
                &line.cells,
                col,
                endpoint.side == nyaterm_terminal::selection::TerminalCellSide::Right,
            )
        });
        Some(TerminalBufferCellPos::new(endpoint.position.line, col))
    }

    fn apply_local_mode_selection(
        &mut self,
        session_id: &str,
        anchor: TerminalBufferCellPos,
        head: TerminalBufferCellPos,
        cx: &mut Context<Self>,
    ) {
        self.terminal.selection.selection = Some(local_mode_selection(anchor, head));
        self.terminal.selection.click_anchor = None;
        self.terminal.selection.session_id = Some(session_id.to_string());
        self.terminal.selection.dragging = true;
        self.terminal.selection.block_gesture = None;
        self.terminal.selection.text.invalidate_preserving_copy();
        self.terminal.selection.source_history = Some(
            self.terminal
                .view
                .views
                .get(session_id)
                .map(|view| view.history_state())
                .unwrap_or_default(),
        );
        self.queue_terminal_selection_drag_visual_notify(cx);
    }

    pub(in crate::features) fn handle_terminal_local_mode_mouse_down(
        &mut self,
        session_id: Option<&str>,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left
            || event.modifiers.alt
            || event.modifiers.control
            || event.modifiers.platform
        {
            return false;
        }
        let Some(active_session) = self.terminal.local_mode.session_id.clone() else {
            return false;
        };
        if session_id.filter(|id| !id.is_empty()) != Some(active_session.as_str()) {
            // Clicking another session hands that surface back to the normal
            // selection path and leaves Local Mode.
            self.exit_terminal_local_mode(cx);
            return false;
        }
        if event.click_count >= 2 {
            self.start_terminal_selection_for_session(Some(&active_session), event, cx);
            self.sync_local_mode_caret_from_selection(&active_session, cx);
            return true;
        }
        if let Some(surface) = self.terminal.view.surfaces.get(&active_session).cloned()
            && let Some(state) =
                surface.update(cx, |surface, _| surface.take_scroll_state_for_selection())
        {
            let _ = self.sync_terminal_local_scroll_visual_state_from_surface(state, cx);
        }
        let Some(head) = self.local_mode_insertion_point_at(&active_session, event.position, cx)
        else {
            return false;
        };
        let anchor = if event.modifiers.shift {
            self.terminal
                .local_mode
                .anchor
                .or_else(|| {
                    self.terminal
                        .selection
                        .selection
                        .map(|selection| local_mode_endpoint_boundary(selection.anchor))
                })
                .unwrap_or(self.terminal.local_mode.cursor)
        } else {
            head
        };
        self.terminal.local_mode.cursor = head;
        self.terminal.local_mode.preferred_col = head.col;
        self.terminal.local_mode.pending_end = false;
        self.terminal.local_mode.anchor = Some(anchor);
        self.clear_terminal_selection(cx);
        self.terminal.selection.session_id = Some(active_session.clone());
        self.terminal.selection.dragging = true;
        self.terminal.selection.drag_pointer_position = Some(event.position);
        if event.modifiers.shift && head != anchor {
            self.apply_local_mode_selection(&active_session, anchor, head, cx);
        }
        self.sync_terminal_local_mode_cursor(&active_session, cx);
        cx.notify();
        true
    }

    pub(in crate::features) fn update_terminal_local_mode_selection_drag(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(active_session) = self.terminal.local_mode.session_id.clone() else {
            return false;
        };
        if !self.terminal.selection.dragging
            || self.terminal.selection.session_id.as_deref() != Some(active_session.as_str())
        {
            return false;
        }
        if self
            .terminal
            .selection
            .selection
            .is_some_and(|s| s.mode == TerminalSelectionMode::Block)
        {
            return false;
        }
        if let Some(geometry) =
            self.terminal_hit_test_geometry_for_session(Some(&active_session), cx)
        {
            self.update_terminal_selection_autoscroll(
                active_session.clone(),
                event.position,
                &geometry,
                cx,
            );
        }
        self.terminal.selection.drag_pointer_position = Some(event.position);
        self.refresh_local_mode_selection_head(&active_session, event.position, cx);
        true
    }

    pub(in crate::features) fn refresh_local_mode_selection_head(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(geometry) = self.terminal_hit_test_geometry_for_session(Some(session_id), cx)
        else {
            return false;
        };
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return false;
        };
        if view.history_state() != geometry.snapshot.history
            || self.terminal_display_offset_for_session(Some(session_id)) != geometry.display_offset
        {
            return false;
        }
        let Some(anchor) = self.terminal.local_mode.anchor else {
            return false;
        };
        let Some(head) = self.local_mode_insertion_point_at(session_id, position, cx) else {
            return false;
        };
        let selection = local_mode_selection(anchor, head);
        if self.terminal.selection.selection != Some(selection) {
            self.terminal.local_mode.cursor = head;
            self.terminal.local_mode.preferred_col = head.col;
            self.terminal.local_mode.pending_end = false;
            self.apply_local_mode_selection(session_id, anchor, head, cx);
            self.sync_terminal_local_mode_cursor(session_id, cx);
        }
        true
    }

    pub(in crate::features) fn sync_local_mode_caret_from_selection(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal.local_mode.is_active_for(Some(session_id)) {
            return;
        }
        if let Some(selection) = self.terminal.selection.selection {
            self.terminal.local_mode.cursor = local_mode_endpoint_boundary(selection.head);
            self.terminal.local_mode.anchor = Some(local_mode_endpoint_boundary(selection.anchor));
            self.terminal.local_mode.preferred_col = self.terminal.local_mode.cursor.col;
            self.terminal.local_mode.pending_end = false;
            self.sync_terminal_local_mode_cursor(session_id, cx);
        }
    }

    pub(in crate::features) fn finish_terminal_local_mode_selection(
        &mut self,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        let Some(active_session) = self.terminal.local_mode.session_id.clone() else {
            return false;
        };
        if self.terminal.selection.session_id.as_deref() != Some(active_session.as_str()) {
            return false;
        }
        if self
            .terminal
            .selection
            .selection
            .is_some_and(|s| s.mode == TerminalSelectionMode::Block)
        {
            self.finish_terminal_selection(event, cx);
            self.sync_local_mode_caret_from_selection(&active_session, cx);
            return true;
        }
        if self.terminal.selection.dragging {
            self.refresh_local_mode_selection_head(&active_session, event.position, cx);
        }
        self.terminal.selection.dragging = false;
        self.terminal.selection.drag_pointer_position = None;
        self.terminal.selection.scroll_rehit_armed = false;
        self.stop_terminal_selection_autoscroll();
        if self
            .terminal
            .selection
            .selection
            .is_some_and(|s| s.is_empty())
        {
            self.clear_terminal_selection(cx);
        }
        self.complete_terminal_selection_reading(cx);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod regression_tests {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, KeyDownEvent, Keystroke, Modifiers, TestAppContext};

    fn pointer(
        app: &crate::features::NyaTermApp,
        col: f32,
        row: f32,
        cx: &mut gpui::Context<crate::features::NyaTermApp>,
    ) -> gpui::Point<gpui::Pixels> {
        let geometry = app
            .terminal_hit_test_geometry_for_session(Some("a"), cx)
            .unwrap();
        gpui::point(
            geometry.bounds.origin.x
                + gpui::px(geometry.padding_left + geometry.gutter + col * geometry.cell_w),
            geometry.bounds.origin.y
                + gpui::px(geometry.padding_top + geometry.visual_y_offset + row * geometry.cell_h),
        )
    }
    fn seed_reader(
        app: &mut crate::features::NyaTermApp,
        cx: &mut gpui::Context<crate::features::NyaTermApp>,
    ) {
        app.terminal.layout.surface_bounds = Some(gpui::Bounds {
            origin: gpui::Point::default(),
            size: gpui::size(gpui::px(800.), gpui::px(600.)),
        });
        app.terminal.seed_session_view(
            "a".into(),
            "first word\r\nsecond word\r\nthird".into(),
            "UTF-8",
        );
        app.enter_terminal_local_mode("a".into(), cx);
    }

    #[test]
    fn reading_mouse_selects_exact_boundaries_and_arms_then_stops_edge_scroll() {
        let dir = TestConfigDir::new("nyaterm-reading-drag");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            seed_reader(app, cx);
            let origin = pointer(app, 0.25, 0.5, cx);
            assert!(app.handle_terminal_local_mode_mouse_down(
                Some("a"),
                &gpui::MouseDownEvent {
                    position: origin,
                    button: gpui::MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                },
                cx
            ));
            let head = pointer(app, 1.25, 0.5, cx);
            assert!(app.update_terminal_local_mode_selection_drag(
                &gpui::MouseMoveEvent {
                    position: head,
                    ..Default::default()
                },
                cx
            ));
            assert_eq!(app.selected_terminal_text().as_deref(), Some("f"));
            let outside = gpui::point(head.x, gpui::px(-20.));
            app.update_terminal_local_mode_selection_drag(
                &gpui::MouseMoveEvent {
                    position: outside,
                    ..Default::default()
                },
                cx,
            );
            assert!(app.terminal.selection.autoscroll.is_some());
            let release = pointer(app, 2.25, 0.5, cx);
            app.finish_terminal_local_mode_selection(
                &gpui::MouseUpEvent {
                    position: release,
                    button: gpui::MouseButton::Left,
                    ..Default::default()
                },
                cx,
            );
            assert!(app.terminal.selection.autoscroll.is_none());
            assert!(!app.terminal.selection.dragging);
            assert_eq!(app.selected_terminal_text().as_deref(), Some("fi"));
        });
    }

    #[test]
    fn reading_wide_characters_and_cancelled_or_lost_release_preserve_copy_policy() {
        let dir = TestConfigDir::new("nyaterm-reading-wide-cancel");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            seed_reader(app, cx);
            app.terminal
                .seed_session_view("a".into(), "中😀文abc".into(), "UTF-8");
            let mut settings = app.settings.summary().clone();
            settings.interaction_copy_on_select = true;
            app.settings.replace_summary(settings);
            for cancel in [true, false] {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
                let origin = pointer(app, 0.25, 0.5, cx);
                app.handle_terminal_local_mode_mouse_down(
                    Some("a"),
                    &gpui::MouseDownEvent {
                        position: origin,
                        button: gpui::MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    },
                    cx,
                );
                let head = pointer(app, 4.25, 0.5, cx);
                app.update_terminal_local_mode_selection_drag(
                    &gpui::MouseMoveEvent {
                        position: head,
                        pressed_button: Some(gpui::MouseButton::Left),
                        ..Default::default()
                    },
                    cx,
                );
                assert_eq!(app.selected_terminal_text().as_deref(), Some("中😀"));
                if cancel {
                    app.cancel_terminal_selection_drag(cx);
                } else {
                    app.recover_terminal_selection_after_lost_mouse_up(cx);
                }
                assert!(!app.terminal.selection.dragging);
                assert!(app.terminal.selection.autoscroll.is_none());
                assert_eq!(
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .as_deref(),
                    Some(if cancel { "unchanged" } else { "中😀" })
                );
            }
        });
    }

    #[test]
    fn reading_block_selection_keeps_rectangular_range_through_cursor_refresh() {
        let dir = TestConfigDir::new("nyaterm-reading-block");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            seed_reader(app, cx);
            let origin = pointer(app, 1.25, 0.5, cx);
            let down = gpui::MouseDownEvent {
                position: origin,
                button: gpui::MouseButton::Left,
                click_count: 1,
                modifiers: Modifiers {
                    alt: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(!app.handle_terminal_local_mode_mouse_down(Some("a"), &down, cx));
            app.start_terminal_selection_for_session(Some("a"), &down, cx);
            let head = pointer(app, 4.25, 1.5, cx);
            let drag = gpui::MouseMoveEvent {
                position: head,
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers: down.modifiers,
            };
            assert!(!app.update_terminal_local_mode_selection_drag(&drag, cx));
            app.update_terminal_selection_drag(&drag, cx);
            app.sync_local_mode_caret_from_selection("a", cx);
            app.refresh_terminal_local_mode("a", cx);
            assert_eq!(
                app.terminal.selection.selection.unwrap().mode,
                nyaterm_terminal::selection::TerminalSelectionMode::Block
            );
            app.finish_terminal_local_mode_selection(
                &gpui::MouseUpEvent {
                    position: head,
                    button: gpui::MouseButton::Left,
                    modifiers: down.modifiers,
                    ..Default::default()
                },
                cx,
            );
            assert_eq!(app.selected_terminal_text().as_deref(), Some("irs\neco"));
        });
    }

    #[test]
    fn reading_double_click_uses_copy_preference_and_triple_click_selects_line() {
        let dir = TestConfigDir::new("nyaterm-reading-clicks");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            seed_reader(app, cx);
            let mut settings = app.settings.summary().clone();
            settings.interaction_copy_on_select = true;
            app.settings.replace_summary(settings);
            let position = pointer(app, 1.25, 0.5, cx);
            for count in [2, 3] {
                assert!(app.handle_terminal_local_mode_mouse_down(
                    Some("a"),
                    &gpui::MouseDownEvent {
                        position,
                        button: gpui::MouseButton::Left,
                        click_count: count,
                        ..Default::default()
                    },
                    cx
                ));
                app.finish_terminal_local_mode_selection(
                    &gpui::MouseUpEvent {
                        position,
                        button: gpui::MouseButton::Left,
                        ..Default::default()
                    },
                    cx,
                );
                let text = app.selected_terminal_text().unwrap();
                if count == 2 {
                    assert_eq!(text, "first");
                } else {
                    assert!(text.starts_with("first word"));
                }
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some(text)
                );
            }
        });
    }

    #[test]
    fn reading_tui_mouse_is_local_without_error_status_and_releases_protocol_on_exit() {
        use crate::features::terminal::terminal_runtime::TerminalMouseReportRequest;
        let dir = TestConfigDir::new("nyaterm-reading-tui");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            seed_reader(app, cx);
            let protocol = &mut app.terminal.view.views.get_mut("a").unwrap().protocol_state;
            protocol.mouse_reporting = true;
            protocol.mouse_motion_reporting = true;
            protocol.alternate_screen = true;
            protocol.alternate_scroll = true;
            let status = app.shell.status().to_string();
            assert!(app.terminal_local_scroll_enabled_for_session("a"));
            assert!(!app.maybe_send_mouse_report_for_session(
                TerminalMouseReportRequest {
                    session_id: "a",
                    button: 2,
                    col: 0,
                    row: 0,
                    press: true,
                    motion: false,
                    modifiers: Modifiers::default(),
                },
                cx
            ));
            assert!(!app.maybe_send_alternate_scroll_for_session("a", 1, cx));
            assert_eq!(app.shell.status(), status);
            app.exit_terminal_local_mode(cx);
            assert!(!app.terminal_local_scroll_enabled_for_session("a"));
        });
    }

    #[test]
    fn reading_session_rules_survive_surface_replacement_and_reconnect_but_not_close() {
        let dir = TestConfigDir::new("nyaterm-session-rule-lifetime");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            app.set_session_keyword_rules(
                "a",
                vec![nyaterm_core::KeywordHighlightRule {
                    id: "retained".into(),
                    ..Default::default()
                }],
                cx,
            );
            app.remove_terminal_surface("a");
            app.ensure_terminal_surface("a", cx);
            assert_eq!(app.session_keyword_rules("a").unwrap()[0].id, "retained");
            app.terminal.rekey_session_view("a", "reconnected", "UTF-8");
            assert_eq!(
                app.session_keyword_rules("reconnected").unwrap()[0].id,
                "retained"
            );
            assert!(app.session_keyword_rules("a").is_none());
            app.terminal.remove_frame_session("reconnected");
            app.terminal
                .seed_session_view("reconnected".into(), String::new(), "UTF-8");
            assert!(app.session_keyword_rules("reconnected").unwrap().is_empty());
        });
    }

    #[test]
    fn regression_reading_mode_blocks_composition_and_preserves_protection_on_history_reset() {
        use crate::features::test_support::session_focus_test_window;
        use gpui::EntityInputHandler as _;
        let dir = TestConfigDir::new("nyaterm-local-mode-ime");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        app.update(&mut cx, |app, cx| {
            app.enter_terminal_local_mode("a".into(), cx)
        });
        let vcx = session_focus_test_window(&mut cx, &app);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.replace_and_mark_text_in_range(None, "中文", None, window, cx);
                assert!(!app.terminal.input.ime.is_active_for(Some("a")));
                app.replace_text_in_range(None, "中文", window, cx);
                assert!(app.selected_text_range(false, window, cx).is_none());
                assert!(
                    app.write_session_wire_input_recorded_as("a", b"paste", b"paste")
                        .is_err()
                );
                assert!(
                    app.write_automated_terminal_input("a", b"command", None)
                        .is_err()
                );
                app.terminal.local_mode.history.epoch =
                    app.terminal.local_mode.history.epoch.wrapping_add(1);
                app.refresh_terminal_local_mode("a", cx);
                assert_eq!(app.terminal_local_mode_session(), Some("a"));
                assert!(app.ensure_terminal_input_allowed("a").is_err());
                app.exit_terminal_local_mode(cx);
                assert!(app.ensure_terminal_input_allowed("a").is_ok());
            })
        });
    }

    #[test]
    fn regression_session_activation_exits_previous_local_mode() {
        let dir = TestConfigDir::new("nyaterm-review-1342-switch");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        cx.update_entity(&app, |app, cx| {
            app.terminal
                .seed_session_view("b".into(), String::new(), "UTF-8");
            app.enter_terminal_local_mode("a".into(), cx);
            app.activate_session_id("b", cx);
            assert!(app.terminal.local_mode.session_id.is_none());
            assert!(!app.terminal.view.views["a"].local_mode_scroll_locked);
        });
    }

    #[test]
    fn regression_unbound_control_key_cannot_reach_input_boundary() {
        let dir = TestConfigDir::new("nyaterm-review-1342-control");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        cx.update_entity(&app, |app, cx| {
            app.enter_terminal_local_mode("a".into(), cx);
            let event = KeyDownEvent {
                keystroke: Keystroke {
                    modifiers: Modifiers {
                        control: true,
                        ..Default::default()
                    },
                    key: "d".into(),
                    key_char: Some("d".into()),
                },
                is_held: false,
                prefer_character_input: false,
            };
            assert!(!app.handle_terminal_local_mode_key(&event, cx));
            assert!(app.ensure_terminal_input_allowed("a").is_err());
            assert!(app.write_session_input_recorded("a", b"test").is_err());
            assert!(app.write_session_raw_input_recorded("a", b"\x04").is_err());
            assert!(app.write_session_sensitive_input("a", b"test").is_err());
        });
    }

    #[test]
    fn regression_reentering_on_another_session_releases_previous_scroll_lock() {
        let dir = TestConfigDir::new("nyaterm-review-1342-lock");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "a");
        cx.update_entity(&app, |app, cx| {
            app.terminal
                .seed_session_view("b".into(), String::new(), "UTF-8");
            app.enter_terminal_local_mode("a".into(), cx);
            app.activate_session_id("b", cx);
            app.toggle_terminal_local_mode(cx);
            app.exit_terminal_local_mode(cx);
            assert!(!app.terminal.view.views["a"].local_mode_scroll_locked);
        });
    }
}
