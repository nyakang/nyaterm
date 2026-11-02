mod bridge;

use std::sync::Arc;

use bridge::{BridgeClient, BridgeEndpoint, endpoint_from_environment_or_discovery};
use nyaterm_mcp_protocol::{
    MCP_TOOL_REGISTRY, tool, validate_tool_arguments, validate_tool_result,
};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

#[derive(Clone)]
struct NyaTermMcp {
    bridge: BridgeClient,
    tools: Arc<Vec<Tool>>,
}

impl NyaTermMcp {
    fn new(endpoint: BridgeEndpoint) -> Self {
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
            .with_instructions("Discover saved terminal connections, open sessions in NyaTerm, and operate scoped sessions. NyaTerm enforces session scope and approvals.")
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if !self.tools.iter().any(|item| item.name == request.name) {
            return Err(McpError::invalid_params("Unknown NyaTerm tool", None));
        }
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        validate_tool_arguments(&request.name, &arguments)
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let result = match self
            .bridge
            .call(&request.name, arguments, context.ct.clone())
            .await
        {
            Ok(value) => match validate_tool_result(&request.name, &value) {
                Ok(()) => CallToolResult::structured(structured_content(&request.name, value)),
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
        // MCP 2026-07-28 requires `ttlMs` and `cacheScope` on list results, and strict
        // clients reject `tools/list` without them. The tool set is per bridge session,
        // so the result is private and not cached.
        Ok(ListToolsResult::with_all_items(self.tools.as_ref().clone())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|item| item.name == name).cloned()
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
fn structured_content(tool_name: &str, value: Value) -> Value {
    match value {
        Value::Array(items) if tool_name == tool::SFTP_LIST => json!({ "items": items }),
        other => other,
    }
}

fn build_tools() -> Vec<Tool> {
    MCP_TOOL_REGISTRY
        .iter()
        .map(|definition| {
            let schema = nyaterm_mcp_protocol::tool_input_schema(definition.tool)
                .expect("registered tool schema");
            let object = schema.as_object().cloned().unwrap_or_else(Map::new);
            let mut item = Tool::new(definition.tool, definition.description, object);
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = endpoint_from_environment_or_discovery()?;
    let server = NyaTermMcp::new(endpoint)
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use nyaterm_mcp_protocol::{MCP_TOOL_REGISTRY, RpcRequest, RpcResponse, tool};
    use rmcp::ServiceExt;
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines, ReadHalf, WriteHalf},
        net::TcpListener,
    };

    use super::{BridgeEndpoint, NyaTermMcp, build_tools, structured_content};

    #[test]
    fn sftp_list_structured_content_is_an_object() {
        assert_eq!(
            structured_content(tool::SFTP_LIST, json!([{ "name": "a" }, { "name": "b" }])),
            json!({ "items": [{ "name": "a" }, { "name": "b" }] })
        );
        // An empty directory still gives an object.
        assert_eq!(
            structured_content(tool::SFTP_LIST, json!([])),
            json!({ "items": [] })
        );
    }

    #[test]
    fn other_tool_results_pass_through_unchanged() {
        let home = json!({ "path": "/home/user" });
        assert_eq!(structured_content(tool::SFTP_HOME, home.clone()), home);
        let stat = json!({ "name": "a", "size": 1 });
        assert_eq!(structured_content(tool::SFTP_STAT, stat.clone()), stat);
    }

    #[test]
    fn listed_tool_annotations_come_from_the_shared_registry() {
        let tools = build_tools();
        assert_eq!(tools.len(), MCP_TOOL_REGISTRY.len());
        for definition in MCP_TOOL_REGISTRY {
            let tool = tools
                .iter()
                .find(|tool| tool.name == definition.tool)
                .unwrap();
            let value = serde_json::to_value(tool).unwrap();
            assert_eq!(
                value["annotations"]["readOnlyHint"],
                definition.read_only_hint
            );
            assert_eq!(
                value["annotations"]["destructiveHint"],
                definition.destructive_hint
            );
            assert_eq!(
                value["annotations"]["openWorldHint"],
                definition.open_world_hint
            );
        }

        let connection_list = tools
            .iter()
            .find(|item| item.name == tool::CONNECTION_LIST)
            .expect("connection_list tool");
        let connection_list = serde_json::to_value(connection_list).unwrap();
        assert_eq!(connection_list["inputSchema"]["type"], "object");
        assert_eq!(connection_list["annotations"]["readOnlyHint"], true);

        let session_open = tools
            .iter()
            .find(|item| item.name == tool::SESSION_OPEN)
            .expect("session_open tool");
        let session_open = serde_json::to_value(session_open).unwrap();
        assert_eq!(
            session_open["inputSchema"]["required"],
            json!(["connectionId"])
        );
        assert_eq!(
            session_open["inputSchema"]["properties"]["connectionId"]["type"],
            "string"
        );
        assert_eq!(session_open["annotations"]["readOnlyHint"], false);
    }

    async fn send_client_message(writer: &mut WriteHalf<tokio::io::DuplexStream>, raw: &str) {
        let value = serde_json::from_str::<Value>(raw).expect("valid MCP client message");
        writer
            .write_all(&serde_json::to_vec(&value).unwrap())
            .await
            .unwrap();
        writer.write_all(b"\n").await.unwrap();
    }

    async fn receive_response(
        lines: &mut Lines<BufReader<ReadHalf<tokio::io::DuplexStream>>>,
        id: u64,
    ) -> Value {
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for MCP response {id}"))
                .expect("server response")
                .expect("server transport remains open");
            let value: Value = serde_json::from_str(&line).expect("valid server response");
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    #[tokio::test]
    async fn mcp_initialize_lists_tools_and_calls_mock_bridge() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: RpcRequest = serde_json::from_str(&line).unwrap();
                let result = match request.method.as_str() {
                    "auth" => {
                        assert_eq!(request.params["token"], "test-token");
                        assert_eq!(request.params["generation"], "test-generation");
                        json!({ "authenticated": true })
                    }
                    "client.identify" => {
                        assert_eq!(request.params["name"], "integration-test");
                        json!({ "identified": true })
                    }
                    "capability.execute" => match request.params["tool"].as_str().unwrap() {
                        tool::GET_ENVIRONMENT => {
                            json!({ "defaultSessionId": "session-1", "sessions": [] })
                        }
                        tool::CONNECTION_LIST => json!({
                            "connections": [{
                                "id": "connection-1",
                                "name": "Local shell",
                                "type": "local_terminal",
                                "groupPath": []
                            }]
                        }),
                        tool::SESSION_OPEN => {
                            assert_eq!(request.params["arguments"]["connectionId"], "connection-1");
                            json!({
                                "sessionId": "session-2",
                                "connectionId": "connection-1",
                                "name": "Local shell",
                                "type": "local",
                                "connected": true
                            })
                        }
                        tool::SFTP_LIST => json!([{
                            "name": "notes.txt",
                            "is_dir": false,
                            "is_symlink": false,
                            "size": 5,
                            "permissions": "rw-r--r--",
                            "owner": "user",
                            "group": "user",
                            "mtime": 0
                        }]),
                        other => panic!("unexpected tool: {other}"),
                    },
                    other => panic!("unexpected bridge method: {other}"),
                };
                let response = RpcResponse {
                    id: request.id,
                    result: Some(result),
                    error: None,
                };
                let mut bytes = serde_json::to_vec(&response).unwrap();
                bytes.push(b'\n');
                writer.write_all(&bytes).await.unwrap();
            }
        });

        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let service = NyaTermMcp::new(BridgeEndpoint::for_test(port))
                .serve(server_transport)
                .await
                .unwrap();
            service.waiting().await.unwrap();
        });
        let (client_reader, mut client_writer) = tokio::io::split(client_transport);
        let mut client_lines = BufReader::new(client_reader).lines();

        send_client_message(
            &mut client_writer,
            r#"{
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": { "name": "integration-test", "version": "1.0.0" }
                    }
                }"#,
        )
        .await;
        let initialized = receive_response(&mut client_lines, 1).await;
        assert_eq!(initialized["result"]["serverInfo"]["name"], "nyaterm-mcp");

        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "method": "notifications/initialized" }"#,
        )
        .await;
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }"#,
        )
        .await;
        let listed = receive_response(&mut client_lines, 2).await;
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 16);
        assert_eq!(listed["result"]["ttlMs"], 0);
        assert_eq!(listed["result"]["cacheScope"], "private");

        send_client_message(
            &mut client_writer,
            r#"{
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": { "name": "get_environment", "arguments": {} }
                }"#,
        )
        .await;
        let called = receive_response(&mut client_lines, 3).await;
        assert_eq!(
            called["result"]["structuredContent"]["defaultSessionId"],
            "session-1"
        );

        send_client_message(
            &mut client_writer,
            r#"{
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": { "name": "connection_list", "arguments": {} }
                }"#,
        )
        .await;
        let connections = receive_response(&mut client_lines, 4).await;
        assert_eq!(
            connections["result"]["structuredContent"]["connections"][0]["id"],
            "connection-1"
        );

        send_client_message(
            &mut client_writer,
            r#"{
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "tools/call",
                    "params": {
                        "name": "session_open",
                        "arguments": { "connectionId": "connection-1" }
                    }
                }"#,
        )
        .await;
        let opened = receive_response(&mut client_lines, 5).await;
        assert_eq!(
            opened["result"]["structuredContent"],
            json!({
                "sessionId": "session-2",
                "connectionId": "connection-1",
                "name": "Local shell",
                "type": "local",
                "connected": true
            })
        );

        // MCP requires structuredContent to be an object (#1002): the bridge's entry array
        // is returned under `items`.
        send_client_message(
            &mut client_writer,
            r#"{
                    "jsonrpc": "2.0",
                    "id": 6,
                    "method": "tools/call",
                    "params": {
                        "name": "sftp_list",
                        "arguments": { "sessionId": "session-2", "path": "/home/user" }
                    }
                }"#,
        )
        .await;
        let listed = receive_response(&mut client_lines, 6).await;
        let structured = &listed["result"]["structuredContent"];
        assert!(
            structured.is_object(),
            "structuredContent must be an object: {structured}"
        );
        assert_eq!(structured["items"][0]["name"], "notes.txt");
        assert_eq!(structured["items"].as_array().map(Vec::len), Some(1));

        drop(client_writer);
        drop(client_lines);
        server_task.abort();
        bridge_task.abort();
    }
}
