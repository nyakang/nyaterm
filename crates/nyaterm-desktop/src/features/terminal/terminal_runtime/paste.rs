use gpui::{AppContext as _, Context, KeyDownEvent, Keystroke, Window};
use nyaterm_core::terminal_input_fanout_status;
use nyaterm_ui::{NyaDocumentEditorEvent, NyaDocumentEditorState};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::models::{is_multi_line_paste, normalize_paste_newlines};

impl NyaTermApp {
    pub(in crate::features) fn paste_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.screen_locked() {
            return;
        }
        let clipboard = cx.read_from_clipboard();
        if let Some(clipboard) = clipboard.as_ref() {
            if let Some(paths) = clipboard.entries().iter().find_map(|entry| match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => Some(paths),
                _ => None,
            }) {
                if let Some(id) = self.session.active_id_owned() {
                    self.handle_terminal_external_file_drop(id, paths.paths().to_vec(), window, cx);
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
        if self.security.screen_locked() {
            return;
        }
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
            let text = normalize_paste_newlines(&text);
            let editor = cx.new(|cx| {
                NyaDocumentEditorState::new_with_placeholder(
                    window,
                    cx,
                    text,
                    t!("terminal.multiLinePasteTextPlaceholder").to_string(),
                )
            });
            let subscription =
                cx.subscribe(&editor, |this, _, event: &NyaDocumentEditorEvent, cx| {
                    if matches!(event, NyaDocumentEditorEvent::Changed(_)) {
                        this.mark_user_activity();
                        cx.notify();
                    }
                });
            self.terminal.paste.open(editor.clone(), subscription);
            self.shell
                .set_status("multi-line paste confirmation opened".to_string());
            editor.update(cx, |editor, cx| {
                editor.move_cursor_to_end(cx);
                editor.focus(window, cx);
            });
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
        if self.security.screen_locked() {
            return;
        }
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

    pub(in crate::features) fn close_multi_line_paste(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.paste.clear();
        self.shell
            .set_status("multi-line paste cancelled".to_string());
        self.focus_active_workspace_surface(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn direct_multi_line_paste(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.multi_line_paste_text(cx) else {
            self.shell
                .set_status("no multi-line paste is active".to_string());
            cx.notify();
            return;
        };
        let Some(text) = normalized_review_text(&text) else {
            self.shell
                .set_status("multi-line paste text is empty".to_string());
            cx.notify();
            return;
        };
        self.terminal.paste.clear();
        self.send_terminal_paste_input(&text, cx);
        self.focus_active_workspace_surface(window, cx);
    }

    pub(in crate::features) fn send_multi_line_paste_by_line(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.multi_line_paste_text(cx) else {
            self.shell
                .set_status("no multi-line paste is active".to_string());
            cx.notify();
            return;
        };
        let Some(text) = normalized_review_text(&text) else {
            self.shell
                .set_status("multi-line paste text is empty".to_string());
            cx.notify();
            return;
        };
        self.terminal.paste.clear();
        // Line-by-line send intentionally skips bracketed paste framing.
        self.send_terminal_input(line_by_line_paste_bytes(&text), cx);
        self.focus_active_workspace_surface(window, cx);
    }

    pub(in crate::features) fn handle_multi_line_paste_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        match multi_line_paste_shortcut(&event.keystroke) {
            Some(MultiLinePasteShortcut::Cancel) => self.close_multi_line_paste(window, cx),
            Some(MultiLinePasteShortcut::Direct) => self.direct_multi_line_paste(window, cx),
            Some(MultiLinePasteShortcut::LineByLine) => {
                self.send_multi_line_paste_by_line(window, cx)
            }
            None => return false,
        }
        true
    }

    fn multi_line_paste_text(&self, cx: &gpui::App) -> Option<String> {
        self.terminal
            .paste_review_editor()
            .map(|editor| editor.read(cx).value(cx))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MultiLinePasteShortcut {
    Cancel,
    Direct,
    LineByLine,
}

fn multi_line_paste_shortcut(keystroke: &Keystroke) -> Option<MultiLinePasteShortcut> {
    let primary = keystroke.modifiers.control || keystroke.modifiers.platform;
    if primary && !keystroke.modifiers.alt && !keystroke.modifiers.function {
        return match keystroke.key.as_str() {
            "enter" => Some(MultiLinePasteShortcut::Direct),
            "l" | "L" => Some(MultiLinePasteShortcut::LineByLine),
            _ => None,
        };
    }
    if !primary
        && !keystroke.modifiers.alt
        && !keystroke.modifiers.function
        && keystroke.key == "escape"
    {
        return Some(MultiLinePasteShortcut::Cancel);
    }
    None
}

fn normalized_review_text(text: &str) -> Option<String> {
    let text = normalize_paste_newlines(text);
    (!text.is_empty()).then_some(text)
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
    use gpui::{
        Entity, InteractiveElement as _, IntoElement, Keystroke, ParentElement as _, Render,
        Styled as _, TestAppContext, div,
    };

    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;

    use super::{
        MultiLinePasteShortcut, line_by_line_paste_bytes, multi_line_paste_shortcut,
        normalized_review_text,
    };

    struct FocusHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for FocusHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .size_full()
                .child(div().track_focus(self.app.read(cx).terminal.input_focus()))
        }
    }

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

    #[test]
    fn paste_review_shortcuts_leave_editor_commands_to_the_editor() {
        assert_eq!(
            multi_line_paste_shortcut(&Keystroke::parse("ctrl-enter").unwrap()),
            Some(MultiLinePasteShortcut::Direct)
        );
        assert_eq!(
            multi_line_paste_shortcut(&Keystroke::parse("ctrl-l").unwrap()),
            Some(MultiLinePasteShortcut::LineByLine)
        );
        assert_eq!(
            multi_line_paste_shortcut(&Keystroke::parse("escape").unwrap()),
            Some(MultiLinePasteShortcut::Cancel)
        );
        assert_eq!(
            multi_line_paste_shortcut(&Keystroke::parse("ctrl-a").unwrap()),
            None
        );
        assert_eq!(
            multi_line_paste_shortcut(&Keystroke::parse("enter").unwrap()),
            None
        );
    }

    #[test]
    fn paste_review_normalizes_newlines_without_accepting_empty_text() {
        assert_eq!(
            normalized_review_text("first\r\nsecond\rthird").as_deref(),
            Some("first\nsecond\nthird")
        );
        assert_eq!(normalized_review_text(""), None);
    }

    #[test]
    fn every_paste_review_action_restores_terminal_focus() {
        let dir = TestConfigDir::new("nyaterm-desktop-paste-review-focus");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, dir.path(), "session-a");
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| FocusHost { app: host_app });

        for action in ["cancel", "direct", "line-by-line"] {
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                app.update(cx, |app, cx| {
                    app.paste_terminal_text("first\nsecond".to_string(), window, cx);
                    assert!(app.terminal.overlay_visibility().paste_review);
                    let editor = app.terminal.paste_review_editor().unwrap();
                    assert_eq!(editor.read(cx).selected_range(cx), 12..12);
                    match action {
                        "cancel" => app.close_multi_line_paste(window, cx),
                        "direct" => app.direct_multi_line_paste(window, cx),
                        "line-by-line" => app.send_multi_line_paste_by_line(window, cx),
                        _ => unreachable!(),
                    }
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                assert!(app.read(cx).terminal.input_focus().is_focused(window));
                assert!(!app.read(cx).terminal.overlay_visibility().paste_review);
            });
        }
    }
}
