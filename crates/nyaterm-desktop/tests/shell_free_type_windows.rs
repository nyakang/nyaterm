//! Opt-in real ConPTY/PowerShell validation using only synthetic command input.
#![cfg(windows)]

use std::time::{Duration, Instant};

use nyaterm_core::terminal::editing::{EditIntent, plan_edit};
use nyaterm_core::terminal::input_tracker::TerminalInputState;
use nyaterm_terminal::TerminalScreen;
use nyaterm_terminal::editing::resolve_input;
use nyaterm_terminal::navigation::{NavigationKey, navigation_key_bytes};
use nyaterm_transport::{LocalSessionConfig, SessionEvent, SessionManager};

struct Shell {
    manager: SessionManager,
    id: String,
    screen: TerminalScreen,
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.manager.close(&self.id);
    }
}

impl Shell {
    fn wait_for(&mut self, ready: impl Fn(&TerminalScreen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                Instant::now() < deadline,
                "synthetic shell input did not reach expected echo/cursor"
            );
            let drain = self
                .manager
                .drain_events_blocking_with_output_budget(128, 65536, Duration::from_millis(50))
                .unwrap();
            for event in drain.events {
                if let SessionEvent::Output { session_id, data } = event
                    && session_id == self.id
                {
                    self.screen.advance(&data);
                }
            }
            for reply in self.screen.take_effects().pty_write {
                self.manager.write(&self.id, &reply).unwrap();
            }
            if ready(&self.screen) {
                return;
            }
        }
    }

    fn edit(
        &mut self,
        input: &TerminalInputState,
        intent: EditIntent<'_>,
        paste: bool,
    ) -> TerminalInputState {
        let plan = plan_edit(input, 0, intent).unwrap();
        let key = if plan.move_right {
            NavigationKey::Right
        } else {
            NavigationKey::Left
        };
        let mut bytes = navigation_key_bytes(key, self.screen.application_cursor_keys())
            .repeat(plan.move_steps);
        bytes.extend(std::iter::repeat_n(0x7f, plan.delete_steps));
        if paste && self.screen.bracketed_paste() {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(plan.insert_text.as_bytes());
        if paste && self.screen.bracketed_paste() {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.manager.write(&self.id, &bytes).unwrap();
        let predicted = plan.predicted;
        self.wait_for(|screen| {
            resolve_input(&screen.snapshot(), &predicted.value, predicted.cursor).is_some()
        });
        predicted
    }
}

#[test]
#[ignore = "requires Windows ConPTY and an interactive PowerShell with PSReadLine"]
fn powershell_echo_confirms_cursor_delete_replace_paste_and_soft_wrap() {
    let manager = SessionManager::new();
    let info = manager.create_local_session(LocalSessionConfig {
        shell_path: Some("powershell.exe".into()),
        shell_args: vec!["-NoLogo".into(), "-NoProfile".into(), "-NoExit".into(), "-Command".into(),
            "Import-Module PSReadLine; Set-PSReadLineOption -HistorySaveStyle SaveNothing; function prompt { 'NYA> ' }; Clear-Host".into()],
        cols: 40,
        rows: 12,
        ..LocalSessionConfig::default()
    }).expect("start isolated PowerShell");
    let mut shell = Shell {
        manager,
        id: info.id,
        screen: TerminalScreen::new(40, 12),
    };
    shell.wait_for(|screen| {
        let snapshot = screen.snapshot();
        snapshot
            .line(snapshot.cursor.row)
            .is_some_and(|line| line.trim() == "NYA>")
    });
    let input = TerminalInputState {
        value: "echo abcdef".into(),
        cursor: 11,
        ..TerminalInputState::default()
    };
    shell
        .manager
        .write(&shell.id, input.value.as_bytes())
        .unwrap();
    shell
        .wait_for(|screen| resolve_input(&screen.snapshot(), &input.value, input.cursor).is_some());
    let input = shell.edit(&input, EditIntent::Move(7), false);
    let input = shell.edit(&input, EditIntent::Delete(6..8), false);
    let input = shell.edit(
        &input,
        EditIntent::Replace {
            range: 5..7,
            text: "XY",
        },
        false,
    );
    let input = shell.edit(
        &input,
        EditIntent::Replace {
            range: 5..7,
            text: "0123456789012345678901234567890123456789",
        },
        true,
    );
    assert!(
        resolve_input(&shell.screen.snapshot(), &input.value, input.cursor)
            .unwrap()
            .cells
            .iter()
            .any(|cell| cell.row > 0)
    );
    let input = shell.edit(&input, EditIntent::Move(5), false);
    assert_eq!(input.cursor, 5);
    shell.manager.write(&shell.id, b"\x03").unwrap();
}

#[test]
#[ignore = "requires Windows ConPTY and an interactive PowerShell with PSReadLine"]
fn powershell_confirms_a_plain_suffix_without_executing_the_command() {
    let manager = SessionManager::new();
    let info = manager.create_local_session(LocalSessionConfig {
        shell_path: Some("powershell.exe".into()),
        shell_args: vec!["-NoLogo".into(), "-NoProfile".into(), "-NoExit".into(), "-Command".into(),
            "Import-Module PSReadLine; Set-PSReadLineOption -HistorySaveStyle SaveNothing; function prompt { 'NYA> ' }; Clear-Host".into()],
        cols: 80,
        rows: 12,
        ..LocalSessionConfig::default()
    }).expect("start isolated PowerShell");
    let mut shell = Shell {
        manager,
        id: info.id,
        screen: TerminalScreen::new(80, 12),
    };
    shell.wait_for(|screen| {
        let snapshot = screen.snapshot();
        snapshot
            .line(snapshot.cursor.row)
            .is_some_and(|line| line.trim() == "NYA>")
    });
    let initial = "Write-Out";
    let target = "Write-Output 'NYATERM_SUFFIX_TEST'";
    shell.manager.write(&shell.id, initial.as_bytes()).unwrap();
    shell.wait_for(|screen| resolve_input(&screen.snapshot(), initial, initial.len()).is_some());
    // The desktop TCP tests validate this is the only emitted payload. This
    // real-shell test validates that the suffix remains an unsubmitted buffer.
    shell
        .manager
        .write(&shell.id, target.strip_prefix(initial).unwrap().as_bytes())
        .unwrap();
    shell.wait_for(|screen| resolve_input(&screen.snapshot(), target, target.len()).is_some());
    shell.manager.write(&shell.id, b"\x03").unwrap();
}
