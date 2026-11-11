//! Real ConPTY command submission and erase behavior; run separately in Windows CI.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::{LocalSessionConfig, SessionEvent, SessionManager};

struct Console {
    manager: SessionManager,
    id: String,
}

impl Console {
    fn new(powershell: bool) -> Self {
        let manager = SessionManager::new();
        let session = manager
            .create_local_session(LocalSessionConfig {
                shell_path: Some(
                    if powershell {
                        "powershell.exe"
                    } else {
                        "cmd.exe"
                    }
                    .into(),
                ),
                shell_args: if powershell {
                    vec!["-NoLogo".into(), "-NoProfile".into()]
                } else {
                    vec!["/d".into(), "/q".into()]
                },
                cols: 240,
                rows: 24,
                ..LocalSessionConfig::default()
            })
            .unwrap();
        let console = Self {
            manager,
            id: session.id,
        };
        console.wait_for_prompt();
        console
    }

    fn drain(&self) -> Vec<u8> {
        let mut output = Vec::new();
        for event in self.manager.drain_events(256).unwrap().events {
            match event {
                SessionEvent::Output { data, .. } => {
                    if data.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                        self.manager.write_raw(&self.id, b"\x1b[1;1R").unwrap();
                    }
                    output.extend_from_slice(&data);
                }
                SessionEvent::CleanupFailed { .. } => panic!("console cleanup failed"),
                _ => {}
            }
        }
        output
    }

    fn wait_for_prompt(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if self.drain().contains(&b'>') {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "console did not reach its prompt"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn replace_and_submit(&self, powershell: bool, file: &Path) {
        let _ = self.drain();
        let command = |value| {
            if powershell {
                format!(
                    "Set-Content -LiteralPath '{}' -Value '{value}'",
                    file.display()
                )
            } else {
                format!("echo {value}>\"{}\"", file.display())
            }
        };
        let wrong = command("BAD");
        self.manager.write_raw(&self.id, wrong.as_bytes()).unwrap();
        // Wait for the unsubmitted text to be echoed, then for the repaint to
        // settle. The file checkpoint below proves execution, rather than
        // mistaking the command's own echo for command output.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last_output = None;
        loop {
            if !self.drain().is_empty() {
                last_output = Some(Instant::now());
            }
            if last_output.is_some_and(|time| time.elapsed() >= Duration::from_millis(250)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "console did not echo the pending input"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!file.exists(), "a line without CR must remain unsubmitted");
        let mut replacement = vec![if powershell { 0x7f } else { 0x08 }; wrong.chars().count()];
        replacement.extend_from_slice(command("GOOD").as_bytes());
        replacement.push(b'\r');
        self.manager.write_raw(&self.id, &replacement).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut prompt_seen = false;
        loop {
            prompt_seen |= self.drain().contains(&b'>');
            if std::fs::metadata(file).is_ok_and(|metadata| metadata.len() > 0) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "CR did not execute the replaced command"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let contents = std::fs::read(file).unwrap();
        // Windows PowerShell may choose UTF-16; compare after stripping BOM and
        // zero bytes, without putting any terminal output in assertion messages.
        let text: Vec<u8> = contents
            .into_iter()
            .filter(|byte| !matches!(byte, 0 | 0xff | 0xfe))
            .collect();
        assert_eq!(String::from_utf8(text).unwrap().trim(), "GOOD");
        if !prompt_seen {
            self.wait_for_prompt();
        }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.manager.close(&self.id);
    }
}

#[test]
#[ignore = "requires real Windows ConPTY; explicitly run by Windows CI"]
fn windows_command_replacement_and_cr_submission_survive_nested_shell_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    for initial_powershell in [false, true] {
        let console = Console::new(initial_powershell);
        console.replace_and_submit(
            initial_powershell,
            &dir.path().join(format!("outer-{initial_powershell}.txt")),
        );
        let child = if initial_powershell {
            b"cmd.exe /d /q\r".as_slice()
        } else {
            b"powershell.exe -NoLogo -NoProfile\r".as_slice()
        };
        console.manager.write_raw(&console.id, child).unwrap();
        console.wait_for_prompt();
        console.replace_and_submit(
            !initial_powershell,
            &dir.path().join(format!("child-{initial_powershell}.txt")),
        );
        console.manager.write_raw(&console.id, b"exit\r").unwrap();
        console.wait_for_prompt();
        console.replace_and_submit(
            initial_powershell,
            &dir.path()
                .join(format!("returned-{initial_powershell}.txt")),
        );
    }
}
