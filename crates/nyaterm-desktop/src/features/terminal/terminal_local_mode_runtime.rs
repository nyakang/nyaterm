//! Keyboard-driven local selection mode runtime.
//!
//! Local Mode is a terminal-surface state, not a global modal context: the
//! existing shortcut interceptor and Terminal scope stay active, while plain
//! navigation keys are consumed here instead of being written to the PTY.

use crate::features::NyaTermApp;
use crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range;
use gpui::{Context, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
use nyaterm_terminal::local_mode::{
    LocalModeMotion, LocalModeViewport, local_mode_insertion_col, local_mode_scroll_delta,
    move_local_mode_cursor,
};
use nyaterm_terminal::selection::{TerminalBufferCellPos, TerminalSelection};
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

impl NyaTermApp {
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
            .map(|selection| selection.head.position)
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

        // Mouse selection head seeds the cursor, but Local Mode starts with a
        // clean selection so the first Shift+motion defines a fresh anchor.
        self.terminal.selection.selection = None;
        self.terminal.selection.click_anchor = None;
        self.terminal.selection.session_id = None;
        self.terminal.selection.text.invalidate();
        self.terminal.selection.source_history = None;
        self.terminal.selection.dragging = false;
        self.notify_terminal_selection_visual_only(&session_id, cx);

        self.terminal.local_mode.session_id = Some(session_id.clone());
        self.terminal.local_mode.cursor = cursor;
        self.terminal.local_mode.anchor = None;
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
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let shift = keystroke.modifiers.shift;
        let control = keystroke.modifiers.control;
        let alt = keystroke.modifiers.alt;
        let platform = keystroke.modifiers.platform;
        let function = keystroke.modifiers.function;

        if !control && !alt && !platform && !function {
            if key == "escape" || key == "i" {
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
            let row = line.saturating_sub(absolute_start);
            // Lines outside the current snapshot (e.g. a whole page away) have
            // no measurable content end yet. Return the full width so the
            // motion keeps the caret's column; it is clamped against the real
            // content end once `reveal` has scrolled the target line on screen.
            snapshot.row(row).map_or(snapshot.cols, row_content_end)
        };

        let current = self.terminal.local_mode.cursor;
        let moved = move_local_mode_cursor(current, motion, viewport, content_end);

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
        let next_content_end = next_snapshot.row(next_row).map_or(1, row_content_end);
        // End/BufferEnd may target a line outside the pre-scroll snapshot, so
        // their column was clamped against a placeholder. Recompute it against
        // the snapshot that now contains the target line.
        let next_col = match motion {
            LocalModeMotion::End | LocalModeMotion::BufferEnd => next_content_end,
            _ => moved.col.min(next_content_end),
        }
        .min(next_snapshot.cols);
        let next = TerminalBufferCellPos::new(moved.line, next_col);

        if shift {
            let anchor = self.terminal.local_mode.anchor.unwrap_or(current);
            self.terminal.local_mode.anchor = Some(anchor);
            let selection = TerminalSelection::from_range(anchor, next);
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
        Some(TerminalBufferCellPos::new(
            endpoint.position.line,
            local_mode_insertion_col(endpoint.position.col, endpoint.side, content_end),
        ))
    }

    fn apply_local_mode_selection(
        &mut self,
        session_id: &str,
        anchor: TerminalBufferCellPos,
        head: TerminalBufferCellPos,
        cx: &mut Context<Self>,
    ) {
        self.terminal.selection.selection = Some(TerminalSelection::from_range(anchor, head));
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
        let Some(head) = self.local_mode_insertion_point_at(&active_session, event.position, cx)
        else {
            return false;
        };
        let anchor = if event.modifiers.shift {
            self.terminal
                .local_mode
                .anchor
                .or_else(|| self.terminal.selection.selection.map(|s| s.anchor.position))
                .unwrap_or(self.terminal.local_mode.cursor)
        } else {
            head
        };
        self.terminal.local_mode.cursor = head;
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
        let Some(anchor) = self.terminal.local_mode.anchor else {
            return false;
        };
        let Some(head) = self.local_mode_insertion_point_at(&active_session, event.position, cx)
        else {
            return false;
        };
        self.terminal.selection.drag_pointer_position = Some(event.position);
        if self.terminal.selection.selection.map(|s| s.head.position) == Some(head) {
            return true;
        }
        self.terminal.local_mode.cursor = head;
        self.apply_local_mode_selection(&active_session, anchor, head, cx);
        self.sync_terminal_local_mode_cursor(&active_session, cx);
        true
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
        self.terminal.selection.dragging = false;
        self.terminal.selection.drag_pointer_position = None;
        self.stop_terminal_selection_autoscroll();
        if self.terminal.selection.selection.is_some() {
            self.ensure_terminal_selection_text(cx);
            self.notify_terminal_selection_visual_only(&active_session, cx);
        }
        cx.notify();
        true
    }
}
