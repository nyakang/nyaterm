fn platform_default_shell() -> (CommandBuilder, String) {
    build_shell_command_from_spec(&default_shell_spec())
}

#[cfg(target_os = "macos")]
fn ensure_macos_interactive_path(cmd: &mut CommandBuilder) {
    let current_path = std::env::var("PATH").unwrap_or_default();
    let mut paths: Vec<String> = current_path
        .split(':')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();

    for path in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
        if !paths.iter().any(|existing| existing == path) && Path::new(path).exists() {
            paths.push(path.to_string());
        }
    }

    if !paths.is_empty() {
        cmd.env("PATH", paths.join(":"));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn configure_local_pty_environment(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
    #[cfg(target_os = "macos")]
    {
        set_utf8_env_if_missing_or_non_utf8(cmd, "LANG", "en_US.UTF-8");
        set_utf8_env_if_missing_or_non_utf8(cmd, "LC_CTYPE", "UTF-8");
    }
}

#[cfg(target_os = "macos")]
fn set_utf8_env_if_missing_or_non_utf8(cmd: &mut CommandBuilder, key: &str, default_value: &str) {
    let value = std::env::var(key)
        .ok()
        .filter(|value| is_utf8_locale(value))
        .unwrap_or_else(|| default_value.to_string());
    cmd.env(key, value);
}

#[cfg(target_os = "macos")]
fn is_utf8_locale(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace('_', "-");
    normalized.contains("utf-8") || normalized.contains("utf8")
}

fn infer_local_ai_execution_profile(shell_name: &str) -> AiExecutionProfile {
    let shell = shell_name.to_ascii_lowercase();
    if shell.contains("powershell") || shell.contains("pwsh") {
        AiExecutionProfile::Powershell
    } else if shell.contains("cmd") {
        AiExecutionProfile::Cmd
    } else if shell.contains("bash")
        || shell.contains("zsh")
        || shell.contains("fish")
        || shell.contains("wsl")
        || shell.ends_with("sh")
        || shell.contains("/sh")
        || shell.contains("\\sh")
    {
        AiExecutionProfile::Posix
    } else {
        AiExecutionProfile::SendOnly
    }
}
