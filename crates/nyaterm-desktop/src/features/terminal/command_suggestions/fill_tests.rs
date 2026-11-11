use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Context, Entity, IntoElement, KeyDownEvent, KeyUpEvent, Render,
    TestAppContext, Window,
};
use nyaterm_core::completion::types::{CompletionFillPolicy, CompletionMode};
use nyaterm_core::terminal::editing::{EDIT_CONFIRMATION_TIMEOUT, EditPhase};
use nyaterm_transport::{SerialSessionConfig, SessionManager, TelnetSessionConfig};

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

struct PopupView(Entity<NyaTermApp>);

impl Render for PopupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |app, cx| {
            app.command_suggestions_overlay(cx).into_any_element()
        })
    }
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

    fn suggest_in_shell(&mut self, current: &str, target: &str) {
        if current.is_empty() {
            self.update(|app, _, _| app.terminal.editing.clear_active());
        } else {
            self.suggest(current, target);
        }
        self.update(|app, cx, id| {
            let launch = app
                .session
                .metadata("fixture")
                .unwrap()
                .launch_config
                .clone();
            let mut metadata = app.session.metadata(id).unwrap().clone();
            metadata.launch_config = launch;
            app.session.register_session_metadata(id, metadata);
            app.terminal.seed_session_view(
                id.into(),
                format!("$ \x1b]133;B\x07{current}"),
                "UTF-8",
            );
            app.reconcile_shell_editing(id, cx);
            assert!(app.terminal.editing.state().unwrap().allows_editing());
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

    fn with_window<R>(
        &mut self,
        action: impl FnOnce(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>, &str) -> R,
    ) -> R {
        let app = self.app.clone();
        let host = app.clone();
        let id = self.id.clone();
        let (_, cx) = self.cx.add_window_view(|_, _| PopupView(host));
        cx.update(|window, cx| app.update(cx, |app, cx| action(app, window, cx, &id)))
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

#[test]
fn expanded_prompts_offer_replacement_in_shell_sessions_and_append_in_telnet_and_serial() {
    let mut f = Fixture::new();
    for kind in 0..4 {
        for prompt in [
            "fuxi:/sdcard # ",
            "host:~$ ",
            "~/src ❯ ",
            "➜ ~/src ",
            "~ % ",
        ] {
            f.suggest("mv", "mv source destination");
            f.update(|app, cx, id| {
                let launch = match kind {
                    0 => app
                        .session
                        .metadata("fixture")
                        .unwrap()
                        .launch_config
                        .clone(),
                    1 => SessionLaunchConfig::Ssh(Box::default()),
                    2 => SessionLaunchConfig::Telnet(TelnetSessionConfig::default()),
                    _ => SessionLaunchConfig::Serial(SerialSessionConfig::default()),
                };
                app.session.metadata_mut(id).unwrap().launch_config = launch;
                app.terminal
                    .seed_session_view(id.into(), format!("{prompt}mv"), "UTF-8");
                app.reconcile_shell_editing(id, cx);
                let expected = if kind < 2 {
                    CompletionFillPolicy::ReplaceAllowed
                } else {
                    CompletionFillPolicy::AppendOnly
                };
                assert_eq!(app.command_suggestion_fill_policy(id), expected, "{prompt}");
                let request = app
                    .prepare_command_suggestion_search(
                        app.terminal.assist.command_suggestion_search_gen,
                        CompletionMode::Token,
                        cx,
                    )
                    .unwrap();
                let revision = app.commands.completion_runtime().revision();
                let mut results = super::tests::history_search_fixture("cat file", "mv", revision);
                results.candidates.extend(
                    super::tests::history_search_fixture("mv source destination", "mv", revision)
                        .candidates,
                );
                results.candidates.extend(
                    super::tests::history_search_fixture("echo first\necho second", "mv", revision)
                        .candidates,
                );
                app.publish_command_suggestion_search(request, results, Duration::ZERO, cx);
                let popup = app.terminal.assist.command_suggestions.as_ref().unwrap();
                assert_eq!(popup.items.len(), if kind < 2 { 3 } else { 2 });
                assert!(popup.items.iter().all(|item| item.fillable));
                assert_eq!(
                    popup.items.iter().any(|item| item.command == "cat file"),
                    kind < 2
                );
                assert!(popup.items.iter().any(|item| item.command.contains('\n')));
            });
        }
    }
    f.assert_quiet();
}

#[test]
fn android_shell_prompt_replacement_writes_only_edit_keys_and_waits_for_confirmation() {
    let mut f = Fixture::new();
    f.suggest("mv", "mv source destination");
    f.update(|app, cx, id| {
        app.session.metadata_mut(id).unwrap().launch_config = app
            .session
            .metadata("fixture")
            .unwrap()
            .launch_config
            .clone();
        app.terminal
            .seed_session_view(id.into(), "fuxi:/sdcard # mv".into(), "UTF-8");
        app.reconcile_shell_editing(id, cx);
        popup(app, id, "cat file");
        assert_eq!(
            app.submit_selected_command_suggestion(true, cx),
            SuggestionSubmitResult::Submitted
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "cat file");
        assert_eq!(
            app.terminal.editing.state().unwrap().model.phase,
            EditPhase::Pending
        );
        assert!(app.session.command_history_for(id).is_none());
    });
    f.receive(b"\x7f\x7fcat file");
    f.assert_quiet();
}

#[test]
fn delayed_echo_hides_candidates_then_schedules_a_refresh_when_input_is_confirmed() {
    let mut f = Fixture::new();
    f.suggest_in_shell("mv", "mv source destination");
    f.update(|app, cx, id| {
        app.note_shell_editing_input(b"x", cx);
        app.terminal.assist.command_suggestion_refresh_task = None;
        assert!(app.terminal.assist.command_suggestions.is_none());
        assert!(
            app.prepare_command_suggestion_search(
                app.terminal.assist.command_suggestion_search_gen,
                CompletionMode::Token,
                cx
            )
            .is_none()
        );
        echo(app, cx, id, "mvx");
        assert!(app.command_suggestion_input_is_confirmed());
        assert!(
            app.terminal
                .assist
                .command_suggestion_refresh_task
                .is_some()
        );
    });
    f.assert_quiet();
}

#[test]
fn unconfirmed_or_nonterminal_line_input_never_enters_candidate_search() {
    let mut f = Fixture::new();
    for scenario in 0..8 {
        f.suggest_in_shell("mv", "mv source destination");
        f.update(|app, cx, _| {
            let state = app.terminal.editing.state_mut();
            match scenario {
                0 => state.awaiting_snapshot = true,
                1 => state.mapping = None,
                2 => state.model.phase = EditPhase::Pending,
                3 => state.model.input.cursor = 0,
                4 => state.model.input.multiline = true,
                5 => state.model.input.paste_mode = true,
                6 => state.model.input.line_rewrite_required = true,
                _ => state.model.input.desynced = true,
            }
            app.reconcile_command_suggestion_availability(cx);
            assert!(app.terminal.assist.command_suggestions.is_none());
            assert!(
                app.prepare_command_suggestion_search(
                    app.terminal.assist.command_suggestion_search_gen,
                    CompletionMode::Token,
                    cx
                )
                .is_none()
            );
            app.terminal.editing.state_mut().awaiting_snapshot = false;
        });
    }
    f.assert_quiet();
}

#[test]
fn restricted_terminal_states_never_publish_or_select_candidates() {
    let mut f = Fixture::new();
    for scenario in 0..8 {
        f.suggest_in_shell("mv", "mv source destination");
        f.update(|app, cx, id| {
            let request = app
                .prepare_command_suggestion_search(
                    app.terminal.assist.command_suggestion_search_gen,
                    CompletionMode::Token,
                    cx,
                )
                .unwrap();
            match scenario {
                0 => {
                    app.toggle_broadcast_to_all(cx);
                }
                1 => app.terminal.assist.credential_prompt_input_until_ms = u64::MAX,
                2 => app.security.activate_screen_lock("test lock".into()),
                3 => {
                    app.terminal
                        .view
                        .views
                        .get_mut(id)
                        .unwrap()
                        .protocol_state
                        .alternate_screen = true
                }
                4 => {
                    app.terminal
                        .view
                        .views
                        .get_mut(id)
                        .unwrap()
                        .protocol_state
                        .mouse_reporting = true
                }
                5 => app.session.metadata_mut(id).unwrap().disconnected = true,
                6 => {
                    app.session.select_active_session("fixture");
                }
                _ => {
                    let view = app.terminal.view.views.get_mut(id).unwrap();
                    view.append_bytes_unprotected("\r\n".repeat(80).as_bytes());
                    view.scroll_offset = 1;
                    assert_eq!(app.active_terminal_display_offset(), 1);
                }
            }
            app.reconcile_command_suggestion_availability(cx);
            assert!(app.terminal.assist.command_suggestions.is_none());
            app.publish_command_suggestion_search(
                request,
                super::tests::history_search_fixture(
                    "mv source destination",
                    "mv",
                    app.commands.completion_runtime().revision(),
                ),
                Duration::ZERO,
                cx,
            );
            assert!(app.terminal.assist.command_suggestions.is_none());
            assert!(
                app.prepare_command_suggestion_search(
                    app.terminal.assist.command_suggestion_search_gen,
                    CompletionMode::Token,
                    cx
                )
                .is_none()
            );
            if app.sync_input.broadcast_to_all() {
                app.toggle_broadcast_to_all(cx);
            }
            app.terminal.assist.credential_prompt_input_until_ms = 0;
            app.security.deactivate_screen_lock();
            app.session.metadata_mut(id).unwrap().disconnected = false;
            let view = app.terminal.view.views.get_mut(id).unwrap();
            view.protocol_state.alternate_screen = false;
            view.protocol_state.mouse_reporting = false;
            view.scroll_offset = 0;
            app.session.select_active_session(id);
        });
    }
    f.assert_quiet();
}

#[test]
fn policy_changes_filter_existing_candidates_and_discard_pending_results() {
    let mut f = Fixture::new();
    f.suggest_in_shell("mv", "cat file");
    f.update(|app, cx, id| {
        let request = app
            .prepare_command_suggestion_search(
                app.terminal.assist.command_suggestion_search_gen,
                CompletionMode::Token,
                cx,
            )
            .unwrap();
        app.session.metadata_mut(id).unwrap().launch_config =
            SessionLaunchConfig::Serial(SerialSessionConfig::default());
        app.reconcile_command_suggestion_availability(cx);
        assert!(app.terminal.assist.command_suggestions.is_none());
        app.publish_command_suggestion_search(
            request,
            super::tests::history_search_fixture(
                "cat file",
                "mv",
                app.commands.completion_runtime().revision(),
            ),
            Duration::ZERO,
            cx,
        );
        assert!(app.terminal.assist.command_suggestions.is_none());
        assert!(
            app.terminal
                .assist
                .command_suggestion_refresh_task
                .is_some()
        );
    });
    f.assert_quiet();
}

#[test]
fn tightening_policy_preserves_selection_of_a_remaining_append_candidate() {
    let mut f = Fixture::new();
    f.suggest_in_shell("mv", "cat file");
    f.update(|app, cx, id| {
        let popup = app.terminal.assist.command_suggestions.as_mut().unwrap();
        let mut append = popup.items[0].clone();
        append.command = "mv source destination".into();
        append.display = append.command.clone();
        popup.items.push(append);
        popup.selected_index = Some(1);
        app.session.metadata_mut(id).unwrap().launch_config =
            SessionLaunchConfig::Telnet(TelnetSessionConfig::default());
        app.reconcile_command_suggestion_availability(cx);
        let popup = app.terminal.assist.command_suggestions.as_ref().unwrap();
        assert_eq!(popup.items.len(), 1);
        assert_eq!(popup.items[0].command, "mv source destination");
        assert_eq!(popup.selected_index, Some(0));
    });
    f.assert_quiet();
}

fn set_encoding(launch: &mut SessionLaunchConfig, encoding: &str) {
    let field = match launch {
        SessionLaunchConfig::Local(config) => &mut config.encoding,
        SessionLaunchConfig::Ssh(config) => &mut config.encoding,
        SessionLaunchConfig::Telnet(config) => &mut config.encoding,
        SessionLaunchConfig::Serial(config) => &mut config.encoding,
        _ => panic!("encoding fixture requires a terminal session"),
    };
    *field = encoding.into();
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
    // Keep a marked shell's input anchor stable across its fill acknowledgement.
    // A network CLI prompt supplies a mapping without Readline evidence.
    let output = if app
        .terminal_snapshot_for_session(Some(id), 0)
        .shell_input_anchor
        .is_some()
    {
        format!("$ \x1b]133;B\x07{value}")
    } else {
        format!("<device>{value}")
    };
    app.terminal.seed_session_view(id.into(), output, "UTF-8");
    app.reconcile_shell_editing(id, cx);
}

fn popup(app: &mut NyaTermApp, id: &str, target: &str) {
    // These tests install exact rows; snapshot-triggered searches must not replace them.
    app.terminal.assist.command_suggestion_refresh_task = None;
    app.terminal.assist.command_suggestion_search_gen = app
        .terminal
        .assist
        .command_suggestion_search_gen
        .saturating_add(1);
    let state = app.terminal.editing.state().unwrap();
    app.terminal.assist.command_suggestions = Some(CommandSuggestionState {
        mode: nyaterm_core::completion::types::CompletionMode::History,
        completion_revision: 0,
        scope: None,
        session_id: id.into(),
        input_version: state.model.version,
        sync_revision: app.sync_input.revision(),
        draft: state.model.input.value.clone(),
        items: vec![CommandSuggestionItem {
            fillable: true,
            description: String::new(),
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
fn identical_input_writes_nothing_and_a_network_prompt_cannot_authorize_replacement() {
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
fn fuzzy_history_and_quick_labels_replace_once_and_wait_for_echo_without_execution() {
    for mode in [
        nyaterm_core::completion::types::CompletionMode::History,
        nyaterm_core::completion::types::CompletionMode::Token,
    ] {
        for (current, target) in [
            ("systmi", "systemctl restart mihomo.service"),
            ("重启代理", "systemctl restart example.service"),
        ] {
            let mut f = Fixture::new();
            f.suggest_in_shell(current, target);
            f.update(|app, cx, id| {
                app.terminal
                    .assist
                    .command_suggestions
                    .as_mut()
                    .unwrap()
                    .mode = mode;
                let version = app.terminal.editing.state().unwrap().model.version;
                assert_eq!(
                    app.submit_selected_command_suggestion(true, cx),
                    SuggestionSubmitResult::Submitted
                );
                let state = app.terminal.editing.state().unwrap();
                assert_eq!(state.model.input.value, target);
                assert_eq!(state.model.version, version + 1);
                assert_eq!(state.model.phase, EditPhase::Pending);
                assert_eq!(state.suggestion_fill.as_ref().unwrap().target, target);
                assert!(state.mapping.is_none());
                assert!(app.session.command_history_for(id).is_none());
                echo(app, cx, id, target);
                let state = app.terminal.editing.state().unwrap();
                assert_eq!(state.model.phase, EditPhase::Ready);
                assert!(state.suggestion_fill.is_none());
                assert!(app.session.command_history_for(id).is_none());
                assert!(app.terminal.assist.pending_command_history_entry.is_none());
            });
            let mut expected = vec![0x7f; current.chars().count()];
            expected.extend_from_slice(target.as_bytes());
            f.receive(&expected);
            f.assert_quiet();
            let expected_enter = f.update(|app, cx, id| {
                let press = key("enter");
                let wire = app
                    .terminal_key_bytes_for_event_for_session(Some(id), &press)
                    .unwrap();
                assert!(app.send_terminal_key_event(&press, true, cx));
                assert_eq!(app.session.command_history_for(id).unwrap(), [target]);
                assert!(app.terminal.assist.pending_command_history_entry.is_none());
                wire
            });
            f.receive(&expected_enter);
            f.assert_quiet();
        }
    }
}

#[test]
fn replacement_uses_native_keyboard_encoding_and_the_session_charset() {
    let mut f = Fixture::new();
    f.suggest_in_shell("代理", "重启服务");
    f.update(|app, cx, id| {
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .kitty_keyboard_disambiguate = true;
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .kitty_keyboard_report_event_types = true;
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .kitty_keyboard_report_all_keys_as_esc = true;
        let mut metadata = app.session.metadata(id).unwrap().clone();
        set_encoding(&mut metadata.launch_config, "GBK");
        app.session.register_session_metadata(id, metadata);
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::Submitted
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "重启服务");
    });
    let mut expected = b"\x1b[127;1:1u\x1b[127;1:3u".repeat(2);
    expected.extend_from_slice(&[0xd6, 0xd8, 0xc6, 0xf4, 0xb7, 0xfe, 0xce, 0xf1]);
    f.receive(&expected);
    f.assert_quiet();
}

#[test]
fn replacement_write_failure_leaves_the_original_prediction_and_no_fill_request() {
    let mut f = Fixture::new();
    f.suggest_in_shell("systmi", "systemctl restart mihomo.service");
    f.manager.close(&f.id).unwrap();
    f.update(|app, cx, id| {
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::WriteFailed
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "systmi");
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
fn replacement_encoding_failure_never_deletes_the_original_input() {
    let mut f = Fixture::new();
    f.suggest_in_shell("dkr", "echo 😀");
    f.update(|app, cx, id| {
        let mut metadata = app.session.metadata(id).unwrap().clone();
        set_encoding(&mut metadata.launch_config, "GBK");
        app.session.register_session_metadata(id, metadata);
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::WriteFailed
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "dkr");
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
    f.assert_quiet();
}

#[test]
fn replacement_confirmation_expiry_and_input_changes_cancel_the_request() {
    for change_input in [false, true] {
        let mut f = Fixture::new();
        f.suggest_in_shell("systmi", "systemctl restart mihomo.service");
        f.update(|app, cx, id| {
            assert_eq!(
                app.submit_selected_command_suggestion(false, cx),
                SuggestionSubmitResult::Submitted
            );
            if change_input {
                app.note_shell_editing_input(b"x", cx);
            } else {
                let now = Instant::now() + EDIT_CONFIRMATION_TIMEOUT;
                assert!(app.terminal.editing.state_mut().model.expire(now));
                let revision = app.sync_input.revision();
                app.terminal
                    .editing
                    .state_mut()
                    .cancel_stale_suggestion_fill(now, revision);
            }
            assert!(
                app.terminal
                    .editing
                    .state()
                    .unwrap()
                    .suggestion_fill
                    .is_none()
            );
            echo(app, cx, id, "systemctl restart mihomo.service");
            assert!(app.session.command_history_for(id).is_none());
        });
        f.receive(b"\x7f\x7f\x7f\x7f\x7f\x7fsystemctl restart mihomo.service");
        f.assert_quiet();
    }
}

#[test]
fn stale_versions_and_unsafe_terminal_states_reject_append_and_replacement() {
    let mut f = Fixture::new();
    for (current, target) in [
        ("display ver", "display version"),
        ("systmi", "systemctl restart mihomo.service"),
    ] {
        for scenario in 0..11 {
            f.suggest_in_shell(current, target);
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
        set_encoding(&mut metadata.launch_config, "GBK");
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
            .prepare_command_suggestion_search(
                generation,
                nyaterm_core::completion::types::CompletionMode::Token,
                cx,
            )
            .unwrap();
        let model = &mut app.terminal.editing.state_mut().model;
        model.note_input("\x1b[D", Instant::now());
        model.note_input("\x1b[C", Instant::now());
        app.publish_command_suggestion_search(
            request,
            super::tests::history_search_fixture(
                "display version",
                "display ver",
                app.commands.completion_runtime().revision(),
            ),
            Duration::ZERO,
            cx,
        );
        assert!(app.terminal.assist.command_suggestions.is_none());
    });
    f.assert_quiet();
}

#[test]
fn no_selection_preserves_normal_enter_and_tab_handling() {
    for mode in [
        nyaterm_core::completion::types::CompletionMode::Token,
        nyaterm_core::completion::types::CompletionMode::History,
    ] {
        let mut f = Fixture::new();
        f.suggest("display ver", "display version");
        f.update(|app, cx, id| {
            let popup = app.terminal.assist.command_suggestions.as_mut().unwrap();
            popup.mode = mode;
            popup.selected_index = None;
            let version = app.terminal.editing.state().unwrap().model.version;
            for name in ["enter", "tab"] {
                app.terminal
                    .view
                    .views
                    .get_mut(id)
                    .unwrap()
                    .pressed_keys
                    .insert(name.into());
                assert_eq!(
                    app.submit_selected_command_suggestion(name == "enter", cx),
                    SuggestionSubmitResult::NotHandled,
                );
                let press = key(name);
                assert!(!app.handle_command_suggestion_key(&press, cx));
                let mut repeat = press;
                repeat.is_held = true;
                assert!(!app.handle_command_suggestion_key(&repeat, cx));
                assert!(
                    app.terminal
                        .view
                        .views
                        .get(id)
                        .unwrap()
                        .pressed_keys
                        .contains(name)
                );
            }
            assert_eq!(app.terminal.editing.state().unwrap().model.version, version);
            assert!(
                app.terminal
                    .editing
                    .state()
                    .unwrap()
                    .suggestion_fill
                    .is_none()
            );
        });
        f.assert_quiet();
    }
}

#[test]
fn unselected_token_keys_reach_the_session_with_its_native_encoding() {
    for kitty in [false, true] {
        for name in ["enter", "tab"] {
            let mut f = Fixture::new();
            f.suggest("dock", "docker");
            let expected = f.update(|app, cx, id| {
                let popup = app.terminal.assist.command_suggestions.as_mut().unwrap();
                popup.mode = nyaterm_core::completion::types::CompletionMode::Token;
                popup.selected_index = None;
                app.terminal
                    .view
                    .views
                    .get_mut(id)
                    .unwrap()
                    .protocol_state
                    .kitty_keyboard_report_all_keys_as_esc = kitty;
                let press = key(name);
                assert!(!app.handle_command_suggestion_key(&press, cx));
                let expected = app
                    .terminal_key_bytes_for_event_for_session(Some(id), &press)
                    .unwrap();
                assert!(app.send_terminal_key_event(&press, true, cx));
                assert!(
                    app.terminal
                        .view
                        .views
                        .get(id)
                        .unwrap()
                        .pressed_keys
                        .contains(name)
                );
                assert!(
                    app.terminal
                        .editing
                        .state()
                        .unwrap()
                        .suggestion_fill
                        .is_none()
                );
                if name == "enter" {
                    assert_eq!(app.session.command_history_for(id).unwrap(), ["dock"]);
                } else {
                    assert!(app.session.command_history_for(id).is_none());
                }
                expected
            });
            f.receive(&expected);
            f.assert_quiet();
        }
    }
}

#[test]
fn typing_and_backspace_close_stale_candidates_before_the_debounced_refresh() {
    for (input, expected) in [(b"x".as_slice(), "dockx"), (b"\x7f".as_slice(), "doc")] {
        let mut f = Fixture::new();
        f.suggest("dock", "docker");
        f.update(|app, cx, _| {
            app.terminal
                .assist
                .command_suggestions
                .as_mut()
                .unwrap()
                .mode = nyaterm_core::completion::types::CompletionMode::Token;
            app.note_shell_editing_input(input, cx);
            app.note_command_suggestion_input(input, cx);
            assert_eq!(
                app.terminal.editing.state().unwrap().model.input.value,
                expected
            );
            assert!(
                app.terminal
                    .assist
                    .command_suggestion_refresh_task
                    .is_some()
            );
            assert!(app.terminal.assist.command_suggestions.is_none());
            assert!(!app.handle_command_suggestion_key(&key("enter"), cx));
            assert!(!app.handle_command_suggestion_key(&key("tab"), cx));
        });
        f.assert_quiet();
    }
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

#[test]
fn token_enter_fills_without_requesting_execution() {
    let mut fixture = Fixture::new();
    fixture.suggest("dock", "docker");
    fixture.update(|app, cx, id| {
        app.terminal
            .assist
            .command_suggestions
            .as_mut()
            .unwrap()
            .mode = nyaterm_core::completion::types::CompletionMode::Token;
        app.terminal
            .assist
            .command_suggestions
            .as_mut()
            .unwrap()
            .selected_index = Some(0);
        assert!(app.handle_command_suggestion_key(&key("enter"), cx));
        assert!(
            !app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .as_ref()
                .unwrap()
                .execute_requested
        );
        echo(app, cx, id, "docker");
        assert!(app.session.command_history_for(id).is_none());
    });
    fixture.receive(b"er");
    fixture.assert_quiet();
}

#[test]
fn clicking_a_token_description_accepts_that_candidate_without_execution() {
    let mut f = Fixture::new();
    f.suggest("dock", "docker");
    let position = f.update(|app, _, id| {
        let popup = app.terminal.assist.command_suggestions.as_mut().unwrap();
        popup.mode = nyaterm_core::completion::types::CompletionMode::Token;
        popup.selected_index = None;
        popup.items[0].display = "docker".into();
        popup.items[0].description = "A self-sufficient runtime for containers".into();
        let placement = app
            .suggestion_overlay_position_for_session(Some(id), 0, 0, 380., 76.)
            .unwrap();
        gpui::point(
            gpui::px(placement.x + 350.),
            gpui::px(placement.y + super::SUGGESTION_OVERLAY_HEADER_HEIGHT + 14.),
        )
    });
    let app = f.app.clone();
    let (_, cx) = f.cx.add_window_view(|_, _| PopupView(app));
    cx.simulate_click(position, gpui::Modifiers::default());
    f.update(|app, _, _| {
        assert!(app.terminal.assist.command_suggestions.is_none());
        let state = app.terminal.editing.state().unwrap();
        assert_eq!(state.model.input.value, "docker");
        assert!(!state.suggestion_fill.as_ref().unwrap().execute_requested);
    });
    f.receive(b"er");
    f.assert_quiet();
}

#[test]
fn disabled_fuzzy_candidate_never_writes_editing_keys() {
    let mut fixture = Fixture::new();
    fixture.suggest("dkr", "docker");
    fixture.update(|app, cx, _| {
        app.terminal
            .assist
            .command_suggestions
            .as_mut()
            .unwrap()
            .items[0]
            .fillable = false;
        assert!(app.handle_command_suggestion_key(&key("tab"), cx));
        assert!(
            app.terminal
                .editing
                .state()
                .unwrap()
                .suggestion_fill
                .is_none()
        );
    });
    fixture.assert_quiet();
}

#[test]
fn multiline_candidates_open_review_without_writing_and_cancellation_preserves_input() {
    for mode in [
        nyaterm_core::completion::types::CompletionMode::History,
        nyaterm_core::completion::types::CompletionMode::Token,
    ] {
        let mut f = Fixture::new();
        let target = "echo first\\\r\n  second\n\necho third";
        f.suggest_in_shell("下载驱动", target);
        let version = f.update(|app, cx, _| {
            let mut settings = app.settings.summary().clone();
            settings.terminal_show_multi_line_paste_dialog = false;
            app.settings.replace_summary(settings);
            app.terminal
                .assist
                .command_suggestions
                .as_mut()
                .unwrap()
                .mode = mode;
            let version = app.terminal.editing.state().unwrap().model.version;
            assert!(app.handle_command_suggestion_key(&key("enter"), cx));
            assert!(app.terminal.assist.command_suggestions.is_none());
            version
        });
        f.with_window(|app, window, cx, id| {
            app.open_pending_suggestion_paste(window, cx);
            assert!(app.terminal.overlay_visibility().paste_review);
            let editor = app.terminal.paste_review_editor().unwrap();
            assert_eq!(
                editor.read(cx).value(cx),
                "echo first\\\n  second\n\necho third"
            );
            assert_eq!(app.terminal.editing.state().unwrap().model.version, version);
            app.close_multi_line_paste(window, cx);
            assert!(app.terminal.paste.suggestion.is_none());
            assert_eq!(app.terminal.editing.predicted_input().value, "下载驱动");
            assert!(app.session.command_history_for(id).is_none());
        });
        f.assert_quiet();
    }
}

#[test]
fn reviewed_multiline_paste_replaces_the_query_once_and_preserves_the_edited_script() {
    for current in ["", "echo", "下载驱动"] {
        let mut f = Fixture::new();
        f.suggest_in_shell(current, "echo first\necho second");
        let version = f.update(|app, cx, id| {
            app.terminal
                .view
                .views
                .get_mut(id)
                .unwrap()
                .protocol_state
                .bracketed_paste = true;
            let version = app.terminal.editing.state().unwrap().model.version;
            assert_eq!(
                app.submit_selected_command_suggestion(false, cx),
                SuggestionSubmitResult::PasteReviewRequested
            );
            version
        });
        let target = "echo first\\\n  '中文 e\u{301}'\n\necho 😀";
        f.with_window(|app, window, cx, id| {
            app.open_pending_suggestion_paste(window, cx);
            let editor = app.terminal.paste_review_editor().unwrap();
            editor.update(cx, |editor, cx| {
                editor.replace_content(target, window, cx);
            });
            assert!(!app.suggestion_paste_needs_bracketed_shell());
            app.direct_multi_line_paste(window, cx);
            let state = app.terminal.editing.state().unwrap();
            assert_eq!(state.model.input.value, target);
            assert_eq!(state.model.input.cursor, target.len());
            assert!(state.model.input.multiline);
            assert_eq!(state.model.version, version + 1);
            assert!(state.suggestion_fill.is_none());
            assert!(app.session.command_history_for(id).is_none());
            assert!(app.terminal.assist.pending_command_history_entry.is_none());
            assert!(!app.terminal.overlay_visibility().paste_review);
        });
        let mut expected = vec![0x7f; current.chars().count()];
        expected.extend_from_slice(b"\x1b[200~");
        expected.extend_from_slice(target.as_bytes());
        expected.extend_from_slice(b"\x1b[201~");
        f.receive(&expected);
        f.assert_quiet();
    }
}

#[test]
fn multiline_direct_paste_rejects_shells_without_bracketed_paste() {
    let mut f = Fixture::new();
    f.suggest_in_shell("下载驱动", "echo first\necho second");
    f.update(|app, cx, _| {
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::PasteReviewRequested
        );
    });
    f.with_window(|app, window, cx, id| {
        app.open_pending_suggestion_paste(window, cx);
        assert!(app.suggestion_paste_needs_bracketed_shell());
        app.direct_multi_line_paste(window, cx);
        assert_eq!(app.terminal.editing.predicted_input().value, "下载驱动");
        assert!(app.session.command_history_for(id).is_none());
    });
    f.assert_quiet();
}

#[test]
fn reviewed_multiline_line_by_line_send_replaces_the_query_and_explicitly_submits() {
    let mut f = Fixture::new();
    f.suggest_in_shell("脚本", "echo first\necho second");
    f.update(|app, cx, _| {
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::PasteReviewRequested
        );
    });
    f.with_window(|app, window, cx, _| {
        app.open_pending_suggestion_paste(window, cx);
        app.send_multi_line_paste_by_line(window, cx);
        assert!(app.terminal.paste.suggestion.is_none());
        assert!(!app.terminal.overlay_visibility().paste_review);
    });
    f.receive(b"\x7f\x7fecho first\recho second\r");
    f.assert_quiet();
}

#[test]
fn review_confirmation_rechecks_stale_input_and_terminal_state_for_both_send_modes() {
    for by_line in [false, true] {
        for scenario in 0..8 {
            let mut f = Fixture::new();
            f.suggest_in_shell("脚本", "echo first\necho second");
            f.update(|app, cx, id| {
                app.terminal
                    .view
                    .views
                    .get_mut(id)
                    .unwrap()
                    .protocol_state
                    .bracketed_paste = true;
                assert_eq!(
                    app.submit_selected_command_suggestion(false, cx),
                    SuggestionSubmitResult::PasteReviewRequested
                );
            });
            f.with_window(|app, window, cx, id| {
                app.open_pending_suggestion_paste(window, cx);
                match scenario {
                    0 => {
                        app.note_shell_editing_input(b"X", cx);
                        echo(app, cx, id, "脚本X");
                    }
                    1 => app.terminal.editing.state_mut().awaiting_snapshot = true,
                    2 => {
                        app.sync_input.toggle_broadcast_to_all();
                    }
                    3 => app.terminal.assist.credential_prompt_input_until_ms = u64::MAX,
                    4 => {
                        app.terminal
                            .view
                            .views
                            .get_mut(id)
                            .unwrap()
                            .protocol_state
                            .alternate_screen = true
                    }
                    5 => {
                        app.session.select_active_session("fixture");
                    }
                    6 => app.security.activate_screen_lock("test lock".into()),
                    7 => app.terminal.editing.state_mut().model.version += 1,
                    _ => unreachable!(),
                }
                if by_line {
                    app.send_multi_line_paste_by_line(window, cx);
                } else {
                    app.direct_multi_line_paste(window, cx);
                }
                assert!(app.terminal.paste.suggestion.is_none());
                assert!(app.session.command_history_for(id).is_none());
                assert!(app.terminal.assist.pending_command_history_entry.is_none());
            });
            f.assert_quiet();
        }
    }
}

#[test]
fn multiline_paste_encoding_failure_does_not_erase_the_query() {
    let mut f = Fixture::new();
    f.suggest_in_shell("脚本", "echo first\necho 😀");
    f.update(|app, cx, id| {
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::PasteReviewRequested
        );
        let mut metadata = app.session.metadata(id).unwrap().clone();
        set_encoding(&mut metadata.launch_config, "GBK");
        app.session.register_session_metadata(id, metadata);
    });
    f.with_window(|app, window, cx, _| {
        app.open_pending_suggestion_paste(window, cx);
        app.direct_multi_line_paste(window, cx);
        assert_eq!(app.terminal.editing.predicted_input().value, "脚本");
        assert!(app.terminal.paste.suggestion.is_none());
    });
    f.assert_quiet();
}

#[test]
fn multiline_paste_uses_native_delete_keys_and_encodes_only_the_paste_body() {
    let mut f = Fixture::new();
    f.suggest_in_shell("脚本", "echo 中\necho 文");
    f.update(|app, cx, id| {
        let protocol = &mut app.terminal.view.views.get_mut(id).unwrap().protocol_state;
        protocol.bracketed_paste = true;
        protocol.kitty_keyboard_disambiguate = true;
        protocol.kitty_keyboard_report_event_types = true;
        protocol.kitty_keyboard_report_all_keys_as_esc = true;
        let mut metadata = app.session.metadata(id).unwrap().clone();
        set_encoding(&mut metadata.launch_config, "GBK");
        app.session.register_session_metadata(id, metadata);
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::PasteReviewRequested
        );
    });
    f.with_window(|app, window, cx, _| {
        app.open_pending_suggestion_paste(window, cx);
        app.direct_multi_line_paste(window, cx);
        assert_eq!(
            app.terminal.editing.predicted_input().value,
            "echo 中\necho 文"
        );
    });
    let mut expected = b"\x1b[127;1:1u\x1b[127;1:3u".repeat(2);
    expected.extend_from_slice(b"\x1b[200~echo \xd6\xd0\necho \xce\xc4\x1b[201~");
    f.receive(&expected);
    f.assert_quiet();
}

#[test]
fn multiline_paste_write_failure_keeps_the_original_prediction_and_does_not_retry() {
    let mut f = Fixture::new();
    f.suggest_in_shell("脚本", "echo first\necho second");
    f.update(|app, cx, id| {
        app.terminal
            .view
            .views
            .get_mut(id)
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        assert_eq!(
            app.submit_selected_command_suggestion(false, cx),
            SuggestionSubmitResult::PasteReviewRequested
        );
    });
    f.with_window(|app, window, cx, _| app.open_pending_suggestion_paste(window, cx));
    f.manager.close(&f.id).unwrap();
    f.with_window(|app, window, cx, id| {
        app.direct_multi_line_paste(window, cx);
        assert_eq!(app.terminal.editing.predicted_input().value, "脚本");
        assert!(app.terminal.paste.suggestion.is_none());
        assert!(app.session.command_history_for(id).is_none());
        app.direct_multi_line_paste(window, cx);
        assert_eq!(app.terminal.editing.predicted_input().value, "脚本");
    });
}

#[test]
fn clicking_a_multiline_candidate_opens_review_even_without_shell_editing_capability() {
    let mut f = Fixture::new();
    f.suggest("脚本", "echo first\necho second");
    let position = f.update(|app, _, id| {
        let placement = app
            .suggestion_overlay_position_for_session(Some(id), 0, 0, 380., 76.)
            .unwrap();
        gpui::point(
            gpui::px(placement.x + 150.),
            gpui::px(placement.y + super::SUGGESTION_OVERLAY_HEADER_HEIGHT + 14.),
        )
    });
    let app = f.app.clone();
    let (_, cx) = f.cx.add_window_view(|_, _| PopupView(app));
    cx.simulate_click(position, gpui::Modifiers::default());
    f.update(|app, cx, _| {
        assert!(app.terminal.assist.command_suggestions.is_none());
        assert!(app.terminal.overlay_visibility().paste_review);
        assert!(app.suggestion_paste_needs_bracketed_shell());
        assert!(app.suggestion_paste_cannot_replace_input());
        assert_eq!(
            app.terminal
                .paste_review_editor()
                .unwrap()
                .read(cx)
                .value(cx),
            "echo first\necho second"
        );
        assert_eq!(app.terminal.editing.predicted_input().value, "脚本");
    });
    f.assert_quiet();
}
