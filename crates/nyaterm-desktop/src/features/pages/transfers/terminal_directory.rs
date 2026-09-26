use std::path::PathBuf;

use gpui::{Context, Window};
use nyaterm_transport::{
    DirectoryShell, SessionKind, build_directory_change_command, build_ssh_reconnect_cwd_command,
    local_directory_shell, valid_terminal_directory_path,
};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::models::{SessionLaunchConfig, StartupCommandRequest};

enum DirectoryTerminalLaunch {
    Local(PathBuf),
    Ssh(String),
}

fn directory_terminal_launch(path: &str, kind: SessionKind) -> Option<DirectoryTerminalLaunch> {
    if !valid_terminal_directory_path(path) {
        return None;
    }
    match kind {
        SessionKind::LocalPty => {
            let path = PathBuf::from(path);
            path.is_absolute()
                .then_some(DirectoryTerminalLaunch::Local(path))
        }
        SessionKind::Ssh => build_ssh_reconnect_cwd_command(path).map(DirectoryTerminalLaunch::Ssh),
        _ => None,
    }
}

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn transfer_context_terminal_available(
        &self,
    ) -> bool {
        let Some(session_id) = self.session.active_id() else {
            return false;
        };
        !self.session.is_disconnected(session_id)
            && self
                .session
                .session_info(session_id)
                .is_some_and(|session| {
                    matches!(session.kind, SessionKind::LocalPty | SessionKind::Ssh)
                })
    }

    pub(in crate::features::pages::transfers) fn enter_transfer_directory_in_terminal(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.transfer_context_terminal_available() {
            self.shell
                .set_status(t!("fileExplorer.directoryTerminalUnavailable").to_string());
            cx.notify();
            return;
        }
        let command = self.session.active_id().and_then(|session_id| {
            let metadata = self.session.metadata(session_id)?;
            match &metadata.launch_config {
                SessionLaunchConfig::Ssh(_) => build_ssh_reconnect_cwd_command(path),
                SessionLaunchConfig::Local(config) => {
                    let shell = local_directory_shell(config.shell_path.as_deref(), cfg!(windows))?;
                    build_directory_change_command(
                        path,
                        shell,
                        cfg!(windows) && shell == DirectoryShell::Posix,
                    )
                }
                _ => None,
            }
        });
        let Some(command) = command else {
            self.shell
                .set_status(t!("fileExplorer.directoryTerminalInvalidPath").to_string());
            cx.notify();
            return;
        };
        let mut input = command.into_bytes();
        input.push(b'\r');
        if let Some(session_id) = self.session.active_id_owned() {
            self.send_terminal_input_to_session(session_id, input, cx);
        }
    }

    pub(in crate::features::pages::transfers) fn open_transfer_directory_in_new_terminal(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transfer_context_terminal_available() {
            self.shell
                .set_status(t!("fileExplorer.directoryTerminalUnavailable").to_string());
            cx.notify();
            return;
        }
        let Some(session_id) = self.session.active_id() else {
            return;
        };
        let Some(kind) = self
            .session
            .session_info(session_id)
            .map(|session| session.kind)
        else {
            return;
        };
        let Some(launch) = directory_terminal_launch(path, kind) else {
            self.shell
                .set_status(t!("fileExplorer.directoryTerminalInvalidPath").to_string());
            cx.notify();
            return;
        };
        match launch {
            DirectoryTerminalLaunch::Local(working_dir) => {
                self.duplicate_active_local_session_in_directory(working_dir, window, cx);
            }
            DirectoryTerminalLaunch::Ssh(command) => {
                let delay_ms = u64::from(
                    self.settings
                        .summary()
                        .interaction_duplicate_session_command_delay_ms,
                );
                self.duplicate_active_session_with_startup(
                    Some(StartupCommandRequest { command, delay_ms }),
                    window,
                    cx,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::SessionKind;

    use super::{DirectoryTerminalLaunch, directory_terminal_launch};

    #[test]
    fn new_local_terminal_uses_the_selected_directory_as_working_dir() {
        let path = std::env::temp_dir();
        let launch = directory_terminal_launch(&path.to_string_lossy(), SessionKind::LocalPty);
        assert!(matches!(launch, Some(DirectoryTerminalLaunch::Local(dir)) if dir == path));
    }

    #[test]
    fn new_ssh_terminal_quotes_path_and_rejects_controls() {
        let launch = directory_terminal_launch("/srv/O'Brien", SessionKind::Ssh);
        assert!(matches!(launch, Some(DirectoryTerminalLaunch::Ssh(command))
            if command == "cd -- '/srv/O'\\''Brien'"));
        assert!(directory_terminal_launch("/srv/x\nexit", SessionKind::Ssh).is_none());
        assert!(directory_terminal_launch("/srv/x", SessionKind::Telnet).is_none());
    }
}
