use std::path::PathBuf;

pub fn resolve_codex_executable(configured: Option<&str>) -> Result<PathBuf, String> {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    if let Some(path) = configured {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Ok(candidate);
        }
        if candidate.components().count() > 1 {
            return Err("Configured Codex executable was not found".to_string());
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["codex.cmd", "codex.exe", "codex"]
    } else {
        &["codex"]
    };
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    #[cfg(windows)]
    for (variable, suffix) in [
        ("APPDATA", "npm"),
        ("LOCALAPPDATA", "pnpm"),
        ("NVM_SYMLINK", ""),
    ] {
        if let Some(root) = std::env::var_os(variable) {
            for name in names {
                let candidate = PathBuf::from(&root).join(suffix).join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err("Codex CLI was not found in PATH or common install locations".to_string())
}
