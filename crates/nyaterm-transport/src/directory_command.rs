#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryShell {
    Posix,
    PowerShell,
    Cmd,
}

pub fn local_directory_shell(shell_path: Option<&str>, windows: bool) -> Option<DirectoryShell> {
    if !windows {
        return Some(DirectoryShell::Posix);
    }
    let default_shell = std::env::var("COMSPEC").ok();
    let executable = shell_path
        .or(default_shell.as_deref())?
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .split([' ', '"', '\''])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match executable.as_str() {
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => Some(DirectoryShell::PowerShell),
        "cmd" | "cmd.exe" => Some(DirectoryShell::Cmd),
        "bash" | "bash.exe" | "sh" | "sh.exe" | "zsh" | "zsh.exe" => Some(DirectoryShell::Posix),
        _ => None,
    }
}

pub fn valid_terminal_directory_path(path: &str) -> bool {
    !path.trim().is_empty() && !path.chars().any(char::is_control)
}

pub fn build_directory_change_command(
    path: &str,
    shell: DirectoryShell,
    windows_posix: bool,
) -> Option<String> {
    if !valid_terminal_directory_path(path) {
        return None;
    }
    match shell {
        DirectoryShell::PowerShell => Some(format!(
            "Set-Location -LiteralPath '{}'",
            path.replace('\'', "''")
        )),
        DirectoryShell::Cmd => {
            if path.contains(['"', '%', '!']) {
                return None;
            }
            Some(format!("cd /d \"{path}\""))
        }
        DirectoryShell::Posix => {
            let path = if windows_posix {
                let bytes = path.as_bytes();
                if bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && matches!(bytes[2], b'/' | b'\\')
                {
                    format!("/{}{}", (bytes[0] as char).to_ascii_lowercase(), &path[2..])
                        .replace('\\', "/")
                } else {
                    path.replace('\\', "/")
                }
            } else {
                path.to_string()
            };
            Some(format!("cd '{}'", path.replace('\'', "'\\''")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DirectoryShell, build_directory_change_command, local_directory_shell,
        valid_terminal_directory_path,
    };

    #[test]
    fn quotes_directory_paths_for_each_shell() {
        assert_eq!(
            build_directory_change_command("/tmp/O'Brien & sons", DirectoryShell::Posix, false),
            Some("cd '/tmp/O'\\''Brien & sons'".into())
        );
        assert_eq!(
            build_directory_change_command("C:\\Work\\O'Brien", DirectoryShell::PowerShell, false),
            Some("Set-Location -LiteralPath 'C:\\Work\\O''Brien'".into())
        );
        assert_eq!(
            build_directory_change_command("D:\\My Files", DirectoryShell::Cmd, false),
            Some("cd /d \"D:\\My Files\"".into())
        );
        assert_eq!(
            build_directory_change_command("C:\\My Files", DirectoryShell::Posix, true),
            Some("cd '/c/My Files'".into())
        );
    }

    #[test]
    fn rejects_unsafe_or_unknown_paths_and_shells() {
        assert!(!valid_terminal_directory_path("/tmp/x\nwhoami"));
        assert_eq!(
            build_directory_change_command("/tmp/x\nwhoami", DirectoryShell::Posix, false),
            None
        );
        assert_eq!(
            build_directory_change_command("D:\\%TEMP%", DirectoryShell::Cmd, false),
            None
        );
        assert_eq!(
            build_directory_change_command("D:\\!TEMP!", DirectoryShell::Cmd, false),
            None
        );
        assert_eq!(local_directory_shell(Some("nu.exe"), true), None);
        assert_eq!(
            local_directory_shell(Some("pwsh.exe"), true),
            Some(DirectoryShell::PowerShell)
        );
    }
}
