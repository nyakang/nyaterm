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
                disconnected: false,
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

            app.activate_session_id("local-b", cx);
            assert!(
                !gpu_polling(app, cx),
                "switching to a session with no SSH config must drop the clock in the \
                 same update"
            );
        });
    }
}
