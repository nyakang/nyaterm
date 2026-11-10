#![cfg(unix)]
//! Child-local PATH fixtures exercise the public adapters without touching services.
use nyaterm_core::SecretString;
use nyaterm_remote_ops::host::{Elevation, HostOs, HostTarget};
use nyaterm_remote_ops::ports::process_details;
use nyaterm_remote_ops::services::{
    ServiceAction, ServiceBackend, ServiceEntry, ServiceStartup, ServiceState, fetch_logs,
    run_action,
};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;
use std::time::{Duration, Instant};

fn entry() -> ServiceEntry {
    ServiceEntry {
        name: "fixture-service".into(),
        description: String::new(),
        state: ServiceState::Stopped,
        detail: String::new(),
        startup: ServiceStartup::Unknown,
        pid: None,
        scope: String::new(),
    }
}
fn script(directory: &std::path::Path, name: &str, body: &str) {
    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
fn fixture(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    symlink("/bin/sh", directory.path().join("sh")).unwrap();
    let read = r#"
case "$NYATERM_READ_FIXTURE_MODE" in
  command-error) printf 'PRIVATE output\n'; printf 'tool: permission denied\nsecond line\n' >&2; exit 2;;
  empty) exit 0;;
esac
"#;
    script(
        directory.path(),
        "ps",
        &format!("{read}\nprintf 'fixture-user /fixture-command\\n'"),
    );
    script(
        directory.path(),
        "journalctl",
        &format!("{read}\nprintf 'fixture log\\n'"),
    );
    script(
        directory.path(),
        "sudo",
        r#"
case "$NYATERM_READ_FIXTURE_MODE" in
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
    script(
        directory.path(),
        "service",
        r#"
(attempt=0; while [ ! -e "$NYATERM_READ_FIXTURE_RELEASE" ]; do
  [ $attempt -lt 300 ] || exit 1
  attempt=$((attempt+1))
  /bin/sleep 0.01
done; printf completed > "$NYATERM_READ_FIXTURE_MARKER") </dev/null >/dev/null 2>&1 &
"#,
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_host_tools_fixture", "--nocapture"])
        .env("PATH", directory.path())
        .env("NYATERM_READ_FIXTURE_MODE", mode)
        .env(
            "NYATERM_READ_FIXTURE_RELEASE",
            directory.path().join("release"),
        )
        .env(
            "NYATERM_READ_FIXTURE_MARKER",
            directory.path().join("completed"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "native fixture was not run"
    );
}

#[test]
fn native_reads_propagate_authentication_and_command_failures() {
    for mode in ["required", "password", "command-error", "success", "empty"] {
        fixture(mode);
    }
}

#[test]
fn successful_sysv_start_preserves_the_background_service() {
    fixture("service-start");
}

#[test]
fn native_host_tools_fixture() {
    let Ok(mode) = std::env::var("NYATERM_READ_FIXTURE_MODE") else {
        return;
    };
    if mode == "service-start" {
        run_action(
            &HostTarget::Local,
            HostOs::Linux,
            ServiceBackend::SysV,
            &entry(),
            ServiceAction::Start,
            Elevation::None,
        )
        .unwrap();
        std::fs::write(
            std::env::var_os("NYATERM_READ_FIXTURE_RELEASE").unwrap(),
            b"released",
        )
        .unwrap();
        let marker =
            std::path::PathBuf::from(std::env::var_os("NYATERM_READ_FIXTURE_MARKER").unwrap());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            marker.exists(),
            "service start succeeded but its child was killed"
        );
        return;
    }
    let elevation = match mode.as_str() {
        "required" => Elevation::SudoPasswordless,
        "password" => Elevation::SudoPassword(SecretString::new("synthetic-secret")),
        _ => Elevation::None,
    };
    let details = process_details(&HostTarget::Local, HostOs::Linux, 42, elevation.clone());
    let logs = fetch_logs(
        &HostTarget::Local,
        HostOs::Linux,
        ServiceBackend::Systemd,
        &entry(),
        20,
        elevation,
    );
    let error = match mode.as_str() {
        "required" => Some("sudo: a password is required"),
        "password" => Some("sudo: incorrect password"),
        "command-error" => Some("tool: permission denied"),
        _ => None,
    };
    if let Some(expected) = error {
        assert_eq!(details.unwrap_err().to_string(), expected);
        assert_eq!(logs.unwrap_err().to_string(), expected);
    } else {
        let details = details.unwrap();
        if mode == "empty" {
            assert!(details.user.is_none());
            assert!(logs.unwrap().is_empty());
        } else {
            assert_eq!(details.user.as_deref(), Some("fixture-user"));
            assert_eq!(details.command.as_deref(), Some("/fixture-command"));
            assert_eq!(logs.unwrap(), "fixture log\n");
        }
    }
}
