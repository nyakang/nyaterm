use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context, Entity, KeyDownEvent, KeyUpEvent, TestAppContext};
use nyaterm_core::terminal::editing::EditPhase;
use nyaterm_transport::{SessionManager, TelnetSessionConfig};

use super::fill::SuggestionSubmitResult;
use super::tests::show_surface_for_session;
use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{CommandSuggestionItem, CommandSuggestionState, SessionLaunchConfig};
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct Fixture {
    cx: TestAppContext,
    app: Entity<NyaTermApp>,
    peer: TcpStream,
    manager: Arc<SessionManager>,
    id: String,
    _cleanup: TestAppCleanup,
    _dir: TestConfigDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = TestConfigDir::new("nyaterm-suggestion-fill");
        let mut cx = TestAppContext::single();
        let cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "fixture");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let (manager, id) = cx.update_entity(&app, |app, _| {
            let config = TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp: true,
                ..TelnetSessionConfig::default()
            };
            let manager = app.session.manager_handle();
            let info = manager.create_telnet_session(config.clone()).unwrap();
            let mut metadata = app.session.metadata("fixture").unwrap().clone();
            metadata.launch_config = SessionLaunchConfig::Telnet(config);
            app.session.register_session_metadata(&info.id, metadata);
            app.session.select_active_session(&info.id);
            app.terminal.activate_shell_editing_session(&info.id);
            show_surface_for_session(app, &info.id);
            (manager, info.id)
        });
        let (peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        Self {
            cx,
            app,
            peer,
            manager,
            id,
            _cleanup: cleanup,
            _dir: dir,
        }
    }

    fn update<R>(
        &mut self,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>, &str) -> R,
    ) -> R {
        let id = self.id.clone();
        self.cx.update_entity(&self.app, |app, cx| f(app, cx, &id))
    }

    fn suggest(&mut self, current: &str, target: &str) {
        self.update(|app, cx, id| {
            app.terminal.editing.clear_active();
            app.terminal
                .seed_session_view(id.into(), String::new(), "UTF-8");
            app.note_shell_editing_input(current.as_bytes(), cx);
            echo(app, cx, id, current);
            assert_eq!(
                app.terminal.editing.state().unwrap().model.phase,
                EditPhase::Ready
            );
            popup(app, id, target);
        });
    }

    fn receive(&mut self, expected: &[u8]) {
        self.peer
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut actual = vec![0; expected.len()];
        self.peer.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    fn assert_quiet(&mut self) {
        self.peer
            .set_read_timeout(Some(Duration::from_millis(60)))
            .unwrap();
        let error = self
            .peer
            .read(&mut [0])
            .expect_err("no additional wire input");
        assert!(matches!(
            error.kind(),
            ErrorKind::TimedOut | ErrorKind::WouldBlock
        ));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.manager.close(&self.id);
    }
}

fn key(name: &str) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: gpui::Keystroke {
            key: name.into(),
            key_char: None,
            modifiers: gpui::Modifiers::default(),
        },
        is_held: false,
        prefer_character_input: false,
    }
}

fn echo(app: &mut NyaTermApp, cx: &mut Context<NyaTermApp>, id: &str, value: &str) {
    // A network CLI prompt provides a unique input mapping, not Readline evidence.
    app.terminal
        .seed_session_view(id.into(), format!("<device>{value}"), "UTF-8");
    app.reconcile_shell_editing(id, cx);
}

fn popup(app: &mut NyaTermApp, id: &str, target: &str) {
    let state = app.terminal.editing.state().unwrap();
    app.terminal.assist.command_suggestions = Some(CommandSuggestionState {
        session_id: id.into(),
        input_version: state.model.version,
        sync_revision: app.sync_input.revision(),
        draft: state.model.input.value.clone(),
        items: vec![CommandSuggestionItem {
            command: target.into(),
            display: target.into(),
            source: "history".into(),
            score: 1,
            indices: Vec::new(),
        }],
        selected_index: Some(0),
        cursor_row: 0,
        cursor_col: 0,
    });
}

#[test]
fn tab_and_enter_only_send_the_missing_suffix_and_confirm_once() {
    for execute in [false, true] {
        let mut f = Fixture::new();
        f.suggest("display ver", "display version");
        f.update(|app, cx, id| {
            let version = app.terminal.editing.state().unwrap().model.version;
            assert!(
                app.handle_command_suggestion_key(&key(if execute { "enter" } else { "tab" }), cx)
            );
            let state = app.terminal.editing.state().unwrap();
            assert_eq!(state.model.input.value, "display version");
            assert_eq!(state.model.version, version + 1);
            assert_eq!(state.model.phase, EditPhase::Pending);
            assert!(state.suggestion_fill.is_some());
            assert!(app.session.command_history_for(id).is_none());
            assert!(app.terminal.assist.pending_command_history_entry.is_none());
            echo(app, cx, id, "display versi");
            assert_eq!(
                app.terminal.editing.state().unwrap().model.phase,
                EditPhase::Pending
            );
            echo(app, cx, id, "display version");
            assert!(
                app.terminal
                    .editing
                    .state()
                    .unwrap()
                    .suggestion_fill
                    .is_none()
            );
            assert_eq!(
                app.terminal.editing.state().unwrap().model.phase,
                EditPhase::Ready
            );
            app.reconcile_shell_editing(id, cx);
        });
        f.receive(b"sion");
        f.assert_quiet();
    }
}

#[test]
fn an_independent_manual_enter_uses_session_encoding_and_records_once() {
    for kitty in [false, true] {
        let mut f = Fixture::new();
        f.suggest("display ver", "display version");
        let expected = f.update(|app, cx, id| {
            let press = key("enter");
            assert!(app.handle_command_suggestion_key(&press, cx));
            let mut repeat = press.clone();
            repeat.is_held = true;
            assert!(!app.send_terminal_key_event(&repeat, true, cx));
            assert!(!app.send_terminal_key_release_event(
                &KeyUpEvent {
                    keystroke: press.keystroke.clone()
                },
                cx
            ));
            echo(app, cx, id, "display version");
            if kitty {
                app.terminal
                    .view
                    .views
                    .get_mut(id)
                    .unwrap()
                    .protocol_state
                    .kitty_keyboard_report_all_keys_as_esc = true;
            }
            let expected = app
                .terminal_key_bytes_for_event_for_session(Some(id), &press)
                .unwrap();
            assert!(app.send_terminal_key_event(&press, true, cx));
            assert_eq!(
                app.session.command_history_for(id).unwrap(),
                ["display version"]
            );
            assert!(app.terminal.assist.pending_command_history_entry.is_none());
            expected
        });
        f.receive(b"sion");
        f.receive(&expected);
        f.assert_quiet();
    }
}

#[test]
fn identical_input_writes_nothing_and_replacement_is_rejected() {
    let mut f = Fixture::new();
    for execute in [false, true] {
        f.suggest("display version", "display version");
        f.update(|app, cx, _| {
            assert_eq!(
                app.submit_selected_command_suggestion(execute, cx),
                SuggestionSubmitResult::NoChange
            );
            assert!(
                app.terminal
                    .editing
                    .state()
                    .unwrap()
                    .suggestion_fill
                    .is_none()
            );
        });
        f.suggest("show ver", "display version");
        f.update(|app, cx, _| {
            assert!(
                app.handle_command_suggestion_key(&key(if execute { "enter" } else { "tab" }), cx)
            );
            assert_eq!(app.terminal.editing.predicted_input().value, "show ver");
        });
    }
    f.assert_quiet();
}

#[test]
fn stale_versions_and_unsafe_terminal_states_consume_acceptance_without_writing() {
    let mut f = Fixture::new();
    for scenario in 0..11 {
        f.suggest("display ver", "display version");
        f.update(|app, cx, id| {
            match scenario {
                0 => app.terminal.editing.state_mut().model.version += 1,
                1 => app.terminal.editing.state_mut().awaiting_snapshot = true,
                2 => {
                    app.sync_input.toggle_broadcast_to_all();
                }
                3 => {
                    app.sync_input.toggle_broadcast_to_all();
                    app.sync_input.toggle_broadcast_to_all();
                }
                4 => app.terminal.assist.credential_prompt_input_until_ms = u64::MAX,
                5 => {
                    app.terminal
                        .view
                        .views
                        .get_mut(id)
                        .unwrap()
                        .protocol_state
                        .mouse_reporting = true
                }
                6 => {
                    app.terminal
                        .view
                        .views
                        .get_mut(id)
                        .unwrap()
                        .protocol_state
                        .alternate_screen = true
                }
                7 => {
                    app.session.select_active_session("fixture");
                }
                8 => {
                    app.terminal
                        .editing
                        .state_mut()
                        .model
                        .input
                        .line_rewrite_required = true
                }
                9 => app.terminal.editing.state_mut().model.input.desynced = true,
                10 => app.security.activate_screen_lock("test lock".into()),
                _ => unreachable!(),
            }
            assert!(
                app.handle_command_suggestion_key(&key("enter"), cx),
                "scenario {scenario}"
            );
            assert!(app.terminal.assist.command_suggestions.is_none());
            assert!(app.terminal.assist.pending_command_history_entry.is_none());
            if app.sync_input.broadcast_to_all() {
                app.sync_input.toggle_broadcast_to_all();
            }
            app.terminal.assist.credential_prompt_input_until_ms = 0;
            app.security.deactivate_screen_lock();
            app.session.select_active_session(id);
            app.terminal.editing.state_mut().awaiting_snapshot = false;
        });
    }
    f.assert_quiet();
}

#[test]
fn fill_requests_cancel_on_input_boundaries_and_late_echo_never_submits() {
    let mut f = Fixture::new();
    for scenario in 0..9 {
        f.suggest("display ver", "display version");
        f.update(|app, cx, id| {
            assert_eq!(
                app.submit_selected_command_suggestion(true, cx),
                SuggestionSubmitResult::Submitted
            );
            match scenario {
                0 => {
                    app.terminal
                        .editing
                        .state_mut()
                        .suggestion_fill
                        .as_mut()
                        .unwrap()
                        .deadline = Instant::now();
                }
                1 => app.note_shell_editing_input(b"x", cx),
                2 => app.note_shell_editing_input(b"\x03", cx),
                3 => {
                    app.terminal.activate_shell_editing_session("fixture");
                    app.terminal.activate_shell_editing_session(id);
                }
                4 => app.terminal.invalidate_shell_editing_session(id),
                5 => {
                    app.sync_input.toggle_broadcast_to_all();
                    app.sync_input.toggle_broadcast_to_all();
                }
                6 => {
                    app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
                    app.reconcile_shell_editing(id, cx);
                    app.terminal.assist.credential_prompt_input_until_ms = 0;
                }
                7 => {
                    app.terminal
                        .view
                        .views
                        .get_mut(id)
                        .unwrap()
                        .protocol_state
                        .alternate_screen = true;
                    app.reconcile_shell_editing(id, cx);
                    app.terminal
                        .view
                        .views
                        .get_mut(id)
                        .unwrap()
                        .protocol_state
                        .alternate_screen = false;
                }
                8 => {
                    app.security.activate_screen_lock("test lock".into());
                    app.reconcile_shell_editing(id, cx);
                    app.security.deactivate_screen_lock();
                }
                _ => unreachable!(),
            }
            echo(app, cx, id, "display version");
            assert!(
                app.terminal
                    .editing
                    .state()
                    .unwrap()
                    .suggestion_fill
                    .is_none()
            );
            assert!(app.session.command_history_for(id).is_none());
        });
        f.receive(b"sion");
    }
    f.assert_quiet();
}

#[test]
fn encoding_and_bracketed_paste_never_add_control_sequences_to_the_suffix() {
    let mut f = Fixture::new();
    f.suggest("echo 中", "echo 中文 ");
    f.update(|app, cx, id| {
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .kitty_keyboard_report_all_keys_as_esc = true;
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::Submitted
        );
    });
    f.receive("文 ".as_bytes());
    f.assert_quiet();

    f.suggest("echo ", "echo 😀");
    f.update(|app, cx, id| {
        let mut metadata = app.session.metadata(id).unwrap().clone();
        if let SessionLaunchConfig::Telnet(config) = &mut metadata.launch_config {
            config.encoding = "GBK".into();
        }
        app.session.register_session_metadata(id, metadata);
        assert_eq!(
            app.submit_selected_command_suggestion(true, cx),
            SuggestionSubmitResult::WriteFailed
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "echo ");
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
        assert!(app.terminal.assist.pending_command_history_entry.is_none());
    });
    f.assert_quiet();
}

#[test]
fn failed_transport_write_leaves_no_fill_prediction_or_history() {
    let mut f = Fixture::new();
    f.suggest("display ver", "display version");
    f.manager.close(&f.id).unwrap();
    f.update(|app, cx, id| {
        assert_eq!(
            app.submit_selected_command_suggestion(true, cx),
            SuggestionSubmitResult::WriteFailed
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "display ver");
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
        assert!(app.session.command_history_for(id).is_none());
    });
}

#[test]
fn search_results_cannot_reuse_a_pattern_after_its_input_version_changes() {
    let mut f = Fixture::new();
    f.suggest("display ver", "display version");
    f.update(|app, cx, _| {
        app.terminal.assist.command_suggestions = None;
        let generation = app.terminal.assist.command_suggestion_search_gen;
        let request = app
            .prepare_command_suggestion_search(generation, cx)
            .unwrap();
        let model = &mut app.terminal.editing.state_mut().model;
        model.note_input("\x1b[D", Instant::now());
        model.note_input("\x1b[C", Instant::now());
        app.publish_command_suggestion_search(
            request,
            vec![nyaterm_core::FuzzyResult {
                command: "display version".into(),
                display: "display version".into(),
                source: "history".into(),
                score: 1,
                indices: Vec::new(),
            }],
            Duration::ZERO,
            cx,
        );
        assert!(app.terminal.assist.command_suggestions.is_none());
    });
    f.assert_quiet();
}

#[test]
fn no_selection_preserves_normal_enter_and_tab_handling() {
    let mut f = Fixture::new();
    f.suggest("display ver", "display version");
    f.update(|app, cx, _| {
        app.terminal
            .assist
            .command_suggestions
            .as_mut()
            .unwrap()
            .selected_index = None;
        assert_eq!(
            app.submit_selected_command_suggestion(true, cx),
            SuggestionSubmitResult::NotHandled
        );
        assert!(!app.handle_command_suggestion_key(&key("enter"), cx));
        assert!(!app.handle_command_suggestion_key(&key("tab"), cx));
    });
    f.assert_quiet();
}

#[test]
fn confirmation_timeout_task_cancels_a_fill_before_late_echo() {
    let mut f = Fixture::new();
    f.suggest("display ver", "display version");
    f.update(|app, cx, _| {
        assert_eq!(
            app.submit_selected_command_suggestion(true, cx),
            SuggestionSubmitResult::Submitted
        );
        app.terminal.editing.state_mut().model.last_input_at =
            Some(Instant::now() - Duration::from_secs(3));
    });
    f.cx.run_until_parked();
    f.cx.executor().advance_clock(Duration::from_secs(3));
    f.cx.run_until_parked();
    f.update(|app, cx, id| {
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
        assert_eq!(
            app.terminal.editing.state().unwrap().model.phase,
            EditPhase::Uncertain
        );
        echo(app, cx, id, "display version");
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
        assert!(app.session.command_history_for(id).is_none());
    });
    f.receive(b"sion");
    f.assert_quiet();
}

#[test]
fn a_different_echo_cancels_the_request_without_sending_recovery_keys() {
    let mut f = Fixture::new();
    f.suggest("display ver", "display version");
    f.update(|app, cx, id| {
        assert_eq!(
            app.submit_selected_command_suggestion(true, cx),
            SuggestionSubmitResult::Submitted
        );
        echo(app, cx, id, "unexpected");
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
        echo(app, cx, id, "display version");
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
    });
    f.receive(b"sion");
    f.assert_quiet();
}
