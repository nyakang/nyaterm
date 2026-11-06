use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ops::Range;

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
