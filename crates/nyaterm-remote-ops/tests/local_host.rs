#![cfg(unix)]
//! The same backends that serve SSH hosts must work against the local machine.

use nyaterm_remote_ops::host::HostTarget;
use nyaterm_remote_ops::remote_process::SshProcessService;
use nyaterm_remote_ops::stats::{RemoteStatsSampler, RemoteStatsService};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn live_child() -> OwnedChild {
    let mut child = OwnedChild(
        Command::new("sh")
            .args(["-c", "trap '' HUP; printf 'ready\\n'; exec sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut ready = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    child
}

#[test]
fn local_process_signals_work_with_the_system_posix_shell() {
    let service = SshProcessService::with_target(HostTarget::Local);
    let mut child = live_child();
    let pid = child.0.id();
    service.signal_process(pid, "HUP").unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    service.signal_process(pid, "STOP").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut status = 0;
        // Only observe this test's own child's stopped status; never reap it here.
        let observed =
            unsafe { libc::waitpid(pid as i32, &mut status, libc::WUNTRACED | libc::WNOHANG) };
        if observed == pid as i32 {
            assert!(libc::WIFSTOPPED(status));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    service.signal_process(pid, "CONT").unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    service.signal_process(pid, "TERM").unwrap();
    assert!(!child.0.wait().unwrap().success());
    let mut child = live_child();
    service.signal_process(child.0.id(), " sigkill ").unwrap();
    assert!(!child.0.wait().unwrap().success());
    assert!(service.signal_process(0, "CONT").is_err());
}

#[test]
fn ports_termination_uses_the_same_local_shell_safely() {
    use nyaterm_remote_ops::host::{Elevation, HostOs, signal_pid};
    for force in [false, true] {
        let mut child = live_child();
        signal_pid(
            &HostTarget::Local,
            HostOs::local(),
            child.0.id(),
            force,
            Elevation::None,
        )
        .unwrap();
        assert!(!child.0.wait().unwrap().success());
    }
    assert!(
        signal_pid(
            &HostTarget::Local,
            HostOs::local(),
            0,
            false,
            Elevation::None
        )
        .is_err()
    );
}

#[test]
fn local_process_list_contains_this_test_process() {
    let service = SshProcessService::with_target(HostTarget::Local);
    let processes = service.list_processes().expect("local ps");
    let me = std::process::id();
    let found = processes
        .iter()
        .find(|p| p.pid == me)
        .expect("own pid listed");
    assert!(!found.command.is_empty());
    assert!(processes.len() > 5);
}

#[test]
fn local_stats_report_cpu_and_memory() {
    let service = RemoteStatsService::with_target(HostTarget::Local);
    let sampler = RemoteStatsSampler::default();
    let stats = service.snapshot(&sampler, "local").expect("local stats");
    println!("{stats:#?}");
}
