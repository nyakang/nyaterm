//! Public MCP execution defaults, independent of terminal presentation.

use nyaterm_mcp_protocol::TerminalBackend;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTerminalExecutionMode {
    #[default]
    Auto,
    Inline,
    Background,
}

impl McpTerminalExecutionMode {
    pub fn resolve_backend(
        self,
        requested: Option<TerminalBackend>,
        saved_connection: bool,
    ) -> TerminalBackend {
        match self {
            Self::Inline => TerminalBackend::InlinePty,
            Self::Background => TerminalBackend::SshExec,
            Self::Auto => requested.unwrap_or(if saved_connection {
                TerminalBackend::SshExec
            } else {
                TerminalBackend::InlinePty
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::McpTerminalExecutionMode;
    use crate::ai::{ExternalMcpSettings, McpTerminalDisplayMode};
    use nyaterm_mcp_protocol::TerminalBackend;

    #[test]
    fn forced_modes_override_clients_and_auto_preserves_target_defaults() {
        for saved in [false, true] {
            for requested in [
                None,
                Some(TerminalBackend::InlinePty),
                Some(TerminalBackend::SshExec),
            ] {
                assert_eq!(
                    McpTerminalExecutionMode::Inline.resolve_backend(requested, saved),
                    TerminalBackend::InlinePty
                );
                assert_eq!(
                    McpTerminalExecutionMode::Background.resolve_backend(requested, saved),
                    TerminalBackend::SshExec
                );
                assert_eq!(
                    McpTerminalExecutionMode::Auto.resolve_backend(requested, saved),
                    requested.unwrap_or(if saved {
                        TerminalBackend::SshExec
                    } else {
                        TerminalBackend::InlinePty
                    })
                );
            }
        }
    }

    #[test]
    fn legacy_display_settings_keep_their_meaning_and_default_to_automatic_execution() {
        for (value, mode) in [
            ("inline", McpTerminalDisplayMode::Inline),
            ("silent", McpTerminalDisplayMode::Silent),
        ] {
            let legacy = serde_json::json!({"terminal_display_mode":value});
            let loaded: ExternalMcpSettings = serde_json::from_value(legacy).unwrap();
            assert_eq!(loaded.terminal_display_mode, mode);
            assert_eq!(
                loaded.terminal_execution_mode,
                McpTerminalExecutionMode::Auto
            );
            for execution in [
                McpTerminalExecutionMode::Auto,
                McpTerminalExecutionMode::Inline,
                McpTerminalExecutionMode::Background,
            ] {
                let settings = ExternalMcpSettings {
                    terminal_execution_mode: execution,
                    ..loaded.clone()
                };
                let encoded = serde_json::to_value(&settings).unwrap();
                assert_eq!(encoded["terminal_display_mode"], value);
                assert_eq!(
                    serde_json::from_value::<ExternalMcpSettings>(encoded).unwrap(),
                    settings
                );
            }
        }
    }
}
