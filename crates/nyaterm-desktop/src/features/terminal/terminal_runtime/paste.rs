use gpui::{AppContext as _, Context, KeyDownEvent, Keystroke, Window};
use nyaterm_core::completion::types::supports_full_command_fill;
use nyaterm_core::terminal::editing::{EditIntent, plan_edit};
use nyaterm_core::terminal::input_tracker::apply_terminal_input_data_in_place;
use nyaterm_core::terminal_input_fanout_status;
use nyaterm_ui::{NyaDocumentEditorEvent, NyaDocumentEditorState};
use rust_i18n::t;
use std::time::Instant;

use crate::features::NyaTermApp;
use crate::features::terminal::command_suggestions::paste::SuggestionPasteContext;
use crate::features::terminal::editing_payload::shell_edit_payload;
use crate::features::terminal::editing_state::SnapshotEvidence;
use crate::models::{is_multi_line_paste, normalize_paste_newlines};

impl NyaTermApp {
    pub(in crate::features) fn paste_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.screen_locked()
            || self.active_workspace_tab().is_some_and(|key| {
                !crate::models::WorkspaceTabId::from_layout_key(&key).is_session()
            })
        {
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
        if self.security.screen_locked()
            || self.active_workspace_tab().is_some_and(|key| {
                !crate::models::WorkspaceTabId::from_layout_key(&key).is_session()
            })
        {
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
            self.open_multi_line_paste_review(text, window, cx);
            return;
        }
        let payload = normalize_paste_newlines(&text);
        // Tauri pasteText: replace smart input selection when present.
        if let Some(selected) = self.smart_cursor_selected_input_range()
            && self.replace_smart_input_selection_with_paste(selected, &payload, cx)
        {
            return;
        }
        self.send_terminal_paste_input(&payload, cx);
    }

    fn open_multi_line_paste_review(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = normalize_paste_newlines(&text);
        let editor = cx.new(|cx| {
            NyaDocumentEditorState::new_source_with_placeholder(
                window,
                cx,
                text,
                "bash",
                t!("terminal.multiLinePasteTextPlaceholder").to_string(),
            )
        });
        let subscription = cx.subscribe(&editor, |this, _, event: &NyaDocumentEditorEvent, cx| {
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
        });
        cx.notify();
    }

    pub(in crate::features) fn open_pending_suggestion_paste(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut review) = self.terminal.paste.suggestion.take() else {
            return;
        };
        let Some(text) = review.pending_text.take() else {
            self.terminal.paste.suggestion = Some(review);
            return;
        };
        if let Err(message) = self.validate_command_suggestion_input(&review.context, cx) {
            self.terminal.paste.clear();
            self.shell.set_status(message);
            cx.notify();
            return;
        }
        // Choosing a multiline completion always opens review, even when the
        // clipboard preference allows ordinary multiline pastes immediately.
        self.open_multi_line_paste_review(text, window, cx);
        self.terminal.paste.suggestion = Some(review);
    }

    pub(in crate::features) fn session_bracketed_paste(&self, session_id: &str) -> bool {
        self.terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.protocol_state.bracketed_paste)
            .unwrap_or(false)
    }

    pub(in crate::features) fn suggestion_paste_needs_bracketed_shell(&self) -> bool {
        self.terminal
            .paste
            .suggestion
            .as_ref()
            .is_some_and(|review| {
                self.session.active_id() != Some(review.context.session_id.as_str())
                    || !self.session_bracketed_paste(&review.context.session_id)
                    || !self
                        .terminal
                        .editing
                        .state()
                        .is_some_and(|state| state.allows_editing())
            })
    }

    pub(in crate::features) fn suggestion_paste_cannot_replace_input(&self) -> bool {
        self.terminal.paste.suggestion.is_some()
            && self
                .terminal
                .editing
                .state()
                .is_none_or(|state| !state.model.input.value.is_empty() && !state.allows_editing())
    }

    pub(in crate::features) fn wrap_terminal_paste_bytes_for_session(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<Vec<u8>, String> {
        let bracketed = self.session_bracketed_paste(session_id);
        // Outside bracketed paste the program sees the bytes as typed input, and
        // a typed line break is CR (the Enter key). Sending LF makes it Ctrl+J,
        // which programs such as nano 3.x treat as "justify" and join the lines.
        // xterm, alacritty and iTerm2 convert the same way. The conversion is
        // done on text, before the session encoding, so it stays encoding-safe.
        let text = if bracketed {
            std::borrow::Cow::Borrowed(text)
        } else {
            std::borrow::Cow::Owned(text.replace('\n', "\r"))
        };
        let body = self.encode_session_outgoing(session_id, text.as_bytes())?;
        Ok(Self::wrap_terminal_paste_wire_bytes_for_bracketed(
            &body, bracketed,
        ))
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
        if self.security.screen_locked()
            || self.active_workspace_tab().is_some_and(|key| {
                !crate::models::WorkspaceTabId::from_layout_key(&key).is_session()
            })
        {
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
        let primary_bytes = match self.wrap_terminal_paste_bytes_for_session(&session_id, text) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.set_terminal_status_if_changed(format!("paste failed: {error}"));
                cx.notify();
                return;
            }
        };
        let byte_count = primary_bytes.len();
        // A newline inside bracketed paste edits the shell buffer; only a
        // later Enter or OSC boundary can confirm an actual submission.
        let navigation_candidate = if primary_bytes.starts_with(b"\x1b[200~") {
            None
        } else {
            self.prepare_command_navigation_input(text.as_bytes())
        };
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
            let peer_bytes = match self.wrap_terminal_paste_bytes_for_session(&peer_id, text) {
                Ok(bytes) => bytes,
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };
            match self.write_session_wire_input_recorded_as(&peer_id, &peer_bytes, recording_bytes)
            {
                Ok(()) => {
                    ok_sessions.push(peer_id);
                    synced += 1;
                }
                Err(_) => failed += 1,
            }
        }

        self.note_shell_editing_input_with_navigation(text.as_bytes(), navigation_candidate, cx);
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
        if let Some(review) = self.terminal.paste.suggestion.take() {
            self.terminal.paste.clear();
            self.send_reviewed_command_suggestion(&review.context, &text, false, cx);
            self.focus_active_workspace_surface(window, cx);
            return;
        }
        self.terminal.paste.clear();
        if let Some(selected) = self.smart_cursor_selected_input_range() {
            self.replace_smart_input_selection_with_paste(selected, &text, cx);
        } else {
            self.send_terminal_paste_input(&text, cx);
        }
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
        if let Some(review) = self.terminal.paste.suggestion.take() {
            self.terminal.paste.clear();
            self.send_reviewed_command_suggestion(&review.context, &text, true, cx);
            self.focus_active_workspace_surface(window, cx);
            return;
        }
        self.terminal.paste.clear();
        // Line-by-line send intentionally skips bracketed paste framing.
        self.send_terminal_input(line_by_line_paste_bytes(&text), cx);
        self.focus_active_workspace_surface(window, cx);
    }

    fn send_reviewed_command_suggestion(
        &mut self,
        context: &SuggestionPasteContext,
        text: &str,
        by_line: bool,
        cx: &mut Context<Self>,
    ) {
        let snapshot = match self.validate_command_suggestion_input(context, cx) {
            Ok(snapshot) => snapshot,
            Err(message) => {
                self.shell.set_status(message);
                cx.notify();
                return;
            }
        };
        let state = self
            .terminal
            .editing
            .state()
            .expect("validated editing state");
        if !supports_full_command_fill(text)
            || state.model.input.desynced
            || state.model.input.line_rewrite_required
            || state.model.input.multiline
            || state.model.input.paste_mode
            || state.model.input.cursor != state.model.input.value.len()
            || (!state.model.input.value.is_empty() && !state.allows_editing())
        {
            self.shell
                .set_status("Cannot safely paste this suggestion; select it again");
            cx.notify();
            return;
        }
        // Direct completion paste retains the fill-only contract. The separate
        // line-by-line action is the user's explicit request to submit lines.
        if !by_line
            && (!state.allows_editing() || !self.session_bracketed_paste(&context.session_id))
        {
            self.shell.set_status(
                "Direct suggestion paste requires a shell with bracketed paste enabled",
            );
            cx.notify();
            return;
        }
        let insertion = if by_line {
            String::from_utf8(line_by_line_paste_bytes(text)).expect("UTF-8 paste text")
        } else {
            text.to_owned()
        };
        let input = &state.model.input;
        let plan = if input.value.is_empty() {
            plan_edit(input, state.model.version, EditIntent::Move(0)).map(|mut plan| {
                plan.insert_text = insertion.clone();
                plan.predicted.value = insertion.clone();
                plan.predicted.cursor = insertion.len();
                plan.predicted.multiline = insertion.contains(['\r', '\n']);
                plan
            })
        } else {
            plan_edit(
                input,
                state.model.version,
                EditIntent::Replace {
                    range: 0..input.value.len(),
                    text: &insertion,
                },
            )
        };
        let Some(mut plan) = plan else {
            self.shell.set_status("Cannot plan this suggestion paste");
            cx.notify();
            return;
        };
        let insertion_wire = if by_line {
            self.encode_session_outgoing(&context.session_id, insertion.as_bytes())
        } else {
            self.wrap_terminal_paste_bytes_for_session(&context.session_id, &insertion)
        };
        let insertion_wire = match insertion_wire {
            Ok(bytes) => bytes,
            Err(error) => {
                self.shell.set_status(format!("paste failed: {error}"));
                cx.notify();
                return;
            }
        };
        let mode = self.terminal_key_mode_for_session(Some(&context.session_id));
        let (wire, logical) = shell_edit_payload(&plan, mode, insertion_wire);
        if by_line {
            plan.predicted = input.clone();
            apply_terminal_input_data_in_place(
                &mut plan.predicted,
                std::str::from_utf8(&logical).expect("logical UTF-8 paste input"),
            );
        }
        self.terminal.view.frame_pipeline.arm_output_event_wake();
        let result = if by_line {
            self.write_session_wire_input_recorded_as(&context.session_id, &wire, &logical)
        } else {
            self.write_session_literal_wire_input_recorded_as(&context.session_id, &wire, &logical)
        };
        if let Err(error) = result {
            self.terminal.editing.state_mut().invalidate();
            self.shell.set_status(format!("paste failed: {error}"));
            cx.notify();
            return;
        }
        let state = self.terminal.editing.state_mut();
        state.model.accept_plan(plan, Instant::now());
        state.pending_evidence = Some(SnapshotEvidence::from_snapshot(&snapshot));
        state.mapping = None;
        state.selection = None;
        state.selection_origin_version = None;
        if by_line {
            let submitted = text
                .split('\n')
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            self.record_submitted_command_history(&[&context.session_id], submitted);
        }
        self.clear_terminal_selection(cx);
        self.arm_terminal_input_wake(cx);
        self.schedule_shell_edit_confirmation_timeout(cx);
        self.shell.set_status(if by_line {
            "Suggestion sent line by line"
        } else {
            "Suggestion pasted; press Enter to execute"
        });
        cx.notify();
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
        // Each line is submitted like a typed Enter, which is CR.
        bytes.push(b'\r');
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
            "first\r第二\r\r".as_bytes()
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
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
                    editor.update(cx, |editor, cx| {
                        editor.apply_edit(" # 编辑".into(), 21..21, window, cx);
                    });
                    assert_eq!(
                        app.multi_line_paste_text(cx).as_deref(),
                        Some("first\nsecond # 编辑")
                    );
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

    #[test]
    fn empty_paste_review_keeps_the_editor_open_for_both_send_actions() {
        let dir = TestConfigDir::new("nyaterm-empty-paste-review");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "session-a");
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| FocusHost { app: host_app });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.paste_terminal_text("first\nsecond".into(), window, cx);
                let editor = app.terminal.paste_review_editor().unwrap();
                editor.update(cx, |editor, cx| editor.replace_content("", window, cx));
                assert_eq!(app.multi_line_paste_text(cx).as_deref(), Some(""));
                app.direct_multi_line_paste(window, cx);
                assert!(app.terminal.overlay_visibility().paste_review);
                app.send_multi_line_paste_by_line(window, cx);
                assert!(app.terminal.overlay_visibility().paste_review);
                app.close_multi_line_paste(window, cx);
            });
        });
    }
}
