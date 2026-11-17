use std::path::PathBuf;

use gpui::{Context, Window};
use nyaterm_core::completion::types::ShellDialect;
use nyaterm_files::file_browser::terminal_path::local_browser_path_to_shell_path;
use nyaterm_transport::directory_command::quote_path_for_shell;
use nyaterm_transport::local_shell::LocalShellPathStyle;
use nyaterm_transport::{
    DirectoryShell, SessionKind, build_directory_change_command, build_ssh_reconnect_cwd_command,
    local_directory_shell, valid_terminal_directory_path,
};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::models::{SessionLaunchConfig, StartupCommandRequest};

use super::TransferPathPart;

enum DirectoryTerminalLaunch {
    Local(PathBuf),
    Ssh(String),
}

fn directory_terminal_launch(
    path: &str,
    kind: SessionKind,
    style: LocalShellPathStyle,
) -> Option<DirectoryTerminalLaunch> {
    if !valid_terminal_directory_path(path) {
        return None;
    }
    match kind {
        SessionKind::LocalPty => {
            let guest_absolute = style == LocalShellPathStyle::Wsl
                && path.starts_with('/')
                && !path.starts_with("//");
            let path = PathBuf::from(path);
            (guest_absolute || path.is_absolute()).then_some(DirectoryTerminalLaunch::Local(path))
        }
        SessionKind::Ssh => build_ssh_reconnect_cwd_command(path).map(DirectoryTerminalLaunch::Ssh),
        _ => None,
    }
}

impl NyaTermApp {
    fn transfer_terminal_shell(&self, session_id: &str) -> Option<DirectoryShell> {
        // Runtime context describes the resolved shell, including Windows
        // Terminal profiles whose saved launcher is only an alias.
        if let Some(context) = self.commands.command_context(session_id) {
            match context.dialect {
                ShellDialect::PowerShell => return Some(DirectoryShell::PowerShell),
                ShellDialect::Cmd => return Some(DirectoryShell::Cmd),
                ShellDialect::Bash | ShellDialect::Zsh | ShellDialect::Fish => {
                    return Some(DirectoryShell::Posix);
                }
                ShellDialect::Unknown => {}
            }
        }
        let metadata = self.session.metadata(session_id)?;
        match &metadata.launch_config {
            SessionLaunchConfig::Local(config) => {
                match self.local_shell_path_style_for_session(session_id) {
                    LocalShellPathStyle::Wsl | LocalShellPathStyle::Msys => {
                        Some(DirectoryShell::Posix)
                    }
                    LocalShellPathStyle::Native => {
                        local_directory_shell(config.shell_path.as_deref(), cfg!(windows))
                    }
                }
            }
            SessionLaunchConfig::Ssh(_) | SessionLaunchConfig::AdbShell(_) => {
                Some(DirectoryShell::Posix)
            }
            _ => None,
        }
    }

    fn transfer_path_in_terminal_namespace(&self, path: &str) -> Option<String> {
        let session_id = self.session.active_id()?;
        if cfg!(windows)
            && matches!(
                self.session.metadata(session_id)?.launch_config,
                SessionLaunchConfig::Local(_)
            )
        {
            local_browser_path_to_shell_path(
                self.local_shell_path_style_for_session(session_id),
                path,
                self.local_wsl_distro_for_session(session_id).as_deref(),
            )
        } else {
            Some(path.to_string())
        }
    }

    pub(in crate::features::pages::transfers) fn quoted_transfer_terminal_path(
        &self,
        value: &str,
        part: TransferPathPart,
    ) -> Option<String> {
        let shell = self.transfer_terminal_shell(self.session.active_id()?)?;
        // A filename is literal data: a backslash in a POSIX name is not a
        // Windows separator, and a drive-like name is not a drive path.
        let path = if matches!(part, TransferPathPart::Name) {
            value.to_string()
        } else {
            self.transfer_path_in_terminal_namespace(value)?
        };
        quote_path_for_shell(&path, shell)
    }

    fn transfer_directory_command(&self, path: &str) -> Option<String> {
        let session_id = self.session.active_id()?;
        let shell = self.transfer_terminal_shell(session_id)?;
        let path = self.transfer_path_in_terminal_namespace(path)?;
        if shell == DirectoryShell::Posix
            && matches!(
                self.session.metadata(session_id)?.launch_config,
                SessionLaunchConfig::Ssh(_)
            )
        {
            return build_ssh_reconnect_cwd_command(&path);
        }
        build_directory_change_command(&path, shell, false)
    }

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
        let command = self.transfer_directory_command(path);
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
        // Validate the share's distribution and pass a guest path to --cd so
        // another distro's path cannot be reinterpreted when duplicating.
        let path = if cfg!(windows)
            && kind == SessionKind::LocalPty
            && self.local_shell_path_style_for_session(session_id) == LocalShellPathStyle::Wsl
        {
            self.transfer_path_in_terminal_namespace(path)
        } else {
            Some(path.to_string())
        };
        let style = self.local_shell_path_style_for_session(session_id);
        let Some(launch) = path.and_then(|path| directory_terminal_launch(&path, kind, style))
        else {
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
    use nyaterm_transport::local_shell::LocalShellPathStyle;

    use super::{DirectoryTerminalLaunch, directory_terminal_launch};

    #[test]
    fn new_local_terminal_uses_the_selected_directory_as_working_dir() {
        let path = std::env::temp_dir();
        let launch = directory_terminal_launch(
            &path.to_string_lossy(),
            SessionKind::LocalPty,
            LocalShellPathStyle::Native,
        );
        assert!(matches!(launch, Some(DirectoryTerminalLaunch::Local(dir)) if dir == path));
    }

    #[test]
    fn new_ssh_terminal_quotes_path_and_rejects_controls() {
        let launch = directory_terminal_launch(
            "/srv/O'Brien",
            SessionKind::Ssh,
            LocalShellPathStyle::Native,
        );
        assert!(matches!(launch, Some(DirectoryTerminalLaunch::Ssh(command))
            if command == "cd -- '/srv/O'\\''Brien'"));
        assert!(
            directory_terminal_launch(
                "/srv/x\nexit",
                SessionKind::Ssh,
                LocalShellPathStyle::Native
            )
            .is_none()
        );
        assert!(
            directory_terminal_launch("/srv/x", SessionKind::Telnet, LocalShellPathStyle::Native)
                .is_none()
        );
    }

    #[test]
    fn new_wsl_terminal_accepts_a_guest_absolute_working_directory_on_windows() {
        let launch =
            directory_terminal_launch("/root", SessionKind::LocalPty, LocalShellPathStyle::Wsl);
        assert!(
            matches!(launch, Some(DirectoryTerminalLaunch::Local(dir)) if dir == std::path::Path::new("/root"))
        );
        assert!(
            directory_terminal_launch("relative", SessionKind::LocalPty, LocalShellPathStyle::Wsl)
                .is_none()
        );
    }

    #[test]
    fn ssh_browser_paths_keep_the_remote_namespace_and_cd_option_guard() {
        use super::super::TransferPathPart;
        use crate::features::test_support::app_with_visible_ssh_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::{AppContext as _, TestAppContext};

        let root = TestConfigDir::new("nyaterm-browser-ssh-path");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_ssh_session(&mut cx, root.path(), "ssh-path-session");
        cx.update_entity(&app, |app, _| {
            assert_eq!(
                app.transfer_directory_command("-P"),
                Some("cd -- '-P'".into())
            );
            assert_eq!(
                app.quoted_transfer_terminal_path(r"/srv/O'Brien\notes", TransferPathPart::Full),
                Some("'/srv/O'\\''Brien\\notes'".into())
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn browser_terminal_actions_share_wsl_translation_and_literal_quoting() {
        use super::super::TransferPathPart;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::models::SessionLaunchConfig;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::{AppContext as _, TestAppContext};

        let root = TestConfigDir::new("nyaterm-browser-terminal-path");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "path-session");
        cx.update_entity(&app, |app, _| {
            let SessionLaunchConfig::Local(config) = &mut app
                .session
                .metadata_mut("path-session")
                .unwrap()
                .launch_config
            else {
                panic!("local session");
            };
            config.shell_path = Some("wsl.exe".into());
            config.shell_args = vec!["-d".into(), "Ubuntu".into()];
            for (browser, expected) in [
                (
                    r"E:\Projects\Rust\nyaterm",
                    "'/mnt/e/Projects/Rust/nyaterm'",
                ),
                (r"\\wsl.localhost\Ubuntu\root", "'/root'"),
                (r"\\wsl$\Ubuntu", "'/'"),
                (
                    r"\\wsl.localhost\Ubuntu\home\O'Brien & notes ",
                    "'/home/O'\\''Brien & notes '",
                ),
            ] {
                for part in [TransferPathPart::Full, TransferPathPart::Directory] {
                    assert_eq!(
                        app.quoted_transfer_terminal_path(browser, part).as_deref(),
                        Some(expected)
                    );
                }
                assert_eq!(
                    app.transfer_directory_command(browser),
                    Some(format!("cd {expected}"))
                );
            }
            for browser in [r"\\wsl.localhost\Debian\root", r"\\server\share", "x\nexit"] {
                assert!(
                    app.quoted_transfer_terminal_path(browser, TransferPathPart::Full)
                        .is_none()
                );
                assert!(app.transfer_directory_command(browser).is_none());
            }
            assert_eq!(
                app.quoted_transfer_terminal_path(
                    r"a\b $(echo x); O'Brien",
                    TransferPathPart::Name
                ),
                Some("'a\\b $(echo x); O'\\''Brien'".into())
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn browser_terminal_actions_use_the_resolved_windows_terminal_shell() {
        use super::super::TransferPathPart;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::models::SessionLaunchConfig;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::{AppContext as _, TestAppContext};
        use nyaterm_core::completion::types::{CompletionScope, ShellCommandContext, ShellDialect};

        let root = TestConfigDir::new("nyaterm-browser-terminal-shell");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "path-session");
        cx.update_entity(&app, |app, _| {
            let SessionLaunchConfig::Local(config) = &mut app
                .session
                .metadata_mut("path-session")
                .unwrap()
                .launch_config
            else {
                panic!("local session");
            };
            config.shell_path = Some("wt.exe".into());
            for (dialect, path, expected) in [
                (
                    ShellDialect::PowerShell,
                    r"E:\O'Brien & notes",
                    "'E:\\O''Brien & notes'",
                ),
                (ShellDialect::Cmd, r"E:\a & b", "\"E:\\a & b\""),
            ] {
                app.commands.set_command_context(
                    "path-session".into(),
                    ShellCommandContext {
                        scope: CompletionScope::legacy(),
                        dialect,
                        command_events: false,
                    },
                );
                assert_eq!(
                    app.quoted_transfer_terminal_path(path, TransferPathPart::Full)
                        .as_deref(),
                    Some(expected)
                );
            }
        });
    }
}
