use std::path::Path;

use portable_pty::CommandBuilder;

use crate::environment::EnvironmentSnapshot;
use crate::session_config::LocalSessionConfig;

#[cfg(windows)]
mod windows_terminal;
#[cfg(all(test, windows))]
mod wsl_tests;

/// One startup decision shared by command construction and environment matching.
/// Arguments may contain secrets, so this type deliberately has no Debug impl.
pub(crate) struct ResolvedLocalShell {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
}

impl ResolvedLocalShell {
    pub(crate) fn validate_working_directory(
        &self,
        directory: Option<&Path>,
    ) -> std::io::Result<()> {
        #[cfg(windows)]
        if self.is_wsl() {
            // WSL validates its guest --cd directory; a host filesystem lookup
            // would reject valid Linux paths and paths overridden by --cd.
            return Ok(());
        }
        if let Some(directory) = directory {
            std::fs::read_dir(directory)?;
        }
        Ok(())
    }

    pub(crate) fn resolve(config: &LocalSessionConfig) -> Self {
        #[cfg(windows)]
        {
            Self::resolve_with_alias_resolver(config, windows_terminal::resolve_alias)
        }
        #[cfg(not(windows))]
        {
            Self::resolve_direct(config)
        }
    }

    #[cfg(windows)]
    fn resolve_with_alias_resolver(
        config: &LocalSessionConfig,
        resolve_alias: impl FnOnce(&[String]) -> Self,
    ) -> Self {
        let shell = Self::resolve_direct(config);
        if nyaterm_core::terminal::windows_terminal::is_windows_terminal_alias(&shell.program) {
            resolve_alias(&shell.args)
        } else {
            shell
        }
    }

    fn resolve_direct(config: &LocalSessionConfig) -> Self {
        let program = config
            .shell_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(crate::default_local_shell_path);
        let shell = Self {
            program,
            args: config.shell_args.clone(),
        };
        #[cfg(not(windows))]
        let shell = {
            let mut shell = shell;
            if shell.args.is_empty() && crate::should_use_interactive_login_args(&shell.program) {
                shell.args = vec!["--login".to_string(), "-i".to_string()];
            }
            shell
        };
        shell
    }

    pub(crate) fn command(&self, working_dir: Option<&Path>) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.program);
        #[cfg(windows)]
        if self.is_wsl() {
            // WSL paths belong to the Linux guest, not CreateProcess's cwd.
            // An explicit --cd wins; otherwise start in Home rather than
            // inheriting the application's Windows working directory.
            if !self.has_wsl_cd_arg() {
                command.arg("--cd");
                command.arg(
                    working_dir
                        .filter(|path| !path.as_os_str().is_empty())
                        .unwrap_or_else(|| Path::new("~")),
                );
            }
            command.args(self.args.iter().map(String::as_str));
            return command;
        }
        command.args(self.args.iter().map(String::as_str));
        if let Some(working_dir) = working_dir {
            command.cwd(working_dir);
        }
        command
    }

    #[cfg(windows)]
    fn is_wsl(&self) -> bool {
        Path::new(&self.program)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("wsl") || name.eq_ignore_ascii_case("wsl.exe")
            })
    }

    #[cfg(windows)]
    fn has_wsl_cd_arg(&self) -> bool {
        self.args
            .iter()
            // Arguments after these delimiters belong to the Linux command.
            .take_while(|arg| !matches!(arg.as_str(), "--" | "-e" | "--exec"))
            .any(|arg| arg == "--cd" || arg.starts_with("--cd="))
    }

    pub(crate) fn matches_environment(&self, snapshot: &EnvironmentSnapshot) -> bool {
        snapshot.matches_shell_path(Some(std::path::Path::new(&self.program)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::session_config::LocalSessionConfig;

    use super::ResolvedLocalShell;

    #[test]
    fn validates_host_directories_before_spawning_a_terminal() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-local-directory");
        let directory = root.path().join("中文 directory ; literal");
        std::fs::create_dir_all(&directory).unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "test").unwrap();
        let shell = ResolvedLocalShell {
            program: "custom-shell".into(),
            args: Vec::new(),
        };
        assert!(shell.validate_working_directory(Some(&directory)).is_ok());
        assert!(shell.validate_working_directory(Some(&file)).is_err());
        assert!(
            shell
                .validate_working_directory(Some(&root.path().join("missing")))
                .is_err()
        );
        assert_eq!(shell.command(Some(&directory)).get_argv().len(), 1);
    }

    #[test]
    fn direct_shell_preserves_explicit_arguments_and_empty_values() {
        let config = LocalSessionConfig {
            shell_path: Some("custom-shell".to_string()),
            shell_args: vec!["-Command".to_string(), String::new()],
            ..Default::default()
        };
        let shell = ResolvedLocalShell::resolve(&config);
        assert_eq!(shell.program, "custom-shell");
        assert_eq!(shell.args, config.shell_args);
        let expected: Vec<std::ffi::OsString> = ["custom-shell", "-Command", ""]
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(shell.command(None).get_argv(), &expected);
    }

    #[test]
    fn ordinary_shell_uses_local_cwd_only_when_configured() {
        let shell = ResolvedLocalShell {
            program: "custom-shell".to_string(),
            args: vec!["--interactive".to_string()],
        };
        assert_eq!(shell.command(None).get_cwd(), None);
        let dir = Path::new("directory with spaces");
        let command = shell.command(Some(dir));
        assert_eq!(
            command.get_cwd().map(|cwd| cwd.as_os_str()),
            Some(dir.as_os_str())
        );
        assert_eq!(command.get_argv(), shell.command(None).get_argv());
    }

    #[cfg(unix)]
    #[test]
    fn unix_login_defaults_and_wt_program_are_unchanged() {
        for (program, expected) in [("bash", vec!["--login", "-i"]), ("wt.exe", vec![])] {
            let shell = ResolvedLocalShell::resolve(&LocalSessionConfig {
                shell_path: Some(program.to_string()),
                ..Default::default()
            });
            assert_eq!(shell.program, program);
            assert_eq!(shell.args, expected);
        }
    }
}
