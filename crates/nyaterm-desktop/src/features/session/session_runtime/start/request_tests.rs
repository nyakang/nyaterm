use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use nyaterm_core::SavedConnection;
use nyaterm_transport::network_route::{NetworkRoute, NetworkRouteConfig};

use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::session::SavedConnectionStartOptions;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{NavItem, SessionLaunchConfig, WorkspaceTabId};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn saved(protocol: &str) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": "saved", "name": "same host", "type": protocol,
        "host": "127.0.0.1", "port": 9,
    }))
    .unwrap()
}

#[test]
fn repeated_saved_ssh_opens_create_independent_tabs_before_workers_finish() {
    let root = TestConfigDir::new("nyaterm-concurrent-saved-ssh");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    app.update(&mut cx, |app, cx| {
        app.continue_saved_connection_start(saved("ssh"), Default::default(), cx);
        app.continue_saved_connection_start(saved("ssh"), Default::default(), cx);
        assert_eq!(app.session.start.pending_count(), 2);
        assert_eq!(app.session_strip_tabs(&None).len(), 3);
        let starts = app.session.start.pending_entries().collect::<Vec<_>>();
        assert_ne!(starts[0].0, starts[1].0);
        assert_ne!(starts[0].1.multiplex_key, starts[1].1.multiplex_key);
        let first = starts[0].0.clone();
        let second = starts[1].0.clone();
        let second_attempt = starts[1].1.attempt.clone();
        app.close_pending_session_start(first, cx);
        assert!(second_attempt.check().is_ok());
        assert_eq!(app.session.start.pending_count(), 1);
        app.close_pending_session_start(second, cx);
    });
}

#[test]
fn cancelled_saved_password_load_cannot_resurrect_a_tab_or_end_its_sibling() {
    let root = TestConfigDir::new("nyaterm-cancel-saved-password");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let mut connection = saved("vnc");
    connection.auth = Some(
        serde_json::from_value(serde_json::json!({
            "mode":"password", "password_id":"missing-test-password", "has_password":true
        }))
        .unwrap(),
    );
    app.update(&mut cx, |app, cx| {
        app.continue_saved_connection_start(connection.clone(), Default::default(), cx);
        let first = app
            .session
            .start
            .pending_entries()
            .next()
            .unwrap()
            .0
            .clone();
        app.continue_saved_connection_start(connection, Default::default(), cx);
        assert_eq!(app.session.start.pending_count(), 2);
        app.close_pending_session_start(first, cx);
        assert_eq!(app.session_strip_tabs(&None).len(), 2);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let done = app.update(&mut cx, |app, _| !app.session.start.has_pending());
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "credential response was not delivered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    app.update(&mut cx, |app, _| {
        assert!(app.session.start.has_failed());
        assert_eq!(app.session_strip_tabs(&None).len(), 2);
        assert!(!app.remote_desktop.has_sessions());
    });
}

#[test]
fn same_saved_remote_desktop_hands_each_route_to_its_own_tab_without_stealing_selection() {
    for protocol in ["rdp", "vnc"] {
        let root = TestConfigDir::new("nyaterm-concurrent-remote-routes");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        app.update(&mut cx, |app, cx| {
            let connection = saved(protocol);
            let mut requests = Vec::new();
            for _ in 0..2 {
                let mut options = app.prepare_session_start_options(Default::default());
                options.request_id = Some(
                    app.session
                        .start
                        .begin_saved_preparation(&connection, &options),
                );
                let route = NetworkRoute::start(NetworkRouteConfig {
                    host: "127.0.0.1".into(),
                    port: 9,
                    proxy: None,
                    jump: None,
                })
                .unwrap();
                requests.push((options, route));
            }
            let (first, first_route) = requests.remove(0);
            let first_request = first.request_id.clone().unwrap();
            let first_attempt = app.session.start.attempt(&first_request);
            let first_address = first_route.endpoint.address;
            app.start_saved_connection_ready(connection.clone(), first, Some(first_route), cx);
            assert_eq!(app.session.active_id(), Some("existing"));
            app.activate_session_id("existing", cx);
            let (second, second_route) = requests.remove(0);
            let second_request = second.request_id.clone().unwrap();
            let second_attempt = app.session.start.attempt(&second_request);
            let second_address = second_route.endpoint.address;
            app.start_saved_connection_ready(connection, second, Some(second_route), cx);
            assert_ne!(first_address, second_address);
            assert_eq!(app.session.active_id(), Some("existing"));
            assert!(!app.session.start.has_pending());
            let routes = app
                .session
                .ordered_sessions()
                .into_iter()
                .filter_map(|session| {
                    let metadata = app.session.metadata(&session.id)?;
                    let relay = match &metadata.launch_config {
                        SessionLaunchConfig::Rdp(config) => config.relay.as_ref(),
                        SessionLaunchConfig::Vnc(config) => config.relay.as_ref(),
                        _ => None,
                    }?;
                    assert_eq!(
                        app.remote_desktop.routes[&session.id].endpoint.address,
                        relay.address
                    );
                    Some((session.id, relay.address))
                })
                .collect::<Vec<_>>();
            assert_eq!(
                routes
                    .iter()
                    .map(|(_, address)| *address)
                    .collect::<Vec<_>>(),
                [first_address, second_address]
            );
            assert!(
                app.remote_desktop
                    .start_matches(&routes[0].0, &first_request)
            );
            assert!(
                app.remote_desktop
                    .start_matches(&routes[1].0, &second_request)
            );
            let _ = app.close_remote_desktop_runtime(&routes[0].0);
            assert!(first_attempt.is_cancelled());
            assert!(!second_attempt.is_cancelled());
        });
    }
}

#[test]
fn cancelled_route_preparation_drops_a_late_route_without_creating_a_runtime() {
    let root = TestConfigDir::new("nyaterm-cancel-late-route");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    app.update(&mut cx, |app, cx| {
        let connection = saved("vnc");
        let mut options = app.prepare_session_start_options(SavedConnectionStartOptions::default());
        options.request_id = Some(
            app.session
                .start
                .begin_saved_preparation(&connection, &options),
        );
        app.close_pending_session_start(options.request_id.clone().unwrap(), cx);
        let route = NetworkRoute::start(NetworkRouteConfig {
            host: "127.0.0.1".into(),
            port: 9,
            proxy: None,
            jump: None,
        })
        .unwrap();
        let weak = Arc::downgrade(&route);
        app.start_saved_connection_ready(connection, options, Some(route), cx);
        assert!(weak.upgrade().is_none());
        assert_eq!(app.session_strip_tabs(&None).len(), 1);
        assert!(!app.remote_desktop.has_sessions());
    });
}

#[test]
fn background_remote_handoff_keeps_selected_settings_without_an_active_terminal() {
    for protocol in ["rdp", "vnc"] {
        let root = TestConfigDir::new("nyaterm-background-remote-settings");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        app.update(&mut cx, |app, cx| {
            let connection = saved(protocol);
            let mut options = app.prepare_session_start_options(Default::default());
            options.request_id = Some(
                app.session
                    .start
                    .begin_saved_preparation(&connection, &options),
            );
            app.open_page(NavItem::Settings, cx);
            assert!(app.shell.has_settings_draft());
            app.session.clear_active_session();
            let selected = WorkspaceTabId::settings_key();
            assert_eq!(app.active_workspace_tab(), Some(selected.clone()));
            app.start_saved_connection_ready(connection, options, None, cx);
            assert!(!app.session.start.has_pending());
            assert_eq!(app.session.session_order_len(), 2);
            assert_eq!(app.active_workspace_tab(), Some(selected));
            assert!(app.session.active_id().is_none());
        });
    }
}

#[test]
fn queued_route_failure_keeps_its_tab_after_a_same_host_preparation_is_closed() {
    for protocol in ["rdp", "vnc"] {
        let scheduler = BlockingJobScheduler::with_limits(2, 8).unwrap();
        let (entered, waiting) = mpsc::channel();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (release, held) = mpsc::channel::<()>();
            releases.push(release);
            let entered = entered.clone();
            scheduler
                .submit_detached("hold-route-test-worker", move |_| {
                    entered.send(()).unwrap();
                    let _ = held.recv();
                })
                .unwrap();
        }
        for _ in 0..2 {
            waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let root = TestConfigDir::new("nyaterm-queued-route-failure");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let worker_releases = releases;
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let (remaining, placement) = app.update(&mut cx, |app, cx| {
            app.blocking_jobs = scheduler.clone();
            let mut connection = saved(protocol);
            connection.network = Some(
                serde_json::from_value(serde_json::json!({"proxy_id":"missing-test-proxy"}))
                    .unwrap(),
            );
            app.continue_saved_connection_start(connection.clone(), Default::default(), cx);
            let first = app
                .session
                .start
                .pending_entries()
                .next()
                .unwrap()
                .0
                .clone();
            app.continue_saved_connection_start(connection, Default::default(), cx);
            assert_eq!(app.session_strip_tabs(&None).len(), 3);
            assert!(
                app.session
                    .start
                    .pending_entries()
                    .all(|(_, p)| p.preparing)
            );
            let (remaining, placement) = app
                .session
                .start
                .pending_entries()
                .find(|(id, _)| **id != first)
                .map(|(id, pending)| (id.clone(), pending.tab_placement.unwrap()))
                .unwrap();
            app.close_pending_session_start(first, cx);
            (remaining, placement)
        });
        drop(worker_releases);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if app.update(&mut cx, |app, _| !app.session.start.has_pending()) {
                break;
            }
            assert!(Instant::now() < deadline, "route failure was not delivered");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.update(&mut cx, |app, _| {
            let failed = app.session.start.failed(&remaining).unwrap();
            assert!(failed.error.contains("proxy"));
            let failed_placement = failed.pending.tab_placement.unwrap();
            assert_eq!(
                failed_placement.request_sequence,
                placement.request_sequence
            );
            assert_eq!(failed_placement.insert_index, placement.insert_index);
            assert_eq!(app.session_strip_tabs(&None).len(), 2);
            assert_eq!(app.session.start.failed_entries().count(), 1);
            assert!(!app.remote_desktop.has_sessions());
            assert!(app.remote_desktop.routes.is_empty());
        });
    }
}
