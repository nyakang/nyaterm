//! GPUI coordination for shell input evidence and predictions.

use std::time::Instant;
use zeroize::Zeroize;

use gpui::Context;
use nyaterm_core::command_starts_suggestion_suppressing_program;
use nyaterm_core::command_suggestion_suppression::command_starts_line_editor;
use nyaterm_core::terminal::editing::{EDIT_CONFIRMATION_TIMEOUT, EditCapability, EditPhase};
use nyaterm_core::terminal::input_tracker::{
    MAX_TRACKED_INPUT_BYTES, TerminalInputState, get_tracked_submission_command,
    strip_terminal_command_prompt,
};
use nyaterm_terminal::editing::{recover_marked_input, resolve_input};

use super::editing_state::SnapshotEvidence;
use crate::features::NyaTermApp;

impl NyaTermApp {
    /// This runs once after a successful logical write, regardless of suggestions.
    pub(in crate::features) fn note_shell_editing_input(
        &mut self,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.session.active_id_owned() else {
            return;
        };
        self.terminal.editing.activate(&id);
        self.terminal.editing.state_mut().suggestion_fill = None;
        if self.active_terminal_uses_alternate_screen()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.credential_suggestions.is_some()
            || self.security.screen_locked()
        {
            self.terminal.editing.clear_active();
            return;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            self.terminal.editing.clear_active();
            return;
        };
        if text.is_empty() {
            return;
        }
        let submitted_for_navigation =
            get_tracked_submission_command(self.terminal.editing.predicted_input());
        let interactive = self
            .terminal
            .editing
            .state_mut()
            .command_navigation_interactive;
        let eligible = text == "\r"
            && !submitted_for_navigation.is_empty()
            && !interactive
            && !self.mcp.capabilities.terminal_executions.capturing(&id)
            && self
                .session
                .metadata(&id)
                .is_some_and(|metadata| match &metadata.launch_config {
                    crate::models::SessionLaunchConfig::Local(_) => true,
                    crate::models::SessionLaunchConfig::Ssh(config) => {
                        !config.effective_terminal_shell_integration()
                    }
                    _ => false,
                });
        self.terminal
            .view
            .frame_pipeline
            .note_command_input(id.clone(), eligible);
        if text.contains(['\r', '\n']) {
            let state = self.terminal.editing.state_mut();
            state.launched_line_editor = command_starts_line_editor(&submitted_for_navigation);
            state.echo_line_editor = false;
        } else if matches!(text, "\u{3}" | "\u{4}") {
            self.terminal.editing.state_mut().launched_line_editor = false;
        }
        if text == "\u{4}"
            || (text == "\r"
                && matches!(
                    submitted_for_navigation.trim(),
                    "exit" | "quit" | "exit()" | "quit()" | ".exit" | ".quit" | "\\q"
                ))
        {
            self.terminal
                .editing
                .state_mut()
                .command_navigation_interactive = false;
        } else if text == "\r"
            && nyaterm_core::command_starts_interactive_input(&submitted_for_navigation)
        {
            self.terminal
                .editing
                .state_mut()
                .command_navigation_interactive = true;
        }
        let was_suppressed = self.terminal.assist.command_suggestions_suppressed;
        if text == "\u{3}" || (was_suppressed && text == "q") {
            self.terminal.assist.command_suggestions_suppressed = false;
        }
        if !was_suppressed
            && self.terminal.assist.pending_command_history_entry.is_none()
            && text.contains(['\r', '\n'])
        {
            let submitted = get_tracked_submission_command(self.terminal.editing.predicted_input());
            if !submitted.is_empty() {
                if command_starts_suggestion_suppressing_program(&submitted) {
                    self.terminal.assist.command_suggestions_suppressed = true;
                }
                self.terminal.assist.pending_command_history_entry = Some(submitted);
            }
        }
        let snapshot = self.terminal_snapshot_for_session(Some(&id), 0);
        let state = self.terminal.editing.state_mut();
        if matches!(text, "\r" | "\u{3}") {
            state.submitted_anchor = snapshot.shell_input_anchor;
        }
        state.model.note_input(text, Instant::now());
        state.pending_evidence = Some(SnapshotEvidence::from_snapshot(&snapshot));
        state.selection = None;
        state.selection_origin_version = None;
        state.mapping = None;
        self.schedule_shell_edit_confirmation_timeout(cx);
    }

    pub(in crate::features) fn schedule_shell_edit_confirmation_timeout(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.session.active_id_owned() else {
            return;
        };
        let version = self.terminal.editing.state_mut().model.version;
        let confirmation_id = self.terminal.editing.allocate_confirmation_id();
        self.terminal.editing.state_mut().confirmation_id = confirmation_id;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(EDIT_CONFIRMATION_TIMEOUT)
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(state) = this.terminal.editing.sessions.get_mut(&id)
                    && state.model.version == version
                    && state.confirmation_id == confirmation_id
                    && state.model.expire(Instant::now())
                {
                    let suggestion_expired = state.suggestion_fill.take().is_some();
                    state.mapping = None;
                    state.selection = None;
                    if this.session.active_id() == Some(id.as_str()) {
                        this.dismiss_command_suggestions(cx);
                        if suggestion_expired {
                            this.shell.set_status("Suggestion confirmation timed out; inspect the input before executing");
                        }
                    }
                    cx.notify();
                }
            });
        });
        self.terminal.editing.state_mut().confirmation_task = Some(task);
    }

    /// Only fresh live snapshots can confirm an edit. An incomplete repaint is
    /// allowed while Pending; it cannot authorize destructive operations.
    pub(in crate::features) fn reconcile_shell_editing(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_id() != Some(id) {
            return;
        }
        self.terminal.editing.activate(id);
        if self.session.is_disconnected(id)
            || self.active_terminal_uses_alternate_screen()
            || self.is_credential_prompt_input_mode()
            || self.security.screen_locked()
        {
            self.terminal.editing.clear_active();
            return;
        }
        let snapshot = self.terminal_snapshot_for_session(Some(id), 0);
        let Some(cursor_row) = snapshot.row(snapshot.cursor.row) else {
            return;
        };
        let Some(line_id) = cursor_row.line_id else {
            return;
        };
        let geometry = (snapshot.cols, snapshot.viewport_rows, line_id.epoch);
        let sync_revision = self.sync_input.revision();
        let shell_session = self.session.metadata(id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Local(_)
                    | crate::models::SessionLaunchConfig::Ssh(_)
            )
        });
        let state = self.terminal.editing.state_mut();
        state.cancel_stale_suggestion_fill(Instant::now(), sync_revision);
        if state.awaiting_snapshot {
            return;
        }
        if state.geometry.is_some_and(|old| old != geometry) {
            state.invalidate();
            state.model.input.desynced |= !state.model.input.value.is_empty();
        }
        state.geometry = Some(geometry);
        if state
            .observed_anchor
            .is_some_and(|old| old != snapshot.shell_input_anchor)
        {
            let old_anchor = state.observed_anchor.flatten();
            let submitted = old_anchor.is_some() && old_anchor == state.submitted_anchor;
            state.invalidate();
            state.submitted_anchor = None;
            // OSC C may arrive after typing has already started in a child shell.
            // The submitted anchor belongs to the previous logical input.
            state.model.input.desynced |=
                old_anchor.is_some() && !submitted && !state.model.input.value.is_empty();
        }
        state.observed_anchor = Some(snapshot.shell_input_anchor);
        if state.model.expire(Instant::now()) {
            state.suggestion_fill = None;
            state.mapping = None;
            state.selection = None;
        }
        let input = &state.model.input;
        if state.model.phase == EditPhase::Pending
            && state.pending_evidence.as_ref() == Some(&SnapshotEvidence::from_snapshot(&snapshot))
        {
            return;
        }
        let mut mapping = if !input.desynced
            && !input.line_rewrite_required
            && !input.multiline
            && !input.paste_mode
        {
            resolve_input(&snapshot, &input.value, input.cursor)
        } else {
            None
        };
        if mapping.is_none()
            && state.model.phase != EditPhase::Pending
            && (state.model.input.desynced
                || state.model.input.line_rewrite_required
                || state.model.input.value.is_empty())
            && let Some((value, cursor)) = recover_marked_input(&snapshot, MAX_TRACKED_INPUT_BYTES)
        {
            state.model.input.value.zeroize();
            state.model.input = TerminalInputState {
                value,
                cursor,
                ..TerminalInputState::default()
            };
            state.model.version = state.model.version.wrapping_add(1);
            mapping = resolve_input(&snapshot, &state.model.input.value, cursor);
        }
        let queued = if let Some(mapping) = mapping {
            // A unique echo can also come from canonical TTY input (cat/tee).
            // Require a shell prompt or a positively identified child line editor.
            let prompt = mapping.cells.first().map(|first| {
                snapshot
                    .row(first.row)
                    .into_iter()
                    .flat_map(|row| row.cells.iter().take(first.col))
                    .filter(|cell| cell.width != 0)
                    .map(|cell| {
                        if cell.text.is_empty() {
                            " "
                        } else {
                            cell.text.as_ref()
                        }
                    })
                    .collect::<String>()
            });
            let known_prompt = prompt.as_ref().is_some_and(|prefix| {
                !prefix.trim().is_empty()
                    && (strip_terminal_command_prompt(prefix).is_empty()
                        || state.line_editor_prompt.as_ref() == Some(prefix))
            });
            let has_prompt = prompt
                .as_ref()
                .is_some_and(|prefix| !prefix.trim().is_empty());
            state.echo_line_editor =
                shell_session && (known_prompt || (state.launched_line_editor && has_prompt));
            if state.echo_line_editor
                && prompt
                    .as_ref()
                    .is_some_and(|prefix| !prefix.trim().is_empty())
            {
                state.line_editor_prompt = prompt;
            }
            if state.mapping.as_ref().is_some_and(|old| old != &mapping) {
                state.selection = None;
                state.selection_origin_version = None;
            }
            state.mapping = Some(mapping);
            state.pending_evidence = None;
            state.capability = if snapshot.shell_input_anchor.is_some() {
                EditCapability::MarkedShell
            } else {
                EditCapability::EchoMatched
            };
            state.model.confirm()
        } else {
            // An incomplete or different redraw cannot acknowledge this
            // suggestion. Ordinary typing reconciliation may continue, but the
            // cancelled request must never be revived by a later matching frame.
            if state.suggestion_fill.take().is_some() {
                self.shell.set_status(
                    "Suggestion echo is not confirmed; inspect the input before executing",
                );
                cx.notify();
            }
            if state.model.phase == EditPhase::Ready {
                state.invalidate();
            }
            None
        };
        if let Some(target) = queued {
            let _ = self.move_smart_input_cursor(target, cx);
        }
        if let Some(execute_requested) = self
            .terminal
            .editing
            .state_mut()
            .take_confirmed_suggestion_fill(Instant::now(), sync_revision)
        {
            // Echo proves the fill only. No current capability proves that the
            // foreground program would interpret a synthetic Enter as intended.
            self.shell.set_status(if execute_requested {
                "Suggestion confirmed; press Enter again to execute"
            } else {
                "Suggestion fill confirmed"
            });
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::time::Duration;
    use std::time::Instant;

    use gpui::{
        AppContext as _, EntityInputHandler, IntoElement, MouseButton, MouseUpEvent, Render,
        TestAppContext, div,
    };
    use nyaterm_core::terminal::editing::{
        EDIT_CONFIRMATION_TIMEOUT, EditCapability, EditIntent, EditPhase, plan_edit,
    };

    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{TerminalBufferCellPos, TerminalSelection};
    use crate::test_support::TestConfigDir;
    use nyaterm_terminal::selection::{TerminalCellSide, TerminalSelectionEndpoint};

    struct InputHost;

    #[test]
    fn half_cell_selection_deletes_and_replaces_only_the_highlighted_input() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-half-cell");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abcdef", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ abcdef".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            app.terminal.selection.session_id = Some("s1".into());
            for (start_col, start_side, end_col, end_side, expected) in [
                (
                    3,
                    TerminalCellSide::Left,
                    5,
                    TerminalCellSide::Left,
                    Some((1, 3)),
                ),
                (
                    5,
                    TerminalCellSide::Left,
                    3,
                    TerminalCellSide::Left,
                    Some((1, 3)),
                ),
                (
                    3,
                    TerminalCellSide::Left,
                    3,
                    TerminalCellSide::Right,
                    Some((1, 2)),
                ),
                (3, TerminalCellSide::Right, 4, TerminalCellSide::Left, None),
                (
                    1,
                    TerminalCellSide::Right,
                    4,
                    TerminalCellSide::Left,
                    Some((0, 2)),
                ),
                (1, TerminalCellSide::Left, 4, TerminalCellSide::Left, None),
            ] {
                app.terminal.selection.selection = Some(TerminalSelection {
                    anchor: TerminalSelectionEndpoint::new(
                        TerminalBufferCellPos::new(0, start_col),
                        start_side,
                    ),
                    head: TerminalSelectionEndpoint::new(
                        TerminalBufferCellPos::new(0, end_col),
                        end_side,
                    ),
                    all_buffer: false,
                });
                app.begin_smart_input_selection();
                app.commit_smart_input_selection();
                let range = app.smart_cursor_selected_input_range();
                assert_eq!(
                    range.as_ref().map(|range| (range.start, range.end)),
                    expected
                );
                if let Some(range) = range {
                    let state = app.terminal.editing.state().unwrap();
                    let delete = plan_edit(
                        &state.model.input,
                        state.model.version,
                        EditIntent::Delete(range.start..range.end),
                    )
                    .unwrap();
                    let replace = plan_edit(
                        &state.model.input,
                        state.model.version,
                        EditIntent::Replace {
                            range: range.start..range.end,
                            text: "X",
                        },
                    )
                    .unwrap();
                    let mut deleted = "abcdef".to_string();
                    deleted.replace_range(range.start..range.end, "");
                    let mut replaced = "abcdef".to_string();
                    replaced.replace_range(range.start..range.end, "X");
                    assert_eq!(delete.predicted.value, deleted);
                    assert_eq!(replace.predicted.value, replaced);
                }
            }
        });
    }

    #[test]
    fn foreground_echo_cannot_move_delete_or_replace_shell_input() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-foreground");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"cat", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ cat".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.can_use_smart_cursor_selection());
            app.note_shell_editing_input(b"\r", cx);
            app.note_shell_editing_input(b"abcdef", cx);
            app.terminal
                .seed_session_view("s1".into(), "abcdef".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert_eq!(
                app.terminal.editing.state().unwrap().capability,
                EditCapability::EchoMatched
            );
            assert!(!app.can_use_smart_cursor_selection());
            assert!(!app.move_smart_input_cursor(1, cx));
            let range =
                nyaterm_core::terminal::input_tracker::InputSelectionRange::new(1, 4).unwrap();
            assert!(!app.delete_smart_input_selection(range, cx));
            assert!(!app.replace_smart_input_selection(range, "X", cx));
            assert!(!app.replace_smart_input_selection_with_paste(range, "X", cx));
            app.terminal.editing.activate("other");
            assert!(
                app.terminal.editing.sessions["s1"]
                    .model
                    .input
                    .value
                    .is_empty()
            );
        });
    }

    #[test]
    fn docker_line_editor_supports_custom_prompt_but_loses_permission_on_cat() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-docker-context");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"docker exec -it box sh", cx);
            app.note_shell_editing_input(b"\r", cx);
            app.note_shell_editing_input(b"cat", cx);
            app.terminal
                .seed_session_view("s1".into(), "custom> cat".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.can_use_smart_cursor_selection());
            app.note_shell_editing_input(b"\r", cx);
            app.note_shell_editing_input(b"abcdef", cx);
            app.terminal
                .seed_session_view("s1".into(), "abcdef".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(!app.can_use_smart_cursor_selection());
        });
    }

    impl Render for InputHost {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn native_ime_ignores_disabled_legacy_setting_and_commits_selection_replacement_once() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-ime");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        cx.update_entity(&app, |app, cx| {
            let metadata = app.session.metadata("s1").unwrap().clone();
            app.session.register_session_metadata(&info.id, metadata);
            app.session.select_active_session(&info.id);
            app.note_shell_editing_input(b"abcdef", cx);
            app.terminal
                .seed_session_view(info.id.clone(), "$ abcdef".into(), "UTF-8");
            app.reconcile_shell_editing(&info.id, cx);
            app.terminal.selection.session_id = Some(info.id.clone());
            app.terminal.selection.selection = Some(TerminalSelection {
                anchor: TerminalBufferCellPos::new(0, 3).into(),
                head: TerminalBufferCellPos::new(0, 6).into(),
                all_buffer: false,
            });
            app.begin_smart_input_selection();
            app.commit_smart_input_selection();
            assert!(app.smart_cursor_selected_input_range().is_some());
        });
        let (_, vcx) = cx.add_window_view(|_, _| InputHost);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut settings = app.settings.summary().clone();
                settings.interaction_mac_ime_compatibility = false;
                app.settings.replace_summary(settings);
                let version = app.terminal.editing.state().unwrap().model.version;
                app.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
                assert_eq!(app.terminal.input.ime.text(), "zhong");
                assert_eq!(
                    app.selected_text_range(false, window, cx).unwrap().range,
                    5..5
                );
                assert_eq!(app.terminal.editing.state().unwrap().model.version, version);
                let key = gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        key: "中".into(),
                        key_char: Some("中".into()),
                        modifiers: gpui::Modifiers::default(),
                    },
                    is_held: false,
                    prefer_character_input: false,
                };
                assert!(app.terminal_should_defer_key_text_to_input_handler(&key));
                app.replace_text_in_range(None, "中文", window, cx);
                assert!(app.terminal.input.ime.text().is_empty());
                assert_eq!(app.terminal.input.ime.selection(), 0..0);
                assert_eq!(app.terminal.editing.predicted_input().value, "a中文ef");
                assert_eq!(
                    app.terminal.editing.state().unwrap().model.version,
                    version + 1
                );
                assert_eq!(
                    app.terminal.editing.state().unwrap().model.phase,
                    EditPhase::Pending
                );
            })
        });
        let expected = "\x1b[D\x1b[D\x7f\x7f\x7f中文".as_bytes();
        let mut received = vec![0; expected.len()];
        peer.read_exact(&mut received).unwrap();
        assert_eq!(received, expected);
        peer.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let error = peer.read(&mut [0]).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        manager.close(&info.id).unwrap();
    }

    #[test]
    fn nested_shell_echo_enables_editing_even_with_suppressed_or_disabled_suggestions() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-nested");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            let mut settings = app.settings.summary().clone();
            settings.interaction_command_suggestions_enabled = false;
            app.settings.replace_summary(settings);
            app.terminal.assist.command_suggestions_suppressed = true;
            app.note_shell_editing_input(b"echo abc", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ echo abc".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            let state = app.terminal.editing.state().unwrap();
            assert_eq!(state.model.input.value, "echo abc");
            assert_eq!(state.model.phase, EditPhase::Ready);
            assert_eq!(state.capability, EditCapability::EchoMatched);
            assert!(app.can_use_smart_cursor_selection());
            app.clear_command_suggestion_draft(cx);
            assert_eq!(app.terminal.editing.predicted_input().value, "echo abc");
        });
    }

    #[test]
    fn submitted_marked_outer_command_does_not_poison_unmarked_child_input() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-outer-boundary");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"docker exec box sh", cx);
            app.terminal.seed_session_view(
                "s1".into(),
                "$ \x1b]133;B\x07docker exec box sh".into(),
                "UTF-8",
            );
            app.reconcile_shell_editing("s1", cx);
            app.note_shell_editing_input(b"\r", cx);
            app.terminal.assist.command_suggestions_suppressed = true;
            // Child typing may precede delivery of the outer shell's OSC C.
            app.note_shell_editing_input(b"echo abc", cx);
            app.terminal
                .seed_session_view("s1".into(), "\x1b]133;C\x07$ e".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(!app.terminal.editing.predicted_input().desynced);
            assert_eq!(app.terminal.editing.predicted_input().value, "echo abc");
            assert!(!app.can_use_smart_cursor_selection());
            app.terminal
                .seed_session_view("s1".into(), "$ echo abc".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert_eq!(
                app.terminal.editing.state().unwrap().capability,
                EditCapability::EchoMatched
            );
            assert!(app.can_use_smart_cursor_selection());
            assert!(app.terminal.assist.command_suggestions_suppressed);
        });
    }

    #[test]
    fn selection_is_retained_and_write_failure_keeps_input_and_selection() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-selection");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abcdef", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ abcdef".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            let selection = TerminalSelection::from_range(
                TerminalBufferCellPos::new(0, 3),
                TerminalBufferCellPos::new(0, 5),
            );
            app.terminal.selection.session_id = Some("s1".into());
            app.terminal.selection.selection = Some(selection);
            app.terminal.selection.dragging = true;
            app.begin_smart_input_selection();
            app.finish_terminal_selection(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    ..MouseUpEvent::default()
                },
                cx,
            );
            assert_eq!(app.terminal.selection.selection, Some(selection));
            let range = app.smart_cursor_selected_input_range().unwrap();
            assert_eq!((range.start, range.end), (1, 4));
            // The fixture deliberately has no live transport; a rejected write
            // consumes the edit gesture without sending a fallback character.
            assert!(app.replace_smart_input_selection(range, "X", cx));
            assert_eq!(app.terminal.editing.predicted_input().value, "abcdef");
            assert_eq!(app.terminal.selection.selection, Some(selection));
            assert_eq!(
                app.terminal.editing.state().unwrap().model.phase,
                EditPhase::Ready
            );
        });
    }

    #[test]
    fn partial_echo_cannot_confirm_and_ordinary_input_invalidates_edit_selection() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-echo");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abc", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ ab".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert_eq!(
                app.terminal.editing.state().unwrap().model.phase,
                EditPhase::Pending
            );
            assert!(!app.can_use_smart_cursor_selection());
            app.terminal
                .seed_session_view("s1".into(), "$ abc".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            app.terminal.selection.session_id = Some("s1".into());
            app.terminal.selection.selection = Some(TerminalSelection::from_range(
                TerminalBufferCellPos::new(0, 2),
                TerminalBufferCellPos::new(0, 3),
            ));
            app.begin_smart_input_selection();
            app.commit_smart_input_selection();
            assert!(app.smart_cursor_selected_input_range().is_some());
            app.note_shell_editing_input(b"d", cx);
            assert!(app.smart_cursor_selected_input_range().is_none());
            assert_eq!(app.terminal.editing.predicted_input().value, "abcd");
            app.terminal
                .seed_session_view("s1".into(), "$ abcd".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            app.commit_smart_input_selection();
            assert!(app.smart_cursor_selected_input_range().is_none());
        });
    }

    #[test]
    fn password_and_native_mouse_modes_cannot_authorize_shell_edits() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-gates");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abc", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ abc\x1b[?1000h".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(!app.can_use_smart_cursor_selection());
            app.terminal
                .seed_session_view("s1".into(), "$ abc".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.can_use_smart_cursor_selection());
            app.sync_input.toggle_broadcast_to_all();
            assert!(!app.can_use_smart_cursor_selection());
            app.sync_input.toggle_broadcast_to_all();
            app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
            app.note_shell_editing_input(b"secret", cx);
            assert!(app.terminal.editing.predicted_input().value.is_empty());
            assert!(!app.can_use_smart_cursor_selection());
        });
    }

    #[test]
    fn pending_clicks_keep_latest_target_and_timeout_cancels_it() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-queued");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abcd", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ abcd".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            let state = app.terminal.editing.state_mut();
            let plan =
                plan_edit(&state.model.input, state.model.version, EditIntent::Move(2)).unwrap();
            assert!(state.model.accept_plan(plan, Instant::now()));
            assert!(app.move_smart_input_cursor(1, cx));
            assert!(app.move_smart_input_cursor(0, cx));
            assert_eq!(
                app.terminal.editing.state().unwrap().model.queued_cursor,
                Some(0)
            );
            assert!(app.smart_cursor_selected_input_range().is_none());
            app.terminal.editing.state_mut().model.last_input_at =
                Some(Instant::now() - EDIT_CONFIRMATION_TIMEOUT);
            app.reconcile_shell_editing("s1", cx);
            let state = app.terminal.editing.state().unwrap();
            assert_eq!(state.model.phase, EditPhase::Uncertain);
            assert_eq!(state.model.queued_cursor, None);
            assert!(!app.can_use_smart_cursor_selection());
        });
    }

    #[test]
    fn snapshotless_output_cannot_reauthorize_edits_from_cached_cells() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-stale-frame");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abcd", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ abcd".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            let state = app.terminal.editing.state_mut();
            state.invalidate();
            state.awaiting_snapshot = true;
            app.reconcile_shell_editing("s1", cx);
            assert!(!app.can_use_smart_cursor_selection());
            assert_eq!(
                app.terminal.editing.state().unwrap().model.phase,
                EditPhase::Uncertain
            );
            app.terminal.editing.state_mut().awaiting_snapshot = false;
            app.terminal
                .seed_session_view("s1".into(), "$ abcd".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.can_use_smart_cursor_selection());
        });
    }

    #[test]
    fn ending_marked_input_cancels_queued_edit_instead_of_using_unmarked_echo() {
        let dir = TestConfigDir::new("nyaterm-shell-edit-boundary");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            app.note_shell_editing_input(b"abcd", cx);
            app.terminal
                .seed_session_view("s1".into(), "$ \x1b]133;B\x07abcd".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(app.can_use_smart_cursor_selection());
            app.terminal.editing.state_mut().model.phase = EditPhase::Pending;
            app.terminal.editing.state_mut().model.queued_cursor = Some(0);
            app.terminal
                .seed_session_view("s1".into(), "$ abcd\x1b]133;C\x07".into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            let state = app.terminal.editing.state().unwrap();
            assert_eq!(state.model.queued_cursor, None);
            assert_eq!(state.model.phase, EditPhase::Uncertain);
            assert!(state.model.input.desynced);
            assert!(!app.can_use_smart_cursor_selection());
        });
    }
}
