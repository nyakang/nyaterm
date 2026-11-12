use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_RPC_LINE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_INLINE_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_TEXT_READ_BYTES: u64 = 64 * 1024;
pub const MAX_TEXT_WRITE_BYTES: usize = 1024 * 1024;

pub mod capability {
    pub const ENVIRONMENT: &str = "session.environment";
    pub const CONNECTION_LIST: &str = "connection.list";
    pub const SESSION_OPEN: &str = "session.open";
    pub const SESSION_GET: &str = "session.get";
    pub const TERMINAL_EXECUTE: &str = "terminal.execute";
    pub const TERMINAL_START: &str = "terminal.start";
    pub const TERMINAL_POLL: &str = "terminal.poll";
    pub const TERMINAL_STOP: &str = "terminal.stop";
    pub const TERMINAL_RECENT_OUTPUT: &str = "terminal.recent_output";
    pub const SFTP_HOME: &str = "sftp.home";
    pub const SFTP_LIST: &str = "sftp.list";
    pub const SFTP_STAT: &str = "sftp.stat";
    pub const SFTP_READ: &str = "sftp.read";
    pub const SFTP_WRITE: &str = "sftp.write";
    pub const SFTP_MKDIR: &str = "sftp.mkdir";
    pub const SFTP_RENAME: &str = "sftp.rename";
    pub const SFTP_DELETE: &str = "sftp.delete";
    pub const SFTP_CHMOD: &str = "sftp.chmod";
    pub const OUTPUT_READ: &str = "tool.output.read";
}

pub mod tool {
    pub const GET_ENVIRONMENT: &str = "get_environment";
    pub const CONNECTION_LIST: &str = "connection_list";
    pub const SESSION_OPEN: &str = "session_open";
    pub const SESSION_GET: &str = "session_get";
    pub const TERMINAL_EXECUTE: &str = "terminal_execute";
    pub const TERMINAL_START: &str = "terminal_start";
    pub const TERMINAL_POLL: &str = "terminal_poll";
    pub const TERMINAL_STOP: &str = "terminal_stop";
    pub const TERMINAL_RECENT_OUTPUT: &str = "terminal_recent_output";
    pub const SFTP_HOME: &str = "sftp_home";
    pub const SFTP_LIST: &str = "sftp_list";
    pub const SFTP_STAT: &str = "sftp_stat";
    pub const SFTP_READ_TEXT: &str = "sftp_read_text";
    pub const SFTP_WRITE_TEXT: &str = "sftp_write_text";
    pub const SFTP_MKDIR: &str = "sftp_mkdir";
    pub const SFTP_RENAME: &str = "sftp_rename";
    pub const SFTP_DELETE: &str = "sftp_delete";
    pub const SFTP_CHMOD: &str = "sftp_chmod";
    pub const OUTPUT_READ: &str = "tool_output_read";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityAccess {
    Read,
    SensitiveRead,
    Write,
    DestructiveWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolDefinition {
    pub tool: &'static str,
    pub capability: &'static str,
    pub description: &'static str,
    pub access: CapabilityAccess,
    pub requires_session: bool,
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub open_world_hint: bool,
}

pub const MCP_TOOL_REGISTRY: &[McpToolDefinition] = &[
    McpToolDefinition {
        tool: tool::GET_ENVIRONMENT,
        capability: capability::ENVIRONMENT,
        description: "Return scoped NyaTerm sessions and the optional active and default sessions.",
        access: CapabilityAccess::Read,
        requires_session: false,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: false,
    },
    McpToolDefinition {
        tool: tool::CONNECTION_LIST,
        capability: capability::CONNECTION_LIST,
        description: "List saved terminal connections using safe metadata only.",
        access: CapabilityAccess::Read,
        requires_session: false,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: false,
    },
    McpToolDefinition {
        tool: tool::SESSION_OPEN,
        capability: capability::SESSION_OPEN,
        description: "Open a saved terminal connection. Default mode=create opens a new session. Prefer mode=reuse_or_create to obtain a usable authorized shell; use an idempotencyKey for retries.",
        access: CapabilityAccess::Write,
        requires_session: false,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SESSION_GET,
        capability: capability::SESSION_GET,
        description: "Return safe metadata and capability availability for a scoped session.",
        access: CapabilityAccess::Read,
        requires_session: true,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: false,
    },
    McpToolDefinition {
        tool: tool::TERMINAL_EXECUTE,
        capability: capability::TERMINAL_EXECUTE,
        description: "Execute a command in an existing scoped NyaTerm terminal session.",
        access: CapabilityAccess::Write,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::TERMINAL_START,
        capability: capability::TERMINAL_START,
        description: "Start a job. Prefer a saved SSH connection target for noninteractive commands: it defaults to ssh_exec without opening a terminal. Session targets default to inline_pty. Exec does not inherit an interactive shell cwd or environment. Use an idempotencyKey for retries.",
        access: CapabilityAccess::Write,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::TERMINAL_POLL,
        capability: capability::TERMINAL_POLL,
        description: "Read incremental UTF-8 output and execution/terminal state of a job owned by this client. eof only means the current output tail, not process completion.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: false,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::TERMINAL_STOP,
        capability: capability::TERMINAL_STOP,
        description: "Request one interrupt of an owned job. Accepted or sent does not mean the remote command has exited; poll to confirm. An uncertain inline terminal needs manual recovery.",
        access: CapabilityAccess::Write,
        requires_session: false,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::TERMINAL_RECENT_OUTPUT,
        capability: capability::TERMINAL_RECENT_OUTPUT,
        description: "Read recent ANSI-free terminal output for a scoped session.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: true,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_HOME,
        capability: capability::SFTP_HOME,
        description: "Return the remote home directory.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: true,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_LIST,
        capability: capability::SFTP_LIST,
        description: "List a remote directory.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: true,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_STAT,
        capability: capability::SFTP_STAT,
        description: "Read remote path metadata.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: true,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_READ_TEXT,
        capability: capability::SFTP_READ,
        description: "Read up to 64 KiB of a remote UTF-8 text file.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: true,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_WRITE_TEXT,
        capability: capability::SFTP_WRITE,
        description: "Write a remote UTF-8 text file with optional conflict protection.",
        access: CapabilityAccess::Write,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_MKDIR,
        capability: capability::SFTP_MKDIR,
        description: "Create a remote directory.",
        access: CapabilityAccess::Write,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_RENAME,
        capability: capability::SFTP_RENAME,
        description: "Rename or move a remote path.",
        access: CapabilityAccess::Write,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_DELETE,
        capability: capability::SFTP_DELETE,
        description: "Delete a remote path using NyaTerm's existing delete semantics.",
        access: CapabilityAccess::DestructiveWrite,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: true,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::SFTP_CHMOD,
        capability: capability::SFTP_CHMOD,
        description: "Change remote path permissions.",
        access: CapabilityAccess::Write,
        requires_session: true,
        read_only_hint: false,
        destructive_hint: false,
        open_world_hint: true,
    },
    McpToolDefinition {
        tool: tool::OUTPUT_READ,
        capability: capability::OUTPUT_READ,
        description: "Read another chunk of a large result produced on this MCP connection.",
        access: CapabilityAccess::SensitiveRead,
        requires_session: false,
        read_only_hint: true,
        destructive_hint: false,
        open_world_hint: false,
    },
];

pub fn definition_for_tool(name: &str) -> Option<&'static McpToolDefinition> {
    MCP_TOOL_REGISTRY
        .iter()
        .find(|definition| definition.tool == name)
}

pub fn definition_for_capability(id: &str) -> Option<&'static McpToolDefinition> {
    MCP_TOOL_REGISTRY
        .iter()
        .find(|definition| definition.capability == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryDocument {
    pub version: u32,
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub generation: String,
    pub permission_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthParams {
    pub token: String,
    pub generation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientIdentifyParams {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityExecuteParams {
    #[serde(default)]
    pub request_id: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestCancelParams {
    pub request_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmptyArgs {}

/// A saved connection is independent of any visible terminal session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionTarget {
    Connection {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
    Session {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
}
impl ExecutionTarget {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session { session_id } => Some(session_id),
            Self::Connection { .. } => None,
        }
    }
    pub fn connection_id(&self) -> Option<&str> {
        match self {
            Self::Connection { connection_id } => Some(connection_id),
            Self::Session { .. } => None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileTargetArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionArgs {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionOpenArgs {
    pub connection_id: String,
    #[serde(default)]
    pub mode: SessionOpenMode,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionOpenMode {
    #[default]
    Create,
    ReuseOrCreate,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBackend {
    #[default]
    InlinePty,
    SshExec,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStartArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub command: String,
    #[serde(default)]
    pub backend: Option<TerminalBackend>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

impl TerminalStartArgs {
    pub fn backend(&self) -> TerminalBackend {
        self.backend.unwrap_or_else(|| {
            if self
                .target
                .as_ref()
                .is_some_and(|target| target.connection_id().is_some())
            {
                TerminalBackend::SshExec
            } else {
                TerminalBackend::InlinePty
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPollArgs {
    pub job_id: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStopArgs {
    pub job_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalJobStatus {
    Starting,
    Running,
    Cancelling,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalProcessState {
    NotStarted,
    Active,
    ExitConfirmed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAvailability {
    Ready,
    Busy,
    RecoveryRequired,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStartResult {
    pub job_id: String,
    pub session_id: String,
    pub backend: TerminalBackend,
    pub status: TerminalJobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPollResult {
    pub job_id: String,
    pub session_id: String,
    pub backend: TerminalBackend,
    pub status: TerminalJobStatus,
    pub process_state: TerminalProcessState,
    pub terminal_state: TerminalAvailability,
    pub session_exists: bool,
    pub connected: bool,
    pub exit_code: Option<i32>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub output: String,
    pub offset: usize,
    pub next_offset: usize,
    pub base_offset: usize,
    pub total_bytes: usize,
    pub eof: bool,
    pub output_complete: bool,
    pub source_truncated: bool,
    pub dropped_before_offset: bool,
    pub stop_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStopResult {
    pub job_id: String,
    pub status: TerminalJobStatus,
    pub stop_state: String,
    pub process_state: TerminalProcessState,
    pub terminal_state: TerminalAvailability,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionStartResult {
    pub job_id: String,
    pub target: ExecutionTarget,
    pub backend: TerminalBackend,
    pub status: TerminalJobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionPollResult {
    pub job_id: String,
    pub target: ExecutionTarget,
    pub backend: TerminalBackend,
    pub status: TerminalJobStatus,
    pub process_state: TerminalProcessState,
    pub exit_code: Option<i32>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub output: String,
    pub offset: usize,
    pub next_offset: usize,
    pub base_offset: usize,
    pub total_bytes: usize,
    pub eof: bool,
    pub output_complete: bool,
    pub source_truncated: bool,
    pub dropped_before_offset: bool,
    pub stop_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionStopResult {
    pub job_id: String,
    pub target: ExecutionTarget,
    pub status: TerminalJobStatus,
    pub stop_state: String,
    pub process_state: TerminalProcessState,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalExecuteArgs {
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub return_job: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRecentOutputArgs {
    pub session_id: String,
    #[serde(default)]
    pub lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpReadTextArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpWriteTextArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub expected_mtime: Option<u64>,
    #[serde(default)]
    pub expected_size: Option<u64>,
    #[serde(default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpMkdirArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    pub path: String,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpRenameArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    pub old_path: String,
    pub new_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpChmodArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExecutionTarget>,
    pub path: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputReadArgs {
    pub output_id: String,
    pub offset: usize,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContractError {
    pub tool: String,
    pub message: String,
}

impl std::fmt::Display for ToolContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.tool, self.message)
    }
}

impl std::error::Error for ToolContractError {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<TerminalAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_job_id: Option<String>,
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentResult {
    pub active_session_id: Option<String>,
    pub default_session_id: Option<String>,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionSummary {
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub group_path: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionListResult {
    pub connections: Vec<ConnectionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionOpenResult {
    pub session_id: String,
    pub connection_id: String,
    pub name: String,
    pub r#type: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<TerminalAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_job_id: Option<String>,
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub connected: bool,
    pub cwd: Option<String>,
    pub terminal_execution: String,
    pub sftp_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalExecuteResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub output: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRecentOutputResult {
    pub session_id: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpHomeResult {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SftpFileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
    pub mtime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_path_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SftpStatResult {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub size: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
    pub uid: String,
    pub gid: String,
    pub mtime: u64,
    pub atime: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpReadTextResult {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub mtime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_nanos: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpWriteTextResult {
    pub status: String,
    pub mtime: Option<u64>,
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_nanos: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtectedOutputResult {
    pub preview: String,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_id: Option<String>,
    pub total_bytes: usize,
    #[serde(default)]
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputChunkResult {
    pub data: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub eof: bool,
}

pub fn validate_tool_arguments(name: &str, value: &Value) -> Result<(), ToolContractError> {
    if supports_connection_target(name)
        && value.get("target").is_some()
        && value.get("sessionId").is_some()
    {
        return contract_error(name, "target and sessionId are mutually exclusive");
    }
    match name {
        tool::GET_ENVIRONMENT | tool::CONNECTION_LIST => {
            parse_contract::<EmptyArgs>(name, value)?;
        }
        tool::SESSION_OPEN => {
            let args = parse_contract::<SessionOpenArgs>(name, value)?;
            require_identifier(name, "connectionId", &args.connection_id)?;
            validate_idempotency_key(name, args.idempotency_key.as_deref())?;
        }
        tool::SESSION_GET => {
            let args = parse_contract::<SessionArgs>(name, value)?;
            require_identifier(name, "sessionId", &args.session_id)?;
        }
        tool::SFTP_HOME => {
            let args = parse_contract::<FileTargetArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
        }
        tool::TERMINAL_EXECUTE => {
            let args = parse_contract::<TerminalExecuteArgs>(name, value)?;
            validate_idempotency_key(name, args.idempotency_key.as_deref())?;
            if let Some(id) = args.session_id.as_deref() {
                require_identifier(name, "sessionId", id)?;
            }
            if args.command.trim().is_empty() {
                return contract_error(name, "command must not be empty");
            }
            if args.command.len() > MAX_TEXT_WRITE_BYTES {
                return contract_error(name, "command exceeds the 1 MiB limit");
            }
            if args
                .timeout_ms
                .is_some_and(|timeout| !(1_000..=300_000).contains(&timeout))
            {
                return contract_error(name, "timeoutMs must be between 1000 and 300000");
            }
        }
        tool::TERMINAL_START => {
            let args = parse_contract::<TerminalStartArgs>(name, value)?;
            validate_target(
                name,
                args.target.as_ref(),
                args.session_id.as_deref(),
                false,
            )?;
            if args
                .target
                .as_ref()
                .is_some_and(|target| target.connection_id().is_some())
                && args.backend() != TerminalBackend::SshExec
            {
                return contract_error(name, "connection targets require ssh_exec");
            }
            validate_idempotency_key(name, args.idempotency_key.as_deref())?;
            if args.command.trim().is_empty()
                || args.command.len() > MAX_TEXT_WRITE_BYTES
                || args.command.contains('\0')
            {
                return contract_error(
                    name,
                    "command must be nonempty, NUL-free and at most 1 MiB",
                );
            }
            if args
                .timeout_ms
                .is_some_and(|t| !(1_000..=86_400_000).contains(&t))
            {
                return contract_error(name, "timeoutMs must be between 1000 and 86400000");
            }
        }
        tool::TERMINAL_POLL => {
            let args = parse_contract::<TerminalPollArgs>(name, value)?;
            require_identifier(name, "jobId", &args.job_id)?;
            if args
                .max_bytes
                .is_some_and(|n| !(4..=MAX_INLINE_OUTPUT_BYTES).contains(&n))
            {
                return contract_error(name, "maxBytes must be between 4 and 65536");
            }
        }
        tool::TERMINAL_STOP => {
            let args = parse_contract::<TerminalStopArgs>(name, value)?;
            require_identifier(name, "jobId", &args.job_id)?;
        }
        tool::TERMINAL_RECENT_OUTPUT => {
            let args = parse_contract::<TerminalRecentOutputArgs>(name, value)?;
            require_identifier(name, "sessionId", &args.session_id)?;
            if args.lines.is_some_and(|lines| !(1..=500).contains(&lines)) {
                return contract_error(name, "lines must be between 1 and 500");
            }
        }
        tool::SFTP_LIST | tool::SFTP_STAT | tool::SFTP_DELETE => {
            let args = parse_contract::<PathArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
            require_path(name, "path", &args.path)?;
        }
        tool::SFTP_READ_TEXT => {
            let args = parse_contract::<SftpReadTextArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
            require_path(name, "path", &args.path)?;
            if args
                .max_bytes
                .is_some_and(|size| size == 0 || size > MAX_TEXT_READ_BYTES)
            {
                return contract_error(name, "maxBytes must be between 1 and 65536");
            }
        }
        tool::SFTP_WRITE_TEXT => {
            let args = parse_contract::<SftpWriteTextArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
            require_path(name, "path", &args.path)?;
            if args.content.len() > MAX_TEXT_WRITE_BYTES {
                return contract_error(name, "content exceeds the 1 MiB limit");
            }
            if let Some(hash) = args.expected_hash.as_deref()
                && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return contract_error(
                    name,
                    "expectedHash must be a 64-character SHA-256 hex value",
                );
            }
        }
        tool::SFTP_MKDIR => {
            let args = parse_contract::<SftpMkdirArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
            require_path(name, "path", &args.path)?;
            if let Some(mode) = args.mode.as_deref() {
                validate_mode(name, mode)?;
            }
        }
        tool::SFTP_RENAME => {
            let args = parse_contract::<SftpRenameArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
            require_path(name, "old_path", &args.old_path)?;
            require_path(name, "newPath", &args.new_path)?;
        }
        tool::SFTP_CHMOD => {
            let args = parse_contract::<SftpChmodArgs>(name, value)?;
            validate_target(name, args.target.as_ref(), Some(&args.session_id), true)?;
            require_path(name, "path", &args.path)?;
            validate_mode(name, &args.mode)?;
        }
        tool::OUTPUT_READ => {
            let args = parse_contract::<OutputReadArgs>(name, value)?;
            require_identifier(name, "outputId", &args.output_id)?;
            if args
                .max_bytes
                .is_some_and(|size| size == 0 || size > MAX_INLINE_OUTPUT_BYTES)
            {
                return contract_error(name, "maxBytes must be between 1 and 65536");
            }
        }
        _ => return contract_error(name, "unknown NyaTerm MCP tool"),
    }
    Ok(())
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("serializable tool schema")
}

/// Canonical schemas shared by the MCP sidecar and the native agent providers.
pub fn tool_input_schema(name: &str) -> Result<Value, ToolContractError> {
    let mut result = match name {
        tool::GET_ENVIRONMENT | tool::CONNECTION_LIST => schema::<EmptyArgs>(),
        tool::SESSION_OPEN => schema::<SessionOpenArgs>(),
        tool::SESSION_GET => schema::<SessionArgs>(),
        tool::SFTP_HOME => schema::<FileTargetArgs>(),
        tool::TERMINAL_EXECUTE => schema::<TerminalExecuteArgs>(),
        tool::TERMINAL_START => schema::<TerminalStartArgs>(),
        tool::TERMINAL_POLL => schema::<TerminalPollArgs>(),
        tool::TERMINAL_STOP => schema::<TerminalStopArgs>(),
        tool::TERMINAL_RECENT_OUTPUT => schema::<TerminalRecentOutputArgs>(),
        tool::SFTP_LIST | tool::SFTP_STAT | tool::SFTP_DELETE => schema::<PathArgs>(),
        tool::SFTP_READ_TEXT => schema::<SftpReadTextArgs>(),
        tool::SFTP_WRITE_TEXT => schema::<SftpWriteTextArgs>(),
        tool::SFTP_MKDIR => schema::<SftpMkdirArgs>(),
        tool::SFTP_RENAME => schema::<SftpRenameArgs>(),
        tool::SFTP_CHMOD => schema::<SftpChmodArgs>(),
        tool::OUTPUT_READ => schema::<OutputReadArgs>(),
        _ => return contract_error(name, "unknown NyaTerm MCP tool"),
    };
    if supports_connection_target(name) {
        result["not"] = serde_json::json!({"required": ["target", "sessionId"]});
        if name != tool::TERMINAL_START {
            result["anyOf"] =
                serde_json::json!([{"required":["target"]},{"required":["sessionId"]}]);
        }
    }
    Ok(result)
}

/// Feature negotiation must change both advertised schemas and accepted arguments.
pub fn tool_input_schema_for_features(
    name: &str,
    connection_targets: bool,
) -> Result<Value, ToolContractError> {
    let mut result = tool_input_schema(name)?;
    if !connection_targets && supports_connection_target(name) {
        if name == tool::TERMINAL_START {
            result["properties"]["backend"] = serde_json::json!({
                "$ref": "#/$defs/TerminalBackend", "default": "inline_pty"
            });
        }
        result["properties"]
            .as_object_mut()
            .unwrap()
            .remove("target");
        result.as_object_mut().unwrap().remove("not");
        result.as_object_mut().unwrap().remove("anyOf");
        if name != tool::TERMINAL_START {
            let required = result
                .as_object_mut()
                .unwrap()
                .entry("required")
                .or_insert_with(|| serde_json::json!([]));
            required
                .as_array_mut()
                .unwrap()
                .push(Value::String("sessionId".into()));
        }
    }
    Ok(result)
}

fn union_schema(mut legacy: Value, mut connection: Value) -> Value {
    let mut definitions = serde_json::Map::new();
    for value in [&mut legacy, &mut connection] {
        if let Some(Value::Object(defs)) = value.as_object_mut().unwrap().remove("$defs") {
            definitions.extend(defs);
        }
    }
    serde_json::json!({"type":"object", "anyOf": [legacy, connection], "$defs": definitions})
}

pub fn terminal_job_result_schema_for_features(
    name: &str,
    connection_targets: bool,
) -> Option<Value> {
    if connection_targets {
        return terminal_job_result_schema(name);
    }
    match name {
        tool::TERMINAL_START => Some(schema::<TerminalStartResult>()),
        tool::TERMINAL_POLL => Some(schema::<TerminalPollResult>()),
        tool::TERMINAL_STOP => Some(schema::<TerminalStopResult>()),
        _ => None,
    }
}

/// Job results stay structured rather than using the legacy static-output envelope.
pub fn terminal_job_result_schema(name: &str) -> Option<Value> {
    match name {
        tool::TERMINAL_START => Some(union_schema(
            schema::<TerminalStartResult>(),
            schema::<ConnectionStartResult>(),
        )),
        tool::TERMINAL_POLL => Some(union_schema(
            schema::<TerminalPollResult>(),
            schema::<ConnectionPollResult>(),
        )),
        tool::TERMINAL_STOP => Some(union_schema(
            schema::<TerminalStopResult>(),
            schema::<ConnectionStopResult>(),
        )),
        _ => None,
    }
}

pub fn validate_tool_result(name: &str, value: &Value) -> Result<(), ToolContractError> {
    if name != tool::OUTPUT_READ
        && value.get("preview").is_some()
        && parse_contract::<ProtectedOutputResult>(name, value).is_ok()
    {
        return Ok(());
    }
    match name {
        tool::GET_ENVIRONMENT => parse_contract::<EnvironmentResult>(name, value).map(drop),
        tool::CONNECTION_LIST => parse_contract::<ConnectionListResult>(name, value).map(drop),
        tool::SESSION_OPEN => parse_contract::<SessionOpenResult>(name, value).map(drop),
        tool::SESSION_GET => parse_contract::<SessionGetResult>(name, value).map(drop),
        tool::TERMINAL_EXECUTE => parse_contract::<TerminalExecuteResult>(name, value).map(drop),
        tool::TERMINAL_START => {
            if value.get("target").is_some() {
                let result = parse_contract::<ConnectionStartResult>(name, value)?;
                if result.target.connection_id().is_none() {
                    return contract_error(name, "connection result requires a connection target");
                }
                Ok(())
            } else {
                parse_contract::<TerminalStartResult>(name, value).map(drop)
            }
        }
        tool::TERMINAL_POLL => {
            if value.get("target").is_some() {
                let result = parse_contract::<ConnectionPollResult>(name, value)?;
                if result.target.connection_id().is_none() {
                    return contract_error(name, "connection result requires a connection target");
                }
                Ok(())
            } else {
                parse_contract::<TerminalPollResult>(name, value).map(drop)
            }
        }
        tool::TERMINAL_STOP => {
            if value.get("target").is_some() {
                let result = parse_contract::<ConnectionStopResult>(name, value)?;
                if result.target.connection_id().is_none() {
                    return contract_error(name, "connection result requires a connection target");
                }
                Ok(())
            } else {
                parse_contract::<TerminalStopResult>(name, value).map(drop)
            }
        }
        tool::TERMINAL_RECENT_OUTPUT => {
            parse_contract::<TerminalRecentOutputResult>(name, value).map(drop)
        }
        tool::SFTP_HOME => parse_contract::<SftpHomeResult>(name, value).map(drop),
        tool::SFTP_LIST => parse_contract::<Vec<SftpFileEntry>>(name, value).map(drop),
        tool::SFTP_STAT => parse_contract::<SftpStatResult>(name, value).map(drop),
        tool::SFTP_READ_TEXT => parse_contract::<SftpReadTextResult>(name, value).map(drop),
        tool::SFTP_WRITE_TEXT => {
            let result = parse_contract::<SftpWriteTextResult>(name, value)?;
            if !matches!(
                result.status.as_str(),
                "saved" | "saved_with_backup" | "conflict"
            ) {
                return contract_error(
                    name,
                    "status must be saved, saved_with_backup, or conflict",
                );
            }
            if result.status == "saved_with_backup" && result.backup_path.is_none() {
                return contract_error(name, "saved_with_backup requires backupPath");
            }
            Ok(())
        }
        tool::SFTP_MKDIR => validate_mutation(name, value, "created"),
        tool::SFTP_RENAME => validate_mutation(name, value, "renamed"),
        tool::SFTP_DELETE => validate_mutation(name, value, "deleted"),
        tool::SFTP_CHMOD => validate_mutation(name, value, "changed"),
        tool::OUTPUT_READ => parse_contract::<OutputChunkResult>(name, value).map(drop),
        _ => contract_error(name, "unknown NyaTerm MCP tool"),
    }
}

impl RpcResponse {
    pub fn validate_envelope(&self) -> Result<(), &'static str> {
        match (&self.result, &self.error) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            (Some(_), Some(_)) => Err("RPC response contains both result and error"),
            (None, None) => Err("RPC response contains neither result nor error"),
        }
    }
}

fn parse_contract<T: serde::de::DeserializeOwned>(
    name: &str,
    value: &Value,
) -> Result<T, ToolContractError> {
    serde_json::from_value(value.clone()).map_err(|error| ToolContractError {
        tool: name.to_string(),
        message: error.to_string(),
    })
}

fn validate_target(
    name: &str,
    target: Option<&ExecutionTarget>,
    session_id: Option<&str>,
    required: bool,
) -> Result<(), ToolContractError> {
    if let Some(target) = target {
        if session_id.is_some_and(|id| !id.is_empty()) {
            return contract_error(name, "target and sessionId are mutually exclusive");
        }
        match target {
            ExecutionTarget::Connection { connection_id } => {
                require_identifier(name, "connectionId", connection_id)
            }
            ExecutionTarget::Session { session_id } => {
                require_identifier(name, "sessionId", session_id)
            }
        }
    } else if let Some(id) = session_id.filter(|id| !id.is_empty() || required) {
        require_identifier(name, "sessionId", id)
    } else if required {
        contract_error(name, "provide target or sessionId")
    } else {
        Ok(())
    }
}
/// Session targets normalize to the legacy spelling before authorization and hashing.
pub fn normalize_session_target(arguments: &mut Value) {
    if let Some(session_id) = arguments
        .get("target")
        .and_then(|value| serde_json::from_value::<ExecutionTarget>(value.clone()).ok())
        .and_then(|target| target.session_id().map(str::to_owned))
        && let Some(object) = arguments.as_object_mut()
    {
        object.remove("target");
        object.insert("sessionId".into(), Value::String(session_id));
    }
}
pub fn supports_connection_target(name: &str) -> bool {
    name == tool::TERMINAL_START
        || matches!(
            name,
            tool::SFTP_HOME
                | tool::SFTP_LIST
                | tool::SFTP_STAT
                | tool::SFTP_READ_TEXT
                | tool::SFTP_WRITE_TEXT
                | tool::SFTP_MKDIR
                | tool::SFTP_RENAME
                | tool::SFTP_DELETE
                | tool::SFTP_CHMOD
        )
}

#[cfg(test)]
mod connection_target_tests {
    use super::{
        ExecutionTarget, TerminalBackend, TerminalStartArgs, normalize_session_target,
        terminal_job_result_schema_for_features, tool, tool_input_schema_for_features,
        validate_tool_arguments, validate_tool_result,
    };
    use serde_json::json;

    #[test]
    fn saved_targets_default_to_exec_and_reject_ambiguous_or_interactive_requests() {
        let request =
            json!({"target":{"type":"connection","connectionId":"saved"},"command":"pwd"});
        assert!(validate_tool_arguments(tool::TERMINAL_START, &request).is_ok());
        let parsed: TerminalStartArgs = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(parsed.backend(), TerminalBackend::SshExec);
        assert_eq!(
            parsed.target,
            Some(ExecutionTarget::Connection {
                connection_id: "saved".into()
            })
        );
        for (field, value) in [
            ("sessionId", json!("session")),
            ("sessionId", json!(null)),
            ("backend", json!("inline_pty")),
        ] {
            let mut invalid = request.clone();
            invalid[field] = value;
            assert!(validate_tool_arguments(tool::TERMINAL_START, &invalid).is_err());
        }
        let mut session = json!({"target":{"type":"session","sessionId":"s"},"command":"pwd"});
        let parsed: TerminalStartArgs = serde_json::from_value(session.clone()).unwrap();
        assert_eq!(parsed.backend(), TerminalBackend::InlinePty);
        normalize_session_target(&mut session);
        assert_eq!(session, json!({"sessionId":"s","command":"pwd"}));
    }

    #[test]
    fn every_file_tool_accepts_both_targets_and_keeps_legacy_validation() {
        for (name, extra) in [
            (tool::SFTP_HOME, json!({})),
            (tool::SFTP_LIST, json!({"path":"/tmp"})),
            (tool::SFTP_STAT, json!({"path":"/tmp"})),
            (tool::SFTP_READ_TEXT, json!({"path":"/tmp/a"})),
            (
                tool::SFTP_WRITE_TEXT,
                json!({"path":"/tmp/a","content":"text"}),
            ),
            (tool::SFTP_MKDIR, json!({"path":"/tmp/a"})),
            (tool::SFTP_DELETE, json!({"path":"/tmp/a"})),
            (
                tool::SFTP_RENAME,
                json!({"oldPath":"/tmp/a","newPath":"/tmp/b"}),
            ),
            (tool::SFTP_CHMOD, json!({"path":"/tmp/a","mode":"600"})),
        ] {
            assert!(validate_tool_arguments(name, &extra).is_err(), "{name}");
            let mut legacy = extra.clone();
            legacy["sessionId"] = json!("s");
            assert!(validate_tool_arguments(name, &legacy).is_ok(), "{name}");
            for target in [
                json!({"type":"connection","connectionId":"c"}),
                json!({"type":"session","sessionId":"s"}),
            ] {
                let mut arguments = extra.clone();
                arguments["target"] = target;
                assert!(validate_tool_arguments(name, &arguments).is_ok(), "{name}");
                arguments["sessionId"] = json!("s");
                assert!(validate_tool_arguments(name, &arguments).is_err(), "{name}");
            }
            let mut invalid = extra;
            invalid["target"] = json!({"type":"connection","connectionId":""});
            assert!(validate_tool_arguments(name, &invalid).is_err(), "{name}");
        }
    }

    #[test]
    fn negotiated_schemas_and_results_never_fabricate_sessions() {
        let legacy_start = tool_input_schema_for_features(tool::TERMINAL_START, false).unwrap();
        assert_eq!(
            legacy_start["properties"]["backend"]["default"],
            "inline_pty"
        );
        assert!(legacy_start["properties"]["backend"].get("anyOf").is_none());
        let old = tool_input_schema_for_features(tool::SFTP_LIST, false).unwrap();
        assert!(old["properties"].get("target").is_none());
        assert!(
            old["required"]
                .as_array()
                .unwrap()
                .contains(&json!("sessionId"))
        );
        let new = tool_input_schema_for_features(tool::SFTP_LIST, true).unwrap();
        assert!(new["properties"].get("target").is_some());
        let old = terminal_job_result_schema_for_features(tool::TERMINAL_START, false).unwrap();
        assert!(old.get("anyOf").is_none());
        let new = terminal_job_result_schema_for_features(tool::TERMINAL_START, true).unwrap();
        assert_eq!(new["type"], "object");
        assert!(new["$defs"].get("ExecutionTarget").is_some());
        let mut result = json!({"jobId":"job","target":{"type":"connection","connectionId":"c"},"backend":"ssh_exec","status":"starting"});
        assert!(validate_tool_result(tool::TERMINAL_START, &result).is_ok());
        result["sessionId"] = json!("fake");
        assert!(validate_tool_result(tool::TERMINAL_START, &result).is_err());
        let old = json!({"jobId":"job","sessionId":"s","backend":"ssh_exec","status":"starting"});
        assert!(validate_tool_result(tool::TERMINAL_START, &old).is_ok());
    }
}

fn require_identifier(name: &str, field: &str, value: &str) -> Result<(), ToolContractError> {
    if value.trim().is_empty() || value.len() > 256 {
        return contract_error(name, &format!("{field} must contain 1 to 256 bytes"));
    }
    Ok(())
}

fn require_path(name: &str, field: &str, value: &str) -> Result<(), ToolContractError> {
    if value.trim().is_empty() || value.len() > 4096 || value.contains('\0') {
        return contract_error(
            name,
            &format!("{field} is empty, too long, or contains NUL"),
        );
    }
    Ok(())
}

fn validate_mode(name: &str, mode: &str) -> Result<(), ToolContractError> {
    let value = mode.trim().strip_prefix("0o").unwrap_or(mode.trim());
    if !(3..=4).contains(&value.len()) || u16::from_str_radix(value, 8).is_err() {
        return contract_error(name, "mode must be a 3 or 4 digit octal value");
    }
    Ok(())
}

fn validate_mutation(name: &str, value: &Value, expected: &str) -> Result<(), ToolContractError> {
    let result = parse_contract::<MutationResult>(name, value)?;
    let fields = [
        ("created", result.created),
        ("renamed", result.renamed),
        ("deleted", result.deleted),
        ("changed", result.changed),
    ];
    if fields
        .iter()
        .any(|(field, value)| *field == expected && *value == Some(true))
        && fields.iter().filter(|(_, value)| value.is_some()).count() == 1
    {
        Ok(())
    } else {
        contract_error(name, &format!("result must contain only {expected}=true"))
    }
}

fn contract_error<T>(name: &str, message: &str) -> Result<T, ToolContractError> {
    Err(ToolContractError {
        tool: name.to_string(),
        message: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        CapabilityAccess, MCP_TOOL_REGISTRY, definition_for_capability, definition_for_tool,
    };

    #[test]
    fn registry_is_unique_and_annotations_match_access() {
        let mut tools = HashSet::new();
        let mut capabilities = HashSet::new();
        for definition in MCP_TOOL_REGISTRY {
            assert!(
                tools.insert(definition.tool),
                "duplicate tool: {}",
                definition.tool
            );
            assert!(
                capabilities.insert(definition.capability),
                "duplicate capability: {}",
                definition.capability
            );
            assert_eq!(
                definition.read_only_hint,
                matches!(
                    definition.access,
                    CapabilityAccess::Read | CapabilityAccess::SensitiveRead
                )
            );
            assert_eq!(
                definition.destructive_hint,
                definition.access == CapabilityAccess::DestructiveWrite
            );
            if definition.access == CapabilityAccess::DestructiveWrite {
                assert!(definition.destructive_hint);
            }
            assert_eq!(definition_for_tool(definition.tool), Some(definition));
            assert_eq!(
                definition_for_capability(definition.capability),
                Some(definition)
            );
        }
    }
}

#[cfg(test)]
mod contract_tests {
    use super::{
        MAX_INLINE_OUTPUT_BYTES, MAX_RPC_LINE_BYTES, MAX_TEXT_READ_BYTES, MAX_TEXT_WRITE_BYTES,
        MCP_TOOL_REGISTRY, RpcError, RpcResponse, SftpWriteTextResult, tool,
        validate_tool_arguments, validate_tool_result,
    };

    #[test]
    fn write_text_result_accepts_older_payloads_and_reports_retained_backups() {
        let old = serde_json::json!({
            "status": "saved",
            "mtime": 1,
            "size": 3,
            "contentHash": "abc"
        });
        let parsed: SftpWriteTextResult = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(parsed.backup_path, None);
        assert_eq!(serde_json::to_value(parsed).unwrap(), old);

        let warning = SftpWriteTextResult {
            status: "saved_with_backup".into(),
            mtime: Some(2),
            size: Some(3),
            mtime_nanos: None,
            content_hash: Some("abc".into()),
            backup_path: Some("/file.txt.nyaterm-backup-1".into()),
        };
        assert_eq!(
            serde_json::to_value(warning).unwrap()["backupPath"],
            "/file.txt.nyaterm-backup-1"
        );
        assert!(
            validate_tool_result(
                tool::SFTP_WRITE_TEXT,
                &serde_json::json!({
                    "status": "saved_with_backup",
                    "mtime": 2,
                    "size": 3,
                    "backupPath": "/file.txt.nyaterm-backup-1"
                })
            )
            .is_ok()
        );
    }

    #[test]
    fn registry_contains_legacy_tools_and_three_job_tools_with_limits() {
        assert_eq!(MCP_TOOL_REGISTRY.len(), 19);
        assert_eq!(
            MCP_TOOL_REGISTRY.first().unwrap().tool,
            tool::GET_ENVIRONMENT
        );
        assert_eq!(MCP_TOOL_REGISTRY.last().unwrap().tool, tool::OUTPUT_READ);
        assert_eq!(MAX_RPC_LINE_BYTES, 2 * 1024 * 1024);
        assert_eq!(MAX_INLINE_OUTPUT_BYTES, 64 * 1024);
        assert_eq!(MAX_TEXT_READ_BYTES, 64 * 1024);
        assert_eq!(MAX_TEXT_WRITE_BYTES, 1024 * 1024);
    }

    #[test]
    fn tool_arguments_reject_unknown_fields_and_size_violations() {
        assert!(validate_tool_arguments(tool::GET_ENVIRONMENT, &serde_json::json!({})).is_ok());
        assert!(
            validate_tool_arguments(
                tool::GET_ENVIRONMENT,
                &serde_json::json!({ "unexpected": true })
            )
            .is_err()
        );
        assert!(
            validate_tool_arguments(
                tool::SFTP_READ_TEXT,
                &serde_json::json!({
                    "sessionId": "session-1",
                    "path": "/tmp/a",
                    "maxBytes": MAX_TEXT_READ_BYTES + 1
                })
            )
            .is_err()
        );
        assert!(
            validate_tool_arguments(
                tool::SFTP_WRITE_TEXT,
                &serde_json::json!({
                    "sessionId": "session-1",
                    "path": "/tmp/a",
                    "content": "x".repeat(MAX_TEXT_WRITE_BYTES + 1)
                })
            )
            .is_err()
        );
        assert!(
            validate_tool_arguments(
                tool::SFTP_WRITE_TEXT,
                &serde_json::json!({
                    "sessionId": "session-1",
                    "path": "/tmp/a",
                    "content": "fixture",
                    "expectedHash": "not-a-sha256"
                })
            )
            .is_err()
        );
    }

    #[test]
    fn tool_results_and_rpc_envelopes_are_strict() {
        assert!(
            validate_tool_result(
                tool::TERMINAL_EXECUTE,
                &serde_json::json!({
                    "output": "ok",
                    "exitCode": 0,
                    "durationMs": 1,
                    "timedOut": false,
                    "sourceTruncated": false
                })
            )
            .is_ok()
        );
        assert!(
            validate_tool_result(
                tool::TERMINAL_EXECUTE,
                &serde_json::json!({
                    "output": "ok",
                    "exitCode": 0,
                    "durationMs": 1,
                    "timedOut": false,
                    "sourceTruncated": false,
                    "unexpected": true
                })
            )
            .is_err()
        );

        let empty = RpcResponse {
            id: 1,
            result: None,
            error: None,
        };
        assert!(empty.validate_envelope().is_err());
        let both = RpcResponse {
            id: 1,
            result: Some(serde_json::json!({})),
            error: Some(RpcError {
                code: "x".into(),
                message: "x".into(),
            }),
        };
        assert!(both.validate_envelope().is_err());
    }
}

fn validate_idempotency_key(name: &str, key: Option<&str>) -> Result<(), ToolContractError> {
    if let Some(key) = key {
        require_identifier(name, "idempotencyKey", key)?;
    }
    Ok(())
}

#[cfg(test)]
mod lifecycle_contract_tests {
    use super::{
        SessionOpenArgs, SessionOpenMode, TerminalBackend, TerminalExecuteArgs,
        TerminalExecuteResult, TerminalStartArgs, tool, validate_tool_arguments,
    };
    use serde_json::json;
    #[test]
    fn omitted_new_parameters_preserve_legacy_open_execute_and_result_shapes() {
        let open: SessionOpenArgs =
            serde_json::from_value(json!({"connectionId":"saved"})).unwrap();
        assert_eq!(open.mode, SessionOpenMode::Create);
        assert!(open.idempotency_key.is_none());
        let execute: TerminalExecuteArgs =
            serde_json::from_value(json!({"command":"pwd"})).unwrap();
        assert!(!execute.return_job);
        let result:TerminalExecuteResult=serde_json::from_value(json!({"output":"ok","exitCode":0,"durationMs":1,"timedOut":false,"sourceTruncated":false})).unwrap();
        assert!(serde_json::to_value(result).unwrap().get("jobId").is_none());
        let start: TerminalStartArgs = serde_json::from_value(json!({"command":"pwd"})).unwrap();
        assert_eq!(start.backend(), TerminalBackend::InlinePty);
    }
    #[test]
    fn long_jobs_validate_timeouts_backends_keys_and_utf8_page_budgets() {
        for timeout in [1000, 300000, 86400000] {
            assert!(validate_tool_arguments(tool::TERMINAL_START,&json!({"command":"echo 中文","timeoutMs":timeout,"backend":"ssh_exec","idempotencyKey":"key"})).is_ok());
        }
        for timeout in [999, 86400001] {
            assert!(
                validate_tool_arguments(
                    tool::TERMINAL_START,
                    &json!({"command":"pwd","timeoutMs":timeout})
                )
                .is_err()
            );
        }
        assert!(
            validate_tool_arguments(
                tool::TERMINAL_START,
                &json!({"command":"pwd","backend":"other"})
            )
            .is_err()
        );
        assert!(
            validate_tool_arguments(tool::TERMINAL_START, &json!({"command":"bad\0command"}))
                .is_err()
        );
        assert!(
            validate_tool_arguments(
                tool::TERMINAL_START,
                &json!({"command":"pwd","idempotencyKey":""})
            )
            .is_err()
        );
        for bytes in [4, 16384, 65536] {
            assert!(
                validate_tool_arguments(
                    tool::TERMINAL_POLL,
                    &json!({"jobId":"job","maxBytes":bytes})
                )
                .is_ok()
            );
        }
        for bytes in [3, 65537] {
            assert!(
                validate_tool_arguments(
                    tool::TERMINAL_POLL,
                    &json!({"jobId":"job","maxBytes":bytes})
                )
                .is_err()
            );
        }
        assert!(
            validate_tool_arguments(tool::TERMINAL_STOP, &json!({"jobId":"job","force":true}))
                .is_err()
        );
    }
}
