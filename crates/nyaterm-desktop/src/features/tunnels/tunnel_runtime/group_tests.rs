use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Context, Modifiers, Render, TestAppContext, VisualTestContext, Window, div,
};
use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

use crate::features::test_support::app_with_visible_local_session;
use crate::models::NetworkTab;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
}

#[test]
fn network_group_dialog_focuses_name_and_closes_after_one_save() {
    for tab in [NetworkTab::Tunnels, NetworkTab::Proxies] {
        let root = TestConfigDir::new("nyaterm-network-group-dialog");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "network-group");
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let host = cx.new(|_| Host);
        let (_, cx) = cx.add_window_view(move |window, cx| nya_root(host, window, cx));

        // Reopening for rename must seed and focus a fresh input too.
        let mut group_id = None;
        for name in ["Created group", "Renamed group"] {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.open_network_group_editor(tab, group_id.clone(), window, cx);
                });
            });
            draw(cx);
            draw(cx);
            cx.update(|window, cx| {
                app.read_with(cx, |app, cx| {
                    let field = app
                        .existing_text_input("network.group-editor.name")
                        .unwrap();
                    assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
                });
            });

            if group_id.is_none() {
                let save = cx.debug_bounds("nya-dialog-action-button").unwrap();
                cx.simulate_click(save.center(), Modifiers::default());
                draw(cx);
                cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
                app.read_with(cx, |app, _| {
                    assert!(
                        app.connection_state
                            .active_network_group_editor()
                            .unwrap()
                            .error
                            .is_some()
                    );
                });
            }

            app.update(cx, |app, cx| {
                app.reset_text_input("network.group-editor.name", name, cx);
                app.apply_network_group_editor_name(name.into(), cx);
            });
            draw(cx);
            let save = cx.debug_bounds("nya-dialog-action-button").unwrap();
            cx.simulate_click(save.center(), Modifiers::default());
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                draw(cx);
                if cx.update(|window, cx| !window.has_active_nya_dialog(cx)) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "successful save left the dialog open"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            group_id = app.read_with(cx, |app, _| {
                assert!(app.connection_state.active_network_group_editor().is_none());
                assert!(
                    app.existing_text_input("network.group-editor.name")
                        .is_none()
                );
                let (id, saved_name, count) = match tab {
                    NetworkTab::Tunnels => {
                        let groups = app.tunnel_state.tunnel_groups();
                        (&groups[0].id, &groups[0].name, groups.len())
                    }
                    NetworkTab::Proxies => {
                        let groups = app.tunnel_state.proxy_groups();
                        (&groups[0].id, &groups[0].name, groups.len())
                    }
                };
                assert_eq!(count, 1);
                assert_eq!(saved_name, name);
                Some(id.clone())
            });
        }
    }
}
