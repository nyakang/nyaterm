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
                        "send-command.tab.1.draft"
                    } else {
                        "send-command.tab.1.hex"
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
        app.reset_text_input("send-command.tab.1.draft", "AT+CSQ", cx);
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

    struct PickerPreview {
        app: Entity<NyaTermApp>,
        send_tabs: bool,
    }
    impl Render for PickerPreview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                if self.send_tabs {
                    return div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .justify_end()
                        .p_4()
                        .child(app.bottom_command_send_bar(cx))
                        .into_any_element();
                }
                let state = app.send_command_bar_view_state(cx);
                div()
                    .size_full()
                    .bg(gpui::rgb(state.palette.surface))
                    .flex()
                    .items_end()
                    .p_4()
                    .child(app.send_command_target_picker(&state, cx))
                    .into_any_element()
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
    let send_tabs = std::env::var("NYATERM_SEND_TABS_PREVIEW_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let theme =
        std::env::var("NYATERM_SEND_TABS_PREVIEW_THEME").unwrap_or_else(|_| "github-dark".into());
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
            if let Some(bytes) = bytes {
                return Ok(Some(std::borrow::Cow::Borrowed(bytes)));
            }
            let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../nyaterm-app/assets")
                .join(path);
            Ok(std::fs::read(asset).ok().map(std::borrow::Cow::Owned))
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
                if let Some(count) = send_tabs {
                    app.send_command.set_target_picker_open(false);
                    app.settings.set_appearance_theme(theme.clone());
                    app.sync_component_theme(cx);
                    for _ in 1..count {
                        app.send_command.add_tab(cx);
                    }
                    app.send_command
                        .apply_draft("docker ps -a\ndocker images".into());
                    if std::env::var_os("NYATERM_SEND_TABS_PREVIEW_RUNNING").is_some() {
                        let labels = app.send_command.tab_labels();
                        for (index, (id, _)) in labels.iter().enumerate() {
                            if index != 0 && index != labels.len() - 1 {
                                continue;
                            }
                            app.send_command.switch_tab(*id);
                            app.send_command
                                .apply_draft("docker ps -a\ndocker images".into());
                            app.send_command.begin_send(10);
                            app.send_command.set_run_id(index as u64 + 1);
                            app.send_command
                                .set_progress_round(*id, index as u64 + 1, 1);
                            app.send_command
                                .complete_progress_units(*id, index as u64 + 1, 3);
                        }
                        app.send_command.switch_tab(labels[0].0);
                    }
                }
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
                    move |_, cx| {
                        cx.new(|_| PickerPreview {
                            app,
                            send_tabs: send_tabs.is_some(),
                        })
                    },
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
                            .app
                            .update(cx, |app, cx| app.shutdown_blocking_jobs(cx))
                    })
                    .unwrap();
                shutdown.await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[test]
fn command_send_tabs_keep_actions_visible_and_active_tab_reachable() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-send-tabs-layout");
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "layout-session",
    );
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
    for count in [1, 2, 10, 50, 100] {
        app.update(cx, |app, cx| {
            while app.send_command.tab_labels().len() < count {
                app.send_command.add_tab(cx);
            }
            cx.notify();
        });
        for width in [1200., 600., 420., 300.] {
            fixture.update(cx, |fixture, cx| {
                fixture.width = width;
                cx.notify();
            });
            cx.run_until_parked();
            for _ in 0..4 {
                cx.update(|window, cx| {
                    _ = window.draw(cx);
                });
                cx.run_until_parked();
            }
            let footer = cx.debug_bounds("send-command.footer").unwrap();
            let action = cx.debug_bounds("bottom-command-send").unwrap();
            let add = cx.debug_bounds("send-command.add-tab").unwrap();
            assert!(
                action.right() <= footer.right(),
                "send action outside footer: {count} tabs, width {width}"
            );
            assert!(
                add.right() <= action.left(),
                "add overlaps action: {count} tabs, width {width}"
            );
            assert_eq!(footer.size.height, px(30.));
            let (_id, overflow) = app.read_with(cx, |app, _| {
                (app.send_command.active_id(), app.send_command.tabs_overflow)
            });
            assert_eq!(
                cx.debug_bounds("send-command.active-tab").is_some(),
                count > 1
            );
            if count > 1 && (overflow || width < 360.) {
                assert!(cx.debug_bounds("send-command.all-tabs").is_some());
            }
            if count == 1 {
                assert!(cx.debug_bounds("send-command.all-tabs").is_none());
            }
            if count >= 10 && width >= 360. {
                assert!(overflow, "expected measured overflow");
                let bounds = cx.debug_bounds("send-command.active-tab").unwrap();
                let viewport = app.read_with(cx, |app, _| app.send_command.tab_scroll.bounds());
                assert!(bounds.left() >= viewport.left() - px(1.));
                assert!(bounds.right() <= viewport.right() + px(1.));
            }
        }
    }
}

#[test]
fn command_send_tabs_release_registered_inputs_and_preserve_background_drafts() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-send-tabs-inputs");
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "input-session",
    );
    let weak_inputs = app.update(&mut cx, |app, cx| {
        let mut weak_inputs = Vec::new();
        let first = app.send_command.active_id();
        let first_input = app.text_input(
            first.input_id("draft"),
            "",
            crate::features::text_inputs::TextInputSetup::multi_line(""),
            cx,
        );
        for _ in 0..100 {
            let id = app.send_command.add_tab(cx);
            let input = app.text_input(
                id.input_id("draft"),
                "",
                crate::features::text_inputs::TextInputSetup::multi_line(""),
                cx,
            );
            assert_ne!(input.entity_id(), first_input.entity_id());
            let weak = input.downgrade();
            drop(input);
            app.send_command.close_tabs(&[id]);
            app.forget_text_inputs(&id.input_id(""));
            app.forget_send_command_selects(&id.input_id(""));
            weak_inputs.push(weak);
        }
        assert_eq!(
            app.text_input_fields_snapshot()
                .keys()
                .filter(|id| id.starts_with("send-command.tab."))
                .count(),
            1
        );
        assert_eq!(app.send_command.active_id(), first);
        weak_inputs
    });
    cx.run_until_parked();
    assert!(weak_inputs.iter().all(|weak| weak.upgrade().is_none()));
}

#[test]
fn command_send_tab_text_editors_retain_independent_undo_history() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-send-tabs-undo");
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "undo-session",
    );
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        CommandPanelFixture {
            app: fixture_app,
            width: 800.,
        }
    });
    let first = app.read_with(cx, |app, _| app.send_command.active_id());
    cx.update(|window, cx| app.update(cx, |app, cx| app.focus_send_command_composer(window, cx)));
    cx.simulate_keystrokes("a b c");
    let second = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(app.send_command.presentation(cx).draft, "abc");
            let second = app.send_command.add_tab(cx);
            app.select_send_tab(second, window, cx);
            second
        })
    });
    cx.simulate_keystrokes("x y z");
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.select_send_tab(first, window, cx)));
    app.read_with(cx, |app, cx| {
        assert_eq!(app.send_command.presentation(cx).draft, "abc")
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    });
    app.read_with(cx, |app, cx| {
        assert_ne!(app.send_command.presentation(cx).draft, "abc")
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.select_send_tab(second, window, cx)));
    app.read_with(cx, |app, cx| {
        assert_ne!(app.send_command.presentation(cx).draft, "abc")
    });
}

#[test]
fn command_send_tabs_confirm_discard_and_rename_with_enter_or_escape() {
    use nyaterm_ui::NyaDialogWindowExt as _;
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-send-tabs-dialogs");
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "dialogs-session",
    );
    let (first, second) = app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
        let first = app.send_command.active_id();
        app.send_command.apply_draft("unsent".into());
        (first, app.send_command.add_tab(cx))
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let fixture = cx.new(|cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            CommandPanelFixture {
                app: fixture_app,
                width: 800.,
            }
        });
        nyaterm_ui::nya_root(fixture, window, cx)
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.request_close_send_tabs(vec![first], window, cx)
        })
    });
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |app, _| {
        assert_eq!(app.send_command.tab_labels().len(), 2)
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.request_close_send_tabs(vec![first], window, cx)
        })
    });
    cx.simulate_keystrokes("enter");
    app.read_with(cx, |app, _| {
        assert_eq!(app.send_command.tab_labels().len(), 1);
        assert_eq!(app.send_command.active_id(), second);
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.rename_send_tab(second, window, cx)));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a n a m e enter"
    } else {
        "ctrl-a n a m e enter"
    });
    app.read_with(cx, |app, _| {
        assert_eq!(app.send_command.tab_name(second), Some("name"));
        assert_eq!(app.send_command.active_id(), second);
    });
    cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
    cx.update(|window, cx| app.update(cx, |app, cx| app.rename_send_tab(second, window, cx)));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a x escape"
    } else {
        "ctrl-a x escape"
    });
    app.read_with(cx, |app, _| {
        assert_eq!(app.send_command.tab_name(second), Some("name"))
    });
}

#[test]
fn command_send_controls_and_stop_button_follow_active_tab_with_multiple_runs() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-send-tabs-concurrent-controls");
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "layout-session",
    );
    let (first, second, idle, first_run, second_run) = app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
        let first = app.send_command.active_id();
        let first_run = app.send_command.begin_send(5);
        app.send_command.set_run_id(101);
        app.send_command.complete_progress_units(first, 101, 2);
        let second = app.send_command.add_tab(cx);
        let second_run = app.send_command.begin_send(7);
        app.send_command.set_run_id(102);
        app.send_command.complete_progress_units(second, 102, 4);
        let idle = app.send_command.add_tab(cx);
        let state = app.send_command_bar_view_state(cx);
        assert!(!state.is_sending);
        assert!(state.send_disabled);
        app.send_command.apply_draft("ready".into());
        let state = app.send_command_bar_view_state(cx);
        assert!(!state.send_disabled);
        assert!(!state.is_sending);
        assert_eq!(state.send.completed, 0);
        (first, second, idle, first_run, second_run)
    });
    let fixture_app = app.clone();
    let (fixture, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        CommandPanelFixture {
            app: fixture_app,
            width: 1200.,
        }
    });
    for (id, completed, run) in [(first, 2, &first_run), (second, 4, &second_run)] {
        app.update(cx, |app, cx| {
            app.send_command.switch_tab(id);
            let state = app.send_command_bar_view_state(cx);
            assert!(state.is_sending);
            assert!(!state.send_disabled);
            assert_eq!(state.send.completed, completed);
            cx.notify();
        });
        fixture.update(cx, |fixture, cx| {
            fixture.width = 300.;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let action = cx.debug_bounds("bottom-command-send").unwrap();
        cx.simulate_click(action.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(run.cancel.load(std::sync::atomic::Ordering::Acquire));
        if id == first {
            assert!(!second_run.cancel.load(std::sync::atomic::Ordering::Acquire));
        }
        app.read_with(cx, |app, _| {
            assert!(app.send_command.tab_is_sending(first));
            assert!(app.send_command.tab_is_sending(second));
            assert!(!app.send_command.tab_is_sending(idle));
        });
    }
}

#[test]
fn command_send_bar_exposes_shortcut_hints_broadcast_state_and_wheel_tab_scroll() {
    use super::state::send_shortcut;
    use super::tabs::{display_send_tab_name, send_tab_scroll_x};
    use crate::send_command::{SendCommandSelection, SendCommandTarget};

    crate::preload_i18n().unwrap();
    assert_eq!(display_send_tab_name("1"), "#1");
    assert_eq!(display_send_tab_name("42"), "#42");
    assert_eq!(display_send_tab_name("custom"), "custom");
    assert_eq!(send_tab_scroll_x(-20., 120., 0., -32.), -52.);
    assert_eq!(send_tab_scroll_x(-100., 120., 0., -40.), -120.);
    assert_eq!(send_tab_scroll_x(-10., 120., 0., 40.), 0.);

    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-send-bar-hints-and-broadcast");
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "first",
    );
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
        let metadata = app.session.metadata("first").unwrap().clone();
        app.session.register_session_metadata("second", metadata);

        let empty_state = app.send_command_bar_view_state(cx);
        assert!(!empty_state.is_broadcast);
        assert_eq!(empty_state.target_count, 1);
        assert!(empty_state.input_hint.contains(send_shortcut()));
        assert!(empty_state.action_hint.contains(send_shortcut()));

        app.set_send_command_data_type(SendCommandDataType::Hex, cx);
        let hex_state = app.send_command_bar_view_state(cx);
        assert!(hex_state.input_hint.contains(send_shortcut()));

        app.set_send_command_data_type(SendCommandDataType::Text, cx);
        app.apply_send_command_draft("uptime".into(), cx);
        let mut selected = SendCommandSelection::default();
        selected.toggle("first".into(), nyaterm_transport::SessionKind::LocalPty);
        selected.toggle("second".into(), nyaterm_transport::SessionKind::LocalPty);
        app.set_send_command_target(SendCommandTarget::Sessions(selected), cx);

        let broadcast_state = app.send_command_bar_view_state(cx);
        assert!(broadcast_state.is_broadcast);
        assert_eq!(broadcast_state.target_count, 2);
        assert!(broadcast_state.action_hint.contains("(2)"));
        assert!(broadcast_state.action_hint.contains(send_shortcut()));
    });
}
