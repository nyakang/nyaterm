//! Suppress command suggestions for interactive/pager programs (Tauri parity).

use crate::sanitize_terminal_command;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One authoritative suppression reason. An outer OSC command can hand input
/// to a child shell, but a known pager must not be released by prompt-like echo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CommandSuggestionSuppression {
    #[default]
    None,
    CommandRunning,
    InteractiveProgram,
}

impl CommandSuggestionSuppression {
    pub fn is_suppressed(self) -> bool {
        self != Self::None
    }

    pub fn command_started(&mut self) {
        if *self != Self::InteractiveProgram {
            *self = Self::CommandRunning;
        }
    }

    pub fn release_at_shell_prompt(&mut self) -> bool {
        if *self != Self::CommandRunning {
            return false;
        }
        *self = Self::None;
        true
    }
}

fn interactive_commands() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        HashSet::from([
            "btop", "htop", "less", "man", "more", "nano", "nvim", "top", "vi", "vim", "watch",
        ])
    })
}

fn sudo_option_requires_value() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| HashSet::from(["-C", "-g", "-h", "-p", "-T", "-u"]))
}

fn split_command_segments(input: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            current.push(ch);
            escaped = true;
            continue;
        }
        if (ch == '\'' || ch == '"') && quote.is_none() {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        if quote == Some(ch) {
            quote = None;
            current.push(ch);
            continue;
        }
        if quote.is_none() && (ch == '|' || ch == ';' || ch == '&') {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                segments.push(trimmed.to_string());
            }
            current.clear();
            continue;
        }
        current.push(ch);
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }
    segments
}

fn tokenize_shell_like(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if (ch == '\'' || ch == '"') && quote.is_none() {
            quote = Some(ch);
            continue;
        }
        if quote == Some(ch) {
            quote = None;
            continue;
        }
        if quote.is_none() && ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn command_name(token: &str) -> String {
    let normalized = token.replace('\\', "/");
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_lowercase()
}

fn skip_env_prefix(tokens: &[String], index: usize) -> usize {
    let mut next = index;
    while next < tokens.len() && tokens[next].contains('=') {
        let name = tokens[next].split('=').next().unwrap_or("");
        if name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            break;
        }
        next += 1;
    }
    next
}

fn skip_sudo_like(tokens: &[String], index: usize) -> usize {
    let mut next = index + 1;
    while next < tokens.len() {
        let token = tokens[next].as_str();
        if token == "--" {
            return next + 1;
        }
        if !token.starts_with('-') || token == "-" {
            return next;
        }
        if sudo_option_requires_value().contains(token) {
            next += 2;
        } else {
            next += 1;
        }
    }
    next
}

fn unwrap_command(tokens: &[String]) -> Vec<String> {
    let mut index = skip_env_prefix(tokens, 0);
    while index < tokens.len() {
        let name = command_name(&tokens[index]);
        if name == "sudo" || name == "doas" {
            index = skip_sudo_like(tokens, index);
            index = skip_env_prefix(tokens, index);
            continue;
        }
        if name == "env" {
            index = skip_env_prefix(tokens, index + 1);
            continue;
        }
        if matches!(name.as_str(), "command" | "builtin" | "exec" | "time") {
            index += 1;
            continue;
        }
        if name == "nice" || name == "nohup" {
            index += 1;
            while index < tokens.len() && tokens[index].starts_with('-') {
                index += 1;
            }
            continue;
        }
        break;
    }
    tokens[index..].to_vec()
}

fn has_option(tokens: &[String], long_name: &str, short_name: Option<char>) -> bool {
    tokens.iter().any(|token| {
        token == long_name
            || short_name.is_some_and(|short| {
                token.starts_with('-') && !token.starts_with("--") && token.contains(short)
            })
    })
}

fn command_segment_starts_interactive_program(segment: &str) -> bool {
    let tokens = unwrap_command(&tokenize_shell_like(segment));
    let Some(first) = tokens.first() else {
        return false;
    };
    let name = command_name(first);
    if name.is_empty() {
        return false;
    }
    if interactive_commands().contains(name.as_str()) {
        return true;
    }
    if name == "journalctl" {
        return !has_option(&tokens, "--no-pager", None);
    }
    if name == "tail" {
        return has_option(&tokens, "--follow", Some('f'));
    }
    false
}

/// True when an unquoted `|` joins commands; the right side then reads a pipe,
/// not a terminal, so it cannot open an interactive shell.
fn has_unquoted_pipe(input: &str) -> bool {
    let chars: Vec<char> = input.chars().collect();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, &ch) in chars.iter().enumerate() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote.is_some() {
            if quote == Some(ch) {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '|'
            && chars.get(index + 1) != Some(&'|')
            && (index == 0 || chars[index - 1] != '|')
        {
            return true;
        }
    }
    false
}

/// Positional arguments, skipping options and the values of `with_value`
/// options, plus whether a `--` ended option parsing.
fn positionals<'a>(tokens: &'a [String], with_value: &[&str]) -> (Vec<&'a str>, bool) {
    let mut result = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token == "--" {
            return (result, true);
        }
        if with_value.contains(&token) {
            index += 2;
            continue;
        }
        if token.starts_with('-') && token != "-" {
            index += 1;
            continue;
        }
        result.push(token);
        index += 1;
    }
    (result, false)
}

fn short_cluster_has(token: &str, letters: &str) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 1
        && token[1..].chars().all(|c| c.is_ascii_alphabetic())
        && token[1..].chars().any(|c| letters.contains(c))
}

fn container_exec_is_interactive(args: &[String], short: &str, long: &[&str]) -> bool {
    args.iter().any(|token| {
        long.contains(&token.as_str())
            || (short_cluster_has(token, short) && !short_cluster_has(token, "d"))
    })
}

fn segment_enters_foreign_shell(segment: &str) -> bool {
    let tokens = unwrap_command(&tokenize_shell_like(segment));
    let Some(first) = tokens.first() else {
        return false;
    };
    let name = command_name(first);
    let args = &tokens[1..];
    let subcommands: Vec<&str> = args
        .iter()
        .filter(|token| !token.starts_with('-'))
        .map(String::as_str)
        .collect();
    let sub = subcommands.first().copied();
    match name.trim_end_matches(".exe") {
        // A remote command (`ssh host uptime`) or a forward-only flag returns to
        // the local prompt by itself, so only a bare `ssh host` opens a shell.
        "ssh" => {
            const VALUE: &[&str] = &[
                "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o",
                "-p", "-Q", "-R", "-S", "-W", "-w", "-B",
            ];
            let batch = args
                .iter()
                .any(|t| ["-N", "-f", "-W", "-O", "-G", "-Q", "-V", "-s"].contains(&t.as_str()));
            let (positional, _) = positionals(args, VALUE);
            !batch && positional.len() == 1
        }
        "mosh" => {
            const VALUE: &[&str] = &["--ssh", "--port", "-p", "--predict", "--family", "--server"];
            let (positional, dashdash) = positionals(args, VALUE);
            !dashdash && positional.len() == 1
        }
        "telnet" => !positionals(args, &["-l", "-e", "-b", "-s"]).0.is_empty(),
        "docker" | "podman" | "nerdctl" => {
            let verb = if sub == Some("container") {
                subcommands.get(1).copied()
            } else {
                sub
            };
            matches!(verb, Some("exec" | "run" | "attach"))
                && container_exec_is_interactive(args, "it", &["--interactive", "--tty"])
        }
        "kubectl" | "oc" => match sub {
            Some("rsh") => true,
            Some("exec" | "debug" | "run") => {
                container_exec_is_interactive(args, "it", &["--stdin", "--tty"])
            }
            _ => false,
        },
        "lxc" | "incus" => matches!(sub, Some("exec" | "shell")),
        "machinectl" => matches!(sub, Some("shell" | "login")),
        "multipass" | "limactl" | "distrobox" | "toolbox" => {
            matches!(sub, Some("shell" | "enter"))
        }
        "vagrant" => sub == Some("ssh") && !has_option(args, "--command", Some('c')),
        "nsenter" => true,
        "chroot" => positionals(args, &[]).0.len() == 1,
        "wsl" => {
            const MANAGEMENT: &[&str] = &[
                "-l",
                "--list",
                "--shutdown",
                "-t",
                "--terminate",
                "--install",
                "--update",
                "--status",
                "--version",
                "-v",
                "--set-default",
                "-s",
                "--set-version",
                "--unregister",
                "--export",
                "--import",
                "--mount",
                "--unmount",
                "-e",
                "--exec",
                "--help",
                "--manage",
                "--import-in-place",
                "--set-default-version",
            ];
            let (positional, dashdash) =
                positionals(args, &["-d", "--distribution", "-u", "--user", "--cd"]);
            !dashdash
                && !args.iter().any(|t| MANAGEMENT.contains(&t.as_str()))
                && positional.iter().all(|p| p.starts_with('~'))
        }
        "adb" => positionals(args, &["-s", "-t", "-H", "-P", "-L"]).0 == ["shell"],
        _ => false,
    }
}

/// How many interactive shells on another host, container or distro this
/// submitted command line opens. Same-host switches (`su`, `sudo -i`, `bash`)
/// do not count: the file system the suggestions list stays the same.
pub fn command_enters_foreign_shell(command: &str) -> usize {
    let normalized = sanitize_terminal_command(command);
    if normalized.is_empty()
        || has_unquoted_pipe(&normalized)
        || (normalized.ends_with('&') && !normalized.ends_with("&&"))
    {
        return 0;
    }
    split_command_segments(&normalized)
        .iter()
        .filter(|segment| segment_enters_foreign_shell(segment))
        .count()
}

/// True when the submitted line ends the current shell (`exit`, `logout`).
pub fn command_leaves_shell(command: &str) -> bool {
    split_command_segments(&sanitize_terminal_command(command))
        .iter()
        .any(|segment| {
            let tokens = unwrap_command(&tokenize_shell_like(segment));
            tokens
                .first()
                .is_some_and(|first| matches!(command_name(first).as_str(), "exit" | "logout"))
        })
}

pub fn command_starts_suggestion_suppressing_program(command: &str) -> bool {
    let normalized = sanitize_terminal_command(command);
    if normalized.is_empty() {
        return false;
    }
    split_command_segments(&normalized)
        .into_iter()
        .any(|segment| command_segment_starts_interactive_program(&segment))
}

/// Conservative submission fallback for command anchors when OSC marks are absent.
/// Script runners with an explicit script or `-c` task are not interactive.
pub fn command_starts_interactive_input(command: &str) -> bool {
    if command_starts_suggestion_suppressing_program(command) {
        return true;
    }
    split_command_segments(&sanitize_terminal_command(command))
        .into_iter()
        .any(|segment| {
            let tokens = unwrap_command(&tokenize_shell_like(&segment));
            let Some(first) = tokens.first() else {
                return false;
            };
            let name = command_name(first);
            match name.as_str() {
                "cmd" | "powershell" | "pwsh" | "bash" | "zsh" | "fish" | "sh" | "wsl" => {
                    tokens.len() == 1
                        || tokens
                            .iter()
                            .skip(1)
                            .all(|token| matches!(token.to_ascii_lowercase().as_str(), "-i" | "/k"))
                }
                "python" | "python3" | "python2" | "node" | "ruby" | "php" => {
                    tokens.len() == 1
                        || tokens
                            .iter()
                            .any(|token| matches!(token.as_str(), "-i" | "--interactive" | "-a"))
                }
                "mysql" | "mariadb" | "psql" | "sqlite3" | "redis-cli" | "irb" | "ipython"
                | "R" | "julia" | "lua" | "telnet" | "ftp" | "sftp" => true,
                _ => false,
            }
        })
}

/// Free Type needs a line editor, unlike command navigation which also handles
/// pagers and TUIs. Compound commands cannot establish a single editor context.
pub fn command_starts_line_editor(command: &str) -> bool {
    let segments = split_command_segments(&sanitize_terminal_command(command));
    if segments.len() != 1 || command_starts_suggestion_suppressing_program(command) {
        return false;
    }
    let tokens = unwrap_command(&tokenize_shell_like(&segments[0]));
    let Some(first) = tokens.first() else {
        return false;
    };
    match command_name(first).as_str() {
        "docker" | "podman" => {
            if !tokens
                .get(1)
                .is_some_and(|token| token == "exec" || token == "run")
                || !has_option(&tokens, "--interactive", Some('i'))
                || !has_option(&tokens, "--tty", Some('t'))
            {
                return false;
            }
            let mut index = 2;
            while let Some(option) = tokens.get(index).filter(|token| token.starts_with('-')) {
                match option.as_str() {
                    "-i" | "-t" | "-it" | "-ti" | "--interactive" | "--tty" | "--rm" => index += 1,
                    "-u" | "--user" | "-w" | "--workdir" | "-e" | "--env" => index += 2,
                    _ => return false,
                }
            }
            // Skip the container/image; do not mistake cat's arguments for a shell.
            let child = &tokens[index.saturating_add(1).min(tokens.len())..];
            child.first().is_some_and(|token| {
                matches!(command_name(token).as_str(), "bash" | "zsh" | "fish" | "sh")
            }) && command_starts_line_editor(&child.join(" "))
        }
        // A remote command may be cat, even with -t. Require its prompt rather
        // than treating all ssh invocations as a line editor.
        "ssh" | "telnet" | "ftp" | "sftp" => false,
        _ => {
            !tokens.iter().any(|token| {
                matches!(
                    token.to_ascii_lowercase().as_str(),
                    "-c" | "-e" | "--execute" | "--command" | "-command" | "-file" | "/c"
                ) || token.starts_with("--execute=")
                    || token.starts_with("--command=")
            }) && command_starts_interactive_input(command)
        }
    }
}

pub fn is_pager_search_or_command_input(value: &str) -> bool {
    value.trim_start().starts_with(['/', '?', ':'])
}

pub fn is_pager_single_key_input(data: &str) -> bool {
    matches!(data, " " | "b" | "g" | "G" | "n" | "N" | "q")
}

#[cfg(test)]
mod tests {
    use crate::command_suggestion_suppression::CommandSuggestionSuppression;

    #[test]
    fn foreign_shells_are_those_on_another_host_container_or_distro() {
        use crate::command_suggestion_suppression::command_enters_foreign_shell as enters;
        for command in [
            "ssh host",
            "ssh -p 2222 -i ~/.ssh/key user@host",
            "/usr/bin/ssh -o StrictHostKeyChecking=no host",
            "sudo ssh host",
            "FOO=1 ssh host",
            "exec ssh host",
            "mosh host",
            "telnet 10.0.0.1 23",
            "docker exec -it web bash",
            "docker exec -ti web sh",
            "docker run --rm -it ubuntu",
            "podman exec --interactive --tty c bash",
            "docker container exec -it web bash",
            "kubectl exec -it pod -- sh",
            "oc rsh pod",
            "lxc exec box -- bash",
            "machinectl shell root@box",
            "vagrant ssh",
            "nsenter -t 1 -a",
            "chroot /mnt/root",
            "wsl",
            "wsl -d Ubuntu",
            "wsl ~",
            "adb shell",
            "adb -s emulator-5554 shell",
            "cd /tmp && ssh host",
        ] {
            assert_eq!(enters(command), 1, "{command}");
        }
        assert_eq!(enters("ssh a; ssh b"), 2);
    }

    #[test]
    fn commands_that_return_by_themselves_are_not_foreign_shells() {
        use crate::command_suggestion_suppression::command_enters_foreign_shell as enters;
        for command in [
            "",
            "ls -la",
            "su",
            "sudo -i",
            "bash",
            "tmux",
            "ssh host uptime",
            "ssh -N -L 8080:localhost:80 host",
            "ssh -f host",
            "ssh -W host:22 jump",
            "ssh host &",
            "cat key | ssh host 'cat > key'",
            "mosh host -- top",
            "telnet",
            "docker exec web ls",
            "docker run --rm ubuntu echo hi",
            "docker run -d nginx",
            "docker run -dit nginx",
            "docker ps",
            "kubectl get pods",
            "kubectl exec pod -- ls",
            "lxc list",
            "vagrant up",
            "chroot /mnt/root /bin/ls",
            "wsl -l -v",
            "wsl --shutdown",
            "wsl -d Ubuntu -e ls",
            "wsl ls",
            "adb devices",
            "adb shell ls /sdcard",
            "echo ssh host",
            "grep ssh file",
        ] {
            assert_eq!(enters(command), 0, "{command}");
        }
    }

    #[test]
    fn exit_and_logout_leave_the_shell() {
        use crate::command_suggestion_suppression::command_leaves_shell as leaves;
        for command in [
            "exit",
            "logout",
            "exit 1",
            "  exit  ",
            "cd /; exit",
            "exec exit",
        ] {
            assert!(leaves(command), "{command}");
        }
        for command in ["", "echo exit", "exitcode", "ls exit", "ssh exit"] {
            assert!(!leaves(command), "{command}");
        }
    }

    #[test]
    fn confirmed_child_prompt_releases_only_outer_command_suppression() {
        let mut state = CommandSuggestionSuppression::None;
        state.command_started();
        assert!(state.release_at_shell_prompt());
        assert!(!state.is_suppressed());

        state = CommandSuggestionSuppression::InteractiveProgram;
        state.command_started();
        assert!(!state.release_at_shell_prompt());
        assert!(state.is_suppressed());
    }

    use super::{
        command_starts_suggestion_suppressing_program, is_pager_search_or_command_input,
        is_pager_single_key_input,
    };

    #[test]
    fn detects_interactive_and_sudo_wrappers() {
        assert!(command_starts_suggestion_suppressing_program("vim file"));
        assert!(command_starts_suggestion_suppressing_program(
            "sudo -u root htop"
        ));
        assert!(command_starts_suggestion_suppressing_program(
            "tail -f /var/log/syslog"
        ));
        assert!(!command_starts_suggestion_suppressing_program(
            "journalctl --no-pager"
        ));
        assert!(!command_starts_suggestion_suppressing_program("ls -la"));
    }

    #[test]
    fn pager_input_helpers() {
        assert!(is_pager_search_or_command_input("/error"));
        assert!(is_pager_single_key_input("q"));
        assert!(!is_pager_single_key_input("x"));
    }
}

#[cfg(test)]
mod navigation_tests {
    #[test]
    fn free_type_context_excludes_foreground_programs_and_noninteractive_tasks() {
        use super::command_starts_line_editor;
        for command in [
            "bash",
            "sudo zsh -i",
            "python3 -i",
            "mysql -u root",
            "docker exec -it box sh",
            "podman run -it --rm alpine /bin/sh",
        ] {
            assert!(command_starts_line_editor(command), "{command}");
        }
        for command in [
            "cat",
            "tee log",
            "less file",
            "vim",
            "telnet host",
            "ssh host cat",
            "docker exec -it box cat sh",
            "docker exec -it box sh -c cat",
            "python3 task.py",
            "mysql -e select",
            "bash; cat",
            "bash | cat",
        ] {
            assert!(!command_starts_line_editor(command), "{command}");
        }
    }

    #[test]
    fn interpreter_detection_distinguishes_scripts_from_interactive_input() {
        use super::command_starts_interactive_input;
        for command in [
            "python3",
            "sudo python -i",
            "mysql -u root",
            "node",
            "env A=1 psql",
            "less file",
        ] {
            assert!(command_starts_interactive_input(command), "{command}");
        }
        for command in [
            "python3 script.py",
            "node task.js",
            "python -c 'print(1)'",
            "ls",
        ] {
            assert!(!command_starts_interactive_input(command), "{command}");
        }
    }
}
