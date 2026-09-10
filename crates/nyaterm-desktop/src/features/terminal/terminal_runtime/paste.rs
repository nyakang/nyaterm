use gpui::{Context, KeyDownEvent, Window};
use nyaterm_core::terminal_input_fanout_status;

use crate::features::NyaTermApp;
use crate::models::{is_multi_line_paste, normalize_paste_newlines};

impl NyaTermApp {
    pub(in crate::features) fn paste_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clipboard = cx.read_from_clipboard();
        if let Some(clipboard) = clipboard.as_ref() {
            if let Some(paths) = clipboard.entries().iter().find_map(|entry| match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => Some(paths),
                _ => None,
            }) {
                if let Some(id) = self.session.active_id_owned() {
                    self.handle_terminal_external_file_drop(id, paths.paths().to_vec(), cx);
                }
                return;
            }
            if self.settings.summary().terminal_paste_image_as_path
                && let Some(image) = clipboard.entries().iter().find_map(|entry| match entry {
                    gpui::ClipboardEntry::Image(image) => Some(image.clone()),
                    _ => None,
                })
            {
                self.paste_clipboard_image(image, cx);
                return;
            }
        }
        let Some(text) = clipboard.and_then(|item| item.text()) else {
            self.shell
                .set_status("clipboard does not contain text".to_string());
            cx.notify();
            return;
        };
        if let Some(session_id) = self
            .session
            .active_id_owned()
            .filter(|session_id| self.remote_desktop.is_session(session_id))
        {
            let _ = self.send_remote_committed_text(&session_id, &text);
            self.mark_user_activity();
            cx.notify();
            return;
        }
        self.paste_terminal_text(text, window, cx);
    }

    pub(in crate::features) fn paste_terminal_text(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            self.shell.set_status("clipboard text is empty".to_string());
            cx.notify();
            return;
        }
        if self
            .settings
            .summary()
            .terminal_show_multi_line_paste_dialog
            && is_multi_line_paste(&text)
        {
            self.terminal.paste.open(text);
            self.shell
                .set_status("multi-line paste confirmation opened".to_string());
            window.focus(&self.terminal.paste.focus, cx);
            cx.notify();
            return;
        }
        let payload = normalize_paste_newlines(&text);
        // Tauri pasteText: replace smart input selection when present.
        if let Some(selected) = self.smart_cursor_selected_input_range()
            && self.replace_smart_input_selection(selected, &payload, cx)
        {
            return;
        }
        self.send_terminal_paste_input(&payload, cx);
    }

    pub(in crate::features) fn session_bracketed_paste(&self, session_id: &str) -> bool {
        self.terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.protocol_state.bracketed_paste)
            .unwrap_or(false)
    }

    pub(in crate::features) fn wrap_terminal_paste_bytes_for_session(
        &self,
        session_id: &str,
        text: &str,
    ) -> Vec<u8> {
        let body = self.encode_session_outgoing(session_id, text.as_bytes());
        Self::wrap_terminal_paste_wire_bytes_for_bracketed(
            &body,
            self.session_bracketed_paste(session_id),
        )
    }

    pub(in crate::features) fn wrap_terminal_paste_wire_bytes_for_bracketed(
        body: &[u8],
        bracketed: bool,
    ) -> Vec<u8> {
        if bracketed {
            let mut out = Vec::with_capacity(body.len() + 12);
            out.extend_from_slice(b"\x1b[200~");
            out.extend_from_slice(body);
            out.extend_from_slice(b"\x1b[201~");
            out
        } else {
            body.to_vec()
        }
    }

    /// Paste fan-out wraps bracketed-paste mode per target session so sync peers
    /// with different DECBPM state receive correct framing.
    pub(in crate::features) fn send_terminal_paste_input(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            return;
        }
        let Some(session_id) = self.session.active_id_owned() else {
            if self.set_terminal_status_if_changed("no active session for paste") {
                cx.notify();
            }
            return;
        };
        if self.session.is_disconnected(&session_id) {
            if self
                .set_terminal_status_if_changed("session disconnected — press Enter to reconnect")
            {
                cx.notify();
            }
            return;
        }
        if self.active_terminal_visual_scroll_active() {
            self.scroll_terminal_to_bottom(cx);
        }

        let peers = self.sync_peer_session_ids(&session_id);
        let mut ok_sessions = Vec::new();
        let recording_bytes = text.as_bytes();
        let primary_bytes = self.wrap_terminal_paste_bytes_for_session(&session_id, text);
        let byte_count = primary_bytes.len();
        match self.write_session_wire_input_recorded_as(
            &session_id,
            &primary_bytes,
            recording_bytes,
        ) {
            Ok(()) => ok_sessions.push(session_id),
            Err(error) => {
                if self.set_terminal_status_if_changed(format!("paste failed: {error}")) {
                    cx.notify();
                }
                return;
            }
        }

        let mut synced = 0usize;
        let mut failed = 0usize;
        for peer_id in peers {
            let peer_bytes = self.wrap_terminal_paste_bytes_for_session(&peer_id, text);
            match self.write_session_wire_input_recorded_as(&peer_id, &peer_bytes, recording_bytes)
            {
                Ok(()) => {
                    ok_sessions.push(peer_id);
                    synced += 1;
                }
                Err(_) => failed += 1,
            }
        }

        // History tracks the logical pasted text, not per-session framing bytes.
        let history_bytes = text.as_bytes();
        let session_refs: Vec<&str> = ok_sessions.iter().map(String::as_str).collect();
        self.record_command_history_for_sessions(&session_refs, history_bytes);

        if self.set_terminal_status_if_changed(terminal_input_fanout_status(
            "pasted", byte_count, synced, failed,
        )) {
            cx.notify();
        }
    }

    pub(in crate::features) fn close_multi_line_paste(&mut self, cx: &mut Context<Self>) {
        self.terminal.paste.clear();
        self.shell
            .set_status("multi-line paste cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn direct_multi_line_paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.terminal.paste.take_normalized_text() else {
            self.shell
                .set_status("no multi-line paste is active".to_string());
            cx.notify();
            return;
        };
        self.send_terminal_paste_input(&text, cx);
    }

    pub(in crate::features) fn send_multi_line_paste_by_line(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.terminal.paste.take_normalized_text() else {
            self.shell
                .set_status("no multi-line paste is active".to_string());
            cx.notify();
            return;
        };
        // Line-by-line send intentionally skips bracketed paste framing.
        self.send_terminal_input(line_by_line_paste_bytes(&text), cx);
    }

    pub(in crate::features) fn handle_multi_line_paste_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        let primary = keystroke.modifiers.control || keystroke.modifiers.platform;
        if primary && !keystroke.modifiers.alt && !keystroke.modifiers.function {
            match keystroke.key.as_str() {
                "a" | "A" => {
                    self.terminal.paste.select_all();
                    cx.notify();
                    return;
                }
                "enter" => {
                    self.direct_multi_line_paste(cx);
                    return;
                }
                "l" | "L" => {
                    self.send_multi_line_paste_by_line(cx);
                    return;
                }
                _ => {}
            }
        }
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return;
        }
        match keystroke.key.as_str() {
            "escape" => self.close_multi_line_paste(cx),
            "backspace" => {
                let range = self.terminal.paste.selected_byte_range();
                let range = if range.is_empty() {
                    let start = self.terminal.paste.previous_char_boundary();
                    start..self.terminal.paste.cursor
                } else {
                    range
                };
                if self.terminal.paste.replace_range(range, "") {
                    cx.notify();
                }
            }
            "enter" => {
                if self.terminal.paste.replace_selection("\n") {
                    cx.notify();
                }
            }
            "delete" => {
                let range = self.terminal.paste.selected_byte_range();
                let range = if range.is_empty() {
                    let end = self.terminal.paste.next_char_boundary();
                    self.terminal.paste.cursor..end
                } else {
                    range
                };
                if self.terminal.paste.replace_range(range, "") {
                    cx.notify();
                }
            }
            "left" => {
                if !keystroke.modifiers.shift
                    && let Some(anchor) = self.terminal.paste.anchor
                {
                    let target = anchor.min(self.terminal.paste.cursor);
                    self.terminal.paste.move_cursor(target, false);
                    cx.notify();
                    return;
                }
                let target = self.terminal.paste.previous_char_boundary();
                self.terminal
                    .paste
                    .move_cursor(target, keystroke.modifiers.shift);
                cx.notify();
            }
            "right" => {
                if !keystroke.modifiers.shift
                    && let Some(anchor) = self.terminal.paste.anchor
                {
                    let target = anchor.max(self.terminal.paste.cursor);
                    self.terminal.paste.move_cursor(target, false);
                    cx.notify();
                    return;
                }
                let target = self.terminal.paste.next_char_boundary();
                self.terminal
                    .paste
                    .move_cursor(target, keystroke.modifiers.shift);
                cx.notify();
            }
            "home" => {
                let target = self.terminal.paste.current_line_start();
                self.terminal
                    .paste
                    .move_cursor(target, keystroke.modifiers.shift);
                cx.notify();
            }
            "end" => {
                let target = self.terminal.paste.current_line_end();
                self.terminal
                    .paste
                    .move_cursor(target, keystroke.modifiers.shift);
                cx.notify();
            }
            "up" => {
                self.terminal
                    .paste
                    .move_vertical(-1, keystroke.modifiers.shift);
                cx.notify();
            }
            "down" => {
                self.terminal
                    .paste
                    .move_vertical(1, keystroke.modifiers.shift);
                cx.notify();
            }
            _ => {
                if let Some(input) = keystroke
                    .key_char
                    .as_deref()
                    .filter(|input| !input.is_empty())
                    && self.terminal.paste.replace_selection(input)
                {
                    cx.notify();
                }
            }
        }
    }
}

fn line_by_line_paste_bytes(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in text.split('\n') {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

#[cfg(test)]
mod tests {
    use crate::features::NyaTermApp;

    use super::line_by_line_paste_bytes;

    #[test]
    fn bracketed_paste_wraps_wire_bytes_without_reencoding_body() {
        let body = [0xb2, 0xe2, b'\n'];
        let wrapped = NyaTermApp::wrap_terminal_paste_wire_bytes_for_bracketed(&body, true);

        assert!(wrapped.starts_with(b"\x1b[200~"));
        assert!(wrapped.ends_with(b"\x1b[201~"));
        assert_eq!(
            &wrapped[b"\x1b[200~".len()..wrapped.len() - b"\x1b[201~".len()],
            &body
        );
    }

    #[test]
    fn plain_paste_wire_bytes_are_body_only() {
        let body = b"plain";
        assert_eq!(
            NyaTermApp::wrap_terminal_paste_wire_bytes_for_bracketed(body, false),
            body
        );
    }

    #[test]
    fn line_by_line_paste_keeps_each_logical_line_and_appends_enter() {
        assert_eq!(
            line_by_line_paste_bytes("first\n第二\n"),
            "first\n第二\n\n".as_bytes()
        );
    }
}
