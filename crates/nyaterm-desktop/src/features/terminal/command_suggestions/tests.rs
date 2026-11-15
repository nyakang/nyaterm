use nyaterm_core::command_suggestion_suppression::CommandSuggestionSuppression;
pub(super) fn history_search_fixture(
    command: &str,
    input: &str,
    revision: u64,
) -> nyaterm_core::completion::types::CompletionResult {
    use nyaterm_core::completion::types::{
        CandidateKind, CandidateSource, CompletionCandidate, CompletionResult,
    };
    CompletionResult {
        revision,
        candidates: vec![CompletionCandidate {
            display: command.into(),
            insert_text: command.into(),
            description: String::new(),
            range: 0..input.len(),
            kind: CandidateKind::History,
            sources: vec![CandidateSource::History],
            use_count: 1,
            last_used_at_ms: 1,
            score: 1,
            indices: vec![],
            fillable: nyaterm_core::completion::types::supports_full_command_fill(command),
            context_priority: 1,
        }],
    }
}
use std::time::Duration;

use gpui::{AppContext as _, KeyDownEvent, TestAppContext};
use nyaterm_core::{TerminalInputState, apply_terminal_input_data};

use crate::features::NyaTermApp;
use crate::features::terminal::command_suggestions::state::{
    CommandSuggestionItem, CommandSuggestionState,
};
use crate::test_support::TestConfigDir;

#[test]
fn disabling_credential_autofill_releases_command_suggestions_and_smart_input() {
    let dir = TestConfigDir::new("nyaterm-credential-mode-cancel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        app.note_shell_editing_input(b"echo abc", cx);
        app.terminal
            .seed_session_view("s1".into(), "$ echo abc".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_some()
        );
        assert!(app.can_use_smart_cursor_selection());
        app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
        assert!(app.is_credential_prompt_input_mode());
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
        assert!(!app.can_use_smart_cursor_selection());

        // The toggle must release these gates without waiting for output or a timeout.
        app.toggle_credential_autofill(cx);
        assert!(!app.is_credential_prompt_input_mode());
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_some()
        );
        assert!(app.can_use_smart_cursor_selection());
    });
}

#[test]
fn uncertain_input_cannot_start_or_publish_a_suggestion_search() {
    let dir = TestConfigDir::new("nyaterm-suggestions-confidence");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        app.note_shell_editing_input(b"echo", cx);
        assert!(
            app.prepare_command_suggestion_search(
                app.terminal.assist.commands.search_generation,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx,
            )
            .is_none()
        );
        app.terminal
            .seed_session_view("s1".into(), "$ echo".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        let request = app
            .prepare_command_suggestion_search(
                app.terminal.assist.commands.search_generation,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx,
            )
            .expect("confirmed echoed input can request suggestions");
        app.terminal.editing.state_mut().model.invalidate();
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
        app.publish_command_suggestion_search(
            request,
            history_search_fixture(
                "echo example",
                "echo ex",
                app.commands.completion_runtime().revision(),
            ),
            Duration::ZERO,
            cx,
        );
        assert!(app.terminal.assist.commands.popup.is_none());
        app.terminal.assist.commands.popup = Some(open_suggestions_popup("s1"));
        assert!(app.apply_selected_command_suggestion(true, cx));
        assert!(app.terminal.assist.commands.pending_history_entry.is_none());
    });
}

#[test]
fn startup_layout_resize_keeps_first_line_input_eligible_for_suggestions() {
    let dir = TestConfigDir::new("nyaterm-suggestions-first-line-resize");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        // Selecting the new session activates editing state, then the first
        // paint fits the pane to its real bounds before the user can type.
        app.terminal.activate_shell_editing_session("s1");
        app.terminal.invalidate_shell_editing_session("s1");
        app.note_shell_editing_input(b"git", cx);
        assert!(!app.terminal.editing.predicted_input().desynced);
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
        app.terminal
            .seed_session_view("s1".into(), "$ git".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(app.command_suggestion_input_is_confirmed());
        assert!(
            app.prepare_command_suggestion_search(
                2,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_some()
        );
    });
}

#[test]
fn layout_resize_with_tracked_text_still_desyncs_the_line() {
    let dir = TestConfigDir::new("nyaterm-suggestions-resize-tracked");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        app.note_shell_editing_input(b"git", cx);
        app.terminal.invalidate_shell_editing_session("s1");
        app.note_shell_editing_input(b"x", cx);
        assert!(app.terminal.editing.predicted_input().desynced);
        assert!(
            app.prepare_command_suggestion_search(
                2,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
    });
}

#[test]
fn pager_space_without_command_text_cannot_start_suggestions() {
    let dir = TestConfigDir::new("nyaterm-suggestions-pager-space");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        // A remote pager consumes Space itself and echoes nothing, so the
        // local tracker holds only a separator while the screen repaints.
        app.note_shell_editing_input(b" ", cx);
        assert_eq!(app.terminal.editing.assist_input().value, " ");
        assert!(app.terminal.assist.commands.popup.is_none());
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );

        // The same signal hides a popup left over from an earlier line.
        app.terminal.assist.commands.popup = Some(open_suggestions_popup("s1"));
        app.note_command_suggestion_input(b" ", cx);
        assert!(app.terminal.assist.commands.popup.is_none());

        // A separator after real command text still offers suggestions.
        app.note_shell_editing_input(b"git ", cx);
        assert!(!app.command_suggestion_input_is_confirmed());
        app.terminal
            .seed_session_view("s1".into(), "$   git ".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(
            app.prepare_command_suggestion_search(
                2,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_some()
        );
    });
}

/// Deterministic replay of a real `su` round trip. Every step feeds the
/// screen text the remote would produce and drives the same gates the live
/// input path drives, so a rejection is localized to the step that causes it.
#[test]
fn su_round_trip_keeps_suggestions_available_across_prompts() {
    use nyaterm_core::completion::types::CompletionMode;

    let dir = TestConfigDir::new("nyaterm-suggestions-su-round-trip");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        macro_rules! output {
            ($screen:expr) => {{
                app.terminal
                    .seed_session_view("s1".into(), $screen.to_string(), "UTF-8");
                if let Some(view) = app.terminal.view.views.get_mut("s1") {
                    let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
                    view.frame_snapshot = Some(snapshot);
                }
                app.drain_pending_credential_autofill_detection(cx);
                app.reconcile_shell_editing("s1", cx);
            }};
        }
        let mut screen = String::from("user@host:~$ ");
        output!(screen);

        // 1. Login shell: type and submit `su - root`.
        app.note_shell_editing_input(b"su - root", cx);
        app.note_command_suggestion_input(b"su - root", cx);
        assert!(!app.command_suggestion_input_is_confirmed());
        screen.push_str("su - root");
        output!(screen);
        assert!(
            app.prepare_command_suggestion_search(1, CompletionMode::Token, cx)
                .is_some(),
            "suggestions must work before su"
        );
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);
        // bash preexec emits OSC 133 C for the foreground `su`.
        app.apply_shell_integration_edges("s1", true, false, true);

        // 2. su writes a no-echo password prompt; the password is never echoed.
        screen.push_str("\r\nPassword: ");
        output!(screen);
        app.note_shell_editing_input(b"password", cx);
        app.note_command_suggestion_input(b"password", cx);
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);

        // 3. Authentication succeeds; the child root shell owns the prompt.
        screen.push_str("\r\nroot@host:~# ");
        output!(screen);
        app.note_shell_editing_input(b"ls", cx);
        app.note_command_suggestion_input(b"ls", cx);
        screen.push_str("ls");
        output!(screen);
        assert!(
            app.prepare_command_suggestion_search(2, CompletionMode::Token, cx)
                .is_some(),
            "suggestions must work inside the child shell"
        );

        // 4. `ls` runs and the child prompt returns without any OSC 133 marks.
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);
        screen.push_str("\r\nfile1 file2\r\nroot@host:~# ");
        output!(screen);
        app.note_shell_editing_input(b"ls", cx);
        app.note_command_suggestion_input(b"ls", cx);
        screen.push_str("ls");
        output!(screen);
        assert!(
            app.prepare_command_suggestion_search(3, CompletionMode::Token, cx)
                .is_some(),
            "a second command inside the child shell must still suggest"
        );

        // 5. `exit` returns to the outer shell, which emits OSC 133 D.
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);
        app.note_shell_editing_input(b"exit", cx);
        app.note_command_suggestion_input(b"exit", cx);
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);
        screen.push_str("\r\nlogout\r\nuser@host:~$ ");
        app.terminal
            .seed_session_view("s1".into(), screen.clone(), "UTF-8");
        app.apply_shell_integration_edges("s1", false, true, false);
        app.drain_pending_credential_autofill_detection(cx);
        app.reconcile_shell_editing("s1", cx);
        app.note_shell_editing_input(b"ls", cx);
        app.note_command_suggestion_input(b"ls", cx);
        screen.push_str("ls");
        output!(screen);
        assert!(
            app.prepare_command_suggestion_search(4, CompletionMode::Token, cx)
                .is_some(),
            "suggestions must work after exiting the child shell"
        );
    });
}

/// Reproduces the reported `su` failure without an SSH server: a saved
/// credential makes the credential tick arm the no-echo input window at
/// `su`'s password prompt. The window must end when the child prompt is
/// visible, otherwise every later keystroke is discarded as password input.
#[test]
fn su_password_prompt_input_window_ends_at_the_child_prompt() {
    use nyaterm_core::completion::types::CompletionMode;

    let dir = TestConfigDir::new("nyaterm-suggestions-su-credential-window");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");

        // The credential tick ran at `su`'s no-echo password prompt and
        // armed the no-echo input window, remembering that prompt instance.
        app.terminal.seed_session_view(
            "s1".into(),
            "user@host:~$ su - root\r\nPassword: ".into(),
            "UTF-8",
        );
        if let Some(view) = app.terminal.view.views.get_mut("s1") {
            let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
            view.frame_snapshot = Some(snapshot);
        }
        app.sync_credential_autofill_for_test(cx);
        assert!(app.is_credential_prompt_input_mode());

        // Authentication succeeded; the prompt line changed and is no longer
        // a credential prompt. `observe_credential_prompt_instance` clears
        // the buffer during this tick.
        app.terminal.seed_session_view(
            "s1".into(),
            "user@host:~$ su - root\r\nPassword: \r\nroot@host:~# ".into(),
            "UTF-8",
        );
        if let Some(view) = app.terminal.view.views.get_mut("s1") {
            let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
            view.frame_snapshot = Some(snapshot);
        }
        app.sync_credential_autofill_for_test(cx);
        assert!(
            !app.is_credential_prompt_input_mode(),
            "the no-echo input window must end once the visible prompt is not a credential prompt"
        );

        // Typing at the child prompt must reach the suggestion gates.
        app.note_shell_editing_input(b"ls", cx);
        app.note_command_suggestion_input(b"ls", cx);
        app.terminal
            .seed_session_view("s1".into(), "root@host:~# ls".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(
            app.prepare_command_suggestion_search(1, CompletionMode::Token, cx)
                .is_some(),
            "suggestions must work in the child shell"
        );

        // `exit` returns to the outer shell. The window must stay closed:
        // this is the reported "still no suggestions after leaving su"
        // symptom, which a per-prompt re-arm would bring back.
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);
        app.note_shell_editing_input(b"exit", cx);
        app.note_command_suggestion_input(b"exit", cx);
        app.note_shell_editing_input(b"\r", cx);
        app.note_command_suggestion_input(b"\r", cx);
        app.terminal
            .seed_session_view("s1".into(), "logout\r\nuser@host:~$ ".into(), "UTF-8");
        if let Some(view) = app.terminal.view.views.get_mut("s1") {
            let snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
            view.frame_snapshot = Some(snapshot);
        }
        app.sync_credential_autofill_for_test(cx);
        app.reconcile_shell_editing("s1", cx);
        assert!(
            !app.is_credential_prompt_input_mode(),
            "the window must not reopen at the outer prompt after exit"
        );
        app.note_shell_editing_input(b"ls", cx);
        app.note_command_suggestion_input(b"ls", cx);
        app.terminal
            .seed_session_view("s1".into(), "logout\r\nuser@host:~$ ls".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(
            app.prepare_command_suggestion_search(2, CompletionMode::Token, cx)
                .is_some(),
            "suggestions must work after exiting the child shell"
        );
    });
}

#[test]
fn confirmed_prompt_echo_in_a_child_shell_releases_suppression() {
    let dir = TestConfigDir::new("nyaterm-suggestions-child-shell-resync");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        // `su - root` started. The outer shell sent OSC 133 C, while its D
        // can only arrive after the child exits, so the latch outlives it.
        app.terminal.assist.commands.suppression = CommandSuggestionSuppression::CommandRunning;
        app.note_shell_editing_input(b"git", cx);
        app.note_command_suggestion_input(b"git", cx);
        assert!(app.terminal.assist.commands.popup.is_none());

        // The root prompt echoes the locally predicted line back.
        app.terminal
            .seed_session_view("s1".into(), "root@host:~# git".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(!app.terminal.assist.commands.suppression.is_suppressed());
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_some()
        );
    });
}

#[test]
fn echo_without_a_shell_prompt_prefix_keeps_suppression() {
    let dir = TestConfigDir::new("nyaterm-suggestions-pager-prompt-resync");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        app.terminal.assist.commands.suppression = CommandSuggestionSuppression::CommandRunning;
        app.note_shell_editing_input(b"git", cx);

        // A pager status line and a child REPL echo the text, but neither
        // prefix is a shell prompt, so the suppression must survive.
        for seeded in ["/git", ">>> git"] {
            app.terminal
                .seed_session_view("s1".into(), seeded.into(), "UTF-8");
            app.reconcile_shell_editing("s1", cx);
            assert!(
                app.terminal.assist.commands.suppression.is_suppressed(),
                "{seeded}"
            );
        }
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
    });
}

#[test]
fn shell_like_pager_echo_cannot_release_explicit_interactive_suppression() {
    let dir = TestConfigDir::new("nyaterm-explicit-pager-suppression");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        app.terminal.assist.commands.pending_history_entry = Some("su - root".into());
        app.note_shell_editing_input(b"less", cx);
        app.note_shell_editing_input(b"\r", cx);
        app.apply_shell_integration_edges("s1", true, false, true);
        app.note_shell_editing_input(b"git", cx);
        app.terminal
            .seed_session_view("s1".into(), "root@host:~# git".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        assert!(app.command_suggestion_input_is_confirmed());
        assert_eq!(
            app.terminal.assist.commands.suppression,
            CommandSuggestionSuppression::InteractiveProgram
        );
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
    });
}

#[test]
fn published_candidates_require_explicit_selection_even_for_a_single_match() {
    let dir = TestConfigDir::new("nyaterm-suggestions-explicit-selection");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        app.note_shell_editing_input(b"docker r", cx);
        app.terminal
            .seed_session_view("s1".into(), "$ docker r".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        for mode in [
            nyaterm_core::completion::types::CompletionMode::Token,
            nyaterm_core::completion::types::CompletionMode::History,
        ] {
            for count in [1, 3] {
                let request_id = app.terminal.assist.commands.search_generation;
                let mut result = history_search_fixture(
                    "docker run",
                    "docker r",
                    app.commands.completion_runtime().revision(),
                );
                result
                    .candidates
                    .resize(count, result.candidates[0].clone());
                for _ in 0..2 {
                    let request = app
                        .prepare_command_suggestion_search(request_id, mode, cx)
                        .unwrap();
                    app.publish_command_suggestion_search(
                        request,
                        nyaterm_core::completion::types::CompletionResult {
                            candidates: result.candidates.clone(),
                            revision: result.revision,
                        },
                        Duration::ZERO,
                        cx,
                    );
                    let popup = app.terminal.assist.commands.popup.as_ref().unwrap();
                    assert_eq!(popup.items.len(), count);
                    assert_eq!(popup.selected_index, None);
                    let mut down = escape_key_event();
                    down.keystroke.key = "down".into();
                    assert!(app.handle_command_suggestion_key(&down, cx));
                    assert_eq!(
                        app.terminal
                            .assist
                            .commands
                            .popup
                            .as_ref()
                            .unwrap()
                            .selected_index,
                        Some(0),
                    );
                }
            }
        }
    });
}

fn escape_key_event() -> KeyDownEvent {
    KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: "escape".to_string(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

/// A visible surface makes the popup authoritative for the session, so the
/// key handler can only bail out for the reason under test.
pub(super) fn show_surface_for_session(app: &mut NyaTermApp, session_id: &str) {
    app.terminal.layout.session_surface_bounds.insert(
        session_id.to_string(),
        gpui::Bounds::new(
            gpui::point(gpui::px(0.0), gpui::px(0.0)),
            gpui::size(gpui::px(800.0), gpui::px(600.0)),
        ),
    );
}

fn open_suggestions_popup(session_id: &str) -> CommandSuggestionState {
    CommandSuggestionState {
        refreshing: false,
        mode: nyaterm_core::completion::types::CompletionMode::History,
        completion_revision: 0,
        scope: None,
        session_id: session_id.to_string(),
        input_version: 0,
        sync_revision: 0,
        draft: "1".to_string(),
        items: vec![CommandSuggestionItem {
            fillable: true,
            description: String::new(),
            command: "1111".to_string(),
            display: "1111".to_string(),
            source: "history".to_string(),
            score: 1,
            indices: Vec::new(),
        }],
        selected_index: Some(0),
    }
}

#[test]
fn selecting_suggestions_keeps_position_when_snapshot_history_window_changes() {
    let dir = TestConfigDir::new("nyaterm-suggestion-anchor");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");

    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        let view = app.terminal.view.views.get_mut("s1").unwrap();
        view.screen.resize(80, 24);
        view.screen.advance_decoded_text(&"output\r\n".repeat(100));
        view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));

        let (cursor_row, cursor_col) = app.active_terminal_cursor_cell();
        let mut state = open_suggestions_popup("s1");
        state.selected_index = None;
        app.terminal.assist.commands.popup = Some(state);
        let before = app
            .suggestion_overlay_position_for_session(
                Some("s1"),
                cursor_row,
                cursor_col,
                380.0,
                272.0,
            )
            .unwrap();

        // Worker frames may grow or shrink history padding without moving the cursor.
        for (older_rows, key) in [(64, "down"), (0, "up"), (96, "down")] {
            let view = app.terminal.view.views.get_mut("s1").unwrap();
            view.frame_snapshot = Some(std::sync::Arc::new(
                view.screen.viewport_snapshot_with_window(0, older_rows, 0),
            ));
            assert_eq!(app.active_terminal_cursor_cell(), (cursor_row, cursor_col));
            let mut event = escape_key_event();
            event.keystroke.key = key.to_string();
            assert!(app.handle_command_suggestion_key(&event, cx));
            let state = app.terminal.assist.commands.popup.as_ref().unwrap();
            assert_eq!(state.selected_index, (key == "down").then_some(0));
            let after = app
                .suggestion_overlay_position_for_session(
                    Some("s1"),
                    app.suggestion_cursor_cell_for_session("s1", cx).unwrap().0,
                    app.suggestion_cursor_cell_for_session("s1", cx).unwrap().1,
                    380.0,
                    272.0,
                )
                .unwrap();
            assert_eq!(after, before);
        }
    });
}

#[test]
fn suggestion_cursor_anchor_accounts_for_leading_cleared_rows() {
    let dir = TestConfigDir::new("nyaterm-suggestion-cleared-anchor");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, _| {
        let view = app.terminal.view.views.get_mut("s1").unwrap();
        view.screen.resize(80, 24);
        view.screen.advance(b"\x1b[24;1Hroot# draft");
        view.screen.clear_except_input();
        view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));

        assert_eq!(app.active_terminal_cursor_cell(), (0, 11));
    });
}

#[test]
fn alternate_screen_keeps_command_suggestions_from_intercepting_keys() {
    let dir = TestConfigDir::new("nyaterm-alt-screen-suggestions");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");

    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        assert!(!app.active_terminal_uses_alternate_screen());

        app.terminal.assist.commands.popup = Some(open_suggestions_popup("s1"));
        // Shell line: the popup owns Escape.
        assert!(app.handle_command_suggestion_key(&escape_key_event(), cx));
        assert!(app.terminal.assist.commands.popup.is_none());
    });

    cx.update_entity(&app, |app, _| {
        // A full-screen program (vim, less, top) takes over the terminal.
        app.terminal
            .view
            .views
            .get_mut("s1")
            .unwrap()
            .append_bytes_unprotected(b"\x1b[?1049h");
    });

    cx.update_entity(&app, |app, cx| {
        assert!(app.active_terminal_uses_alternate_screen());

        app.terminal.assist.commands.popup = Some(open_suggestions_popup("s1"));
        // Escape must reach the program instead of closing a stale popup.
        assert!(!app.handle_command_suggestion_key(&escape_key_event(), cx));
        assert!(app.terminal.assist.commands.popup.is_none());

        // Typing inside the program neither suggests nor records history.
        app.note_command_suggestion_input(b"11", cx);
        assert!(app.terminal.assist.commands.popup.is_none());
        assert_eq!(
            app.terminal.editing.predicted_input(),
            &TerminalInputState::new()
        );

        *app.terminal.editing.input_mut() =
            apply_terminal_input_data(&TerminalInputState::new(), "vim ");
        app.note_shell_editing_input(b"a.txt\r", cx);
        assert!(app.terminal.assist.commands.pending_history_entry.is_none());
        assert_eq!(
            app.terminal.editing.predicted_input(),
            &TerminalInputState::new()
        );
    });
}

#[test]
fn alternate_screen_blocks_the_manual_trigger_and_the_deferred_publisher() {
    let dir = TestConfigDir::new("nyaterm-alt-screen-manual");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");

    cx.update_entity(&app, |app, _| {
        show_surface_for_session(app, "s1");
        app.terminal
            .view
            .views
            .get_mut("s1")
            .unwrap()
            .append_bytes_unprotected(b"\x1b[?1049h");
    });

    cx.update_entity(&app, |app, cx| {
        assert!(app.active_terminal_uses_alternate_screen());
        *app.terminal.editing.input_mut() =
            apply_terminal_input_data(&TerminalInputState::new(), "vim 1");

        // An explicit trigger must not open the popup inside a full-screen
        // program, even with a trackable shell-style line.
        app.show_manual_command_suggestions(cx);
        assert!(app.terminal.assist.commands.popup.is_none());

        // A search deferred on the shell line would otherwise land after the
        // program has taken the screen; the request must be dropped.
        assert!(
            app.prepare_command_suggestion_search(
                1,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx
            )
            .is_none()
        );
        assert!(app.terminal.assist.commands.popup.is_none());
    });
}

#[test]
fn alternate_screen_drops_a_search_that_was_already_in_flight() {
    let dir = TestConfigDir::new("nyaterm-alt-screen-inflight");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");

    let request = cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        assert!(!app.active_terminal_uses_alternate_screen());
        app.note_shell_editing_input(b"vim 1", cx);
        app.terminal
            .seed_session_view("s1".into(), "$ vim 1".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        app.prepare_command_suggestion_search(
            app.terminal.assist.commands.search_generation,
            nyaterm_core::completion::types::CompletionMode::Token,
            cx,
        )
        .expect("shell line should still build a request")
    });

    // The user starts a full-screen program while the search is running.
    cx.update_entity(&app, |app, _| {
        app.terminal
            .view
            .views
            .get_mut("s1")
            .unwrap()
            .append_bytes_unprotected(b"\x1b[?1049h");
    });

    cx.update_entity(&app, |app, cx| {
        assert!(app.active_terminal_uses_alternate_screen());
        let results = history_search_fixture(
            "vim 1111",
            "vim 1",
            app.commands.completion_runtime().revision(),
        );
        app.publish_command_suggestion_search(request, results, Duration::ZERO, cx);
        assert!(app.terminal.assist.commands.popup.is_none());
    });
}

#[test]
fn leaving_the_alternate_screen_restores_shell_line_suggestions() {
    let dir = TestConfigDir::new("nyaterm-alt-screen-return");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");

    cx.update_entity(&app, |app, _| {
        app.terminal
            .view
            .views
            .get_mut("s1")
            .unwrap()
            .append_bytes_unprotected(b"\x1b[?1049h");
    });
    cx.update_entity(&app, |app, _| {
        assert!(app.active_terminal_uses_alternate_screen());
        app.terminal
            .view
            .views
            .get_mut("s1")
            .unwrap()
            .append_bytes_unprotected(b"\x1b[?1049l");
    });

    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        assert!(!app.active_terminal_uses_alternate_screen());
        app.terminal.assist.commands.popup = Some(open_suggestions_popup("s1"));
        assert!(app.handle_command_suggestion_key(&escape_key_event(), cx));
    });
}
#[test]
fn revision_only_changes_retry_search_but_shell_context_changes_discard_it() {
    let dir = TestConfigDir::new("nyaterm-suggestion-revision-retry");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app =
        crate::features::test_support::app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        show_surface_for_session(app, "s1");
        app.note_shell_editing_input(b"echo ex", cx);
        app.terminal
            .seed_session_view("s1".into(), "$ echo ex".into(), "UTF-8");
        app.reconcile_shell_editing("s1", cx);
        let generation = app.terminal.assist.commands.search_generation;
        let request = app
            .prepare_command_suggestion_search(
                generation,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx,
            )
            .unwrap();
        let stale = app.commands.completion_runtime().revision().wrapping_sub(1);
        app.publish_command_suggestion_search(
            request,
            history_search_fixture("echo example", "echo ex", stale),
            Duration::ZERO,
            cx,
        );
        assert_ne!(app.terminal.assist.commands.search_generation, generation);
        assert!(app.terminal.assist.commands.popup.is_none());
        let generation = app.terminal.assist.commands.search_generation;
        let mut request = app
            .prepare_command_suggestion_search(
                generation,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx,
            )
            .unwrap();
        request.context.command_events = !request.context.command_events;
        let current = app.commands.completion_runtime().revision();
        app.publish_command_suggestion_search(
            request,
            history_search_fixture("echo example", "echo ex", current),
            Duration::ZERO,
            cx,
        );
        assert_eq!(app.terminal.assist.commands.search_generation, generation);
        assert!(app.terminal.assist.commands.popup.is_none());
    });
}
