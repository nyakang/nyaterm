use super::executor::{HostToolsExecutor, Output, Request};
use super::jobs::Lane;
use super::{
    ActionPresentation, HostContext, HostResolution, HostToolKind, HostToolsPanel, LoadState,
    PendingAction, RefreshOutput,
};
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_core::{AppRuntime, RuntimeMode, SecretString, uuid};
use nyaterm_remote_ops::host::{Elevation, HostOs, HostTarget};
use nyaterm_remote_ops::ports::{PortEntry, PortProtocol, PortsSnapshot, ProcessDetails};
use nyaterm_remote_ops::services::{
    ServiceAction, ServiceBackend, ServiceEntry, ServiceStartup, ServiceState,
};
use std::collections::VecDeque;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Gate {
    result: Mutex<Option<Result<Output, String>>>,
    ready: Condvar,
}
impl Gate {
    fn release(&self, result: Result<Output, String>) {
        *self.result.lock().unwrap() = Some(result);
        self.ready.notify_one();
    }
    fn wait(&self) -> Result<Output, String> {
        let (mut result, _) = self
            .ready
            .wait_timeout_while(
                self.result.lock().unwrap(),
                Duration::from_secs(5),
                |result| result.is_none(),
            )
            .unwrap();
        result
            .take()
            .unwrap_or_else(|| Err("controlled job was not released".into()))
    }
}
enum Behavior {
    Block(Arc<Gate>),
    Error(&'static str),
    Panic,
}
#[derive(Default)]
struct ControlledExecutor {
    calls: AtomicUsize,
    behaviors: Mutex<VecDeque<(Lane, Behavior)>>,
}
impl ControlledExecutor {
    fn push(&self, lane: Lane, behavior: Behavior) {
        self.behaviors.lock().unwrap().push_back((lane, behavior));
    }
}
impl HostToolsExecutor for ControlledExecutor {
    fn execute(&self, _: &HostContext, request: Request, _: Elevation) -> Result<Output, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let lane = match &request {
            Request::Refresh(_) => Lane::Refresh,
            Request::Action(_, _) => Lane::Action,
            Request::Logs(_, _) => Lane::Logs,
            Request::Details(_) => Lane::Details,
        };
        let mut behaviors = self.behaviors.lock().unwrap();
        let behavior = behaviors
            .iter()
            .position(|(expected, _)| *expected == lane)
            .map(|index| behaviors.remove(index).unwrap().1);
        drop(behaviors);
        match behavior {
            Some(Behavior::Block(gate)) => gate.wait(),
            Some(Behavior::Error(error)) => Err(error.into()),
            Some(Behavior::Panic) => panic!("controlled worker failure"),
            None => match request {
                Request::Refresh(HostToolKind::Ports) => Ok(ports(80)),
                Request::Refresh(HostToolKind::Services) => Ok(Output::Refresh(
                    RefreshOutput::Services(nyaterm_remote_ops::services::ServicesSnapshot {
                        backend: ServiceBackend::Systemd,
                        entries: vec![service("system")],
                        partial: false,
                    }),
                )),
                Request::Action(_, _) => Ok(Output::Action),
                Request::Logs(_, _) => Ok(Output::Logs("current host log".into())),
                Request::Details(_) => Ok(Output::Details(ProcessDetails::default())),
            },
        }
    }
}
fn ports(port: u16) -> Output {
    Output::Refresh(RefreshOutput::Ports(PortsSnapshot {
        entries: vec![PortEntry {
            protocol: PortProtocol::Tcp,
            address: "127.0.0.1".into(),
            port,
            pid: Some(42),
            process: Some("fixture".into()),
            user: None,
        }],
        owners_hidden: false,
    }))
}
fn service(scope: &str) -> ServiceEntry {
    ServiceEntry {
        name: "same-service".into(),
        description: String::new(),
        state: ServiceState::Running,
        detail: String::new(),
        startup: ServiceStartup::Enabled,
        pid: Some(42),
        scope: scope.into(),
    }
}
fn host(key: &str) -> HostResolution {
    HostResolution::Ready(HostContext {
        key: key.into(),
        target: HostTarget::Local,
        os: HostOs::Linux,
    })
}
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
            panel.executor = executor;
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
                panel.executor = executor.clone();
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
                panel.refresh(cx);
                panel.open_logs(service("system"), cx);
                panel.submit_job(Lane::Details, "fixture".into(), Request::Details(42), cx);
                panel.tick(cx);
                assert!(
                    !panel.jobs.pending(Lane::Refresh)
                        && !panel.jobs.pending(Lane::Logs)
                        && !panel.jobs.pending(Lane::Details)
                );
            });
        }
    });
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
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
            panel.update(cx, |panel, _| panel.executor = executor.clone());
            panel
        });
        cx.update_entity(&app, |app, cx| {
            app.shell.set_viewport_size_for_test((1400., 900.));
            register_host_session(app, "local-a", false);
            app.activate_session_id("local-a", cx);
            app.show_activity_entry(kind.nav_item().persistence_id().into(), cx);
            for multi in [false, true] {
                if app.shell.panel_multi_open() != multi {
                    app.toggle_panel_multi_open(cx);
                }
                app.open_panel(kind.nav_item(), cx);
                assert!(panel.read(cx).is_polling());
                app.open_panel(kind.nav_item(), cx);
                assert!(!panel.read(cx).is_polling());
            }
            app.open_panel(kind.nav_item(), cx);
            assert!(panel.read(cx).is_polling());
            app.toggle_right_inspector(cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(kind.nav_item(), cx);
            app.hide_activity_entry(kind.nav_item().persistence_id().into(), cx);
            assert!(!panel.read(cx).is_polling());
            app.show_activity_entry(kind.nav_item().persistence_id().into(), cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(kind.nav_item(), cx);
            app.move_activity_entry(
                kind.nav_item().persistence_id().into(),
                ActivityBarZone::LeftBottom,
                None,
                cx,
            );
            assert!(!panel.read(cx).is_polling());
            app.open_panel(kind.nav_item(), cx);
            assert!(panel.read(cx).is_polling());
            app.toggle_left_sidebar(cx);
            assert!(!panel.read(cx).is_polling());
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            app.open_panel(kind.nav_item(), cx);
            assert!(panel.read(cx).is_polling());
            app.close_floating_panel(PanelSide::Left, cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(kind.nav_item(), cx);
            app.set_panel_open_mode(PanelOpenMode::Docked, cx);
            assert!(!panel.read(cx).is_polling());
            app.open_panel(kind.nav_item(), cx);
            app.reset_activity_bar_layout(cx);
            assert!(!panel.read(cx).is_polling());
            assert!(app.shell.activity_bar_layout().is_hidden("portsManager"));
            assert!(app.shell.activity_bar_layout().is_hidden("servicesManager"));
        });
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn closing_a_host_tool_cancels_reads_and_does_not_submit_more_after_completion() {
    use crate::models::NavItem;
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let gate = Arc::new(Gate::default());
    executor.push(Lane::Refresh, Behavior::Block(gate.clone()));
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
    wait_for(&mut cx, &panel, |_| {
        executor.calls.load(Ordering::SeqCst) == 1
    });
    cx.update_entity(&app, |app, cx| {
        app.open_panel(NavItem::Ports, cx);
        assert!(!panel.read(cx).is_polling());
        panel.update(cx, |panel, cx| {
            panel.refresh(cx);
            panel.open_logs(service("system"), cx);
            assert!(
                panel
                    .cancellations
                    .values()
                    .all(|handle| handle.is_cancelled())
            );
        });
    });
    gate.release(Ok(ports(99)));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    cx.update_entity(&panel, |panel, _| assert!(panel.cancellations.is_empty()));
}

#[test]
fn closing_a_panel_cancels_queued_reads_before_the_executor_runs() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    let scheduler = crate::blocking_jobs::BlockingJobScheduler::with_limits(1, 3).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let held = scheduler
        .submit_task("held", move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        })
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cx.update_entity(&panel, |panel, cx| {
        panel.scheduler = scheduler.clone();
        panel.refresh(cx);
        panel.services_backend = Some(ServiceBackend::Systemd);
        panel.open_logs(service("system"), cx);
        panel.submit_job(Lane::Details, "fixture".into(), Request::Details(42), cx);
        assert_eq!(panel.cancellations.len(), 3);
        panel.set_demand(false, cx);
    });
    release_tx.send(()).unwrap();
    assert!(futures::executor::block_on(held).is_ok());
    wait_for(&mut cx, &panel, |panel| panel.cancellations.is_empty());
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    cx.update_entity(&panel, |panel, _| {
        for lane in [Lane::Refresh, Lane::Logs, Lane::Details] {
            assert!(!panel.jobs.pending(lane));
        }
    });
}

#[test]
fn switching_hosts_discards_password_draft_and_cached_authorization() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        panel
            .sudo_input
            .update(cx, |input, cx| input.set_content("synthetic-secret", cx));
        panel.sudo_password = Some(SecretString::new("synthetic-secret"));
        panel.pending_action = Some((
            panel.jobs.epoch,
            PendingAction::Kill {
                pid: 42,
                force: true,
            },
        ));
        let old = panel.jobs.epoch;
        panel.set_host("b".into(), host("b"), cx);
        assert!(panel.sudo_input.read(cx).value(cx).is_empty());
        assert!(panel.sudo_password.is_none() && panel.pending_action.is_none());
        panel.kill_pid_for_epoch(old, 42, true, cx);
        panel.service_action_for_epoch(old, service("system"), ServiceAction::Stop, cx);
        assert!(!panel.jobs.pending(Lane::Action));
    });
}

#[test]
fn submitting_cancelling_and_hiding_clear_the_secret_input_immediately() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        panel.sudo_prompt = Some("fixture".into());
        panel
            .sudo_input
            .update(cx, |input, cx| input.set_content("synthetic-secret", cx));
        panel.submit_sudo(cx);
        assert!(panel.sudo_input.read(cx).value(cx).is_empty());
        assert!(panel.sudo_password.is_some());
        panel.cancel_sudo(cx);
        assert!(panel.sudo_password.is_none());
        panel
            .sudo_input
            .update(cx, |input, cx| input.set_content("synthetic-secret", cx));
        panel.sudo_password = Some(SecretString::new("synthetic-secret"));
        panel.elevated = true;
        panel.toggle_elevation(cx);
        assert!(panel.sudo_password.is_none());
        assert!(panel.sudo_input.read(cx).value(cx).is_empty());
        panel
            .sudo_input
            .update(cx, |input, cx| input.set_content("synthetic-secret", cx));
        panel.sudo_password = Some(SecretString::new("synthetic-secret"));
        panel.set_demand(false, cx);
        assert!(panel.sudo_password.is_none());
        assert!(panel.sudo_input.read(cx).value(cx).is_empty());
    });
}

#[test]
fn slow_refresh_is_single_flight_and_manual_requests_are_coalesced() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let gate = Arc::new(Gate::default());
    executor.push(Lane::Refresh, Behavior::Block(gate.clone()));
    let (_app, panel) = setup(&mut cx, executor.clone());
    cx.update_entity(&panel, |panel, cx| {
        panel.load = LoadState::Ready;
        panel.refresh(cx);
        let ticket = panel.jobs.ticket(Lane::Refresh);
        panel.last_refresh = Some(Instant::now() - super::PORTS_INTERVAL);
        panel.tick(cx);
        for _ in 0..20 {
            panel.refresh(cx);
        }
        assert_eq!(panel.jobs.ticket(Lane::Refresh), ticket);
    });
    gate.release(Ok(ports(90)));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn refreshing_during_an_action_cannot_orphan_its_busy_state() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let gate = Arc::new(Gate::default());
    executor.push(Lane::Action, Behavior::Block(gate.clone()));
    let (_app, panel) = setup(&mut cx, executor);
    cx.update_entity(&panel, |panel, cx| {
        panel.kill_pid(42, true, cx);
        for _ in 0..20 {
            panel.refresh(cx);
        }
        panel.toggle_elevation(cx);
        assert!(!panel.elevated);
        assert!(panel.busy_label().is_some());
    });
    gate.release(Ok(Output::Action));
    wait_for(&mut cx, &panel, |panel| {
        !panel.jobs.pending(Lane::Action) && !panel.jobs.pending(Lane::Refresh)
    });
    cx.update_entity(&panel, |panel, _| {
        assert!(panel.busy_label().is_none());
        assert!(panel.can_refresh());
    });
}

#[test]
fn stale_logs_and_details_are_dropped_even_for_the_same_entry_on_another_host() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        let key = service("system").key();
        let old_logs = panel.jobs.start(Lane::Logs, key.clone());
        let old_details = panel.jobs.start(Lane::Details, "TCP|22|42".into());
        panel.set_host("b".into(), host("b"), cx);
        panel.logs = Some(super::LogsDrawer {
            title: "fixture".into(),
            lines: Vec::new(),
            entry: service("system"),
        });
        panel.port_details = Some(super::PortDetailsState {
            key: "TCP|22|42".into(),
            details: ProcessDetails::default(),
            error: None,
        });
        let new_logs = panel.jobs.start(Lane::Logs, key.clone());
        let new_details = panel.jobs.start(Lane::Details, "TCP|22|42".into());
        panel.apply_job(
            Lane::Logs,
            old_logs,
            &key,
            Ok(Output::Logs("wrong host".into())),
            cx,
        );
        panel.apply_job(
            Lane::Details,
            old_details,
            "TCP|22|42",
            Ok(Output::Details(ProcessDetails {
                user: Some("wrong host".into()),
                command: None,
            })),
            cx,
        );
        assert!(panel.logs.as_ref().unwrap().lines.is_empty());
        assert!(panel.port_details.as_ref().unwrap().details.user.is_none());
        assert_eq!(panel.jobs.ticket(Lane::Logs), Some(new_logs));
        assert_eq!(panel.jobs.ticket(Lane::Details), Some(new_details));
        panel.apply_job(
            Lane::Logs,
            new_logs,
            &key,
            Ok(Output::Logs("correct host".into())),
            cx,
        );
        panel.apply_job(
            Lane::Details,
            new_details,
            "TCP|22|42",
            Err("permission denied".into()),
            cx,
        );
        assert!(!panel.jobs.pending(Lane::Details));
        assert!(panel.port_details.as_ref().unwrap().error.is_some());
    });
}

#[test]
fn action_errors_and_panics_release_the_slot_and_allow_retry() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    for behavior in [Behavior::Error("fixture failure"), Behavior::Panic] {
        executor.push(Lane::Action, behavior);
        cx.update_entity(&panel, |panel, cx| panel.kill_pid(42, true, cx));
        wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Action));
        cx.update_entity(&panel, |panel, _| {
            assert!(panel.busy_label().is_none());
            assert!(panel.status.as_ref().unwrap().1);
        });
    }
}

#[test]
fn rejected_refresh_logs_details_and_actions_finish_loading_immediately() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        let scheduler = crate::blocking_jobs::BlockingJobScheduler::with_limits(1, 1).unwrap();
        scheduler.shutdown();
        panel.scheduler = scheduler;
        panel.refresh(cx);
        panel.services_backend = Some(ServiceBackend::Systemd);
        panel.open_logs(service("system"), cx);
        let row = super::state::derive_ports(
            &match ports(80) {
                Output::Refresh(RefreshOutput::Ports(snapshot)) => snapshot.entries,
                _ => unreachable!(),
            },
            "",
            super::state::PortsView::default(),
        )
        .remove(0);
        panel.load_port_details(&row, cx);
        panel.kill_pid(42, true, cx);
        for lane in [Lane::Refresh, Lane::Logs, Lane::Details, Lane::Action] {
            assert!(!panel.jobs.pending(lane));
        }
        assert!(matches!(panel.load, LoadState::Failed(_)));
        assert!(!panel.logs.as_ref().unwrap().lines.is_empty());
        assert!(panel.port_details.as_ref().unwrap().error.is_some());
        assert!(panel.busy_label().is_none());
    });
}

#[test]
fn same_named_services_in_different_domains_have_independent_selection() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        panel.kind = HostToolKind::Services;
        panel.services = vec![service("system"), service("gui/501")].into();
        panel.rederive();
        panel.select(service("gui/501").key(), cx);
        assert_eq!(panel.selected_service().unwrap().scope, "gui/501");
    });
}

#[test]
fn permission_failure_keeps_a_visible_prompt_and_binds_retry_to_host_epoch() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        panel.fail_refresh("sudo: a password is required".into(), cx);
        assert!(panel.sudo_prompt.is_some());
        let ticket = panel.jobs.start(Lane::Action, String::new());
        panel.action = Some(ActionPresentation {
            label: "fixture".into(),
            done: "fixture".into(),
            retry: PendingAction::Kill {
                pid: 42,
                force: true,
            },
        });
        panel.apply_job(
            Lane::Action,
            ticket,
            "",
            Err("operation not permitted".into()),
            cx,
        );
        assert_eq!(panel.pending_action.as_ref().unwrap().0, panel.jobs.epoch);
        panel.set_host("b".into(), host("b"), cx);
        assert!(panel.pending_action.is_none());
    });
}

#[test]
fn full_queue_rejection_cleans_every_lane_and_cancelled_reads_can_recover() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    let scheduler = crate::blocking_jobs::BlockingJobScheduler::with_limits(1, 1).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let held = scheduler
        .submit_task("held", move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        })
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let queued = scheduler.submit_task("queued", |_| {}).unwrap();
    cx.update_entity(&panel, |panel, cx| {
        panel.scheduler = scheduler.clone();
        panel.refresh(cx);
        panel.services_backend = Some(ServiceBackend::Systemd);
        panel.open_logs(service("system"), cx);
        panel.port_details = Some(super::PortDetailsState {
            key: "fixture".into(),
            details: ProcessDetails::default(),
            error: None,
        });
        panel.submit_job(Lane::Details, "fixture".into(), Request::Details(42), cx);
        panel.kill_pid(42, true, cx);
        for lane in [Lane::Refresh, Lane::Logs, Lane::Details, Lane::Action] {
            assert!(!panel.jobs.pending(lane));
        }
        assert!(panel.logs.as_ref().unwrap().lines[0].contains("queue is full"));
        assert!(panel.port_details.as_ref().unwrap().error.is_some());
        assert!(panel.action.is_none());
    });
    release_tx.send(()).unwrap();
    assert!(futures::executor::block_on(held).is_ok());
    assert!(futures::executor::block_on(queued).is_ok());
    let gate = Arc::new(Gate::default());
    executor.push(Lane::Refresh, Behavior::Block(gate.clone()));
    cx.update_entity(&panel, |panel, cx| panel.refresh(cx));
    wait_for(&mut cx, &panel, |_| {
        executor.calls.load(Ordering::SeqCst) == 1
    });
    // Hiding cancels result delivery even if the native read has already begun.
    cx.update_entity(&panel, |panel, cx| panel.set_demand(false, cx));
    gate.release(Ok(ports(99)));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    cx.update_entity(&panel, |panel, cx| {
        assert!(panel.ports.is_empty());
        panel.set_demand(true, cx);
        panel.refresh(cx);
    });
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    cx.update_entity(&panel, |panel, _| assert_eq!(panel.ports[0].port, 80));
}

#[test]
fn hidden_action_failure_cannot_restore_authorization_or_retry_state() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        let ticket = panel.jobs.start(Lane::Action, String::new());
        panel.action = Some(ActionPresentation {
            label: "fixture".into(),
            done: "fixture".into(),
            retry: PendingAction::Kill {
                pid: 42,
                force: true,
            },
        });
        panel.set_demand(false, cx);
        panel.apply_job(
            Lane::Action,
            ticket,
            "",
            Err("sudo: incorrect password".into()),
            cx,
        );
        assert!(panel.busy_label().is_none());
        assert!(!panel.elevated);
        assert!(panel.sudo_prompt.is_none());
        assert!(panel.pending_action.is_none());
        assert!(panel.sudo_password.is_none());
    });
}

#[test]
fn replacing_a_connection_with_the_same_host_signature_invalidates_its_actions() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        let epoch = panel.jobs.epoch;
        panel
            .sudo_input
            .update(cx, |input, cx| input.set_content("synthetic-secret", cx));
        panel.invalidate_connection(cx);
        assert_ne!(epoch, panel.jobs.epoch);
        assert_eq!(panel.signature(), "a");
        assert!(panel.sudo_input.read(cx).value(cx).is_empty());
        panel.kill_pid_for_epoch(epoch, 42, true, cx);
        assert!(!panel.jobs.pending(Lane::Action));
    });
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
                panel.update(cx, |panel, cx| panel.confirm_kill(42, true, window, cx));
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
            assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
            panel.read_with(cx, |panel, _| assert!(!panel.jobs.pending(Lane::Action)));
        } else {
            let deadline = Instant::now() + Duration::from_secs(3);
            while executor.calls.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
                cx.run_until_parked();
            }
        }
    }
}

#[test]
fn ports_menu_offers_only_forced_termination_on_windows_and_keeps_posix_stop() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        let entries = match ports(80) {
            Output::Refresh(RefreshOutput::Ports(snapshot)) => snapshot.entries,
            _ => unreachable!(),
        };
        let row =
            super::state::derive_ports(&entries, "", super::state::PortsView::default()).remove(0);
        panel.host = Some(HostResolution::Ready(HostContext {
            key: "fixture".into(),
            target: HostTarget::Local,
            os: HostOs::Windows,
        }));
        let windows = super::view::port_menu_items(panel, &row, cx);
        panel.host = Some(host("fixture"));
        let posix = super::view::port_menu_items(panel, &row, cx);
        // Three copy commands, a separator and confirmed KILL; POSIX adds TERM.
        assert_eq!(windows.len(), 5);
        assert_eq!(posix.len(), 6);
    });
}

#[test]
fn refresh_and_drawer_panics_finish_loading_and_preserve_existing_data() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    cx.update_entity(&panel, |panel, cx| panel.refresh(cx));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    executor.push(Lane::Refresh, Behavior::Panic);
    cx.update_entity(&panel, |panel, cx| panel.refresh(cx));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    cx.update_entity(&panel, |panel, _| {
        assert_eq!(panel.ports[0].port, 80);
        assert!(matches!(panel.load, LoadState::Ready));
        assert!(panel.status.as_ref().unwrap().1);
    });
    executor.push(Lane::Logs, Behavior::Panic);
    executor.push(Lane::Details, Behavior::Panic);
    cx.update_entity(&panel, |panel, cx| {
        panel.services_backend = Some(ServiceBackend::Systemd);
        panel.open_logs(service("system"), cx);
        panel.port_details = Some(super::PortDetailsState {
            key: "fixture".into(),
            details: ProcessDetails::default(),
            error: None,
        });
        panel.submit_job(Lane::Details, "fixture".into(), Request::Details(42), cx);
    });
    wait_for(&mut cx, &panel, |panel| {
        !panel.jobs.pending(Lane::Logs) && !panel.jobs.pending(Lane::Details)
    });
    cx.update_entity(&panel, |panel, _| {
        assert!(panel.logs.as_ref().unwrap().lines[0].contains("panicked"));
        assert!(panel.port_details.as_ref().unwrap().error.is_some());
        assert!(panel.cancellations.is_empty());
    });
}

#[test]
fn authorization_controls_cannot_clear_an_active_operation_but_hiding_does() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        panel.elevated = true;
        panel.sudo_password = Some(SecretString::new("synthetic-secret"));
        panel.jobs.start(Lane::Action, String::new());
        panel.cancel_sudo(cx);
        panel.toggle_elevation(cx);
        assert!(panel.elevated && panel.sudo_password.is_some());
        panel.set_demand(false, cx);
        assert!(!panel.elevated && panel.sudo_password.is_none());
        assert!(panel.jobs.pending(Lane::Action));
    });
}

#[test]
fn an_action_during_a_running_refresh_discards_old_data_then_runs_one_followup() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    cx.update_entity(&panel, |panel, cx| panel.refresh(cx));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    let gate = Arc::new(Gate::default());
    executor.push(Lane::Refresh, Behavior::Block(gate.clone()));
    cx.update_entity(&panel, |panel, cx| panel.refresh(cx));
    wait_for(&mut cx, &panel, |_| {
        executor.calls.load(Ordering::SeqCst) == 2
    });
    cx.update_entity(&panel, |panel, cx| {
        panel.kill_pid(42, true, cx);
        for _ in 0..20 {
            panel.refresh(cx);
        }
    });
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Action));
    cx.update_entity(&panel, |panel, _| {
        assert!(panel.jobs.pending(Lane::Refresh));
        assert!(panel.jobs.refresh_requested);
        assert_eq!(panel.ports[0].port, 80);
    });
    assert_eq!(executor.calls.load(Ordering::SeqCst), 3);
    gate.release(Ok(ports(99)));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Refresh));
    cx.update_entity(&panel, |panel, _| {
        assert_eq!(panel.ports[0].port, 80);
        assert!(panel.cancellations.is_empty());
    });
    assert_eq!(executor.calls.load(Ordering::SeqCst), 4);
}

#[cfg(unix)]
#[test]
fn native_adapter_reads_finish_loading_and_recover_authorization() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;
    let directory = tempfile::tempdir().unwrap();
    symlink("/bin/sh", directory.path().join("sh")).unwrap();
    let write_tool = |name: &str, body: &str| {
        let path = directory.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    let failure = r#"
case "$NYATERM_NATIVE_ADAPTER_FIXTURE" in
  command-error) printf 'PRIVATE command or log\n'; printf 'fixture: permission denied\nsecond line\n' >&2; exit 2;;
  empty) exit 0;;
esac
"#;
    write_tool(
        "ps",
        &format!("{failure}\nprintf 'fixture-user /fixture-command\\n'"),
    );
    write_tool("journalctl", &format!("{failure}\nprintf 'fixture log\\n'"));
    write_tool(
        "sudo",
        r#"
case "$NYATERM_NATIVE_ADAPTER_FIXTURE" in
  required) printf 'sudo: a password is required\nsecond line\n' >&2; exit 1;;
  password) printf 'sudo: incorrect password\nsecond line\n' >&2; exit 1;;
esac
while [ "$#" -gt 0 ]; do
  if [ "$1" = sh ]; then exec "$@"; fi
  shift
done
exit 2
"#,
    );
    for mode in ["required", "password", "command-error", "success", "empty"] {
        // Set PATH only on the child process; production adapters and real jobs
        // are exercised there without racing other GPUI or subprocess tests.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "features::pages::remote::host_tools::tests::native_adapter_child_fixture",
                "--nocapture",
            ])
            .env("PATH", directory.path())
            .env("NYATERM_NATIVE_ADAPTER_FIXTURE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "native adapter fixture was not run"
        );
    }
}

#[cfg(unix)]
#[test]
fn native_adapter_child_fixture() {
    let Ok(mode) = std::env::var("NYATERM_NATIVE_ADAPTER_FIXTURE") else {
        return;
    };
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        panel.executor = Arc::new(super::executor::NativeExecutor);
        panel.elevated = true;
        panel.sudo_password = Some(SecretString::new("synthetic-secret"));
        panel
            .sudo_input
            .update(cx, |input, cx| input.set_content("synthetic-draft", cx));
        panel.services_backend = Some(ServiceBackend::Systemd);
        panel.open_logs(service("system"), cx);
        panel.port_details = Some(super::PortDetailsState {
            key: "fixture".into(),
            details: ProcessDetails::default(),
            error: None,
        });
        panel.submit_job(Lane::Details, "fixture".into(), Request::Details(42), cx);
    });
    wait_for(&mut cx, &panel, |panel| {
        !panel.jobs.pending(Lane::Logs) && !panel.jobs.pending(Lane::Details)
    });
    cx.update_entity(&panel, |panel, cx| {
        let expected = match mode.as_str() {
            "required" => Some("sudo: a password is required"),
            "password" => Some("sudo: incorrect password"),
            "command-error" => Some("fixture: permission denied"),
            _ => None,
        };
        let details = panel.port_details.as_ref().unwrap();
        let logs = panel.logs.as_ref().unwrap();
        if let Some(expected) = expected {
            assert_eq!(details.error.as_deref(), Some(expected));
            assert_eq!(logs.lines[0].as_ref(), expected);
            assert!(panel.sudo_password.is_none());
            assert!(panel.sudo_input.read(cx).value(cx).is_empty());
            assert!(panel.sudo_prompt.is_some());
        } else {
            assert!(details.error.is_none());
            if mode == "empty" {
                assert!(details.details.user.is_none());
                assert_eq!(logs.lines[0], super::t!("hostTools.noLogs").to_string());
            } else {
                assert_eq!(details.details.user.as_deref(), Some("fixture-user"));
                assert_eq!(logs.lines[0].as_ref(), "fixture log");
            }
        }
        assert!(panel.cancellations.is_empty());
        assert!(panel.busy_label().is_none());
        assert!(panel.can_refresh());
    });
}
