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
                let trigger_slot = cx.debug_bounds("send-command.target-trigger-slot").unwrap();
                let editor = cx
                    .debug_bounds(if data_type == SendCommandDataType::Text {
                        "send-command.draft"
                    } else {
                        "send-command.hex"
                    })
                    .unwrap();
                let footer = cx.debug_bounds("send-command.footer").unwrap();
                let action = cx.debug_bounds("bottom-command-send").unwrap();
                assert_eq!(controls.size.height, px(36.));
                assert!(trigger_slot.top() - controls.top() >= px(2.));
                assert!(controls.bottom() - trigger_slot.bottom() >= px(2.));
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
    use crate::send_command::SendCommandTarget;
    use crate::test_support::TestConfigDir;
    use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers};

    crate::preload_i18n().unwrap();
    assert_eq!(
        rust_i18n::t!("serialSend.chooseSessions", locale = "zh-CN"),
        "选择会话"
    );
    assert_eq!(
        rust_i18n::t!("serialSend.selectedTargets", locale = "zh-CN", count = 2),
        "已选 2 个会话"
    );
    let root = TestConfigDir::new("nyaterm-send-target-picker");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "first");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
        let mut metadata = app.session.metadata("first").unwrap().clone();
        metadata.launch_config = crate::models::SessionLaunchConfig::Serial(Default::default());
        app.session.register_session_metadata("second", metadata);
        app.set_send_command_target(SendCommandTarget::Current, cx);
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
        for _ in 0..3 {
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        }
        app.read_with(cx, |app, _| {
            assert_eq!(*app.send_command.target(), SendCommandTarget::Current)
        });
        let content = cx.debug_bounds("send-command.target-content").unwrap();
        assert!(content.size.width <= px(280.));
        let actions = cx.debug_bounds("send-command.target-actions").unwrap();
        let list = cx.debug_bounds("send-command.target-list").unwrap();
        assert!(actions.left() >= content.left());
        assert!(actions.right() <= content.right());
        assert!(actions.size.height <= px(28.));
        assert!(list.top() >= actions.bottom());
        assert!(content.top() >= px(0.));
        assert!(content.bottom() <= px(760.));
        for selector in [
            "send-command.target-row-first",
            "send-command.target-row-second",
        ] {
            let row = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(row.center(), Modifiers::default());
            cx.run_until_parked();
            app.read_with(cx, |app, _| assert!(app.send_command.target_picker_open()));
        }
        app.read_with(cx, |app, _| {
            let SendCommandTarget::Sessions(selection) = app.send_command.target() else {
                panic!("manual mode lost");
            };
            assert_eq!(selection.ids().len(), 2);
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let clear = cx.debug_bounds("send-command.target-clear-action").unwrap();
        cx.simulate_click(clear.center(), Modifiers::default());
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(app.send_command_target_session_ids().is_empty())
        });
        for _ in 0..3 {
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        }
        let all = cx.debug_bounds("send-command.target-all-action").unwrap();
        cx.simulate_click(all.center(), Modifiers::default());
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.send_command_target_session_ids().len(), 2);
            assert!(app.send_command.target_picker_open());
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
            app.set_send_command_target(SendCommandTarget::Current, cx)
        });
    }
}

/// Uses synthetic metadata only; never opens a shell or connects to a host.
#[test]
#[ignore = "native target picker visual acceptance; run alone with --nocapture"]
fn command_send_target_picker_native_preview() {
    use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};
    use crate::send_command::{SendCommandSelection, SendCommandTarget};
    use gpui::{Bounds, WindowBounds, WindowOptions, size};
    use nyaterm_core::AiExecutionProfile;
    use nyaterm_transport::LocalSessionConfig;
    use std::time::Duration;

    struct PickerPreview(Entity<NyaTermApp>);
    impl Render for PickerPreview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |app, cx| {
                let state = app.send_command_bar_view_state(cx);
                div()
                    .size_full()
                    .bg(gpui::rgb(state.palette.surface))
                    .flex()
                    .items_end()
                    .p_4()
                    .child(app.send_command_target_picker(&state, cx))
            })
        }
    }
    crate::preload_i18n().unwrap();
    rust_i18n::set_locale("zh-CN");
    let output = std::env::var("NYATERM_TARGET_PREVIEW_OUTPUT").expect("PNG output path");
    let width = std::env::var("NYATERM_TARGET_PREVIEW_WIDTH")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(640.);
    let root = TestTempDir::new("nyaterm-target-preview");
    struct PreviewAssets;
    impl gpui::AssetSource for PreviewAssets {
        fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
            let bytes: Option<&'static [u8]> = match path {
                "icons/check.svg" => Some(include_bytes!(
                    "../../../../../nyaterm-app/assets/icons/check.svg"
                )),
                "icons/menu/chevron-right.svg" => Some(include_bytes!(
                    "../../../../../nyaterm-app/assets/icons/menu/chevron-right.svg"
                )),
                "icons/chevron-down.svg" => Some(include_bytes!(
                    "../../../../../nyaterm-app/assets/icons/chevron-down.svg"
                )),
                _ => None,
            };
            Ok(bytes.map(std::borrow::Cow::Borrowed))
        }
        fn list(&self, _: &str) -> gpui::Result<Vec<gpui::SharedString>> {
            Ok(Vec::new())
        }
    }
    gpui_platform::application()
        .with_assets(PreviewAssets)
        .run(move |cx| {
            let runtime = AppRuntime::from_parts_for_test(
                RuntimeMode::Portable,
                root.path().into(),
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
            app.update(cx, |app, cx| {
                app.sync_component_theme(cx);
                rust_i18n::set_locale("zh-CN");
                for (id, name) in [
                    ("first", "开发环境"),
                    ("second", "串口设备 COM3"),
                    ("third", "本地终端 — 一个用于检查省略显示的很长的会话名称"),
                ] {
                    app.session.register_session_metadata(
                        id,
                        SessionRuntimeMetadata {
                            confirmed_ssh_runtime_mode: None,
                            pending_ssh_startup_command: None,
                            ssh_config: None,
                            ssh_multiplex_key: None,
                            source_connection_id: None,
                            ai_execution_profile: AiExecutionProfile::Posix,
                            launch_config: if id == "second" {
                                SessionLaunchConfig::Serial(
                                    nyaterm_transport::SerialSessionConfig {
                                        name: name.into(),
                                        ..Default::default()
                                    },
                                )
                            } else {
                                SessionLaunchConfig::Local(LocalSessionConfig {
                                    name: name.into(),
                                    ..Default::default()
                                })
                            },
                            disconnected: false,
                        },
                    );
                }
                app.session.select_active_session("first");
                let mut selected = SendCommandSelection::default();
                selected.toggle("first".into(), nyaterm_transport::SessionKind::LocalPty);
                selected.toggle("second".into(), nyaterm_transport::SessionKind::Serial);
                app.set_send_command_target(SendCommandTarget::Sessions(selected), cx);
                app.send_command.set_target_picker_open(true);
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(width), px(400.)),
                            cx,
                        ))),
                        show: true,
                        focus: false,
                        ..Default::default()
                    },
                    move |_, cx| cx.new(|_| PickerPreview(app)),
                )
                .unwrap();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(800))
                    .await;
                let image = window
                    .update(cx, |_, window, _| window.render_to_image().unwrap())
                    .unwrap();
                cx.background_executor()
                    .spawn(async move {
                        image.save(output).unwrap();
                    })
                    .await;
                let shutdown = window
                    .update(cx, |preview, _, cx| {
                        preview
                            .0
                            .update(cx, |app, cx| app.shutdown_blocking_jobs(cx))
                    })
                    .unwrap();
                shutdown.await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
