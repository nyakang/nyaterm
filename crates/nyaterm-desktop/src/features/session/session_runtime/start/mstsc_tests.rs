use gpui::{
    AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, Render,
    Styled as _, TestAppContext, div,
};
use nyaterm_core::SavedConnection;

use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn connection() -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id":"mstsc", "name":"System RDP", "type":"rdp", "backend":"mstsc",
        "host":"example.test", "port":3390,
        "auth":{"mode":"password", "password_id":"missing-account"}
    }))
    .unwrap()
}

struct FocusHost(FocusHandle);

impl Render for FocusHost {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().track_focus(&self.0)
    }
}

#[cfg(windows)]
#[test]
fn mstsc_launch_result_updates_usage_only_on_success_and_never_registers_a_session() {
    use crate::platform::mstsc::MstscError;
    use nyaterm_store::StoreDomain;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    for succeeds in [true, false] {
        let root = TestConfigDir::new("nyaterm-mstsc-launch-result");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let store = app.update(&mut cx, |app, _| app.store_blocking_client());
        let saved = connection();
        let persisted = saved.clone();
        store
            .request_fn(StoreDomain::Connections, move |store| {
                store.save_connection(&persisted)
            })
            .unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let sync_generation = store.sync_mutation_generation();
        let launched = called.clone();
        app.update(&mut cx, |app, cx| {
            app.connection_state
                .replace_loaded(vec![saved.clone()], Vec::new());
            app.shell.select_nav(crate::models::NavItem::Connections);
            app.continue_saved_connection_start_with_launcher(
                saved,
                Default::default(),
                move |_| {
                    launched.store(true, Ordering::Release);
                    if succeeds {
                        Ok(())
                    } else {
                        Err(MstscError::Spawn(std::io::ErrorKind::NotFound.into()))
                    }
                },
                cx,
            );
            assert!(!app.session.start_has_pending());
            assert_eq!(app.session.session_order(), &["existing"]);
        });
        let expected = if succeeds {
            rust_i18n::t!("dialog.rdpMstscLaunched").to_string()
        } else {
            super::super::mstsc::error_message(MstscError::Spawn(
                std::io::ErrorKind::NotFound.into(),
            ))
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let done = app.update(&mut cx, |app, _| {
                let usage_updated = app
                    .connection_state
                    .connections()
                    .iter()
                    .find(|connection| connection.id == "mstsc")
                    .is_some_and(|connection| connection.last_used_at_ms.is_some());
                app.shell.status() == expected && usage_updated == succeeds
            });
            if done {
                break;
            }
            assert!(Instant::now() < deadline, "mstsc result was not applied");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(called.load(Ordering::Acquire));
        app.update(&mut cx, |app, _| {
            assert_eq!(
                app.shell.selected_nav(),
                crate::models::NavItem::Connections
            );
            assert_eq!(app.session.session_order(), &["existing"]);
            assert_eq!(app.session.active_id(), Some("existing"));
            assert!(!app.session.start_has_pending());
            assert!(
                !app.session
                    .start_saved_connection_is_pending_or_preparing(&connection())
            );
            assert!(!app.remote_desktop.has_sessions());
            assert!(app.remote_desktop.prepared_routes.is_empty());
            assert!(app.session.prompt_active_credential().is_none());
        });
        let persisted = store
            .request_fn(StoreDomain::Connections, |store| {
                store.get_connection("mstsc")
            })
            .unwrap()
            .unwrap();
        assert_eq!(persisted.last_used_at_ms.is_some(), succeeds);
        assert_eq!(store.sync_mutation_generation(), sync_generation);
    }
}

#[test]
fn mstsc_rejects_relay_before_reserving_tabs_loading_credentials_or_calling_launcher() {
    let root = TestConfigDir::new("nyaterm-mstsc-reject-route");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut saved = connection();
    saved.network = Some(nyaterm_core::ConnectionNetwork {
        proxy_id: Some("missing-proxy".into()),
        proxy_jump_id: None,
        host_key_alias: None,
    });
    app.update(&mut cx, |app, cx| {
        app.continue_saved_connection_start_with_launcher(
            saved,
            Default::default(),
            |_| panic!("invalid route must not launch"),
            cx,
        );
        assert!(!app.session.start_has_pending());
        assert_eq!(app.session.session_order(), &["existing"]);
        assert!(app.remote_desktop.prepared_routes.is_empty());
        assert!(!app.remote_desktop.has_sessions());
        let key = if cfg!(windows) {
            "dialog.rdpMstscRelayUnsupported"
        } else {
            "dialog.rdpMstscWindowsOnly"
        };
        assert_eq!(app.shell.status(), rust_i18n::t!(key));
    });
}

#[test]
fn mstsc_start_keeps_the_current_input_focus_even_when_launch_validation_fails() {
    let root = TestConfigDir::new("nyaterm-mstsc-keep-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let focus = app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.terminal.input_focus().clone()
    });
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|_| FocusHost(focus));
        nyaterm_ui::nya_root(view, window, cx)
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            window.focus(app.terminal.input_focus(), cx);
            let mut saved = connection();
            if let nyaterm_core::ConnectionType::Rdp { host, .. } = &mut saved.config {
                host.clear();
            }
            app.start_saved_connection(saved, window, cx);
            assert!(app.terminal.input_focus().is_focused(window));
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| assert!(app.read(cx).terminal.input_focus().is_focused(window)));
}
