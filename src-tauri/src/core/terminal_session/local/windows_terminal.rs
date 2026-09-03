fn is_windows_terminal_alias(program: &str) -> bool {
    matches!(program.to_ascii_lowercase().as_str(), "wt" | "wt.exe")
}

#[cfg(target_os = "windows")]
fn resolve_windows_terminal_default_profile_shell(
    extra_args: Vec<String>,
) -> Option<ShellCommandSpec> {
    for settings_path in windows_terminal_settings_paths() {
        let Ok(raw_settings) = std::fs::read_to_string(settings_path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&raw_settings) else {
            continue;
        };
        let Some(commandline) = windows_terminal_default_profile_commandline(&settings) else {
            continue;
        };
        if let Some(spec) = shell_spec_from_windows_commandline(&commandline, extra_args.clone()) {
            return Some(spec);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn fallback_windows_terminal_shell(args: Vec<String>) -> ShellCommandSpec {
    ShellCommandSpec {
        program: resolve_program_for_spawn("powershell.exe"),
        args,
        resolution_source: ShellResolutionSource::WindowsTerminalFallback,
    }
}

#[cfg(target_os = "windows")]
fn windows_terminal_settings_paths() -> Vec<PathBuf> {
    let Some(local_data_dir) = dirs::data_local_dir() else {
        return Vec::new();
    };

    vec![
        local_data_dir
            .join("Packages")
            .join("Microsoft.WindowsTerminal_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        local_data_dir
            .join("Packages")
            .join("Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        local_data_dir
            .join("Microsoft")
            .join("Windows Terminal")
            .join("settings.json"),
        local_data_dir
            .join("Microsoft")
            .join("Windows Terminal Preview")
            .join("settings.json"),
    ]
}

#[cfg(target_os = "windows")]
fn windows_terminal_default_profile_commandline(settings: &serde_json::Value) -> Option<String> {
    let default_profile = settings.get("defaultProfile")?.as_str()?;
    let profiles = settings.get("profiles")?.get("list")?.as_array()?;

    profiles
        .iter()
        .find(|profile| {
            profile
                .get("guid")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|guid| guid.eq_ignore_ascii_case(default_profile))
        })
        .and_then(windows_terminal_profile_commandline)
}

#[cfg(target_os = "windows")]
fn windows_terminal_profile_commandline(profile: &serde_json::Value) -> Option<String> {
    if let Some(commandline) = profile
        .get("commandline")
        .and_then(serde_json::Value::as_str)
        .map(expand_windows_env_vars)
    {
        return Some(commandline);
    }

    let name = profile
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let source = profile
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    // Dynamic WT profiles often omit `commandline` and identify their host
    // solely through `source`. Resolve source before the localized/display
    // name so PowerShell Core is never downgraded to Windows PowerShell 5.1.
    if source == "windows.terminal.powershellcore" {
        Some("pwsh.exe".to_string())
    } else if source.contains("wsl") || name.contains("ubuntu") || name.contains("debian") {
        Some("wsl.exe".to_string())
    } else if name.contains("powershell") {
        Some("powershell.exe".to_string())
    } else if name.contains("command prompt") || name.contains("cmd") || name.contains("命令提示符")
    {
        Some("cmd.exe".to_string())
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn shell_spec_from_windows_commandline(
    commandline: &str,
    extra_args: Vec<String>,
) -> Option<ShellCommandSpec> {
    let mut parts = parse_shell_args(commandline).ok()?;
    if parts.is_empty() {
        return None;
    }

    let program = parts.remove(0);
    parts.extend(extra_args);

    Some(ShellCommandSpec {
        program: resolve_program_for_spawn(&program),
        args: parts,
        resolution_source: ShellResolutionSource::WindowsTerminalProfile,
    })
}

#[cfg(target_os = "windows")]
fn expand_windows_env_vars(value: &str) -> String {
    let mut expanded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('%') else {
            expanded.push_str(&rest[start..]);
            return expanded;
        };

        let name = &after_start[..end];
        if name.is_empty() {
            expanded.push_str("%%");
        } else if let Ok(env_value) = std::env::var(name) {
            expanded.push_str(&env_value);
        } else {
            expanded.push('%');
            expanded.push_str(name);
            expanded.push('%');
        }
        rest = &after_start[end + 1..];
    }

    expanded.push_str(rest);
    expanded
}

fn resolve_program_for_spawn(program: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        resolve_windows_program_for_spawn(program).unwrap_or_else(|| program.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        program.to_string()
    }
}

#[cfg(target_os = "windows")]
fn resolve_windows_program_for_spawn(program: &str) -> Option<String> {
    let program = trim_wrapping_quotes(program).trim();
    if program.is_empty() || looks_like_path(program) {
        return None;
    }

    resolve_windows_builtin_shell(program).or_else(|| find_windows_program_on_search_path(program))
}

#[cfg(target_os = "windows")]
fn resolve_windows_builtin_shell(program: &str) -> Option<String> {
    let lower = program.to_ascii_lowercase();
    match lower.as_str() {
        "cmd" | "cmd.exe" => {
            let mut candidates = Vec::new();
            if let Some(comspec) = std::env::var_os("COMSPEC").map(PathBuf::from) {
                candidates.push(comspec);
            }
            for system_dir in windows_system_dirs() {
                candidates.push(system_dir.join("cmd.exe"));
            }
            first_existing_file(candidates)
        }
        "powershell" | "powershell.exe" => {
            let mut candidates = Vec::new();
            for system_dir in windows_system_dirs() {
                candidates.push(
                    system_dir
                        .join("WindowsPowerShell")
                        .join("v1.0")
                        .join("powershell.exe"),
                );
            }
            first_existing_file(candidates)
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn find_windows_program_on_search_path(program: &str) -> Option<String> {
    let names = windows_program_candidate_names(program);
    let mut dirs = windows_default_search_dirs();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }

    for dir in dirs {
        for name in &names {
            if let Some(path) = first_existing_file([dir.join(name)]) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn windows_program_candidate_names(program: &str) -> Vec<String> {
    if Path::new(program).extension().is_some() {
        return vec![program.to_string()];
    }

    let mut names = vec![format!("{program}.exe")];
    if let Some(pathext) = std::env::var_os("PATHEXT") {
        for ext in pathext.to_string_lossy().split(';') {
            let ext = ext.trim();
            if ext.is_empty() {
                continue;
            }
            let normalized_ext = if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            };
            let candidate = format!("{program}{normalized_ext}");
            if !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&candidate))
            {
                names.push(candidate);
            }
        }
    }
    names
}

#[cfg(target_os = "windows")]
fn windows_default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.extend(windows_system_dirs());
    if let Some(windows_dir) = windows_dir() {
        dirs.push(windows_dir);
    }
    dirs
}

#[cfg(target_os = "windows")]
fn windows_system_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(windows_dir) = windows_dir() {
        dirs.push(windows_dir.join("System32"));
        dirs.push(windows_dir.join("Sysnative"));
        dirs.push(windows_dir.join("SysWOW64"));
    }
    dirs
}

#[cfg(target_os = "windows")]
fn windows_dir() -> Option<PathBuf> {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| {
            let fallback = PathBuf::from(r"C:\Windows");
            fallback.is_dir().then_some(fallback)
        })
}

#[cfg(target_os = "windows")]
fn first_existing_file<I>(paths: I) -> Option<String>
where
    I: IntoIterator<Item = PathBuf>,
{
    paths
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

fn should_treat_as_literal_program(value: &str) -> bool {
    !value.chars().any(char::is_whitespace)
        || path_exists(value)
        || looks_like_path(value)
        || is_quoted(value)
}

fn path_exists(value: &str) -> bool {
    Path::new(trim_wrapping_quotes(value)).exists()
}

fn looks_like_path(value: &str) -> bool {
    value.contains('\\') || value.contains('/') || Path::new(value).is_absolute()
}

fn is_quoted(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
}

fn trim_wrapping_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    if is_quoted(trimmed) {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}
