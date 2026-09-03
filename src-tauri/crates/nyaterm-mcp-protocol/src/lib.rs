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
        description: "Open a new NyaTerm terminal session from a saved connection.",
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
#[serde(rename_all = "camelCase")]
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
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthParams {
    pub token: String,
    pub generation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIdentifyParams {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityExecuteParams {
    #[serde(default)]
    pub request_id: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmptyArgs {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionArgs {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionOpenArgs {
    pub connection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathArgs {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalExecuteArgs {
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
    pub session_id: String,
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpWriteTextArgs {
    pub session_id: String,
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
    pub session_id: String,
    pub path: String,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpRenameArgs {
    pub session_id: String,
    pub old_path: String,
    pub new_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpChmodArgs {
    pub session_id: String,
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn registry_is_unique_and_annotations_match_access() {
        let mut tools = HashSet::new();
        let mut capabilities = HashSet::new();
        for definition in MCP_TOOL_REGISTRY {
            assert!(tools.insert(definition.tool), "duplicate tool: {}", definition.tool);
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
