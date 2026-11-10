use super::{SERVICES_SCRIPT, file_logs_command, parse_unix_output};
use crate::host::{Elevation, HostTarget, ensure_host_command_success};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;
use std::time::Duration;

fn executable(directory: &std::path::Path, name: &str, body: &str) {
    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn launchd_listing_keeps_the_invoking_users_domain_under_sudo() {
    let directory = tempfile::tempdir().unwrap();
    // No systemctl/service in this child-local PATH, irrespective of the host.
    symlink("/bin/sh", directory.path().join("sh")).unwrap();
    let awk = Command::new("sh")
        .args(["-c", "command -v awk"])
        .output()
        .unwrap();
    symlink(
        String::from_utf8(awk.stdout).unwrap().trim(),
        directory.path().join("awk"),
    )
    .unwrap();
    executable(directory.path(), "uname", "printf 'Darwin\\n'");
    executable(
        directory.path(),
        "id",
        "printf '%s\\n' \"$NYATERM_TEST_EFFECTIVE_UID\"",
    );
    executable(
        directory.path(),
        "launchctl",
        r#"
if [ "$1" = print ]; then
  printf '\tservices = {\n\t 123 0 same-label\n\t}\n'
fi
"#,
    );
    for (effective, sudo, expected) in [
        ("501", Some("900"), "gui/501"),
        ("0", Some("501"), "gui/501"),
        ("0", None, "gui/0"),
        ("0", Some(""), "gui/0"),
        ("0", Some("501/evil"), "gui/0"),
    ] {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", SERVICES_SCRIPT])
            .env("PATH", directory.path())
            .env("NYATERM_TEST_EFFECTIVE_UID", effective)
            .env_remove("SUDO_UID");
        if let Some(sudo) = sudo {
            command.env("SUDO_UID", sudo);
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let snapshot = parse_unix_output(&String::from_utf8(output.stdout).unwrap()).unwrap();
        assert_eq!(snapshot.entries.len(), 2);
        assert!(snapshot.entries.iter().any(|entry| entry.scope == expected));
        assert!(snapshot.entries.iter().any(|entry| entry.scope == "system"));
        assert_ne!(snapshot.entries[0].key(), snapshot.entries[1].key());
    }
}

fn file_logs(name: &str, lines: u32, paths: &[&str]) -> anyhow::Result<String> {
    let output = HostTarget::Local.run_sh(
        &file_logs_command(name, lines, paths),
        Elevation::None,
        Duration::from_secs(5),
    )?;
    Ok(ensure_host_command_success(output, "service logs unavailable")?.stdout)
}

#[test]
fn file_logs_distinguish_empty_matches_missing_files_and_read_errors() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let log = directory.path().join("log");
    std::fs::write(
        &log,
        "one fIxTuRe[1]\nother fixture1\ntwo fixture[1]\nthree FIXTURE[1]\n",
    )
    .unwrap();
    let paths = [missing.to_str().unwrap(), log.to_str().unwrap()];
    assert_eq!(
        file_logs("fixture[1]", 2, &paths).unwrap(),
        "two fixture[1]\nthree FIXTURE[1]\n"
    );
    assert!(file_logs("unmatched", 2, &paths).unwrap().is_empty());
    let permissions = std::fs::metadata(&log).unwrap().permissions();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root bypasses mode bits. Run this same test binary as an ordinary user
    // when validating the permission branch on a root-owned build machine.
    if unsafe { libc::geteuid() } != 0 {
        let error = file_logs("fixture", 2, &paths).unwrap_err().to_string();
        assert!(error.contains("not readable"));
        assert!(crate::host::looks_like_permission_error(&error));
    }
    std::fs::set_permissions(&log, permissions).unwrap();
    assert!(
        file_logs("fixture", 2, &[paths[0]])
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    // A directory is deterministically unreadable as a log, even under root.
    assert!(
        file_logs("fixture", 2, &[directory.path().to_str().unwrap()])
            .unwrap_err()
            .to_string()
            .contains("not a regular file")
    );

    executable(
        directory.path(),
        "awk",
        "printf 'fixture read failure\\n' >&2; exit 2",
    );
    let output = Command::new("/bin/sh")
        .args(["-c", &file_logs_command("fixture", 2, &[paths[1]])])
        .env("PATH", directory.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "reader failure was masked");
}
