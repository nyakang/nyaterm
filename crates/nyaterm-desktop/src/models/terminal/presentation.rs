use std::time::Duration;

/// Large-output protection modes (Tauri XTerminal performanceMode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TerminalPerformanceMode {
    #[default]
    Normal,
    Overloaded,
}

/// In-pane large-output protection banner (Tauri PerformanceOverlayState).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalPerformanceOverlay {
    Overloaded,
    Recovered,
}

pub(crate) use nyaterm_terminal_runtime::work_policy::{TerminalPresentation, TerminalWorkPolicy};

pub(crate) const TERMINAL_OUTPUT_VISIBLE_BURST_OVERLOAD: usize = 256 * 1024;
pub(crate) const TERMINAL_UI_OUTPUT_TAIL_CAP: usize = 128 * 1024;
pub(crate) const TERMINAL_PERFORMANCE_RECOVERY_NOTICE: Duration = Duration::from_secs(3);
pub(crate) const TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM: Duration = Duration::from_millis(400);
