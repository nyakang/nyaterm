use std::time::{Duration, Instant};

use gpui::{Context, KeyDownEvent, KeyUpEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Window};
use nyaterm_core::{
    TerminalMouseReportEligibility, TerminalWireWriteKind, terminal_input_fanout_status,
    terminal_mouse_report_should_send, terminal_wire_write_disposition,
};

use crate::features::NyaTermApp;
use crate::terminal::{
    TerminalKeyMode, terminal_key_bytes_with_mode, terminal_key_release_bytes_with_mode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseReportWriteResult {
    NotHandled,
    Sent,
    Failed,
}

pub(super) fn terminal_mouse_report_button(button: u8) -> Option<MouseButton> {
    match button {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

pub(super) fn lost_mouse_report_release_button(
    captured_button: u8,
    pressed_button: Option<MouseButton>,
) -> Option<MouseButton> {
    let expected_button = terminal_mouse_report_button(captured_button)?;
    (pressed_button != Some(expected_button)).then_some(expected_button)
}

#[derive(Clone, Copy)]
pub(in crate::features) struct TerminalMouseReportRequest<'a> {
    pub session_id: &'a str,
    pub button: u8,
    pub col: u16,
    pub row: u16,
    pub press: bool,
    pub motion: bool,
    pub modifiers: gpui::Modifiers,
}

const TERMINAL_INPUT_SLOW_THRESHOLD: Duration = Duration::from_millis(12);

fn terminal_session_write_failure_safe_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x1b' => out.push_str("\\x1b"),
            ch if ch.is_control() => out.push_str(&format!("\\u{{{:x}}}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

pub(super) fn terminal_session_write_failure_log(context: &str, error: &str) -> String {
    let safe_error = terminal_session_write_failure_safe_text(error);
    format!("\n# session write failed ({context}): {safe_error}\n")
}

pub(super) fn terminal_should_track_command_suggestion_input(
    track_suggestions: bool,
    low_latency_mode: bool,
    command_suggestions_enabled: bool,
) -> bool {
    track_suggestions && !low_latency_mode && command_suggestions_enabled
}

impl NyaTermApp {
    pub(in crate::features) fn send_terminal_input(
        &mut self,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_terminal_input_with_options(bytes, true, cx)
    }

    pub(in crate::features) fn send_terminal_input_without_suggestion_track(
        &mut self,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_terminal_input_with_options(bytes, false, cx)
    }

    pub(in crate::features) fn send_terminal_input_with_options(
        &mut self,
        bytes: Vec<u8>,
        track_suggestions: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let input_started_at = Instant::now();
        if bytes.is_empty() {
            return false;
        }
        // Tauri/xterm custom key path: non-smart buffer selections stay painted while
        // typing. Smart input selections are handled earlier and clear themselves.
        // Only drop an in-progress drag so a stuck drag cannot block further input.
        if self.terminal.selection.dragging {
            self.terminal.selection.dragging = false;
            self.terminal.selection.drag_pointer_position = None;
            self.terminal.selection.scroll_rehit_armed = false;
            self.stop_terminal_selection_autoscroll();
        }
        let Some(session_id) = self.session.active_id_owned() else {
            if self.set_terminal_status_if_changed("start a session before typing") {
                cx.notify();
            }
            return false;
        };
        if self.session.is_disconnected(&session_id) {
            // Key handler owns Enter-to-reconnect (needs Window). Block writes here.
            if self
                .set_terminal_status_if_changed("session disconnected — press Enter to reconnect")
            {
                cx.notify();
            }
            return false;
        }
        // Typing while scrolled in history returns to the live bottom (xterm-like).
        if self.active_terminal_visual_scroll_active() {
            self.scroll_terminal_to_bottom(cx);
        }
        let peers = self.sync_peer_session_ids(&session_id);
        let byte_count = bytes.len();

        debug_assert!(
            terminal_wire_write_disposition(TerminalWireWriteKind::LogicalInput)
                .allow_command_history
        );
        self.terminal.view.frame_pipeline.arm_output_event_wake();
        // Primary + sync peers share write/record/history so recording and per-session
        // command history stay consistent. Resolve history once after all writes so a
        // pending Enter submission is applied to every successful peer.
        let mut ok_sessions = Vec::new();
        let write_started_at = Instant::now();
        match self.write_session_input_recorded(&session_id, &bytes) {
            Ok(()) => ok_sessions.push(session_id),
            Err(error) => {
                if self.set_terminal_status_if_changed(format!("input failed: {error}")) {
                    cx.notify();
                }
                return false;
            }
        }

        let mut synced = 0usize;
        let mut failed = 0usize;
        for peer_id in peers {
            match self.write_session_input_recorded(&peer_id, &bytes) {
                Ok(()) => {
                    ok_sessions.push(peer_id);
                    synced += 1;
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }
        let write_duration = write_started_at.elapsed();

        let input_wake_started_at = Instant::now();
        if !ok_sessions.is_empty() {
            self.arm_terminal_input_wake(cx);
        }
        let input_wake_duration = input_wake_started_at.elapsed();

        let suggestion_started_at = Instant::now();
        if track_suggestions {
            if terminal_should_track_command_suggestion_input(
                track_suggestions,
                self.settings.summary().terminal_low_latency_mode,
                self.settings
                    .summary()
                    .interaction_command_suggestions_enabled,
            ) {
                self.note_command_suggestion_input(&bytes, cx);
            } else {
                self.note_command_history_input(&bytes);
            }
        }
        let suggestion_duration = suggestion_started_at.elapsed();

        let session_refs: Vec<&str> = ok_sessions.iter().map(String::as_str).collect();
        let history_started_at = Instant::now();
        self.record_command_history_for_sessions(&session_refs, &bytes);
        let history_duration = history_started_at.elapsed();

        let should_notify = synced > 0 || failed > 0;
        let notify_started_at = Instant::now();
        if should_notify
            && self.set_terminal_status_if_changed(terminal_input_fanout_status(
                "sent", byte_count, synced, failed,
            ))
        {
            cx.notify();
        }
        let notify_duration = input_wake_duration + notify_started_at.elapsed();
        log_slow_terminal_input_diagnostic(
            "input_bytes",
            byte_count,
            synced,
            failed,
            input_started_at.elapsed(),
            Duration::ZERO,
            write_duration,
            suggestion_duration,
            history_duration,
            notify_duration,
        );
        failed == 0
    }

    pub(in crate::features) fn send_terminal_key_event(
        &mut self,
        event: &KeyDownEvent,
        track_suggestions: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let input_started_at = Instant::now();
        // Key protocol modes are session-local (application cursor/keypad,
        // Kitty keyboard). Re-encode for each sync peer instead of broadcasting
        // the active session's wire bytes.
        if self.terminal.selection.dragging {
            self.terminal.selection.dragging = false;
            self.terminal.selection.drag_pointer_position = None;
            self.terminal.selection.scroll_rehit_armed = false;
            self.stop_terminal_selection_autoscroll();
        }
        let Some(session_id) = self.session.active_id_owned() else {
            if self.set_terminal_status_if_changed("start a session before typing") {
                cx.notify();
            }
            return false;
        };
        if self.session.is_disconnected(&session_id) {
            if self
                .set_terminal_status_if_changed("session disconnected — press Enter to reconnect")
            {
                cx.notify();
            }
            return false;
        }
        let encode_started_at = Instant::now();
        let Some(primary_bytes) =
            self.terminal_key_bytes_for_event_for_session(Some(&session_id), event)
        else {
            return false;
        };
        if primary_bytes.is_empty() {
            return false;
        }
        let mut encode_duration = encode_started_at.elapsed();
        if self.active_terminal_visual_scroll_active() {
            self.scroll_terminal_to_bottom(cx);
        }

        debug_assert!(
            terminal_wire_write_disposition(TerminalWireWriteKind::LogicalInput)
                .allow_command_history
        );
        self.terminal.view.frame_pipeline.arm_output_event_wake();
        let byte_count = primary_bytes.len();
        let peers = self.sync_peer_session_ids(&session_id);
        let mut ok_sessions = Vec::new();
        let write_started_at = Instant::now();
        match self.write_session_input_recorded(&session_id, &primary_bytes) {
            Ok(()) => ok_sessions.push(session_id),
            Err(error) => {
                if self.set_terminal_status_if_changed(format!("input failed: {error}")) {
                    cx.notify();
                }
                return false;
            }
        }

        let mut synced = 0usize;
        let mut failed = 0usize;
        for peer_id in peers {
            let peer_encode_started_at = Instant::now();
            let Some(peer_bytes) =
                self.terminal_key_bytes_for_event_for_session(Some(&peer_id), event)
            else {
                encode_duration += peer_encode_started_at.elapsed();
                continue;
            };
            encode_duration += peer_encode_started_at.elapsed();
            if peer_bytes.is_empty() {
                continue;
            }
            match self.write_session_input_recorded(&peer_id, &peer_bytes) {
                Ok(()) => {
                    ok_sessions.push(peer_id);
                    synced += 1;
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }
        let write_duration = write_started_at.elapsed();

        let input_wake_started_at = Instant::now();
        if !ok_sessions.is_empty() {
            self.arm_terminal_input_wake(cx);
        }
        let input_wake_duration = input_wake_started_at.elapsed();

        let suggestion_started_at = Instant::now();
        if track_suggestions {
            if terminal_should_track_command_suggestion_input(
                track_suggestions,
                self.settings.summary().terminal_low_latency_mode,
                self.settings
                    .summary()
                    .interaction_command_suggestions_enabled,
            ) {
                self.note_command_suggestion_input(&primary_bytes, cx);
            } else {
                self.note_command_history_input(&primary_bytes);
            }
        }
        let suggestion_duration = suggestion_started_at.elapsed();

        let session_refs: Vec<&str> = ok_sessions.iter().map(String::as_str).collect();
        let history_started_at = Instant::now();
        self.record_command_history_for_sessions(&session_refs, &primary_bytes);
        let history_duration = history_started_at.elapsed();

        let should_notify = synced > 0 || failed > 0;
        let notify_started_at = Instant::now();
        if should_notify
            && self.set_terminal_status_if_changed(terminal_input_fanout_status(
                "sent", byte_count, synced, failed,
            ))
        {
            cx.notify();
        }
        let notify_duration = input_wake_duration + notify_started_at.elapsed();
        log_slow_terminal_input_diagnostic(
            "key_down",
            byte_count,
            synced,
            failed,
            input_started_at.elapsed(),
            encode_duration,
            write_duration,
            suggestion_duration,
            history_duration,
            notify_duration,
        );
        failed == 0
    }

    pub(in crate::features) fn send_terminal_key_release_event(
        &mut self,
        event: &KeyUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        if self.session.is_disconnected(&session_id) {
            return false;
        }
        let Some(primary_bytes) =
            self.terminal_key_release_bytes_for_event_for_session(Some(&session_id), event)
        else {
            return false;
        };
        if primary_bytes.is_empty() {
            return false;
        }

        let byte_count = primary_bytes.len();
        let peers = self.sync_peer_session_ids(&session_id);
        let mut synced = 0usize;
        let mut failed = 0usize;
        match self.write_session_input_recorded(&session_id, &primary_bytes) {
            Ok(()) => {}
            Err(error) => {
                if self.set_terminal_status_if_changed(format!("input failed: {error}")) {
                    cx.notify();
                }
                return false;
            }
        }
        for peer_id in peers {
            let Some(peer_bytes) =
                self.terminal_key_release_bytes_for_event_for_session(Some(&peer_id), event)
            else {
                continue;
            };
            if peer_bytes.is_empty() {
                continue;
            }
            match self.write_session_input_recorded(&peer_id, &peer_bytes) {
                Ok(()) => synced += 1,
                Err(_) => failed += 1,
            }
        }
        if (synced > 0 || failed > 0)
            && self.set_terminal_status_if_changed(terminal_input_fanout_status(
                "sent", byte_count, synced, failed,
            ))
        {
            cx.notify();
        }
        failed == 0
    }

    /// On the alternate screen with alternate-scroll enabled and no mouse
    /// tracking, emulate xterm: wheel becomes Up/Down cursor sequences.
    pub(in crate::features) fn maybe_send_alternate_scroll_for_session(
        &mut self,
        session_id: &str,
        delta_lines: i32,
        cx: &mut Context<Self>,
    ) -> bool {
        if delta_lines == 0 || session_id.is_empty() {
            return false;
        }
        if self.session.is_disconnected(session_id) {
            return false;
        }
        let Some(payload) = self.alternate_scroll_payload_for_session(session_id, delta_lines)
        else {
            return false;
        };
        if let Err(error) = self.write_session_input_recorded(session_id, &payload) {
            if self.set_terminal_status_if_changed(format!("alternate scroll failed: {error}")) {
                cx.notify();
            }
            return true;
        }

        let peers = self.sync_peer_session_ids(session_id);
        let mut synced = 0usize;
        let mut failed = 0usize;
        for peer_id in peers {
            let Some(peer_payload) =
                self.alternate_scroll_payload_for_session(&peer_id, delta_lines)
            else {
                continue;
            };
            match self.write_session_input_recorded(&peer_id, &peer_payload) {
                Ok(()) => synced += 1,
                Err(_) => failed += 1,
            }
        }
        if failed > 0 {
            self.shell.set_status(format!(
                "alternate scroll synced {synced} peer(s), {failed} failed"
            ));
            cx.notify();
        }
        true
    }

    fn alternate_scroll_payload_for_session(
        &self,
        session_id: &str,
        delta_lines: i32,
    ) -> Option<Vec<u8>> {
        if delta_lines == 0 || session_id.is_empty() || self.session.is_disconnected(session_id) {
            return None;
        }
        self.terminal_protocol_state_for_session(session_id)
            .alternate_scroll_payload(delta_lines)
    }

    /// When the active session's screen has mouse reporting enabled, encode and
    /// send a mouse report instead of performing local selection/scroll.
    /// Returns true when the terminal app handled the event (caller should skip
    /// local handling). Protocol traffic is recorded but not command history.
    pub(in crate::features) fn maybe_send_mouse_report_for_session(
        &mut self,
        report: TerminalMouseReportRequest<'_>,
        cx: &mut Context<Self>,
    ) -> bool {
        let TerminalMouseReportRequest {
            session_id,
            button,
            col,
            row,
            press,
            motion,
            modifiers,
        } = report;
        let result = self.write_mouse_report_to_session(report, cx);
        if !press && self.terminal.selection.mouse_report_button.is_some() {
            let peers = std::mem::take(&mut self.terminal.selection.mouse_report_peer_session_ids);
            for peer_id in peers {
                let _ = self.write_mouse_report_to_session(
                    TerminalMouseReportRequest {
                        session_id: &peer_id,
                        button,
                        col,
                        row,
                        press: false,
                        motion: false,
                        modifiers,
                    },
                    cx,
                );
            }
            self.clear_terminal_mouse_report_capture();
            return true;
        }
        match result {
            MouseReportWriteResult::NotHandled => return false,
            MouseReportWriteResult::Failed => return true,
            MouseReportWriteResult::Sent => {}
        }

        self.terminal.selection.mouse_report_position = Some((col, row));
        if motion {
            let peers = self
                .terminal
                .selection
                .mouse_report_peer_session_ids
                .clone();
            for peer_id in peers {
                let _ = self.write_mouse_report_to_session(
                    TerminalMouseReportRequest {
                        session_id: &peer_id,
                        button,
                        col,
                        row,
                        press,
                        motion: true,
                        modifiers,
                    },
                    cx,
                );
            }
            return true;
        }
        if press && button < 3 {
            let peers = self.sync_peer_session_ids(session_id);
            let mut captured_peers = Vec::new();
            for peer_id in peers {
                if self.write_mouse_report_to_session(
                    TerminalMouseReportRequest {
                        session_id: &peer_id,
                        button,
                        col,
                        row,
                        press: true,
                        motion: false,
                        modifiers,
                    },
                    cx,
                ) == MouseReportWriteResult::Sent
                {
                    captured_peers.push(peer_id);
                }
            }
            self.terminal.selection.mouse_report_button = Some(button);
            self.terminal.selection.mouse_report_session_id = Some(session_id.to_string());
            self.terminal.selection.mouse_report_peer_session_ids = captured_peers;
        }
        true
    }

    pub(in crate::features) fn maybe_send_terminal_any_motion_report(
        &mut self,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal.selection.mouse_report_button.is_some() {
            return false;
        }
        let Some(session_id) = self.terminal_session_at_point(event.position) else {
            return false;
        };
        let session_id = session_id
            .or_else(|| self.session.active_id_owned())
            .unwrap_or_default();
        if session_id.is_empty() {
            return false;
        }
        let protocol = self.terminal_protocol_state_for_session(&session_id);
        if !protocol.mouse_motion_reporting {
            return false;
        }
        let Some(cell) =
            self.point_to_terminal_cell_for_session(Some(session_id.as_str()), event.position, cx)
        else {
            return false;
        };
        let col = cell.col as u16;
        let row = cell.row as u16;
        match self.write_mouse_report_to_session(
            TerminalMouseReportRequest {
                session_id: &session_id,
                button: 3,
                col,
                row,
                press: true,
                motion: true,
                modifiers: event.modifiers,
            },
            cx,
        ) {
            MouseReportWriteResult::Sent => {
                for peer_id in self.sync_peer_session_ids(&session_id) {
                    let _ = self.write_mouse_report_to_session(
                        TerminalMouseReportRequest {
                            session_id: &peer_id,
                            button: 3,
                            col,
                            row,
                            press: true,
                            motion: true,
                            modifiers: event.modifiers,
                        },
                        cx,
                    );
                }
                true
            }
            MouseReportWriteResult::Failed => true,
            MouseReportWriteResult::NotHandled => false,
        }
    }

    fn write_mouse_report_to_session(
        &mut self,
        report: TerminalMouseReportRequest<'_>,
        cx: &mut Context<Self>,
    ) -> MouseReportWriteResult {
        let TerminalMouseReportRequest {
            session_id,
            button,
            col,
            row,
            press,
            motion,
            modifiers,
        } = report;
        if session_id.is_empty() {
            return MouseReportWriteResult::NotHandled;
        }
        let disconnected = self.session.is_disconnected(session_id);
        let protocol = self.terminal_protocol_state_for_session(session_id);
        if !protocol.mouse_reporting {
            return MouseReportWriteResult::NotHandled;
        }
        if !terminal_mouse_report_should_send(TerminalMouseReportEligibility {
            session_id_empty: false,
            disconnected,
            mouse_reporting: protocol.mouse_reporting,
            motion,
            mouse_drag_reporting: protocol.mouse_drag_reporting,
        }) {
            return MouseReportWriteResult::NotHandled;
        }
        let bytes = protocol.encode_mouse_report(
            button,
            col,
            row,
            press,
            motion,
            modifiers.shift,
            modifiers.alt,
            modifiers.control || modifiers.platform,
        );
        if bytes.is_empty() {
            return MouseReportWriteResult::NotHandled;
        }
        if let Err(error) = self.write_session_input_recorded(session_id, &bytes) {
            if self.set_terminal_status_if_changed(format!("mouse report failed: {error}")) {
                cx.notify();
            }
            return MouseReportWriteResult::Failed;
        }
        MouseReportWriteResult::Sent
    }

    pub(in crate::features) fn finish_terminal_mouse_report(
        &mut self,
        event: &gpui::MouseUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(button) = self.terminal.selection.mouse_report_button else {
            return false;
        };
        let Some(session_id) = self
            .terminal
            .selection
            .mouse_report_session_id
            .clone()
            .or_else(|| self.session.active_id_owned())
        else {
            self.clear_terminal_mouse_report_capture();
            return false;
        };
        let (col, row) = if let Some(cell) =
            self.point_to_terminal_cell_for_session(Some(session_id.as_str()), event.position, cx)
        {
            (cell.col as u16, cell.row as u16)
        } else if let Some((col, row)) = self.terminal.selection.mouse_report_position {
            (col, row)
        } else {
            self.clear_terminal_mouse_report_capture();
            return false;
        };
        self.maybe_send_mouse_report_for_session(
            TerminalMouseReportRequest {
                session_id: &session_id,
                button,
                col,
                row,
                press: false,
                motion: false,
                modifiers: event.modifiers,
            },
            cx,
        )
    }

    pub(in crate::features) fn recover_terminal_mouse_report_after_lost_mouse_up(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(captured_button) = self.terminal.selection.mouse_report_button else {
            return;
        };
        let Some(release_button) =
            lost_mouse_report_release_button(captured_button, event.pressed_button)
        else {
            if terminal_mouse_report_button(captured_button).is_none() {
                self.clear_terminal_mouse_report_capture();
            }
            return;
        };
        let release = MouseUpEvent {
            button: release_button,
            position: event.position,
            modifiers: event.modifiers,
            click_count: 1,
        };
        let _ = self.finish_terminal_mouse_report(&release, cx);
        // Protocol mode can change between press and recovery. A release that
        // is no longer encodable must still end the local capture.
        self.clear_terminal_mouse_report_capture();
    }

    pub(in crate::features) fn clear_terminal_mouse_report_for_session(
        &mut self,
        session_id: &str,
    ) {
        if self.terminal.selection.mouse_report_session_id.as_deref() == Some(session_id)
            || self
                .terminal
                .selection
                .mouse_report_peer_session_ids
                .iter()
                .any(|peer_id| peer_id == session_id)
        {
            self.clear_terminal_mouse_report_capture();
        }
    }

    fn clear_terminal_mouse_report_capture(&mut self) {
        self.terminal.selection.mouse_report_button = None;
        self.terminal.selection.mouse_report_session_id = None;
        self.terminal
            .selection
            .mouse_report_peer_session_ids
            .clear();
        self.terminal.selection.mouse_report_position = None;
    }

    /// Write UTF-8/ASCII input to a live session and mirror the logical input
    /// into the session recording buffer.
    /// Does not touch command history, status text, or UI notify.
    pub(in crate::features) fn write_session_input_recorded(
        &mut self,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.security.screen_locked() {
            return Err("screen locked".to_string());
        }
        // Charset-encode paste/typed text; pure ASCII CSI/mouse reports pass through.
        let disposition = terminal_wire_write_disposition(TerminalWireWriteKind::LogicalInput);
        let encoded = if disposition.encode_session_charset {
            self.encode_session_outgoing(session_id, bytes)
        } else {
            bytes.to_vec()
        };
        if let Err(error) = self
            .session
            .manager()
            .write(session_id, &encoded)
            .map_err(|error| error.to_string())
        {
            self.record_terminal_session_write_failure(session_id, "input", &error);
            return Err(error);
        }
        if disposition.record_logical_input {
            self.recording
                .write_input(session_id.to_string(), bytes.to_vec());
        }
        Ok(())
    }

    /// Write already-encoded/raw bytes to a live session and mirror the exact
    /// bytes into the session recording buffer.
    pub(in crate::features) fn write_session_raw_input_recorded(
        &mut self,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.security.screen_locked() {
            return Err("screen locked".to_string());
        }
        let disposition = terminal_wire_write_disposition(TerminalWireWriteKind::RawInput);
        debug_assert!(!disposition.encode_session_charset);
        if let Err(error) = self
            .session
            .manager()
            .write(session_id, bytes)
            .map_err(|error| error.to_string())
        {
            self.record_terminal_session_write_failure(session_id, "raw input", &error);
            return Err(error);
        }
        if disposition.record_raw_input {
            self.recording
                .write_raw_input(session_id.to_string(), bytes.to_vec());
        }
        Ok(())
    }

    /// Write secret user input without recording, history, suggestion tracking,
    /// or sync-input fanout. The payload still follows the session charset.
    pub(in crate::features) fn write_session_sensitive_input(
        &mut self,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.security.screen_locked() {
            return Err("screen locked".to_string());
        }
        let disposition = terminal_wire_write_disposition(TerminalWireWriteKind::SensitiveInput);
        debug_assert!(disposition.encode_session_charset);
        debug_assert!(!disposition.record_logical_input);
        debug_assert!(!disposition.record_raw_input);
        debug_assert!(!disposition.allow_command_history);
        let encoded = self.encode_session_outgoing(session_id, bytes);
        self.session
            .manager()
            .write(session_id, &encoded)
            .map_err(|error| error.to_string())
    }

    pub(in crate::features) fn send_sensitive_input_to_active_session(
        &mut self,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        if self.session.is_disconnected(&session_id) {
            return false;
        }
        let is_terminal = self
            .session
            .session_info(&session_id)
            .is_some_and(|session| {
                !matches!(
                    session.kind,
                    nyaterm_transport::SessionKind::Rdp | nyaterm_transport::SessionKind::Vnc
                )
            });
        if !is_terminal {
            return false;
        }
        self.terminal.view.frame_pipeline.arm_output_event_wake();
        match self.write_session_sensitive_input(&session_id, &bytes) {
            Ok(()) => {
                self.arm_terminal_input_wake(cx);
                true
            }
            Err(error) => {
                self.security
                    .set_status(format!("sensitive terminal input failed: {error}"));
                cx.notify();
                false
            }
        }
    }

    /// Write bytes already prepared for the PTY while recording a separate
    /// logical input stream. Used when protocol framing (for example bracketed
    /// paste) should reach the session but not pollute input recordings.
    pub(in crate::features) fn write_session_wire_input_recorded_as(
        &mut self,
        session_id: &str,
        wire_bytes: &[u8],
        recording_bytes: &[u8],
    ) -> Result<(), String> {
        if self.security.screen_locked() {
            return Err("screen locked".to_string());
        }
        let disposition = terminal_wire_write_disposition(TerminalWireWriteKind::FramedInput);
        debug_assert!(!disposition.encode_session_charset);
        if let Err(error) = self
            .session
            .manager()
            .write(session_id, wire_bytes)
            .map_err(|error| error.to_string())
        {
            self.record_terminal_session_write_failure(session_id, "framed input", &error);
            return Err(error);
        }
        if disposition.record_logical_input {
            self.recording
                .write_input(session_id.to_string(), recording_bytes.to_vec());
        }
        Ok(())
    }

    /// Write terminal-emulator protocol bytes (DSR/OSC/Kitty replies, focus
    /// reports) without marking them as user input in recordings.
    pub(in crate::features) fn write_session_protocol_response(
        &mut self,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let disposition = terminal_wire_write_disposition(TerminalWireWriteKind::ProtocolResponse);
        debug_assert!(!disposition.encode_session_charset);
        debug_assert!(!disposition.record_logical_input);
        debug_assert!(!disposition.record_raw_input);
        debug_assert!(!disposition.allow_command_history);
        if let Err(error) = self
            .session
            .manager()
            .write(session_id, bytes)
            .map_err(|error| error.to_string())
        {
            self.record_terminal_session_write_failure(session_id, "protocol response", &error);
            return Err(error);
        }
        Ok(())
    }

    fn record_terminal_session_write_failure(
        &mut self,
        session_id: &str,
        context: &str,
        error: &str,
    ) {
        let safe_error = terminal_session_write_failure_safe_text(error);
        tracing::warn!(
            diagnostic = "session_write_failed",
            session_id = %session_id,
            context,
            error = %safe_error,
            "terminal session write failed"
        );
        if session_id.is_empty() {
            return;
        }
        let log = terminal_session_write_failure_log(context, error);
        self.recording
            .write_output(session_id.to_string(), log.clone());
        self.append_terminal_log_for_session(Some(session_id), &log, true);
    }

    /// Encode UTF-8 host input for the session wire charset (UTF-8/GBK/…).
    pub(in crate::features) fn encode_session_outgoing(
        &self,
        session_id: &str,
        bytes: &[u8],
    ) -> Vec<u8> {
        if let Some(view) = self.terminal.view.views.get(session_id) {
            return view.screen.encode_outgoing(bytes);
        }
        self.terminal.view.screen.encode_outgoing(bytes)
    }

    /// Keep all live terminal screens on the current interaction encoding.
    pub(in crate::features) fn sync_terminal_encodings_from_settings(&mut self) {
        let label = self.settings.summary().interaction_default_encoding.clone();
        self.terminal.view.screen.set_encoding(&label);
        self.terminal.view.output_decoder.set_encoding(&label);
        let session_encodings = self
            .session
            .metadata_entries()
            .filter_map(|(session_id, metadata)| {
                metadata
                    .launch_config
                    .encoding()
                    .map(|encoding| (session_id.to_string(), encoding.to_string()))
            })
            .collect::<std::collections::HashMap<_, _>>();
        for (session_id, view) in &mut self.terminal.view.views {
            let encoding = session_encodings
                .get(session_id)
                .map(String::as_str)
                .unwrap_or(label.as_str());
            view.set_encoding(encoding);
        }
        self.sync_session_event_bridge_config();
    }

    pub(in crate::features) fn ensure_terminal_focus_reporting(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal.input.focus_subscriptions.is_empty() {
            return;
        }
        let focus_in = cx.on_focus_in(&self.terminal.input.focus, window, |this, _window, cx| {
            this.report_terminal_focus(true, cx);
        });
        let focus_out = cx.on_focus_out(
            &self.terminal.input.focus,
            window,
            |this, _event, _window, cx| {
                this.report_terminal_focus(false, cx);
            },
        );
        self.terminal.input.focus_subscriptions = vec![focus_in, focus_out];
    }

    pub(in crate::features) fn report_terminal_focus(
        &mut self,
        focused: bool,
        cx: &mut Context<Self>,
    ) {
        if focused && self.security.screen_locked() {
            return;
        }
        self.terminal.input.focus_active = focused;
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        if self.write_terminal_focus_report_to_session(&session_id, focused) {
            cx.notify();
        }
    }

    /// Send a DECSET 1004 focus report to a specific session when that session
    /// has enabled focus reporting. Protocol traffic is not command history.
    pub(in crate::features) fn write_terminal_focus_report_to_session(
        &mut self,
        session_id: &str,
        focused: bool,
    ) -> bool {
        if focused && self.security.screen_locked() {
            return false;
        }
        if self.session.is_disconnected(session_id) {
            return false;
        }
        if !self
            .terminal_protocol_state_for_session(session_id)
            .focus_reporting
        {
            return false;
        }
        let bytes = nyaterm_terminal::TerminalScreen::encode_focus_report(focused);
        self.write_session_protocol_response(session_id, &bytes)
            .is_ok()
    }

    pub(in crate::features) fn sync_terminal_cell_metrics_to_screens(&mut self) {
        let (width, height) = self.terminal_cell_size();
        let width = width.round().clamp(1.0, 512.0) as u16;
        let height = height.round().clamp(1.0, 512.0) as u16;
        self.terminal.view.screen.set_cell_metrics(width, height);
        for view in self.terminal.view.views.values_mut() {
            view.screen.set_cell_metrics(width, height);
        }
    }

    pub(in crate::features) fn send_terminal_input_to_session(
        &mut self,
        session_id: String,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> bool {
        if bytes.is_empty() {
            return false;
        }
        if self.session.is_disconnected(&session_id) {
            if self.set_terminal_status_if_changed(
                "session disconnected — reconnect before sending".to_string(),
            ) {
                cx.notify();
            }
            return false;
        }
        self.terminal.view.frame_pipeline.arm_output_event_wake();

        match self.write_session_input_recorded(&session_id, &bytes) {
            Ok(()) => {
                self.record_command_history_from_bytes(Some(&session_id), &bytes);
                let terminal_status_changed =
                    self.set_terminal_status_if_changed(format!("sent {} byte(s)", bytes.len()));
                self.arm_terminal_input_wake(cx);
                if terminal_status_changed {
                    cx.notify();
                }
                true
            }
            Err(error) => {
                if self.set_terminal_status_if_changed(format!("input failed: {error}")) {
                    cx.notify();
                }
                false
            }
        }
    }

    pub(in crate::features) fn send_terminal_raw_input_to_session(
        &mut self,
        session_id: String,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> bool {
        if bytes.is_empty() {
            return false;
        }
        if self.session.is_disconnected(&session_id) {
            if self.set_terminal_status_if_changed(
                "session disconnected — reconnect before sending".to_string(),
            ) {
                cx.notify();
            }
            return false;
        }
        self.terminal.view.frame_pipeline.arm_output_event_wake();

        match self.write_session_raw_input_recorded(&session_id, &bytes) {
            Ok(()) => {
                let terminal_status_changed =
                    self.set_terminal_status_if_changed(format!("sent {} byte(s)", bytes.len()));
                self.arm_terminal_input_wake(cx);
                if terminal_status_changed {
                    cx.notify();
                }
                true
            }
            Err(error) => {
                if self.set_terminal_status_if_changed(format!("input failed: {error}")) {
                    cx.notify();
                }
                false
            }
        }
    }

    pub(in crate::features) fn set_terminal_status_if_changed(
        &mut self,
        status: impl Into<String>,
    ) -> bool {
        let status = status.into();
        if !terminal_status_changed(self.shell.status(), status.as_str()) {
            return false;
        }
        self.shell.set_status(status);
        true
    }

    pub(in crate::features) fn terminal_key_mode_for_session(
        &self,
        session_id: Option<&str>,
    ) -> TerminalKeyMode {
        let (
            application_cursor,
            application_keypad,
            kitty_keyboard_disambiguate,
            kitty_keyboard_report_event_types,
            kitty_keyboard_report_alternate_keys,
            kitty_keyboard_report_all_keys_as_esc,
            kitty_keyboard_report_associated_text,
        ) = if let Some(session_id) = session_id {
            self.terminal
                .view
                .views
                .get(session_id)
                .map(|view| {
                    let protocol = view.protocol_state;
                    (
                        protocol.application_cursor_keys,
                        protocol.application_keypad,
                        protocol.kitty_keyboard_disambiguate,
                        protocol.kitty_keyboard_report_event_types,
                        protocol.kitty_keyboard_report_alternate_keys,
                        protocol.kitty_keyboard_report_all_keys_as_esc,
                        protocol.kitty_keyboard_report_associated_text,
                    )
                })
                .unwrap_or((false, false, false, false, false, false, false))
        } else {
            (
                self.terminal.view.screen.application_cursor_keys(),
                self.terminal.view.screen.application_keypad(),
                self.terminal.view.screen.kitty_keyboard_disambiguate(),
                self.terminal
                    .view
                    .screen
                    .kitty_keyboard_report_event_types(),
                self.terminal
                    .view
                    .screen
                    .kitty_keyboard_report_alternate_keys(),
                self.terminal
                    .view
                    .screen
                    .kitty_keyboard_report_all_keys_as_esc(),
                self.terminal
                    .view
                    .screen
                    .kitty_keyboard_report_associated_text(),
            )
        };
        TerminalKeyMode {
            application_cursor,
            application_keypad,
            kitty_keyboard_disambiguate,
            kitty_keyboard_report_event_types,
            kitty_keyboard_report_alternate_keys,
            kitty_keyboard_report_all_keys_as_esc,
            kitty_keyboard_report_associated_text,
        }
    }

    pub(in crate::features) fn terminal_key_bytes_for_event_for_session(
        &self,
        session_id: Option<&str>,
        event: &KeyDownEvent,
    ) -> Option<Vec<u8>> {
        terminal_key_bytes_for_mode_and_settings(
            event,
            self.terminal_key_mode_for_session(session_id),
            self.settings.summary().interaction_alt_as_meta,
        )
    }

    pub(in crate::features) fn terminal_should_defer_key_text_to_input_handler(
        &self,
        event: &KeyDownEvent,
    ) -> bool {
        terminal_should_defer_key_text_to_input_handler_for_state(
            self.settings.summary().interaction_mac_ime_compatibility,
            &self.terminal.input.ime_marked_text,
            event,
        )
    }

    pub(in crate::features) fn terminal_key_release_bytes_for_event_for_session(
        &self,
        session_id: Option<&str>,
        event: &KeyUpEvent,
    ) -> Option<Vec<u8>> {
        terminal_key_release_bytes_with_mode(event, self.terminal_key_mode_for_session(session_id))
    }
}

pub(super) fn terminal_key_bytes_for_mode_and_settings(
    event: &KeyDownEvent,
    mode: TerminalKeyMode,
    alt_as_meta: bool,
) -> Option<Vec<u8>> {
    // Prefer structured CSI for modified arrows (Ctrl/Alt) from terminal_key_bytes.
    if let Some(bytes) = terminal_key_bytes_with_mode(event, mode) {
        return Some(bytes);
    }
    // Alt-as-meta: ESC + character for shell word ops (Alt+b/f/d, etc.).
    if alt_as_meta
        && event.keystroke.modifiers.alt
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.function
        && let Some(input) = event.keystroke.key_char.as_deref()
        && !input.is_empty()
    {
        let mut bytes = Vec::with_capacity(input.len() + 1);
        bytes.push(0x1b);
        bytes.extend_from_slice(input.as_bytes());
        return Some(bytes);
    }
    // Even when alt-as-meta is off, still emit ESC+letter for Alt+b/f/d word ops
    // that shells commonly expect (Tauri XTerminal parity).
    if event.keystroke.modifiers.alt
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.function
        && !event.keystroke.modifiers.shift
    {
        let key = event.keystroke.key.as_str();
        if matches!(key, "b" | "B" | "f" | "F" | "d" | "D") {
            return Some(vec![0x1b, key.as_bytes()[0].to_ascii_lowercase()]);
        }
        if let Some(input) = event.keystroke.key_char.as_deref()
            && input.len() == 1
        {
            let ch = input.chars().next().unwrap();
            if matches!(ch, 'b' | 'B' | 'f' | 'F' | 'd' | 'D') {
                return Some(vec![0x1b, ch.to_ascii_lowercase() as u8]);
            }
        }
    }
    None
}

pub(super) fn terminal_should_defer_key_text_to_input_handler_for_state(
    ime_compatibility: bool,
    marked_text: &str,
    event: &KeyDownEvent,
) -> bool {
    if !ime_compatibility
        || !event
            .keystroke
            .modifiers
            .is_subset_of(&gpui::Modifiers::shift())
    {
        return false;
    }
    if !marked_text.is_empty() {
        return true;
    }
    event.keystroke.key_char.as_deref().is_some_and(|input| {
        !input.is_empty() && input.chars().all(|ch| !ch.is_control()) && !input.is_ascii()
    })
}

pub(super) fn terminal_status_changed(current: &str, next: &str) -> bool {
    current != next
}

#[allow(clippy::too_many_arguments)]
fn log_slow_terminal_input_diagnostic(
    kind: &'static str,
    byte_count: usize,
    synced: usize,
    failed: usize,
    total_duration: Duration,
    encode_duration: Duration,
    write_duration: Duration,
    suggestion_duration: Duration,
    history_duration: Duration,
    notify_duration: Duration,
) {
    if total_duration < TERMINAL_INPUT_SLOW_THRESHOLD {
        return;
    }
    tracing::warn!(
        diagnostic = "terminal_input_slow",
        kind,
        byte_count,
        synced,
        failed,
        total_us = total_duration.as_micros(),
        encode_us = encode_duration.as_micros(),
        write_us = write_duration.as_micros(),
        suggestion_us = suggestion_duration.as_micros(),
        history_us = history_duration.as_micros(),
        notify_us = notify_duration.as_micros(),
        "slow terminal input path"
    );
}
