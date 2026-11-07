use std::io::Read;
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

use gpui::{
    Entity, Modifiers, MouseButton, MouseDownEvent, NavigationDirection, Pixels, PlatformInput,
    Point, PointerCancelEvent, PointerCancelReason, TestAppContext, VisualTestContext, point, px,
    size,
};
use nyaterm_core::{AiModelConfigItem, AiModelSource, AiProviderKind};
use nyaterm_transport::TelnetSessionConfig;
use nyaterm_ui::{NyaDialogWindowExt as _, nya_popup_is_active};

use crate::features::NyaTermApp;
use crate::features::shell::state::NewSessionMenuAnchor;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{NavItem, PanelOpenMode, PanelSide, SessionLaunchConfig};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
    let app = app_with_visible_local_session(cx, root, "floating");
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        app.set_panel_open_mode(PanelOpenMode::Floating, cx);
        let mut settings = app.ai.settings_config_cloned();
        settings.enabled = true;
        settings.models = vec![AiModelConfigItem {
            id: "fixture".into(),
            name: "Fixture model".into(),
            provider_kind: Some(AiProviderKind::OpenaiCompatible),
            credential_id: None,
            enabled: true,
            source: AiModelSource::Manual,
            backend: Default::default(),
            last_seen_at: None,
            supported_reasoning_efforts: None,
        }];
        settings.default_model_id = Some("fixture".into());
        app.ai.replace_settings_config(settings, false);
        app.flush_ai_panel_snapshot(cx);
        app.flush_connection_panel_snapshot(cx);
        app.flush_transfer_panel_snapshot(cx);
        app.shell.chrome.activity_bar_layout.left_top = vec![
            "savedConnections".into(),
            "network".into(),
            "fileExplorer".into(),
        ];
        app.shell.chrome.activity_bar_layout.right_top = vec!["aiAssistant".into()];
        app.shell.chrome.activity_bar_layout.left_bottom.clear();
        app.shell.chrome.activity_bar_layout.right_bottom.clear();
    });
    app
}

fn window<'a>(cx: &'a mut TestAppContext, app: &Entity<NyaTermApp>) -> &'a mut VisualTestContext {
    let app = app.clone();
    let (_, vcx) = cx.add_window_view({
        let app = app.clone();
        move |window, cx| nyaterm_ui::nya_root(app.clone(), window, cx)
    });
    vcx.simulate_resize(size(px(1200.), px(800.)));
    draw(&app, vcx);
    vcx
}

fn draw(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let viewport = window.viewport_size();
            app.shell.set_viewport_size_for_test((
                f32::from(viewport.width),
                f32::from(viewport.height),
            ));
            cx.notify();
        });
        _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn click_icon(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, selector: &'static str) {
    draw(app, cx);
    let position = cx
        .debug_bounds(selector)
        .expect("activity icon is rendered")
        .center();
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(app, cx);
}

fn panels(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> [Option<NavItem>; 2] {
    cx.read(|cx| {
        let app = app.read(cx);
        [
            app.shell.floating_panel(PanelSide::Left),
            app.shell.floating_panel(PanelSide::Right),
        ]
    })
}

fn open_both(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    click_icon(app, cx, "activity-savedConnections");
    click_icon(app, cx, "activity-aiAssistant");
    assert_eq!(
        panels(app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
}

fn outside(cx: &mut VisualTestContext) -> Point<Pixels> {
    let left = cx.debug_bounds("floating-left-panel").unwrap();
    let right = cx.debug_bounds("floating-right-panel").unwrap();
    assert!(left.right() < right.left());
    point((left.right() + right.left()) / 2., left.center().y)
}

#[test]
fn activity_clicks_open_switch_and_close_without_dismissing_the_other_side() {
    let root = TestConfigDir::new("nyaterm-floating-activity-clicks");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    assert_eq!(panels(&app, cx), [None, None]);
    open_both(&app, cx);
    click_icon(&app, cx, "activity-network");
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Tunnels), Some(NavItem::AiAssistant)]
    );
    click_icon(&app, cx, "activity-network");
    assert_eq!(panels(&app, cx), [None, Some(NavItem::AiAssistant)]);
    click_icon(&app, cx, "activity-savedConnections");
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
}

#[test]
fn one_outside_click_closes_both_but_inside_either_keeps_both() {
    let root = TestConfigDir::new("nyaterm-floating-both-clicks");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    for selector in ["floating-left-panel", "floating-right-panel"] {
        let bounds = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(
            point(bounds.right() - px(8.), bounds.center().y),
            Modifiers::none(),
        );
        draw(&app, cx);
        assert_eq!(
            panels(&app, cx),
            [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
        );
    }
    let position = outside(cx);
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn drags_auxiliary_buttons_and_pointer_cancellation_keep_both_panels() {
    let root = TestConfigDir::new("nyaterm-floating-pointer-lifecycle");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let position = outside(cx);
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        position + point(px(20.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(position, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    cx.simulate_event(PointerCancelEvent {
        reason: PointerCancelReason::CancelMode,
    });
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    // A cancellation must not prevent the next clean click from dismissing.
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
    open_both(&app, cx);
    for button in [MouseButton::Right, MouseButton::Middle] {
        cx.simulate_mouse_down(position, button, Modifiers::none());
        cx.simulate_mouse_up(position, button, Modifiers::none());
        draw(&app, cx);
        assert_eq!(
            panels(&app, cx),
            [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
        );
    }
}

#[test]
fn escape_closes_the_remaining_panel_and_focus_changes_do_not_close_panels() {
    let root = TestConfigDir::new("nyaterm-floating-escape-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let prompt = cx.debug_bounds("ai-prompt").unwrap().center();
    cx.simulate_click(prompt, Modifiers::none());
    cx.simulate_input("draft");
    draw(&app, cx);
    cx.read(|cx| assert_eq!(app.read(cx).ai.chat_prompt_draft(), "draft"));
    cx.update(|window, cx| {
        let focus = app.read(cx).terminal.input_focus().clone();
        window.focus(&focus, cx);
    });
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.simulate_keystrokes("escape");
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [Some(NavItem::Connections), None]);
    cx.simulate_keystrokes("escape");
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn terminal_input_reaches_the_backend_before_opening_and_after_escape_or_outside_click() {
    let root = TestConfigDir::new("nyaterm-floating-terminal-input");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (manager, id) = app.update(&mut cx, |app, _| {
        let manager = app.session.manager_handle();
        let config = TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp: true,
            ..Default::default()
        };
        let info = manager.create_telnet_session(config.clone()).unwrap();
        let mut metadata = app.session.metadata("floating").unwrap().clone();
        metadata.launch_config = SessionLaunchConfig::Telnet(config);
        app.session.register_session_metadata(&info.id, metadata);
        app.terminal
            .seed_session_view(info.id.clone(), String::new(), "UTF-8");
        app.session.select_active_session(&info.id);
        (manager, info.id)
    });
    let mut stream = listener.accept().unwrap().0;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let cx = window(&mut cx, &app);
    cx.update(|window, cx| {
        let focus = app.read(cx).terminal.input_focus().clone();
        window.focus(&focus, cx);
    });
    draw(&app, cx);
    cx.simulate_input("before");
    click_icon(&app, cx, "activity-savedConnections");
    cx.update(|window, cx| assert!(app.read(cx).terminal.input_focus().is_focused(window)));
    cx.simulate_input("open");
    cx.simulate_keystrokes("escape");
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
    cx.simulate_input("after");
    open_both(&app, cx);
    let position = outside(cx);
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    cx.simulate_input("outside");
    let expected = b"beforeopenafteroutside";
    let mut actual = vec![0; expected.len()];
    stream.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    manager.close(&id).unwrap();
}

#[test]
fn connection_popup_outside_the_panel_dismisses_before_its_parent() {
    let root = TestConfigDir::new("nyaterm-floating-connection-popup");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let bounds = cx.debug_bounds("connections-list-scroll").unwrap();
    let position = point(bounds.right() - px(5.), bounds.top() + px(30.));
    cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
    draw(&app, cx);
    cx.update(|window, cx| assert!(nya_popup_is_active(window, cx)));
    let click = position + point(px(40.), px(10.));
    assert!(click.x > cx.debug_bounds("floating-left-panel").unwrap().right());
    cx.simulate_click(click, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    // The menu's first item opens the production connection editor.
    cx.read(|cx| {
        assert!(
            app.read(cx)
                .connection_state
                .active_editor_draft()
                .is_some()
        )
    });
}

#[test]
fn dialog_and_ai_menu_dismissal_clicks_preserve_the_floating_panels() {
    let root = TestConfigDir::new("nyaterm-floating-overlay-priority");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    click_icon(&app, cx, "ai-model-control");
    draw(&app, cx);
    assert!(cx.debug_bounds("ai-model-menu").is_some());
    let position = outside(cx);
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.read(|cx| assert!(!app.read(cx).ai.transient_menus_are_open()));
    cx.update(|window, cx| {
        window.open_nya_dialog(cx, |dialog, _, _| {
            dialog.title("Fixture dialog").content("Content")
        });
    });
    draw(&app, cx);
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.update(|window, cx| window.close_all_nya_dialogs(cx));
    draw(&app, cx);
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn ai_model_choice_beyond_a_narrow_panel_remains_clickable() {
    let root = TestConfigDir::new("nyaterm-floating-narrow-ai-menu");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    app.update(&mut cx, |app, cx| {
        app.shell.panels.right_width = 200.;
        let mut settings = app.ai.settings_config_cloned();
        let mut second = settings.models[0].clone();
        second.id = "second".into();
        second.name = "Second fixture model".into();
        settings.models.push(second);
        app.ai.replace_settings_config(settings, false);
        app.flush_ai_panel_snapshot(cx);
    });
    let cx = window(&mut cx, &app);
    cx.simulate_resize(size(px(900.), px(600.)));
    open_both(&app, cx);
    click_icon(&app, cx, "ai-model-control");
    let choice = cx.debug_bounds("ai-model-choice-second").unwrap();
    let click = point(choice.left() + px(5.), choice.center().y);
    assert!(click.x < cx.debug_bounds("floating-right-panel").unwrap().left());
    cx.simulate_click(click, Modifiers::none());
    draw(&app, cx);
    cx.read(|cx| {
        let app = app.read(cx);
        assert_eq!(
            app.ai.settings_config().default_model_id.as_deref(),
            Some("second")
        );
        assert!(!app.ai.transient_menus_are_open());
    });
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
}

#[test]
fn resized_panels_use_the_rendered_bounds_and_releasing_inside_either_does_not_close() {
    let root = TestConfigDir::new("nyaterm-floating-resized-bounds");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    for (viewport_width, left_width, right_width) in [(900., 200., 240.), (1280., 320., 280.)] {
        cx.simulate_resize(size(px(viewport_width), px(600.)));
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.shell.panels.left_width = left_width;
                app.shell.panels.right_width = right_width;
            });
        });
        open_both(&app, cx);
        for (side, selector) in [
            (PanelSide::Left, "floating-left-panel"),
            (PanelSide::Right, "floating-right-panel"),
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.read(|cx| {
                assert_eq!(
                    app.read(cx).floating_panel_window_bounds(side),
                    Some(bounds)
                )
            });
            let (press, release) = match side {
                PanelSide::Left => (
                    point(bounds.right() + px(1.), bounds.center().y),
                    point(bounds.right() - px(1.), bounds.center().y),
                ),
                PanelSide::Right => (
                    point(bounds.left() - px(1.), bounds.center().y),
                    point(bounds.left() + px(1.), bounds.center().y),
                ),
            };
            cx.simulate_mouse_down(press, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::none());
            draw(&app, cx);
            assert_eq!(
                panels(&app, cx),
                [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
            );
        }
        let position = outside(cx);
        cx.simulate_click(position, Modifiers::none());
        draw(&app, cx);
        assert_eq!(panels(&app, cx), [None, None]);
    }
}

// Exercise the exact method installed in Window's native outside callback.
// GPUI's test platform has no native child-window implementation, so model the
// two dispatch orders here; HWND hit testing remains a Windows manual check.
fn native_press(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    position: Point<Pixels>,
    button: MouseButton,
    owner_receives_press: bool,
) {
    let event = MouseDownEvent {
        position,
        button,
        ..Default::default()
    };
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_native_overlay_outside_mouse_down(&event, window, cx);
        });
        if owner_receives_press {
            window.dispatch_event(PlatformInput::MouseDown(event), cx);
        }
    });
}

#[test]
fn native_outside_press_only_dismisses_on_the_left_button_and_needs_no_release() {
    let root = TestConfigDir::new("nyaterm-floating-native-buttons");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let position = outside(cx);
    for button in [
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Navigate(NavigationDirection::Back),
        MouseButton::Navigate(NavigationDirection::Forward),
    ] {
        native_press(&app, cx, position, button, false);
        draw(&app, cx);
        assert_eq!(
            panels(&app, cx),
            [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
        );
    }
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
    open_both(&app, cx);
    let position = outside(cx);
    cx.simulate_click(position, Modifiers::none());
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn native_inside_press_keeps_either_floating_panel() {
    let root = TestConfigDir::new("nyaterm-floating-native-inside");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    for selector in ["floating-left-panel", "floating-right-panel"] {
        let position = cx.debug_bounds(selector).unwrap().center();
        native_press(&app, cx, position, MouseButton::Left, false);
        draw(&app, cx);
        assert_eq!(
            panels(&app, cx),
            [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
        );
    }
}

#[test]
fn owner_capture_takes_over_native_observation_and_waits_for_release() {
    let root = TestConfigDir::new("nyaterm-floating-native-owner");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let position = outside(cx);
    native_press(&app, cx, position, MouseButton::Left, true);
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn native_menu_dismissal_stays_blocked_when_owner_capture_follows() {
    let root = TestConfigDir::new("nyaterm-floating-native-menu-order");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    for owner_receives_press in [false, true] {
        open_both(&app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.shell
                    .open_new_session_menu(NewSessionMenuAnchor::MainTabStrip);
                cx.notify();
            })
        });
        draw(&app, cx);
        let position = outside(cx);
        native_press(&app, cx, position, MouseButton::Left, owner_receives_press);
        if owner_receives_press {
            cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
        }
        draw(&app, cx);
        cx.read(|cx| assert!(!app.read(cx).shell.new_session_menu_is_open()));
        assert_eq!(
            panels(&app, cx),
            [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
        );
        native_press(&app, cx, position, MouseButton::Left, false);
        draw(&app, cx);
        assert_eq!(panels(&app, cx), [None, None]);
    }
}

#[test]
fn native_outside_press_closes_docker_and_ai_menus_before_the_panels() {
    let root = TestConfigDir::new("nyaterm-floating-native-feature-menus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.remote_ops.toggle_docker_header_menu();
            app.flush_remote_panel_snapshots(cx);
        })
    });
    let position = outside(cx);
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    cx.read(|cx| assert!(!app.read(cx).remote_ops.docker_menus_open()));
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    click_icon(&app, cx, "ai-model-control");
    assert!(cx.debug_bounds("ai-model-menu").is_some());
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    cx.read(|cx| assert!(!app.read(cx).ai.transient_menus_are_open()));
    assert!(cx.debug_bounds("ai-model-menu").is_none());
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn native_observation_preserves_component_popup_dismissal_priority() {
    let root = TestConfigDir::new("nyaterm-floating-native-component-popup");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let bounds = cx.debug_bounds("connections-list-scroll").unwrap();
    let position = point(bounds.right() - px(5.), bounds.top() + px(30.));
    cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
    draw(&app, cx);
    cx.update(|window, cx| assert!(nya_popup_is_active(window, cx)));
    let position = outside(cx);
    native_press(&app, cx, position, MouseButton::Left, true);
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    cx.update(|window, cx| assert!(!nya_popup_is_active(window, cx)));
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn a_dialog_opened_before_native_defer_prevents_panel_dismissal() {
    let root = TestConfigDir::new("nyaterm-floating-native-deferred-dialog");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let position = outside(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_native_overlay_outside_mouse_down(
                &MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    ..Default::default()
                },
                window,
                cx,
            );
        });
        window.open_nya_dialog(cx, |dialog, _, _| {
            dialog.title("Fixture").content("Content")
        });
    });
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.update(|window, cx| window.close_all_nya_dialogs(cx));
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn native_observation_does_not_close_an_activity_bar_selection() {
    let root = TestConfigDir::new("nyaterm-floating-native-activity");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let position = cx.debug_bounds("activity-network").unwrap().center();
    native_press(&app, cx, position, MouseButton::Left, true);
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Tunnels), Some(NavItem::AiAssistant)]
    );
}

#[test]
fn native_observation_survives_owner_drag_and_cancellation_without_stale_dismissals() {
    let root = TestConfigDir::new("nyaterm-floating-native-cancel");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let position = outside(cx);
    native_press(&app, cx, position, MouseButton::Left, true);
    cx.simulate_mouse_move(
        position + point(px(20.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_native_overlay_outside_mouse_down(
                &MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    ..Default::default()
                },
                window,
                cx,
            );
            app.cancel_pointer_interactions(window, cx);
        })
    });
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn an_old_native_defer_cannot_finish_a_new_press_at_the_same_position() {
    let root = TestConfigDir::new("nyaterm-floating-native-stale");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    let event = MouseDownEvent {
        position: outside(cx),
        button: MouseButton::Left,
        ..Default::default()
    };
    let observed = std::rc::Rc::new(std::cell::Cell::new(false));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_native_overlay_outside_mouse_down(&event, window, cx)
        });
        let app_for_check = app.clone();
        let observed = observed.clone();
        window.defer(cx, move |_, cx| {
            let app = app_for_check.read(cx);
            assert_eq!(
                app.shell.floating_panel(PanelSide::Left),
                Some(NavItem::Connections)
            );
            observed.set(true);
        });
        app.update(cx, |app, cx| {
            app.handle_native_overlay_outside_mouse_down(&event, window, cx)
        });
    });
    draw(&app, cx);
    assert!(observed.get());
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn native_tab_actions_take_the_first_outside_click() {
    let root = TestConfigDir::new("nyaterm-floating-native-tab-menu");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    open_both(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let session_id = app.session.active_id().unwrap().to_owned();
            app.open_tab_actions_at(session_id, Some((100., 100.)), window, cx);
            assert!(app.session.dialog_tab_actions_target().is_some());
        })
    });
    draw(&app, cx);
    let position = outside(cx);
    native_press(&app, cx, position, MouseButton::Left, true);
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    draw(&app, cx);
    cx.read(|cx| assert!(app.read(cx).session.dialog_tab_actions_target().is_none()));
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Connections), Some(NavItem::AiAssistant)]
    );
    native_press(&app, cx, position, MouseButton::Left, false);
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn native_press_without_open_panels_is_a_noop() {
    let root = TestConfigDir::new("nyaterm-floating-native-no-panel");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    native_press(
        &app,
        cx,
        point(px(600.), px(400.)),
        MouseButton::Left,
        false,
    );
    draw(&app, cx);
    assert_eq!(panels(&app, cx), [None, None]);
}

#[test]
fn file_menu_keyboard_action_opens_a_dialog_without_dismissing_the_panels() {
    let root = TestConfigDir::new("nyaterm-floating-file-popup");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let cx = window(&mut cx, &app);
    click_icon(&app, cx, "activity-fileExplorer");
    click_icon(&app, cx, "activity-aiAssistant");
    let table = cx.debug_bounds("transfer-browser-table-scroll").unwrap();
    let position = point(table.right() - px(5.), table.center().y);
    cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
    draw(&app, cx);
    cx.update(|window, cx| assert!(nya_popup_is_active(window, cx)));
    // Local browser: Refresh, New file, New folder (separators are skipped).
    cx.simulate_keystrokes("down down down enter");
    draw(&app, cx);
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    cx.read(|cx| assert!(app.read(cx).transfer.new_folder_dialog().is_some()));
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Transfers), Some(NavItem::AiAssistant)]
    );
    cx.simulate_keystrokes("escape");
    draw(&app, cx);
    assert_eq!(
        panels(&app, cx),
        [Some(NavItem::Transfers), Some(NavItem::AiAssistant)]
    );
}
