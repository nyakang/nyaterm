use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ops::Range;

use crate::action_links::ActionLinkPlatform;
use crate::models::quick_commands::CommandHistoryEntry;
use crate::terminal::editing::{ShellLineEditing, supports_scalar_editing};
use crate::terminal::input_tracker::MAX_TRACKED_INPUT_BYTES;

/// Multiline whole commands require paste review rather than scalar insertion.
pub fn supports_full_command_fill(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TRACKED_INPUT_BYTES
        && (supports_scalar_editing(text)
            || (text.contains(['\r', '\n'])
                && !text
                    .chars()
                    .any(|ch| ch.is_control() && !matches!(ch, '\r' | '\n' | '\t'))))
}

/// Ephemeral terminal operation policy, independent of transport and presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionFillPolicy {
    AppendOnly,
    ReplaceAllowed,
}

impl CompletionFillPolicy {
    pub fn allows(self, input: &str, target: &str) -> bool {
        (target.starts_with(input) || self == Self::ReplaceAllowed || target.contains(['\r', '\n']))
            && supports_full_command_fill(target)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellDialect {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
    #[default]
    Unknown,
}

impl ShellDialect {
    pub fn from_program(program: &str) -> Self {
        let name = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        match name.strip_suffix(".exe").unwrap_or(&name) {
            "bash" => Self::Bash,
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            "pwsh" | "powershell" => Self::PowerShell,
            "cmd" => Self::Cmd,
            _ => Self::Unknown,
        }
    }
    /// Line-editing backend implied by the launched program alone.
    ///
    /// `None` means the program is not a known Windows console host: remote
    /// POSIX shells and nested shells must be classified from live evidence
    /// (OSC 133 anchor or matched echo) instead.
    pub fn line_editing_backend(self) -> Option<ShellLineEditing> {
        match self {
            Self::Cmd => Some(ShellLineEditing::LegacyConsole),
            Self::PowerShell => Some(ShellLineEditing::AnsiLineEditor),
            Self::Bash | Self::Zsh | Self::Fish | Self::Unknown => None,
        }
    }
    /// Platform implied by a dialect alone. PowerShell also runs on POSIX;
    /// callers need current prompt or host evidence to classify it.
    ///
    /// `None` for shells that carry no platform information (Telnet/Serial,
    /// or a prompt the tracker has not classified yet).
    pub fn action_link_platform(self) -> Option<ActionLinkPlatform> {
        match self {
            Self::Cmd => Some(ActionLinkPlatform::Windows),
            Self::Bash | Self::Zsh | Self::Fish => Some(ActionLinkPlatform::Posix),
            Self::PowerShell | Self::Unknown => None,
        }
    }
    pub fn native_options(self, command: &str) -> bool {
        let name = command.to_ascii_lowercase();
        match self {
            Self::PowerShell => {
                matches!(
                    name.as_str(),
                    "ls" | "dir"
                        | "gci"
                        | "cat"
                        | "type"
                        | "gc"
                        | "echo"
                        | "write"
                        | "pwd"
                        | "cd"
                        | "chdir"
                        | "sl"
                        | "cp"
                        | "copy"
                        | "cpi"
                        | "mv"
                        | "move"
                        | "mi"
                        | "rm"
                        | "del"
                        | "erase"
                        | "rd"
                        | "rmdir"
                        | "ri"
                        | "mkdir"
                        | "md"
                        | "start"
                        | "saps"
                        | "ps"
                        | "gps"
                        | "kill"
                        | "spps"
                        | "sort"
                        | "select"
                        | "where"
                        | "foreach"
                ) || name.split_once('-').is_some_and(|(verb, _)| {
                    matches!(
                        verb,
                        "get"
                            | "set"
                            | "new"
                            | "remove"
                            | "select"
                            | "where"
                            | "add"
                            | "clear"
                            | "start"
                            | "stop"
                            | "test"
                            | "invoke"
                            | "write"
                            | "read"
                            | "out"
                            | "format"
                            | "measure"
                            | "import"
                            | "export"
                            | "join"
                            | "split"
                            | "compare"
                            | "resolve"
                    )
                })
            }
            Self::Cmd => matches!(
                name.as_str(),
                "dir"
                    | "cd"
                    | "copy"
                    | "del"
                    | "erase"
                    | "type"
                    | "set"
                    | "move"
                    | "ren"
                    | "mkdir"
                    | "rmdir"
                    | "rd"
                    | "md"
                    | "assoc"
                    | "attrib"
                    | "break"
                    | "call"
                    | "chcp"
                    | "chdir"
                    | "cls"
                    | "color"
                    | "date"
                    | "echo"
                    | "endlocal"
                    | "exit"
                    | "fc"
                    | "find"
                    | "findstr"
                    | "ftype"
                    | "help"
                    | "mklink"
                    | "more"
                    | "path"
                    | "pause"
                    | "popd"
                    | "prompt"
                    | "pushd"
                    | "setlocal"
                    | "shift"
                    | "sort"
                    | "time"
                    | "title"
                    | "ver"
                    | "verify"
                    | "vol"
                    | "where"
                    | "xcopy"
                    | "robocopy"
                    | "tasklist"
                    | "taskkill"
                    | "ipconfig"
                    | "ping"
                    | "net"
                    | "netstat"
                    | "sc"
                    | "reg"
                    | "schtasks"
                    | "wmic"
            ),
            _ => false,
        }
    }
    pub fn case_insensitive_commands(self) -> bool {
        matches!(self, Self::PowerShell | Self::Cmd)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompletionMode {
    #[default]
    Token,
    History,
}

/// Only the fingerprint is retained; endpoint and user names never enter diagnostics.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompletionScope(pub String);
impl CompletionScope {
    pub fn new(parts: &[&str]) -> Self {
        let mut hash = Sha256::new();
        for part in parts {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        Self(hex::encode(hash.finalize()))
    }
    pub fn legacy() -> Self {
        Self("legacy".into())
    }
}
impl std::fmt::Debug for CompletionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompletionScope([REDACTED])")
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopedHistoryRecord {
    pub version: u32,
    pub scope: CompletionScope,
    pub dialect: ShellDialect,
    pub command: String,
    pub use_count: u32,
    pub last_used_at_ms: u64,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}
impl std::fmt::Debug for ScopedHistoryRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedHistoryRecord")
            .field("version", &self.version)
            .field("command", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// A consistent, schema-neutral view of both history tables. Never serialized.
pub struct CompletionHistorySnapshot {
    pub revision: u64,
    pub global_history: Vec<CommandHistoryEntry>,
    pub scoped_history: Vec<ScopedHistoryRecord>,
}

impl std::fmt::Debug for CompletionHistorySnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionHistorySnapshot")
            .field("revision", &self.revision)
            .field("global_count", &self.global_history.len())
            .field("scoped_count", &self.scoped_history.len())
            .finish()
    }
}

#[derive(Clone)]
pub struct CompletionContext {
    pub input: String,
    pub cursor: usize,
    pub token_range: Range<usize>,
    pub prefix: String,
    pub preceding: Vec<String>,
    pub command_path: Vec<String>,
    pub option_value: Option<String>,
    pub dialect: ShellDialect,
    pub scope: CompletionScope,
}
impl std::fmt::Debug for CompletionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompletionContext([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Command,
    Subcommand,
    Option,
    Argument,
    History,
    QuickCommand,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateSource {
    Catalog,
    History,
    QuickCommand,
    LegacyHistory,
}

#[derive(Clone)]
pub struct CompletionCandidate {
    pub display: String,
    pub insert_text: String,
    pub description: String,
    pub range: Range<usize>,
    pub kind: CandidateKind,
    pub sources: Vec<CandidateSource>,
    pub use_count: u32,
    pub last_used_at_ms: u64,
    pub score: u32,
    pub indices: Vec<u32>,
    pub fillable: bool,
    pub context_priority: u8,
}
impl std::fmt::Debug for CompletionCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionCandidate")
            .field("kind", &self.kind)
            .field("sources", &self.sources)
            .field("fillable", &self.fillable)
            .finish_non_exhaustive()
    }
}
impl CompletionCandidate {
    pub fn target_line(&self, input: &str) -> Option<String> {
        let before = input.get(..self.range.start)?;
        let after = input.get(self.range.end..)?;
        Some(format!("{before}{}{after}", self.insert_text))
    }
}
#[derive(Debug, Default)]
pub struct CompletionResult {
    pub candidates: Vec<CompletionCandidate>,
    pub revision: u64,
}

/// Transport's known launch/detection facts, separate from terminal echo evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandContext {
    pub scope: CompletionScope,
    pub dialect: ShellDialect,
    pub command_events: bool,
}

#[cfg(test)]
mod tests {
    use crate::action_links::ActionLinkPlatform;
    use crate::completion::types::{ShellDialect, supports_full_command_fill};
    use crate::terminal::editing::ShellLineEditing;
    use crate::terminal::input_tracker::MAX_TRACKED_INPUT_BYTES;

    #[test]
    fn shell_dialects_name_the_platform_their_tools_run_on() {
        assert_eq!(
            ShellDialect::Cmd.action_link_platform(),
            Some(ActionLinkPlatform::Windows)
        );
        for dialect in [ShellDialect::Bash, ShellDialect::Zsh, ShellDialect::Fish] {
            assert_eq!(
                dialect.action_link_platform(),
                Some(ActionLinkPlatform::Posix)
            );
        }
        assert_eq!(ShellDialect::Unknown.action_link_platform(), None);
        assert_eq!(ShellDialect::PowerShell.action_link_platform(), None);
    }

    #[test]
    fn console_hosts_pick_their_own_line_editing_backend() {
        assert_eq!(
            ShellDialect::Cmd.line_editing_backend(),
            Some(ShellLineEditing::LegacyConsole)
        );
        assert_eq!(
            ShellDialect::PowerShell.line_editing_backend(),
            Some(ShellLineEditing::AnsiLineEditor)
        );
        for dialect in [
            ShellDialect::Bash,
            ShellDialect::Zsh,
            ShellDialect::Fish,
            ShellDialect::Unknown,
        ] {
            assert_eq!(dialect.line_editing_backend(), None);
        }
    }

    #[test]
    fn whole_command_fill_allows_reviewable_multiline_text_and_rejects_terminal_controls() {
        for text in [
            "echo hi",
            "echo first\\\r\nsecond\n\nthird",
            "echo e\u{301}\n\t😀",
        ] {
            assert!(supports_full_command_fill(text));
        }
        for text in [
            "",
            "echo\t",
            "e\u{301}",
            "echo\n\x1b[201~",
            "echo\n\0",
            "echo\r\x7f",
        ] {
            assert!(!supports_full_command_fill(text));
        }
        assert!(!supports_full_command_fill(&format!(
            "{}\n",
            "x".repeat(MAX_TRACKED_INPUT_BYTES)
        )));
    }
}
