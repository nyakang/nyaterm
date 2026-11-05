//! Windows ConPTY regressions. Run explicitly on a host with WSL installed.
#![cfg(windows)]

use nyaterm_terminal::TerminalCore;
use nyaterm_transport::{LocalSessionConfig, SessionEvent, SessionManager};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires a working Windows ConPTY and CMD"]
fn external_local_directory_with_chinese_and_spaces_is_used_without_a_startup_command() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("中文 directory with spaces");
    std::fs::create_dir(&directory).unwrap();
    let manager = SessionManager::new();
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("cmd.exe".into()),
            shell_args: vec!["/D".into(), "/Q".into()],
            working_dir: Some(directory.clone()),
            cols: 240,
            ..Default::default()
        })
        .unwrap();
    let mut terminal = TerminalCore::new(240, 24);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut matched = false;
    while Instant::now() < deadline && !matched {
        for event in manager.drain_events(64).unwrap().events {
            if let SessionEvent::Output { data, .. } = event {
                terminal.advance(&data);
                for reply in terminal.take_effects().pty_write {
                    manager.write_raw(&session.id, &reply).unwrap();
                }
            }
        }
        matched = terminal
            .lines()
            .iter()
            .any(|line| line.contains(directory.to_str().unwrap()));
        std::thread::sleep(Duration::from_millis(20));
    }
    manager.close(&session.id).unwrap();
    assert!(
        matched,
        "CMD prompt must show the requested working directory"
    );
    assert!(manager.list_sessions().unwrap().is_empty());
}

#[test]
#[ignore = "requires a working Windows ConPTY and CMD"]
fn conpty_long_output_retains_soft_wraps_and_hard_newlines_for_logical_display() {
    let directory = tempfile::tempdir().unwrap();
    let long_line = format!("NYATERM_WRAP_BEGIN_{}_END", "0123456789".repeat(20));
    let hard_line = "NYATERM_HARD_BREAK";
    let manager = SessionManager::new();
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("cmd.exe".into()),
            shell_args: vec![
                "/D".into(),
                "/Q".into(),
                "/K".into(),
                format!("echo {long_line}&echo {hard_line}"),
            ],
            working_dir: Some(directory.path().into()),
            cols: 32,
            rows: 12,
            ..Default::default()
        })
        .unwrap();
    let mut terminal = TerminalCore::new(32, 12);
    terminal.set_logical_line_snapshots(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut matched = false;
    while Instant::now() < deadline && !matched {
        for event in manager.drain_events(64).unwrap().events {
            if let SessionEvent::Output { data, .. } = event {
                terminal.advance(&data);
                for reply in terminal.take_effects().pty_write {
                    manager.write_raw(&session.id, &reply).unwrap();
                }
            }
        }
        matched = terminal
            .all_text()
            .is_some_and(|text| text.contains(&format!("{long_line}\n{hard_line}")));
        if !matched {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    manager.close(&session.id).unwrap();
    assert!(
        matched,
        "ConPTY must retain the soft/hard break distinction"
    );
    let snapshot = terminal.snapshot();
    let index = snapshot.logical_lines.as_ref().expect("logical display");
    assert!(index.logical_rows() < index.physical_rows());
    assert!(snapshot.rows().iter().any(|row| row.wraps_to_next));
    assert_eq!((terminal.cols(), terminal.rows()), (32, 12));
    assert!(manager.list_sessions().unwrap().is_empty());
}

fn observe(args: &[&str], input: Option<&[u8]>) -> (bool, bool) {
    let manager = SessionManager::new();
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("wsl.exe".into()),
            shell_args: args.iter().map(|arg| (*arg).into()).collect(),
            working_dir: Some("/tmp".into()),
            ..Default::default()
        })
        .expect("WSL session");
    let mut terminal = TerminalCore::new(80, 24);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut cwd_seen = false;
    let mut marker_seen = false;
    let mut sent_input = false;
    while Instant::now() < deadline {
        for event in manager.drain_events(64).unwrap().events {
            match event {
                SessionEvent::Output { data, .. } => {
                    terminal.advance(&data);
                    for reply in terminal.take_effects().pty_write {
                        let _ = manager.write_raw(&session.id, &reply);
                    }
                }
                SessionEvent::CwdChanged { cwd, .. } => cwd_seen |= cwd == "/tmp",
                _ => {}
            }
        }
        marker_seen |= terminal
            .lines()
            .iter()
            .any(|line| line.contains("NYATERM_INPUT_OK"));
        // The child's read prompt is visible before any input is supplied.
        if !sent_input
            && terminal
                .lines()
                .iter()
                .any(|line| line.contains("NYATERM_WAITING"))
            && let Some(input) = input
        {
            std::thread::sleep(Duration::from_millis(700));
            manager.write_raw(&session.id, input).unwrap();
            sent_input = true;
        }
        if marker_seen || (input.is_none() && cwd_seen) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    manager.close(&session.id).unwrap();
    (cwd_seen, marker_seen)
}

#[test]
#[ignore = "requires WSL with Bash and Fish; does not read user rc files"]
fn explicit_interactive_shells_report_cwd_without_stdin_injection() {
    for args in [
        vec!["--exec", "bash", "--noprofile", "--norc", "-i"],
        vec!["--exec", "fish", "--no-config", "--interactive"],
    ] {
        assert!(observe(&args, None).0, "interactive shell must report cwd");
    }
}

#[test]
#[ignore = "requires WSL; does not read user rc files"]
fn noninteractive_commands_preserve_output_and_receive_only_user_input() {
    let script = "printf 'NYATERM_WAITING\\n'; IFS= read -r value; if [ \"$value\" = user-input ]; then printf 'NYATERM_INPUT_OK\\n'; fi; sleep 1";
    assert!(observe(&["--exec", "sh", "-c", script], Some(b"user-input\r")).1);
    assert!(
        observe(
            &[
                "--exec",
                "sh",
                "-c",
                "printf 'NYATERM_INPUT_OK\\n'; sleep 1"
            ],
            None
        )
        .1
    );
}

#[test]
#[ignore = "requires WSL with Python, Bash, Zsh and Fish; uses isolated temporary HOME"]
fn native_posix_startup_preserves_rc_prompts_hooks_and_cleanup() {
    let result = std::process::Command::new("wsl.exe")
        .args([
            "--exec",
            "python3",
            "-c",
            include_str!("local_shell_startup.py"),
        ])
        .arg(include_str!("../src/local_shell_integration/startup.sh").replace("\r\n", "\n"))
        .output()
        .expect("WSL Python test runner");
    assert!(
        result.status.success(),
        "isolated startup checks: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "requires WSL with Bash; exercises active ConPTY teardown"]
fn wsl_active_close_removes_host_files_and_terminates_foreground_processes() {
    let manager = std::sync::Arc::new(SessionManager::new());
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("wsl.exe".into()),
            shell_args: ["--exec", "bash", "--noprofile", "--norc", "-i"]
                .map(str::to_string)
                .to_vec(),
            working_dir: Some("/tmp".into()),
            ..Default::default()
        })
        .unwrap();
    let prefix = format!("nyaterm-shell-{}-", session.id);
    let startup_dir = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .expect("session owns a host startup directory")
        .path();
    let guest_dir = std::process::Command::new("wsl.exe")
        .args(["--exec", "wslpath", "-u"])
        .arg(&startup_dir)
        .output()
        .unwrap();
    assert!(guest_dir.status.success());
    let guest_dir = String::from_utf8(guest_dir.stdout).unwrap();
    let quoted_dir = format!("'{}'", guest_dir.trim().replace('\'', "'\\''"));
    let mut terminal = TerminalCore::new(80, 24);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut started = false;
    while Instant::now() < deadline
        && !std::fs::metadata(startup_dir.join("foreground.pid")).is_ok_and(|file| file.len() > 0)
    {
        for event in manager.drain_events(64).unwrap().events {
            match event {
                SessionEvent::Output { data, .. } => {
                    terminal.advance(&data);
                    for reply in terminal.take_effects().pty_write {
                        let _ = manager.write_raw(&session.id, &reply);
                    }
                }
                SessionEvent::CwdChanged { .. } if !started => {
                    let command = format!(
                        "printf '%s' \"$$\" > {quoted_dir}/shell.pid; ( printf '%s' \"$BASHPID\" > {quoted_dir}/foreground.pid; exec sleep 120 )\r"
                    );
                    manager.write_raw(&session.id, command.as_bytes()).unwrap();
                    started = true;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        startup_dir.join("foreground.pid").is_file(),
        "controlled foreground process must start"
    );
    let pids = ["shell.pid", "foreground.pid"]
        .map(|file| std::fs::read_to_string(startup_dir.join(file)).unwrap());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let closing_manager = std::sync::Arc::clone(&manager);
    let closer = std::thread::spawn(move || {
        tx.send(closing_manager.close(&session.id).is_ok()).unwrap();
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(5))
            .expect("WSL ConPTY close must finish promptly")
    );
    closer.join().unwrap();
    assert!(
        !startup_dir.exists(),
        "WSL startup files must be removed by the host"
    );
    assert!(
        !manager
            .drain_events(64)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::CleanupFailed { .. }))
    );
    for pid in pids {
        assert!(!pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let gone = std::process::Command::new("wsl.exe")
                .args([
                    "--exec",
                    "sh",
                    "-c",
                    "! kill -0 \"$1\" 2>/dev/null",
                    "nyaterm-lifecycle",
                    &pid,
                ])
                .status()
                .unwrap()
                .success();
            if gone {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "WSL shell or foreground process survived ConPTY close"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
