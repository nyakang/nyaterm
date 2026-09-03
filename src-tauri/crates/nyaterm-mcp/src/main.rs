mod bridge;

use std::sync::Arc;

use bridge::{BridgeClient, BridgeEndpoint, endpoint_from_environment_or_discovery};
use nyaterm_mcp_protocol::{
    EmptyArgs, MCP_TOOL_REGISTRY, McpToolDefinition, OutputReadArgs, PathArgs, SessionArgs,
    SftpChmodArgs, SftpMkdirArgs, SftpReadTextArgs, SftpRenameArgs, SftpWriteTextArgs,
    SessionOpenArgs, TerminalExecuteArgs, TerminalRecentOutputArgs, tool,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use schemars::JsonSchema;
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
        let result = match self
            .bridge
            .call(&request.name, arguments, context.ct.clone())
            .await
        {
            Ok(value) => CallToolResult::structured(value),
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
        Ok(ListToolsResult::with_all_items(self.tools.as_ref().clone()))
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

fn build_tools() -> Vec<Tool> {
    MCP_TOOL_REGISTRY
        .iter()
        .map(|definition| match definition.tool {
            tool::GET_ENVIRONMENT => tool_def::<EmptyArgs>(definition),
            tool::CONNECTION_LIST => tool_def::<EmptyArgs>(definition),
            tool::SESSION_OPEN => tool_def::<SessionOpenArgs>(definition),
            tool::SESSION_GET | tool::SFTP_HOME => tool_def::<SessionArgs>(definition),
            tool::TERMINAL_EXECUTE => tool_def::<TerminalExecuteArgs>(definition),
            tool::TERMINAL_RECENT_OUTPUT => tool_def::<TerminalRecentOutputArgs>(definition),
            tool::SFTP_LIST | tool::SFTP_STAT | tool::SFTP_DELETE => {
                tool_def::<PathArgs>(definition)
            }
            tool::SFTP_READ_TEXT => tool_def::<SftpReadTextArgs>(definition),
            tool::SFTP_WRITE_TEXT => tool_def::<SftpWriteTextArgs>(definition),
            tool::SFTP_MKDIR => tool_def::<SftpMkdirArgs>(definition),
            tool::SFTP_RENAME => tool_def::<SftpRenameArgs>(definition),
            tool::SFTP_CHMOD => tool_def::<SftpChmodArgs>(definition),
            tool::OUTPUT_READ => tool_def::<OutputReadArgs>(definition),
            _ => unreachable!("registry contains an unknown MCP tool"),
        })
        .collect()
}

fn tool_def<T: JsonSchema>(definition: &McpToolDefinition) -> Tool {
    let schema = serde_json::to_value(schemars::schema_for!(T))
        .unwrap_or_else(|_| json!({ "type": "object" }));
    let object = schema.as_object().cloned().unwrap_or_else(Map::new);
    let mut item = Tool::new(definition.tool, definition.description, object);
    item.annotations = Some(
        ToolAnnotations::new()
            .read_only(definition.read_only_hint)
            .destructive(definition.destructive_hint)
            .open_world(definition.open_world_hint),
    );
    item
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
    use nyaterm_mcp_protocol::{MCP_TOOL_REGISTRY, RpcRequest, RpcResponse};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines, ReadHalf, WriteHalf},
        net::TcpListener,
    };

    use super::*;

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
                            assert_eq!(
                                request.params["arguments"]["connectionId"],
                                "connection-1"
                            );
                            json!({
                                "sessionId": "session-2",
                                "connectionId": "connection-1",
                                "name": "Local shell",
                                "type": "local",
                                "connected": true
                            })
                        }
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

        drop(client_writer);
        drop(client_lines);
        server_task.abort();
        bridge_task.abort();
    }
}
