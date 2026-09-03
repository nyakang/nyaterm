pub use nyaterm_mcp_protocol::CapabilityAccess;
use nyaterm_mcp_protocol::{McpToolDefinition, definition_for_tool};

pub fn capability_for_tool(name: &str) -> Option<&'static McpToolDefinition> {
    definition_for_tool(name)
}

#[cfg(test)]
mod tests {
    use nyaterm_mcp_protocol::{CapabilityAccess, MCP_TOOL_REGISTRY, capability, tool};

    use super::*;

    #[test]
    fn every_registered_tool_has_a_host_capability() {
        for definition in MCP_TOOL_REGISTRY {
            assert_eq!(capability_for_tool(definition.tool), Some(definition));
        }
        assert_eq!(
            capability_for_tool(tool::SFTP_DELETE).unwrap().capability,
            capability::SFTP_DELETE
        );
        assert_eq!(
            capability_for_tool(tool::SFTP_DELETE).unwrap().access,
            CapabilityAccess::DestructiveWrite
        );
    }
}
