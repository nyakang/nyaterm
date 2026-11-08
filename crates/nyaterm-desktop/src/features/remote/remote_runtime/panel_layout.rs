//! Synchronize remote monitoring demand after active-session changes.

use crate::features::NyaTermApp;
use gpui::Context;

impl NyaTermApp {
    /// Re-sync the Remote panels' GPUI-facing state after the active session changed.
    ///
    /// Called from `activate_session_id`, immediately after
    /// `reset_remote_runtime_for_session_switch`, so the reset and everything that has to
    /// follow it land in one outer GPUI update transaction rather than waiting for a
    /// paint.
    ///
    /// **Deliberately narrow.** This is for Remote-panel state that must follow the
    /// active session, not a general activation side-effect bucket. Today that is refresh
    /// demand: a panel's clock is armed on `(visible || the header wants it || icon detection is pending) && a supported SSH session`, and the last term just changed --
    /// switching to a non-SSH session has to drop the clocks. The snapshot flush joins
    /// this helper, which keeps every activation caller from switching sessions while
    /// omitting either.
    pub(in crate::features) fn sync_remote_panels_after_activation(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.sync_remote_panel_demand(cx);
        self.defer_remote_panel_snapshot_flush(cx);
    }
}

#[cfg(test)]
mod tests {
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::pages::remote::RemoteMonitorKind;
    use crate::models::{NavItem, RemoteProcessSortKey};
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, uuid};
    use nyaterm_remote_ops::remote_process::RemoteProcess;

    fn app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        let root = std::env::temp_dir().join(format!(
            "nyaterm-panel-layout-{}-{}",
            std::process::id(),
            uuid()
        ));
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
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    fn register_session(app: &mut NyaTermApp, session_id: &str, ssh: bool) {
        register_session_with_state(app, session_id, ssh, false);
    }

    fn register_session_with_state(
        app: &mut NyaTermApp,
        session_id: &str,
        ssh: bool,
        disconnected: bool,
    ) {
        app.session.register_session_metadata(
            session_id,
            crate::models::SessionRuntimeMetadata {
                ssh_config: ssh.then(nyaterm_transport::SshSessionConfig::default),
                ssh_multiplex_key: None,
                source_connection_id: None,
                ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                launch_config: crate::models::SessionLaunchConfig::Local(
                    nyaterm_transport::LocalSessionConfig::default(),
                ),
                disconnected,
            },
        );
    }

    fn process(pid: u32, cpu_percent: f64, memory_percent: f64) -> RemoteProcess {
        RemoteProcess {
            pid,
            ppid: 1,
            user: "user".to_string(),
            state: "S".to_string(),
            cpu_percent,
            memory_percent,
            rss_kb: 3,
            vsz_kb: 4,
            elapsed: "00:01".to_string(),
            command: "sleep".to_string(),
            command_line: "sleep 10".to_string(),
        }
    }

    #[test]
    fn narrowing_the_right_panel_preserves_sort_and_selection() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.remote_ops
                .apply_processes(vec![process(1, 9., 1.), process(2, 1., 9.)]);
            app.remote_ops
                .toggle_process_sort(RemoteProcessSortKey::Memory);
            app.remote_ops.toggle_process_selection(2);
            for width in [720., 600., 430., 320., 280., 200.] {
                app.shell.set_right_panel_width_for_test(width);
                app.flush_remote_panel_snapshots(cx);
                let state = app.remote_ops.process_presentation();
                assert_eq!(state.sort_key, RemoteProcessSortKey::Memory);
                assert_eq!(state.selected_pid, Some(2));
                assert_eq!(app.remote_ops.derived_processes()[0].pid, 2);
            }
        });
    }

    /// A session switch must reconcile Remote-panel refresh demand inside the same
    /// update, with no paint.
    ///
    /// Demand is `(visible || the header wants it) && enabled && a session with an SSH
    /// config`. Before `activate_session_id` took `cx`, only `render` reconciled it, so a
    /// switch to a non-SSH session left the panel clock running until something painted.
    ///
    /// **There is no `window.draw` in this test, and that is the assertion.** Every check
    /// runs inside one `update_entity`, which is the same transaction the real switch
    /// happens in.
    #[test]
    fn a_session_switch_reconciles_remote_panel_demand_without_a_paint() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(NavItem::GpuMonitor, cx);

            register_session(app, "ssh-a", true);
            register_session(app, "local-b", false);

            let gpu_polling = |app: &NyaTermApp, cx: &gpui::App| {
                app.remote_panels
                    .entity(RemoteMonitorKind::Gpu)
                    .read(cx)
                    .is_polling()
            };

            assert!(
                !gpu_polling(app, cx),
                "no session is active yet, so nothing polls"
            );

            app.activate_session_id("ssh-a", cx);
            assert!(
                gpu_polling(app, cx),
                "activating an SSH session with the GPU panel open must arm its clock \
                 here, not at the next paint"
            );

            // A local shell is a host too: the panels inspect this machine.
            app.activate_session_id("local-b", cx);
            assert!(
                gpu_polling(app, cx),
                "a live local shell keeps the host panels polling"
            );

            register_session_with_state(app, "dead-c", false, true);
            app.activate_session_id("dead-c", cx);
            assert!(
                !gpu_polling(app, cx),
                "switching to a disconnected session must drop the clock in the \
                 same update"
            );
        });
    }

    /// The whole Ports path against the real machine: a live local shell resolves to
    /// "this machine", the panel arms its clock, the job runs on the scheduler, and
    /// the listeners come back. Leaving for a dead session drops the clock again.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_local_shell_makes_the_ports_panel_list_real_listeners() {
        use crate::features::pages::remote::HostToolKind;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            register_session(app, "local-a", false);
            app.open_or_toggle_panel(NavItem::Ports, cx);
            app.activate_session_id("local-a", cx);
            let panel = app.host_tool_panels.entity(HostToolKind::Ports).clone();
            assert!(
                panel.read(cx).host_ready(),
                "a local shell is a usable host"
            );
            assert!(panel.read(cx).is_polling(), "an open panel polls");
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let loaded = loop {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(60));
            cx.run_until_parked();
            let loaded = cx.update_entity(&app, |app, cx| {
                app.host_tool_panels
                    .entity(HostToolKind::Ports)
                    .read(cx)
                    .loaded_port_count()
            });
            if loaded > 0 || std::time::Instant::now() > deadline {
                break loaded;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        assert!(
            loaded > 0,
            "the local machine always has a listening socket"
        );
        cx.update_entity(&app, |app, cx| {
            register_session_with_state(app, "dead", false, true);
            // A disconnected local shell still resolves to this machine, so the
            // panel keeps its host; closing the panel is what drops the clock.
            app.open_or_toggle_panel(NavItem::Ports, cx);
            app.activate_session_id("dead", cx);
            assert!(
                !app.host_tool_panels
                    .entity(HostToolKind::Ports)
                    .read(cx)
                    .is_polling(),
                "a hidden panel must not poll"
            );
        });
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_local_shell_makes_the_services_panel_list_real_services() {
        use crate::features::pages::remote::HostToolKind;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            register_session(app, "local-a", false);
            app.open_or_toggle_panel(NavItem::Services, cx);
            app.activate_session_id("local-a", cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let loaded = loop {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(60));
            cx.run_until_parked();
            let loaded = cx.update_entity(&app, |app, cx| {
                app.host_tool_panels
                    .entity(HostToolKind::Services)
                    .read(cx)
                    .loaded_service_count()
            });
            if loaded > 0 || std::time::Instant::now() > deadline {
                break loaded;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        assert!(loaded > 0, "this machine runs at least one service");
    }

    /// Regression: the periodic refresh gates only let SSH sessions through, so a
    /// local shell showed "Loading..." in Resources forever.
    #[test]
    fn a_local_shell_is_refreshed_by_the_periodic_remote_probes() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            register_session(app, "local-a", false);
            app.activate_session_id("local-a", cx);
            assert!(
                app.refresh_stats_if_due(cx),
                "stats must start for a local shell"
            );
            assert!(
                app.refresh_processes_if_due(cx),
                "processes must start for a local shell"
            );
            register_session_with_state(app, "dead", false, true);
            app.activate_session_id("dead", cx);
            assert!(
                !app.refresh_docker_if_due(cx),
                "a disconnected shell must not be probed"
            );
        });
    }

    /// Like Resources and Processes, Ports and Services need an open terminal:
    /// with none (or a dead local one) there is no host to inspect.
    #[test]
    fn ports_and_services_need_a_live_terminal() {
        use crate::features::pages::remote::HostToolKind;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(NavItem::Ports, cx);
            app.sync_remote_panel_demand(cx);
            for kind in [HostToolKind::Ports, HostToolKind::Services] {
                let panel = app.host_tool_panels.entity(kind).clone();
                assert!(
                    !panel.read(cx).host_ready(),
                    "{kind:?}: no terminal, no host"
                );
                assert!(!panel.read(cx).is_polling());
            }
            register_session(app, "local-a", false);
            app.activate_session_id("local-a", cx);
            assert!(
                app.host_tool_panels
                    .entity(HostToolKind::Ports)
                    .read(cx)
                    .host_ready()
            );
            register_session_with_state(app, "dead", false, true);
            app.activate_session_id("dead", cx);
            assert!(
                !app.host_tool_panels
                    .entity(HostToolKind::Ports)
                    .read(cx)
                    .host_ready(),
                "a disconnected local terminal is not a host"
            );
        });
    }

    /// Regression: the top-bar status line kept saying "open a local terminal to
    /// view resources" while a local terminal was open.
    #[test]
    fn the_header_status_line_sees_a_live_local_terminal_as_a_host() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            assert!(!app.header_status_has_host(), "no terminal, no host");
            register_session(app, "local-a", false);
            app.activate_session_id("local-a", cx);
            assert!(app.header_status_has_host());
            register_session_with_state(app, "dead", false, true);
            app.activate_session_id("dead", cx);
            assert!(
                !app.header_status_has_host(),
                "a disconnected shell is not a host"
            );
        });
    }
}
