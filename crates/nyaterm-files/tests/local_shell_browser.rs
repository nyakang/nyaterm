#![cfg(windows)]
use nyaterm_terminal::TerminalCore;
use nyaterm_transport::{LocalSessionConfig, SessionEvent, SessionManager};
use std::time::{Duration, Instant};

#[test]
#[ignore = "set NYATERM_TEST_POSIX_SHELL to the Git Bash executable"]
fn git_bash_reports_cwd_through_conpty() {
    let shell = std::env::var("NYATERM_TEST_POSIX_SHELL").expect("Git Bash executable");
    let directory = tempfile::tempdir().unwrap();
    let manager = SessionManager::new();
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some(shell),
            shell_args: vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            working_dir: Some(directory.path().into()),
            ..Default::default()
        })
        .unwrap();
    let mut terminal = TerminalCore::new(80, 24);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut matched = false;
    let mut cwd_count = 0;
    let mut mapped_count = 0;
    let mut output_bytes = 0;
    let mut error_seen = false;
    while Instant::now() < deadline && !matched {
        for event in manager.drain_events(64).unwrap().events {
            match event {
                SessionEvent::Output { data, .. } => {
                    output_bytes += data.len();
                    error_seen |= String::from_utf8_lossy(&data).contains("not found");
                    terminal.advance(&data);
                    for reply in terminal.take_effects().pty_write {
                        let _ = manager.write_raw(&session.id, &reply);
                    }
                }
                SessionEvent::CwdChanged { cwd, .. } => {
                    cwd_count += 1;
                    let style = manager.local_shell_path_style(&session.id).unwrap();
                    let mapped =
                        nyaterm_files::file_browser::local_shell_cwd_to_browser_path(style, &cwd);
                    mapped_count += usize::from(mapped.is_some());
                    matched = mapped.is_some_and(|mapped| {
                        nyaterm_files::file_browser::file_browser_path_eq(
                            nyaterm_files::file_browser::FileBrowserBackendKind::Local,
                            &mapped,
                            &directory.path().to_string_lossy(),
                        )
                    });
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    manager.close(&session.id).unwrap();
    assert!(
        matched,
        "Git Bash cwd must resolve to the actual Windows directory: cwd_count={cwd_count}, mapped_count={mapped_count}, output_bytes={output_bytes}, error_seen={error_seen}"
    );
}
