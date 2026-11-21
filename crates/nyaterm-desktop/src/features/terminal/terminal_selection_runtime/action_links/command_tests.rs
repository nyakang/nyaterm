use std::{io::Read, net::TcpListener, time::Duration};

use gpui::{TestAppContext, px};
use nyaterm_transport::TelnetSessionConfig;

use crate::action_links::ActionLinkTarget;
use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use crate::models::{ActionLinkMenuAction, ActionLinkMenuState, SessionLaunchConfig};
use crate::test_support::TestConfigDir;

#[gpui::test]
fn action_link_command_dispatch_and_menu_follow_current_insert_setting(cx: &mut TestAppContext) {
    let root = TestConfigDir::new("nyaterm-action-link-command-wire");
    let app = app_with_visible_local_session(cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (manager, id) = app.update(cx, |app, _| {
        let manager = app.session.manager_handle();
        let config = TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp: true,
            ..Default::default()
        };
        let info = manager.create_telnet_session(config.clone()).unwrap();
        let mut metadata = app.session.metadata("fixture").unwrap().clone();
        metadata.launch_config = SessionLaunchConfig::Telnet(config);
        app.session.register_session_metadata(&info.id, metadata);
        app.terminal
            .seed_session_view(info.id.clone(), String::new(), "UTF-8");
        app.session.select_active_session(&info.id);
        (manager, info.id)
    });
    let mut stream = listener.accept().unwrap().0;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let cx = session_focus_test_window(cx, &app);
    // Return to execution after insertion to catch cached preference values.
    for insert in [false, true, false] {
        for via_menu in [false, true] {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let target = ActionLinkTarget::ExecuteCommand("echo action-link\r\n".into());
                    let menu = ActionLinkMenuState {
                        generation: 1,
                        session_id: id.clone(),
                        x: px(0.),
                        y: px(0.),
                        kind_label: "command".into(),
                        value: "fixture".into(),
                        actions: vec![ActionLinkMenuAction {
                            id: "fixture-command".into(),
                            label: "Fixture command".into(),
                            target: target.clone(),
                            mac_format: None,
                            is_default: true,
                        }],
                    };
                    if via_menu {
                        app.terminal.menus.action_link_menu = Some(menu.clone());
                    }
                    let mut settings = app.settings.summary().clone();
                    settings.terminal_action_links_insert_commands = insert;
                    app.settings.replace_summary(settings);
                    assert!(app.action_link_source_is_active(&id));
                    if via_menu {
                        app.execute_action_link_menu_action(&menu, target, window, cx);
                        assert!(app.terminal.menus.action_link_menu.is_none());
                    } else {
                        app.execute_action_link_target(target, &id, window, cx);
                    }
                    assert!(app.terminal.input.focus.is_focused(window));
                    // The sentinel makes an unexpected CR visible without a timeout.
                    app.write_session_protocol_response(&id, b"END").unwrap();
                });
            });
            let expected = if insert {
                b"echo action-linkEND".as_slice()
            } else {
                b"echo action-link\rEND".as_slice()
            };
            let mut actual = vec![0; expected.len()];
            stream.read_exact(&mut actual).unwrap();
            assert_eq!(actual, expected, "insert={insert}, via_menu={via_menu}");
        }
    }
    manager.close(&id).unwrap();
}
