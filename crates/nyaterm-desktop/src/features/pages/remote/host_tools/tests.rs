use super::{HostToolKind, HostToolsPanel};
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::remote::remote_runtime::host_tools::host_tool_nav_item;
use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_core::{AppRuntime, RuntimeMode, uuid};
use nyaterm_host_tools_gpui::test_support::{
    ControlledExecutor, all_cancelled, cancellation_count, has_pending_action, has_pending_reads,
    host, install_executor, probe_hidden_reads, request_kill,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
fn setup(
    cx: &mut TestAppContext,
    executor: Arc<ControlledExecutor>,
) -> (Entity<NyaTermApp>, Entity<HostToolsPanel>) {
    // The production scheduler uses real workers; permit their oneshot wakeups.
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("nyaterm-host-tools-regression-{}", uuid()));
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.clone(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    let panel = cx.update_entity(&app, |app, cx| {
        let panel = app.host_tool_panels.entity(HostToolKind::Ports).clone();
        panel.update(cx, |panel, cx| {
            install_executor(panel, executor);
            panel.set_host("a".into(), host("a"), cx);
            panel.set_demand(true, cx);
        });
        panel
    });
    (app, panel)
}
fn wait_for(
    cx: &mut TestAppContext,
    panel: &Entity<HostToolsPanel>,
    predicate: impl Fn(&HostToolsPanel) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if cx.update_entity(panel, |panel, _| predicate(panel)) {
            break;
        }
        assert!(Instant::now() < deadline, "controlled job did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn register_host_session(app: &mut NyaTermApp, id: &str, ssh: bool) {
    app.session.register_session_metadata(
        id,
        crate::models::SessionRuntimeMetadata {
            confirmed_ssh_runtime_mode: None,
            pending_ssh_startup_command: None,
            ssh_config: ssh.then(nyaterm_transport::SshSessionConfig::default),
            ssh_multiplex_key: None,
            source_connection_id: None,
            ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
            launch_config: crate::models::SessionLaunchConfig::Local(
                nyaterm_transport::LocalSessionConfig::default(),
            ),
            disconnected: false,
        },
    );
}

#[test]
fn unopened_host_tools_and_showing_their_entries_submit_no_reads() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (app, panel) = setup(&mut cx, executor.clone());
    cx.update_entity(&app, |app, cx| {
        for kind in [HostToolKind::Ports, HostToolKind::Services] {
            app.host_tool_panels.entity(kind).update(cx, |panel, cx| {
                install_executor(panel, executor.clone());
                panel.set_demand(false, cx);
            });
        }
        app.shell.set_viewport_size_for_test((1400., 900.));
        register_host_session(app, "local-a", false);
        register_host_session(app, "ssh-b", true);
        for id in ["local-a", "ssh-b", "local-a"] {
            app.activate_session_id(id, cx);
            app.sync_host_tools(cx);
        }
        for id in ["portsManager", "servicesManager"] {
            assert!(app.shell.activity_bar_layout().is_hidden(id));
            app.show_activity_entry(id.into(), cx);
        }
        assert!(!app.panel_is_rendered(crate::models::NavItem::Ports));
        assert!(!app.panel_is_rendered(crate::models::NavItem::Services));
        for kind in [HostToolKind::Ports, HostToolKind::Services] {
            app.host_tool_panels.entity(kind).update(cx, |panel, cx| {
                assert!(panel.host_ready());
                assert!(!panel.can_refresh() && !panel.is_polling());
                probe_hidden_reads(panel, cx);
                assert!(!has_pending_reads(panel));
            });
        }
    });
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    assert_eq!(executor.calls(), 0);
    cx.update_entity(&panel, |panel, _| assert!(!panel.is_polling()));
}

#[test]
fn host_tool_visibility_mutations_reconcile_clocks_without_a_paint() {
    use crate::models::{ActivityBarZone, PanelOpenMode, PanelSide};
    for kind in [HostToolKind::Ports, HostToolKind::Services] {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let executor = Arc::new(ControlledExecutor::default());
        let (app, _ports_panel) = setup(&mut cx, executor.clone());
        let panel = cx.update_entity(&app, |app, cx| {
            let panel = app.host_tool_panels.entity(kind).clone();
            panel.update(cx, |panel, _| install_executor(panel, executor.clone()));
            panel
        });
        cx.update_entity(&app, |app, cx| {
            app.shell.set_viewport_size_for_test((1400., 900.));
            register_host_session(app, "local-a", false);
            app.activate_session_id("local-a", cx);
            app.show_activity_entry(host_tool_nav_item(kind).persistence_id().into(), cx);
            for multi in [false, true] {
                if app.shell.panel_multi_open() != multi {
                    app.toggle_panel_multi_open(cx);
                }
                app.open_panel(host_tool_nav_item(kind), cx);
                assert!(panel.read(cx).is_polling());
                app.open_panel(host_tool_nav_item(kind), cx);
                assert!(!panel.read(cx).is_polling());
            }
            app.open_panel(host_tool_nav_item(kind), cx);
            assert!(panel.read(cx).is_polling());
            app.toggle_right_inspector(cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(host_tool_nav_item(kind), cx);
            app.hide_activity_entry(host_tool_nav_item(kind).persistence_id().into(), cx);
            assert!(!panel.read(cx).is_polling());
            app.show_activity_entry(host_tool_nav_item(kind).persistence_id().into(), cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(host_tool_nav_item(kind), cx);
            app.move_activity_entry(
                host_tool_nav_item(kind).persistence_id().into(),
                ActivityBarZone::LeftBottom,
                None,
                cx,
            );
            assert!(!panel.read(cx).is_polling());
            app.open_panel(host_tool_nav_item(kind), cx);
            assert!(panel.read(cx).is_polling());
            app.toggle_left_sidebar(cx);
            assert!(!panel.read(cx).is_polling());
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            app.open_panel(host_tool_nav_item(kind), cx);
            assert!(panel.read(cx).is_polling());
            app.close_floating_panel(PanelSide::Left, cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(host_tool_nav_item(kind), cx);
            app.set_panel_open_mode(PanelOpenMode::Docked, cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(host_tool_nav_item(kind), cx);
            app.reset_activity_bar_layout(cx);
            assert!(!panel.read(cx).is_polling());
            assert!(app.shell.activity_bar_layout().is_hidden("portsManager"));
            assert!(app.shell.activity_bar_layout().is_hidden("servicesManager"));
        });
        assert_eq!(executor.calls(), 0);
    }
}

#[test]
fn closing_a_host_tool_cancels_reads_and_does_not_submit_more_after_completion() {
    use crate::models::NavItem;
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let gate = executor.block_next_refresh();
    let (app, panel) = setup(&mut cx, executor.clone());
    cx.update_entity(&app, |app, cx| {
        app.shell.set_viewport_size_for_test((1400., 900.));
        register_host_session(app, "local-a", false);
        app.activate_session_id("local-a", cx);
        // An explicitly opened panel remains usable while its rail entry is hidden.
        app.open_panel(NavItem::Ports, cx);
        assert!(app.shell.activity_bar_layout().is_hidden("portsManager"));
        panel.update(cx, |panel, cx| panel.refresh(cx));
    });
    wait_for(&mut cx, &panel, |_| executor.calls() == 1);
    cx.update_entity(&app, |app, cx| {
        app.open_panel(NavItem::Ports, cx);
        assert!(!panel.read(cx).is_polling());
        panel.update(cx, |panel, cx| {
            probe_hidden_reads(panel, cx);
            assert!(all_cancelled(panel));
        });
    });
    gate.release_ports(99);
    wait_for(&mut cx, &panel, |panel| !has_pending_reads(panel));
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    assert_eq!(executor.calls(), 1);
    cx.update_entity(&panel, |panel, _| assert!(cancellation_count(panel) == 0));
}

#[test]
fn confirming_an_old_host_dialog_cannot_submit_ports_or_process_operations() {
    use gpui::{Context, IntoElement, Modifiers, Render, Window};
    use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

    struct DialogHost;
    impl Render for DialogHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    // Also confirm a current Ports dialog, so the test exercises a working
    // confirmation callback rather than only an inactive button.
    for (processes, switch_host) in [(false, true), (true, true), (false, false)] {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let executor = Arc::new(ControlledExecutor::default());
        let (app, panel) = setup(&mut cx, executor.clone());
        app.update(&mut cx, |app, cx| {
            app.sync_component_theme(cx);
            app.remote_ops.set_process_status("original host status");
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| DialogHost);
            nya_root(view, window, cx)
        });
        cx.update(|window, cx| {
            if processes {
                app.update(cx, |app, cx| {
                    app.request_process_signal(42, "KILL", window, cx);
                });
            } else {
                panel.update(cx, |panel, cx| request_kill(panel, 42, true, window, cx));
            }
            if switch_host {
                panel.update(cx, |panel, cx| panel.set_host("b".into(), host("b"), cx));
            }
            assert!(window.has_active_nya_dialog(cx));
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let confirm = cx.debug_bounds("nya-dialog-action-button").unwrap();
        cx.simulate_click(confirm.center(), Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert_eq!(
                app.read(cx).remote_ops.process_status(),
                "original host status"
            );
        });
        if switch_host {
            assert_eq!(executor.calls(), 0);
            panel.read_with(cx, |panel, _| assert!(!has_pending_action(panel)));
        } else {
            let deadline = Instant::now() + Duration::from_secs(3);
            while executor.calls() == 0 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
                cx.run_until_parked();
            }
        }
    }
}
