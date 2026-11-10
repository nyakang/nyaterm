//! Controlled subprocesses keep Windows coverage independent of installed services.
use super::{Supervisor, platform, run_local};
use crate::host::{Elevation, HostCommandError, HostOs, HostTarget, signal_pid};
use crate::process_control::UnsupportedProcessControl;
use crate::remote_process::SshProcessService;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_TEST: &str = "host::local_exec::windows_tests::controlled_child";
fn command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env("NYATERM_HOST_EXEC_TEST_MODE", mode);
    command
}

#[test]
fn controlled_child() {
    let Ok(mode) = std::env::var("NYATERM_HOST_EXEC_TEST_MODE") else {
        return;
    };
    match mode.as_str() {
        "background-success" | "background-nonzero" => {
            // Windows inherits every inheritable handle, including the
            // harness's original pipes even with redirected child stdio.
            // Model a daemon that has relinquished those original handles.
            for handle in [
                std::io::stdout().as_raw_handle(),
                std::io::stderr().as_raw_handle(),
            ] {
                assert_ne!(
                    unsafe {
                        windows_sys::Win32::Foundation::SetHandleInformation(
                            handle,
                            windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
                            0,
                        )
                    },
                    0
                );
            }
            command("detached-marker")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            std::process::exit(if mode == "background-success" { 0 } else { 7 });
        }
        "detached-marker" => {
            let release = std::path::PathBuf::from(
                std::env::var_os("NYATERM_HOST_EXEC_TEST_RELEASE").unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            while !release.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(release.exists());
            std::fs::write(
                std::env::var_os("NYATERM_HOST_EXEC_TEST_MARKER").unwrap(),
                b"completed",
            )
            .unwrap();
        }
        "parent-exits" | "parent-waits" | "panic-waits" => {
            let mut descendant = command("marker");
            // Both parent and descendant retain the runner's pipe handles.
            descendant.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            let child = descendant.spawn().unwrap();
            if mode == "panic-waits" {
                std::fs::write(
                    std::env::var_os("NYATERM_HOST_EXEC_TEST_READY").unwrap(),
                    b"ready",
                )
                .unwrap();
            }
            if mode == "parent-exits" {
                std::process::exit(0);
            }
            let _child = child;
            std::thread::sleep(Duration::from_secs(30));
        }
        "marker" => {
            std::thread::sleep(Duration::from_millis(1200));
            std::fs::write(
                std::env::var_os("NYATERM_HOST_EXEC_TEST_MARKER").unwrap(),
                b"survived",
            )
            .unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
        "echo" => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stderr().write_all(&bytes).unwrap();
            std::process::exit(7);
        }
        "flood" | "flood-stderr" => {
            let bytes = vec![b'x'; 65536];
            for _ in 0..512 {
                if mode == "flood" {
                    std::io::stdout().write_all(&bytes).unwrap();
                } else {
                    std::io::stderr().write_all(&bytes).unwrap();
                }
            }
        }
        "holds-stdin" => std::thread::sleep(Duration::from_secs(30)),
        "success" => {
            print!("recovered");
            std::process::exit(0);
        }
        _ => panic!("unknown controlled mode"),
    }
}

#[test]
fn windows_panic_reaps_the_job_before_joining_a_blocked_writer() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("escaped");
    let ready = directory.path().join("ready");
    let mut controlled = command("panic-waits");
    controlled
        .env("NYATERM_HOST_EXEC_TEST_MARKER", &marker)
        .env("NYATERM_HOST_EXEC_TEST_READY", &ready)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut owner = Supervisor {
        process: platform::Process::spawn(&mut controlled).unwrap(),
        input: None,
    };
    owner.input = Some(
        platform::Input::new(
            owner.process.child.stdin.take(),
            zeroize::Zeroizing::new(vec![b'x'; 4_000_000]),
        )
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(ready.exists());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owner = owner;
        panic!("controlled supervisor panic");
    }));
    assert!(result.is_err());
    std::thread::sleep(Duration::from_millis(1400));
    assert!(!marker.exists());
    assert!(run_local(&mut command("success"), None, Duration::from_secs(5)).is_ok());
}

#[test]
fn windows_completed_job_preserves_background_work_with_redirected_pipes() {
    for (mode, code) in [("background-success", 0), ("background-nonzero", 7)] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("completed");
        let release = directory.path().join("release");
        let mut command = command(mode);
        command
            .env("NYATERM_HOST_EXEC_TEST_MARKER", &marker)
            .env("NYATERM_HOST_EXEC_TEST_RELEASE", &release);
        let output = run_local(&mut command, None, Duration::from_secs(5)).unwrap();
        assert_eq!(output.exit_status, Some(code));
        std::fs::write(release, b"released").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "normal completion killed background work");
    }
}

#[test]
fn windows_job_deadline_covers_inherited_pipes_and_reaps_parent_and_descendants() {
    for mode in ["parent-exits", "parent-waits", "holds-stdin"] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("survived");
        let mut command = command(mode);
        command.env("NYATERM_HOST_EXEC_TEST_MARKER", &marker);
        let started = Instant::now();
        let error = run_local(
            &mut command,
            Some("x".repeat(4_000_000)),
            Duration::from_millis(400),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<HostCommandError>(),
            Some(HostCommandError::Timeout(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
        std::thread::sleep(Duration::from_millis(1400));
        assert!(!marker.exists(), "descendant survived job cleanup");
        assert!(
            run_local(&mut self::command("success"), None, Duration::from_secs(5))
                .unwrap()
                .stdout
                .ends_with("recovered")
        );
    }
}

#[test]
fn windows_large_stdin_dual_output_and_nonzero_exit_are_preserved() {
    let data = "x".repeat(2_000_000);
    let result = run_local(
        &mut command("echo"),
        Some(data.clone()),
        Duration::from_secs(10),
    )
    .unwrap();
    // --nocapture's harness prelude precedes the controlled stdout payload.
    assert!(result.stdout.ends_with(&data));
    assert_eq!(result.stderr, data);
    assert_eq!(result.exit_status, Some(7));
}

#[test]
fn windows_output_limit_is_an_error_for_each_stream_and_following_work_recovers() {
    for mode in ["flood", "flood-stderr"] {
        let error = run_local(&mut command(mode), None, Duration::from_secs(10)).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<HostCommandError>(),
            Some(HostCommandError::OutputLimitExceeded)
        ));
        assert!(run_local(&mut command("success"), None, Duration::from_secs(5)).is_ok());
    }
}

struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn unsupported_windows_signals_leave_a_live_child_intact_until_explicit_kill() {
    let mut command = command("holds-stdin");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = OwnedChild(command.spawn().unwrap());
    let pid = child.0.id();
    let service = SshProcessService::with_target(HostTarget::Local);
    for signal in ["TERM", "HUP", "STOP", "CONT", " sigterm "] {
        assert!(
            service
                .signal_process(pid, signal)
                .unwrap_err()
                .is::<UnsupportedProcessControl>()
        );
        assert!(child.0.try_wait().unwrap().is_none());
    }
    assert!(
        signal_pid(
            &HostTarget::Local,
            HostOs::Windows,
            pid,
            false,
            Elevation::None
        )
        .unwrap_err()
        .is::<UnsupportedProcessControl>()
    );
    assert!(
        signal_pid(
            &HostTarget::Local,
            HostOs::OtherUnix,
            pid,
            false,
            Elevation::None
        )
        .unwrap_err()
        .is::<UnsupportedProcessControl>()
    );
    assert!(
        crate::local_process::kill_local_process(pid, false)
            .unwrap_err()
            .is::<UnsupportedProcessControl>()
    );
    assert!(
        service
            .renice_process(pid, 0)
            .unwrap_err()
            .is::<UnsupportedProcessControl>()
    );
    assert!(child.0.try_wait().unwrap().is_none());
    service.signal_process(pid, " sigkill ").unwrap();
    assert!(!child.0.wait().unwrap().success());
}
