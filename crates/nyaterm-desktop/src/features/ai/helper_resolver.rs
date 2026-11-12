use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const MCP_HELPER_OVERRIDE: &str = "NYATERM_MCP_HELPER";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum McpHelperStatus {
    Available,
    Missing,
}

pub(in crate::features) fn mcp_helper_status() -> McpHelperStatus {
    if resolve_mcp_helper().is_ok() {
        McpHelperStatus::Available
    } else {
        McpHelperStatus::Missing
    }
}

pub(in crate::features) fn resolve_mcp_helper() -> Result<PathBuf, String> {
    let debug_path = cfg!(debug_assertions)
        .then(|| std::env::var_os("PATH"))
        .flatten();
    resolve_mcp_helper_from(
        std::env::var_os(MCP_HELPER_OVERRIDE).as_deref(),
        std::env::current_exe().ok().as_deref(),
        debug_path.as_deref(),
    )
}

fn resolve_mcp_helper_from(
    explicit: Option<&OsStr>,
    executable: Option<&Path>,
    debug_path: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(explicit) = explicit.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(explicit);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(helper_missing_error);
    }

    let helper_name = if cfg!(windows) {
        "nyaterm-mcp.exe"
    } else {
        "nyaterm-mcp"
    };
    if let Some(directory) = executable.and_then(Path::parent) {
        let sibling = directory.join(helper_name);
        if sibling.is_file() {
            return Ok(sibling);
        }
        if directory.file_name().is_some_and(|name| name == "deps")
            && let Some(profile_directory) = directory.parent()
        {
            for candidate in [
                profile_directory.join(helper_name),
                profile_directory.join("deps").join(helper_name),
            ] {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    if let Some(debug_path) = debug_path {
        for directory in std::env::split_paths(debug_path) {
            let candidate = directory.join(helper_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(helper_missing_error())
}

fn helper_missing_error() -> String {
    "NyaTerm MCP helper is missing; reinstall NyaTerm or build nyaterm-mcp beside the application"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::resolve_mcp_helper_from;

    #[test]
    fn resolver_prefers_sibling_and_does_not_expose_override_paths_on_failure() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-helper");
        std::fs::create_dir_all(root.path()).unwrap();
        let helper = root.join(if cfg!(windows) {
            "nyaterm-mcp.exe"
        } else {
            "nyaterm-mcp"
        });
        std::fs::write(&helper, b"fixture").unwrap();
        let executable = root.join(if cfg!(windows) {
            "nyaterm.exe"
        } else {
            "nyaterm"
        });
        assert_eq!(
            resolve_mcp_helper_from(None, Some(&executable), None).unwrap(),
            helper
        );

        let missing = root.join("private-sensitive-location");
        let error = resolve_mcp_helper_from(Some(missing.as_os_str()), None, None).unwrap_err();
        assert!(!error.contains("private-sensitive-location"));
    }
}
