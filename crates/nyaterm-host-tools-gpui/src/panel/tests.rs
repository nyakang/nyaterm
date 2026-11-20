use super::executor::{Output, Request};
use super::jobs::Lane;
use super::test_support::{Behavior, ControlledExecutor, Gate, host, ports, service};
use super::{
    ActionPresentation, HostContext, HostResolution, HostToolKind, HostToolsPanel, LoadState,
    PendingAction, RefreshOutput,
};
use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_core::SecretString;
use nyaterm_executor::blocking::BlockingJobScheduler;
use nyaterm_remote_ops::host::{HostOs, HostTarget};
use nyaterm_remote_ops::ports::ProcessDetails;
use nyaterm_remote_ops::services::{ServiceAction, ServiceBackend};
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};
struct TestAppCleanup(TestAppContext);
impl TestAppCleanup {
    fn new(cx: &TestAppContext) -> Self {
        Self(cx.clone())
    }
}
impl Drop for TestAppCleanup {
    fn drop(&mut self) {
        self.0.quit();
    }
}

fn setup(
    cx: &mut TestAppContext,
    executor: Arc<ControlledExecutor>,
) -> (BlockingJobScheduler, Entity<HostToolsPanel>) {
    cx.executor().allow_parking();
    let scheduler = BlockingJobScheduler::new();
    let panel = cx.new(|cx| {
        HostToolsPanel::new(
            HostToolKind::Ports,
            std::rc::Rc::new(|_, _, _| {}),
            scheduler.clone(),
            cx,
        )
    });
    panel.update(cx, |panel, cx| {
        panel.executor = executor;
        panel.set_host("a".into(), host("a"), cx);
        panel.set_demand(true, cx);
    });
    (scheduler, panel)
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

#[test]
fn closing_a_panel_cancels_queued_reads_before_the_executor_runs() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    let scheduler = BlockingJobScheduler::with_limits(1, 3).unwrap();
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
    let (_app, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
    cx.update_entity(&panel, |panel, cx| {
        let scheduler = BlockingJobScheduler::with_limits(1, 1).unwrap();
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_app, panel) = setup(&mut cx, executor.clone());
    let scheduler = BlockingJobScheduler::with_limits(1, 1).unwrap();
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
fn ports_menu_offers_only_forced_termination_on_windows_and_keeps_posix_stop() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
    let _cleanup = TestAppCleanup::new(&cx);
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
                "panel::tests::native_adapter_child_fixture",
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
    let _cleanup = TestAppCleanup::new(&cx);
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

#[test]
fn sort_select_events_update_each_panel_without_toggling_reselected_direction() {
    use super::state::{PortSortKey, ServiceSortKey, SortDirection};
    use nyaterm_ui::NyaSelectEvent;
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    for kind in [HostToolKind::Ports, HostToolKind::Services] {
        let (_scheduler, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
        panel.update(&mut cx, |panel, _| panel.kind = kind);
        let select = panel.read_with(&cx, |panel, _| panel.sort_select.clone());
        select.update(&mut cx, |_, cx| {
            cx.emit(NyaSelectEvent::Changed(Some("1".into())))
        });
        cx.run_until_parked();
        panel.update(&mut cx, |panel, _| match kind {
            HostToolKind::Ports => {
                assert_eq!(panel.ports_view.sort, PortSortKey::ALL[1]);
                assert_eq!(panel.ports_view.direction, SortDirection::Ascending);
                panel.ports_view.direction = SortDirection::Descending;
            }
            HostToolKind::Services => {
                assert_eq!(panel.services_view.sort, ServiceSortKey::ALL[1]);
                assert_eq!(panel.services_view.direction, SortDirection::Ascending);
                panel.services_view.direction = SortDirection::Descending;
            }
        });
        select.update(&mut cx, |_, cx| {
            cx.emit(NyaSelectEvent::Changed(Some("1".into())))
        });
        cx.run_until_parked();
        panel.read_with(&cx, |panel, _| match kind {
            HostToolKind::Ports => {
                assert_eq!(panel.ports_view.direction, SortDirection::Descending)
            }
            HostToolKind::Services => {
                assert_eq!(panel.services_view.direction, SortDirection::Descending)
            }
        });
    }
}

#[test]
fn confirmation_executes_once_only_after_approval() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let executor = Arc::new(ControlledExecutor::default());
    let (_scheduler, panel) = setup(&mut cx, executor.clone());
    let cancelled = panel.update(&mut cx, |panel, cx| {
        panel.confirmation_request(42, true, cx)
    });
    drop(cancelled);
    let request = panel.update(&mut cx, |panel, cx| {
        panel.confirmation_request(42, true, cx)
    });
    assert_eq!(executor.action_calls(), 0);
    cx.update(|cx| request.approve(cx));
    wait_for(&mut cx, &panel, |panel| !panel.jobs.pending(Lane::Action));
    assert_eq!(executor.action_calls(), 1);
    cx.update(|cx| request.approve(cx));
    cx.run_until_parked();
    assert_eq!(executor.action_calls(), 1);
}

#[test]
fn obsolete_confirmation_cannot_execute_after_host_change_hide_reopen_or_panel_drop() {
    for change in 0..4 {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let executor = Arc::new(ControlledExecutor::default());
        let (_scheduler, panel) = setup(&mut cx, executor.clone());
        let request = panel.update(&mut cx, |panel, cx| {
            panel.confirmation_request(42, true, cx)
        });
        panel.update(&mut cx, |panel, cx| match change {
            0 => panel.set_host("b".into(), host("b"), cx),
            1 => {
                panel.set_demand(false, cx);
                panel.set_demand(true, cx);
            }
            2 => panel.set_demand(false, cx),
            _ => {}
        });
        if change == 3 {
            drop(panel);
        }
        cx.update(|cx| request.approve(cx));
        cx.run_until_parked();
        assert_eq!(executor.calls(), 0);
    }
}

#[test]
fn panels_render_with_injected_appearance_without_an_application_entity() {
    use super::HostToolsAppearance;
    use gpui::{font, px, rgb};
    use nyaterm_ui::{apply_component_theme, nya_root, theme_palette};
    for kind in [HostToolKind::Ports, HostToolKind::Services] {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (_scheduler, panel) = setup(&mut cx, Arc::new(ControlledExecutor::default()));
        panel.update(&mut cx, |panel, cx| {
            panel.set_demand(false, cx);
            panel.kind = kind;
            panel.load = LoadState::Ready;
            if let Output::Refresh(RefreshOutput::Ports(snapshot)) = ports(80) {
                panel.ports = snapshot.entries.into();
            }
            panel.services = vec![service("system")].into();
            panel.services_backend = Some(ServiceBackend::Systemd);
            panel.rederive();
        });
        cx.update(|cx| {
            apply_component_theme(
                theme_palette("windterm-dark"),
                font("Consolas"),
                px(14.),
                cx,
            )
        });
        let (_, cx) = cx.add_window_view(|window, cx| nya_root(panel.clone(), window, cx));
        for theme in ["windterm-dark", "windterm-light"] {
            cx.update(|window, cx| {
                let palette = theme_palette(theme);
                apply_component_theme(palette, font("Consolas"), px(14.), cx);
                panel.update(cx, |panel, cx| {
                    panel.set_chrome(
                        HostToolsAppearance {
                            palette,
                            transparent_surface: rgb(palette.surface),
                            code_font_family: "Consolas",
                        },
                        cx,
                    )
                });
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
            let id = match kind {
                HostToolKind::Ports => "host-tools-ports-scroll",
                HostToolKind::Services => "host-tools-services-scroll",
            };
            let bounds = cx.debug_bounds(id).expect("panel list rendered");
            assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        }
    }
}
