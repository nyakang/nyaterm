/// Per-connection local terminal config.
pub struct LocalSessionConfig {
    pub connection_id: Option<String>,
    pub shell_path: String,
    pub shell_args: String,
    pub working_dir: Option<String>,
    pub fail_on_missing_working_dir: bool,
    pub name: String,
    pub encoding: String,
    /// When true, enable Local dynamic-title/cwd integration for this
    /// connection. Command-history confirmation remains an independent policy.
    pub dynamic_tab_title: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellResolutionSource {
    Direct,
    WindowsTerminalProfile,
    WindowsTerminalFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellCommandSpec {
    program: String,
    args: Vec<String>,
    resolution_source: ShellResolutionSource,
}
