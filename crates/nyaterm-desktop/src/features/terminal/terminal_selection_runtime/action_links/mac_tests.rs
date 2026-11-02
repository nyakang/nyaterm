use std::{io::Read, net::TcpListener, time::Duration};

use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    KeyDownEvent, KeyUpEvent, Keystroke, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use nyaterm_core::{ActionLinksMatcherSettings, mac_address::MacAddressFormat};
use nyaterm_transport::TelnetSessionConfig;
use nyaterm_ui::nya_root;

use crate::action_links::{ActionLinkTarget, actions_for_match, find_action_links};
use crate::features::NyaTermApp;
use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use crate::models::{ActionLinkMenuAction, ActionLinkMenuState, SessionLaunchConfig};
use crate::test_support::TestConfigDir;

fn menu(session_id: &str, generation: u64) -> ActionLinkMenuState {
    let item = find_action_links(
        "905d.7c16.b6d9",
        &ActionLinksMatcherSettings::default(),
        true,
    )
    .remove(0);
    ActionLinkMenuState {
        generation,
        session_id: session_id.into(),
        x: px(900.),
        y: px(700.),
        kind_label: "MAC".into(),
        value: item.value.clone(),
        actions: actions_for_match(&item)
            .into_iter()
            .map(|action| ActionLinkMenuAction {
                id: action.id,
                label: action.label,
                target: action.target,
                mac_format: action.mac_format,
                is_default: action.is_default,
            })
            .collect(),
    }
}

#[gpui::test]
fn mac_copy_has_no_wire_effect_and_insert_preserves_paste_framing_and_sync(
    cx: &mut TestAppContext,
) {
    let root = TestConfigDir::new("nyaterm-mac-actions-wire");
    let app = app_with_visible_local_session(cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (manager, ids) = app.update(cx, |app, _| {
        let manager = app.session.manager_handle();
        let metadata = app.session.metadata("fixture").unwrap().clone();
        app.sync_input.create_group();
        let mut ids = Vec::new();
        for encoding in ["GBK", "UTF-8"] {
            let config = TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp: true,
                encoding: encoding.into(),
                ..Default::default()
            };
            let info = manager.create_telnet_session(config.clone()).unwrap();
            let mut metadata = metadata.clone();
            metadata.launch_config = SessionLaunchConfig::Telnet(config);
            app.session.register_session_metadata(&info.id, metadata);
            app.terminal
                .seed_session_view(info.id.clone(), String::new(), encoding);
            app.sync_input.toggle_selected_session(info.id.clone());
            ids.push(info.id);
        }
        app.session.select_active_session(&ids[0]);
        app.terminal
            .view
            .views
            .get_mut(&ids[1])
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        (manager, ids)
    });
    let mut streams = (0..2)
        .map(|_| {
            let stream = listener.accept().unwrap().0;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
        })
        .collect::<Vec<_>>();
    let cx = session_focus_test_window(cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.action_link_source_is_active(&ids[0]));
            let candidates = menu(&ids[0], 1);
            let copy = candidates
                .actions
                .iter()
                .find(|action| action.is_default)
                .unwrap()
                .target
                .clone();
            app.execute_action_link_target(copy, &ids[0], window, cx);
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("90:5D:7C:16:B6:D9")
            );
            let paste = candidates
                .actions
                .iter()
                .find(|action| {
                    action.mac_format == Some(MacAddressFormat::GroupHyphen)
                        && matches!(action.target, ActionLinkTarget::PasteText(_))
                })
                .unwrap()
                .target
                .clone();
            app.execute_action_link_target(paste, &ids[0], window, cx);
            // Ordinary paste remains byte-for-byte unchanged, even for a MAC.
            app.paste_terminal_text("90:5d:7c:16:b6:d9".into(), window, cx);
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("90:5D:7C:16:B6:D9")
            );

            // Every rejected operation would appear before END if it wrote any bytes.
            let rejected = ActionLinkTarget::PasteText("REJECTED".into());
            app.execute_action_link_target(rejected.clone(), &ids[1], window, cx);
            app.terminal.menus.action_link_menu = Some(menu(&ids[0], 2));
            app.execute_action_link_menu_action(&candidates, rejected.clone(), window, cx);
            app.security.activate_screen_lock(String::new());
            app.execute_action_link_target(rejected.clone(), &ids[0], window, cx);
            app.security.deactivate_screen_lock();
            let mut metadata = app.session.metadata(&ids[0]).unwrap().clone();
            metadata.disconnected = true;
            app.session.register_session_metadata(&ids[0], metadata);
            app.execute_action_link_target(rejected, &ids[0], window, cx);
            for id in &ids {
                app.write_session_protocol_response(id, b"END").unwrap();
            }
            assert!(app.terminal.input.focus.is_focused(window));
        })
    });
    for (stream, expected) in streams.iter_mut().zip([
        b"905D-7C16-B6D990:5d:7c:16:b6:d9END".as_slice(),
        b"\x1b[200~905D-7C16-B6D9\x1b[201~\x1b[200~90:5d:7c:16:b6:d9\x1b[201~END".as_slice(),
    ]) {
        let mut actual = vec![0; expected.len()];
        stream.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }
    for id in ids {
        manager.close(&id).unwrap();
    }
}

struct MenuHost {
    app: Entity<NyaTermApp>,
}

impl Render for MenuHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let overlay = self.app.update(cx, |app, cx| {
            app.shell
                .set_viewport_size_for_test((viewport.width.into(), viewport.height.into()));
            app.action_link_menu_overlay(cx).into_any_element()
        });
        let focus = self.app.read(cx).terminal.input_focus().clone();
        div()
            .size_full()
            .relative()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this
                    .app
                    .update(cx, |app, cx| app.handle_global_shortcut(event, window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            .child(div().track_focus(&focus))
            .child(overlay)
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn press_key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).unwrap();
    // Native button activation completes on key-up, unlike menu key bindings.
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
}

#[gpui::test]
fn mac_menu_has_five_button_pairs_supports_keyboard_and_restores_focus(cx: &mut TestAppContext) {
    let root = TestConfigDir::new("nyaterm-mac-menu-keyboard");
    let app = app_with_visible_local_session(cx, root.path(), "source");
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        app.terminal.menus.action_link_menu = Some(menu("source", 1));
    });
    let host_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        cx.observe(&host_app, |_, _, cx| cx.notify()).detach();
        let host = cx.new(|_| MenuHost { app: host_app });
        nya_root(host, window, cx)
    });
    draw(cx);
    let bounds = cx.debug_bounds("action-link-menu").unwrap();
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(bounds.right() <= viewport.width - px(8.));
    assert!(bounds.bottom() <= viewport.height - px(8.));
    for (copy_id, paste_id) in [
        (
            "action-link-menu-mac-copy-0",
            "action-link-menu-mac-paste-0",
        ),
        (
            "action-link-menu-mac-copy-1",
            "action-link-menu-mac-paste-1",
        ),
        (
            "action-link-menu-mac-copy-2",
            "action-link-menu-mac-paste-2",
        ),
        (
            "action-link-menu-mac-copy-3",
            "action-link-menu-mac-paste-3",
        ),
        (
            "action-link-menu-mac-copy-4",
            "action-link-menu-mac-paste-4",
        ),
    ] {
        let copy = cx.debug_bounds(copy_id).unwrap();
        let paste = cx.debug_bounds(paste_id).unwrap();
        assert_eq!(copy.origin.y, paste.origin.y);
        assert!(paste.origin.x >= copy.right());
    }
    press_key(cx, "enter");
    cx.update(|window, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("90-5D-7C-16-B6-D9")
        );
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
        assert!(app.read(cx).terminal.menus.action_link_menu.is_none());
    });
    draw(cx);
    app.update(cx, |app, cx| {
        app.terminal.menus.action_link_menu = Some(menu("source", 2));
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.notify();
    });
    draw(cx);
    press_key(cx, "tab");
    press_key(cx, "enter");
    cx.update(|window, cx| {
        assert!(app.read(cx).terminal.menus.action_link_menu.is_none());
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("unchanged")
        );
    });
    draw(cx);
    app.update(cx, |app, cx| {
        app.terminal.menus.action_link_menu = Some(menu("source", 3));
        cx.notify();
    });
    draw(cx);
    press_key(cx, "escape");
    cx.update(|window, cx| {
        assert!(app.read(cx).terminal.menus.action_link_menu.is_none());
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
    });
    draw(cx);
    app.update(cx, |app, cx| {
        app.terminal.menus.action_link_menu = Some(menu("source", 4));
        cx.notify();
    });
    draw(cx);
    let copy = cx.debug_bounds("action-link-menu-mac-copy-4").unwrap();
    cx.simulate_click(copy.center(), gpui::Modifiers::default());
    cx.update(|window, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("905D7C16B6D9")
        );
        assert!(app.read(cx).terminal.menus.action_link_menu.is_none());
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
    });
}

#[gpui::test]
fn switching_session_closes_mac_menu_and_rejects_old_callbacks(cx: &mut TestAppContext) {
    let root = TestConfigDir::new("nyaterm-mac-menu-session-switch");
    let app = app_with_visible_local_session(cx, root.path(), "source");
    let cx = session_focus_test_window(cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let original = menu("source", 1);
            app.terminal.menus.action_link_menu = Some(original.clone());
            let metadata = app.session.metadata("source").unwrap().clone();
            app.session.register_session_metadata("other", metadata);
            app.terminal
                .seed_session_view("other".into(), String::new(), "UTF-8");
            app.activate_session_id("other", cx);
            assert!(app.terminal.menus.action_link_menu.is_none());
            cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
            app.execute_action_link_menu_action(
                &original,
                ActionLinkTarget::CopyText("REJECTED".into()),
                window,
                cx,
            );
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("unchanged")
            );
        })
    });
}
