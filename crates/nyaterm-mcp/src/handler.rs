use std::sync::Arc;

use crate::bridge::{BridgeClient, BridgeEndpoint};
use crate::terminal_start;
use nyaterm_mcp_protocol::{
    MCP_TOOL_REGISTRY, tool, validate_tool_arguments, validate_tool_result,
};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Map, Value, json};

#[derive(Clone)]
pub(crate) struct NyaTermMcp {
    pub(crate) bridge: BridgeClient,
    pub(crate) tools: Arc<Vec<Tool>>,
}

impl NyaTermMcp {
    pub(crate) fn with_discovery_path(mut self, path: std::path::PathBuf) -> Self {
        self.bridge = self.bridge.with_discovery_path(path);
        self
    }

    pub(crate) fn new(endpoint: BridgeEndpoint) -> Self {
        Self {
            bridge: BridgeClient::new(endpoint),
            tools: Arc::new(build_tools()),
        }
    }
}

impl ServerHandler for NyaTermMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "nyaterm-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("Use connection_list to discover saved hosts. When target parameters are advertised, prefer terminal_start with a connection target for noninteractive SSH commands and SFTP tools with the same target for file operations. Open a session only for an interactive terminal. NyaTerm enforces permissions and approvals.")
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if !self.tools.iter().any(|item| item.name == request.name) {
            return Err(McpError::invalid_params("Unknown NyaTerm tool", None));
        }
        // The feature guards below read connection-negotiated state and must
        // not race the first bridge connection (the same race tools/list had);
        // await the bounded negotiation first. Inside bridge.call this is a
        // no-op once connected.
        self.bridge.ensure_connected().await;
        if matches!(
            request.name.as_ref(),
            tool::TERMINAL_START | tool::TERMINAL_POLL | tool::TERMINAL_STOP
        ) && !self.bridge.supports_lifecycle()
        {
            return Ok(CallToolResult::structured_error(json!({"code":"unsupported_capability","message":"The connected NyaTerm Host does not support terminal jobs."})).into());
        }
        let mut arguments = Value::Object(request.arguments.unwrap_or_default());
        if arguments.get("target").is_some() && !self.bridge.supports_connection_targets() {
            return Ok(CallToolResult::structured_error(json!({"code":"unsupported_capability","message":"The connected NyaTerm Host does not support execution targets."})).into());
        }
        let wait = terminal_start::StartWait::take(&request.name, &mut arguments)
            .map_err(|message| McpError::invalid_params(message, None))?;
        validate_tool_arguments(&request.name, &arguments)
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let result = match self
            .bridge
            .call(&request.name, arguments, context.ct.clone())
            .await
        {
            Ok(value) => match validate_tool_result(&request.name, &value) {
                Ok(()) => {
                    // Start and poll have different wire contracts. Validate each
                    // response before allowing it to replace the last good result.
                    let value = if request.name == tool::TERMINAL_START {
                        wait.wait_for_result(&self.bridge, value, context.ct.clone())
                            .await
                    } else {
                        value
                    };
                    CallToolResult::structured(structured_content(&request.name, value))
                }
                Err(error) => CallToolResult::structured_error(json!({
                    "code": "bridge_protocol_error",
                    "message": error.to_string()
                })),
            },
            Err(error) => CallToolResult::structured_error(
                json!({ "code": error.code, "message": error.message }),
            ),
        };
        Ok(result.into())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // The bridge connects lazily, so the initial tools/list would race
        // on_initialized's identify() and report the degraded tool set. Await the
        // bounded negotiation; an unreachable host keeps serving it as before.
        self.bridge.ensure_connected().await;
        // MCP 2026-07-28 requires `ttlMs` and `cacheScope` on list results, and strict
        // clients reject `tools/list` without them. The tool set is per bridge session,
        // so the result is private and not cached.
        Ok(ListToolsResult::with_all_items(
            self.tools
                .iter()
                .filter_map(|item| self.get_tool(&item.name))
                .collect(),
        )
        .with_ttl_ms(0)
        .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        let mut item = self.tools.iter().find(|item| item.name == name)?.clone();
        let targets = self.bridge.supports_connection_targets();
        item.input_schema = Arc::new(
            terminal_start::input_schema(name, targets)
                .ok()?
                .as_object()?
                .clone(),
        );
        item.output_schema = terminal_start::result_schema(name, targets)
            .and_then(|schema| schema.as_object().cloned())
            .map(Arc::new);
        if !targets && name == tool::TERMINAL_START {
            item.description = Some("Start a job in an existing scoped terminal session. Defaults to inline_pty; ssh_exec uses an independent channel. Use an idempotencyKey for retries.".into());
        }
        if name == tool::TERMINAL_START {
            let description = item.description.get_or_insert_with(Default::default);
            *description = format!("{description} {}", terminal_start::WAIT_DESCRIPTION).into();
        }
        if !self.bridge.supports_lifecycle() {
            if matches!(
                name,
                tool::TERMINAL_START | tool::TERMINAL_POLL | tool::TERMINAL_STOP
            ) {
                return None;
            }
            let mut schema = (*item.input_schema).clone();
            if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                match name {
                    tool::SESSION_OPEN => {
                        properties.remove("mode");
                        properties.remove("idempotencyKey");
                        item.description = Some(
                            "Open a saved terminal connection as a new NyaTerm session.".into(),
                        );
                    }
                    tool::TERMINAL_EXECUTE => {
                        properties.remove("returnJob");
                        properties.remove("idempotencyKey");
                    }
                    _ => {}
                }
            }
            item.input_schema = std::sync::Arc::new(schema);
        }
        Some(item)
    }

    async fn on_initialized(&self, context: rmcp::service::NotificationContext<RoleServer>) {
        if let Some(info) = context.peer.peer_info() {
            self.bridge
                .identify(
                    info.client_info.name.to_string(),
                    Some(info.client_info.version.to_string()),
                )
                .await;
        }
    }
}

/// MCP requires `structuredContent` to be an object. `sftp_list` returns a bare array of
/// entries over the bridge, so it is wrapped as `{ "items": [...] }`; other results pass
/// through unchanged.
pub(crate) fn structured_content(tool_name: &str, value: Value) -> Value {
    match value {
        Value::Array(items) if tool_name == tool::SFTP_LIST => json!({ "items": items }),
        other => other,
    }
}

pub(crate) fn build_tools() -> Vec<Tool> {
    MCP_TOOL_REGISTRY
        .iter()
        .map(|definition| {
            let schema = terminal_start::input_schema(definition.tool, true)
                .expect("registered tool schema");
            let object = schema.as_object().cloned().unwrap_or_else(Map::new);
            let mut item = Tool::new(definition.tool, definition.description, object);
            item.output_schema = terminal_start::result_schema(definition.tool, true)
                .and_then(|schema| schema.as_object().cloned())
                .map(std::sync::Arc::new);
            item.annotations = Some(
                ToolAnnotations::new()
                    .read_only(definition.read_only_hint)
                    .destructive(definition.destructive_hint)
                    .open_world(definition.open_world_hint),
            );
            item
        })
        .collect()
}
