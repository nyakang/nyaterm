use nyaterm_core::QuickCommand;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_transport::{SessionManager, TelnetSessionConfig};

use crate::features::NyaTermApp;
use crate::features::file_manager::{FileManagerSeed, PaneEndpoint};
use crate::features::test_support::{app_with_visible_local_session, editor_document};
use crate::models::{SessionLaunchConfig, WorkspaceTabId};
use crate::test_support::{TestAppCleanup, TestConfigDir};

use super::{quick_command_delivery_status, quick_command_executes};

fn command_with_execution_mode(execution_mode: Option<&str>) -> QuickCommand {
    QuickCommand {
        id: "command-1".to_string(),
        label: "Command".to_string(),
        command: "pwd".to_string(),
        category_id: None,
        description: None,
        color_tag: None,
        icon_tag: None,
        pinned: None,
        execution_mode: execution_mode.map(ToOwned::to_owned),
        raw_send: None,
        source: None,
        risk_level: None,
        updated_at: None,
        created_at: None,
        use_count: None,
        sort_order: None,
    }
}

#[test]
fn append_mode_inserts_without_execution() {
    assert!(!quick_command_executes(&command_with_execution_mode(Some(
        "append"
    ))));
}

#[test]
fn execute_and_legacy_modes_execute() {
    assert!(quick_command_executes(&command_with_execution_mode(Some(
        "execute"
    ))));
    assert!(quick_command_executes(&command_with_execution_mode(None)));
}

#[test]
fn staged_delivery_reports_broadcast_reach() {
    assert_eq!(
        quick_command_delivery_status("Login", true, true, 4, 0),
        "sent quick command 'Login' to 4 session(s)"
    );
    assert_eq!(
        quick_command_delivery_status("Login", true, true, 2, 1),
        "sent quick command 'Login' to 2 session(s), 1 failed"
    );
}

#[test]
fn staged_delivery_keeps_the_single_session_wording() {
    assert_eq!(
        quick_command_delivery_status("Login", true, false, 1, 0),
        "ran quick command 'Login'"
    );
    assert_eq!(
        quick_command_delivery_status("Login", false, false, 1, 0),
        "inserted quick command 'Login'"
    );
    assert_eq!(
        quick_command_delivery_status("Login", true, false, 0, 1),
        "quick command 'Login' failed on 1 session(s)"
    );
}

struct WireFixture {
    cx: TestAppContext,
    app: Entity<NyaTermApp>,
    manager: Arc<SessionManager>,
    ids: Vec<String>,
    streams: Vec<TcpStream>,
    _cleanup: TestAppCleanup,
    root: TestConfigDir,
}

impl WireFixture {
    fn new(count: usize) -> Self {
        let root = TestConfigDir::new("nyaterm-staged-quick-command");
        let mut cx = TestAppContext::single();
        let cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (manager, ids) = cx.update_entity(&app, |app, _| {
            let manager = app.session.manager_handle();
            let metadata = app.session.metadata("fixture").unwrap().clone();
            let mut ids = Vec::new();
            for _ in 0..count {
                let config = TelnetSessionConfig {
                    host: "127.0.0.1".into(),
                    port,
                    raw_tcp: true,
                    ..TelnetSessionConfig::default()
                };
                let info = manager.create_telnet_session(config.clone()).unwrap();
                let mut metadata = metadata.clone();
                metadata.launch_config = SessionLaunchConfig::Telnet(config);
                app.session.register_session_metadata(&info.id, metadata);
                app.terminal
                    .seed_session_view(info.id.clone(), String::new(), "UTF-8");
                ids.push(info.id);
            }
            app.session.metadata_mut("fixture").unwrap().disconnected = true;
            app.session.select_active_session(&ids[0]);
            (manager, ids)
        });
        let streams = (0..count)
            .map(|_| {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
            })
            .collect();
        Self {
            cx,
            app,
            manager,
            ids,
            streams,
            _cleanup: cleanup,
            root,
        }
    }

    fn send(&mut self, text: &str) {
        self.cx.update_entity(&self.app, |app, cx| {
            app.send_resolved_quick_command(
                "test".into(),
                "Test".into(),
                text.into(),
                true,
                false,
                cx,
            );
        });
        self.cx.run_until_parked();
    }

    fn advance(&mut self) {
        self.cx.executor().advance_clock(Duration::from_secs(1));
        self.cx.run_until_parked();
    }

    fn assert_wire(&mut self, index: usize, expected: &[u8]) {
        self.cx.update_entity(&self.app, |app, _| {
            app.write_session_protocol_response(&self.ids[index], b"END")
                .unwrap();
        });
        let mut actual = vec![0; expected.len() + 3];
        self.streams[index].read_exact(&mut actual).unwrap();
        let mut expected = expected.to_vec();
        expected.extend_from_slice(b"END");
        assert_eq!(actual, expected);
    }

    fn open_editor(&mut self) {
        self.cx.update_entity(&self.app, |app, cx| {
            let document = editor_document(
                &self.ids[0],
                "/test.txt",
                nyaterm_core::InternalEditorDisplay::Workspace,
            );
            let id = document.id.clone();
            app.transfer.open_editor_tab(document);
            app.open_workspace_editor_tab(&id, cx);
        });
    }

    fn open_file_manager(&mut self) {
        self.cx.executor().allow_parking();
        self.cx.update_entity(&self.app, |app, cx| {
            let path = self.root.path().to_string_lossy().into_owned();
            app.open_file_manager_tab(
                Some(FileManagerSeed {
                    left: PaneEndpoint::Local,
                    left_path: Some(nyaterm_transport::RemoteFilePath::new(path.clone())),
                    right: PaneEndpoint::Local,
                    right_path: Some(nyaterm_transport::RemoteFilePath::new(path)),
                }),
                cx,
            );
            assert!(matches!(
                WorkspaceTabId::from_layout_key(&app.active_workspace_tab().unwrap()),
                WorkspaceTabId::FileManager(_)
            ));
            assert_eq!(app.session.active_id(), Some(self.ids[0].as_str()));
        });
    }
}

impl Drop for WireFixture {
    fn drop(&mut self) {
        for id in &self.ids {
            let _ = self.manager.close(id);
        }
    }
}

#[test]
fn literal_quick_command_sends_exact_text_without_variables_or_pauses_in_both_execution_modes() {
    let mut fixture = WireFixture::new(1);
    for execute in [false, true] {
        for ending in ["  ", "\r", "\n", "\r\n"] {
            let text = format!("  {{{{变量}}}} \\r\\n\\t\\p\\\\ C:\\temp\n下一行{ending}");
            fixture.cx.update_entity(&fixture.app, |app, cx| {
                let mut command =
                    command_with_execution_mode(Some(if execute { "execute" } else { "append" }));
                command.command = text.clone();
                command.raw_send = Some(true);
                app.commands
                    .replace_quick_command_catalog(vec![command], Vec::new());
                app.apply_quick_command_by_id("command-1", false, cx);
                assert!(app.commands.quick_variable_prompt().is_none());
            });
            fixture.cx.run_until_parked();
            let mut expected = text.into_bytes();
            if execute && ending == "  " {
                expected.push(b'\r');
            }
            fixture.assert_wire(0, &expected);
        }
    }
}

#[test]
fn literal_quick_command_broadcast_preserves_text_for_each_session() {
    let mut fixture = WireFixture::new(2);
    let text = "  {{host}} \\p C:\\temp\n下一行 ";
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        let mut command = command_with_execution_mode(Some("append"));
        command.command = text.into();
        command.raw_send = Some(true);
        app.commands
            .replace_quick_command_catalog(vec![command], Vec::new());
        app.apply_quick_command_by_id("command-1", true, cx);
        assert!(app.commands.quick_variable_prompt().is_none());
    });
    fixture.cx.run_until_parked();
    for index in 0..2 {
        fixture.assert_wire(index, text.as_bytes());
    }
}

#[test]
fn disabling_literal_mode_restores_variable_prompts_and_escape_processing() {
    let mut fixture = WireFixture::new(1);
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        let mut command = command_with_execution_mode(Some("append"));
        command.command = r"{{host}}\t\pnext".into();
        command.raw_send = Some(false);
        app.commands
            .replace_quick_command_catalog(vec![command], Vec::new());
        app.apply_quick_command_by_id("command-1", false, cx);
        assert!(app.commands.quick_variable_prompt().is_some());
        app.commands.set_quick_variable_value(0, "prod".into());
        app.submit_quick_command_variable_prompt(cx);
    });
    fixture.cx.run_until_parked();
    fixture.assert_wire(0, b"prod\t");
    fixture.advance();
    fixture.assert_wire(0, b"next");
}

#[test]
fn staged_quick_command_rejects_editor_workspace_like_ordinary_input() {
    let mut fixture = WireFixture::new(1);
    fixture.open_editor();
    fixture.send("ordinary");
    fixture.send(r"staged\pcommand");
    fixture.advance();
    fixture.assert_wire(0, b"");
}

#[test]
fn staged_quick_command_rejects_file_manager_workspace_like_ordinary_input() {
    let mut fixture = WireFixture::new(1);
    fixture.open_file_manager();
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        assert!(!app.send_terminal_input(b"typed".to_vec(), cx));
    });
    fixture.send("ordinary");
    fixture.send(r"staged\pcommand");
    fixture.send(r"\pdelayed");
    fixture.advance();
    fixture.assert_wire(0, b"");
}

#[test]
fn staged_quick_command_keeps_sending_to_pinned_target_after_opening_file_manager() {
    let mut fixture = WireFixture::new(1);
    fixture.send(r"echo foo\pbar");
    fixture.assert_wire(0, b"echo foo");
    fixture.open_file_manager();
    fixture.advance();
    fixture.assert_wire(0, b"bar\r");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["echo foobar"]
        );
    });
}

#[test]
fn staged_quick_command_keeps_target_and_full_history_after_focus_changes() {
    let mut fixture = WireFixture::new(2);
    fixture.send(r"echo foo\pbar");
    fixture.assert_wire(0, b"echo foo");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert!(app.session.command_history_for(&fixture.ids[0]).is_none());
        app.session.select_active_session(&fixture.ids[1]);
    });
    fixture.open_editor();
    fixture.advance();
    fixture.assert_wire(0, b"bar\r");
    fixture.assert_wire(1, b"");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["echo foobar"]
        );
        assert!(app.session.command_history_for(&fixture.ids[1]).is_none());
    });
}

#[test]
fn staged_quick_command_is_cancelled_by_new_staged_or_ordinary_commands() {
    for replacement in [r"new \pcommand", "new command"] {
        let mut fixture = WireFixture::new(1);
        fixture.send(r"old\pobsolete");
        fixture.send(replacement);
        fixture.advance();
        fixture.assert_wire(0, b"oldnew command\r");
    }
}

#[test]
fn staged_quick_command_yields_to_typed_pasted_raw_and_sensitive_input() {
    for kind in 0..4 {
        let mut fixture = WireFixture::new(1);
        fixture.send(r"old\pobsolete");
        fixture.cx.update_entity(&fixture.app, |app, cx| {
            let id = &fixture.ids[0];
            match kind {
                0 => assert!(app.send_terminal_input(b"manual".to_vec(), cx)),
                1 => app.send_terminal_paste_input("manual", cx),
                2 => app.write_session_raw_input_recorded(id, b"manual").unwrap(),
                _ => app.write_session_sensitive_input(id, b"manual").unwrap(),
            }
        });
        fixture.advance();
        fixture.assert_wire(0, b"oldmanual");
    }
}

#[test]
fn staged_quick_command_cancellation_only_affects_competing_sync_target() {
    let mut fixture = WireFixture::new(2);
    fixture.cx.update_entity(&fixture.app, |app, _| {
        app.sync_input.create_group();
        for id in &fixture.ids {
            app.sync_input.toggle_selected_session(id.clone());
        }
    });
    fixture.send(r"echo foo\pbar");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        app.write_session_sensitive_input(&fixture.ids[1], b"manual")
            .unwrap();
    });
    fixture.advance();
    fixture.assert_wire(0, b"echo foobar\r");
    fixture.assert_wire(1, b"echo foomanual");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["echo foobar"]
        );
        assert!(app.session.command_history_for(&fixture.ids[1]).is_none());
    });
}

#[test]
fn staged_quick_command_history_suppresses_sensitive_line_per_target() {
    let mut fixture = WireFixture::new(2);
    fixture.cx.update_entity(&fixture.app, |app, _| {
        app.sync_input.create_group();
        for id in &fixture.ids {
            app.sync_input.toggle_selected_session(id.clone());
        }
    });
    fixture.send(r"echo foo\pbar\r\pnext");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        app.observe_recording_prompt_output(&fixture.ids[1], "Password: ");
    });
    fixture.advance();
    fixture.cx.update_entity(&fixture.app, |app, _| {
        app.observe_recording_prompt_output(&fixture.ids[1], "\r\nuser@host:~$ ");
    });
    fixture.advance();
    for index in 0..2 {
        fixture.assert_wire(index, b"echo foobar\rnext\r");
    }
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["next", "echo foobar"]
        );
        assert_eq!(
            app.session.command_history_for(&fixture.ids[1]).unwrap(),
            ["next"]
        );
    });
}

#[test]
fn staged_quick_command_honors_leading_pause_and_does_not_resume_after_disconnect() {
    let mut fixture = WireFixture::new(1);
    fixture.send(r"\pcommand");
    fixture.assert_wire(0, b"");
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        app.mark_session_disconnected(&fixture.ids[0], cx);
        // Even reuse of this ID cannot restore the old script's ownership.
        let mut metadata = app.session.metadata(&fixture.ids[0]).unwrap().clone();
        metadata.disconnected = false;
        app.session
            .register_session_metadata(&fixture.ids[0], metadata);
    });
    fixture.advance();
    fixture.assert_wire(0, b"");
}

#[test]
fn staged_quick_command_history_only_commits_complete_lines_and_collapses_crlf() {
    let mut fixture = WireFixture::new(1);
    fixture.send(r"one\r\npart\pial\n\pnext");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["one"]
        );
    });
    fixture.advance();
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["partial", "one"]
        );
    });
    fixture.advance();
    // Raw TCP text input now honors the fixture's default CR Enter mode.
    fixture.assert_wire(0, b"one\rpartial\rnext\r");
    fixture.cx.update_entity(&fixture.app, |app, _| {
        assert_eq!(
            app.session.command_history_for(&fixture.ids[0]).unwrap(),
            ["next", "partial", "one"]
        );
    });
}

#[test]
fn staged_quick_command_waits_for_all_leading_pauses_before_writing() {
    let mut fixture = WireFixture::new(1);
    fixture.send(r"\p\pcommand");
    fixture.assert_wire(0, b"");
    fixture.advance();
    fixture.assert_wire(0, b"");
    fixture.advance();
    fixture.assert_wire(0, b"command\r");
}

#[test]
fn target_exclusions_apply_to_keyboard_paste_raw_and_quick_commands_without_blocking_sources() {
    let mut fixture = WireFixture::new(4);
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        app.sync_input.toggle_broadcast_to_all();
        app.set_sessions_receive_sync_input(
            &[fixture.ids[0].clone(), fixture.ids[3].clone()],
            false,
            cx,
        );
        assert!(app.send_terminal_input(b"key".to_vec(), cx));
        app.send_terminal_paste_input("paste", cx);
        assert!(app.send_terminal_literal_input(b"raw".to_vec(), cx));
    });
    fixture.send("quick");
    fixture.assert_wire(0, b"keypasterawquick\r");
    fixture.assert_wire(1, b"keypasterawquick\r");
    fixture.assert_wire(2, b"keypasterawquick\r");
    fixture.assert_wire(3, b"");
}

#[test]
fn excluding_a_staged_receiver_cancels_future_chunks_even_after_rechecking() {
    let mut fixture = WireFixture::new(2);
    fixture.cx.update_entity(&fixture.app, |app, _| {
        app.sync_input.toggle_broadcast_to_all();
    });
    fixture.send(r"first\psecond");
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        app.set_sessions_receive_sync_input(&fixture.ids, false, cx);
        app.set_sessions_receive_sync_input(&fixture.ids, true, cx);
    });
    fixture.advance();
    fixture.assert_wire(0, b"firstsecond\r");
    fixture.assert_wire(1, b"first");
}

#[test]
fn ime_preedit_is_local_and_commit_reaches_each_eligible_session_once() {
    use crate::features::test_support::session_focus_test_window;
    use gpui::EntityInputHandler;

    let mut fixture = WireFixture::new(3);
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        app.sync_input.toggle_broadcast_to_all();
        app.set_sessions_receive_sync_input(
            &[fixture.ids[0].clone(), fixture.ids[2].clone()],
            false,
            cx,
        );
    });
    let vcx = session_focus_test_window(&mut fixture.cx, &fixture.app);
    vcx.update(|window, cx| {
        fixture.app.update(cx, |app, cx| {
            app.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
            let key = gpui::KeyDownEvent {
                keystroke: gpui::Keystroke {
                    key: "中".into(),
                    key_char: Some("中".into()),
                    modifiers: Default::default(),
                },
                is_held: false,
                prefer_character_input: false,
            };
            assert!(app.terminal_should_defer_key_text_to_input_handler(&key));
            app.replace_text_in_range(None, "中文", window, cx);
        });
    });
    fixture.assert_wire(0, "中文".as_bytes());
    fixture.assert_wire(1, "中文".as_bytes());
    fixture.assert_wire(2, b"");
}

#[test]
fn explicit_send_to_all_respects_excluded_targets_for_immediate_and_staged_commands() {
    for text in ["firstsecond", r"first\psecond"] {
        let mut fixture = WireFixture::new(2);
        fixture.cx.update_entity(&fixture.app, |app, cx| {
            app.set_sessions_receive_sync_input(&[fixture.ids[1].clone()], false, cx);
            app.send_resolved_quick_command(
                "test".into(),
                "Test".into(),
                text.into(),
                true,
                true,
                cx,
            );
        });
        fixture.cx.run_until_parked();
        fixture.advance();
        fixture.assert_wire(0, b"firstsecond\r");
        fixture.assert_wire(1, b"");
    }
}

#[test]
fn helper_backed_and_disconnected_sessions_are_never_terminal_sync_targets() {
    let mut fixture = WireFixture::new(4);
    fixture.cx.update_entity(&fixture.app, |app, cx| {
        app.sync_input.toggle_broadcast_to_all();
        app.session
            .metadata_mut(&fixture.ids[1])
            .unwrap()
            .launch_config = SessionLaunchConfig::Rdp(nyaterm_remote_desktop::RdpSessionConfig {
            relay: None,
            backend: Default::default(),
            windows_authentication: None,
            name: "RDP".into(),
            host: "127.0.0.1".into(),
            port: 3389,
            username: String::new(),
            domain: String::new(),
            password: None,
            use_nla: false,
            certificate_policy: Default::default(),
            administrative_session: false,
            display: Default::default(),
            clipboard: Default::default(),
            reconnect: Default::default(),
        });
        app.session
            .metadata_mut(&fixture.ids[2])
            .unwrap()
            .launch_config = SessionLaunchConfig::Vnc(nyaterm_remote_desktop::VncSessionConfig {
            relay: None,
            username: String::new(),
            name: "VNC".into(),
            host: "127.0.0.1".into(),
            port: 5900,
            password: None,
            security: Default::default(),
            display: Default::default(),
            clipboard: Default::default(),
            reconnect: Default::default(),
            shared: true,
            view_only: false,
        });
        app.session
            .metadata_mut(&fixture.ids[3])
            .unwrap()
            .disconnected = true;
        assert!(app.sync_peer_session_ids(&fixture.ids[0]).is_empty());
        app.set_sessions_receive_sync_input(&fixture.ids, false, cx);
        assert!(app.sync_input.target_allowed(&fixture.ids[1]));
        assert!(app.sync_input.target_allowed(&fixture.ids[2]));
    });
}
