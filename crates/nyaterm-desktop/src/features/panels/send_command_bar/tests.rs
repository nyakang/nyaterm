use gpui::{
    AppContext as _, Context, Entity, IntoElement, MouseDownEvent, MouseMoveEvent, Render,
    TestAppContext, Window, div, point, prelude::*, px,
};
use nyaterm_core::{AppRuntime, RuntimeMode, test_support::TestTempDir};

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::models::BottomPanelMode;
use crate::send_command::SendCommandDataType;

struct CommandPanelFixture {
    app: Entity<NyaTermApp>,
    width: f32,
}

impl Render for CommandPanelFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self
            .app
            .update(cx, |app, cx| app.workspace_view(cx).into_any_element());
        div()
            .w(px(self.width))
            .h(px(760.))
            .flex()
            .flex_col()
            .child(workspace)
    }
}

#[test]
fn command_panel_keeps_editor_and_action_inside_empty_workspace_at_all_sizes() {
    let root = TestTempDir::new("nyaterm-command-panel-layout");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().to_path_buf(),
        root.path().join("config"),
        root.path().join("logs"),
        root.path().join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
    });
    let fixture_app = app.clone();
    let (fixture, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        CommandPanelFixture {
            app: fixture_app,
            width: 1200.,
        }
    });
    for width in [1200., 420.] {
        fixture.update(cx, |fixture, cx| {
            fixture.width = width;
            cx.notify();
        });
        for data_type in [SendCommandDataType::Text, SendCommandDataType::Hex] {
            for height in [120., 180., 520., 120.] {
                app.update(cx, |app, cx| {
                    let delta = app.shell.command_send_height() - height;
                    app.start_bottom_panel_resize(&MouseDownEvent::default(), cx);
                    app.update_bottom_panel_resize(
                        &MouseMoveEvent {
                            position: point(px(0.), px(delta)),
                            ..Default::default()
                        },
                        cx,
                    );
                    app.finish_bottom_panel_resize(cx);
                    app.set_send_command_data_type(data_type, cx);
                    cx.notify();
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    _ = window.draw(cx);
                });
                let controls = cx.debug_bounds("bottom-command-controls").unwrap();
                let editor = cx
                    .debug_bounds(if data_type == SendCommandDataType::Text {
                        "send-command.draft"
                    } else {
                        "send-command.hex"
                    })
                    .unwrap();
                let footer = cx.debug_bounds("send-command.footer").unwrap();
                let action = cx.debug_bounds("bottom-command-send").unwrap();
                assert_eq!(controls.size.height, px(32.));
                assert!(editor.origin.y >= controls.bottom());
                assert!(editor.size.height >= px(32.));
                assert!(editor.bottom() <= px(760.));
                assert!(action.bottom() <= px(760.));
                assert!(editor.bottom() <= footer.origin.y);
                assert!(action.origin.y >= footer.origin.y);
                assert!(action.origin.x >= footer.origin.x);
                assert!(action.right() <= footer.right());
                assert!(action.bottom() <= footer.bottom());
                assert!(action.right() <= px(width));
            }
        }
    }

    app.update(cx, |app, cx| {
        app.apply_send_command_draft("AT+CSQ".into(), cx);
        app.reset_text_input("send-command.draft", "AT+CSQ", cx);
        app.send_command
            .presentation(cx)
            .hex
            .update(cx, |hex, cx| hex.set_bytes(vec![1, 255], cx));
    });
    for data_type in [SendCommandDataType::Hex, SendCommandDataType::Text] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.set_send_command_data_type(data_type, cx);
                app.focus_send_command_composer(window, cx);
            });
        });
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |app, cx| {
            assert!(!app.send_command.presentation(cx).draft.is_empty())
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.focus_send_command_composer(window, cx))
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-l"
        } else {
            "ctrl-l"
        });
        app.read_with(cx, |app, cx| {
            assert!(app.send_command.presentation(cx).draft.is_empty())
        });
        if data_type == SendCommandDataType::Hex {
            app.update(cx, |app, cx| {
                app.set_send_command_data_type(SendCommandDataType::Text, cx);
                assert_eq!(app.send_command.presentation(cx).draft, "AT+CSQ");
            });
        }
    }
}

#[test]
fn command_send_target_picker_keeps_multiselect_open_and_restores_keyboard_focus() {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::send_command::{SendCommandSelection, SendCommandTarget};
    use crate::test_support::TestConfigDir;
    use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers};

    let root = TestConfigDir::new("nyaterm-send-target-picker");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "first");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
        let metadata = app.session.metadata("first").unwrap().clone();
        app.session.register_session_metadata("second", metadata);
        app.set_send_command_target(
            SendCommandTarget::Sessions(SendCommandSelection::default()),
            cx,
        );
    });
    let fixture_app = app.clone();
    let (fixture, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        CommandPanelFixture {
            app: fixture_app,
            width: 1200.,
        }
    });
    for width in [1200., 420.] {
        fixture.update(cx, |fixture, cx| {
            fixture.width = width;
            cx.notify();
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                window.focus(app.send_command.target_trigger_focus(), cx)
            });
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let keystroke = Keystroke::parse("enter").unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        cx.run_until_parked();
        for _ in 0..2 {
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        }
        let content = cx.debug_bounds("send-command.target-content").unwrap();
        assert!(content.size.width <= px(320.));
        assert!(content.top() >= px(0.));
        assert!(content.bottom() <= px(760.));
        for selector in [
            "send-command.target-row-first",
            "send-command.target-row-second",
        ] {
            let row = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(row.origin + point(px(7.), px(7.)), Modifiers::default());
            cx.run_until_parked();
            app.read_with(cx, |app, _| assert!(app.send_command.target_picker_open()));
        }
        app.read_with(cx, |app, _| {
            let SendCommandTarget::Sessions(selection) = app.send_command.target() else {
                panic!("manual mode lost");
            };
            assert_eq!(selection.ids().len(), 2);
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.read_with(cx, |app, _| {
                assert!(!app.send_command.target_picker_open());
                assert!(app.send_command.target_trigger_focus().is_focused(window));
            });
        });
        app.update(cx, |app, cx| {
            app.set_send_command_target(
                SendCommandTarget::Sessions(SendCommandSelection::default()),
                cx,
            )
        });
    }
}
