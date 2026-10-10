/// Per-connection local terminal config.
pub struct LocalSessionConfig {
    pub connection_id: Option<String>,
    pub shell_path: String,
    pub shell_args: String,
    pub shell_argv: Option<Vec<String>>,
    pub working_dir: Option<String>,
    pub fail_on_missing_working_dir: bool,
    pub name: String,
    pub encoding: String,
    /// When true, allow Local dynamic titles to be promoted for this connection.
    /// Windows cwd tracking and command-history confirmation are independent policies.
    pub dynamic_tab_title: bool,
    pub session_type: SessionType,
    pub remote_file_browser_enabled: bool,
    pub remote_stats_enabled: bool,
    pub zmodem_enabled: bool,
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
